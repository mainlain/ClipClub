use crate::{
    core::{
        Clip, MAX_FILE_BYTES, MAX_LIBRARY_FILES, MAX_ROLL_WAGER_MICROPOINTS, MICROS_PER_POINT,
        RatingReceipt, Settings, Store, roll_multiplier,
    },
    media::{self, Limits},
    peer::{self, PeerEvent, PeerHandle},
    torrent::{self, PreparedTorrent, TorrentEngine},
};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use libp2p::identity::Keypair;
use librqbit::ManagedTorrent;
use rand::seq::IteratorRandom;
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

pub struct App {
    pub root: PathBuf,
    pub store: Mutex<Store>,
    pub mesh: PeerHandle,
    identity: Keypair,
    engine: TorrentEngine,
    downloader: TorrentEngine,
    roll_downloader: TorrentEngine,
    roll_preparation: Mutex<()>,
    probe: PathBuf,
    runtime: Mutex<Runtime>,
    /// Serialize user mutations across disk and network operations.
    mutation: Mutex<()>,
    wake: Notify,
    pub shutdown: CancellationToken,
    pub updates: Arc<crate::update::Updater>,
    pub p2p_port: u16,
    pub torrent_port: u16,
}
struct Presence {
    name: String,
    seeded: Vec<String>,
    endpoint: Option<SocketAddr>,
    seen: Instant,
}
#[derive(Clone, Serialize)]
struct RollResult {
    id: String,
    multiplier: u8,
    wager: f64,
    payout: f64,
    balance: f64,
    clip: Option<Clip>,
    message: Option<String>,
}

struct RollClip {
    clip: Clip,
    source: Option<PathBuf>,
    peers: Vec<SocketAddr>,
}

#[derive(Default)]
struct Runtime {
    peers: BTreeMap<String, Presence>,
    addresses: BTreeSet<String>,
    ready: VecDeque<String>,
    session: bool,
    temporary: BTreeMap<String, Arc<ManagedTorrent>>,
    permanent: BTreeMap<String, Arc<ManagedTorrent>>,
    downloading: Option<(String, Arc<ManagedTorrent>)>,
    failed: BTreeMap<String, Instant>,
    pending: VecDeque<RatingReceipt>,
    notice: Option<String>,
    replaying: bool,
    replay_requested: bool,
    roll: Option<RollResult>,
    roll_cancel: Option<CancellationToken>,
    roll_file: Option<String>,
}

fn wager_micropoints(wager: f64) -> Result<u64> {
    let scaled = wager * MICROS_PER_POINT as f64;
    ensure!(
        wager.is_finite() && wager > 0.0 && scaled >= 1.0,
        "Wager at least 0.000001 points"
    );
    ensure!(
        scaled < MAX_ROLL_WAGER_MICROPOINTS as f64,
        "Wager is too large"
    );
    ensure!(
        (scaled - scaled.round()).abs() <= 0.000001,
        "Use at most six decimal places for your wager"
    );
    Ok(scaled.round() as u64)
}

fn roll_rating(average: f64) -> u8 {
    average.round().clamp(1.0, 9.0) as u8
}

pub fn prepared(clip: &Clip) -> Result<PreparedTorrent> {
    clip.validate()?;
    let bytes = STANDARD.decode(&clip.torrent)?;
    let torrent = torrent::validate_torrent(&bytes, &clip.id, &clip.extension, clip.size)?;
    ensure!(
        torrent.info_hash == clip.info_hash,
        "Torrent info hash does not match the advertised clip"
    );
    Ok(torrent)
}

