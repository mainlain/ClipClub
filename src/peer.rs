//! The application mesh. BitTorrent transfers run separately; this mesh only
//! carries bounded metadata, seeder presence and independently signed ratings.
//!
//! A message's author comes from authenticated GossipSub, never its JSON body.
//! Presence is an observation, not proof of storage or a global user census.

use crate::{
    core::{Clip, RatingReceipt},
    discovery::Discovery,
};
use anyhow::{Context, Result, anyhow, bail};
use futures::StreamExt;
use libp2p::{
    PeerId, Swarm, SwarmBuilder, connection_limits, gossipsub, identity, mdns,
    multiaddr::Protocol,
    noise,
    swarm::{NetworkBehaviour, SwarmEvent, behaviour::toggle::Toggle},
    tcp, upnp, yamux,
};
use librqbit::dht::{DhtBuilder, DhtConfig, Id20, RequestPeersStream};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::{self, OpenOptions},
    io::Write,
    net::{IpAddr, SocketAddr},
    path::Path,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

const TOPIC: &str = "clipcrop/catalog/2";
const MAX_WIRE: usize = 128 * 1024;
const MAX_SEEDED: usize = 100;
const MAX_AUTHORS: usize = 2048;
const REFRESH: Duration = Duration::from_secs(20);
const DISCOVERY_TICK: Duration = Duration::from_secs(5);
const MAX_DIAGNOSTIC_ERRORS: usize = 16;
const MAX_DIAGNOSTIC_ENDPOINTS: usize = 256;

/// A bounded, in-memory view of the running mesh. These observations are not
/// proof of multicast delivery or of a remote application's video availability.
#[derive(Clone, Debug, Default)]
pub struct PeerDiagnostics {
    pub mdns_requested: bool,
    pub mdns_enabled: bool,
    pub mdns_endpoints: Vec<String>,
    pub authenticated_peers: Vec<String>,
    pub subscribed_peers: usize,
    pub listen_addresses: Vec<String>,
    pub internet_discovery_running: bool,
    pub received_adverts: u64,
    pub received_presence: u64,
    pub rejected_messages: u64,
    pub recent_dial_errors: Vec<DiagnosticDialError>,
}

#[derive(Clone, Debug)]
pub struct DiagnosticDialError {
    pub at_ms: u64,
    pub detail: String,
}

fn remember_dial_error(errors: &mut VecDeque<DiagnosticDialError>, detail: String) {
    if errors.len() == MAX_DIAGNOSTIC_ERRORS {
        errors.pop_front();
    }
    errors.push_back(DiagnosticDialError {
        at_ms: crate::core::current_time_ms(),
        detail: detail.chars().take(512).collect(),
    });
}

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

    pub async fn shutdown(&self) -> Result<()> {
        self.send(Command::Shutdown).await
    }

    pub async fn diagnostics(&self) -> Result<PeerDiagnostics> {
        tokio::time::timeout(Duration::from_secs(3), async {
            let (reply, result) = oneshot::channel();
            self.send(Command::Diagnostics(reply)).await?;
            result.await.map_err(|_| anyhow!("Peer mesh has stopped"))
        })
        .await
        .context("Peer mesh did not respond to diagnostics within three seconds")?
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
    Diagnostics(oneshot::Sender<PeerDiagnostics>),
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
    mdns: Toggle<mdns::tokio::Behaviour>,
    upnp: Toggle<upnp::tokio::Behaviour>,
    limits: connection_limits::Behaviour,
}

/// Start on the application's Tokio runtime. Dropping the last handle stops the
/// task; no daemon or external application service is launched.
pub async fn start(
    identity: identity::Keypair,
    port: u16,
) -> Result<(PeerHandle, mpsc::Receiver<PeerEvent>)> {
    start_with_options(identity, port, DiscoveryOptions::default()).await
}

struct DiscoveryOptions {
    local: bool,
    port_mapping: bool,
    bootstrap: Option<Vec<String>>,
    #[cfg(test)]
    allow_local_dht: bool,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            local: true,
            port_mapping: true,
            bootstrap: None,
            #[cfg(test)]
            allow_local_dht: false,
        }
    }
}

