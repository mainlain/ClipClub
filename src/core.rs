//! Portable persistence and signed, append-only rating receipts.
//!
//! Signatures prevent impersonation and receipt replay. They do not establish a
//! global user count, prove seed ownership, prevent Sybil identities, or provide
//! consensus during network partitions. This is an experimental points ledger.

use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use libp2p::{
    PeerId,
    identity::{Keypair, PublicKey},
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const MAX_FILE_BYTES: u64 = 8_000_000;
pub const MAX_DURATION_SECONDS: f64 = 180.0;
pub const MAX_DIMENSION: u32 = 2160;
pub const MAX_LIBRARY_FILES: usize = 100;
pub const MAX_CATALOG_CLIPS: usize = 10_000;
pub const MICROS_PER_POINT: u64 = 1_000_000;
pub const INITIAL_MICROPOINTS: u64 = 100 * MICROS_PER_POINT;
pub const ROLL_TICKETS: u16 = 1023;
pub const MAX_ROLL_WAGER_MICROPOINTS: u64 = u64::MAX / 9;
const ROLL_CLIP_ID: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const MAX_STATE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TORRENT_BYTES: usize = 256 * 1024;
const MAX_LEDGER_RECEIPTS: usize = 100_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Clip {
    pub id: String,
    pub info_hash: String,
    pub torrent: String,
    pub title: String,
    pub extension: String,
    pub size: u64,
    pub duration_secs: f64,
    pub width: u32,
    pub height: u32,
}

impl Clip {
    fn same_content(&self, other: &Self) -> bool {
        self.info_hash == other.info_hash
            && self.torrent == other.torrent
            && self.extension == other.extension
            && self.size == other.size
            && self.width == other.width
            && self.height == other.height
            && (self.duration_secs - other.duration_secs).abs() < 0.001
    }

    /// Cheap envelope checks. Media bytes and torrent paths are independently
    /// validated by the transfer layer before any file is downloaded or played.
    pub fn validate(&self) -> Result<()> {
        validate_hex(&self.id, 32, "content hash")?;
        validate_hex(&self.info_hash, 20, "torrent info hash")?;
        ensure!(
            matches!(self.extension.as_str(), "mp4" | "webm"),
            "Only MP4 and WebM files are accepted"
        );
        ensure!(
            self.size > 0 && self.size <= MAX_FILE_BYTES,
            "Files must be at most 8 MB (8,000,000 bytes)"
        );
        ensure!(
            self.duration_secs.is_finite()
                && self.duration_secs > 0.0
                && self.duration_secs <= MAX_DURATION_SECONDS,
            "Videos must be at most 180 seconds"
        );
        ensure!(
            self.width > 0
                && self.height > 0
                && self.width <= MAX_DIMENSION
                && self.height <= MAX_DIMENSION,
            "Video dimensions must be at most 2160 × 2160"
        );
        ensure!(
            !self.title.is_empty()
                && self.title.len() <= 240
                && !self.title.chars().any(char::is_control),
            "Invalid clip title"
        );
        ensure!(
            self.torrent.len() <= MAX_TORRENT_BYTES * 4 / 3 + 4,
            "Torrent metadata is too large"
        );
        let metadata = STANDARD
            .decode(&self.torrent)
            .context("Invalid torrent encoding")?;
        ensure!(
            !metadata.is_empty() && metadata.len() <= MAX_TORRENT_BYTES,
            "Torrent metadata is too large or empty"
        );
        Ok(())
    }

    pub fn filename(&self) -> String {
        format!("{}.{}", self.id, self.extension)
    }
}

fn validate_hex(value: &str, length: usize, label: &str) -> Result<()> {
    ensure!(
        value.len() == length * 2
            && value
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
        "Invalid {label}"
    );
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub display_name: String,
    pub muted: bool,
    pub volume: f64,
    pub prefetch: bool,
    pub bootstrap_peers: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            display_name: "Anonymous peer".into(),
            muted: true,
            volume: 0.7,
            prefetch: true,
            bootstrap_peers: Vec::new(),
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.display_name.trim().is_empty()
                && self.display_name.len() <= 80
                && !self.display_name.chars().any(char::is_control),
            "Display name must contain 1–80 characters"
        );
        ensure!(
            self.volume.is_finite() && (0.0..=1.0).contains(&self.volume),
            "Volume must be between 0 and 1"
        );
        ensure!(
            self.bootstrap_peers.len() <= 32,
            "At most 32 bootstrap peers are allowed"
        );
        for peer in &self.bootstrap_peers {
            ensure!(peer.len() <= 512, "Peer address is too long");
            let address: libp2p::Multiaddr = peer.parse().context("Invalid peer multiaddress")?;
            ensure!(
                address
                    .iter()
                    .any(|p| matches!(p, libp2p::multiaddr::Protocol::P2p(_))),
                "Peer addresses must include /p2p/<peer-id>"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocalClip {
    pub clip_id: String,
    pub path: PathBuf,
    /// When this clip joined the permanent library. Legacy entries have no known date.
    #[serde(default)]
    pub added_at_ms: u64,
}

#[derive(Debug, Default, PartialEq)]
pub struct ClipStats {
    pub earned_micropoints: u64,
    pub rating_count: usize,
    rating_total: u64,
}

impl ClipStats {
    pub fn average_rating(&self) -> Option<f64> {
        (self.rating_count > 0).then(|| self.rating_total as f64 / self.rating_count as f64)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RollPayload {
    pub wager_micropoints: u64,
    pub ticket: u16,
}

impl RollPayload {
    pub fn payout_micropoints(&self) -> Result<u64> {
        ensure!(
            self.wager_micropoints > 0 && self.wager_micropoints <= MAX_ROLL_WAGER_MICROPOINTS,
            "Wager must be positive and within the points limit"
        );
        self.wager_micropoints
            .checked_mul(u64::from(roll_multiplier(self.ticket)?))
            .context("Roll payout exceeds the points limit")
    }
}

/// Exact integer sampling: the ten outcomes have weights 512, 256, ..., 1.
/// A uniform ticket therefore gives P(k) = 2^(-k) * 512/1023.
pub fn roll_multiplier(ticket: u16) -> Result<u8> {
    ensure!(ticket < ROLL_TICKETS, "Invalid roll ticket");
    let mut remaining = ticket;
    for multiplier in 0..=9 {
        let weight = 512 >> multiplier;
        if remaining < weight {
            return Ok(multiplier);
        }
        remaining -= weight;
    }
    unreachable!("All valid tickets fall into one of the ten buckets")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RatingPayload {
    pub sender: String,
    pub clip_id: String,
    pub score: u8,
    /// Sorted, distinct permanent seeders seen by the rater at rating time.
    pub seeders: Vec<String>,
    /// Observed peers including the rater, with a minimum of three.
    pub total_users: usize,
    pub sequence: u64,
    pub previous: Option<String>,
    // Omit this field on ratings to preserve the bytes of all legacy signatures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roll: Option<RollPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RatingReceipt {
    pub id: String,
    pub payload: RatingPayload,
    pub public_key: String,
    pub signature: String,
}

impl RatingReceipt {
    pub fn verify(&self) -> Result<()> {
        validate_hex(&self.id, 32, "receipt ID")?;
        validate_hex(&self.payload.clip_id, 32, "clip ID")?;
        ensure!(self.payload.score <= 9, "Rating must be between 0 and 9");
        ensure!(self.payload.sequence > 0, "Invalid receipt sequence");
        ensure!(
            self.public_key.len() <= 1024 && self.signature.len() <= 1024,
            "Signature is too large"
        );
        let key = PublicKey::try_decode_protobuf(&STANDARD.decode(&self.public_key)?)
            .context("Invalid signing key")?;
        ensure!(
            key.to_peer_id().to_string() == self.payload.sender,
            "Receipt identity does not match its signing key"
        );
        ensure!(
            self.payload.total_users >= 3 && self.payload.total_users <= 1_000_000,
            "Invalid total user count"
        );
        ensure!(
            (self.payload.score == 0 || !self.payload.seeders.is_empty())
                && self.payload.seeders.len() <= 10_000,
            "Invalid seeder count"
        );
        let mut previous: Option<&str> = None;
        for seeder in &self.payload.seeders {
            ensure!(
                seeder.len() <= 128 && seeder.parse::<PeerId>().is_ok(),
                "Invalid seeder identity"
            );
            ensure!(
                seeder != &self.payload.sender,
                "You cannot rate your own seeded clip"
            );
            ensure!(
                previous.is_none_or(|p| p < seeder.as_str()),
                "Seeder identities must be sorted and unique"
            );
            previous = Some(seeder);
        }
        reward_micropoints(
            self.payload.score,
            self.payload.seeders.len(),
            self.payload.total_users,
        )?;
        if let Some(roll) = &self.payload.roll {
            ensure!(
                self.payload.clip_id == ROLL_CLIP_ID
                    && self.payload.score == 0
                    && self.payload.seeders.is_empty()
                    && self.payload.total_users == 3,
                "Invalid roll receipt fields"
            );
            roll.payout_micropoints()?;
        }
        if let Some(previous) = &self.payload.previous {
            validate_hex(previous, 32, "previous receipt ID")?;
        }
        let bytes = receipt_bytes(&self.payload)?;
        ensure!(
            hex::encode(Sha256::digest(&bytes)) == self.id,
            "Receipt content hash is invalid"
        );
        ensure!(
            key.verify(&bytes, &STANDARD.decode(&self.signature)?),
            "Invalid rating signature"
        );
        Ok(())
    }

    pub fn reward_per_seeder(&self) -> Result<f64> {
        Ok(reward_micropoints(
            self.payload.score,
            self.payload.seeders.len(),
            self.payload.total_users,
        )? as f64
            / MICROS_PER_POINT as f64)
    }
}

fn receipt_bytes(payload: &RatingPayload) -> Result<Vec<u8>> {
    // Fixed domain and struct field order make signing independent of gossip
    // message signatures and prevent reuse as another protocol's message.
    let mut bytes = b"clipcrop/rating/v1\0".to_vec();
    bytes.extend(serde_json::to_vec(payload)?);
    Ok(bytes)
}

/// Each seeder receives this amount. Rewards intentionally create new points
/// when there are multiple seeders; they are not split between seeders.
pub fn reward_micropoints(score: u8, seeders: usize, total_users: usize) -> Result<u64> {
    ensure!(score <= 9, "Rating must be between 0 and 9");
    let population = total_users.max(3);
    if score == 0 && seeders == 0 {
        return Ok(0);
    }
    ensure!(
        seeders > 0 && seeders <= population,
        "Seeder count must be between 1 and total users"
    );
    let amount = u64::from(score) * MICROS_PER_POINT;
    if seeders == 1 {
        return Ok(amount);
    }
    let fraction = seeders as f64 / population as f64;
    Ok((amount as f64 * (1.0 - fraction.powi(2)).max(0.0).sqrt()).floor() as u64)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    pub profile_id: String,
    pub settings: Settings,
    pub catalog: BTreeMap<String, Clip>,
    pub library: BTreeMap<String, LocalClip>,
    pub watched: BTreeSet<String>,
    pub receipts: Vec<RatingReceipt>,
}

impl State {
    fn new(profile_id: String) -> Self {
        Self {
            version: 1,
            profile_id,
            settings: Settings::default(),
            catalog: BTreeMap::new(),
            library: BTreeMap::new(),
            watched: BTreeSet::new(),
            receipts: Vec::new(),
        }
    }
}

pub struct Store {
    pub data: State,
    path: PathBuf,
}

impl Store {
    pub fn open(path: PathBuf, identity_id: String) -> Result<Self> {
        ensure!(
            identity_id.parse::<PeerId>().is_ok(),
            "Invalid local identity"
        );
        // Recover the pre-replacement state after a Windows crash between the
        // backup rename and replacement. Never silently create another grant.
        let backup = path.with_extension("json.backup");
        if !path.exists() && backup.exists() {
            fs::rename(&backup, &path).context("Could not recover saved state")?;
        }
        let data = if path.exists() {
            ensure!(
                fs::metadata(&path)?.len() <= MAX_STATE_BYTES,
                "State file exceeds its size limit"
            );
            let data: State = serde_json::from_slice(&fs::read(&path)?)
                .context("State is corrupt; restore a backup instead of resetting your identity")?;
            ensure!(data.version == 1, "Unsupported state version");
            ensure!(
                data.profile_id == identity_id,
                "State belongs to a different identity"
            );
            data.settings.validate()?;
            ensure!(
                data.library.len() <= MAX_LIBRARY_FILES,
                "Library exceeds 100 clips"
            );
            ensure!(
                data.catalog.len() <= MAX_CATALOG_CLIPS,
                "Catalog exceeds 10,000 clips"
            );
            for (id, clip) in &data.catalog {
                ensure!(id == &clip.id, "Catalog key does not match clip");
                clip.validate()?;
            }
            for (id, local) in &data.library {
                ensure!(
                    id == &local.clip_id && data.catalog.contains_key(id),
                    "Invalid local library entry"
                );
            }
            ensure!(
                data.receipts.len() <= MAX_LEDGER_RECEIPTS,
                "Ledger exceeds its size limit"
            );
            data
        } else {
            State::new(identity_id)
        };
        let receipts = data.receipts.clone();
        let watched = data.watched.clone();
        let mut store = Self { data, path };
        store.data.receipts.clear();
        // Revalidate persisted receipts and recompute balances; no editable
        // balance field is trusted on disk. Existing order is causal order.
        for receipt in receipts {
            store.validate_receipt(&receipt)?;
            store.data.receipts.push(receipt);
        }
        store.data.watched = watched;
        store.save()?;
        Ok(store)
    }

    pub fn save(&self) -> Result<()> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let bytes = serde_json::to_vec_pretty(&self.data)?;
        ensure!(
            bytes.len() as u64 <= MAX_STATE_BYTES,
            "State file exceeds its size limit"
        );
        let temporary = self.path.with_extension("json.tmp");
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        // Windows does not replace an existing destination with rename. Keep a
        // recoverable backup until the replacement has completed there.
        #[cfg(windows)]
        {
            let backup = self.path.with_extension("json.backup");
            if self.path.exists() {
                fs::rename(&self.path, &backup)?;
            }
            if let Err(error) = fs::rename(&temporary, &self.path) {
                if backup.exists() {
                    let _ = fs::rename(&backup, &self.path);
                }
                return Err(error.into());
            }
            let _ = fs::remove_file(backup);
        }
        #[cfg(not(windows))]
        fs::rename(&temporary, &self.path)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    }

    fn commit(&mut self, before: State) -> Result<()> {
        if let Err(error) = self.save() {
            self.data = before;
            return Err(error);
        }
        Ok(())
    }

    pub fn set_settings(&mut self, settings: Settings) -> Result<()> {
        settings.validate()?;
        let before = self.data.clone();
        self.data.settings = settings;
        self.commit(before)
    }

    pub fn add_clip(&mut self, clip: Clip) -> Result<bool> {
        clip.validate()?;
        if let Some(existing) = self.data.catalog.get(&clip.id) {
            ensure!(
                existing.same_content(&clip),
                "Conflicting metadata for an existing content hash"
            );
            return Ok(false);
        }
        ensure!(
            self.data.catalog.len() < MAX_CATALOG_CLIPS,
            "Catalog is full"
        );
        let before = self.data.clone();
        self.data.catalog.insert(clip.id.clone(), clip);
        self.commit(before)?;
        Ok(true)
    }

    #[cfg(test)]
    pub fn add_library(&mut self, clip: Clip, path: PathBuf) -> Result<bool> {
        clip.validate()?;
        if self.data.library.contains_key(&clip.id) {
            return Ok(false);
        }
        ensure!(
            self.data.library.len() < MAX_LIBRARY_FILES,
            "You can seed at most 100 permanent clips"
        );
        if let Some(existing) = self.data.catalog.get(&clip.id) {
            ensure!(
                existing.same_content(&clip),
                "Conflicting metadata for an existing content hash"
            );
        }
        ensure!(
            self.data.catalog.contains_key(&clip.id) || self.data.catalog.len() < MAX_CATALOG_CLIPS,
            "Catalog is full"
        );
        let before = self.data.clone();
        let added_at_ms = self.next_library_added_at_ms();
        self.data.library.insert(
            clip.id.clone(),
            LocalClip {
                clip_id: clip.id.clone(),
                path,
                added_at_ms,
            },
        );
        self.data.catalog.entry(clip.id.clone()).or_insert(clip);
        self.commit(before)?;
        Ok(true)
    }

    /// Replace advertised metadata with independently verified local facts.
    ///
    /// The caller MUST first decode the complete file with the media validator,
    /// verify its SHA-256 content ID, and recreate/verify its canonical torrent.
    /// This method is a trust boundary: never call it for a peer advertisement.
    /// An earlier advert's false dimensions or duration must not permanently
    /// prevent the actual matching bytes from being accepted.
    pub fn add_verified_clip(&mut self, clip: Clip) -> Result<bool> {
        clip.validate()?;
        if self.data.catalog.get(&clip.id) == Some(&clip) {
            return Ok(false);
        }
        ensure!(
            self.data.catalog.contains_key(&clip.id) || self.data.catalog.len() < MAX_CATALOG_CLIPS,
            "Catalog is full"
        );
        let before = self.data.clone();
        self.data.catalog.insert(clip.id.clone(), clip);
        self.commit(before)?;
        Ok(true)
    }

    /// Commit a locally validated upload and its canonical metadata together.
    /// See `add_verified_clip` for the required byte/decode/torrent proof.
    /// Returns whether this adds a new permanent library entry.
    pub fn add_verified_library(&mut self, clip: Clip, path: PathBuf) -> Result<bool> {
        clip.validate()?;
        let is_new = !self.data.library.contains_key(&clip.id);
        ensure!(
            !is_new || self.data.library.len() < MAX_LIBRARY_FILES,
            "You can seed at most 100 permanent clips"
        );
        ensure!(
            self.data.catalog.contains_key(&clip.id) || self.data.catalog.len() < MAX_CATALOG_CLIPS,
            "Catalog is full"
        );
        let local = LocalClip {
            clip_id: clip.id.clone(),
            path,
            added_at_ms: self.data.library.get(&clip.id).map_or_else(
                || self.next_library_added_at_ms(),
                |local| local.added_at_ms,
            ),
        };
        if self.data.catalog.get(&clip.id) == Some(&clip)
            && self.data.library.get(&clip.id) == Some(&local)
        {
            return Ok(false);
        }
        let before = self.data.clone();
        self.data.library.insert(clip.id.clone(), local);
        self.data.catalog.insert(clip.id.clone(), clip);
        self.commit(before)?;
        Ok(is_new)
    }

    fn next_library_added_at_ms(&self) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let latest = self
            .data
            .library
            .values()
            .map(|local| local.added_at_ms)
            .max()
            .unwrap_or(0);
        now.max(latest.saturating_add(1))
    }

    /// Derive ratings and this profile's earnings from validated signed receipts.
    /// A zero is a free skip, so it contributes neither a rating nor points.
    pub fn clip_stats(&self) -> BTreeMap<&str, ClipStats> {
        let mut stats = BTreeMap::<&str, ClipStats>::new();
        for receipt in &self.data.receipts {
            let payload = &receipt.payload;
            if payload.roll.is_some() {
                continue;
            }
            let entry = stats.entry(&payload.clip_id).or_default();
            if payload.score > 0 {
                entry.rating_count += 1;
                entry.rating_total += u64::from(payload.score);
            }
            if payload.seeders.contains(&self.data.profile_id) {
                entry.earned_micropoints +=
                    reward_micropoints(payload.score, payload.seeders.len(), payload.total_users)
                        .expect("Stored receipts have already passed validation");
            }
        }
        stats
    }

    pub fn remove_library(&mut self, id: &str) -> Result<()> {
        let before = self.data.clone();
        self.data.library.remove(id);
        self.commit(before)
    }

    pub fn balance_micropoints(&self, identity: &str) -> u64 {
        let mut balance = i128::from(INITIAL_MICROPOINTS);
        for receipt in &self.data.receipts {
            if let Some(roll) = &receipt.payload.roll {
                if receipt.payload.sender == identity {
                    balance -= i128::from(roll.wager_micropoints);
                    balance += i128::from(
                        roll.payout_micropoints()
                            .expect("Stored rolls have already passed validation"),
                    );
                }
                continue;
            }
            if receipt.payload.sender == identity {
                balance -= i128::from(u64::from(receipt.payload.score) * MICROS_PER_POINT);
            }
            if receipt.payload.seeders.iter().any(|s| s == identity) {
                // Receipts entering the ledger have already passed validation.
                if let Ok(reward) = reward_micropoints(
                    receipt.payload.score,
                    receipt.payload.seeders.len(),
                    receipt.payload.total_users,
                ) {
                    balance += i128::from(reward);
                }
            }
        }
        balance.clamp(0, i128::from(u64::MAX)) as u64
    }

    pub fn balance(&self, identity: &str) -> f64 {
        self.balance_micropoints(identity) as f64 / MICROS_PER_POINT as f64
    }

    fn validate_receipt(&self, receipt: &RatingReceipt) -> Result<()> {
        receipt.verify()?;
        ensure!(
            self.data.receipts.len() < MAX_LEDGER_RECEIPTS,
            "Ledger is full"
        );
        let payload = &receipt.payload;
        if payload.roll.is_none() {
            ensure!(
                self.data.catalog.contains_key(&payload.clip_id),
                "Rating references an unknown clip"
            );
            ensure!(
                !self.data.receipts.iter().any(|r| r.payload.roll.is_none()
                    && r.payload.sender == payload.sender
                    && r.payload.clip_id == payload.clip_id),
                "This identity has already rated this clip"
            );
        }
        ensure!(
            !self.data.receipts.iter().any(|r| r.id == receipt.id),
            "Receipt has already been applied"
        );
        let previous = self
            .data
            .receipts
            .iter()
            .rev()
            .find(|r| r.payload.sender == payload.sender);
        let expected_sequence = previous.map_or(1, |r| r.payload.sequence.saturating_add(1));
        ensure!(
            payload.sequence == expected_sequence
                && payload.previous.as_deref() == previous.map(|r| r.id.as_str()),
            "Receipt is out of order or conflicts with the sender's history"
        );
        let balance = self.balance_micropoints(&payload.sender);
        if let Some(roll) = &payload.roll {
            let remainder = balance
                .checked_sub(roll.wager_micropoints)
                .context("Insufficient points")?;
            remainder
                .checked_add(roll.payout_micropoints()?)
                .context("Roll balance exceeds the points limit")?;
        } else {
            ensure!(
                balance >= u64::from(payload.score) * MICROS_PER_POINT,
                "Insufficient points"
            );
        }
        Ok(())
    }

    pub fn apply_receipt(&mut self, receipt: RatingReceipt) -> Result<bool> {
        if self.data.receipts.iter().any(|r| r.id == receipt.id) {
            return Ok(false);
        }
        self.validate_receipt(&receipt)?;
        let before = self.data.clone();
        if receipt.payload.roll.is_none() && receipt.payload.sender == self.data.profile_id {
            self.data.watched.insert(receipt.payload.clip_id.clone());
        }
        self.data.receipts.push(receipt);
        self.commit(before)?;
        Ok(true)
    }

    pub fn rate(
        &mut self,
        key: &Keypair,
        clip_id: &str,
        score: u8,
        mut seeders: Vec<String>,
        total_users: usize,
    ) -> Result<RatingReceipt> {
        let sender = key.public().to_peer_id().to_string();
        ensure!(
            sender == self.data.profile_id,
            "Signing key is not this profile's identity"
        );
        ensure!(
            !self.data.library.contains_key(clip_id),
            "You cannot rate your own seeded clip"
        );
        ensure!(
            !self.data.watched.contains(clip_id),
            "You have already watched this clip"
        );
        seeders.sort();
        seeders.dedup();
        let previous = self
            .data
            .receipts
            .iter()
            .rev()
            .find(|r| r.payload.sender == sender);
        let payload = RatingPayload {
            sender,
            clip_id: clip_id.to_owned(),
            score,
            seeders,
            total_users: total_users.max(3),
            sequence: previous.map_or(1, |r| r.payload.sequence.saturating_add(1)),
            previous: previous.map(|r| r.id.clone()),
            roll: None,
        };
        let bytes = receipt_bytes(&payload)?;
        let receipt = RatingReceipt {
            id: hex::encode(Sha256::digest(&bytes)),
            payload,
            public_key: STANDARD.encode(key.public().encode_protobuf()),
            signature: STANDARD.encode(key.sign(&bytes).context("Could not sign rating")?),
        };
        self.apply_receipt(receipt.clone())?;
        Ok(receipt)
    }

    pub fn roll(&mut self, key: &Keypair, wager_micropoints: u64) -> Result<RatingReceipt> {
        self.record_roll(
            key,
            RollPayload {
                wager_micropoints,
                ticket: rand::rng().random_range(0..ROLL_TICKETS),
            },
        )
    }

    fn record_roll(&mut self, key: &Keypair, roll: RollPayload) -> Result<RatingReceipt> {
        let sender = key.public().to_peer_id().to_string();
        ensure!(
            sender == self.data.profile_id,
            "Signing key is not this profile's identity"
        );
        let previous = self
            .data
            .receipts
            .iter()
            .rev()
            .find(|r| r.payload.sender == sender);
        let payload = RatingPayload {
            sender,
            clip_id: ROLL_CLIP_ID.to_owned(),
            score: 0,
            seeders: Vec::new(),
            total_users: 3,
            sequence: previous.map_or(1, |r| r.payload.sequence.saturating_add(1)),
            previous: previous.map(|r| r.id.clone()),
            roll: Some(roll),
        };
        let bytes = receipt_bytes(&payload)?;
        let receipt = RatingReceipt {
            id: hex::encode(Sha256::digest(&bytes)),
            payload,
            public_key: STANDARD.encode(key.public().encode_protobuf()),
            signature: STANDARD.encode(key.sign(&bytes).context("Could not sign roll")?),
        };
        self.apply_receipt(receipt.clone())?;
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Store, Keypair, Vec<String>) {
        let key = Keypair::generate_ed25519();
        let peers = (0..4)
            .map(|_| {
                Keypair::generate_ed25519()
                    .public()
                    .to_peer_id()
                    .to_string()
            })
            .collect();
        let directory = tempfile::Builder::new()
            .prefix("clipcrop-points-core-")
            .tempdir()
            .unwrap()
            .keep();
        let store = Store::open(
            directory.join("state.json"),
            key.public().to_peer_id().to_string(),
        )
        .unwrap();
        (store, key, peers)
    }

    fn clip(n: u8) -> Clip {
        Clip {
            id: format!("{n:064x}"),
            info_hash: format!("{n:040x}"),
            torrent: STANDARD.encode(b"test torrent envelope"),
            title: format!("Clip {n}"),
            extension: "mp4".into(),
            size: 1000,
            duration_secs: 3.0,
            width: 100,
            height: 100,
        }
    }

    fn resign(receipt: &mut RatingReceipt, key: &Keypair) {
        let bytes = receipt_bytes(&receipt.payload).unwrap();
        receipt.id = hex::encode(Sha256::digest(&bytes));
        receipt.signature = STANDARD.encode(key.sign(&bytes).unwrap());
    }

    #[test]
    fn roll_tickets_have_the_exact_requested_distribution() {
        let mut counts = [0; 10];
        for ticket in 0..ROLL_TICKETS {
            counts[usize::from(roll_multiplier(ticket).unwrap())] += 1;
        }
        assert_eq!(counts, [512, 256, 128, 64, 32, 16, 8, 4, 2, 1]);
        assert_eq!(roll_multiplier(0).unwrap(), 0);
        assert_eq!(roll_multiplier(1022).unwrap(), 9);
        assert!(roll_multiplier(1023).is_err());
        assert!(roll_multiplier(u16::MAX).is_err());
    }

    #[test]
    fn every_roll_pays_wager_times_multiplier_without_marking_watched() {
        let mut ticket = 0;
        for multiplier in 0..=9 {
            let (mut store, key, peers) = fixture();
            let wager = 1_234_567;
            let receipt = store
                .record_roll(
                    &key,
                    RollPayload {
                        wager_micropoints: wager,
                        ticket,
                    },
                )
                .unwrap();
            receipt.verify().unwrap();
            assert_eq!(
                store.balance_micropoints(&store.data.profile_id),
                INITIAL_MICROPOINTS - wager + wager * multiplier
            );
            assert_eq!(store.balance_micropoints(&peers[0]), INITIAL_MICROPOINTS);
            assert!(store.data.watched.is_empty());
            assert!(store.clip_stats().is_empty());
            let reopened = Store::open(store.path.clone(), store.data.profile_id.clone()).unwrap();
            assert_eq!(
                reopened.balance_micropoints(&reopened.data.profile_id),
                store.balance_micropoints(&store.data.profile_id)
            );
            assert_eq!(reopened.data.receipts, store.data.receipts);
            assert!(!store.apply_receipt(receipt).unwrap());
            ticket += 512 >> multiplier;
            let _ = fs::remove_dir_all(store.path.parent().unwrap());
        }
    }

    #[test]
    fn rolls_and_ratings_share_one_replayable_history() {
        let (mut sender, key, peers) = fixture();
        let (mut receiver, _, _) = fixture();
        for store in [&mut sender, &mut receiver] {
            store.add_clip(clip(1)).unwrap();
            store.add_clip(clip(2)).unwrap();
        }
        let first = sender
            .rate(&key, &clip(1).id, 5, vec![peers[0].clone()], 3)
            .unwrap();
        // A legacy rating keeps exactly the old JSON fields and signature bytes.
        let encoded = serde_json::to_value(&first).unwrap();
        assert!(encoded["payload"].get("roll").is_none());
        let decoded: RatingReceipt = serde_json::from_value(encoded).unwrap();
        decoded.verify().unwrap();
        assert_eq!(decoded, first);
        let roll = sender
            .record_roll(
                &key,
                RollPayload {
                    wager_micropoints: 10 * MICROS_PER_POINT,
                    ticket: 1022,
                },
            )
            .unwrap();
        let second_roll = sender
            .record_roll(
                &key,
                RollPayload {
                    wager_micropoints: MICROS_PER_POINT,
                    ticket: 0,
                },
            )
            .unwrap();
        let last = sender
            .rate(&key, &clip(2).id, 7, vec![peers[0].clone()], 3)
            .unwrap();
        assert_eq!(last.payload.sequence, 4);
        assert_eq!(last.payload.previous.as_ref(), Some(&second_roll.id));
        assert!(receiver.apply_receipt(roll.clone()).is_err());
        assert!(receiver.apply_receipt(last.clone()).is_err());
        for receipt in [first, roll, second_roll, last] {
            receiver.apply_receipt(receipt).unwrap();
        }
        assert_eq!(sender.balance(&sender.data.profile_id), 167.0);
        assert_eq!(receiver.balance(&sender.data.profile_id), 167.0);
        assert_eq!(receiver.balance(&peers[0]), 112.0);
        assert_eq!(sender.data.watched.len(), 2);
        assert!(receiver.data.watched.is_empty());
        assert_eq!(sender.clip_stats().len(), 2);
        for store in [sender, receiver] {
            let reopened = Store::open(store.path.clone(), store.data.profile_id.clone()).unwrap();
            assert_eq!(reopened.data.receipts, store.data.receipts);
            let _ = fs::remove_dir_all(store.path.parent().unwrap());
        }
    }

    #[test]
    fn invalid_rolls_never_change_the_ledger() {
        let (mut store, key, _) = fixture();
        for roll in [
            RollPayload {
                wager_micropoints: 0,
                ticket: 0,
            },
            RollPayload {
                wager_micropoints: INITIAL_MICROPOINTS + 1,
                ticket: 0,
            },
            RollPayload {
                wager_micropoints: MAX_ROLL_WAGER_MICROPOINTS + 1,
                ticket: 1022,
            },
            RollPayload {
                wager_micropoints: 1,
                ticket: ROLL_TICKETS,
            },
        ] {
            assert!(store.record_roll(&key, roll).is_err());
        }
        assert!(
            store
                .roll(&Keypair::generate_ed25519(), MICROS_PER_POINT)
                .is_err()
        );
        assert_eq!(store.balance(&store.data.profile_id), 100.0);
        assert!(store.data.receipts.is_empty());
        assert!(store.data.watched.is_empty());
        let receipt = store.roll(&key, MICROS_PER_POINT).unwrap();
        let mut changed = receipt.clone();
        let ticket = &mut changed.payload.roll.as_mut().unwrap().ticket;
        *ticket = (*ticket + 1) % ROLL_TICKETS;
        assert!(changed.verify().is_err());
        let mut changed = receipt.clone();
        changed.payload.clip_id = clip(1).id;
        resign(&mut changed, &key);
        assert!(changed.verify().is_err());
        let mut changed = receipt.clone();
        changed.payload.total_users = 4;
        resign(&mut changed, &key);
        assert!(changed.verify().is_err());
        let mut changed = receipt;
        changed.payload.roll.as_mut().unwrap().wager_micropoints = 0;
        resign(&mut changed, &key);
        assert!(changed.verify().is_err());
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    #[test]
    fn rolls_reject_balance_overflow_and_roll_back_failed_persistence() {
        let (mut store, key, _) = fixture();
        loop {
            let balance = store.balance_micropoints(&store.data.profile_id);
            let wager = balance.min(MAX_ROLL_WAGER_MICROPOINTS);
            let outcome = store.record_roll(
                &key,
                RollPayload {
                    wager_micropoints: wager,
                    ticket: 1022,
                },
            );
            if balance.checked_add(wager * 8).is_none() {
                assert!(outcome.is_err());
                assert_eq!(store.balance_micropoints(&store.data.profile_id), balance);
                break;
            }
            outcome.unwrap();
        }
        let before = store.data.receipts.clone();
        let balance = store.balance_micropoints(&store.data.profile_id);
        let directory = store.path.parent().unwrap().to_path_buf();
        let blocker = directory.join("regular-file");
        fs::write(&blocker, b"not a directory").unwrap();
        store.path = blocker.join("state.json");
        assert!(store.roll(&key, 1).is_err());
        assert_eq!(store.balance_micropoints(&store.data.profile_id), balance);
        assert_eq!(store.data.receipts, before);
        assert!(store.data.watched.is_empty());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn payout_formula_has_lone_seeder_exception_and_individual_rewards() {
        assert_eq!(reward_micropoints(5, 1, 3).unwrap(), 5_000_000);
        assert_eq!(reward_micropoints(5, 2, 3).unwrap(), 3_726_779);
        assert!(2 * reward_micropoints(5, 2, 3).unwrap() > 5_000_000);
        assert_eq!(reward_micropoints(9, 3, 3).unwrap(), 0);
        assert_eq!(reward_micropoints(0, 2, 3).unwrap(), 0);
        assert_eq!(reward_micropoints(0, 0, 3).unwrap(), 0);
        assert!(reward_micropoints(10, 1, 3).is_err());
        assert!(reward_micropoints(5, 4, 3).is_err());
        assert!(reward_micropoints(5, 0, 3).is_err());
    }

    #[test]
    fn clip_stats_separate_rating_quality_from_earned_points_and_survive_restart() {
        let (mut receiver, _, peers) = fixture();
        let (mut first, first_key, _) = fixture();
        let (mut second, second_key, _) = fixture();
        for store in [&mut receiver, &mut first, &mut second] {
            for n in 1..=4 {
                store.add_clip(clip(n)).unwrap();
            }
        }
        assert!(receiver.clip_stats().is_empty());
        let local = receiver.data.profile_id.clone();
        let low = first
            .rate(&first_key, &clip(1).id, 4, vec![local.clone()], 3)
            .unwrap();
        receiver.apply_receipt(low.clone()).unwrap();
        receiver
            .apply_receipt(
                second
                    .rate(&second_key, &clip(1).id, 4, vec![local.clone()], 3)
                    .unwrap(),
            )
            .unwrap();
        receiver
            .apply_receipt(
                first
                    .rate(
                        &first_key,
                        &clip(2).id,
                        9,
                        vec![local.clone(), peers[0].clone()],
                        3,
                    )
                    .unwrap(),
            )
            .unwrap();
        receiver
            .apply_receipt(first.rate(&first_key, &clip(3).id, 0, vec![], 3).unwrap())
            .unwrap();
        receiver
            .apply_receipt(
                first
                    .rate(
                        &first_key,
                        &clip(4).id,
                        9,
                        vec![local, peers[0].clone(), peers[1].clone()],
                        3,
                    )
                    .unwrap(),
            )
            .unwrap();
        // Ratings paid to other seeders count toward quality, not our earnings.
        receiver
            .apply_receipt(
                second
                    .rate(&second_key, &clip(2).id, 1, vec![peers[0].clone()], 3)
                    .unwrap(),
            )
            .unwrap();
        assert!(!receiver.apply_receipt(low.clone()).unwrap());
        let mut tampered = low;
        tampered.id = "f".repeat(64);
        tampered.payload.score = 9;
        assert!(receiver.apply_receipt(tampered).is_err());
        let stats = receiver.clip_stats();
        assert_eq!(stats[clip(1).id.as_str()].average_rating(), Some(4.0));
        assert_eq!(stats[clip(1).id.as_str()].rating_count, 2);
        assert_eq!(stats[clip(1).id.as_str()].earned_micropoints, 8_000_000);
        assert_eq!(stats[clip(2).id.as_str()].average_rating(), Some(5.0));
        assert_eq!(stats[clip(2).id.as_str()].rating_count, 2);
        assert_eq!(stats[clip(2).id.as_str()].earned_micropoints, 6_708_203);
        assert_eq!(stats[clip(3).id.as_str()], ClipStats::default());
        assert_eq!(stats[clip(3).id.as_str()].average_rating(), None);
        assert_eq!(stats[clip(4).id.as_str()].average_rating(), Some(9.0));
        assert_eq!(stats[clip(4).id.as_str()].earned_micropoints, 0);
        let reopened =
            Store::open(receiver.path.clone(), receiver.data.profile_id.clone()).unwrap();
        assert_eq!(stats, reopened.clip_stats());
        for store in [receiver, first, second] {
            let _ = fs::remove_dir_all(store.path.parent().unwrap());
        }
    }

    #[test]
    fn library_recency_survives_repairs_restart_and_legacy_state() {
        let (mut store, _, _) = fixture();
        let first = clip(1);
        store
            .add_verified_library(first.clone(), PathBuf::from(first.filename()))
            .unwrap();
        let added_at_ms = store.data.library[&first.id].added_at_ms;
        assert!(added_at_ms > 0);
        assert!(
            !store
                .add_verified_library(first.clone(), PathBuf::from(first.filename()))
                .unwrap()
        );
        // Startup revalidates metadata and can repair paths after moving a profile.
        let repaired_path = PathBuf::from("moved").join(first.filename());
        assert!(
            !store
                .add_verified_library(first.clone(), repaired_path)
                .unwrap()
        );
        assert_eq!(store.data.library[&first.id].added_at_ms, added_at_ms);
        let second = clip(2);
        store
            .add_verified_library(second.clone(), PathBuf::from(second.filename()))
            .unwrap();
        assert!(store.data.library[&second.id].added_at_ms > added_at_ms);
        let reopened = Store::open(store.path.clone(), store.data.profile_id.clone()).unwrap();
        assert_eq!(reopened.data.library, store.data.library);

        let mut legacy = serde_json::to_value(&store.data).unwrap();
        for entry in legacy["library"].as_object_mut().unwrap().values_mut() {
            entry.as_object_mut().unwrap().remove("added_at_ms");
        }
        fs::write(&store.path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let mut migrated = Store::open(store.path.clone(), store.data.profile_id.clone()).unwrap();
        assert!(
            migrated
                .data
                .library
                .values()
                .all(|local| local.added_at_ms == 0)
        );
        assert!(
            !migrated
                .add_verified_library(first.clone(), PathBuf::from(first.filename()))
                .unwrap()
        );
        assert_eq!(migrated.data.library[&first.id].added_at_ms, 0);
        migrated.remove_library(&first.id).unwrap();
        migrated
            .add_verified_library(first.clone(), PathBuf::from(first.filename()))
            .unwrap();
        assert!(migrated.data.library[&first.id].added_at_ms > 0);
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    #[test]
    fn signed_spend_credit_replay_and_restart() {
        let (mut store, key, peers) = fixture();
        store.add_clip(clip(1)).unwrap();
        let receipt = store
            .rate(&key, &clip(1).id, 5, vec![peers[0].clone()], 3)
            .unwrap();
        assert_eq!(store.balance(&store.data.profile_id), 95.0);
        assert_eq!(store.balance(&peers[0]), 105.0);
        assert!(!store.apply_receipt(receipt).unwrap());
        assert!(
            store
                .rate(&key, &clip(1).id, 4, vec![peers[0].clone()], 3)
                .is_err()
        );
        let reopened = Store::open(store.path.clone(), store.data.profile_id.clone()).unwrap();
        assert_eq!(reopened.balance(&reopened.data.profile_id), 95.0);
        assert!(reopened.data.watched.contains(&clip(1).id));
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    #[test]
    fn tampering_and_self_rating_are_rejected() {
        let (mut store, key, peers) = fixture();
        store.add_clip(clip(1)).unwrap();
        assert!(
            store
                .rate(&key, &clip(1).id, 5, vec![store.data.profile_id.clone()], 3)
                .is_err()
        );
        let receipt = store
            .rate(&key, &clip(1).id, 5, vec![peers[0].clone()], 3)
            .unwrap();
        let mut changed = receipt.clone();
        changed.payload.score = 9;
        assert!(changed.verify().is_err());
        let mut changed = receipt;
        changed.payload.sender = peers[1].clone();
        assert!(changed.verify().is_err());
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    #[test]
    fn overspending_is_rejected_without_marking_watched() {
        let (mut store, key, peers) = fixture();
        for n in 1..=12 {
            store.add_clip(clip(n)).unwrap();
            if n <= 11 {
                store
                    .rate(&key, &clip(n).id, 9, vec![peers[0].clone()], 3)
                    .unwrap();
            }
        }
        assert_eq!(store.balance(&store.data.profile_id), 1.0);
        assert!(
            store
                .rate(&key, &clip(12).id, 2, vec![peers[0].clone()], 3)
                .is_err()
        );
        assert!(!store.data.watched.contains(&clip(12).id));
        store.rate(&key, &clip(12).id, 0, Vec::new(), 3).unwrap();
        assert_eq!(store.balance(&store.data.profile_id), 1.0);
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    #[test]
    fn limits_and_conflicting_content_are_rejected() {
        let (mut store, _, _) = fixture();
        let mut value = clip(1);
        value.size = MAX_FILE_BYTES + 1;
        assert!(store.add_clip(value).is_err());
        let mut value = clip(1);
        value.duration_secs = f64::NAN;
        assert!(store.add_clip(value).is_err());
        store.add_clip(clip(1)).unwrap();
        let mut value = clip(1);
        value.info_hash = "f".repeat(40);
        assert!(store.add_clip(value).is_err());
        for n in 1..=100 {
            store
                .add_library(clip(n), PathBuf::from(format!("{n}.mp4")))
                .unwrap();
        }
        assert!(
            store
                .add_library(clip(101), PathBuf::from("101.mp4"))
                .is_err()
        );
        assert!(!store.add_library(clip(1), PathBuf::from("1.mp4")).unwrap());
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    #[test]
    fn imported_receipts_require_an_unbroken_sender_chain() {
        let (mut sender, key, peers) = fixture();
        let (mut receiver, _, _) = fixture();
        for n in 1..=2 {
            sender.add_clip(clip(n)).unwrap();
            receiver.add_clip(clip(n)).unwrap();
        }
        let first = sender
            .rate(&key, &clip(1).id, 4, vec![peers[0].clone()], 3)
            .unwrap();
        let second = sender
            .rate(&key, &clip(2).id, 7, vec![peers[0].clone()], 3)
            .unwrap();
        assert!(receiver.apply_receipt(second.clone()).is_err());
        receiver.apply_receipt(first).unwrap();
        receiver.apply_receipt(second).unwrap();
        assert_eq!(receiver.balance(&sender.data.profile_id), 89.0);
        assert_eq!(receiver.balance(&peers[0]), 111.0);
        let _ = fs::remove_dir_all(sender.path.parent().unwrap());
        let _ = fs::remove_dir_all(receiver.path.parent().unwrap());
    }

    #[test]
    fn persistence_failure_does_not_spend_or_mark_watched() {
        let (mut store, key, peers) = fixture();
        store.add_clip(clip(1)).unwrap();
        let directory = store.path.parent().unwrap().to_path_buf();
        let blocker = directory.join("regular-file");
        fs::write(&blocker, b"not a directory").unwrap();
        store.path = blocker.join("state.json");
        assert!(
            store
                .rate(&key, &clip(1).id, 5, vec![peers[0].clone()], 3)
                .is_err()
        );
        assert_eq!(store.balance(&store.data.profile_id), 100.0);
        assert!(store.data.receipts.is_empty());
        assert!(store.data.watched.is_empty());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn corrupt_state_never_silently_resets_a_profile() {
        let (store, _, _) = fixture();
        fs::write(&store.path, b"broken JSON").unwrap();
        assert!(Store::open(store.path.clone(), store.data.profile_id.clone()).is_err());
        assert_eq!(fs::read(&store.path).unwrap(), b"broken JSON");
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    #[test]
    fn verified_upload_repairs_first_seen_metadata_poisoning_atomically() {
        let (mut store, _, _) = fixture();
        let verified = clip(1);
        let mut poisoned = verified.clone();
        poisoned.width = 50;
        poisoned.duration_secs = 17.0;
        store.add_clip(poisoned.clone()).unwrap();
        let path = PathBuf::from(verified.filename());
        // An untrusted second advert cannot replace the first advert.
        assert!(store.add_clip(verified.clone()).is_err());
        assert!(store.add_library(verified.clone(), path.clone()).is_err());
        // Independent proof of the actual bytes uses the explicit trusted API.
        assert!(
            store
                .add_verified_library(verified.clone(), path.clone())
                .unwrap()
        );
        assert_eq!(store.data.catalog.get(&verified.id), Some(&verified));
        assert_eq!(store.data.library.get(&verified.id).unwrap().path, path);
        assert!(store.add_clip(poisoned).is_err());
        let reopened = Store::open(store.path.clone(), store.data.profile_id.clone()).unwrap();
        assert_eq!(reopened.data.catalog.get(&verified.id), Some(&verified));
        assert_eq!(reopened.data.library.len(), 1);

        let second = clip(2);
        let original_path = store.path.clone();
        let blocker = original_path.parent().unwrap().join("blocked");
        fs::write(&blocker, b"not a directory").unwrap();
        store.path = blocker.join("state.json");
        assert!(
            store
                .add_verified_library(second.clone(), PathBuf::from(second.filename()))
                .is_err()
        );
        assert!(!store.data.catalog.contains_key(&second.id));
        assert!(!store.data.library.contains_key(&second.id));
        let _ = fs::remove_dir_all(original_path.parent().unwrap());
    }

    #[test]
    fn catalog_cap_applies_to_new_entries_but_allows_verified_repairs() {
        let (mut store, _, _) = fixture();
        for number in 0..MAX_CATALOG_CLIPS {
            let mut advertised = clip(1);
            advertised.id = format!("{number:064x}");
            store.data.catalog.insert(advertised.id.clone(), advertised);
        }
        let mut extra = clip(1);
        extra.id = format!("{:064x}", MAX_CATALOG_CLIPS);
        assert!(store.add_clip(extra.clone()).is_err());
        assert!(
            store
                .add_library(extra.clone(), PathBuf::from(extra.filename()))
                .is_err()
        );
        assert!(store.add_verified_clip(extra.clone()).is_err());
        assert!(
            store
                .add_verified_library(extra.clone(), PathBuf::from(extra.filename()))
                .is_err()
        );
        assert_eq!(store.data.catalog.len(), MAX_CATALOG_CLIPS);
        // Independently verified facts can correct an existing entry at capacity.
        let mut repaired = clip(1);
        repaired.width = 101;
        assert!(store.add_verified_clip(repaired.clone()).unwrap());
        assert_eq!(store.data.catalog.get(&repaired.id), Some(&repaired));
        assert_eq!(store.data.catalog.len(), MAX_CATALOG_CLIPS);
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }
}
