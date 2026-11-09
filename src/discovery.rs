//! Bounded, expiring connection candidates shared by local and Internet discovery.

use libp2p::{Multiaddr, PeerId, multiaddr::Protocol};
use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::{Duration, Instant},
};

const MAX_CANDIDATES: usize = 256;
const MAX_DIALS: usize = 16;
const INITIAL_RETRY: Duration = Duration::from_secs(5);
const MAX_RETRY: Duration = Duration::from_secs(5 * 60);
const CANDIDATE_TTL: Duration = Duration::from_secs(10 * 60);

struct Candidate {
    peer: Option<PeerId>,
    connected: bool,
    last_seen: Instant,
    next_try: Instant,
    retry: Duration,
}

pub(crate) struct Discovery {
    local: PeerId,
    candidates: HashMap<Multiaddr, Candidate>,
}

impl Discovery {
    pub(crate) fn new(local: PeerId) -> Self {
        Self {
            local,
            candidates: HashMap::new(),
        }
    }

    pub(crate) fn remember(&mut self, address: Multiaddr, peer: Option<PeerId>) {
        self.remember_at(address, peer, Instant::now());
    }

    fn remember_at(&mut self, address: Multiaddr, peer: Option<PeerId>, now: Instant) {
        let Some((address, embedded_peer)) = direct_endpoint(&address) else {
            return;
        };
        if peer.is_some() && embedded_peer.is_some() && peer != embedded_peer {
            return;
        }
        let peer = peer.or(embedded_peer);
        if peer == Some(self.local) {
            return;
        }
        self.expire(now);
        if let Some(candidate) = self.candidates.get_mut(&address) {
            // Discovery cannot replace the identity authenticated at an endpoint.
            if candidate.peer.is_some() && peer.is_some() && candidate.peer != peer {
                return;
            }
            candidate.peer = candidate.peer.or(peer);
            candidate.last_seen = now;
            return;
        }
        if self.candidates.len() < MAX_CANDIDATES {
            let connected = peer.is_some_and(|peer| {
                self.candidates
                    .values()
                    .any(|candidate| candidate.peer == Some(peer) && candidate.connected)
            });
            self.candidates.insert(
                address,
                Candidate {
                    peer,
                    connected,
                    last_seen: now,
                    next_try: now,
                    retry: INITIAL_RETRY,
                },
            );
        }
    }

    pub(crate) fn remember_internet(&mut self, endpoint: SocketAddr) {
        // The application transfer engine currently supports IPv4. Public DHT
        // responses must never instruct connections to local or reserved ranges.
        let IpAddr::V4(ip) = endpoint.ip() else {
            return;
        };
        if endpoint.port() != 0 && public_ipv4(ip) {
            self.remember(
                Multiaddr::empty()
                    .with(Protocol::Ip4(ip))
                    .with(Protocol::Tcp(endpoint.port())),
                None,
            );
        }
    }

    /// mDNS silently renews existing records instead of emitting `Discovered`
    /// again. Keep those candidates alive while its current inventory has them.
    pub(crate) fn refresh_local(&mut self, peers: impl IntoIterator<Item = PeerId>) {
        self.refresh_local_at(peers, Instant::now());
    }

    fn refresh_local_at(&mut self, peers: impl IntoIterator<Item = PeerId>, now: Instant) {
        let peers: HashSet<_> = peers.into_iter().take(MAX_CANDIDATES).collect();
        for candidate in self.candidates.values_mut() {
            if candidate.peer.is_some_and(|peer| peers.contains(&peer)) {
                candidate.last_seen = now;
            }
        }
    }

    pub(crate) fn forget(&mut self, address: &Multiaddr, peer: PeerId) {
        let Some((address, _)) = direct_endpoint(address) else {
            return;
        };
        if self
            .candidates
            .get(&address)
            .is_some_and(|candidate| candidate.peer == Some(peer) && !candidate.connected)
        {
            self.candidates.remove(&address);
        }
    }

    pub(crate) fn connected(&mut self, peer: PeerId, address: &Multiaddr) {
        if peer == self.local {
            return;
        }
        let endpoint = direct_endpoint(address).map(|(endpoint, _)| endpoint);
        for (address, candidate) in &mut self.candidates {
            if candidate.peer == Some(peer) || endpoint.as_ref() == Some(address) {
                candidate.peer = Some(peer);
                candidate.connected = true;
                candidate.retry = INITIAL_RETRY;
            }
        }
    }

    pub(crate) fn disconnected(&mut self, peer: PeerId) {
        self.disconnected_at(peer, Instant::now());
    }

