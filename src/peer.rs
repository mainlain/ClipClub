//! The application mesh. BitTorrent transfers run separately; this mesh only
//! carries bounded metadata, seeder presence and independently signed ratings.
//!
//! A message's author comes from authenticated GossipSub, never its JSON body.
//! Presence is an observation, not proof of storage or a global user census.

use crate::core::{Clip, RatingReceipt};
use anyhow::{Context, Result, anyhow, bail};
use futures::StreamExt;
use libp2p::{
    Multiaddr, PeerId, Swarm, SwarmBuilder, connection_limits, gossipsub, identity, mdns,
    multiaddr::Protocol,
    noise,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::Write,
    net::{IpAddr, SocketAddr},
    path::Path,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

const TOPIC: &str = "clipcrop/catalog/2";
const MAX_WIRE: usize = 128 * 1024;
const MAX_SEEDED: usize = 100;
const MAX_AUTHORS: usize = 2048;
const REFRESH: Duration = Duration::from_secs(20);

/// Identity lives beside portable state; moving the folder preserves it.
/// Never silently replace an unreadable/corrupt identity with a new wallet.
pub fn load_identity(root: &Path) -> Result<identity::Keypair> {
    fs::create_dir_all(root)?;
    let path = root.join("identity.key");
    if path.exists() {
        let meta = fs::symlink_metadata(&path)?;
        if !meta.is_file() || meta.len() > 4096 {
            bail!("Invalid identity file");
        }
        let bytes = fs::read(&path).context("Cannot read portable identity")?;
        return identity::Keypair::from_protobuf_encoding(&bytes)
            .context("Cannot decode portable identity; preserve identity.key before repairing it");
    }
    let key = identity::Keypair::generate_ed25519();
    let bytes = key.to_protobuf_encoding()?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .context("Cannot create portable identity")?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(key)
}

#[derive(Clone, Debug)]
pub enum PeerEvent {
    Listening(String),
    Connected(String),
    Disconnected(String),
    Advert {
        peer: String,
        clip: Clip,
    },
    Presence {
        peer: String,
        seeded: Vec<String>,
        permanent: Vec<String>,
        wallet: Option<String>,
        /// Available only for a directly authenticated connection. An author
        /// cannot use gossip to instruct arbitrary third-party TCP connections.
        torrent_addr: Option<SocketAddr>,
    },
    Receipt(RatingReceipt),
    Error(String),
}

#[derive(Clone)]
pub struct PeerHandle {
    pub peer_id: String,
    commands: mpsc::Sender<Command>,
}

impl PeerHandle {
    /// Replaces the local snapshot; it is periodically reannounced for new peers.
    pub async fn announce(
        &self,
        clips: Vec<Clip>,
        seeded: Vec<String>,
        torrent_port: u16,
        permanent: Vec<String>,
        wallet: Option<String>,
    ) -> Result<()> {
        if clips.len() > MAX_SEEDED || seeded.len() > MAX_SEEDED {
            bail!("Seeder announcements are limited to 100 clips");
        }
        if seeded.iter().any(|id| !valid_id(id)) {
            bail!("Invalid content ID in seeder announcement");
        }
        self.send(Command::Announce(Snapshot {
            clips,
            seeded,
            torrent_port,
            permanent,
            wallet,
        }))
        .await
    }

    pub async fn receipt(&self, receipt: RatingReceipt) -> Result<()> {
        self.send(Command::Receipt(receipt)).await
    }

    /// Forward a historical descriptor before replaying a receipt about it.
    /// Advertising metadata never asserts that this peer currently seeds it;
    /// current seed ownership comes exclusively from Presence.
    pub async fn advert(&self, clip: Clip) -> Result<()> {
        clip.validate()?;
        if serde_json::to_vec(&WireMessage::Advert { clip: clip.clone() })?.len() >= MAX_WIRE - 1024
        {
            bail!("Clip metadata is too large for a peer announcement");
        }
        self.send(Command::Advert(clip)).await
    }

    pub async fn dial(&self, address: String) -> Result<()> {
        // Validate synchronously so the UI receives actionable input errors.
        validate_address(&address)?;
        self.send(Command::Dial(address)).await
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.send(Command::Shutdown).await
    }

    async fn send(&self, command: Command) -> Result<()> {
        self.commands
            .send(command)
            .await
            .map_err(|_| anyhow!("Peer mesh has stopped"))
    }
}

#[derive(Clone, Default)]
struct Snapshot {
    clips: Vec<Clip>,
    seeded: Vec<String>,
    torrent_port: u16,
    permanent: Vec<String>,
    wallet: Option<String>,
}

enum Command {
    Announce(Snapshot),
    Advert(Clip),
    Receipt(RatingReceipt),
    Dial(String),
    Shutdown,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum WireMessage {
    Advert {
        clip: Clip,
    },
    Presence {
        seeded: Vec<String>,
        torrent_port: u16,
        #[serde(default)]
        permanent: Vec<String>,
        #[serde(default)]
        wallet: Option<String>,
    },
    Receipt {
        receipt: RatingReceipt,
    },
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    gossip: gossipsub::Behaviour,
    mdns: mdns::tokio::Behaviour,
    limits: connection_limits::Behaviour,
}

/// Start on the application's Tokio runtime. Dropping the last handle stops the
/// task; no daemon or external application service is launched.
pub async fn start(
    identity: identity::Keypair,
    port: u16,
    bootstrap: Vec<String>,
) -> Result<(PeerHandle, mpsc::Receiver<PeerEvent>)> {
    let id = identity.public().to_peer_id();
    let mut swarm = SwarmBuilder::with_existing_identity(identity)
        .with_tokio()
        .with_tcp(
            tcp::Config::default().nodelay(true),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_dns()?
        .with_behaviour(|key| {
            let config = gossipsub::ConfigBuilder::default()
                .heartbeat_interval(Duration::from_secs(1))
                .validation_mode(gossipsub::ValidationMode::Strict)
                .validate_messages()
                .max_transmit_size(MAX_WIRE)
                .build()
                .map_err(std::io::Error::other)?;
            let gossip = gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                config,
            )
            .map_err(std::io::Error::other)?;
            let mdns =
                mdns::tokio::Behaviour::new(mdns::Config::default(), key.public().to_peer_id())?;
            let limits = connection_limits::Behaviour::new(
                connection_limits::ConnectionLimits::default()
                    .with_max_pending_incoming(Some(32))
                    .with_max_pending_outgoing(Some(32))
                    .with_max_established_incoming(Some(128))
                    .with_max_established_outgoing(Some(128))
                    .with_max_established(Some(192))
                    .with_max_established_per_peer(Some(2)),
            );
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(Behaviour {
                gossip,
                mdns,
                limits,
            })
        })?
        .with_swarm_config(|config| config.with_idle_connection_timeout(Duration::from_secs(120)))
        .build();
    let topic = gossipsub::IdentTopic::new(TOPIC);
    swarm.behaviour_mut().gossip.subscribe(&topic)?;
    swarm.listen_on(format!("/ip4/0.0.0.0/tcp/{port}").parse()?)?;
    let (command_tx, command_rx) = mpsc::channel(512);
    let (event_tx, event_rx) = mpsc::channel(1024);
    let mut retry_addresses = Vec::new();
    for address in bootstrap.iter().take(64) {
        if let Ok(address) = validate_address(address) {
            retry_addresses.push(address);
        }
        match validate_address(address).and_then(|addr| swarm.dial(addr).map_err(Into::into)) {
            Ok(()) => {}
            Err(error) => {
                let _ = event_tx.try_send(PeerEvent::Error(format!(
                    "Cannot connect to {address}: {error}"
                )));
            }
        }
    }
    tokio::spawn(run(swarm, topic, command_rx, event_tx, retry_addresses));
    Ok((
        PeerHandle {
            peer_id: id.to_string(),
            commands: command_tx,
        },
        event_rx,
    ))
}

fn validate_address(address: &str) -> Result<Multiaddr> {
    if address.len() > 512 {
        bail!("Peer address is too long");
    }
    let addr: Multiaddr = address
        .parse()
        .context("Use a peer multiaddress, for example /ip4/192.168.1.5/tcp/42831/p2p/12D3…")?;
    if !addr
        .iter()
        .any(|part| matches!(part, Protocol::Tcp(port) if port != 0))
    {
        bail!("A peer address must include a TCP port");
    }
    if addr.iter().any(|part| {
        matches!(
            part,
            Protocol::Http | Protocol::Https | Protocol::Ws(_) | Protocol::Wss(_)
        )
    }) {
        bail!("Only direct TCP peer addresses are supported");
    }
    Ok(addr)
}

fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|ch| ch.is_ascii_digit() || (b'a'..=b'f').contains(&ch))
}

