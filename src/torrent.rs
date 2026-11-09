//! A deliberately tiny, deterministic single-file torrent format.
//! Untrusted metainfo must pass this parser before librqbit sees it.
use anyhow::{Context, Result, ensure};
use librqbit::{
    AddTorrent, AddTorrentOptions, ListenerOptions, ManagedTorrent, Session, SessionOptions,
};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
};

pub const PIECE_LENGTH: usize = 65_536;
pub const MAX_METAINFO_BYTES: usize = 16_384;

#[derive(Debug, Clone)]
pub struct PreparedTorrent {
    pub id: String,
    pub info_hash: String,
    pub filename: String,
    pub bytes: Vec<u8>,
}

fn canonical_info(size: u64, name: &str, pieces: &[u8]) -> Vec<u8> {
    let mut info = format!(
        "d6:lengthi{size}e4:name{}:{name}12:piece lengthi{PIECE_LENGTH}e6:pieces{}:",
        name.len(),
        pieces.len()
    )
    .into_bytes();
    info.extend_from_slice(pieces);
    info.push(b'e');
    info
}

fn canonical_torrent(info: &[u8]) -> Vec<u8> {
    let mut bytes = b"d4:info".to_vec();
    bytes.extend_from_slice(info);
    bytes.push(b'e');
    bytes
}

pub fn create_torrent(bytes: &[u8], extension: &str) -> Result<PreparedTorrent> {
    ensure!(
        matches!(extension, "mp4" | "webm"),
        "Unsupported video extension"
    );
    ensure!(
        !bytes.is_empty() && bytes.len() as u64 <= crate::media::Limits::default().max_bytes,
        "Invalid video length"
    );
    let id = hex::encode(Sha256::digest(bytes));
    let filename = format!("{id}.{extension}");
    let pieces: Vec<u8> = bytes
        .chunks(PIECE_LENGTH)
        .flat_map(|piece| Sha1::digest(piece).to_vec())
        .collect();
    let info = canonical_info(bytes.len() as u64, &filename, &pieces);
    Ok(PreparedTorrent {
        id,
        info_hash: hex::encode(Sha1::digest(&info)),
        filename,
        bytes: canonical_torrent(&info),
    })
}

/// Rejects multi-file torrents, path traversal, trackers, web seeds, unexpected keys,
/// alternate encodings and inconsistent lengths. The canonical info hash is checked by the caller.
pub fn validate_torrent(
    bytes: &[u8],
    id: &str,
    extension: &str,
    size: u64,
) -> Result<PreparedTorrent> {
    ensure!(
        bytes.len() <= MAX_METAINFO_BYTES,
        "Torrent metadata exceeds the limit"
    );
    ensure!(
        id.len() == 64
            && id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "Invalid content hash"
    );
    ensure!(
        matches!(extension, "mp4" | "webm"),
        "Unsupported video extension"
    );
    ensure!(
        size > 0 && size <= crate::media::Limits::default().max_bytes,
        "Invalid video length"
    );
    let filename = format!("{id}.{extension}");
    let mut reader = Reader { bytes, at: 0 };
    reader.literal(b"d4:infod6:lengthi")?;
    let length = reader.decimal(b'e')?;
    ensure!(
        length == size,
        "Torrent length does not match the advertised video"
    );
    reader.literal(b"4:name")?;
    ensure!(
        reader.string()? == filename.as_bytes(),
        "Torrent filename must be the content hash and extension"
    );
    reader.literal(b"12:piece lengthi")?;
    ensure!(
        reader.decimal(b'e')? == PIECE_LENGTH as u64,
        "Unexpected torrent piece length"
    );
    reader.literal(b"6:pieces")?;
    let pieces = reader.string()?;
    let expected = size.div_ceil(PIECE_LENGTH as u64) as usize * 20;
    ensure!(
        pieces.len() == expected,
        "Torrent piece hashes do not match the declared length"
    );
    reader.literal(b"ee")?;
    ensure!(reader.at == bytes.len(), "Unexpected trailing torrent data");
    let info = canonical_info(size, &filename, pieces);
    ensure!(
        canonical_torrent(&info) == bytes,
        "Torrent metadata must use canonical encoding"
    );
    Ok(PreparedTorrent {
        id: id.to_owned(),
        info_hash: hex::encode(Sha1::digest(&info)),
        filename,
        bytes: bytes.to_vec(),
    })
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn literal(&mut self, literal: &[u8]) -> Result<()> {
        ensure!(
            self.bytes
                .get(self.at..self.at.saturating_add(literal.len()))
                == Some(literal),
            "Unsupported or malformed torrent metadata"
        );
        self.at += literal.len();
        Ok(())
    }
    fn decimal(&mut self, terminator: u8) -> Result<u64> {
        let start = self.at;
        while let Some(byte) = self.bytes.get(self.at) {
            if *byte == terminator {
                break;
            }
            ensure!(
                byte.is_ascii_digit() && self.at - start < 20,
                "Invalid torrent integer"
            );
            self.at += 1;
        }
        ensure!(
            self.at > start && self.bytes.get(self.at) == Some(&terminator),
            "Truncated torrent integer"
        );
        let digits = &self.bytes[start..self.at];
        ensure!(
            digits.len() == 1 || digits[0] != b'0',
            "Non-canonical torrent integer"
        );
        self.at += 1;
        Ok(std::str::from_utf8(digits)?.parse()?)
    }
    fn string(&mut self) -> Result<&'a [u8]> {
        let length = usize::try_from(self.decimal(b':')?)?;
        let end = self
            .at
            .checked_add(length)
            .context("Torrent string length overflow")?;
        let result = self
            .bytes
            .get(self.at..end)
            .context("Truncated torrent string")?;
        self.at = end;
        Ok(result)
    }
}