    fn disconnected_at(&mut self, peer: PeerId, now: Instant) {
        for candidate in self.candidates.values_mut() {
            if candidate.peer == Some(peer) {
                candidate.connected = false;
                candidate.next_try = now + INITIAL_RETRY;
                candidate.retry = INITIAL_RETRY;
            }
        }
    }

    pub(crate) fn due(&mut self) -> Vec<Multiaddr> {
        self.due_at(Instant::now())
    }

    fn due_at(&mut self, now: Instant) -> Vec<Multiaddr> {
        self.expire(now);
        let mut ready: Vec<_> = self
            .candidates
            .iter()
            .filter(|(_, candidate)| !candidate.connected && candidate.next_try <= now)
            .map(|(address, candidate)| (candidate.next_try, address.clone()))
            .collect();
        ready.sort_by(|(left_time, left_address), (right_time, right_address)| {
            left_time
                .cmp(right_time)
                .then_with(|| left_address.to_vec().cmp(&right_address.to_vec()))
        });
        ready
            .into_iter()
            .take(MAX_DIALS)
            .map(|(_, address)| {
                let candidate = self.candidates.get_mut(&address).expect("known candidate");
                candidate.next_try = now + candidate.retry;
                candidate.retry = (candidate.retry * 2).min(MAX_RETRY);
                match candidate.peer {
                    Some(peer) => address.with(Protocol::P2p(peer)),
                    None => address,
                }
            })
            .collect()
    }

    fn expire(&mut self, now: Instant) {
        self.candidates
            .retain(|_, candidate| now.duration_since(candidate.last_seen) < CANDIDATE_TTL);
    }
}