fn validate_wire(wire: &WireMessage) -> bool {
    match wire {
        WireMessage::Advert { clip } => clip.validate().is_ok(),
        WireMessage::Presence {
            seeded,
            torrent_port,
            permanent,
            wallet,
        } => {
            seeded.len() <= MAX_SEEDED
                && *torrent_port != 0
                && seeded.iter().all(|id| valid_id(id))
                && seeded.iter().collect::<HashSet<_>>().len() == seeded.len()
                && permanent.len() <= MAX_SEEDED
                && permanent.iter().all(|id| seeded.contains(id))
                && permanent.iter().collect::<HashSet<_>>().len() == permanent.len()
                && wallet.as_ref().is_none_or(|address| {
                    address.len() == 42
                        && address.starts_with("0x")
                        && address[2..].bytes().all(|b| b.is_ascii_hexdigit())
                })
        }
        WireMessage::Receipt { receipt } => receipt.verify().is_ok(),
    }
}

fn publish(swarm: &mut Swarm<Behaviour>, topic: &gossipsub::IdentTopic, wire: &WireMessage) {
    if let Ok(bytes) = serde_json::to_vec(wire)
        && bytes.len() < MAX_WIRE - 1024
    {
        // No peers and duplicate messages are normal, especially at startup.
        let _ = swarm.behaviour_mut().gossip.publish(topic.clone(), bytes);
    }
}