impl App {
    pub async fn start(root: PathBuf, p2p_port: u16, torrent_port: u16) -> Result<Arc<Self>> {
        tokio::fs::create_dir_all(root.join("clips")).await?;
        tokio::fs::create_dir_all(root.join("cache")).await?;
        tokio::fs::create_dir_all(root.join("incoming")).await?;
        tokio::fs::create_dir_all(root.join("roll-cache")).await?;
        // Cache has no identity or wallet state and is deliberately session-local.
        for folder in ["cache", "incoming", "roll-cache"] {
            let mut files = tokio::fs::read_dir(root.join(folder)).await?;
            while let Some(file) = files.next_entry().await? {
                if file.file_type().await?.is_file() {
                    tokio::fs::remove_file(file.path()).await?;
                }
            }
        }
        let identity = peer::load_identity(&root)?;
        let store = Store::open(
            root.join("state.json"),
            identity.public().to_peer_id().to_string(),
        )?;
        let probe = media::install_probe(&root)?;
        let engine = TorrentEngine::new(root.join("clips"), torrent_port).await?;
        let torrent_port = engine.port().unwrap_or(torrent_port);
        let downloader = TorrentEngine::downloader(root.join("cache")).await?;
        let roll_downloader = TorrentEngine::downloader(root.join("roll-cache")).await?;
        let (mesh, mut events) = peer::start(
            identity.clone(),
            p2p_port,
            store.data.settings.bootstrap_peers.clone(),
        )
        .await?;
        let app = Arc::new(Self {
            updates: Arc::new(crate::update::Updater::new(root.clone())),
            root,
            store: Mutex::new(store),
            mesh,
            identity,
            engine,
            downloader,
            roll_downloader,
            roll_preparation: Mutex::new(()),
            probe,
            runtime: Mutex::new(Runtime::default()),
            mutation: Mutex::new(()),
            wake: Notify::new(),
            shutdown: CancellationToken::new(),
            p2p_port,
            torrent_port,
        });
        // Validate managed files before resuming; never blindly trust saved paths or bytes.
        let library = app.store.lock().await.data.library.clone();
        for (id, _) in library {
            let clip = app
                .store
                .lock()
                .await
                .data
                .catalog
                .get(&id)
                .cloned()
                .context("Missing library metadata")?;
            let expected = app.root.join("clips").join(clip.filename());
            let result = async {
                let video =
                    media::validate_video(&expected, &Limits::default(), &app.probe).await?;
                ensure!(video.id == clip.id, "Saved clip hash changed");
                let bytes = tokio::fs::read(&expected).await?;
                let actual = torrent::create_torrent(&bytes, &video.extension)?;
                ensure!(
                    actual.info_hash == clip.info_hash,
                    "Saved torrent metadata changed"
                );
                // A portable profile can move between folders or operating systems.
                // Rebuild its path from the verified content ID, never a saved path.
                let mut verified = clip.clone();
                verified.width = video.width;
                verified.height = video.height;
                verified.duration_secs = video.duration_secs;
                app.store
                    .lock()
                    .await
                    .add_verified_library(verified, expected.clone())?;
                let handle = app
                    .engine
                    .add(&actual, &app.root.join("clips"), vec![], false)
                    .await?;
                app.runtime
                    .lock()
                    .await
                    .permanent
                    .insert(id.clone(), handle);
                Ok::<_, anyhow::Error>(())
            }
            .await;
            if let Err(error) = result {
                app.notice(&format!("Could not resume {}: {error}", clip.title))
                    .await;
            }
        }
        app.announce().await;
        let task_app = app.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = task_app.shutdown.cancelled() => break,
                    event = events.recv() => match event { Some(event) => task_app.peer_event(event).await, None => break }
                }
            }
        });
        let task_app = app.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = task_app.shutdown.cancelled() => break,
                    _ = task_app.updates.check() => (),
                }
                tokio::select! {
                    _ = task_app.shutdown.cancelled() => break,
                    _ = tokio::time::sleep(crate::update::CHECK_INTERVAL) => (),
                }
            }
        });
        let task_app = app.clone();
        tokio::spawn(async move {
            task_app.prefetch().await;
        });
        let task_app = app.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(20));
            loop {
                tokio::select! {
                    _ = task_app.shutdown.cancelled() => break,
                    _ = interval.tick() => { task_app.runtime.lock().await.peers.retain(|_, peer| peer.seen.elapsed() < Duration::from_secs(65)); task_app.announce().await; }
                }
            }
        });
        Ok(app)
    }
    async fn notice(&self, message: &str) {
        self.runtime.lock().await.notice = Some(message.to_owned());
        eprintln!("{message}");
    }
    async fn announce(&self) {
        let store = self.store.lock().await;
        let runtime = self.runtime.lock().await;
        let ids: Vec<String> = runtime
            .permanent
            .keys()
            .chain(runtime.temporary.keys())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let clips = ids
            .iter()
            .filter_map(|id| store.data.catalog.get(id).cloned())
            .collect();
        let name = store.data.settings.display_name.clone();
        drop(runtime);
        drop(store);
        if let Err(error) = self
            .mesh
            .announce(clips, ids, name, self.torrent_port)
            .await
        {
            self.notice(&error.to_string()).await;
        }
    }
    async fn peer_event(self: &Arc<Self>, event: PeerEvent) {
        match event {
            PeerEvent::Listening(address) => {
                self.runtime.lock().await.addresses.insert(address);
            }
            PeerEvent::Connected(peer) => {
                if peer == self.mesh.peer_id {
                    return;
                }
                self.announce().await;
                let mut rt = self.runtime.lock().await;
                rt.replay_requested = true;
                if !rt.replaying {
                    rt.replaying = true;
                    let app = self.clone();
                    tokio::spawn(async move {
                        loop {
                            app.runtime.lock().await.replay_requested = false;
                            let data = app.store.lock().await.data.clone();
                            for receipt in data.receipts {
                                if app.shutdown.is_cancelled() {
                                    return;
                                }
                                // Historical receipts need their associated clip descriptor, too.
                                if let Some(clip) = data.catalog.get(&receipt.payload.clip_id) {
                                    let _ = app.mesh.advert(clip.clone()).await;
                                }
                                let _ = app.mesh.receipt(receipt).await;
                                tokio::time::sleep(Duration::from_millis(150)).await;
                            }
                            let mut rt = app.runtime.lock().await;
                            if !rt.replay_requested {
                                rt.replaying = false;
                                break;
                            }
                        }
                    });
                }
            }
            PeerEvent::Disconnected(peer) => {
                self.runtime.lock().await.peers.remove(&peer);
            }
            PeerEvent::Presence {
                peer,
                name,
                seeded,
                torrent_addr,
            } => {
                self.runtime.lock().await.peers.insert(
                    peer,
                    Presence {
                        name,
                        seeded,
                        endpoint: torrent_addr,
                        seen: Instant::now(),
                    },
                );
                self.wake.notify_one();
            }
            PeerEvent::Advert { peer, clip } => {
                if peer != self.mesh.peer_id && prepared(&clip).is_ok() {
                    let _ = self.store.lock().await.add_clip(clip);
                    self.retry_receipts().await;
                    self.wake.notify_one();
                }
            }
            PeerEvent::Receipt(receipt) => {
                let mut store = self.store.lock().await;
                match store.apply_receipt(receipt.clone()) {
                    Ok(true) => {
                        drop(store);
                        self.retry_receipts().await;
                    }
                    Ok(false) => (),
                    Err(_) => {
                        drop(store);
                        let mut rt = self.runtime.lock().await;
                        if rt.pending.len() < 2000 && !rt.pending.iter().any(|r| r.id == receipt.id)
                        {
                            rt.pending.push_back(receipt);
                        }
                    }
                }
            }
            PeerEvent::Error(error) => self.notice(&error).await,
        }
    }
    async fn retry_receipts(&self) {
        let mut pending = std::mem::take(&mut self.runtime.lock().await.pending);
        let mut store = self.store.lock().await;
        loop {
            let before = pending.len();
            pending.retain(|receipt| store.apply_receipt(receipt.clone()).is_err());
            if pending.is_empty() || pending.len() == before {
                break;
            }
        }
        drop(store);
        self.runtime.lock().await.pending.extend(pending);
    }
    pub async fn state(&self) -> Value {
        let update = self.updates.status().await;
        let store = self.store.lock().await;
        let rt = self.runtime.lock().await;
        let local = &store.data.profile_id;
        let stats = store.clip_stats();
        let earned = stats.values().map(|s| s.earned_micropoints).sum::<u64>() as f64
            / MICROS_PER_POINT as f64;
        let library: Vec<Value> = store
            .data
            .library
            .values()
            .filter_map(|local| store.data.catalog.get(&local.clip_id).map(|clip| (local, clip)))
            .map(|(local, clip)| {
                let stats = stats.get(clip.id.as_str());
                json!({
                    "clip":clip,
                    "seeding":rt.permanent.contains_key(&clip.id),
                    "added_at_ms":local.added_at_ms,
                    "points_earned":stats.map_or(0, |s| s.earned_micropoints) as f64 / MICROS_PER_POINT as f64,
                    "average_rating":stats.and_then(|s| s.average_rating()),
                    "rating_count":stats.map_or(0, |s| s.rating_count)
                })
            })
            .collect();
        let queue: Vec<&Clip> = rt
            .ready
            .iter()
            .filter_map(|id| store.data.catalog.get(id))
            .collect();
        let downloading = rt.downloading.as_ref().map(|(id, handle)| {
            let stats = handle.stats();
            json!({"id":id,"downloaded":stats.progress_bytes,"total":stats.total_bytes})
        });
        json!({"version":env!("CARGO_PKG_VERSION"),"update":update,"profile_id":local,"balance":store.balance(local),"earned":earned,"settings":store.data.settings,"library":library,"queue":queue,"session":rt.session,"temporary_seeds":rt.temporary.len(),"peers":rt.peers.iter().map(|(id,p)|json!({"id":id,"name":p.name})).collect::<Vec<_>>(),"addresses":rt.addresses,"downloading":downloading,"notice":rt.notice,"roll":rt.roll,"data_folder":self.root,"p2p_port":self.p2p_port,"torrent_port":self.torrent_port,"limits":Limits::default()})
    }
    pub async fn upload(&self, filename: String, bytes: Vec<u8>) -> Result<Value> {
        let _guard = self.mutation.lock().await;
        ensure!(
            !bytes.is_empty() && bytes.len() as u64 <= MAX_FILE_BYTES,
            "Videos must be at most 8 MB (8,000,000 bytes)"
        );
        let extension = filename
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        ensure!(
            matches!(extension.as_str(), "mp4" | "webm"),
            "Only MP4 and WebM files are supported"
        );
        let snapshot = self.root.join("incoming").join(format!(
            "{}.{}",
            hex::encode(rand::random::<[u8; 16]>()),
            extension
        ));
        tokio::fs::write(&snapshot, &bytes).await?;
        let result = self.import_snapshot(&snapshot, filename, &bytes).await;
        let _ = tokio::fs::remove_file(snapshot).await;
        result
    }
    async fn import_snapshot(
        &self,
        path: &std::path::Path,
        filename: String,
        bytes: &[u8],
    ) -> Result<Value> {
        let video = media::validate_video(path, &Limits::default(), &self.probe).await?;
        let torrent = torrent::create_torrent(bytes, &video.extension)?;
        ensure!(video.id == torrent.id, "Video changed during validation");
        let title: String = std::path::Path::new(&filename)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled clip")
            .chars()
            .filter(|c| !c.is_control())
            .take(60)
            .collect();
        let clip = Clip {
            id: video.id,
            info_hash: torrent.info_hash.clone(),
            torrent: STANDARD.encode(&torrent.bytes),
            title: if title.trim().is_empty() {
                "Untitled clip".into()
            } else {
                title
            },
            extension: video.extension,
            size: video.size,
            duration_secs: video.duration_secs,
            width: video.width,
            height: video.height,
        };
        let mut store = self.store.lock().await;
        if store.data.library.contains_key(&clip.id) {
            return Ok(json!({"duplicate":true,"clip":store.data.catalog[&clip.id]}));
        }
        // A duplicate in the collection consumes no new slot. Promoting a
        // session seed adds to the collection while retaining its existing slot.
        ensure!(
            store.data.library.len() < MAX_LIBRARY_FILES,
            "Your collection already contains 100 clips"
        );
        let rt = self.runtime.lock().await;
        let temporary = rt.temporary.get(&clip.id).cloned();
        let promoted = temporary.is_some();
        ensure!(
            promoted || rt.permanent.len() + rt.temporary.len() < MAX_LIBRARY_FILES,
            "All 100 seed slots are in use. Remove a clip or end the rating session."
        );
        drop(rt);
        let duplicate = store.data.catalog.contains_key(&clip.id);
        let dest = self.root.join("clips").join(clip.filename());
        // Canonical managed path; never use a user-provided path for torrent storage.
        if let Ok(meta) = tokio::fs::symlink_metadata(&dest).await {
            ensure!(
                meta.file_type().is_file(),
                "Managed destination is not a regular file"
            );
        }
        tokio::fs::copy(path, &dest).await?;
        // Promoting a session seed must retain its live swarm. librqbit deduplicates
        // by info hash, so add() followed by remove(old) would remove this same handle.
        // Keep the cache backing file until quit; the managed copy resumes next launch.
        let handle = match temporary {
            Some(handle) => handle,
            None => {
                self.engine
                    .add(&torrent, &self.root.join("clips"), vec![], false)
                    .await?
            }
        };
        if let Err(error) = store.add_verified_library(clip.clone(), dest.clone()) {
            if !promoted {
                let _ = self.engine.remove(&handle).await;
            }
            return Err(error);
        }
        drop(store);
        let mut rt = self.runtime.lock().await;
        rt.temporary.remove(&clip.id);
        rt.ready.retain(|id| id != &clip.id);
        rt.permanent.insert(clip.id.clone(), handle);
        drop(rt);
        self.announce().await;
        self.wake.notify_one();
        Ok(json!({"duplicate":duplicate,"clip":clip}))
    }
    pub async fn remove(&self, id: &str) -> Result<()> {
        let _guard = self.mutation.lock().await;
        let mut store = self.store.lock().await;
        ensure!(
            store.data.library.contains_key(id),
            "Clip is not in your collection"
        );
        let path = store.data.library[id].path.clone();
        store.remove_library(id)?;
        drop(store);
        if let Some(handle) = self.runtime.lock().await.permanent.remove(id) {
            self.engine.remove(&handle).await?;
        }
        if path.parent() == Some(self.root.join("clips").as_path()) {
            let _ = tokio::fs::remove_file(path).await;
        }
        self.announce().await;
        Ok(())
    }
    pub async fn session(&self, active: bool) -> Result<()> {
        let _guard = self.mutation.lock().await;
        let mut rt = self.runtime.lock().await;
        rt.session = active;
        let handles = if !active {
            std::mem::take(&mut rt.temporary)
        } else {
            BTreeMap::new()
        };
        drop(rt);
        for (id, handle) in handles {
            self.engine.remove(&handle).await?;
            if let Some(clip) = self.store.lock().await.data.catalog.get(&id) {
                let _ = tokio::fs::remove_file(self.root.join("cache").join(clip.filename())).await;
            }
        }
        self.announce().await;
        self.wake.notify_one();
        Ok(())
    }
    pub async fn rate(&self, id: &str, score: u8) -> Result<Value> {
        let _guard = self.mutation.lock().await;
        ensure!(score <= 9, "Choose a rating from 0 to 9");
        let mut store = self.store.lock().await;
        let mut rt = self.runtime.lock().await;
        ensure!(rt.session, "Start a rating session first");
        ensure!(
            rt.ready.front().is_some_and(|current| current == id),
            "This clip is no longer at the front of your queue"
        );
        let seeders: Vec<String> = rt
            .peers
            .iter()
            .filter(|(_, p)| {
                p.seen.elapsed() < Duration::from_secs(65) && p.seeded.iter().any(|s| s == id)
            })
            .map(|(peer, _)| peer.clone())
            .collect();
        if score > 0 {
            ensure!(
                rt.permanent.len() + rt.temporary.len() < MAX_LIBRARY_FILES,
                "All 100 seed slots are in use. End the session or free a seed slot first."
            );
        }
        let clip = store
            .data
            .catalog
            .get(id)
            .cloned()
            .context("Unknown clip")?;
        // Start a validated temporary seed before spending; roll it back if saving fails.
        let handle = if score > 0 {
            Some(
                self.engine
                    .add(&prepared(&clip)?, &self.root.join("cache"), vec![], false)
                    .await?,
            )
        } else {
            None
        };
        let receipt = match store.rate(&self.identity, id, score, seeders, rt.peers.len() + 1) {
            Ok(receipt) => receipt,
            Err(error) => {
                if let Some(h) = handle {
                    let _ = self.engine.remove(&h).await;
                }
                return Err(error);
            }
        };
        rt.ready.pop_front();
        if let Some(handle) = handle {
            rt.temporary.insert(id.to_owned(), handle);
        }
        drop(rt);
        drop(store);
        if score == 0 {
            let _ = tokio::fs::remove_file(self.root.join("cache").join(clip.filename())).await;
        }
        let reward = receipt.reward_per_seeder()?;
        let recipients = receipt.payload.seeders.len();
        if let Err(error) = self.mesh.receipt(receipt).await {
            self.notice(&format!(
                "Rating saved locally; network delivery will retry: {error}"
            ))
            .await;
        }
        self.announce().await;
        self.wake.notify_one();
        Ok(json!({"score":score,"reward_per_seeder":reward,"seeders":recipients}))
    }
    pub async fn roll(self: &Arc<Self>, wager: f64) -> Result<Value> {
        let wager = wager_micropoints(wager)?;
        let _guard = self.mutation.lock().await;
        let mut store = self.store.lock().await;
        let receipt = store.roll(&self.identity, wager)?;
        let payload = receipt
            .payload
            .roll
            .as_ref()
            .context("Missing roll result")?;
        let multiplier = roll_multiplier(payload.ticket)?;
        let mut rt = self.runtime.lock().await;
        // Availability never affects the draw, and watched history does not filter Roll.
        let stats = store.clip_stats();
        let choice = (multiplier > 0)
            .then(|| {
                store
                    .data
                    .catalog
                    .values()
                    .filter(|clip| {
                        stats
                            .get(clip.id.as_str())
                            .and_then(|stats| stats.average_rating())
                            .is_some_and(|average| roll_rating(average) == multiplier)
                    })
                    .filter_map(|clip| {
                        let source = if rt.permanent.contains_key(&clip.id) {
                            Some(self.root.join("clips").join(clip.filename()))
                        } else if rt.ready.contains(&clip.id) || rt.temporary.contains_key(&clip.id)
                        {
                            Some(self.root.join("cache").join(clip.filename()))
                        } else if rt.roll_file.as_ref() == Some(&clip.id) {
                            Some(self.root.join("roll-cache").join(clip.filename()))
                        } else {
                            None
                        };
                        let peers: Vec<_> = rt
                            .peers
                            .values()
                            .filter(|peer| {
                                peer.seen.elapsed() < Duration::from_secs(65)
                                    && peer.seeded.contains(&clip.id)
                            })
                            .filter_map(|peer| peer.endpoint)
                            .collect();
                        (source.is_some() || !peers.is_empty()).then(|| RollClip {
                            clip: clip.clone(),
                            source,
                            peers,
                        })
                    })
                    .choose(&mut rand::rng())
            })
            .flatten();
        let result = RollResult {
            id: receipt.id.clone(),
            multiplier,
            wager: wager as f64 / MICROS_PER_POINT as f64,
            payout: payload.payout_micropoints()? as f64 / MICROS_PER_POINT as f64,
            balance: store.balance(&store.data.profile_id),
            clip: None,
            message: Some(if multiplier == 0 {
                "No clip on a 0. Your wager was lost.".into()
            } else if choice.is_some() {
                format!("Preparing a rating {multiplier} clip…")
            } else {
                format!("No rating {multiplier} clips are available yet. Your payout is saved.")
            }),
        };
        if let Some(cancel) = rt.roll_cancel.take() {
            cancel.cancel();
        }
        let cancel = self.shutdown.child_token();
        rt.roll_cancel = Some(cancel.clone());
        rt.roll = Some(result.clone());
        drop(rt);
        drop(store);
        if let Some(choice) = choice {
            let app = self.clone();
            let id = result.id.clone();
            tokio::spawn(async move {
                app.prepare_roll(id, choice, cancel).await;
            });
        }
        if let Err(error) = self.mesh.receipt(receipt).await {
            self.notice(&format!(
                "Roll saved locally; network delivery will retry: {error}"
            ))
            .await;
        }
        Ok(serde_json::to_value(result)?)
    }

    async fn prepare_roll(&self, id: String, choice: RollClip, cancel: CancellationToken) {
        // Roll uses its own bounded cache so Rate cleanup cannot delete a playing clip.
        let _guard = tokio::select! {
            _ = cancel.cancelled() => return,
            guard = self.roll_preparation.lock() => guard,
        };
        if cancel.is_cancelled() {
            return;
        }
        let path = self.root.join("roll-cache").join(choice.clip.filename());
        let preparation = async {
            let mut files = tokio::fs::read_dir(self.root.join("roll-cache")).await?;
            while let Some(file) = files.next_entry().await? {
                if file.path() != path && file.file_type().await?.is_file() {
                    let _ = tokio::fs::remove_file(file.path()).await;
                }
            }
            let copied = if let Some(source) = &choice.source {
                if source == &path {
                    true
                } else {
                    tokio::fs::copy(source, &path).await.is_ok()
                }
            } else {
                false
            };
            if !copied {
                ensure!(!choice.peers.is_empty(), "Clip is no longer available");
                let handle = self
                    .roll_downloader
                    .add(&prepared(&choice.clip)?, &self.root.join("roll-cache"), choice.peers, false)
                    .await?;
                let download = tokio::select! {
                    _ = cancel.cancelled() => Err(anyhow::anyhow!("Roll superseded")),
                    result = tokio::time::timeout(Duration::from_secs(90), self.roll_downloader.wait(&handle)) => {
                        match result {
                            Ok(result) => result,
                            Err(error) => Err(anyhow::Error::new(error).context("Roll clip download timed out")),
                        }
                    }
                };
                let removal = self.roll_downloader.remove(&handle).await;
                download?;
                removal?;
            }
            ensure!(!cancel.is_cancelled(), "Roll superseded");
            let limits = Limits::default();
            let video = tokio::select! {
                _ = cancel.cancelled() => return Err(anyhow::anyhow!("Roll superseded")),
                video = media::validate_video(&path, &limits, &self.probe) => video?,
            };
            ensure!(video.id == choice.clip.id, "Roll clip content hash changed");
            let bytes = tokio::fs::read(&path).await?;
            ensure!(
                torrent::create_torrent(&bytes, &choice.clip.extension)?.info_hash == choice.clip.info_hash,
                "Roll clip torrent metadata changed"
            );
            let mut verified = choice.clip;
            verified.width = video.width;
            verified.height = video.height;
            verified.duration_secs = video.duration_secs;
            self.store.lock().await.add_verified_clip(verified.clone())?;
            Ok::<_, anyhow::Error>(verified)
        }
        .await;
        let mut rt = self.runtime.lock().await;
        if cancel.is_cancelled() {
            return;
        }
        if rt.roll.as_ref().is_some_and(|result| result.id == id) {
            match preparation {
                Ok(clip) => {
                    rt.roll_file = Some(clip.id.clone());
                    let result = rt.roll.as_mut().unwrap();
                    result.clip = Some(clip);
                    result.message = None;
                }
                Err(error) => {
                    rt.roll.as_mut().unwrap().message = Some(format!(
                        "Could not prepare this clip: {error}. Your payout is saved."
                    ));
                    rt.roll_file = None;
                    drop(rt);
                    let _ = tokio::fs::remove_file(path).await;
                }
            }
        }
    }

    pub async fn settings(&self, settings: Settings) -> Result<()> {
        settings.validate()?;
        for address in &settings.bootstrap_peers {
            self.mesh.dial(address.clone()).await?;
        }
        self.store.lock().await.set_settings(settings)?;
        self.announce().await;
        self.wake.notify_one();
        Ok(())
    }
    pub async fn media_path(&self, id: &str) -> Result<(PathBuf, String)> {
        let store = self.store.lock().await;
        let clip = store.data.catalog.get(id).context("Unknown clip")?;
        let rt = self.runtime.lock().await;
        let path = if rt
            .roll
            .as_ref()
            .and_then(|result| result.clip.as_ref())
            .is_some_and(|clip| clip.id == id)
        {
            self.root.join("roll-cache").join(clip.filename())
        } else if store.data.library.contains_key(id) {
            self.root.join("clips").join(clip.filename())
        } else {
            ensure!(
                rt.ready.iter().any(|v| v == id) || rt.temporary.contains_key(id),
                "Clip is not ready"
            );
            self.root.join("cache").join(clip.filename())
        };
        Ok((path, clip.extension.clone()))
    }
    async fn prefetch(self: Arc<Self>) {
        loop {
            if self.shutdown.is_cancelled() {
                break;
            }
            let choice = {
                let store = self.store.lock().await;
                let rt = self.runtime.lock().await;
                if (!store.data.settings.prefetch && !rt.session) || rt.ready.len() >= 10 {
                    None
                } else {
                    let candidates = store.data.catalog.values().filter(|clip| {
                        !store.data.watched.contains(&clip.id)
                            && !store.data.library.contains_key(&clip.id)
                            && !rt.ready.contains(&clip.id)
                            && !rt
                                .failed
                                .get(&clip.id)
                                .is_some_and(|when| when.elapsed() < Duration::from_secs(120))
                    });
                    let available = candidates.filter_map(|clip| {
                        let peers: Vec<SocketAddr> = rt
                            .peers
                            .values()
                            .filter(|peer| {
                                peer.seen.elapsed() < Duration::from_secs(65)
                                    && peer.seeded.contains(&clip.id)
                            })
                            .filter_map(|p| p.endpoint)
                            .collect();
                        (!peers.is_empty()).then(|| (clip.clone(), peers))
                    });
                    available.choose(&mut rand::rng())
                }
            };
            if let Some((clip, peers)) = choice {
                let result = self.download(&clip, peers).await;
                if let Err(error) = result
                    && !self.shutdown.is_cancelled()
                {
                    self.notice(&format!("Could not prepare {}: {error}", clip.title))
                        .await;
                    self.runtime
                        .lock()
                        .await
                        .failed
                        .insert(clip.id.clone(), Instant::now());
                }
                self.runtime.lock().await.downloading = None;
            } else {
                tokio::select! {_ = self.shutdown.cancelled()=>break,_=self.wake.notified()=>(),_=tokio::time::sleep(Duration::from_secs(3))=>()}
            }
        }
    }
    async fn download(&self, clip: &Clip, peers: Vec<SocketAddr>) -> Result<()> {
        let torrent = prepared(clip)?;
        let handle = self
            .downloader
            .add(&torrent, &self.root.join("cache"), peers, false)
            .await?;
        self.runtime.lock().await.downloading = Some((clip.id.clone(), handle.clone()));
        let result = tokio::select! {
            _=self.shutdown.cancelled()=>Err(anyhow::anyhow!("Shutting down")),
            result=tokio::time::timeout(Duration::from_secs(90),self.downloader.wait(&handle))=>match result {
                Ok(result) => result,
                Err(error) => Err(anyhow::Error::new(error).context("Peer download timed out")),
            }
        };
        let _ = self.downloader.remove(&handle).await;
        let path = self.root.join("cache").join(clip.filename());
        let validation = async {
            result?;
            let video = media::validate_video(&path, &Limits::default(), &self.probe).await?;
            ensure!(
                video.id == clip.id,
                "Downloaded video does not match its content hash"
            );
            let bytes = tokio::fs::read(&path).await?;
            ensure!(
                torrent::create_torrent(&bytes, &clip.extension)?.info_hash == clip.info_hash,
                "Downloaded torrent is not canonical for these bytes"
            );
            let mut store = self.store.lock().await;
            let mut verified = clip.clone();
            verified.width = video.width;
            verified.height = video.height;
            verified.duration_secs = video.duration_secs;
            store.add_verified_clip(verified)?;
            if !store.data.watched.contains(&clip.id) && !store.data.library.contains_key(&clip.id)
            {
                self.runtime.lock().await.ready.push_back(clip.id.clone());
            }
            Ok::<_, anyhow::Error>(())
        }
        .await;
        if validation.is_err() {
            let _ = tokio::fs::remove_file(path).await;
        }
        validation
    }
    pub async fn stop(&self) {
        self.shutdown.cancel();
        self.engine.stop().await;
        let _ = self.mesh.shutdown().await;
        self.downloader.stop().await;
        self.roll_downloader.stop().await;
        let _ = self.store.lock().await.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roll_wagers_use_exact_point_precision_and_reject_invalid_amounts() {
        assert_eq!(wager_micropoints(1.0).unwrap(), 1_000_000);
        assert_eq!(wager_micropoints(0.000001).unwrap(), 1);
        assert_eq!(wager_micropoints(1.234567).unwrap(), 1_234_567);
        assert_eq!(wager_micropoints(0.1 + 0.2).unwrap(), 300_000);
        for invalid in [
            0.0,
            -1.0,
            0.0000001,
            1.0000001,
            f64::NAN,
            f64::INFINITY,
            f64::MAX,
        ] {
            assert!(wager_micropoints(invalid).is_err(), "Accepted {invalid}");
        }
    }

    #[test]
    fn roll_rating_uses_nearest_integer_tiers() {
        for (average, tier) in [
            (1.0, 1),
            (1.49, 1),
            (1.5, 2),
            (4.49, 4),
            (4.5, 5),
            (8.5, 9),
            (9.0, 9),
        ] {
            assert_eq!(roll_rating(average), tier);
        }
    }
}