fn direct_endpoint(address: &Multiaddr) -> Option<(Multiaddr, Option<PeerId>)> {
    let mut parts = address.iter();
    let ip = match parts.next()? {
        Protocol::Ip4(ip) if !ip.is_unspecified() && !ip.is_multicast() && !ip.is_broadcast() => {
            Protocol::Ip4(ip)
        }
        Protocol::Ip6(ip) if !ip.is_unspecified() && !ip.is_multicast() => Protocol::Ip6(ip),
        _ => return None,
    };
    let Protocol::Tcp(port) = parts.next()? else {
        return None;
    };
    if port == 0 {
        return None;
    }
    let peer = match parts.next() {
        Some(Protocol::P2p(peer)) => Some(peer),
        None => None,
        _ => return None,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((Multiaddr::empty().with(ip).with(Protocol::Tcp(port)), peer))
}

fn public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !matches!(a, 0 | 10 | 127 | 224..=255)
        && !(a == 100 && (64..=127).contains(&b))
        && !(a == 169 && b == 254)
        && !(a == 172 && (16..=31).contains(&b))
        && !(a == 192 && (b == 168 || (b == 0 && matches!(c, 0 | 2))))
        && !(a == 198 && (matches!(b, 18 | 19) || (b == 51 && c == 100)))
        && !(a == 203 && b == 0 && c == 113)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> PeerId {
        libp2p::identity::Keypair::generate_ed25519()
            .public()
            .to_peer_id()
    }

    fn endpoint(port: u16) -> Multiaddr {
        format!("/ip4/127.0.0.1/tcp/{port}").parse().unwrap()
    }

    #[test]
    fn identities_and_addresses_are_deduplicated_and_validated() {
        let local = id();
        let remote = id();
        let mut discovery = Discovery::new(local);
        let now = Instant::now();
        discovery.remember_at(endpoint(1000), Some(remote), now);
        discovery.remember_at(
            endpoint(1000).with(Protocol::P2p(remote)),
            Some(remote),
            now,
        );
        discovery.remember_at(endpoint(1001), Some(local), now);
        discovery.remember_at(endpoint(1002).with(Protocol::P2p(remote)), Some(id()), now);
        discovery.remember_at(endpoint(0), None, now);
        discovery.remember_at("/ip4/0.0.0.0/tcp/1003".parse().unwrap(), None, now);
        discovery.remember_at(
            endpoint(1004)
                .with(Protocol::P2p(remote))
                .with(Protocol::P2p(remote)),
            None,
            now,
        );
        assert_eq!(discovery.candidates.len(), 1);
        assert_eq!(
            discovery.due_at(now),
            vec![endpoint(1000).with(Protocol::P2p(remote))]
        );
    }

    #[test]
    fn internet_discovery_rejects_local_reserved_and_invalid_endpoints() {
        let mut discovery = Discovery::new(id());
        for address in [
            "0.1.2.3:80",
            "10.0.0.1:80",
            "100.64.0.1:80",
            "127.0.0.1:80",
            "169.254.1.1:80",
            "172.16.0.1:80",
            "192.168.1.1:80",
            "192.0.0.1:80",
            "192.0.2.1:80",
            "198.18.0.1:80",
            "198.51.100.1:80",
            "203.0.113.1:80",
            "224.0.0.1:80",
            "255.255.255.255:80",
            "8.8.8.8:0",
            "[::1]:80",
            "[2001:4860:4860::8888]:80",
        ] {
            discovery.remember_internet(address.parse().unwrap());
        }
        assert!(discovery.candidates.is_empty());
        discovery.remember_internet("8.8.8.8:1234".parse().unwrap());
        assert_eq!(discovery.candidates.len(), 1);
    }

    #[test]
    fn retries_back_off_and_refresh_does_not_reset_the_delay() {
        let mut discovery = Discovery::new(id());
        let address = endpoint(1000);
        let start = Instant::now();
        discovery.remember_at(address.clone(), None, start);
        let mut now = start;
        for expected in [5, 10, 20, 40, 80, 160, 300, 300] {
            discovery.remember_at(address.clone(), None, now);
            assert_eq!(discovery.due_at(now), vec![address.clone()]);
            let next = discovery.candidates[&address].next_try;
            assert_eq!(next.duration_since(now), Duration::from_secs(expected));
            assert!(discovery.due_at(next - Duration::from_millis(1)).is_empty());
            now = next;
        }
    }

    #[test]
    fn discovery_and_each_dial_batch_are_bounded() {
        let mut discovery = Discovery::new(id());
        let now = Instant::now();
        for port in 1..=300 {
            discovery.remember_at(endpoint(port), None, now);
        }
        assert_eq!(discovery.candidates.len(), MAX_CANDIDATES);
        let first = discovery.due_at(now);
        assert_eq!(first.len(), MAX_DIALS);
        let second = discovery.due_at(now);
        assert_eq!(second.len(), MAX_DIALS);
        assert!(first.iter().all(|address| !second.contains(address)));
        assert!(discovery.due_at(now + CANDIDATE_TTL).is_empty());
        assert!(discovery.candidates.is_empty());
    }

    #[test]
    fn authenticated_connections_stop_dialing_and_disconnects_retry() {
        let mut discovery = Discovery::new(id());
        let remote = id();
        let address = endpoint(1000);
        let now = Instant::now();
        discovery.remember_at(address.clone(), None, now);
        discovery.connected(remote, &address.with(Protocol::P2p(remote)));
        discovery.remember_at(endpoint(1001), Some(remote), now);
        assert!(discovery.due_at(now + INITIAL_RETRY).is_empty());
        discovery.disconnected_at(remote, now + INITIAL_RETRY);
        assert!(discovery.due_at(now + INITIAL_RETRY).is_empty());
        assert_eq!(discovery.due_at(now + INITIAL_RETRY * 2).len(), 2);
    }

    #[test]
    fn expiration_removes_only_the_discovered_endpoint_until_refreshed() {
        let mut discovery = Discovery::new(id());
        let remote = id();
        let address = endpoint(1000);
        let now = Instant::now();
        discovery.remember_at(address.clone(), Some(remote), now);
        discovery.forget(&address, id());
        assert_eq!(discovery.candidates.len(), 1);
        discovery.connected(remote, &address);
        discovery.forget(&address, remote);
        assert_eq!(discovery.candidates.len(), 1);
        discovery.disconnected_at(remote, now);
        discovery.forget(&address, remote);
        assert!(discovery.candidates.is_empty());
        discovery.remember_at(address.clone(), Some(remote), now);
        discovery.remember_at(address, Some(remote), now + CANDIDATE_TTL / 2);
        discovery.due_at(now + CANDIDATE_TTL);
        assert_eq!(discovery.candidates.len(), 1);
        discovery.due_at(now + CANDIDATE_TTL + CANDIDATE_TTL / 2);
        assert!(discovery.candidates.is_empty());
    }

    #[test]
    fn silent_mdns_renewals_keep_candidates_available_for_reconnection() {
        let mut discovery = Discovery::new(id());
        let remote = id();
        let address = endpoint(1000);
        let now = Instant::now();
        discovery.remember_at(address.clone(), Some(remote), now);
        discovery.refresh_local_at([remote], now + CANDIDATE_TTL);
        assert_eq!(
            discovery.due_at(now + CANDIDATE_TTL),
            vec![address.with(Protocol::P2p(remote))]
        );
        assert!(discovery.due_at(now + CANDIDATE_TTL * 2).is_empty());
    }
}