fn broadcast(swarm: &mut Swarm<Behaviour>, topic: &gossipsub::IdentTopic, snapshot: &Snapshot) {
    if snapshot.torrent_port == 0 {
        return;
    }
    let presence = WireMessage::Presence {
        seeded: snapshot.seeded.clone(),
        torrent_port: snapshot.torrent_port,
        permanent: snapshot.permanent.clone(),
        wallet: snapshot.wallet.clone(),
    };
    publish(swarm, topic, &presence);
    for clip in &snapshot.clips {
        publish(swarm, topic, &WireMessage::Advert { clip: clip.clone() });
    }
}

fn broadcast_changes(
    swarm: &mut Swarm<Behaviour>,
    topic: &gossipsub::IdentTopic,
    previous: &Snapshot,
    current: &Snapshot,
) {
    if current.torrent_port == 0 {
        return;
    }
    if previous.seeded != current.seeded
        || previous.permanent != current.permanent
        || previous.wallet != current.wallet
        || previous.torrent_port != current.torrent_port
    {
        publish(
            swarm,
            topic,
            &WireMessage::Presence {
                seeded: current.seeded.clone(),
                torrent_port: current.torrent_port,
                permanent: current.permanent.clone(),
                wallet: current.wallet.clone(),
            },
        );
    }
    for clip in &current.clips {
        if !previous.clips.contains(clip) {
            publish(swarm, topic, &WireMessage::Advert { clip: clip.clone() });
        }
    }
}

struct AuthorBudget {
    started: Instant,
    messages: usize,
}

impl AuthorBudget {
    fn accept(&mut self) -> bool {
        if self.started.elapsed() >= REFRESH {
            self.started = Instant::now();
            self.messages = 0;
        }
        self.messages += 1;
        // Historical receipts may refer to more than 100 different clips. Keep
        // the 100-seed limit in Presence and bound all history by message rate.
        self.messages <= 512
    }
}

