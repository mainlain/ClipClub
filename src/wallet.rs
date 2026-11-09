//! Password-encrypted, local Ethereum wallet. No plaintext key export is exposed.
//!
//! The owner must serialize access to this vault. Password derivation is blocking
//! and should run outside an async executor worker when called by the local API.

use alloy_consensus::{
    SignableTransaction, Transaction, TxLegacy, transaction::RlpEcdsaDecodableTx,
};
use alloy_primitives::{Address, TxKind, U256};
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;
use anyhow::{Context, Result, bail, ensure};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use rand::{TryRngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

const MAX_KEYSTORE_BYTES: u64 = 16 * 1024;
const MEMORY_KIB: u32 = 64 * 1024;
const ITERATIONS: u32 = 3;
const PARALLELISM: u32 = 1;
const CIPHER: &str = "xchacha20poly1305";
const KDF: &str = "argon2id";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EncryptedWallet {
    version: u8,
    address: String,
    crypto: Encryption,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Encryption {
    cipher: String,
    kdf: String,
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
    salt: String,
    nonce: String,
    ciphertext: String,
}

/// Signed EIP-155 transaction, ready for `eth_sendRawTransaction`.
pub struct SignedTransaction {
    pub raw: String,
    pub hash: String,
}

/// Verify the journal's signed transaction before a retry can rebroadcast it.
/// The original contract call is kept byte-for-byte; only a canonical, signed,
/// zero-ETH call from the selected wallet on the configured chain is accepted.
pub fn validate_signed_transaction(
    raw: &str,
    hash: &str,
    chain_id: u64,
    wallet: &str,
    contract: &str,
) -> Result<()> {
    ensure!(
        raw.len() <= 1_048_576,
        "Saved signed transaction is too large"
    );
    let bytes = hex::decode(
        raw.strip_prefix("0x")
            .context("Invalid saved signed transaction")?,
    )
    .context("Invalid saved signed transaction encoding")?;
    let expected_hash: alloy_primitives::B256 =
        hash.parse().context("Invalid saved transaction hash")?;
    ensure!(
        alloy_primitives::keccak256(&bytes) == expected_hash,
        "Saved transaction hash does not match its signed bytes"
    );
    let mut remaining = bytes.as_slice();
    let signed =
        TxLegacy::rlp_decode_signed(&mut remaining).context("Invalid saved signed transaction")?;
    ensure!(remaining.is_empty(), "Saved transaction has trailing bytes");
    let mut canonical = Vec::new();
    signed.rlp_encode(&mut canonical);
    ensure!(
        canonical == bytes,
        "Saved signed transaction is not canonically encoded"
    );
    let transaction = signed.tx();
    ensure!(
        chain_id > 0 && transaction.chain_id() == Some(chain_id),
        "Saved transaction belongs to a different network"
    );
    let wallet: Address = wallet.parse().context("Invalid saved wallet address")?;
    let contract: Address = contract.parse().context("Invalid saved token contract")?;
    ensure!(
        signed
            .recover_signer()
            .context("Invalid saved transaction signature")?
            == wallet,
        "Saved transaction belongs to a different wallet"
    );
    ensure!(
        transaction.to == TxKind::Call(contract),
        "Saved transaction targets a different contract"
    );
    ensure!(
        transaction.value().is_zero(),
        "Saved token transaction cannot send ETH"
    );
    Ok(())
}

pub struct WalletVault {
    path: PathBuf,
    encrypted: Option<EncryptedWallet>,
    key: Option<Zeroizing<[u8; 32]>>,
}

impl WalletVault {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        let metadata =
            fs::symlink_metadata(root).context("Cannot inspect wallet profile folder")?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "Wallet profile must be a real directory"
        );
        let path = root.canonicalize()?.join("wallet.json");
        let encrypted = match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                ensure!(
                    metadata.is_file() && !metadata.file_type().is_symlink(),
                    "Wallet keystore must be a regular file"
                );
                let mut options = OpenOptions::new();
                options.read(true);
                no_follow(&mut options);
                let file = options.open(&path).context("Cannot open wallet keystore")?;
                ensure!(
                    file.metadata()?.is_file() && file.metadata()?.len() <= MAX_KEYSTORE_BYTES,
                    "Wallet keystore is too large or is not a regular file"
                );
                let mut bytes = Vec::new();
                file.take(MAX_KEYSTORE_BYTES + 1).read_to_end(&mut bytes)?;
                ensure!(
                    bytes.len() as u64 <= MAX_KEYSTORE_BYTES,
                    "Wallet keystore is too large"
                );
                let encrypted: EncryptedWallet =
                    serde_json::from_slice(&bytes).context("Invalid wallet keystore")?;
                encrypted.validate()?;
                Some(encrypted)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error).context("Cannot inspect wallet keystore"),
        };
        Ok(Self {
            path,
            encrypted,
            key: None,
        })
    }

    pub fn status(&self) -> Value {
        json!({
            "exists": self.encrypted.is_some(),
            "address": self.address(),
            "unlocked": self.key.is_some(),
        })
    }

    pub fn address(&self) -> Option<String> {
        self.encrypted.as_ref().map(|wallet| wallet.address.clone())
    }

    pub fn create(&mut self, password: &str) -> Result<String> {
        self.ensure_empty()?;
        check_password(password)?;
        let mut key = Zeroizing::new([0u8; 32]);
        loop {
            fill_random(key.as_mut())?;
            if PrivateKeySigner::from_slice(key.as_ref()).is_ok() {
                break;
            }
        }
        self.install_key(key, password)
    }

    pub fn import(&mut self, private_key: &str, password: &str) -> Result<String> {
        self.ensure_empty()?;
        check_password(password)?;
        let private_key = private_key.trim();
        let private_key = private_key.strip_prefix("0x").unwrap_or(private_key);
        ensure!(
            private_key.len() == 64,
            "Private key must contain exactly 64 hexadecimal characters"
        );
        let mut key = Zeroizing::new([0u8; 32]);
        hex::decode_to_slice(private_key, key.as_mut())
            .context("Private key must contain exactly 64 hexadecimal characters")?;
        PrivateKeySigner::from_slice(key.as_ref()).context("Invalid Ethereum private key")?;
        self.install_key(key, password)
    }

    pub fn unlock(&mut self, password: &str) -> Result<String> {
        check_password(password)?;
        let wallet = self
            .encrypted
            .as_ref()
            .context("Create or import a wallet first")?;
        let key = wallet.decrypt(password)?;
        let address = wallet.address.clone();
        self.key = Some(key);
        Ok(address)
    }

    pub fn lock(&mut self) {
        self.key = None;
    }

    /// Only ciphertext and public metadata are returned, even while unlocked.
    pub fn encrypted_backup(&self) -> Result<Value> {
        serde_json::to_value(
            self.encrypted
                .as_ref()
                .context("Create or import a wallet first")?,
        )
        .context("Cannot encode encrypted wallet backup")
    }

    pub fn restore(&mut self, backup: Value, password: &str) -> Result<String> {
        self.ensure_empty()?;
        check_password(password)?;
        ensure!(
            serde_json::to_vec(&backup)?.len() as u64 <= MAX_KEYSTORE_BYTES,
            "Wallet backup is too large"
        );
        let encrypted: EncryptedWallet =
            serde_json::from_value(backup).context("Invalid encrypted wallet backup")?;
        encrypted.validate()?;
        let key = encrypted.decrypt(password)?;
        self.persist_new(&encrypted)?;
        let address = encrypted.address.clone();
        self.encrypted = Some(encrypted);
        self.key = Some(key);
        Ok(address)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn sign_legacy(
        &mut self,
        chain_id: u64,
        nonce: u64,
        gas_price: u128,
        gas_limit: u64,
        to: &str,
        value: U256,
        data: &[u8],
    ) -> Result<SignedTransaction> {
        ensure!(chain_id > 0, "A nonzero Ethereum chain ID is required");
        ensure!(gas_limit > 0, "A nonzero gas limit is required");
        ensure!(data.len() <= 1_048_576, "Transaction data is too large");
        let key = self
            .key
            .as_ref()
            .context("Unlock your built-in wallet first")?;
        let signer =
            PrivateKeySigner::from_slice(key.as_ref()).context("Invalid local wallet key")?;
        let to: Address = to.parse().context("Invalid transaction destination")?;
        let transaction = TxLegacy {
            chain_id: Some(chain_id),
            nonce,
            gas_price,
            gas_limit,
            to: TxKind::Call(to),
            value,
            input: data.to_vec().into(),
        };
        let signature = signer
            .sign_hash_sync(&transaction.signature_hash())
            .context("Cannot sign wallet transaction")?;
        let signed = transaction.into_signed(signature);
        let mut raw = Vec::new();
        signed.rlp_encode(&mut raw);
        Ok(SignedTransaction {
            raw: format!("0x{}", hex::encode(raw)),
            hash: format!("{:#x}", signed.hash()),
        })
    }

    fn ensure_empty(&self) -> Result<()> {
        ensure!(
            self.encrypted.is_none(),
            "A wallet already exists in this profile"
        );
        match fs::symlink_metadata(&self.path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).context("Cannot inspect wallet keystore"),
            Ok(_) => bail!("A wallet keystore already exists; it will not be overwritten"),
        }
    }

    fn install_key(&mut self, key: Zeroizing<[u8; 32]>, password: &str) -> Result<String> {
        let signer =
            PrivateKeySigner::from_slice(key.as_ref()).context("Invalid Ethereum private key")?;
        let address = format!("{:#x}", signer.address());
        let mut salt = [0u8; 16];
        let mut nonce = [0u8; 24];
        fill_random(&mut salt)?;
        fill_random(&mut nonce)?;
        let encryption_key = derive_key(password, &salt)?;
        let cipher = XChaCha20Poly1305::new_from_slice(encryption_key.as_ref())
            .map_err(|_| anyhow::anyhow!("Cannot initialize wallet encryption"))?;
        let aad = authenticated_address(&address);
        let ciphertext = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: key.as_ref(),
                    aad: &aad,
                },
            )
            .map_err(|_| anyhow::anyhow!("Cannot encrypt wallet"))?;
        let encrypted = EncryptedWallet {
            version: 1,
            address: address.clone(),
            crypto: Encryption {
                cipher: CIPHER.into(),
                kdf: KDF.into(),
                memory_kib: MEMORY_KIB,
                iterations: ITERATIONS,
                parallelism: PARALLELISM,
                salt: hex::encode(salt),
                nonce: hex::encode(nonce),
                ciphertext: hex::encode(ciphertext),
            },
        };
        self.persist_new(&encrypted)?;
        self.encrypted = Some(encrypted);
        self.key = Some(key);
        Ok(address)
    }

    fn persist_new(&self, encrypted: &EncryptedWallet) -> Result<()> {
        self.ensure_empty()?;
        let mut random = [0u8; 16];
        fill_random(&mut random)?;
        let parent = self.path.parent().context("Invalid wallet path")?;
        let temporary = parent.join(format!(".wallet-{}.tmp", hex::encode(random)));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        no_follow(&mut options);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let result = (|| -> Result<()> {
            let mut file = options
                .open(&temporary)
                .context("Cannot create encrypted wallet file")?;
            file.write_all(&serde_json::to_vec_pretty(encrypted)?)?;
            file.sync_all()?;
            // Creating a hard link atomically fails if a destination already exists.
            // rename() would silently replace an existing wallet on Unix.
            fs::hard_link(&temporary, &self.path)
                .context("Cannot save wallet without replacing an existing file")?;
            #[cfg(unix)]
            File::open(parent)?.sync_all()?;
            Ok(())
        })();
        let _ = fs::remove_file(&temporary);
        result
    }
}

