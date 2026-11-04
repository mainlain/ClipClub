//! Noncustodial MegaETH adapter. The RPC verifies receipts; only the user's wallet signs.
use crate::core::ChainPayment;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

const DRAND: &str = "https://api.drand.sh/v2/chains/52db9ba70e0cc0f6eaf7803dd07447a1f5477735fd3f661792ba94600c84e971/rounds/";
const GENESIS: u64 = 1692803367;
const MAINNET_CONTRACT: &str = "0x98d49dC44f1CbbA5ABef438985b20A729dE4ab16";
// A full 1,000-seeder settlement includes both ERC-20 Transfer and SeederPaid
// logs. Its canonical RPC receipt can exceed one megabyte.
const MAX_RPC_RESPONSE_BYTES: usize = 4_000_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChainRoll {
    pub id: String,
    pub status: String,
    pub multiplier: Option<u8>,
    pub wager_units: String,
    pub payout_units: String,
    pub round: u64,
    pub transaction_hash: String,
    pub settlement_hash: Option<String>,
}
#[derive(Default, Clone, Serialize, Deserialize)]
struct Journal {
    wallet: Option<String>,
    roll: Option<ChainRoll>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_transaction: Option<PendingTransaction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rating_session: Option<RatingSession>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completed_rating_session: Option<CompletedRatingSession>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionRating {
    pub clip_id: String,
    pub score: u8,
    /// Absent only on legacy single-recipient drafts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distribution: Option<SeederDistribution>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SeederRecipient {
    pub peer: String,
    pub wallet: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SeederDistribution {
    pub population: usize,
    pub reward_units: String,
    pub recipients: Vec<SeederRecipient>,
}
impl SeederDistribution {
    pub fn new(score: u8, population: usize, recipients: Vec<SeederRecipient>) -> Result<Self> {
        let reward =
            crate::rating_distribution::reward_micropoints(score, recipients.len(), population)?;
        let distribution = Self {
            population,
            reward_units: reward.to_string(),
            recipients,
        };
        distribution.validate(score)?;
        Ok(distribution)
    }
    fn validate(&self, score: u8) -> Result<()> {
        ensure!(
            !self.recipients.is_empty() && self.recipients.len() <= 1000,
            "A rating requires 1–1000 eligible permanent seeders"
        );
        let mut peers = std::collections::BTreeSet::new();
        let mut previous = String::new();
        for recipient in &self.recipients {
            ensure!(
                recipient.peer.parse::<libp2p::PeerId>().is_ok() && peers.insert(&recipient.peer),
                "Invalid or duplicate seeder identity"
            );
            let wallet = address(&recipient.wallet)?;
            ensure!(
                wallet == recipient.wallet && wallet > previous,
                "Seeder wallets must be distinct and sorted"
            );
            previous = wallet;
        }
        ensure!(
            self.reward_units
                == crate::rating_distribution::reward_micropoints(
                    score,
                    self.recipients.len(),
                    self.population
                )?
                .to_string(),
            "Seeder reward disagrees with the local curve"
        );
        Ok(())
    }
    pub fn payout_units(&self) -> Result<u64> {
        self.reward_units
            .parse::<u64>()?
            .checked_mul(self.recipients.len() as u64)
            .context("Seeder payout overflow")
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RatingSession {
    pub id: String,
    pub ratings: Vec<SessionRating>,
    pub total_units: String,
    pub status: String,
}
impl RatingSession {
    fn validate(&self) -> Result<()> {
        ensure!(
            bytes32(&self.id)? == self.id && self.id.bytes().any(|b| b != b'0'),
            "Invalid rating session ID"
        );
        ensure!(
            !self.ratings.is_empty() && self.ratings.len() <= 100,
            "A rating session requires 1–100 clips"
        );
        ensure!(
            matches!(self.status.as_str(), "draft" | "prepared" | "submitted"),
            "Invalid rating session status"
        );
        let mut clips = std::collections::BTreeSet::new();
        let mut total = 0u64;
        let mut payments = 0usize;
        let seeder_mode = self.ratings[0].distribution.is_some();
        for rating in &self.ratings {
            ensure!(
                bytes32(&rating.clip_id)? == rating.clip_id && clips.insert(&rating.clip_id),
                "Invalid or repeated session clip"
            );
            ensure!((1..=9).contains(&rating.score), "Invalid session rating");
            ensure!(
                rating.distribution.is_some() == seeder_mode,
                "Legacy and seeder payments cannot share a session"
            );
            if let Some(distribution) = &rating.distribution {
                distribution.validate(rating.score)?;
                payments += distribution.recipients.len();
            }
            total += u64::from(rating.score) * 1_000_000;
        }
        ensure!(
            payments <= 1000,
            "Settle this session before adding more seeder payments (maximum 1000)"
        );
        ensure!(
            self.total_units == total.to_string(),
            "Invalid session total"
        );
        Ok(())
    }
    pub fn payout_units(&self) -> Result<u64> {
        self.ratings.iter().try_fold(0u64, |sum, rating| {
            let payout = match &rating.distribution {
                Some(distribution) => distribution.payout_units()?,
                None => u64::from(rating.score) * 1_000_000,
            };
            sum.checked_add(payout).context("Session payout overflow")
        })
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct CompletedRatingSession {
    session: RatingSession,
    hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PendingTransaction {
    pub action: String,
    pub hash: String,
    pub clip_id: Option<String>,
    pub score: Option<u8>,
    pub roll_id: Option<String>,
    pub wager_units: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
}
impl PendingTransaction {
    fn validate(&mut self) -> Result<()> {
        hash32(&self.hash)?;
        ensure!(
            self.hash[2..].bytes().any(|byte| byte != b'0'),
            "Invalid pending transaction hash"
        );
        self.hash.make_ascii_lowercase();
        if let Some(id) = &mut self.session_id {
            *id = bytes32(id)?;
        }
        if let Some(clip) = &mut self.clip_id {
            *clip = bytes32(clip)?;
            ensure!(
                clip.bytes().any(|byte| byte != b'0'),
                "Invalid pending clip ID"
            );
        }
        for amount in [&self.roll_id, &self.wager_units].into_iter().flatten() {
            word_decimal(amount)?;
            ensure!(
                amount != "0",
                "Pending roll IDs and wagers must be positive"
            );
        }
        let valid = match self.action.as_str() {
            "register_seeder" => {
                self.clip_id.is_none()
                    && self.score.is_none()
                    && self.roll_id.is_none()
                    && self.wager_units.is_none()
            }
            "rate_session" => {
                self.session_id.is_some()
                    && self.clip_id.is_none()
                    && self.score.is_none()
                    && self.roll_id.is_none()
                    && self.wager_units.is_none()
            }
            "request_roll" => {
                self.wager_units.is_some()
                    && self.roll_id.is_none()
                    && self.clip_id.is_none()
                    && self.score.is_none()
            }
            "settle_roll" => {
                self.roll_id.is_some()
                    && self.wager_units.is_none()
                    && self.clip_id.is_none()
                    && self.score.is_none()
            }
            "register_clip" => {
                self.clip_id.is_some()
                    && self.score.is_none()
                    && self.roll_id.is_none()
                    && self.wager_units.is_none()
            }
            "rate_clip" => {
                self.clip_id.is_some()
                    && self.score.is_some_and(|score| (1..=9).contains(&score))
                    && self.roll_id.is_none()
                    && self.wager_units.is_none()
            }
            _ => false,
        };
        ensure!(
            valid && (self.action == "rate_session" || self.session_id.is_none()),
            "Invalid pending token action fields"
        );
        Ok(())
    }
}
pub struct Crypto {
    pub chain_id: u64,
    contract: Option<String>,
    rpc: String,
    public_rpc: &'static str,
    explorer: &'static str,
    client: reqwest::Client,
    path: PathBuf,
    journal: Mutex<Journal>,
    cache: Mutex<Option<(Instant, Value)>>,
    operation: Mutex<()>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransactionRequest {
    action: String,
    wager_units: Option<String>,
    roll_id: Option<String>,
    clip_id: Option<String>,
    score: Option<u8>,
    expected_session_id: Option<String>,
    expected_rating_count: Option<usize>,
    expected_total_units: Option<String>,
}
fn selected_contract(chain_id: u64, configured: Option<&str>) -> Result<Option<String>> {
    configured
        .or_else(|| (chain_id == 4326).then_some(MAINNET_CONTRACT))
        .filter(|value| !value.is_empty())
        .map(address)
        .transpose()
}
impl Crypto {
    pub async fn selected_wallet(&self) -> Option<String> {
        self.journal.lock().await.wallet.clone()
    }
    pub fn peer_commitment(&self, peer: &str) -> String {
        peer_commitment(peer)
    }
    pub fn open(root: &Path) -> Result<Self> {
        let chain_id = std::env::var("CLIPCLUB_MEGAETH_CHAIN_ID")
            .unwrap_or_else(|_| "4326".into())
            .parse()?;
        let (public_rpc, explorer) = match chain_id {
            4326 => (
                "https://mainnet.megaeth.com/rpc",
                "https://mega.etherscan.io",
            ),
            6343 => (
                "https://carrot.megaeth.com/rpc",
                "https://megaeth-testnet-v2.blockscout.com",
            ),
            _ => bail!("CLIPCLUB_MEGAETH_CHAIN_ID must be 4326 or 6343"),
        };
        let configured_contract = match std::env::var("CLIPCLUB_JOAH_CONTRACT") {
            Ok(value) => Some(value),
            Err(std::env::VarError::NotPresent) => None,
            Err(error) => return Err(error.into()),
        };
        let contract = selected_contract(chain_id, configured_contract.as_deref())?;
        let rpc = std::env::var("CLIPCLUB_MEGAETH_RPC_URL").unwrap_or_else(|_| public_rpc.into());
        let url = reqwest::Url::parse(&rpc).context("Invalid MegaETH RPC URL")?;
        ensure!(
            url.scheme() == "https"
                || (url.scheme() == "http"
                    && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))),
            "Use HTTPS for a remote MegaETH RPC"
        );
        let path = root.join(format!(
            "megaeth-{chain_id}-{}.json",
            contract.as_deref().unwrap_or("unconfigured")
        ));
        let journal = load_journal(&path)?;
        Ok(Self {
            chain_id,
            contract,
            rpc,
            public_rpc,
            explorer,
            path,
            journal: Mutex::new(journal),
            cache: Mutex::new(None),
            operation: Mutex::new(()),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(12))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
        })
    }
    fn contract(&self) -> Result<&str> {
        self.contract
            .as_deref()
            .context("Set CLIPCLUB_JOAH_CONTRACT before using tokens on this network")
    }
    pub async fn wallet(&self) -> Result<String> {
        self.journal
            .lock()
            .await
            .wallet
            .clone()
            .context("Connect your MegaETH wallet first")
    }
    pub async fn set_wallet(&self, wallet: Option<String>) -> Result<()> {
        let _guard = self.operation.lock().await;
        let wallet = wallet.map(|w| address(&w)).transpose()?;
        let mut j = self.journal.lock().await;
        let mut next = j.clone();
        if next.wallet != wallet {
            ensure!(
                next.pending_transaction.is_none() && next.rating_session.is_none(),
                "Settle or cancel the rating session and recover pending transactions before switching wallets"
            );
            next.roll = None;
            next.completed_rating_session = None;
        }
        next.wallet = wallet;
        self.save(&next)?;
        *j = next;
        drop(j);
        *self.cache.lock().await = None;
        Ok(())
    }
    pub async fn set_pending(&self, pending: Option<PendingTransaction>) -> Result<()> {
        let _guard = self.operation.lock().await;
        let mut pending = pending;
        if let Some(transaction) = &mut pending {
            transaction.validate()?;
        }
        let mut journal = self.journal.lock().await;
        if let Some(transaction) = &pending {
            ensure!(
                journal.wallet.is_some(),
                "Connect the wallet that sent this transaction first"
            );
            if let Some(existing) = &journal.pending_transaction {
                ensure!(
                    existing == transaction,
                    "Recover the existing pending transaction before recording another one"
                );
            }
        }
        let mut next = journal.clone();
        if let Some(transaction) = &pending {
            if transaction.action == "rate_session" {
                let session = next
                    .rating_session
                    .as_mut()
                    .context("No rating session to submit")?;
                ensure!(
                    transaction.session_id.as_deref() == Some(session.id.as_str())
                        && matches!(session.status.as_str(), "prepared" | "submitted"),
                    "Transaction does not match the prepared rating session"
                );
                session.status = "submitted".into();
            } else {
                ensure!(
                    next.rating_session
                        .as_ref()
                        .is_none_or(|s| s.status == "draft"),
                    "Finish the prepared rating session first"
                );
            }
        } else {
            ensure!(
                !next
                    .pending_transaction
                    .as_ref()
                    .is_some_and(|p| p.action == "rate_session")
                    || next.rating_session.is_none(),
                "Confirm the rating session or verify its reverted transaction before clearing it"
            );
        }
        next.pending_transaction = pending;
        self.save(&next)?;
        *journal = next;
        drop(journal);
        *self.cache.lock().await = None;
        Ok(())
    }
    pub async fn session_snapshot(&self) -> Option<RatingSession> {
        self.journal.lock().await.rating_session.clone()
    }
    #[cfg(test)]
    pub async fn queue_rating(&self, clip: &str, score: u8) -> Result<RatingSession> {
        self.queue_rating_inner(clip, score, None).await
    }
    pub async fn queue_seeder_rating(
        &self,
        clip: &str,
        score: u8,
        distribution: SeederDistribution,
    ) -> Result<RatingSession> {
        distribution.validate(score)?;
        self.queue_rating_inner(clip, score, Some(distribution))
            .await
    }
    async fn queue_rating_inner(
        &self,
        clip: &str,
        score: u8,
        distribution: Option<SeederDistribution>,
    ) -> Result<RatingSession> {
        let _guard = self.operation.lock().await;
        self.contract()?;
        ensure!((1..=9).contains(&score), "Choose a rating from 1 to 9");
        let clip = bytes32(clip)?;
        let snapshot = self.journal.lock().await.clone();
        let wallet = snapshot
            .wallet
            .as_ref()
            .context("Connect your MegaETH wallet first")?;
        ensure!(
            snapshot.pending_transaction.is_none(),
            "Recover the pending transaction first"
        );
        ensure!(
            snapshot
                .rating_session
                .as_ref()
                .is_none_or(|s| s.status == "draft"),
            "The rating session is frozen for signing or confirmation"
        );
        self.check_chain().await?;
        if distribution.is_none() {
            let recipient = self.call("clipBeneficiary(bytes32)", &clip).await?;
            ensure!(recipient.len() == 64, "Invalid clip beneficiary response");
            let recipient = word_address(&recipient)
                .context("This clip needs a registered payment recipient")?;
            ensure!(
                &recipient != wallet,
                "You cannot pay a rating to your own wallet"
            );
        }
        if let Some(distribution) = &distribution {
            ensure!(
                distribution.recipients.iter().all(|r| &r.wallet != wallet),
                "You cannot pay your own wallet"
            );
        }
        let balance = self
            .call("balanceOf(address)", &address_word(wallet)?)
            .await?;
        let mut session = snapshot
            .rating_session
            .clone()
            .unwrap_or_else(|| RatingSession {
                id: hex::encode(rand::random::<[u8; 32]>()),
                ratings: vec![],
                total_units: "0".into(),
                status: "draft".into(),
            });
        ensure!(
            !session.ratings.iter().any(|r| r.clip_id == clip),
            "This clip is already queued in the rating session"
        );
        ensure!(
            session.ratings.len() < 100,
            "Settle this 100-clip rating session before adding more"
        );
        session.ratings.push(SessionRating {
            clip_id: clip,
            score,
            distribution,
        });
        let total = session
            .ratings
            .iter()
            .map(|r| u64::from(r.score) * 1_000_000)
            .sum::<u64>();
        session.total_units = total.to_string();
        session.validate()?;
        ensure!(
            word_decimal(&session.total_units)? <= balance[..64].to_string(),
            "Insufficient ✨ balance for this rating session"
        );
        let mut next = snapshot;
        next.rating_session = Some(session.clone());
        self.save(&next)?;
        *self.journal.lock().await = next;
        *self.cache.lock().await = None;
        Ok(session)
    }
    pub async fn cancel_rating_session(&self) -> Result<()> {
        let _guard = self.operation.lock().await;
        let mut journal = self.journal.lock().await;
        ensure!(
            journal.pending_transaction.is_none()
                && journal
                    .rating_session
                    .as_ref()
                    .is_none_or(|s| s.status == "draft"),
            "Only an unsigned draft can be cancelled"
        );
        let mut next = journal.clone();
        next.rating_session = None;
        self.save(&next)?;
        *journal = next;
        drop(journal);
        *self.cache.lock().await = None;
        Ok(())
    }
    pub async fn cancel_rating_preparation(&self) -> Result<()> {
        let _guard = self.operation.lock().await;
        let mut journal = self.journal.lock().await;
        ensure!(
            journal.pending_transaction.is_none(),
            "A submitted transaction must be confirmed or verified as reverted"
        );
        let mut next = journal.clone();
        if let Some(session) = next.rating_session.as_mut() {
            ensure!(
                session.status != "submitted",
                "A submitted session cannot be unlocked"
            );
            session.status = "draft".into();
        }
        self.save(&next)?;
        *journal = next;
        drop(journal);
        *self.cache.lock().await = None;
        Ok(())
    }
    fn save(&self, journal: &Journal) -> Result<()> {
        use std::io::Write;
        let bytes = serde_json::to_vec_pretty(journal)?;
        ensure!(bytes.len() < 1_000_000, "MegaETH journal is too large");
        let temp = self.path.with_extension("json.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        #[cfg(windows)]
        {
            let backup = self.path.with_extension("json.backup");
            if self.path.exists() {
                if backup.exists() {
                    std::fs::remove_file(&backup)?;
                }
                std::fs::rename(&self.path, &backup)?;
            }
            if let Err(error) = std::fs::rename(&temp, &self.path) {
                if backup.exists() {
                    let _ = std::fs::rename(&backup, &self.path);
                }
                return Err(error.into());
            }
            let _ = std::fs::remove_file(backup);
        }
        #[cfg(not(windows))]
        std::fs::rename(&temp, &self.path)?;
        #[cfg(unix)]
        std::fs::File::open(self.path.parent().unwrap_or_else(|| Path::new(".")))?.sync_all()?;
        Ok(())
    }
    async fn bounded_json(response: reqwest::Response) -> Result<Value> {
        ensure!(
            response.status().is_success(),
            "Network request failed ({})",
            response.status()
        );
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= MAX_RPC_RESPONSE_BYTES,
                "Network response exceeds its size limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }
    async fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        let result = Self::bounded_json(
            self.client
                .post(&self.rpc)
                .header("Content-Type", "application/json")
                .body(json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string())
                .send()
                .await
                .context("MegaETH RPC unavailable")?,
        )
        .await?;
        if let Some(error) = result.get("error") {
            bail!(
                "MegaETH: {}",
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("RPC request failed")
            );
        }
        result
            .get("result")
            .cloned()
            .context("Malformed MegaETH RPC response")
    }
    async fn check_chain(&self) -> Result<()> {
        let id = self.rpc("eth_chainId", json!([])).await?;
        ensure!(
            id.as_str().map(parse_hex_u64).transpose()? == Some(self.chain_id),
            "RPC belongs to a different chain"
        );
        Ok(())
    }
    async fn call(&self, signature: &str, args: &str) -> Result<String> {
        let data = format!("{}{}", selector(signature)?, args);
        let result = self
            .rpc(
                "eth_call",
                json!([{"to":self.contract()?,"data":data},"latest"]),
            )
            .await?;
        let value = result.as_str().context("Invalid contract response")?;
        ensure!(
            value.starts_with("0x") && value.len() >= 66 && (value.len() - 2) % 64 == 0,
            "Contract ABI mismatch"
        );
        hex::decode(&value[2..]).context("Invalid contract ABI bytes")?;
        Ok(value[2..].to_lowercase())
    }
    /// Small bounded batches for the local library; JSON-RPC responses may be
    /// returned in any order, so associate each result by its request ID.
    #[cfg(test)]
    pub async fn beneficiaries(&self, clips: &[String]) -> Result<BTreeMap<String, String>> {
        ensure!(clips.len() <= 100, "At most 100 local clips may be queried");
        let mut registered = BTreeMap::new();
        for chunk in clips.chunks(20) {
            let calls = chunk.iter().enumerate().map(|(id, clip)| {
                Ok(json!({"jsonrpc":"2.0","id":id,"method":"eth_call","params":[{"to":self.contract()?,"data":format!("{}{}",selector("clipBeneficiary(bytes32)")?,bytes32(clip)?)},"latest"]}))
            }).collect::<Result<Vec<_>>>()?;
            let response = Self::bounded_json(
                self.client
                    .post(&self.rpc)
                    .header("Content-Type", "application/json")
                    .body(serde_json::to_vec(&calls)?)
                    .send()
                    .await
                    .context("MegaETH RPC unavailable")?,
            )
            .await?;
            let responses = response
                .as_array()
                .context("Malformed MegaETH batch response")?;
            ensure!(
                responses.len() == chunk.len(),
                "Incomplete MegaETH batch response"
            );
            let mut seen = vec![false; chunk.len()];
            for result in responses {
                let id: usize = result["id"]
                    .as_u64()
                    .context("Missing batch response ID")?
                    .try_into()?;
                ensure!(id < chunk.len() && !seen[id], "Invalid batch response ID");
                seen[id] = true;
                if result.get("error").is_some() {
                    bail!("Could not read a clip payment recipient");
                }
                let word = bytes32(result["result"].as_str().context("Missing batch result")?)?;
                if word.bytes().any(|byte| byte != b'0') {
                    registered.insert(chunk[id].clone(), word_address(&word)?);
                }
            }
        }
        Ok(registered)
    }
    /// Match wallet registrations against authenticated peer announcements.
    /// Multiple identities advertising one wallet receive at most one payout.
    pub async fn eligible_seeders(
        &self,
        candidates: Vec<SeederRecipient>,
    ) -> Result<Vec<SeederRecipient>> {
        ensure!(candidates.len() <= 2048, "Too many observed seeders");
        let mut eligible = BTreeMap::new();
        for chunk in candidates.chunks(20) {
            let calls = chunk.iter().enumerate().map(|(id, r)| Ok(json!({"jsonrpc":"2.0","id":id,"method":"eth_call","params":[{"to":self.contract()?,"data":format!("{}{}",selector("seederPeer(address)")?,address_word(&r.wallet)? )},"latest"]}))).collect::<Result<Vec<_>>>()?;
            let response = Self::bounded_json(
                self.client
                    .post(&self.rpc)
                    .header("Content-Type", "application/json")
                    .body(serde_json::to_vec(&calls)?)
                    .send()
                    .await?,
            )
            .await?;
            let results = response
                .as_array()
                .context("Malformed seeder registration response")?;
            ensure!(
                results.len() == chunk.len(),
                "Incomplete seeder registration response"
            );
            let mut seen = std::collections::BTreeSet::new();
            for result in results {
                let id = result["id"]
                    .as_u64()
                    .context("Missing seeder response ID")? as usize;
                ensure!(
                    id < chunk.len() && seen.insert(id) && result.get("error").is_none(),
                    "Invalid seeder registration response"
                );
                let commitment = bytes32(
                    result["result"]
                        .as_str()
                        .context("Missing seeder registration")?,
                )?;
                let recipient = &chunk[id];
                ensure!(
                    recipient.peer.parse::<libp2p::PeerId>().is_ok(),
                    "Invalid seeder peer"
                );
                if commitment == peer_commitment(&recipient.peer) {
                    let wallet = address(&recipient.wallet)?;
                    eligible.entry(wallet.clone()).or_insert(SeederRecipient {
                        peer: recipient.peer.clone(),
                        wallet,
                    });
                }
            }
        }
        Ok(eligible.into_values().collect())
    }
    pub async fn state(&self) -> Value {
        // Never hold cache while acquiring operation or journal: mutations
        // acquire those locks before invalidating cache.
        if let Some((at, value)) = self.cache.lock().await.as_ref()
            && at.elapsed() < Duration::from_secs(2)
        {
            return value.clone();
        }
        let (wallet, pending_transaction, rating_session) = {
            let journal = self.journal.lock().await;
            (
                journal.wallet.clone(),
                journal.pending_transaction.clone(),
                journal.rating_session.clone(),
            )
        };
        let mut state = json!({"name":"좋아","symbol":"✨","decimals":6,"chain_id":self.chain_id,"chain_hex":format!("0x{:x}",self.chain_id),"chain_name":if self.chain_id==4326 {"MegaETH"} else {"MegaETH Testnet"},"rpc_url":self.public_rpc,"explorer_url":self.explorer,"contract":self.contract,"configured":self.contract.is_some(),"wallet":wallet,"balance_units":"0","total_supply_units":"0","rating_credit_units":"0","total_rating_minted_units":"0","error":null});
        state["pending_transaction"] =
            serde_json::to_value(&pending_transaction).unwrap_or(Value::Null);
        state["rating_session"] = serde_json::to_value(&rating_session).unwrap_or(Value::Null);
        if let Some(session) = &rating_session {
            let payout = session.payout_units().unwrap_or(0);
            state["rating_session"]["payout_units"] = json!(payout.to_string());
            state["rating_session"]["mint_required_units"] = json!(
                payout
                    .saturating_sub(session.total_units.parse().unwrap_or(0))
                    .to_string()
            );
        }
        if self.contract.is_some() {
            let result = async {
                self.check_chain().await?;
                let (decimals, supply, verifier, rating_minted) = tokio::try_join!(
                    self.call("decimals()", ""),
                    self.call("totalSupply()", ""),
                    self.call("verifier()", ""),
                    self.call("totalRatingMinted()", "")
                )?;
                ensure!(
                    decimal_word(&decimals[..64])? == "6",
                    "Configured token must have six decimal places"
                );
                let expected_verifier = match self.chain_id {
                    4326 => "0x7a53a6efa81c426838fcf4824e6e207923969b36",
                    6343 => "0x4e1673dcaa38136b5032f27ef93423162af977cc",
                    _ => bail!("Unsupported MegaETH network"),
                };
                ensure!(
                    verifier.len() == 64 && word_address(&verifier)? == expected_verifier,
                    "Configured token uses a different randomness verifier"
                );
                state["total_supply_units"] = decimal_word(&supply[..64])?.into();
                state["total_rating_minted_units"] = decimal_word(&rating_minted)?.into();
                if let Some(wallet) = wallet.as_ref() {
                    let balance = self
                        .call("balanceOf(address)", &address_word(&wallet)?)
                        .await?;
                    state["balance_units"] = decimal_word(&balance[..64])?.into();
                    let wallet_arg = address_word(wallet)?;
                    let (credit, seeder) = tokio::try_join!(
                        self.call("ratingCredit(address)", &wallet_arg),
                        self.call("seederPeer(address)", &wallet_arg)
                    )?;
                    let credit = decimal_word(&credit)?;
                    state["rating_credit_units"] = credit.clone().into();
                    if let Some(session) = &rating_session {
                        state["rating_session"]["mint_required_units"] = json!(
                            rating_mint_units(
                                session.payout_units()?,
                                session.total_units.parse()?,
                                &credit
                            )?
                            .to_string()
                        );
                    }
                    state["seeder_peer_commitment"] = seeder.into();
                    self.refresh_roll(&wallet).await?;
                }
                Ok::<_, anyhow::Error>(())
            }
            .await;
            if let Err(error) = result {
                state["error"] = error.to_string().into();
            }
        }
        // A wallet switch during these network reads must not cache balances
        // for the previously selected account.
        let journal = self.journal.lock().await;
        if journal.wallet == wallet
            && journal.pending_transaction == pending_transaction
            && journal.rating_session == rating_session
        {
            *self.cache.lock().await = Some((Instant::now(), state.clone()));
        }
        state
    }
    pub async fn current_roll(&self) -> Option<ChainRoll> {
        self.journal.lock().await.roll.clone()
    }
    async fn read_roll(&self, id: &str, old: Option<&ChainRoll>) -> Result<ChainRoll> {
        let data = self.call("rolls(uint256)", &word_decimal(id)?).await?;
        ensure!(data.len() == 320, "Unexpected roll ABI");
        let player = word_address(&data[..64])?;
        ensure!(
            player == self.wallet().await?,
            "This roll belongs to another wallet"
        );
        let settled = match decimal_word(&data[192..256])?.as_str() {
            "0" => false,
            "1" => true,
            _ => bail!("Invalid on-chain settlement flag"),
        };
        let multiplier: u8 = decimal_word(&data[256..320])?.parse()?;
        ensure!(multiplier <= 9, "Invalid on-chain multiplier");
        let wager = decimal_word(&data[64..128])?;
        ensure!(wager != "0", "Invalid on-chain wager");
        let round = decimal_word(&data[128..192])?.parse()?;
        publish_time(round)?;
        let payout = if settled {
            multiply_decimal(&wager, multiplier)?
        } else {
            "0".into()
        };
        Ok(ChainRoll {
            id: id.into(),
            status: if settled { "settled" } else { "pending" }.into(),
            multiplier: settled.then_some(multiplier),
            wager_units: wager,
            payout_units: payout,
            round,
            transaction_hash: old.map(|r| r.transaction_hash.clone()).unwrap_or_default(),
            settlement_hash: old.and_then(|r| r.settlement_hash.clone()),
        })
    }
    async fn refresh_roll(&self, wallet: &str) -> Result<()> {
        let _guard = self.operation.lock().await;
        // Recover an outstanding wager from the contract even if the browser closed immediately after signing.
        let pending = self
            .call("pendingRoll(address)", &address_word(wallet)?)
            .await?;
        let pending = decimal_word(&pending[..64])?;
        let old = self.current_roll().await;
        let id = if pending != "0" {
            Some(pending.as_str())
        } else {
            old.as_ref()
                .filter(|r| r.status == "pending")
                .map(|r| r.id.as_str())
        };
        if let Some(id) = id {
            let roll = self
                .read_roll(id, old.as_ref().filter(|r| r.id == id))
                .await?;
            let mut j = self.journal.lock().await;
            if j.wallet.as_deref() != Some(wallet) {
                return Ok(());
            }
            let mut next = j.clone();
            next.roll = Some(roll);
            self.save(&next)?;
            *j = next;
        }
        Ok(())
    }
    pub async fn transaction(&self, input: TransactionRequest, profile_id: &str) -> Result<Value> {
        let _guard = self.operation.lock().await;
        self.check_chain().await?;
        let wallet = self.wallet().await?;
        let journal = self.journal.lock().await.clone();
        ensure!(
            input.action == "rate_session"
                || (input.expected_session_id.is_none()
                    && input.expected_rating_count.is_none()
                    && input.expected_total_units.is_none()),
            "Only session checkout accepts session expectations"
        );
        ensure!(
            journal.pending_transaction.is_none(),
            "Recover the pending transaction first"
        );
        ensure!(
            input.action == "rate_session"
                || journal
                    .rating_session
                    .as_ref()
                    .is_none_or(|s| s.status == "draft"),
            "Finish or unlock the prepared rating session first"
        );
        let data = match input.action.as_str() {
            "register_seeder" => format!(
                "{}{}",
                selector("registerSeeder(bytes32)")?,
                peer_commitment(profile_id)
            ),
            "rate_session" => {
                ensure!(
                    input.wager_units.is_none()
                        && input.roll_id.is_none()
                        && input.clip_id.is_none()
                        && input.score.is_none(),
                    "Session checkout accepts no client-supplied ratings"
                );
                let session = journal
                    .rating_session
                    .as_ref()
                    .context("Queue a rating session first")?;
                ensure!(
                    input.expected_session_id.as_deref() == Some(session.id.as_str())
                        && input.expected_rating_count == Some(session.ratings.len())
                        && input.expected_total_units.as_deref()
                            == Some(session.total_units.as_str()),
                    "The rating session changed; review its latest clips and total before signing"
                );
                ensure!(
                    session.status == "draft",
                    "This rating session is already prepared or submitted; finish the existing wallet request or recover its transaction"
                );
                session_data(session, profile_id)?
            }
            "request_roll" => {
                let wager = input.wager_units.context("Enter a wager in token units")?;
                let word = word_decimal(&wager)?;
                ensure!(wager != "0", "Wager must be positive");
                format!("{}{}", selector("requestRoll(uint256)")?, word)
            }
            "register_clip" => format!(
                "{}{}",
                selector("registerClip(bytes32)")?,
                bytes32(input.clip_id.as_deref().context("Missing clip ID")?)?
            ),
            "rate_clip" => {
                let score = input.score.context("Missing rating")?;
                ensure!((1..=9).contains(&score), "Choose a rating from 1 to 9");
                format!(
                    "{}{}{}{}",
                    selector("rateClip(bytes32,uint8,bytes32)")?,
                    bytes32(input.clip_id.as_deref().context("Missing clip ID")?)?,
                    word_decimal(&score.to_string())?,
                    peer_commitment(profile_id),
                )
            }
            "settle_roll" => {
                let id = input.roll_id.context("Missing roll ID")?;
                let roll = self.read_roll(&id, None).await?;
                ensure!(roll.status == "pending", "This roll has already settled");
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_secs();
                ensure!(
                    now >= publish_time(roll.round)?,
                    "Waiting for the committed drand round"
                );
                let beacon = Self::bounded_json(
                    self.client
                        .get(format!("{DRAND}{}", roll.round))
                        .send()
                        .await
                        .context("Could not fetch the drand proof; retry this same roll")?,
                )
                .await?;
                settlement_data(&id, roll.round, &beacon)?
            }
            _ => bail!("Unknown token action"),
        };
        let transaction = json!({"from":wallet,"to":self.contract()?,"data":data,"value":"0x0"});
        // MegaETH has a distinct storage gas model. Simulate with the actual RPC and let the wallet estimate fees.
        self.rpc("eth_estimateGas", json!([transaction])).await?;
        if input.action == "rate_session" {
            let mut next = journal;
            next.rating_session
                .as_mut()
                .context("Missing rating session")?
                .status = "prepared".into();
            self.save(&next)?;
            *self.journal.lock().await = next;
            *self.cache.lock().await = None;
        }
        let mut transaction = transaction;
        transaction["chainId"] = format!("0x{:x}", self.chain_id).into();
        Ok(transaction)
    }
    async fn receipt(&self, hash: &str) -> Result<Value> {
        let receipt = self.canonical_receipt(hash).await?;
        ensure!(
            receipt["status"].as_str() == Some("0x1"),
            "The transaction reverted; no token action was applied"
        );
        Ok(receipt)
    }
    async fn canonical_receipt(&self, hash: &str) -> Result<Value> {
        hash32(hash)?;
        self.check_chain().await?;
        let r = self.rpc("eth_getTransactionReceipt", json!([hash])).await?;
        ensure!(
            !r.is_null(),
            "Transaction is pending; retry confirmation with the same transaction hash"
        );
        ensure!(
            r["transactionHash"]
                .as_str()
                .is_some_and(|h| h.eq_ignore_ascii_case(hash)),
            "Receipt hash mismatch"
        );
        ensure!(
            r["to"].as_str().map(address).transpose()?.as_deref() == Some(self.contract()?),
            "Transaction targets a different contract"
        );
        let number = r["blockNumber"]
            .as_str()
            .context("Receipt has no block number")?;
        parse_hex_u64(number)?;
        let block = self
            .rpc("eth_getBlockByNumber", json!([number, false]))
            .await?;
        let canonical_hash = block["hash"]
            .as_str()
            .context("Canonical block is unavailable; retry")?;
        let receipt_hash = r["blockHash"]
            .as_str()
            .context("Receipt has no block hash")?;
        hash32(canonical_hash)?;
        hash32(receipt_hash)?;
        ensure!(
            canonical_hash.eq_ignore_ascii_case(receipt_hash),
            "Receipt is no longer in the canonical chain; retry"
        );
        Ok(r)
    }
    fn event<'a>(&self, receipt: &'a Value, signature: &str) -> Result<&'a Value> {
        self.events(receipt, signature)?
            .into_iter()
            .next()
            .context("Transaction does not contain the required 좋아 event")
    }
    fn events<'a>(&self, receipt: &'a Value, signature: &str) -> Result<Vec<&'a Value>> {
        let topic = event_topic(signature)?;
        Ok(receipt["logs"]
            .as_array()
            .context("Receipt has no event logs")?
            .iter()
            .filter(|l| {
                l["removed"] != true
                    && l["address"].as_str().is_some_and(|a| {
                        self.contract
                            .as_deref()
                            .is_some_and(|expected| a.eq_ignore_ascii_case(expected))
                    })
                    && l["topics"][0]
                        .as_str()
                        .is_some_and(|t| t.eq_ignore_ascii_case(&topic))
            })
            .collect())
    }
    pub async fn confirm_roll(&self, hash: &str) -> Result<ChainRoll> {
        let _guard = self.operation.lock().await;
        let r = self.receipt(hash).await?;
        let (event, settled) = match self.event(&r, "RollRequested(uint256,address,uint256,uint64)")
        {
            Ok(e) => (e, false),
            Err(_) => (
                self.event(
                    &r,
                    "RollSettled(uint256,address,uint256,uint256,uint8,uint64)",
                )?,
                true,
            ),
        };
        let topics = event["topics"]
            .as_array()
            .context("Malformed roll topics")?;
        ensure!(topics.len() == 3, "Malformed roll topics");
        let player = word_address(&bytes32(topics[2].as_str().context("Missing player")?)?)?;
        ensure!(
            player == self.wallet().await?,
            "This roll belongs to another wallet"
        );
        let id = decimal_word(&bytes32(topics[1].as_str().context("Missing roll ID")?)?)?;
        let old = self.current_roll().await;
        let mut roll = self
            .read_roll(&id, old.as_ref().filter(|r| r.id == id))
            .await?;
        let event_data = event["data"]
            .as_str()
            .and_then(|data| data.strip_prefix("0x"))
            .context("Missing roll event data")?;
        let expected_data = if settled {
            ensure!(
                roll.status == "settled",
                "Settlement is absent from canonical contract state"
            );
            format!(
                "{}{}{}{}",
                word_decimal(&roll.wager_units)?,
                word_decimal(&roll.payout_units)?,
                word_decimal(
                    &roll
                        .multiplier
                        .context("Missing settlement multiplier")?
                        .to_string()
                )?,
                word_decimal(&roll.round.to_string())?
            )
        } else {
            format!(
                "{}{}",
                word_decimal(&roll.wager_units)?,
                word_decimal(&roll.round.to_string())?
            )
        };
        ensure!(
            event_data.eq_ignore_ascii_case(&expected_data),
            "Roll event disagrees with canonical contract state"
        );
        if settled {
            roll.settlement_hash = Some(hash.to_lowercase());
        } else {
            roll.transaction_hash = hash.to_lowercase();
        }
        let mut j = self.journal.lock().await;
        let mut next = j.clone();
        next.roll = Some(roll.clone());
        self.save(&next)?;
        *j = next;
        drop(j);
        *self.cache.lock().await = None;
        Ok(roll)
    }
    pub async fn verify_rating(
        &self,
        clip: &str,
        score: u8,
        payment: &ChainPayment,
        profile_id: &str,
    ) -> Result<()> {
        ensure!(
            (1..=9).contains(&score),
            "A paid rating must be between 1 and 9"
        );
        payment.validate()?;
        ensure!(
            payment.chain_id == self.chain_id
                && payment.contract.eq_ignore_ascii_case(self.contract()?),
            "Rating payment belongs to another deployment"
        );
        let r = self.receipt(&payment.transaction_hash).await?;
        let wanted = bytes32(clip)?;
        let matches = self
            .events(&r, "ClipRated(bytes32,address,uint8,uint256,bytes32)")?
            .into_iter()
            .filter(|event| {
                event["topics"][1]
                    .as_str()
                    .is_some_and(|value| bytes32(value).is_ok_and(|id| id == wanted))
            })
            .collect::<Vec<_>>();
        ensure!(
            matches.len() == 1,
            "Payment must contain exactly one rating for this clip"
        );
        verify_rating_event(matches[0], clip, score, &payment.wallet, profile_id)?;
        Ok(())
    }
    /// Verify every item proof using one canonical receipt read. Only gossip
    /// callers may briefly cache this result; checkout always revalidates.
    pub async fn verify_rating_group(
        &self,
        payment: &ChainPayment,
        profile_id: &str,
    ) -> Result<BTreeMap<String, u8>> {
        payment.validate()?;
        ensure!(
            payment.chain_id == self.chain_id
                && payment.contract.eq_ignore_ascii_case(self.contract()?),
            "Rating payment belongs to another deployment"
        );
        let receipt = self.receipt(&payment.transaction_hash).await?;
        let events = self.events(&receipt, "ClipRated(bytes32,address,uint8,uint256,bytes32)")?;
        ensure!(
            !events.is_empty() && events.len() <= 100,
            "Payment must contain 1–100 rating events"
        );
        let mut ratings = BTreeMap::new();
        for event in events {
            let topics = event["topics"]
                .as_array()
                .context("Malformed rating topics")?;
            ensure!(topics.len() == 3, "Malformed rating topics");
            let clip = bytes32(topics[1].as_str().context("Missing clip topic")?)?;
            let data = event["data"]
                .as_str()
                .and_then(|value| value.strip_prefix("0x"))
                .context("Missing rating event data")?;
            ensure!(
                data.len() == 192 && data.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "Invalid rating event data"
            );
            let score = decimal_word(&data[..64])?.parse::<u8>()?;
            ensure!((1..=9).contains(&score), "Invalid paid rating score");
            verify_rating_event(event, &clip, score, &payment.wallet, profile_id)?;
            ensure!(
                ratings.insert(clip, score).is_none(),
                "Payment contains a repeated rating event"
            );
        }
        Ok(ratings)
    }
    pub async fn session_payment(
        &self,
        hash: &str,
        profile_id: &str,
    ) -> Result<(RatingSession, ChainPayment)> {
        let _guard = self.operation.lock().await;
        let journal = self.journal.lock().await.clone();
        let wallet = journal
            .wallet
            .as_ref()
            .context("Connect the session wallet first")?;
        let session = if let Some(session) = &journal.rating_session {
            session.clone()
        } else {
            let completed = journal
                .completed_rating_session
                .as_ref()
                .filter(|s| s.hash.eq_ignore_ascii_case(hash))
                .context("No pending rating session matches this transaction")?;
            completed.session.clone()
        };
        ensure!(
            session.status != "draft",
            "Prepare this rating session before confirming it"
        );
        if let Some(pending) = &journal.pending_transaction {
            ensure!(
                pending.action == "rate_session"
                    && pending.session_id.as_deref() == Some(&session.id),
                "Transaction does not match the submitted session"
            );
        }
        let receipt = self.receipt(hash).await?;
        let summaries = self.events(
            &receipt,
            "RatingsSettled(bytes32,address,uint256,uint256,bytes32)",
        )?;
        ensure!(
            summaries.len() == 1,
            "Missing or repeated session settlement event"
        );
        let summary = summaries[0];
        let topics = summary["topics"]
            .as_array()
            .context("Malformed session topics")?;
        ensure!(
            topics.len() == 3
                && bytes32(topics[1].as_str().context("Missing session ID")?)? == session.id
                && bytes32(topics[2].as_str().context("Missing session wallet")?)?
                    == address_word(wallet)?,
            "Settlement belongs to another session or wallet"
        );
        let expected = format!(
            "0x{}{}{}",
            word_decimal(&session.ratings.len().to_string())?,
            word_decimal(&session.total_units)?,
            peer_commitment(profile_id)
        );
        ensure!(
            summary["data"]
                .as_str()
                .is_some_and(|data| data.eq_ignore_ascii_case(&expected)),
            "Session count, total, or peer commitment mismatch"
        );
        ensure!(
            receipt["logs"].as_array().and_then(|logs| logs.last()) == Some(summary),
            "Session summary must be the final event"
        );
        let ratings = self.events(&receipt, "ClipRated(bytes32,address,uint8,uint256,bytes32)")?;
        ensure!(
            ratings.len() == session.ratings.len(),
            "Session contains missing, repeated, or unexpected ratings"
        );
        for (event, rating) in ratings.into_iter().zip(&session.ratings) {
            verify_rating_event(event, &rating.clip_id, rating.score, wallet, profile_id)?;
        }
        self.verify_distributions(&receipt, &session)?;
        self.verify_seeder_funding(&receipt, &session, wallet)?;
        let payment = ChainPayment {
            chain_id: self.chain_id,
            contract: self.contract()?.into(),
            transaction_hash: hash.to_ascii_lowercase(),
            wallet: wallet.clone(),
        };
        let mut session = session;
        session.status = "submitted".into();
        // A confirmation can recover the hash after a crash between wallet
        // broadcast and recording it; persist it before core receipts commit.
        if journal.rating_session.is_some() {
            let mut next = journal;
            next.rating_session = Some(session.clone());
            next.pending_transaction = Some(PendingTransaction {
                action: "rate_session".into(),
                hash: hash.to_ascii_lowercase(),
                session_id: Some(session.id.clone()),
                clip_id: None,
                score: None,
                roll_id: None,
                wager_units: None,
            });
            self.save(&next)?;
            *self.journal.lock().await = next;
            *self.cache.lock().await = None;
        }
        Ok((session, payment))
    }
    fn verify_seeder_funding(
        &self,
        receipt: &Value,
        session: &RatingSession,
        wallet: &str,
    ) -> Result<()> {
        let events = self.events(
            receipt,
            "SeederFunding(bytes32,address,uint256,uint256,uint256,uint256)",
        )?;
        if session.ratings[0].distribution.is_none() {
            ensure!(
                events.is_empty(),
                "Unexpected funding event for a legacy session"
            );
            return Ok(());
        }
        ensure!(
            events.len() == 1,
            "Missing or repeated seeder funding event"
        );
        let event = events[0];
        let topics = event["topics"]
            .as_array()
            .context("Missing seeder funding topics")?;
        ensure!(
            topics.len() == 3
                && bytes32(topics[1].as_str().context("Missing funding session ID")?)?
                    == session.id
                && bytes32(topics[2].as_str().context("Missing funding wallet")?)?
                    == address_word(wallet)?,
            "Seeder funding belongs to another session or wallet"
        );
        let data = event["data"]
            .as_str()
            .and_then(|s| s.strip_prefix("0x"))
            .context("Missing funding event data")?;
        ensure!(
            data.len() == 256 && data.bytes().all(|b| b.is_ascii_hexdigit()),
            "Malformed funding event data"
        );
        let payout = session.payout_units()?;
        let credit_before = decimal_word(&data[128..192])?;
        let minted = rating_mint_units(payout, session.total_units.parse()?, &credit_before)?;
        let credit_after = subtract_amount_words(
            &add_amount_words(&credit_before, &session.total_units)?,
            &word_decimal(&(payout - minted).to_string())?,
        )?;
        ensure!(
            data[..64].eq_ignore_ascii_case(&word_decimal(&payout.to_string())?)
                && data[64..128].eq_ignore_ascii_case(&word_decimal(&minted.to_string())?)
                && data[192..].eq_ignore_ascii_case(&credit_after),
            "Seeder funding or mint amount mismatch"
        );
        Ok(())
    }

    fn verify_distributions(&self, receipt: &Value, session: &RatingSession) -> Result<()> {
        let distributions = self.events(
            receipt,
            "SeederDistribution(bytes32,bytes32,uint256,uint256,uint256)",
        )?;
        let payments = self.events(
            receipt,
            "SeederPaid(bytes32,bytes32,address,bytes32,uint256)",
        )?;
        let expected: Vec<_> = session
            .ratings
            .iter()
            .filter_map(|r| r.distribution.as_ref().map(|d| (r, d)))
            .collect();
        ensure!(
            distributions.len() == expected.len()
                && payments.len()
                    == expected
                        .iter()
                        .map(|(_, d)| d.recipients.len())
                        .sum::<usize>(),
            "Unexpected or missing seeder payout events"
        );
        let mut payments = payments.into_iter();
        for (event, (rating, d)) in distributions.into_iter().zip(expected) {
            let topics = event["topics"]
                .as_array()
                .context("Missing distribution topics")?;
            let expected_data = format!(
                "0x{}{}{}",
                word_decimal(&d.population.to_string())?,
                word_decimal(&d.reward_units)?,
                word_decimal(&d.recipients.len().to_string())?
            );
            ensure!(
                topics.len() == 3
                    && topics[1]
                        .as_str()
                        .is_some_and(|s| s.eq_ignore_ascii_case(&format!("0x{}", session.id)))
                    && topics[2]
                        .as_str()
                        .is_some_and(|s| s.eq_ignore_ascii_case(&format!("0x{}", rating.clip_id)))
                    && event["data"]
                        .as_str()
                        .is_some_and(|s| s.eq_ignore_ascii_case(&expected_data)),
                "Seeder distribution changed from the reviewed session"
            );
            for recipient in &d.recipients {
                let payment = payments.next().context("Missing seeder payment")?;
                let topics = payment["topics"]
                    .as_array()
                    .context("Missing seeder payment topics")?;
                let data = format!(
                    "0x{}{}",
                    peer_commitment(&recipient.peer),
                    word_decimal(&d.reward_units)?
                );
                ensure!(
                    topics.len() == 4
                        && topics[1]
                            .as_str()
                            .is_some_and(|s| s.eq_ignore_ascii_case(&format!("0x{}", session.id)))
                        && topics[2].as_str().is_some_and(
                            |s| s.eq_ignore_ascii_case(&format!("0x{}", rating.clip_id))
                        )
                        && topics[3]
                            .as_str()
                            .is_some_and(|s| address_word(&recipient.wallet)
                                .is_ok_and(|w| s.eq_ignore_ascii_case(&format!("0x{w}"))))
                        && payment["data"]
                            .as_str()
                            .is_some_and(|s| s.eq_ignore_ascii_case(&data)),
                    "Seeder recipient or amount changed from the reviewed session"
                );
            }
        }
        Ok(())
    }

    pub async fn confirm_account_action(
        &self,
        hash: &str,
        action: &str,
        profile_id: &str,
    ) -> Result<()> {
        let _guard = self.operation.lock().await;
        ensure!(action == "register_seeder", "Unknown account action");
        let receipt = self.receipt(hash).await?;
        let wallet = self.wallet().await?;
        let events = self.events(&receipt, "SeederRegistered(address,bytes32)")?;
        ensure!(
            events.len() == 1,
            "Missing or repeated seeder registration event"
        );
        let topics = events[0]["topics"]
            .as_array()
            .context("Malformed seeder registration topics")?;
        ensure!(
            topics.len() == 3
                && bytes32(topics[1].as_str().context("Missing seeder wallet")?)?
                    == address_word(&wallet)?,
            "Seeder registration belongs to another wallet"
        );
        ensure!(
            bytes32(topics[2].as_str().context("Missing seeder identity")?)?
                == peer_commitment(profile_id),
            "Seeder registration belongs to another peer"
        );
        *self.cache.lock().await = None;
        Ok(())
    }
    pub async fn complete_rating_session(&self, id: &str, hash: &str) -> Result<()> {
        let _guard = self.operation.lock().await;
        let mut journal = self.journal.lock().await;
        if journal.rating_session.is_none()
            && journal
                .completed_rating_session
                .as_ref()
                .is_some_and(|s| s.session.id == id && s.hash.eq_ignore_ascii_case(hash))
        {
            return Ok(());
        }
        let session = journal
            .rating_session
            .as_ref()
            .context("No session to complete")?;
        ensure!(
            session.id == id
                && session.status == "submitted"
                && journal.pending_transaction.as_ref().is_some_and(
                    |p| p.action == "rate_session" && p.hash.eq_ignore_ascii_case(hash)
                ),
            "Completion does not match the confirmed session"
        );
        let mut next = journal.clone();
        next.completed_rating_session = Some(CompletedRatingSession {
            session: session.clone(),
            hash: hash.to_ascii_lowercase(),
        });
        next.rating_session = None;
        self.save(&next)?;
        *journal = next;
        drop(journal);
        *self.cache.lock().await = None;
        Ok(())
    }
    pub async fn reset_session_submission(&self, hash: &str) -> Result<()> {
        let _guard = self.operation.lock().await;
        let snapshot = self.journal.lock().await.clone();
        let pending = snapshot
            .pending_transaction
            .as_ref()
            .context("No submitted transaction to reset")?;
        ensure!(
            pending.action == "rate_session" && pending.hash.eq_ignore_ascii_case(hash),
            "Hash does not match the submitted rating session"
        );
        let receipt = self.canonical_receipt(hash).await?;
        ensure!(
            receipt["status"].as_str() == Some("0x0"),
            "Only a canonical reverted transaction may be reset"
        );
        let mut next = snapshot;
        next.pending_transaction = None;
        next.rating_session
            .as_mut()
            .context("Missing submitted session")?
            .status = "draft".into();
        self.save(&next)?;
        *self.journal.lock().await = next;
        *self.cache.lock().await = None;
        Ok(())
    }
    pub async fn rating_payment(
        &self,
        clip: &str,
        score: u8,
        hash: Option<String>,
        profile_id: &str,
    ) -> Result<Option<ChainPayment>> {
        if score == 0 {
            ensure!(hash.is_none(), "A skip needs no payment");
            return Ok(None);
        }
        let p = ChainPayment {
            chain_id: self.chain_id,
            contract: self.contract()?.into(),
            transaction_hash: hash.context("Confirm the on-chain rating payment first")?,
            wallet: self.wallet().await?,
        };
        self.verify_rating(clip, score, &p, profile_id).await?;
        Ok(Some(p))
    }
}
fn verify_rating_event(
    e: &Value,
    clip: &str,
    score: u8,
    wallet: &str,
    profile_id: &str,
) -> Result<()> {
    let topics = e["topics"].as_array().context("Malformed rating topics")?;
    ensure!(topics.len() == 3, "Malformed rating topics");
    ensure!(
        bytes32(topics[1].as_str().context("Missing clip topic")?)? == bytes32(clip)?,
        "Payment belongs to another clip"
    );
    ensure!(
        bytes32(topics[2].as_str().context("Missing rater topic")?)? == address_word(wallet)?,
        "Payment belongs to another wallet"
    );
    let data = e["data"]
        .as_str()
        .context("Missing rating event data")?
        .strip_prefix("0x")
        .context("Invalid event data")?;
    ensure!(
        data.len() == 192
            && data.bytes().all(|byte| byte.is_ascii_hexdigit())
            && decimal_word(&data[..64])? == score.to_string()
            && decimal_word(&data[64..128])? == (u64::from(score) * 1_000_000).to_string(),
        "Payment rating or amount mismatch"
    );
    ensure!(
        data[128..].eq_ignore_ascii_case(&peer_commitment(profile_id)),
        "Payment belongs to another peer identity"
    );
    Ok(())
}
fn session_data(session: &RatingSession, peer: &str) -> Result<String> {
    session.validate()?;
    if session.ratings[0].distribution.is_some() {
        // ABI: a dynamic array of dynamic tuples. Tuple offsets are relative
        // to the array payload after its length; nested arrays to each tuple.
        let mut tuples = Vec::new();
        for rating in &session.ratings {
            let d = rating
                .distribution
                .as_ref()
                .context("Missing distribution")?;
            let count = d.recipients.len();
            let wallets = d
                .recipients
                .iter()
                .map(|r| address_word(&r.wallet))
                .collect::<Result<Vec<_>>>()?
                .concat();
            let peers = d
                .recipients
                .iter()
                .map(|r| peer_commitment(&r.peer))
                .collect::<String>();
            tuples.push(format!(
                "{}{}{}{}{}{}{}{}{}{}",
                rating.clip_id,
                word_decimal(&rating.score.to_string())?,
                word_decimal(&d.population.to_string())?,
                word_decimal(&d.reward_units)?,
                word_decimal("192")?,
                word_decimal(&(192 + 32 + count * 32).to_string())?,
                word_decimal(&count.to_string())?,
                wallets,
                word_decimal(&count.to_string())?,
                peers
            ));
        }
        let mut offset = tuples.len() * 32;
        let mut offsets = String::new();
        for tuple in &tuples {
            offsets.push_str(&word_decimal(&offset.to_string())?);
            offset += tuple.len() / 2;
        }
        return Ok(format!(
            "{}{}{}{}{}{}{}",
            selector(
                "settleSeederRatings(bytes32,(bytes32,uint8,uint256,uint256,address[],bytes32[])[],bytes32)"
            )?,
            session.id,
            word_decimal("96")?,
            peer_commitment(peer),
            word_decimal(&tuples.len().to_string())?,
            offsets,
            tuples.concat()
        ));
    }
    let count = session.ratings.len();
    let clips = format!(
        "{}{}",
        word_decimal(&count.to_string())?,
        session
            .ratings
            .iter()
            .map(|r| r.clip_id.clone())
            .collect::<String>()
    );
    let scores = format!(
        "{}{}",
        word_decimal(&count.to_string())?,
        session
            .ratings
            .iter()
            .map(|r| word_decimal(&r.score.to_string()))
            .collect::<Result<Vec<_>>>()?
            .concat()
    );
    Ok(format!(
        "{}{}{}{}{}{}{}",
        selector("rateClips(bytes32,bytes32[],uint8[],bytes32)")?,
        session.id,
        word_decimal("128")?,
        word_decimal(&(128 + 32 + count * 32).to_string())?,
        peer_commitment(peer),
        clips,
        scores
    ))
}
fn peer_commitment(profile_id: &str) -> String {
    hex::encode(Sha256::digest(profile_id.as_bytes()))
}
fn load_journal(path: &Path) -> Result<Journal> {
    // Windows replacement uses a backup rename. Recover it after a crash in
    // the interval before the new journal becomes visible.
    let backup = path.with_extension("json.backup");
    if !path.exists() && backup.exists() {
        std::fs::rename(&backup, path).context("Could not recover MegaETH journal")?;
    }
    if !path.exists() {
        return Ok(Journal::default());
    }
    ensure!(
        std::fs::metadata(path)?.len() < 1_000_000,
        "MegaETH journal is too large"
    );
    let mut journal: Journal = serde_json::from_slice(&std::fs::read(path)?)
        .context("Invalid MegaETH journal; restore its backup")?;
    journal.wallet = journal.wallet.map(|wallet| address(&wallet)).transpose()?;
    if let Some(session) = &journal.rating_session {
        ensure!(
            journal.wallet.is_some(),
            "Saved rating session has no wallet"
        );
        session.validate()?;
        if session.status == "submitted" {
            ensure!(
                journal
                    .pending_transaction
                    .as_ref()
                    .is_some_and(|p| p.action == "rate_session"
                        && p.session_id.as_deref() == Some(session.id.as_str())),
                "Submitted session has no matching transaction"
            );
        }
    }
    if let Some(completed) = &journal.completed_rating_session {
        completed.session.validate()?;
        hash32(&completed.hash)?;
        ensure!(
            journal.wallet.is_some() && completed.session.status == "submitted",
            "Invalid completed rating session"
        );
    }
    if let Some(transaction) = &mut journal.pending_transaction {
        ensure!(journal.wallet.is_some(), "Saved transaction has no wallet");
        transaction.validate()?;
        if transaction.action == "rate_session" {
            let id = transaction.session_id.as_deref();
            ensure!(
                journal
                    .rating_session
                    .as_ref()
                    .is_some_and(|s| Some(s.id.as_str()) == id && s.status == "submitted")
                    || journal
                        .completed_rating_session
                        .as_ref()
                        .is_some_and(|s| Some(s.session.id.as_str()) == id
                            && s.hash.eq_ignore_ascii_case(&transaction.hash)),
                "Saved rating transaction has no matching session"
            );
        }
    }
    if let Some(roll) = &journal.roll {
        ensure!(journal.wallet.is_some(), "Saved roll has no wallet");
        word_decimal(&roll.id)?;
        word_decimal(&roll.wager_units)?;
        word_decimal(&roll.payout_units)?;
        ensure!(
            roll.id != "0" && roll.wager_units != "0",
            "Invalid saved roll"
        );
        publish_time(roll.round)?;
        match roll.status.as_str() {
            "pending" => ensure!(
                roll.multiplier.is_none() && roll.payout_units == "0",
                "Invalid pending roll"
            ),
            "settled" => {
                let multiplier = roll
                    .multiplier
                    .context("Saved settlement has no multiplier")?;
                ensure!(
                    multiplier <= 9
                        && multiply_decimal(&roll.wager_units, multiplier)? == roll.payout_units,
                    "Invalid saved payout"
                );
            }
            _ => bail!("Invalid saved roll status"),
        }
        if !roll.transaction_hash.is_empty() {
            hash32(&roll.transaction_hash)?;
        }
        if let Some(hash) = &roll.settlement_hash {
            hash32(hash)?;
        }
    }
    Ok(journal)
}
fn publish_time(round: u64) -> Result<u64> {
    round
        .checked_sub(1)
        .and_then(|round| round.checked_mul(3))
        .and_then(|offset| GENESIS.checked_add(offset))
        .context("Invalid drand round")
}
fn settlement_data(id: &str, round: u64, beacon: &Value) -> Result<String> {
    publish_time(round)?;
    ensure!(
        beacon["round"].as_u64() == Some(round),
        "Drand returned a different round"
    );
    let signature = beacon["signature"]
        .as_str()
        .context("Missing drand signature")?;
    ensure!(
        matches!(signature.len(), 96 | 192),
        "Invalid drand signature length"
    );
    let proof = hex::decode(signature).context("Invalid drand signature hex")?;
    let padded = format!(
        "{signature:0<width$}",
        width = proof.len().div_ceil(32) * 64
    );
    Ok(format!(
        "{}{}{}{}{}",
        selector("settleRoll(uint256,bytes)")?,
        word_decimal(id)?,
        word_decimal("64")?,
        word_decimal(&proof.len().to_string())?,
        padded
    ))
}
fn selector(signature: &str) -> Result<String> {
    abi_value("functions", signature)
}
fn event_topic(signature: &str) -> Result<String> {
    abi_value("events", signature)
}
fn abi_value(kind: &str, signature: &str) -> Result<String> {
    let abi: Value = serde_json::from_str(include_str!("../contracts/abi-selectors.json"))?;
    abi[kind][signature]
        .as_str()
        .map(str::to_owned)
        .with_context(|| format!("Missing ABI {signature}"))
}
pub fn address(s: &str) -> Result<String> {
    ensure!(
        s.starts_with("0x")
            && s.len() == 42
            && s[2..].bytes().all(|c| c.is_ascii_hexdigit())
            && s[2..].bytes().any(|b| b != b'0'),
        "Invalid Ethereum wallet or contract address"
    );
    Ok(s.to_ascii_lowercase())
}
fn address_word(s: &str) -> Result<String> {
    Ok(format!("{:0>64}", &address(s)?[2..]))
}
fn word_address(word: &str) -> Result<String> {
    let word = bytes32(word)?;
    ensure!(
        word[..24].bytes().all(|byte| byte == b'0'),
        "Invalid ABI address padding"
    );
    address(&format!("0x{}", &word[24..]))
}
fn bytes32(s: &str) -> Result<String> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    ensure!(
        s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()),
        "Expected 32-byte hex value"
    );
    Ok(s.to_ascii_lowercase())
}
fn hash32(s: &str) -> Result<()> {
    ensure!(s.starts_with("0x"), "Transaction hash must start with 0x");
    bytes32(s)?;
    Ok(())
}
fn parse_hex_u64(s: &str) -> Result<u64> {
    Ok(u64::from_str_radix(
        s.strip_prefix("0x").context("Invalid hex quantity")?,
        16,
    )?)
}
fn word_decimal(s: &str) -> Result<String> {
    ensure!(
        !s.is_empty()
            && s.len() <= 78
            && s.bytes().all(|c| c.is_ascii_digit())
            && (s == "0" || !s.starts_with('0')),
        "Use an unsigned integer token amount without leading zeros"
    );
    let mut bytes = [0u8; 32];
    for digit in s.bytes() {
        let mut carry = u16::from(digit - b'0');
        for byte in bytes.iter_mut().rev() {
            let n = u16::from(*byte) * 10 + carry;
            *byte = n as u8;
            carry = n >> 8;
        }
        ensure!(carry == 0, "Token amount exceeds uint256");
    }
    Ok(hex::encode(bytes))
}
fn add_amount_words(a: &str, b: &str) -> Result<String> {
    let mut a = hex::decode(word_decimal(a)?)?;
    let b = hex::decode(word_decimal(b)?)?;
    let mut carry = 0u16;
    for (a, b) in a.iter_mut().rev().zip(b.iter().rev()) {
        let sum = u16::from(*a) + u16::from(*b) + carry;
        *a = sum as u8;
        carry = sum >> 8;
    }
    ensure!(carry == 0, "Token amount exceeds uint256");
    Ok(hex::encode(a))
}
fn subtract_amount_words(a: &str, b: &str) -> Result<String> {
    ensure!(a.len() == 64 && b.len() == 64, "Invalid amount word");
    let mut a = hex::decode(a)?;
    let b = hex::decode(b)?;
    let mut borrow = 0i16;
    for (a, b) in a.iter_mut().rev().zip(b.iter().rev()) {
        let difference = i16::from(*a) - i16::from(*b) - borrow;
        *a = difference.rem_euclid(256) as u8;
        borrow = i16::from(difference < 0);
    }
    ensure!(borrow == 0, "Insufficient rating credit");
    Ok(hex::encode(a))
}
fn rating_mint_units(payout: u64, charge: u64, credit: &str) -> Result<u64> {
    let shortfall = payout.saturating_sub(charge);
    if word_decimal(credit)? >= word_decimal(&shortfall.to_string())? {
        return Ok(0);
    }
    Ok(shortfall - credit.parse::<u64>()?)
}
fn decimal_word(s: &str) -> Result<String> {
    ensure!(s.len() == 64, "Invalid ABI word length");
    let bytes = hex::decode(s)?;
    let mut digits = vec![0u16];
    for byte in bytes {
        let mut carry = u16::from(byte);
        for d in &mut digits {
            let n = *d * 256 + carry;
            *d = n % 10;
            carry = n / 10;
        }
        while carry > 0 {
            digits.push(carry % 10);
            carry /= 10;
        }
    }
    Ok(digits
        .iter()
        .rev()
        .map(|d| char::from(b'0' + *d as u8))
        .collect())
}
fn multiply_decimal(s: &str, multiplier: u8) -> Result<String> {
    let mut bytes = hex::decode(word_decimal(s)?)?;
    let mut carry = 0u16;
    for b in bytes.iter_mut().rev() {
        let n = u16::from(*b) * u16::from(multiplier) + carry;
        *b = n as u8;
        carry = n >> 8;
    }
    ensure!(carry == 0, "Payout exceeds uint256");
    decimal_word(&hex::encode(bytes))
}
#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State, routing::post};
    use std::sync::{Arc, Mutex as StdMutex};

    const PEER: &str = "12D3KooWExampleSignedPeerIdentity";
    const CONTRACT: &str = "0xabcdef1234567890123456789012345678901234";
    const WALLET: &str = "0xfeedab1234567890123456789012345678901234";
    const OTHER: &str = "0xcccccc1234567890123456789012345678901234";
    const CLIP: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const HASH: &str = "0xdddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    const BLOCK: &str = "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

    #[derive(Clone)]
    struct RpcFixture {
        chain: String,
        verifier: String,
        receipt: Value,
        canonical_block: String,
        roll_data: String,
        pending: String,
        beneficiaries: BTreeMap<String, String>,
        balance: String,
        estimate_error: bool,
        rating_credit: String,
        seeder_peers: BTreeMap<String, String>,
    }
    #[derive(Clone)]
    struct RpcState {
        fixture: Arc<StdMutex<RpcFixture>>,
        requests: Arc<StdMutex<Vec<Value>>>,
        balance_read: Arc<tokio::sync::Notify>,
    }
    struct Mock {
        crypto: Arc<Crypto>,
        state: RpcState,
        task: tokio::task::JoinHandle<()>,
        _directory: tempfile::TempDir,
    }
    impl Drop for Mock {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    fn rpc_result(state: &RpcState, input: &Value) -> Value {
        state.requests.lock().unwrap().push(input.clone());
        let f = state.fixture.lock().unwrap().clone();
        if input["method"] == "eth_estimateGas" && f.estimate_error {
            return json!({"jsonrpc":"2.0","id":input["id"],"error":{"message":"Execution reverted"}});
        }
        let result = match input["method"].as_str().unwrap() {
            "eth_chainId" => json!(f.chain),
            "eth_getTransactionReceipt" => f.receipt,
            "eth_getBlockByNumber" => json!({"hash":f.canonical_block}),
            "eth_estimateGas" => json!("0x186a0"),
            "eth_call" => {
                let data = input["params"][0]["data"].as_str().unwrap();
                let signature = &data[..10];
                let word = if signature == selector("rolls(uint256)").unwrap() {
                    f.roll_data
                } else if signature == selector("pendingRoll(address)").unwrap() {
                    word_decimal(&f.pending).unwrap()
                } else if signature == selector("decimals()").unwrap() {
                    word_decimal("6").unwrap()
                } else if signature == selector("verifier()").unwrap() {
                    address_word(&f.verifier).unwrap()
                } else if signature == selector("balanceOf(address)").unwrap() {
                    state.balance_read.notify_one();
                    word_decimal(&f.balance).unwrap()
                } else if signature == selector("clipBeneficiary(bytes32)").unwrap() {
                    f.beneficiaries
                        .get(&data[10..])
                        .map(|address| address_word(address).unwrap())
                        .unwrap_or_else(|| word_decimal("0").unwrap())
                } else if signature == selector("ratingCredit(address)").unwrap() {
                    word_decimal(&f.rating_credit).unwrap()
                } else if signature == selector("seederPeer(address)").unwrap() {
                    f.seeder_peers
                        .get(&data[10..])
                        .cloned()
                        .unwrap_or_else(|| "0".repeat(64))
                } else {
                    word_decimal("1000000000").unwrap()
                };
                json!(format!("0x{word}"))
            }
            method => panic!("Unexpected RPC method {method}"),
        };
        json!({"jsonrpc":"2.0","id":input["id"],"result":result})
    }
    async fn rpc_handler(State(state): State<RpcState>, Json(input): Json<Value>) -> Json<Value> {
        Json(if let Some(batch) = input.as_array() {
            Value::Array(
                batch
                    .iter()
                    .rev()
                    .map(|request| rpc_result(&state, request))
                    .collect(),
            )
        } else {
            rpc_result(&state, &input)
        })
    }
    fn roll_data(wallet: &str, wager: &str, round: u64, settled: bool, multiplier: u8) -> String {
        format!(
            "{}{}{}{}{}",
            address_word(wallet).unwrap(),
            word_decimal(wager).unwrap(),
            word_decimal(&round.to_string()).unwrap(),
            word_decimal(if settled { "1" } else { "0" }).unwrap(),
            word_decimal(&multiplier.to_string()).unwrap()
        )
    }
    fn receipt(event: Value) -> Value {
        json!({"transactionHash":HASH,"status":"0x1","to":CONTRACT,"blockNumber":"0x10","blockHash":BLOCK,"logs":[event]})
    }
    fn rating_event() -> Value {
        json!({"address":CONTRACT,"removed":false,"topics":[event_topic("ClipRated(bytes32,address,uint8,uint256,bytes32)").unwrap(),format!("0x{CLIP}"),format!("0x{}",address_word(WALLET).unwrap())],"data":format!("0x{}{}{}",word_decimal("5").unwrap(),word_decimal("5000000").unwrap(),peer_commitment(PEER))})
    }
    fn roll_event(settled: bool) -> Value {
        let (topic, data) = if settled {
            (
                event_topic("RollSettled(uint256,address,uint256,uint256,uint8,uint64)").unwrap(),
                format!(
                    "{}{}{}{}",
                    word_decimal("1000000").unwrap(),
                    word_decimal("9000000").unwrap(),
                    word_decimal("9").unwrap(),
                    word_decimal("100").unwrap()
                ),
            )
        } else {
            (
                event_topic("RollRequested(uint256,address,uint256,uint64)").unwrap(),
                format!(
                    "{}{}",
                    word_decimal("1000000").unwrap(),
                    word_decimal("100").unwrap()
                ),
            )
        };
        json!({"address":CONTRACT,"topics":[topic,format!("0x{}",word_decimal("7").unwrap()),format!("0x{}",address_word(WALLET).unwrap())],"data":format!("0x{data}")})
    }
    async fn mock() -> Mock {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let state = RpcState {
            fixture: Arc::new(StdMutex::new(RpcFixture {
                chain: "0x18c7".into(),
                verifier: "0x4e1673dcAA38136b5032F27ef93423162aF977Cc".into(),
                receipt: receipt(rating_event()),
                canonical_block: BLOCK.into(),
                roll_data: roll_data(WALLET, "1000000", 100, false, 0),
                pending: "0".into(),
                beneficiaries: BTreeMap::from([(CLIP.into(), WALLET.into())]),
                balance: "100000000".into(),
                estimate_error: false,
                rating_credit: "0".into(),
                seeder_peers: BTreeMap::new(),
            })),
            requests: Arc::new(StdMutex::new(vec![])),
            balance_read: Arc::new(tokio::sync::Notify::new()),
        };
        let app = Router::new()
            .route("/", post(rpc_handler))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let directory = tempfile::tempdir().unwrap();
        let crypto = Arc::new(Crypto {
            chain_id: 6343,
            contract: Some(CONTRACT.into()),
            rpc: endpoint,
            public_rpc: "https://carrot.megaeth.com/rpc",
            explorer: "https://testnet-mega.etherscan.io",
            client: reqwest::Client::new(),
            path: directory.path().join("journal.json"),
            journal: Mutex::new(Journal {
                wallet: Some(WALLET.into()),
                roll: None,
                pending_transaction: None,
                rating_session: None,
                completed_rating_session: None,
            }),
            cache: Mutex::new(None),
            operation: Mutex::new(()),
        });
        Mock {
            crypto,
            state,
            task,
            _directory: directory,
        }
    }
    fn payment() -> ChainPayment {
        ChainPayment {
            chain_id: 6343,
            contract: CONTRACT.into(),
            transaction_hash: HASH.into(),
            wallet: WALLET.into(),
        }
    }
    #[test]
    fn malformed_unicode_event_data_is_rejected_without_slicing_untrusted_text() {
        let mut event = rating_event();
        event["data"] = format!("0x{}é{}", "0".repeat(63), "0".repeat(127)).into();
        assert!(verify_rating_event(&event, CLIP, 5, WALLET, PEER).is_err());
    }
    fn checkout_request(session: &RatingSession) -> TransactionRequest {
        TransactionRequest {
            action: "rate_session".into(),
            wager_units: None,
            roll_id: None,
            clip_id: None,
            score: None,
            expected_session_id: Some(session.id.clone()),
            expected_rating_count: Some(session.ratings.len()),
            expected_total_units: Some(session.total_units.clone()),
        }
    }
    fn pending_session(session: &RatingSession) -> PendingTransaction {
        PendingTransaction {
            action: "rate_session".into(),
            hash: HASH.into(),
            session_id: Some(session.id.clone()),
            clip_id: None,
            score: None,
            roll_id: None,
            wager_units: None,
        }
    }
    fn session_receipt(session: &RatingSession) -> Value {
        let mut logs=session.ratings.iter().map(|rating|json!({"address":CONTRACT,"topics":[event_topic("ClipRated(bytes32,address,uint8,uint256,bytes32)").unwrap(),format!("0x{}",rating.clip_id),format!("0x{}",address_word(WALLET).unwrap())],"data":format!("0x{}{}{}",word_decimal(&rating.score.to_string()).unwrap(),word_decimal(&(u64::from(rating.score)*1_000_000).to_string()).unwrap(),peer_commitment(PEER))})).collect::<Vec<_>>();
        for rating in &session.ratings {
            if let Some(d) = &rating.distribution {
                logs.push(json!({"address":CONTRACT,"topics":[event_topic("SeederDistribution(bytes32,bytes32,uint256,uint256,uint256)").unwrap(),format!("0x{}",session.id),format!("0x{}",rating.clip_id)],"data":format!("0x{}{}{}",word_decimal(&d.population.to_string()).unwrap(),word_decimal(&d.reward_units).unwrap(),word_decimal(&d.recipients.len().to_string()).unwrap())}));
                for r in &d.recipients {
                    logs.push(json!({"address":CONTRACT,"topics":[event_topic("SeederPaid(bytes32,bytes32,address,bytes32,uint256)").unwrap(),format!("0x{}",session.id),format!("0x{}",rating.clip_id),format!("0x{}",address_word(&r.wallet).unwrap())],"data":format!("0x{}{}",peer_commitment(&r.peer),word_decimal(&d.reward_units).unwrap())}));
                }
            }
        }
        if session.ratings[0].distribution.is_some() {
            let payout = session.payout_units().unwrap();
            let charge = session.total_units.parse::<u64>().unwrap();
            let minted = payout.saturating_sub(charge);
            let credit_after = charge.saturating_sub(payout);
            logs.push(json!({"address":CONTRACT,"topics":[event_topic("SeederFunding(bytes32,address,uint256,uint256,uint256,uint256)").unwrap(),format!("0x{}",session.id),format!("0x{}",address_word(WALLET).unwrap())],"data":format!("0x{}{}{}{}",word_decimal(&payout.to_string()).unwrap(),word_decimal(&minted.to_string()).unwrap(),word_decimal("0").unwrap(),word_decimal(&credit_after.to_string()).unwrap())}));
        }
        logs.push(json!({"address":CONTRACT,"topics":[event_topic("RatingsSettled(bytes32,address,uint256,uint256,bytes32)").unwrap(),format!("0x{}",session.id),format!("0x{}",address_word(WALLET).unwrap())],"data":format!("0x{}{}{}",word_decimal(&session.ratings.len().to_string()).unwrap(),word_decimal(&session.total_units).unwrap(),peer_commitment(PEER))}));
        let mut result = receipt(Value::Null);
        result["logs"] = Value::Array(logs);
        result
    }
    async fn draft_two(m: &Mock) -> RatingSession {
        m.state.fixture.lock().unwrap().beneficiaries =
            BTreeMap::from([(CLIP.into(), OTHER.into()), ("b".repeat(64), OTHER.into())]);
        m.crypto.queue_rating(CLIP, 5).await.unwrap();
        m.crypto.queue_rating(&"b".repeat(64), 7).await.unwrap()
    }

    fn test_seeder(wallet: &str) -> SeederRecipient {
        SeederRecipient {
            peer: libp2p::identity::Keypair::generate_ed25519()
                .public()
                .to_peer_id()
                .to_string(),
            wallet: wallet.into(),
        }
    }

    #[tokio::test]
    async fn seeder_rewards_queue_without_prefunding_and_confirmation_rejects_changed_payouts() {
        let m = mock().await;
        let recipients = vec![
            test_seeder("0x1111111111111111111111111111111111111111"),
            test_seeder(OTHER),
        ];
        let d = SeederDistribution::new(5, 100, recipients).unwrap();
        assert!(d.payout_units().unwrap() > 5_000_000);
        let draft = m
            .crypto
            .queue_seeder_rating(CLIP, 5, d.clone())
            .await
            .unwrap();
        assert_eq!(draft.total_units, "5000000");
        assert_eq!(
            load_journal(&m.crypto.path).unwrap().rating_session,
            Some(draft.clone())
        );
        let tx = m
            .crypto
            .transaction(checkout_request(&draft), PEER)
            .await
            .unwrap();
        assert!(tx["data"].as_str().unwrap().starts_with(&selector("settleSeederRatings(bytes32,(bytes32,uint8,uint256,uint256,address[],bytes32[])[],bytes32)").unwrap()));
        let valid = session_receipt(&draft);
        for mutation in 0..4 {
            let mut bad = valid.clone();
            match mutation {
                0 => {
                    bad["logs"][2]["topics"][3] =
                        format!("0x{}", address_word(WALLET).unwrap()).into()
                }
                1 => {
                    bad["logs"][2]["data"] = format!(
                        "0x{}{}",
                        peer_commitment(&d.recipients[0].peer),
                        word_decimal("1").unwrap()
                    )
                    .into()
                }
                2 => {
                    bad["logs"].as_array_mut().unwrap().remove(2);
                }
                _ => {
                    let extra = bad["logs"][2].clone();
                    bad["logs"].as_array_mut().unwrap().insert(2, extra);
                }
            }
            m.state.fixture.lock().unwrap().receipt = bad;
            assert!(m.crypto.session_payment(HASH, PEER).await.is_err());
            assert_eq!(
                m.crypto.session_snapshot().await.unwrap().status,
                "prepared"
            );
        }
        m.state.fixture.lock().unwrap().receipt = valid;
        let (confirmed, _) = m.crypto.session_payment(HASH, PEER).await.unwrap();
        assert_eq!(confirmed.ratings, draft.ratings);
        assert_eq!(confirmed.status, "submitted");
    }

    #[tokio::test]
    async fn seeder_registrations_match_the_announcing_peer_and_deduplicate_wallets() {
        let m = mock().await;
        let a = test_seeder(OTHER);
        let impostor = test_seeder(OTHER);
        let b = test_seeder("0x1111111111111111111111111111111111111111");
        m.state.fixture.lock().unwrap().seeder_peers = BTreeMap::from([
            (address_word(OTHER).unwrap(), peer_commitment(&a.peer)),
            (address_word(&b.wallet).unwrap(), peer_commitment(&b.peer)),
        ]);
        let eligible = m
            .crypto
            .eligible_seeders(vec![a.clone(), impostor, b.clone(), a.clone()])
            .await
            .unwrap();
        assert_eq!(eligible, vec![b, a]);
    }

    #[tokio::test]
    async fn mint_forecast_uses_credit_and_confirmation_rejects_forged_funding() {
        let m = mock().await;
        let recipients = (1..=5)
            .map(|n| test_seeder(&format!("0x{n:040x}")))
            .collect();
        let distribution = SeederDistribution::new(5, 100, recipients).unwrap();
        assert_eq!(distribution.payout_units().unwrap(), 12_851_655);
        let draft = m
            .crypto
            .queue_seeder_rating(CLIP, 5, distribution)
            .await
            .unwrap();
        m.state.fixture.lock().unwrap().rating_credit = "1000000".into();
        let state = m.crypto.state().await;
        assert_eq!(state["rating_session"]["mint_required_units"], "6851655");
        assert_eq!(state["rating_session"]["total_units"], "5000000");
        m.crypto
            .transaction(checkout_request(&draft), PEER)
            .await
            .unwrap();
        let mut valid = session_receipt(&draft);
        let index = valid["logs"].as_array().unwrap().len() - 2;
        valid["logs"][index]["data"] = format!(
            "0x{}{}{}{}",
            word_decimal("12851655").unwrap(),
            word_decimal("6851655").unwrap(),
            word_decimal("1000000").unwrap(),
            word_decimal("0").unwrap()
        )
        .into();
        for mutation in 0..6 {
            let mut bad = valid.clone();
            match mutation {
                0 => {
                    bad["logs"].as_array_mut().unwrap().remove(index);
                }
                1 => {
                    let extra = bad["logs"][index].clone();
                    bad["logs"].as_array_mut().unwrap().insert(index, extra);
                }
                2 => bad["logs"][index]["topics"][1] = format!("0x{}", "f".repeat(64)).into(),
                3 => {
                    bad["logs"][index]["topics"][2] =
                        format!("0x{}", address_word(OTHER).unwrap()).into()
                }
                4 => {
                    bad["logs"][index]["data"] = format!(
                        "0x{}{}{}{}",
                        word_decimal("12851655").unwrap(),
                        word_decimal("7851655").unwrap(),
                        word_decimal("1000000").unwrap(),
                        word_decimal("0").unwrap()
                    )
                    .into()
                }
                _ => {
                    bad["logs"][index]["data"] = format!(
                        "0x{}{}{}{}",
                        word_decimal("12851655").unwrap(),
                        word_decimal("6851655").unwrap(),
                        word_decimal("1000000").unwrap(),
                        word_decimal("1").unwrap()
                    )
                    .into()
                }
            }
            m.state.fixture.lock().unwrap().receipt = bad;
            assert!(m.crypto.session_payment(HASH, PEER).await.is_err());
            assert_eq!(
                m.crypto.session_snapshot().await.unwrap().status,
                "prepared"
            );
        }
        m.state.fixture.lock().unwrap().receipt = valid;
        assert!(m.crypto.session_payment(HASH, PEER).await.is_ok());
    }

    #[test]
    fn mint_and_credit_arithmetic_preserves_full_uint256_credit_values() {
        let large = "18446744073709551616";
        assert_eq!(
            rating_mint_units(12_851_655, 5_000_000, "0").unwrap(),
            7_851_655
        );
        assert_eq!(
            rating_mint_units(12_851_655, 5_000_000, "7851655").unwrap(),
            0
        );
        assert_eq!(rating_mint_units(12_851_655, 5_000_000, large).unwrap(), 0);
        let after = subtract_amount_words(
            &add_amount_words(large, "5000000").unwrap(),
            &word_decimal("12851655").unwrap(),
        )
        .unwrap();
        assert_eq!(decimal_word(&after).unwrap(), "18446744073701699961");
        assert!(
            subtract_amount_words(&word_decimal("0").unwrap(), &word_decimal("1").unwrap())
                .is_err()
        );
    }

    #[test]
    fn distribution_rejects_repeated_wallets_peers_and_tampered_curve_amounts() {
        let a = test_seeder(OTHER);
        assert!(SeederDistribution::new(5, 100, vec![a.clone(), a.clone()]).is_err());
        let mut d = SeederDistribution::new(5, 100, vec![a]).unwrap();
        d.reward_units = "1".into();
        assert!(d.validate(5).is_err());
        assert!(SeederDistribution::new(5, 100, vec![]).is_err());
    }

    #[tokio::test]
    async fn seeder_registration_confirmation_binds_the_wallet_and_peer() {
        let m = mock().await;
        let registration = receipt(
            json!({"address":CONTRACT,"topics":[event_topic("SeederRegistered(address,bytes32)").unwrap(),format!("0x{}",address_word(WALLET).unwrap()),format!("0x{}",peer_commitment(PEER))],"data":"0x"}),
        );
        m.state.fixture.lock().unwrap().receipt = registration.clone();
        m.crypto
            .confirm_account_action(HASH, "register_seeder", PEER)
            .await
            .unwrap();
        assert!(
            m.crypto
                .confirm_account_action(HASH, "register_seeder", "another peer")
                .await
                .is_err()
        );
        let mut wrong = registration;
        wrong["logs"][0]["topics"][1] = format!("0x{}", address_word(OTHER).unwrap()).into();
        m.state.fixture.lock().unwrap().receipt = wrong;
        assert!(
            m.crypto
                .confirm_account_action(HASH, "register_seeder", PEER)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn session_checkout_freezes_durable_server_ratings_and_recovers_atomic_completion() {
        let m = mock().await;
        let draft = draft_two(&m).await;
        assert_eq!(draft.total_units, "12000000");
        assert_eq!(draft.status, "draft");
        assert_eq!(
            load_journal(&m.crypto.path).unwrap().rating_session,
            Some(draft.clone())
        );
        assert!(m.crypto.set_wallet(Some(OTHER.into())).await.is_err());
        assert!(m.crypto.queue_rating(CLIP, 5).await.is_err());
        let tx = m
            .crypto
            .transaction(checkout_request(&draft), PEER)
            .await
            .unwrap();
        assert_eq!(tx["data"], session_data(&draft, PEER).unwrap());
        let encoded = hex::decode(&tx["data"].as_str().unwrap()[10..]).unwrap();
        assert_eq!(decimal_word(&hex::encode(&encoded[32..64])).unwrap(), "128");
        assert_eq!(decimal_word(&hex::encode(&encoded[64..96])).unwrap(), "224");
        assert_eq!(
            m.crypto.session_snapshot().await.unwrap().status,
            "prepared"
        );
        // A second window must not acquire ownership of an existing wallet
        // prompt and later unlock it merely by rejecting its own prompt.
        assert!(
            m.crypto
                .transaction(checkout_request(&draft), PEER)
                .await
                .is_err()
        );
        assert_eq!(
            m.crypto.session_snapshot().await.unwrap().status,
            "prepared"
        );
        assert!(m.crypto.cancel_rating_session().await.is_err());
        assert!(m.crypto.queue_rating(&"c".repeat(64), 1).await.is_err());
        m.crypto.cancel_rating_preparation().await.unwrap();
        assert_eq!(m.crypto.session_snapshot().await.unwrap().status, "draft");
        m.crypto
            .transaction(checkout_request(&draft), PEER)
            .await
            .unwrap();
        m.crypto
            .set_pending(Some(pending_session(&draft)))
            .await
            .unwrap();
        assert!(m.crypto.set_pending(None).await.is_err());
        assert!(m.crypto.cancel_rating_preparation().await.is_err());
        m.state.fixture.lock().unwrap().receipt = session_receipt(&draft);
        let (settled, payment) = m.crypto.session_payment(HASH, PEER).await.unwrap();
        assert_eq!(settled.status, "submitted");
        assert_eq!(payment.transaction_hash, HASH);
        // A batched receipt's second ClipRated log remains individually valid.
        m.crypto
            .verify_rating(&draft.ratings[1].clip_id, 7, &payment, PEER)
            .await
            .unwrap();
        m.crypto
            .complete_rating_session(&draft.id, HASH)
            .await
            .unwrap();
        assert!(
            load_journal(&m.crypto.path)
                .unwrap()
                .rating_session
                .is_none()
        );
        assert!(m.crypto.session_snapshot().await.is_none());
        let (recovered, _) = m.crypto.session_payment(HASH, PEER).await.unwrap();
        assert_eq!(recovered.ratings, draft.ratings);
        m.crypto
            .complete_rating_session(&draft.id, HASH)
            .await
            .unwrap();
        m.crypto.set_pending(None).await.unwrap();
        m.crypto.set_wallet(Some(OTHER.into())).await.unwrap();
    }

    #[tokio::test]
    async fn canonical_speedup_receipt_replaces_pending_hash_only_after_full_validation() {
        let m = mock().await;
        let draft = draft_two(&m).await;
        m.crypto
            .transaction(checkout_request(&draft), PEER)
            .await
            .unwrap();
        m.crypto
            .set_pending(Some(pending_session(&draft)))
            .await
            .unwrap();
        let mut replacement = session_receipt(&draft);
        replacement["transactionHash"] = BLOCK.into();
        let mut wrong = replacement.clone();
        wrong["logs"][2]["topics"][1] = format!("0x{}", "c".repeat(64)).into();
        m.state.fixture.lock().unwrap().receipt = wrong;
        assert!(m.crypto.session_payment(BLOCK, PEER).await.is_err());
        assert_eq!(
            load_journal(&m.crypto.path)
                .unwrap()
                .pending_transaction
                .unwrap()
                .hash,
            HASH
        );
        m.state.fixture.lock().unwrap().receipt = replacement;
        let (session, payment) = m.crypto.session_payment(BLOCK, PEER).await.unwrap();
        assert_eq!(session.id, draft.id);
        assert_eq!(payment.transaction_hash, BLOCK);
        assert_eq!(
            load_journal(&m.crypto.path)
                .unwrap()
                .pending_transaction
                .unwrap()
                .hash,
            BLOCK
        );
        m.crypto
            .complete_rating_session(&draft.id, BLOCK)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn grouped_gossip_proofs_read_one_receipt_and_reject_a_bad_second_event() {
        let m = mock().await;
        let draft = draft_two(&m).await;
        m.state.fixture.lock().unwrap().receipt = session_receipt(&draft);
        let readings_before = m
            .state
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["method"] == "eth_getTransactionReceipt")
            .count();
        let ratings = m
            .crypto
            .verify_rating_group(&payment(), PEER)
            .await
            .unwrap();
        assert_eq!(
            ratings,
            BTreeMap::from([(CLIP.into(), 5), ("b".repeat(64), 7)])
        );
        let readings_after = m
            .state
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["method"] == "eth_getTransactionReceipt")
            .count();
        assert_eq!(readings_after - readings_before, 1);
        let mut wrong = session_receipt(&draft);
        wrong["logs"][1]["data"] = format!(
            "0x{}{}{}",
            word_decimal("7").unwrap(),
            word_decimal("7000000").unwrap(),
            peer_commitment("different-peer")
        )
        .into();
        m.state.fixture.lock().unwrap().receipt = wrong;
        assert!(
            m.crypto
                .verify_rating_group(&payment(), PEER)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn session_rejects_forged_extra_missing_or_wrong_commitments_and_resets_only_reverts() {
        let m = mock().await;
        let draft = draft_two(&m).await;
        m.crypto
            .transaction(checkout_request(&draft), PEER)
            .await
            .unwrap();
        m.crypto
            .set_pending(Some(pending_session(&draft)))
            .await
            .unwrap();
        let original = session_receipt(&draft);
        let mut wrong_total = original.clone();
        wrong_total["logs"][2]["data"] = format!(
            "0x{}{}{}",
            word_decimal("2").unwrap(),
            word_decimal("1").unwrap(),
            peer_commitment(PEER)
        )
        .into();
        let mut repeated = original.clone();
        repeated["logs"][1] = repeated["logs"][0].clone();
        let mut extra = original.clone();
        extra["logs"]
            .as_array_mut()
            .unwrap()
            .insert(0, rating_event());
        for receipt in [wrong_total, repeated, extra] {
            m.state.fixture.lock().unwrap().receipt = receipt;
            assert!(m.crypto.session_payment(HASH, PEER).await.is_err());
            assert_eq!(
                m.crypto.session_snapshot().await.unwrap().ratings,
                draft.ratings
            );
        }
        m.state.fixture.lock().unwrap().receipt = original.clone();
        assert!(
            m.crypto
                .session_payment(HASH, "another-peer")
                .await
                .is_err()
        );
        assert!(m.crypto.reset_session_submission(HASH).await.is_err());
        m.state.fixture.lock().unwrap().receipt["status"] = "0x0".into();
        m.crypto.reset_session_submission(HASH).await.unwrap();
        assert_eq!(m.crypto.session_snapshot().await.unwrap().status, "draft");
        assert!(
            load_journal(&m.crypto.path)
                .unwrap()
                .pending_transaction
                .is_none()
        );
        m.crypto.cancel_rating_session().await.unwrap();
        assert!(m.crypto.session_snapshot().await.is_none());
    }

    #[tokio::test]
    async fn stale_review_is_rejected_before_estimation_or_session_freeze() {
        let m = mock().await;
        m.state.fixture.lock().unwrap().beneficiaries =
            BTreeMap::from([(CLIP.into(), OTHER.into()), ("b".repeat(64), OTHER.into())]);
        let reviewed = m.crypto.queue_rating(CLIP, 5).await.unwrap();
        let current = m.crypto.queue_rating(&"b".repeat(64), 7).await.unwrap();
        let count_before = m
            .state
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["method"] == "eth_estimateGas")
            .count();
        assert!(
            m.crypto
                .transaction(checkout_request(&reviewed), PEER)
                .await
                .unwrap_err()
                .to_string()
                .contains("changed")
        );
        assert_eq!(m.crypto.session_snapshot().await, Some(current));
        assert_eq!(
            m.state
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r["method"] == "eth_estimateGas")
                .count(),
            count_before
        );
    }

    #[tokio::test]
    async fn draft_liabilities_and_failed_gas_estimation_never_mutate_frozen_ratings() {
        let m = mock().await;
        {
            let mut f = m.state.fixture.lock().unwrap();
            f.beneficiaries =
                BTreeMap::from([(CLIP.into(), OTHER.into()), ("b".repeat(64), OTHER.into())]);
            f.balance = "5000000".into();
        }
        let draft = m.crypto.queue_rating(CLIP, 5).await.unwrap();
        assert!(m.crypto.queue_rating(&"b".repeat(64), 1).await.is_err());
        assert_eq!(m.crypto.session_snapshot().await, Some(draft.clone()));
        m.state.fixture.lock().unwrap().estimate_error = true;
        assert!(
            m.crypto
                .transaction(checkout_request(&draft), PEER)
                .await
                .is_err()
        );
        assert_eq!(m.crypto.session_snapshot().await, Some(draft));
    }

    #[tokio::test]
    async fn pending_transaction_survives_restart_and_protects_wallet_context() {
        let m = mock().await;
        let pending = PendingTransaction {
            action: "rate_clip".into(),
            session_id: None,
            hash: HASH.into(),
            clip_id: Some(CLIP.into()),
            score: Some(5),
            roll_id: None,
            wager_units: None,
        };
        m.crypto.set_pending(Some(pending.clone())).await.unwrap();
        assert_eq!(
            load_journal(&m.crypto.path).unwrap().pending_transaction,
            Some(pending.clone())
        );
        assert_eq!(m.crypto.state().await["pending_transaction"]["hash"], HASH);
        m.crypto.set_pending(Some(pending.clone())).await.unwrap();
        assert!(m.crypto.set_wallet(Some(OTHER.into())).await.is_err());
        assert!(m.crypto.set_wallet(None).await.is_err());
        let mut different = pending.clone();
        different.hash = BLOCK.into();
        assert!(m.crypto.set_pending(Some(different)).await.is_err());
        let backup = m.crypto.path.with_extension("json.backup");
        std::fs::rename(&m.crypto.path, &backup).unwrap();
        assert_eq!(
            load_journal(&m.crypto.path).unwrap().pending_transaction,
            Some(pending.clone())
        );
        m.crypto.set_pending(None).await.unwrap();
        assert!(
            load_journal(&m.crypto.path)
                .unwrap()
                .pending_transaction
                .is_none()
        );
        assert!(m.crypto.state().await["pending_transaction"].is_null());
        for invalid in [
            PendingTransaction {
                action: "anything".into(),
                ..pending.clone()
            },
            PendingTransaction {
                score: Some(0),
                ..pending.clone()
            },
            PendingTransaction {
                hash: "bad".into(),
                ..pending.clone()
            },
            PendingTransaction {
                wager_units: Some("1".into()),
                ..pending
            },
        ] {
            assert!(m.crypto.set_pending(Some(invalid)).await.is_err());
        }
        m.crypto.set_wallet(Some(OTHER.into())).await.unwrap();
        assert_eq!(m.crypto.wallet().await.unwrap(), OTHER);
    }

    #[tokio::test]
    async fn malformed_roll_abi_is_rejected_before_preparing_settlement() {
        let m = mock().await;
        let valid = roll_data(WALLET, "1000000", 100, false, 0);
        let mut invalid_bool = valid.clone();
        invalid_bool.replace_range(192..256, &word_decimal("2").unwrap());
        let mut invalid_padding = valid.clone();
        invalid_padding.replace_range(..1, "1");
        for data in [
            invalid_bool,
            invalid_padding,
            roll_data(WALLET, "1000000", 0, false, 0),
        ] {
            m.state.fixture.lock().unwrap().roll_data = data;
            assert!(m.crypto.read_roll("7", None).await.is_err());
        }
    }

    #[tokio::test]
    async fn state_rejects_token_using_a_different_randomness_verifier() {
        let m = mock().await;
        assert!(m.crypto.state().await["error"].is_null());
        *m.crypto.cache.lock().await = None;
        m.state.fixture.lock().unwrap().verifier = OTHER.into();
        assert!(
            m.crypto.state().await["error"]
                .as_str()
                .unwrap()
                .contains("different randomness verifier")
        );
    }

    #[tokio::test]
    async fn rating_calldata_binds_payment_to_the_trusted_local_peer() {
        let m = mock().await;
        let input = TransactionRequest {
            action: "rate_clip".into(),
            wager_units: None,
            roll_id: None,
            clip_id: Some(CLIP.into()),
            score: Some(5),
            expected_session_id: None,
            expected_rating_count: None,
            expected_total_units: None,
        };
        let transaction = m.crypto.transaction(input, PEER).await.unwrap();
        let data = transaction["data"].as_str().unwrap();
        assert_eq!(
            data,
            format!(
                "{}{}{}{}",
                selector("rateClip(bytes32,uint8,bytes32)").unwrap(),
                CLIP,
                word_decimal("5").unwrap(),
                peer_commitment(PEER)
            )
        );
        assert_eq!(transaction["from"], WALLET);
        assert!(
            m.state
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|request| request["method"] == "eth_estimateGas")
        );
        m.crypto
            .verify_rating(CLIP, 5, &payment(), PEER)
            .await
            .unwrap();
        let error = m
            .crypto
            .verify_rating(CLIP, 5, &payment(), "another-peer")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("another peer identity"));
    }

    #[tokio::test]
    async fn paid_rating_requires_canonical_success_and_matching_event() {
        let m = mock().await;
        // Providers commonly checksum addresses and use mixed-case hash hex.
        {
            let mut f = m.state.fixture.lock().unwrap();
            f.receipt["to"] = format!("0x{}", CONTRACT[2..].to_uppercase()).into();
            f.receipt["logs"][0]["address"] = format!("0x{}", CONTRACT[2..].to_uppercase()).into();
            f.canonical_block = format!("0x{}", BLOCK[2..].to_uppercase());
        }
        m.crypto
            .verify_rating(CLIP, 5, &payment(), PEER)
            .await
            .unwrap();
        let original = m.state.fixture.lock().unwrap().clone();
        for (label, path, value) in [
            ("reverted", vec!["status"], json!("0x0")),
            ("forged emitter", vec!["logs", "0", "address"], json!(OTHER)),
            ("removed event", vec!["logs", "0", "removed"], json!(true)),
            (
                "wrong amount",
                vec!["logs", "0", "data"],
                json!(format!(
                    "0x{}{}",
                    word_decimal("5").unwrap(),
                    word_decimal("1").unwrap()
                )),
            ),
            ("wrong hash", vec!["transactionHash"], json!(BLOCK)),
        ] {
            let mut changed = original.clone();
            let mut target = &mut changed.receipt;
            for key in path {
                target = if key == "0" {
                    &mut target[0]
                } else {
                    &mut target[key]
                };
            }
            *target = value;
            *m.state.fixture.lock().unwrap() = changed;
            assert!(
                m.crypto
                    .verify_rating(CLIP, 5, &payment(), PEER)
                    .await
                    .is_err(),
                "{label}"
            );
        }
        *m.state.fixture.lock().unwrap() = original.clone();
        assert!(
            m.crypto
                .verify_rating(CLIP, 4, &payment(), PEER)
                .await
                .is_err()
        );
        assert!(
            m.crypto
                .verify_rating(&"b".repeat(64), 5, &payment(), PEER)
                .await
                .is_err()
        );
        let mut wrong_wallet = payment();
        wrong_wallet.wallet = OTHER.into();
        assert!(
            m.crypto
                .verify_rating(CLIP, 5, &wrong_wallet, PEER)
                .await
                .is_err()
        );
        m.state.fixture.lock().unwrap().canonical_block = HASH.into();
        assert!(
            m.crypto
                .verify_rating(CLIP, 5, &payment(), PEER)
                .await
                .unwrap_err()
                .to_string()
                .contains("canonical")
        );
        *m.state.fixture.lock().unwrap() = original.clone();
        m.state.fixture.lock().unwrap().receipt = Value::Null;
        assert!(
            m.crypto
                .verify_rating(CLIP, 5, &payment(), PEER)
                .await
                .unwrap_err()
                .to_string()
                .contains("pending")
        );
        *m.state.fixture.lock().unwrap() = original;
        m.state.fixture.lock().unwrap().chain = "0x1".into();
        assert!(
            m.crypto
                .verify_rating(CLIP, 5, &payment(), PEER)
                .await
                .is_err()
        );
        assert!(m.crypto.current_roll().await.is_none());
    }

    #[tokio::test]
    async fn roll_confirmation_reconciles_and_persists_without_trusting_client_results() {
        let m = mock().await;
        m.state.fixture.lock().unwrap().receipt = receipt(roll_event(false));
        let pending = m.crypto.confirm_roll(HASH).await.unwrap();
        assert_eq!(pending.id, "7");
        assert_eq!(pending.status, "pending");
        assert_eq!(pending.wager_units, "1000000");
        {
            let mut f = m.state.fixture.lock().unwrap();
            f.receipt = receipt(roll_event(true));
            f.roll_data = roll_data(WALLET, "1000000", 100, true, 9);
        }
        let settled = m.crypto.confirm_roll(HASH).await.unwrap();
        assert_eq!(settled.payout_units, "9000000");
        assert_eq!(settled.multiplier, Some(9));
        assert_eq!(
            load_journal(&m.crypto.path).unwrap().roll,
            Some(settled.clone())
        );
        m.state.fixture.lock().unwrap().roll_data = roll_data(OTHER, "1000000", 100, true, 9);
        assert!(m.crypto.confirm_roll(HASH).await.is_err());
        assert_eq!(m.crypto.current_roll().await, Some(settled));
    }

    #[tokio::test]
    async fn wallet_changes_and_state_refresh_do_not_deadlock_or_cache_old_wallet() {
        let m = mock().await;
        let operation = m.crypto.operation.lock().await;
        let crypto = m.crypto.clone();
        let refresh = tokio::spawn(async move { crypto.state().await });
        tokio::time::timeout(Duration::from_secs(2), m.state.balance_read.notified())
            .await
            .unwrap();
        assert!(
            m.crypto.cache.try_lock().is_ok(),
            "state must release cache before waiting for operation"
        );
        drop(operation);
        let (state, result) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(refresh, m.crypto.set_wallet(Some(OTHER.into())))
        })
        .await
        .unwrap();
        state.unwrap();
        result.unwrap();
        assert_eq!(m.crypto.wallet().await.unwrap(), OTHER);
        let cached = m.crypto.cache.lock().await;
        assert!(
            cached
                .as_ref()
                .is_none_or(|(_, state)| state["wallet"] == OTHER)
        );
    }

    #[tokio::test]
    async fn beneficiary_batches_match_response_ids_and_zero_means_unregistered() {
        let m = mock().await;
        let clips = vec![CLIP.into(), "b".repeat(64)];
        let result = m.crypto.beneficiaries(&clips).await.unwrap();
        assert_eq!(result.get(CLIP).map(String::as_str), Some(WALLET));
        assert_eq!(result.len(), 1);
        let single = m.crypto.beneficiaries(&clips[..1]).await.unwrap();
        assert_eq!(single.get(CLIP).map(String::as_str), Some(WALLET));
    }

    #[test]
    fn drand_settlement_calldata_has_exact_round_and_dynamic_bytes_encoding() {
        for length in [48, 96] {
            let signature = "ab".repeat(length);
            let data =
                settlement_data("7", 100, &json!({"round":100,"signature":signature})).unwrap();
            assert!(data.starts_with("0xbb3a34d3"));
            let encoded = hex::decode(&data[10..]).unwrap();
            assert_eq!(encoded[31], 7);
            assert_eq!(encoded[63], 64);
            assert_eq!(usize::from(encoded[95]), length);
            assert_eq!(&encoded[96..96 + length], vec![0xab; length]);
            assert!(encoded[96 + length..].iter().all(|byte| *byte == 0));
            assert_eq!(encoded.len(), 96 + length.div_ceil(32) * 32);
        }
        assert!(
            settlement_data("7", 100, &json!({"round":101,"signature":"ab".repeat(48)})).is_err()
        );
        assert!(
            settlement_data("7", 100, &json!({"round":100,"signature":"zz".repeat(48)})).is_err()
        );
        assert!(
            settlement_data("7", 100, &json!({"round":100,"signature":"ab".repeat(47)})).is_err()
        );
        assert!(publish_time(0).is_err());
        assert!(publish_time(u64::MAX).is_err());
    }

    #[test]
    fn journal_recovers_interrupted_replacement_without_resetting_corrupt_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("journal.json");
        let backup = path.with_extension("json.backup");
        std::fs::write(
            &backup,
            serde_json::to_vec(&Journal {
                wallet: Some(WALLET.into()),
                roll: None,
                pending_transaction: None,
                rating_session: None,
                completed_rating_session: None,
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(load_journal(&path).unwrap().wallet.as_deref(), Some(WALLET));
        assert!(path.exists());
        assert!(!backup.exists());
        std::fs::write(&path, b"corrupt").unwrap();
        assert!(load_journal(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"corrupt");
    }
    #[test]
    fn amounts_roundtrip_without_float_loss() {
        for n in [
            "0",
            "1",
            "1000000",
            "9007199254740993123456",
            "115792089237316195423570985008687907853269984665640564039457584007913129639935",
        ] {
            assert_eq!(decimal_word(&word_decimal(n).unwrap()).unwrap(), n);
        }
        for n in [
            "",
            "01",
            "-1",
            "1.1",
            "1e6",
            "115792089237316195423570985008687907853269984665640564039457584007913129639936",
        ] {
            assert!(word_decimal(n).is_err());
        }
        assert_eq!(multiply_decimal("1234567", 9).unwrap(), "11111103");
    }
    #[test]
    fn live_contract_defaults_only_to_mainnet_and_preserves_explicit_configuration() {
        let deployed = "0x98d49dc44f1cbba5abef438985b20a729de4ab16";
        let custom = "0xAB12345678901234567890123456789012345678";
        assert_eq!(
            selected_contract(4326, None).unwrap().as_deref(),
            Some(deployed)
        );
        assert_eq!(selected_contract(6343, None).unwrap(), None);
        for chain_id in [4326, 6343] {
            assert_eq!(
                selected_contract(chain_id, Some(custom)).unwrap(),
                Some(custom.to_ascii_lowercase())
            );
            assert_eq!(selected_contract(chain_id, Some("")).unwrap(), None);
            assert!(selected_contract(chain_id, Some("invalid")).is_err());
        }
    }
    #[test]
    fn wallet_and_hash_inputs_are_strict() {
        assert!(address("0x0000000000000000000000000000000000000000").is_err());
        assert!(hash32("bad").is_err());
        assert!(
            address("0xAB12345678901234567890123456789012345678")
                .unwrap()
                .starts_with("0xab")
        );
    }
}