/// This rendezvous key identifies the application protocol, not a video. DHT
/// results are only connection candidates; Noise and GossipSub authenticate them.
fn discovery_key() -> Id20 {
    Id20::new(Sha1::digest(format!("clipclub/peer-discovery/{TOPIC}")).into())
}

struct InternetDiscovery {
    peers: RequestPeersStream,
    cancel: CancellationToken,
}

impl InternetDiscovery {
    async fn new(port: u16, bootstrap: Option<Vec<String>>) -> Result<Self> {
        let cancel = CancellationToken::new();
        let dht = DhtBuilder::with_config(DhtConfig {
            listen_addr: Some("0.0.0.0:0".parse()?),
            bootstrap_addrs: bootstrap,
            cancellation_token: Some(cancel.clone()),
            ..Default::default()
        })
        .await?;
        // The library keeps looking up and renewing this announcement, even
        // with an empty catalog. No global torrent configuration is persisted.
        let peers = dht.get_peers(discovery_key(), Some(port));
        Ok(Self { peers, cancel })
    }
}

impl Drop for InternetDiscovery {
    fn drop(&mut self) {
        // DHT workers own an Arc: dropping the stream alone is insufficient.
        self.cancel.cancel();
    }
}

async fn start_with_options(
    identity: identity::Keypair,
    port: u16,
    options: DiscoveryOptions,
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
            let mdns = if options.local {
                // A machine without multicast support can still discover peers
                // over the internet. A short TTL also tracks interface changes.
                let mdns = mdns::tokio::Behaviour::new(
                    mdns::Config {
                        ttl: Duration::from_secs(60),
                        query_interval: Duration::from_secs(15),
                        ..Default::default()
                    },
                    key.public().to_peer_id(),
                );
                mdns.ok()
            } else {
                None
            };
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
                mdns: mdns.into(),
                upnp: options
                    .port_mapping
                    .then(upnp::tokio::Behaviour::default)
                    .into(),
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
    tokio::spawn(run(swarm, topic, command_rx, event_tx, options));
    Ok((
        PeerHandle {
            peer_id: id.to_string(),
            commands: command_tx,
        },
        event_rx,
    ))
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
    options: DiscoveryOptions,
) {
    let mut snapshot = Snapshot::default();
    let mut timer = tokio::time::interval(REFRESH);
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut peer_ips = HashMap::<PeerId, IpAddr>::new();
    let mut budgets = HashMap::<PeerId, AuthorBudget>::new();
    let mut subscribed = HashSet::<PeerId>::new();
    let mut discovery = Discovery::new(*swarm.local_peer_id());
    let mut discovery_timer = tokio::time::interval(DISCOVERY_TICK);
    discovery_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut internet: Option<InternetDiscovery> = None;
    let mut listen_port = None;
    let mut mdns_endpoints = HashSet::<(PeerId, libp2p::Multiaddr)>::new();
    let mut recent_dial_errors = VecDeque::new();
    let mut received_adverts = 0_u64;
    let mut received_presence = 0_u64;
    let mut rejected_messages = 0_u64;
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
                Some(Command::Diagnostics(reply)) => {
                    let current_mdns: HashSet<_> = swarm.behaviour().mdns.as_ref()
                        .map(|mdns| mdns.discovered_nodes().copied().take(MAX_DIAGNOSTIC_ENDPOINTS).collect())
                        .unwrap_or_default();
                    mdns_endpoints.retain(|(peer, _)| current_mdns.contains(peer));
                    let mut endpoints: Vec<_> = mdns_endpoints.iter()
                        .map(|(peer, address)| {
                            let mut address = address.clone();
                            if !address.iter().any(|part| matches!(part, Protocol::P2p(_))) {
                                address.push(Protocol::P2p(*peer));
                            }
                            address.to_string()
                        })
                        .collect();
                    endpoints.sort();
                    let mut authenticated_peers: Vec<_> = swarm.connected_peers().map(ToString::to_string).collect();
                    authenticated_peers.sort();
                    let mut listen_addresses: Vec<_> = swarm.listeners().take(MAX_DIAGNOSTIC_ENDPOINTS).map(ToString::to_string).collect();
                    listen_addresses.sort();
                    let _ = reply.send(PeerDiagnostics {
                        mdns_requested: options.local,
                        mdns_enabled: swarm.behaviour().mdns.as_ref().is_some(),
                        mdns_endpoints: endpoints,
                        authenticated_peers,
                        subscribed_peers: subscribed.len(),
                        listen_addresses,
                        internet_discovery_running: internet.is_some(),
                        received_adverts,
                        received_presence,
                        rejected_messages,
                        recent_dial_errors: recent_dial_errors.iter().cloned().collect(),
                    });
                },
            },
            _ = timer.tick() => {
                broadcast(&mut swarm, &topic, &snapshot);
                budgets.retain(|_, budget| budget.started.elapsed() < Duration::from_secs(120));
                if internet.is_none() && let Some(port) = listen_port {
                    match InternetDiscovery::new(port, options.bootstrap.clone()).await {
                        Ok(found) => internet = Some(found),
                        Err(error) => {
                            let _ = events.try_send(PeerEvent::Error(format!("Internet peer discovery will retry: {error}")));
                        }
                    }
                }
            },
            _ = discovery_timer.tick() => {
                if let Some(mdns) = swarm.behaviour().mdns.as_ref() {
                    discovery.refresh_local(mdns.discovered_nodes().copied());
                }
                for address in discovery.due() {
                    if let Err(error) = swarm.dial(address.clone()) {
                        remember_dial_error(&mut recent_dial_errors, format!("{address}: {error}"));
                    }
                }
            },
            endpoint = async {
                match internet.as_mut() {
                    Some(internet) => internet.peers.next().await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some(endpoint) = endpoint {
                    #[cfg(test)]
                    if options.allow_local_dht {
                        discovery.remember(socket_multiaddr(endpoint), None);
                    } else {
                        discovery.remember_internet(endpoint);
                    }
                    #[cfg(not(test))]
                    discovery.remember_internet(endpoint);
                } else {
                    internet = None;
                }
            },
            event = swarm.select_next_some() => match event {
                SwarmEvent::NewListenAddr { address, .. } => {
                    if listen_port.is_none() {
                        listen_port = address.iter().find_map(|part| match part {
                            Protocol::Tcp(port) if port != 0 => Some(port),
                            _ => None,
                        });
                        if let Some(port) = listen_port {
                            match InternetDiscovery::new(port, options.bootstrap.clone()).await {
                                Ok(found) => internet = Some(found),
                                Err(error) => {
                                    let _ = events.try_send(PeerEvent::Error(format!("Internet peer discovery will retry: {error}")));
                                }
                            }
                        }
                    }
                    let _ = events.try_send(PeerEvent::Listening(format!("{address}/p2p/{}", swarm.local_peer_id())));
                },
                SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                    if peer_ips.len() >= MAX_AUTHORS {
                        let _ = swarm.disconnect_peer_id(peer_id);
                        continue;
                    }
                    discovery.connected(peer_id, endpoint.get_remote_address());
                    if let Some(ip) = endpoint.get_remote_address().iter().find_map(|part| match part {
                        Protocol::Ip4(ip) => Some(IpAddr::V4(ip)),
                        Protocol::Ip6(ip) => Some(IpAddr::V6(ip)),
                        _ => None,
                    }) { peer_ips.insert(peer_id, ip); }
                    swarm.behaviour_mut().gossip.add_explicit_peer(&peer_id);
                },
                SwarmEvent::ConnectionClosed { peer_id, num_established: 0, .. } => {
                    discovery.disconnected(peer_id);
                    peer_ips.remove(&peer_id);
                    subscribed.remove(&peer_id);
                    swarm.behaviour_mut().gossip.remove_explicit_peer(&peer_id);
                    let _ = events.try_send(PeerEvent::Disconnected(peer_id.to_string()));
                },
                SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                    for (peer_id, address) in list.into_iter().take(128) {
                        if mdns_endpoints.len() < MAX_DIAGNOSTIC_ENDPOINTS {
                            mdns_endpoints.insert((peer_id, address.clone()));
                        }
                        discovery.remember(address.clone(), Some(peer_id));
                        if swarm.is_connected(&peer_id) {
                            discovery.connected(peer_id, &address);
                        }
                    }
                },
                SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Expired(list))) => {
                    for (peer_id, address) in list {
                        mdns_endpoints.remove(&(peer_id, address.clone()));
                        discovery.forget(&address, peer_id);
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
                SwarmEvent::Behaviour(BehaviourEvent::Gossip(gossipsub::Event::Unsubscribed { peer_id, topic: remote_topic })) => {
                    if remote_topic == topic.hash() {
                        subscribed.remove(&peer_id);
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
                    if !valid {
                        rejected_messages = rejected_messages.saturating_add(1);
                        continue;
                    }
                    let Some(author) = message.source else { continue; };
                    match wire.expect("validated wire") {
                        WireMessage::Advert { clip } => {
                            received_adverts = received_adverts.saturating_add(1);
                            let _ = events.try_send(PeerEvent::Advert { peer: author.to_string(), clip });
                        },
                        WireMessage::Presence { seeded, torrent_port, permanent, wallet } => {
                            received_presence = received_presence.saturating_add(1);
                            let torrent_addr = peer_ips.get(&author).map(|ip| SocketAddr::new(*ip, torrent_port));
                            let _ = events.try_send(PeerEvent::Presence { peer: author.to_string(), seeded, permanent, wallet, torrent_addr });
                        },
                        WireMessage::Receipt { receipt } => { let _ = events.try_send(PeerEvent::Receipt(receipt)); },
                    }
                },
                SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                    let peer = peer_id.map(|peer| peer.to_string()).unwrap_or_else(|| "unknown peer".into());
                    remember_dial_error(&mut recent_dial_errors, format!("{peer}: {error}"));
                },
                // Stale/unreachable discovery results are normal. Candidate
                // retries are bounded and do not overwrite application notices.
                _ => {},
            }
        }
    }
    // Dropping the swarm closes every socket immediately.
}