impl EncryptedWallet {
    fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "Unsupported wallet backup version");
        let address: Address = self
            .address
            .parse()
            .context("Invalid wallet backup address")?;
        ensure!(
            self.address == format!("{address:#x}"),
            "Wallet backup address must be canonical"
        );
        let crypto = &self.crypto;
        // Fixed parameters prevent a supplied backup from requesting excessive work.
        ensure!(
            crypto.cipher == CIPHER
                && crypto.kdf == KDF
                && crypto.memory_kib == MEMORY_KIB
                && crypto.iterations == ITERATIONS
                && crypto.parallelism == PARALLELISM,
            "Unsupported wallet encryption settings"
        );
        ensure!(
            crypto.salt.len() == 32 && crypto.nonce.len() == 48 && crypto.ciphertext.len() == 96,
            "Invalid encrypted wallet lengths"
        );
        ensure!(
            crypto
                .salt
                .bytes()
                .chain(crypto.nonce.bytes())
                .chain(crypto.ciphertext.bytes())
                .all(|byte| byte.is_ascii_hexdigit()),
            "Invalid encrypted wallet encoding"
        );
        Ok(())
    }

    fn decrypt(&self, password: &str) -> Result<Zeroizing<[u8; 32]>> {
        self.validate()?;
        let salt = hex::decode(&self.crypto.salt)?;
        let nonce = hex::decode(&self.crypto.nonce)?;
        let ciphertext = hex::decode(&self.crypto.ciphertext)?;
        let encryption_key = derive_key(password, &salt)?;
        let cipher = XChaCha20Poly1305::new_from_slice(encryption_key.as_ref())
            .map_err(|_| anyhow::anyhow!("Cannot initialize wallet encryption"))?;
        let aad = authenticated_address(&self.address);
        let plaintext = Zeroizing::new(
            cipher
                .decrypt(
                    XNonce::from_slice(&nonce),
                    Payload {
                        msg: &ciphertext,
                        aad: &aad,
                    },
                )
                .map_err(|_| anyhow::anyhow!("Incorrect password or damaged wallet backup"))?,
        );
        ensure!(plaintext.len() == 32, "Invalid decrypted wallet length");
        let mut key = Zeroizing::new([0u8; 32]);
        key.copy_from_slice(&plaintext);
        let signer =
            PrivateKeySigner::from_slice(key.as_ref()).context("Invalid decrypted Ethereum key")?;
        ensure!(
            format!("{:#x}", signer.address()) == self.address,
            "Wallet address does not match its encrypted key"
        );
        Ok(key)
    }
}