async fn run(
    mut swarm: Swarm<Behaviour>,
    topic: gossipsub::IdentTopic,
    mut commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<PeerEvent>,
    mut retry_addresses: Vec<Multiaddr>,
) {
    let mut snapshot = Snapshot::default();
    let mut timer = tokio::time::interval(REFRESH);
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut peer_ips = HashMap::<PeerId, IpAddr>::new();
    let mut budgets = HashMap::<PeerId, AuthorBudget>::new();
    let mut subscribed = HashSet::<PeerId>::new();
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                None | Some(Command::Shutdown) => break,
                Some(Command::Announce(updated)) => {
                    // The app also refreshes its snapshot periodically and after
                    // every rating. Resending every unchanged descriptor here
                    // would exhaust the history rate budget for full libraries.
                    broadcast_changes(&mut swarm, &topic, &snapshot, &updated);
                    snapshot = updated;
                }
                Some(Command::Advert(clip)) => publish(&mut swarm, &topic, &WireMessage::Advert { clip }),
                Some(Command::Receipt(receipt)) => publish(&mut swarm, &topic, &WireMessage::Receipt { receipt }),
                Some(Command::Dial(address)) => {
                    if let Ok(address) = validate_address(&address)
                        && retry_addresses.len() < 64 && !retry_addresses.contains(&address) {
                            retry_addresses.push(address);
                        }
                    if let Err(error) = validate_address(&address).and_then(|addr| swarm.dial(addr).map_err(Into::into)) {
                        let _ = events.try_send(PeerEvent::Error(format!("Peer connection failed: {error}")));
                    }
                }
            },
            _ = timer.tick() => {
                broadcast(&mut swarm, &topic, &snapshot);
                budgets.retain(|_, budget| budget.started.elapsed() < Duration::from_secs(120));
                for address in &retry_addresses {
                    let connected = address.iter().find_map(|part| match part {
                        Protocol::P2p(peer) => Some(swarm.is_connected(&peer)),
                        _ => None,
                    }).unwrap_or(false);
                    if !connected { let _ = swarm.dial(address.clone()); }
                }
            },
            event = swarm.select_next_some() => match event {
                SwarmEvent::NewListenAddr { address, .. } => {
                    let _ = events.try_send(PeerEvent::Listening(format!("{address}/p2p/{}", swarm.local_peer_id())));
                },
                SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                    if peer_ips.len() >= MAX_AUTHORS {
                        let _ = swarm.disconnect_peer_id(peer_id);
                        continue;
                    }
                    if let Some(ip) = endpoint.get_remote_address().iter().find_map(|part| match part {
                        Protocol::Ip4(ip) => Some(IpAddr::V4(ip)),
                        Protocol::Ip6(ip) => Some(IpAddr::V6(ip)),
                        _ => None,
                    }) { peer_ips.insert(peer_id, ip); }
                    swarm.behaviour_mut().gossip.add_explicit_peer(&peer_id);
                },
                SwarmEvent::ConnectionClosed { peer_id, num_established: 0, .. } => {
                    peer_ips.remove(&peer_id);
                    subscribed.remove(&peer_id);
                    let _ = events.try_send(PeerEvent::Disconnected(peer_id.to_string()));
                },
                SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                    for (peer_id, address) in list.into_iter().take(128) {
                        if peer_id != *swarm.local_peer_id() && !swarm.is_connected(&peer_id) {
                            swarm.behaviour_mut().gossip.add_explicit_peer(&peer_id);
                            let _ = swarm.dial(address.with(Protocol::P2p(peer_id)));
                        }
                    }
                },
                SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Expired(list))) => {
                    for (peer_id, _) in list {
                        swarm.behaviour_mut().gossip.remove_explicit_peer(&peer_id);
                    }
                },
                SwarmEvent::Behaviour(BehaviourEvent::Gossip(gossipsub::Event::Subscribed { peer_id, topic: remote_topic })) => {
                    if remote_topic == topic.hash() && subscribed.insert(peer_id) {
                        broadcast(&mut swarm, &topic, &snapshot);
                        // Replay exactly once per connection and only after the
                        // recipient can receive our topic. A raw transport event
                        // is too early and used to launch a second replay task.
                        let _ = events.try_send(PeerEvent::Connected(peer_id.to_string()));
                    }
                },
                SwarmEvent::Behaviour(BehaviourEvent::Gossip(gossipsub::Event::Message {
                    propagation_source, message_id, message,
                })) => {
                    let wire = (message.data.len() < MAX_WIRE).then(|| serde_json::from_slice::<WireMessage>(&message.data).ok()).flatten();
                    let valid = if let (Some(author), Some(wire)) = (message.source, wire.as_ref()) {
                        if !budgets.contains_key(&author) && budgets.len() >= MAX_AUTHORS { false }
                        else {
                            let budget = budgets.entry(author).or_insert_with(|| AuthorBudget {
                                started: Instant::now(), messages: 0,
                            });
                            validate_wire(wire) && budget.accept()
                        }
                    } else { false };
                    let result = if valid { gossipsub::MessageAcceptance::Accept } else { gossipsub::MessageAcceptance::Reject };
                    let _ = swarm.behaviour_mut().gossip.report_message_validation_result(&message_id, &propagation_source, result);
                    if !valid { continue; }
                    let Some(author) = message.source else { continue; };
                    match wire.expect("validated wire") {
                        WireMessage::Advert { clip } => { let _ = events.try_send(PeerEvent::Advert { peer: author.to_string(), clip }); },
                        WireMessage::Presence { seeded, torrent_port, permanent, wallet } => {
                            let torrent_addr = peer_ips.get(&author).map(|ip| SocketAddr::new(*ip, torrent_port));
                            let _ = events.try_send(PeerEvent::Presence { peer: author.to_string(), seeded, permanent, wallet, torrent_addr });
                        },
                        WireMessage::Receipt { receipt } => { let _ = events.try_send(PeerEvent::Receipt(receipt)); },
                    }
                },
                SwarmEvent::OutgoingConnectionError { error, .. } => {
                    let _ = events.try_send(PeerEvent::Error(format!("Peer unavailable: {error}")));
                },
                _ => {},
            }
        }
    }
    // Dropping the swarm closes every socket immediately.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_is_bounded_and_cannot_duplicate_seeder_identity() {
        let id = "a".repeat(64);
        assert!(validate_wire(&WireMessage::Presence {
            seeded: vec![id.clone()],
            torrent_port: 42832,
            permanent: vec![],
            wallet: None
        }));
        assert!(!validate_wire(&WireMessage::Presence {
            seeded: vec![id.clone(), id],
            torrent_port: 42832,
            permanent: vec![],
            wallet: None
        }));
        assert!(!validate_wire(&WireMessage::Presence {
            seeded: vec!["../evil".into()],
            torrent_port: 42832,
            permanent: vec![],
            wallet: None
        }));
    }

    #[test]
    fn permanent_seeds_must_be_available_and_wallet_announcements_are_bounded() {
        let id = "a".repeat(64);
        let valid = WireMessage::Presence {
            seeded: vec![id.clone()],
            permanent: vec![id.clone()],
            wallet: Some("0x1111111111111111111111111111111111111111".into()),
            torrent_port: 14000,
        };
        assert!(validate_wire(&valid));
        let mut value = serde_json::to_value(&valid).unwrap();
        value["permanent"] = serde_json::json!(["b".repeat(64)]);
        assert!(!validate_wire(&serde_json::from_value(value).unwrap()));
        let mut value = serde_json::to_value(valid).unwrap();
        value["wallet"] = "not-a-wallet".into();
        assert!(!validate_wire(&serde_json::from_value(value).unwrap()));
    }

    #[test]
    fn peer_addresses_require_real_tcp_port() {
        assert!(validate_address("/ip4/127.0.0.1/tcp/42831").is_ok());
        assert!(validate_address("/ip4/127.0.0.1/tcp/0").is_err());
        assert!(validate_address("https://example.com").is_err());
    }

    #[test]
    fn portable_identity_is_reused_and_corruption_is_not_reset() {
        let root = std::env::temp_dir().join(format!("clipcrop-peer-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let first = load_identity(&root).unwrap().public().to_peer_id();
        let second = load_identity(&root).unwrap().public().to_peer_id();
        assert_eq!(first, second);
        fs::write(root.join("identity.key"), b"corrupt").unwrap();
        assert!(load_identity(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn two_real_peers_exchange_authenticated_presence() {
        let (alice, mut alice_events) = start(identity::Keypair::generate_ed25519(), 0, vec![])
            .await
            .unwrap();
        let address = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(PeerEvent::Listening(address)) = alice_events.recv().await {
                    break address.replace("/ip4/0.0.0.0/", "/ip4/127.0.0.1/");
                }
            }
        })
        .await
        .unwrap();
        alice
            .announce(vec![], vec![], 14001, vec![], None)
            .await
            .unwrap();
        let (bob, mut bob_events) = start(identity::Keypair::generate_ed25519(), 0, vec![])
            .await
            .unwrap();
        bob.announce(vec![], vec![], 14002, vec![], None)
            .await
            .unwrap();
        bob.dial(address).await.unwrap();
        let presence = tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if let Some(PeerEvent::Presence {
                    peer, torrent_addr, ..
                }) = bob_events.recv().await
                {
                    if peer == alice.peer_id {
                        break torrent_addr;
                    }
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(presence, Some("127.0.0.1:14001".parse().unwrap()));
        alice.shutdown().await.unwrap();
        bob.shutdown().await.unwrap();
    }
}