#[cfg(test)]
fn socket_multiaddr(endpoint: SocketAddr) -> libp2p::Multiaddr {
    let ip = match endpoint.ip() {
        IpAddr::V4(ip) => Protocol::Ip4(ip),
        IpAddr::V6(ip) => Protocol::Ip6(ip),
    };
    libp2p::Multiaddr::empty()
        .with(ip)
        .with(Protocol::Tcp(endpoint.port()))
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

    async fn presence_from(
        events: &mut mpsc::Receiver<PeerEvent>,
        expected: &str,
        port: u16,
    ) -> SocketAddr {
        tokio::time::timeout(Duration::from_secs(40), async {
            while let Some(event) = events.recv().await {
                if let PeerEvent::Presence {
                    peer,
                    torrent_addr: Some(address),
                    ..
                } = event
                    && peer == expected
                    && address.port() == port
                {
                    return address;
                }
            }
            panic!("mesh stopped before presence");
        })
        .await
        .expect("automatic discovery timed out")
    }

    #[tokio::test]
    #[ignore = "requires a multicast-capable LAN; run with --ignored"]
    async fn mdns_discovers_real_peers_without_manual_addresses() {
        let options = || DiscoveryOptions {
            port_mapping: false,
            bootstrap: Some(vec!["127.0.0.1:9".into()]),
            ..Default::default()
        };
        let (alice, mut alice_events) =
            start_with_options(identity::Keypair::generate_ed25519(), 0, options())
                .await
                .unwrap();
        let (bob, mut bob_events) =
            start_with_options(identity::Keypair::generate_ed25519(), 0, options())
                .await
                .unwrap();
        alice
            .announce(vec![], vec![], 14001, vec![], None)
            .await
            .unwrap();
        bob.announce(vec![], vec![], 14002, vec![], None)
            .await
            .unwrap();
        let (alice_endpoint, bob_endpoint) = tokio::join!(
            presence_from(&mut bob_events, &alice.peer_id, 14001),
            presence_from(&mut alice_events, &bob.peer_id, 14002),
        );
        assert_eq!(alice_endpoint.port(), 14001);
        assert_eq!(bob_endpoint.port(), 14002);
        alice.shutdown().await.unwrap();
        bob.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn dht_discovers_real_peers_without_mdns_or_manual_addresses() {
        // Local Mainline node: exercise actual get_peers/announce_peer packets
        // without contacting public infrastructure. Its ID is close enough to
        // the rendezvous key to store that key under the library's distance cap.
        let seed = DhtBuilder::with_config(DhtConfig {
            peer_id: Some(discovery_key()),
            listen_addr: Some("127.0.0.1:0".parse().unwrap()),
            bootstrap_addrs: Some(vec!["127.0.0.1:9".into()]),
            ..Default::default()
        })
        .await
        .unwrap();
        let _seed_guard = seed.cancellation_token().clone().drop_guard();
        // Bootstrap replies must include a routing contact, as on the public
        // network. An isolated empty routing table cannot bootstrap another node.
        let mut table = seed.with_routing_tables(|table, _| table.clone());
        table.add_node(discovery_key(), seed.listen_addr());
        let router = DhtBuilder::with_config(DhtConfig {
            peer_id: Some(discovery_key()),
            listen_addr: Some("127.0.0.1:0".parse().unwrap()),
            bootstrap_addrs: Some(vec![seed.listen_addr().to_string()]),
            routing_table: Some(table),
            ..Default::default()
        })
        .await
        .unwrap();
        let _router_guard = router.cancellation_token().clone().drop_guard();
        let options = || DiscoveryOptions {
            local: false,
            port_mapping: false,
            bootstrap: Some(vec![router.listen_addr().to_string()]),
            allow_local_dht: true,
        };
        let alice_identity = identity::Keypair::generate_ed25519();
        let (alice, mut alice_events) = start_with_options(alice_identity.clone(), 0, options())
            .await
            .unwrap();
        let alice_port = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = alice_events.recv().await {
                if let PeerEvent::Listening(address) = event {
                    let address: libp2p::Multiaddr = address.parse().unwrap();
                    if let Some(port) = address.iter().find_map(|part| match part {
                        Protocol::Tcp(port) => Some(port),
                        _ => None,
                    }) {
                        return port;
                    }
                }
            }
            panic!("mesh stopped before listening");
        })
        .await
        .unwrap();
        let (bob, mut bob_events) =
            start_with_options(identity::Keypair::generate_ed25519(), 0, options())
                .await
                .unwrap();
        alice
            .announce(vec![], vec![], 14001, vec![], None)
            .await
            .unwrap();
        bob.announce(vec![], vec![], 14002, vec![], None)
            .await
            .unwrap();
        let (alice_endpoint, bob_endpoint) = tokio::join!(
            presence_from(&mut bob_events, &alice.peer_id, 14001),
            presence_from(&mut alice_events, &bob.peer_id, 14002),
        );
        assert_eq!(alice_endpoint, "127.0.0.1:14001".parse().unwrap());
        assert_eq!(bob_endpoint, "127.0.0.1:14002".parse().unwrap());
        alice.shutdown().await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while alice_events.recv().await.is_some() {}
            while let Some(event) = bob_events.recv().await {
                if matches!(event, PeerEvent::Disconnected(peer) if peer == alice.peer_id) {
                    return;
                }
            }
            panic!("remaining mesh stopped before disconnect");
        })
        .await
        .unwrap();
        let (restarted, _restarted_events) =
            start_with_options(alice_identity, alice_port, options())
                .await
                .unwrap();
        assert_eq!(restarted.peer_id, alice.peer_id);
        restarted
            .announce(vec![], vec![], 14003, vec![], None)
            .await
            .unwrap();
        let endpoint = presence_from(&mut bob_events, &restarted.peer_id, 14003).await;
        assert_eq!(endpoint, "127.0.0.1:14003".parse().unwrap());
        restarted.shutdown().await.unwrap();
        bob.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn internet_discovery_cancels_workers_on_drop() {
        let discovery = InternetDiscovery::new(14001, Some(vec!["127.0.0.1:9".into()]))
            .await
            .unwrap();
        let cancel = discovery.cancel.clone();
        drop(discovery);
        assert!(cancel.is_cancelled());
    }
}