fn check_password(password: &str) -> Result<()> {
    ensure!(password.len() <= 1024, "Wallet password is too long");
    ensure!(
        password.chars().count() >= 12,
        "Use a password with at least 12 characters"
    );
    Ok(())
}

fn derive_key(password: &str, salt: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
    let params = Params::new(MEMORY_KIB, ITERATIONS, PARALLELISM, Some(32))
        .map_err(|_| anyhow::anyhow!("Invalid wallet encryption parameters"))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut output = Zeroizing::new([0u8; 32]);
    argon2
        .hash_password_into(password.as_bytes(), salt, output.as_mut())
        .map_err(|_| anyhow::anyhow!("Cannot derive wallet encryption key"))?;
    Ok(output)
}

fn fill_random(bytes: &mut [u8]) -> Result<()> {
    OsRng
        .try_fill_bytes(bytes)
        .context("Operating system randomness is unavailable")
}

fn authenticated_address(address: &str) -> Vec<u8> {
    format!("clipclub-wallet-v1:{address}").into_bytes()
}

fn no_follow(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(not(unix))]
    let _ = options;
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWORD: &str = "correct horse battery staple";
    const KEY: &str = "4646464646464646464646464646464646464646464646464646464646464646";
    const ADDRESS: &str = "0x9d8a62f656a8d1615c1294fd71e9cfb3e4855a4f";

    #[test]
    fn import_reopen_password_backup_restore_and_no_overwrite() {
        let directory = tempfile::tempdir().unwrap();
        let mut wallet = WalletVault::open(directory.path()).unwrap();
        assert_eq!(wallet.status()["exists"], false);
        assert_eq!(wallet.import(KEY, PASSWORD).unwrap(), ADDRESS);
        let backup = wallet.encrypted_backup().unwrap();
        assert!(!backup.to_string().contains(KEY));
        assert!(wallet.create(PASSWORD).is_err());
        assert!(wallet.import(KEY, PASSWORD).is_err());
        assert!(wallet.restore(backup.clone(), PASSWORD).is_err());
        let mut reopened = WalletVault::open(directory.path()).unwrap();
        assert_eq!(reopened.status()["unlocked"], false);
        assert!(reopened.unlock("different password").is_err());
        assert_eq!(reopened.unlock(PASSWORD).unwrap(), ADDRESS);
        reopened.lock();
        assert_eq!(reopened.status()["unlocked"], false);
        let second = tempfile::tempdir().unwrap();
        let mut restored = WalletVault::open(second.path()).unwrap();
        assert_eq!(restored.restore(backup, PASSWORD).unwrap(), ADDRESS);
        assert_eq!(restored.status()["unlocked"], true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(directory.path().join("wallet.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn create_validates_password_and_generates_unique_wallets() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let mut wallet = WalletVault::open(first.path()).unwrap();
        let mut other = WalletVault::open(second.path()).unwrap();
        assert!(wallet.create("short").is_err());
        assert!(wallet.import(&"0".repeat(64), PASSWORD).is_err());
        assert!(wallet.import(KEY, &"a".repeat(1025)).is_err());
        let address = wallet.create(PASSWORD).unwrap();
        assert_ne!(address, other.create(PASSWORD).unwrap());
        assert_eq!(address.len(), 42);
        assert_eq!(wallet.status()["unlocked"], true);
    }

    #[test]
    fn wallet_remains_unlocked_during_the_app_session_and_reopens_locked() {
        let directory = tempfile::tempdir().unwrap();
        let mut wallet = WalletVault::open(directory.path()).unwrap();
        wallet.import(KEY, PASSWORD).unwrap();
        for _ in 0..3 {
            assert_eq!(wallet.status()["unlocked"], true);
            wallet.encrypted_backup().unwrap();
            wallet
                .sign_legacy(4326, 0, 1, 21_000, ADDRESS, U256::ZERO, &[])
                .unwrap();
        }
        // A failed redundant unlock must not lock an active app session.
        assert!(wallet.unlock("incorrect wallet password").is_err());
        assert_eq!(wallet.status()["unlocked"], true);

        wallet.lock();
        assert_eq!(wallet.status()["unlocked"], false);
        assert!(
            wallet
                .sign_legacy(4326, 0, 1, 21_000, ADDRESS, U256::ZERO, &[])
                .is_err()
        );
        drop(wallet);
        let mut reopened = WalletVault::open(directory.path()).unwrap();
        assert_eq!(reopened.status()["unlocked"], false);
        assert!(reopened.unlock("incorrect wallet password").is_err());
        assert_eq!(reopened.status()["unlocked"], false);
        assert_eq!(reopened.unlock(PASSWORD).unwrap(), ADDRESS);
        assert_eq!(reopened.status()["unlocked"], true);
    }

    #[test]
    fn tampering_and_unbounded_kdf_are_rejected_without_installing() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let mut wallet = WalletVault::open(source.path()).unwrap();
        wallet.import(KEY, PASSWORD).unwrap();
        let backup = wallet.encrypted_backup().unwrap();
        let mut restored = WalletVault::open(target.path()).unwrap();
        let mut changed = backup.clone();
        changed["address"] = json!("0x1111111111111111111111111111111111111111");
        assert!(restored.restore(changed, PASSWORD).is_err());
        let mut changed = backup.clone();
        changed["crypto"]["ciphertext"] = json!("00".repeat(48));
        assert!(restored.restore(changed, PASSWORD).is_err());
        let mut changed = backup;
        changed["crypto"]["memory_kib"] = json!(u32::MAX);
        assert!(restored.restore(changed, PASSWORD).is_err());
        assert!(!target.path().join("wallet.json").exists());
    }

    #[test]
    fn signs_eip155_canonical_vector_and_recovers_sender() {
        let directory = tempfile::tempdir().unwrap();
        let mut wallet = WalletVault::open(directory.path()).unwrap();
        wallet.import(KEY, PASSWORD).unwrap();
        let signed = wallet
            .sign_legacy(
                1,
                9,
                20_000_000_000,
                21_000,
                "0x3535353535353535353535353535353535353535",
                U256::from(1_000_000_000_000_000_000u64),
                &[],
            )
            .unwrap();
        assert_eq!(
            signed.raw,
            "0xf86c098504a817c800825208943535353535353535353535353535353535353535880de0b6b3a76400008025a028ef61340bd939bc2195fe537567866003e1a15d3c71ff63e1590620aa636276a067cbe9d8997f761aecb703304b3800ccf555c9f3dc64214b297fb1966a3b6d83"
        );
        let bytes = hex::decode(signed.raw.trim_start_matches("0x")).unwrap();
        let decoded = TxLegacy::rlp_decode_signed(&mut bytes.as_slice()).unwrap();
        assert_eq!(decoded.tx().chain_id(), Some(1));
        assert_eq!(format!("{:#x}", decoded.recover_signer().unwrap()), ADDRESS);
        assert_eq!(
            signed.hash,
            format!("{:#x}", alloy_primitives::keccak256(&bytes))
        );
        wallet.lock();
        assert!(
            wallet
                .sign_legacy(1, 9, 1, 21_000, ADDRESS, U256::ZERO, &[])
                .is_err()
        );
    }

    #[test]
    fn journal_validation_binds_raw_bytes_to_network_sender_target_and_zero_eth() {
        let directory = tempfile::tempdir().unwrap();
        let mut wallet = WalletVault::open(directory.path()).unwrap();
        wallet.import(KEY, PASSWORD).unwrap();
        let contract = "0x3535353535353535353535353535353535353535";
        let signed = wallet
            .sign_legacy(4326, 0, 1, 100_000, contract, U256::ZERO, &[1, 2, 3])
            .unwrap();
        validate_signed_transaction(&signed.raw, &signed.hash, 4326, ADDRESS, contract).unwrap();
        assert!(
            validate_signed_transaction(&signed.raw, &signed.hash, 1, ADDRESS, contract).is_err()
        );
        assert!(
            validate_signed_transaction(&signed.raw, &signed.hash, 4326, contract, contract)
                .is_err()
        );
        assert!(
            validate_signed_transaction(&signed.raw, &signed.hash, 4326, ADDRESS, ADDRESS).is_err()
        );
        assert!(
            validate_signed_transaction(
                &signed.raw,
                &format!("0x{}", "00".repeat(32)),
                4326,
                ADDRESS,
                contract
            )
            .is_err()
        );
        let mut bytes = hex::decode(signed.raw.trim_start_matches("0x")).unwrap();
        bytes.push(0);
        let trailing = format!("0x{}", hex::encode(&bytes));
        let trailing_hash = format!("{:#x}", alloy_primitives::keccak256(bytes));
        assert!(
            validate_signed_transaction(&trailing, &trailing_hash, 4326, ADDRESS, contract)
                .is_err()
        );
        let sends_eth = wallet
            .sign_legacy(4326, 0, 1, 21_000, contract, U256::from(1), &[])
            .unwrap();
        assert!(
            validate_signed_transaction(&sends_eth.raw, &sends_eth.hash, 4326, ADDRESS, contract)
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_and_refuses_racing_overwrite() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let mut first = WalletVault::open(directory.path()).unwrap();
        let mut racing = WalletVault::open(directory.path()).unwrap();
        first.import(KEY, PASSWORD).unwrap();
        assert!(racing.create(PASSWORD).is_err());
        let link = tempfile::tempdir().unwrap();
        symlink(
            directory.path().join("wallet.json"),
            link.path().join("wallet.json"),
        )
        .unwrap();
        assert!(WalletVault::open(link.path()).is_err());
    }
}