#[derive(Clone)]
pub struct TorrentEngine {
    session: Arc<Session>,
}

impl TorrentEngine {
    pub async fn new(root: PathBuf, port: u16) -> Result<Self> {
        Self::start(root, Some(port), false).await
    }

    /// A separate session that cannot upload any pieces, even before validation finishes.
    /// It has no listener, announces nothing, and connects only to supplied seed endpoints.
    pub async fn downloader(root: PathBuf) -> Result<Self> {
        Self::start(root, None, true).await
    }

    async fn start(root: PathBuf, listen_port: Option<u16>, disable_upload: bool) -> Result<Self> {
        tokio::fs::create_dir_all(&root).await?;
        let session = Session::new_with_opts(
            root,
            SessionOptions {
                persistence: None,
                fastresume: false,
                disable_trackers: true,
                // Application-level libp2p discovery supplies explicit peer endpoints.
                // No global rqbit configuration or DHT state is written outside portable data.
                dht: None,
                listen: listen_port.map(|port| ListenerOptions {
                    listen_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port),
                    enable_upnp_port_forwarding: true,
                    ipv4_only: true,
                    ..Default::default()
                }),
                disable_upload,
                disable_local_service_discovery: disable_upload,
                peer_limit: Some(40),
                concurrent_init_limit: Some(1),
                ipv4_only: true,
                ..Default::default()
            },
        )
        .await?;
        Ok(Self { session })
    }

    pub fn port(&self) -> Option<u16> {
        self.session.announce_port()
    }

    pub async fn add(
        &self,
        torrent: &PreparedTorrent,
        folder: &Path,
        initial_peers: Vec<SocketAddr>,
        paused: bool,
    ) -> Result<Arc<ManagedTorrent>> {
        // Defense in depth: callers cannot accidentally hand arbitrary metainfo to the engine.
        // Obtain the expected size from the canonical format before revalidating everything.
        let mut reader = Reader {
            bytes: &torrent.bytes,
            at: 0,
        };
        reader.literal(b"d4:infod6:lengthi")?;
        let size = reader.decimal(b'e')?;
        let extension = torrent
            .filename
            .rsplit('.')
            .next()
            .context("Missing extension")?;
        let verified = validate_torrent(&torrent.bytes, &torrent.id, extension, size)?;
        ensure!(
            verified.info_hash == torrent.info_hash,
            "Torrent info hash mismatch"
        );
        tokio::fs::create_dir_all(folder).await?;
        let metadata = tokio::fs::symlink_metadata(folder).await?;
        ensure!(
            metadata.file_type().is_dir(),
            "Torrent destination must be a real directory"
        );
        if let Ok(metadata) = tokio::fs::symlink_metadata(folder.join(&torrent.filename)).await {
            ensure!(
                metadata.file_type().is_file(),
                "Torrent file must be a regular file"
            );
        }
        let handle = self
            .session
            .add_torrent(
                AddTorrent::from_bytes(torrent.bytes.clone()),
                Some(AddTorrentOptions {
                    paused,
                    overwrite: true,
                    output_folder: Some(
                        folder
                            .to_str()
                            .context("Torrent destination is not UTF-8")?
                            .to_owned(),
                    ),
                    initial_peers: Some(initial_peers),
                    disable_trackers: true,
                    peer_limit: Some(40),
                    ..Default::default()
                }),
            )
            .await?
            .into_handle()
            .context("Torrent was not added to the session")?;
        Ok(handle)
    }

    pub async fn wait(&self, handle: &Arc<ManagedTorrent>) -> Result<()> {
        handle.wait_until_completed().await?;
        Ok(())
    }
    #[cfg(test)]
    pub async fn pause(&self, handle: &Arc<ManagedTorrent>) -> Result<()> {
        self.session.pause(handle).await?;
        Ok(())
    }
    /// Remove the swarm but retain managed files. The caller owns cache deletion.
    pub async fn remove(&self, handle: &Arc<ManagedTorrent>) -> Result<()> {
        self.session
            .delete(librqbit::api::TorrentIdOrHash::Id(handle.id()), false)
            .await?;
        Ok(())
    }
    pub async fn stop(&self) {
        self.session.stop().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identical_bytes_have_one_swarm() {
        let bytes = vec![42; PIECE_LENGTH + 3];
        let first = create_torrent(&bytes, "mp4").unwrap();
        let second = create_torrent(&bytes, "mp4").unwrap();
        assert_eq!(first.bytes, second.bytes);
        assert_eq!(first.info_hash, second.info_hash);
        assert_eq!(
            validate_torrent(&first.bytes, &first.id, "mp4", bytes.len() as u64)
                .unwrap()
                .info_hash,
            first.info_hash
        );
        let parsed = librqbit::torrent_from_bytes(&first.bytes).unwrap();
        assert_eq!(parsed.info_hash.as_string(), first.info_hash);
    }
    #[test]
    fn rejects_hostile_metainfo_and_claimed_sizes() {
        let torrent = create_torrent(b"abc", "webm").unwrap();
        assert!(validate_torrent(&torrent.bytes, &torrent.id, "webm", 4).is_err());
        assert!(validate_torrent(&torrent.bytes, &torrent.id, "mp4", 3).is_err());
        let mut trailing = torrent.bytes.clone();
        trailing.extend_from_slice(b"4:evil");
        assert!(validate_torrent(&trailing, &torrent.id, "webm", 3).is_err());
        let traversal =
            canonical_torrent(&canonical_info(3, "../../evil.webm", &Sha1::digest(b"abc")));
        assert!(validate_torrent(&traversal, &torrent.id, "webm", 3).is_err());
        let mut with_tracker = b"d8:announce17:http://evil.test/4:info".to_vec();
        with_tracker.extend_from_slice(&torrent.bytes[7..]);
        assert!(validate_torrent(&with_tracker, &torrent.id, "webm", 3).is_err());
    }
    #[test]
    fn rejects_noncanonical_and_inconsistent_pieces() {
        let torrent = create_torrent(b"abc", "mp4").unwrap();
        let invalid = canonical_torrent(&canonical_info(3, &torrent.filename, &[0; 40]));
        assert!(validate_torrent(&invalid, &torrent.id, "mp4", 3).is_err());
        let invalid = torrent.bytes.windows(3).position(|w| w == b"i3e").unwrap();
        let mut encoded = torrent.bytes.clone();
        encoded.insert(invalid + 1, b'0');
        assert!(validate_torrent(&encoded, &torrent.id, "mp4", 3).is_err());
    }

    /// This opens only loopback TCP connections but is opt-in on sandboxed CI hosts.
    #[tokio::test]
    #[ignore = "requires local TCP sockets; run with --ignored"]
    async fn two_engines_transfer_exact_file() {
        let unique = format!(
            "clipcrop-transfer-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        let source = root.join("source");
        let destination = root.join("destination");
        tokio::fs::create_dir_all(&source).await.unwrap();
        let data: Vec<u8> = (0..200_000).map(|n| (n % 251) as u8).collect();
        let torrent = create_torrent(&data, "mp4").unwrap();
        tokio::fs::write(source.join(&torrent.filename), &data)
            .await
            .unwrap();
        let seeder = TorrentEngine::new(source.clone(), 0).await.unwrap();
        let seed_handle = seeder.add(&torrent, &source, vec![], false).await.unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            seeder.wait(&seed_handle),
        )
        .await
        .unwrap()
        .unwrap();
        let leecher = TorrentEngine::downloader(destination.clone())
            .await
            .unwrap();
        let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), seeder.port().unwrap());
        let download = leecher
            .add(&torrent, &destination, vec![peer], false)
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(20), leecher.wait(&download))
            .await
            .unwrap()
            .unwrap();
        leecher.pause(&download).await.unwrap();
        assert_eq!(
            download.stats().uploaded_bytes,
            0,
            "The prefetch engine must never upload pieces"
        );
        assert_eq!(
            tokio::fs::read(destination.join(&torrent.filename))
                .await
                .unwrap(),
            data
        );
        leecher.stop().await;
        seeder.stop().await;
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
