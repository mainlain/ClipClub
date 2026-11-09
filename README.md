# clipclub

**좋아 is deployed on MegaETH mainnet at [`0x98d49dC44f1CbbA5ABef438985b20A729dE4ab16`](https://mega.etherscan.io/address/0x98d49dC44f1CbbA5ABef438985b20A729dE4ab16).** Each eligible permanent seeder receives `rating × y(x) / 100` 좋아, rounded down to six decimals, using the selected normalized logistic curve. The rater pays exactly the rating score. Unused rating contributions carry forward as wallet-scoped credit, and settlement automatically mints any remaining reward shortfall. Rating payments never spend roll escrow.

The desktop app calculates rewards locally from registered permanent seeder wallets and a persistent 30-day active-peer population estimate, with **N ≥ 21**. Recipients and rewards are frozen when queued and settled together on-chain. See [rating distribution rules](docs/RATING_DISTRIBUTION.md) for the formula, eligibility, funding and trust model.

A Rust desktop app for sharing short videos through BitTorrent and paying with **좋아 (✨)**, its Ethereum-compatible ERC-20 token on **MegaETH**. The token uses six decimals and a fireworks logo. There is no application account server or hosted video service. The interface and its assets are embedded in the executable.

Token balances and payments live on MegaETH. Rolls commit to a future drand quicknet round and settle through MegaETH’s deployed verifier. The source includes the contracts and deployment tools. The mainnet deployment created an initial supply of 1,000,000,000 ✨ for treasury `0xcBBF8e516F9a3ad56Efe28A0aEBf69Edc4639637`. The app defaults to the deployed mainnet contract. Set up your built-in wallet on first launch and unlock it when opening the app. Token actions require your approval; sharing and free skips require no payment.

## Run

On Apple Silicon macOS, open `dist/clipclub.app`, or run the executable:

```sh
./dist/clipclub
```

On macOS, your profile is stored in `~/Library/Application Support/ClipCrop Points`. On Linux, every launch method uses `$XDG_DATA_HOME/clipcrop-points`, defaulting to `~/.local/share/clipcrop-points`; empty or relative `XDG_DATA_HOME` values use the default. Neither platform searches Downloads for a profile, and the app can be moved without moving this data. On other platforms, the executable defaults to an adjacent `clipcrop-points-data` directory; keep that directory when moving the app. Use `--data-dir PATH` to explicitly choose a different profile folder on any platform.

The profile contains your private peer identity, legacy rating history, watched history, uploaded files, cached videos, an encrypted wallet keystore when you create or import a wallet, and a public-wallet/roll journal. Back up the entire profile directory. New peer identities receive no cryptocurrency grant; creating profiles cannot mint ✨. To open a copy of a profile, use `--data-dir /path/to/profile-copy`. Do not share `identity.key` with other people.

The app closes its network sessions when you quit. It resumes your permanent library after reopening and revalidating the stored videos. It does not install a background service.

## Linux

Linux x86-64 builds target Ubuntu 22.04 or a compatible newer distribution with GTK 3 / WebKitGTK 4.1. The video validator is embedded; the OS supplies the desktop WebView and GStreamer playback codecs.

See [Linux run and build instructions](docs/LINUX.md) for runtime packages, portable data storage, and the optional application-menu launcher. The Linux release artifacts and build/test details are recorded alongside the packaged files.

## App updates

clipclub checks the [latest published GitHub release](https://github.com/mainlain/ClipClub/releases/latest) at startup and every six hours. Settings shows the installed version, available update, release notes, and a manual check. Drafts and prereleases are excluded. GitHub access is anonymous; the app does not contain a GitHub credential.

On a writable macOS app bundle or portable Linux installation, **Install and restart** downloads the matching OS/CPU package, verifies GitHub's SHA-256 asset digest, validates the package, and replaces the app after it exits. A failed replacement restores the previous app. Your identity, settings, built-in wallet, and clips stay in the existing profile folder. A direct macOS executable and system-managed installations such as a Debian package offer the matching download for manual installation. An update is installed only when you choose it.

Release builds support Apple Silicon macOS and Linux x86-64. See [release publishing and update packaging](docs/RELEASES.md) for the tag-driven build workflow.

## The four screens

After wallet setup or startup unlock, the app opens in a compact, resizable window with four menu rows: Upload, Rate, Roll, and Settings. Your wallet’s ✨ balance stays in the bottom-right corner, with peer status in the bottom-left. Click the balance to open wallet details. Use **← Menu** or Escape to return to the main menu. Rating sessions start when you enter Rate and end when you leave it.

- **Seed / upload:** select or drop MP4 or WebM videos. Files pass validation before they enter your library. New clips become torrents and begin seeding automatically. Uploading the exact same bytes joins the same deterministic torrent, even if the original filename differs. You can keep up to 100 permanent clips. Removing a clip stops its permanent seeding. Each clip shows its average rating out of 9 and rating count. Sort by recency or rating. Register your seeder wallet in Settings to receive rewards for your permanently seeded clips.
- **Rate:** videos loop until you press a number from 0–9 or use a rating button. A positive number queues that many ✨ as your cost. The app calculates rewards for eligible permanent seeders locally, uses your unused rating credit, and automatically mints any remaining shortfall. Queue up to 100 ratings and 1,000 seeder payments, then settle them together with one wallet-approved transaction. Zero records the clip as watched without paying anyone. Positive ratings stay unpaid and private until the session batch settles. Positive ratings temporarily seed the watched clip for the rest of the rating session; leaving Rate ends those temporary seeds. You cannot rate your own permanently seeded clips or rate the same content twice.
- **Roll:** commit ✨ for a verified multiplier from 0 through 9. After the future drand round is published, submit its proof to settle. A 0 burns the wager and plays no clip; a 1 returns the stake; 2–9 return the stake and mint the profit. Total payout is `wager × multiplier`. Positive results play one clip from that rating tier, without looping. Previously watched clips and clips in your own library are eligible.
- **Settings:** inspect the token network, contract, wallet, and unconfirmed transactions. Network details show automatic discovery status, the local peer identity, and connection addresses. Peers connect automatically without entering addresses.

Videos start with sound on. Use the video player's controls to adjust volume or mute.

Rate prefetch starts with the app. Its downloads run one at a time, in a shuffled order, and completed files are validated before they become playable. The app targets ten unwatched ready clips and fills vacancies as you rate. It can only fill that buffer when the connected network has enough eligible clips and reachable seeders. Pending queue downloads do not announce themselves as seeders before completion. Roll prepares its result through a separate download session and cache, with the same validation, so Rate cleanup cannot remove a playing roll clip.

## File restrictions

| Restriction | Current value |
| --- | --- |
| Containers | MP4, WebM |
| Maximum file size | 8,000,000 bytes (decimal 8 MB) |
| Maximum duration | 180 seconds |
| Maximum width and height | 2160 pixels each |
| Permanent library | 100 clips per identity |

File limits are defined by the authoritative constants in `src/core.rs` and supplied through the local API for the interface to display. Validation, upload limits, and probing use that policy. Changing network-wide limits should also change the protocol version: peers must agree on what they accept.

The current peer protocol uses `clipcrop/catalog/2` and identifies peers by their peer IDs. Peers on the older display-name protocol must update to exchange clips and ratings with this version.

Validation checks real bytes and decoded frames with `ffprobe`, including container, streams, dimensions, frame timestamps, and duration. Only one video stream and at most one audio stream are accepted; attachments, subtitles, data streams, and unknown codecs are rejected. Network downloads receive the same checks as uploads. SHA-256 verifies content identity, and canonical single-file torrent metadata is checked before downloading. Paths and filenames from peers are never used as arbitrary filesystem destinations.

The validator has a timeout, bounded output and allocation settings, a demuxer allowlist, and a file-only protocol allowlist. These measures are defense in depth, not an operating-system sandbox or a guarantee against decoder vulnerabilities. The player uses the OS WebView, so codec availability still depends on the platform. A valid MP4/WebM file may use a codec unavailable in that WebView. There is no transcoding or separately bundled playback engine.

## 좋아 on MegaETH

The name is **좋아**, the ERC-20 symbol is **✨**, and the logo is the [multicolor fireworks image](assets/fireworks.png), shared with the desktop icon. A [256×256 PNG](assets/joah-256.png) is available for token lists and wallets. Explorer-specific sizes and export instructions are in [the asset guide](assets/README.md). One token is 1,000,000 base units. Every monetary API uses decimal integer strings; neither JavaScript numbers nor floating-point JSON amounts determine balances or payouts.

The [mainnet deployment transaction](https://mega.etherscan.io/tx/0x776686e41987d79d2eda546eab5c74e9fd38f3e6ad1678b1195194c87441211f) was included in block **31,031,878** on 2026-11-04. Its full initcode, token metadata, immutable verifier and genesis mint to the selected treasury were independently checked through the mainnet RPC. The public record is in [contracts/mainnet-deployment.json](contracts/mainnet-deployment.json). Deployment commands reject a second deployment of this launch.

Open the app normally to use the deployed mainnet token. You can also select the address explicitly:

```sh
CLIPCLUB_MEGAETH_CHAIN_ID=4326 \
CLIPCLUB_JOAH_CONTRACT=0x98d49dC44f1CbbA5ABef438985b20A729dE4ab16 \
./dist/clipclub
```

`6343` selects MegaETH testnet; `4326` selects mainnet, the project default. Testnet requires an explicit contract address and never inherits the mainnet token. Set `CLIPCLUB_JOAH_CONTRACT` to an empty string to disable token configuration. The default RPCs are `https://carrot.megaeth.com/rpc` and `https://mainnet.megaeth.com/rpc`. `CLIPCLUB_MEGAETH_RPC_URL` optionally selects a trusted HTTPS RPC. The app checks its chain ID. Custom RPC credentials are never sent to the UI. All peers that exchange paid ratings must configure the same chain and contract. Wallet keys are generated locally or imported by you. There are no embedded shared keys, API subscriptions, or automatically funded wallets.

On first launch, set up your wallet before entering the app: create a built-in Ethereum wallet, import a 32-byte private key, or restore an encrypted ClipClub wallet backup. New and imported keys are encrypted with a password of at least 12 characters and stored in your profile. On later launches, enter your password at startup. The wallet stays unlocked for the entire app session and locks only when you quit; it has no inactivity timeout or manual lock control. Passwords and imported keys are cleared from the form and are never saved in browser storage. Open **Wallet** or **Wallet details** in Settings and choose **Download encrypted backup**. Keep that file and its password safely; there is no password reset or recovery service.

The built-in wallet signs inside the app. Each payment opens a review showing the action, network, sender, token contract, token amount and maximum gas fee. **Approve and send** authorizes that single prepared transaction; **Cancel** leaves the payment unpaid. The app validates the account, configured contract and chain, estimates gas against the selected MegaETH network, and saves the signed transaction before broadcasting. **Retry confirmation** can rebroadcast the same bytes without signing another payment. ETH on the selected network pays gas. The receive address accepts ETH and 좋아 (✨) on MegaETH; sending funds on another network does not make them available here.

The wallet manages one private key per profile and signs the app’s token actions on its configured MegaETH mainnet or testnet. It does not offer seed phrases, arbitrary dapp connections, arbitrary message signing, or a general-purpose transfer screen. Backups use ClipClub’s encrypted keystore format.

The constructor creates 1,000,000,000 ✨ once for the selected treasury. There is no public mint function or per-profile grant. Existing points receipts remain compatible for historical clip ratings, but cannot pay for anything or redeem tokens. Initial token distribution is an operator decision; the app does not assume a conversion rate or a faucet.

Paid ratings debit `score × 1,000,000` existing units from the rater and distribute locally calculated rewards to registered permanent seeder wallets. Unused rating contributions carry forward as credit for that wallet's future seeder rewards. Settlement automatically mints any shortfall left after applying credit; no treasury grant, sponsor deposit or prefunding is required. Settings displays unused credit, and session review estimates the new tokens to be minted.

Positive ratings are queued in a durable local session draft with frozen recipients, population and per-seeder rewards. Up to 100 ratings and 1,000 seeder payments settle together in one wallet-approved transaction and one gas fee. Any invalid item, insufficient rater balance or exhausted arithmetic capacity reverts the entire batch, including minting. Individual videos never prompt a wallet payment.

Register a seeder wallet with your peer identity in Settings. Authenticated peer announcements distinguish permanent seeds from temporary session seeds, and the app verifies each advertised wallet’s on-chain peer binding. A wallet can pay for each content ID once and cannot pay itself. Zero is a free local skip. Seeder registration and presence do not prove actual storage, authorship or a unique human. Local clients can be modified, and the contract does not independently verify seeding or the curve. Automatic reward minting accepts distributions within its registration, reward and batch bounds; there is no additional attestation gate or emissions schedule. The old first-registered clip wallet APIs remain only for legacy drafts and never mint seeder rewards.

Ending a session offers one batch settlement; navigating away or quitting keeps the unpaid draft for later. A rejected wallet approval leaves the draft intact. Submitted batches are frozen until confirmation or a verified reverted receipt. Successful wallet speed-ups can be recovered using their replacement transaction hash. Retries confirm the existing transaction hash without paying twice. Separate wallet nonce-cancellation transactions are not automatically recovered. Discarding an unsubmitted draft makes no payment and adds no public ratings. Confirmed batches are written to the local receipt history in one atomic commit.

Peer receipts retain rating metadata and payment references; the Rust adapter validates paid receipts against canonical successful contract events, rather than trusting peer-reported balances. Each paid event commits to the rater’s peer identity, so another peer cannot copy the public payment proof to impersonate that rating.

### Verifiable rolls and supply

`requestRoll(wager)` escrows the wager and commits to the current drand quicknet round plus two. The round, wallet, wager, and roll ID cannot be changed. After publication, `settleRoll(id, signature)` verifies the proof through MegaETH’s canonical quicknet verifier and derives a game-, chain-, player-, and roll-specific ticket from the normalized verified hash. Rejection sampling avoids modulo bias. The exact outcome weights are 512, 256, 128, 64, 32, 16, 8, 4, 2, and 1 out of 1023 for multipliers 0–9.

| Result | Wallet receives | Total supply change |
| --- | --- | --- |
| 0× loss | 0 | `−wager`, permanently burned |
| 1× push | Original stake | 0 |
| 2–9× win | `wager × multiplier` | `+wager × (multiplier − 1)`, minted profit |

Roll and seeder-rating settlements can mint after genesis. Rating minting preserves the supply headroom reserved at commitment so even extreme pending roll payouts remain settleable. The weights have expected total payout `1013/1023` of a wager (before gas); an individual win increases supply even though the average roll decreases it.

Anyone can submit the committed round’s proof. The UI provides a settlement button and links to the transaction and drand round. One roll per wallet can remain pending; outstanding IDs are recovered from the contract after a restart. Pending transaction hashes are saved to the profile journal before confirmation; **Retry confirmation** never sends the transaction again. Import a request or settlement hash to restore a historical roll. A transaction rejected by the wallet creates no wager. A successful commitment remains escrowed until the same roll settles. There is no timeout refund or player-selected replacement round; drand/verifier/network availability is a settlement dependency. This repository includes manual permissionless settlement, not a separately running keeper.

MegaETH inclusion receipts are checked against the RPC’s canonical block; inclusion is not a promise of Ethereum L1 finality. Wallet users and operators must account for reorg/finality requirements when distributing or accepting tokens. Contract source and local tests are included; no independent contract audit is implied.

Clips are separate from randomness and settlement. A positive roll chooses a reachable clip whose average positive rating rounds to the multiplier. Unrated clips are excluded. Previously watched and owned clips remain eligible. Missing or unplayable media cannot alter the committed result or token settlement. Video bytes and catalogs stay off-chain. Clip selection is local; the **monetary multiplier** is the provably verified draw.

The implementation follows [MegaETH’s developer skill](https://github.com/megaeth-labs/skills), [current VRF documentation](https://docs.megaeth.com/developer-docs/vrf), and [MegaETH gas guidance](https://github.com/megaeth-labs/skills/blob/main/skills/megaeth-developer-skills/references/gas-model.md).

### Network and trust limits

Peers are discovered automatically through libp2p mDNS on the local network and the public BitTorrent Mainline DHT across the internet. A shared, versioned ClipClub discovery key advertises the peer mesh's TCP listening port; public DHT bootstrap nodes supply the initial routing contacts. Discovery runs while the app is open and requires no saved peer addresses. DHT results are untrusted connection candidates; Noise and signed GossipSub messages authenticate connected peers before the app accepts their metadata or ratings.

BitTorrent transfers use `librqbit`; catalog metadata and signed rating receipts use the authenticated libp2p mesh. The app attempts UPnP port forwarding for the peer mesh and BitTorrent TCP listeners on supported routers. Internet discovery requires outbound DHT UDP access and a reachable peer mesh connection; video transfer also requires a reachable seeder's BitTorrent port. Restrictive NAT, carrier-grade NAT, firewalls, or disabled UPnP can still prevent inbound connections and may require router configuration. There is no hosted relay or guaranteed NAT traversal.

MegaETH provides the token ledger and verified monetary outcomes. Peer discovery, content availability, and historical clip ratings remain local observations. Signed receipts identify a peer but do not prove actual seeding or a unique human, and disconnected peers may show different clip ratings. Sybil resistance for content rankings and moderation is not implemented. Legacy points receipts never become on-chain assets.

Duplicate detection is exact-byte detection. Re-encoding, editing, or changing embedded metadata creates different content and a new torrent. Perceptual duplicate detection is not implemented.

The embedded UI is served only on `127.0.0.1`, with a fresh per-process access token. The local HTTP service is an internal part of the desktop process. It is not a public application backend. Video uploads and the player communicate with that local service; video bytes move between users through BitTorrent.

## Automatic peer discovery

Open the app and it searches for peers automatically on the local network and across the internet. To verify local discovery, open two instances with separate data directories and ports:

```sh
./dist/clipclub --data-dir /tmp/clipclub-alice --p2p-port 42931 --torrent-port 42932
./dist/clipclub --data-dir /tmp/clipclub-bob --p2p-port 42933 --torrent-port 42934
```

The commands run in separate terminals. No address entry or settings change is needed. Allow local multicast for mDNS discovery and outbound UDP for internet DHT discovery. Internet peers also need reachable TCP ports as described above.

The default peer mesh port is TCP **42931**; the default torrent listening port is TCP **42932**. The HTTP interface binds an ephemeral loopback port by default. CLI options are `--data-dir PATH`, `--p2p-port PORT`, `--torrent-port PORT`, `--http-port PORT`, and `--headless`. A data directory is locked to prevent concurrent processes from modifying the same profile.

## Build

Install a stable Rust toolchain and the system dependencies required by Wry/Tao. macOS needs Xcode Command Line Tools; Windows needs the WebView2 runtime; Linux needs WebKitGTK 4.1/GTK development packages and the corresponding runtime libraries. The desktop shell is portable source, but builds are specific to the target OS and CPU, and those OS WebView runtimes are required.

For a development build, point to a trusted `ffprobe` binary for your platform:

```sh
CLIPCROP_FFPROBE="$PWD/vendor/ffprobe" cargo run
```

For a portable release with `ffprobe` embedded in the executable:

```sh
CLIPCROP_FFPROBE="$PWD/vendor/ffprobe" ./scripts/package.sh
```

`package.sh` requires Python 3 to collect dependency notices and the Xcode Swift tools on macOS to resize the shared fireworks image into the macOS app icon. It uses an installed Cargo, or the workspace-local `.build/cargo/bin/cargo` toolchain if Cargo is not on `PATH`. You can select Cargo explicitly with `CLIPCROP_CARGO`. `CLIPCROP_FFPROBE` must be an absolute path to an executable for the same target as the app; the script resolves the path before building. For a cross-build, set `CLIPCROP_TARGET` and provide the matching probe. A bundled build extracts its verified embedded probe into the profile directory on first launch. No user-installed FFmpeg is required for that release.

Packaging creates the direct executable, a macOS app bundle on macOS, and license notices in `dist`. These are local development packages; the app is not notarized. The default vendored probe is a GPLv3-or-later third-party build. Its exact version, checksum, source links, and redistribution caveats are in [THIRD_PARTY.md](THIRD_PARTY.md). Public release CI instead builds a minimal validator from pinned upstream FFmpeg/dav1d source and retains the source, build recipe, configuration, and notices in each release package; see [release instructions](docs/RELEASES.md).

For backend-only operation, which needs no desktop WebView dependencies:

```sh
CLIPCROP_FFPROBE="$PWD/vendor/ffprobe" cargo run --no-default-features -- --headless --data-dir /tmp/clipclub-test
```

Run the checks with:

```sh
./scripts/check.sh
```

This runs formatting when `rustfmt` is installed, all backend targets/tests, a desktop compile check, and JavaScript syntax checks when Node is available. Tests cover rating and roll math, signatures and persistence, roll playback behavior, menu navigation, file limits and malformed media, deterministic torrents and unsafe metadata, and peer identity/message validation. Automatic DHT discovery and reconnection use a local Mainline DHT fixture without public infrastructure. The mDNS network test requires a multicast-capable LAN and runs separately with `cargo test --no-default-features mdns_discovers_real_peers -- --ignored`. A separate real multi-process smoke test is needed for network behavior across your actual devices, routers, and OS codecs.

## Source layout

| Path | Responsibility |
| --- | --- |
| `src/main.rs` / `src/app.rs` | Desktop lifecycle, local API, prefetch and rating sessions |
| `src/core.rs` | Models, durable state, signed rating receipts and legacy receipt compatibility |
| `src/crypto.rs` | MegaETH RPC, native transaction approvals, receipt validation and roll recovery |
| `contracts/` | 좋아 ERC-20, drand rolls, rating payments, deployment tooling and EVM tests |
| `src/wallet.rs` | Encrypted private-key storage, lock/unlock, backups and transaction signing |
| `src/media.rs` | Bounded media validation and portable validator installation |
| `src/torrent.rs` | Deterministic torrent creation and BitTorrent sessions |
| `src/peer.rs` | Identity, discovery, authenticated gossip and presence |
| `src/discovery.rs` | Bounded discovery candidates, address validation and connection retries |
| `ui/` | Embedded minimalist interface and native WebView video element |

clipclub's original source is MIT licensed. Third-party components retain their own licenses.

## Validation

Run `bash scripts/check.sh` for Rust tests, desktop compilation, UI syntax, and Node behavior tests. With contract dependencies installed (`npm ci --prefix contracts`), it also runs the Solidity compilation and local EVM tests. `npm --prefix contracts run check:verifier` optionally checks the deployed MegaETH verifier through the public RPC without signing anything.

`python3 scripts/integration.py` exercises real local peer discovery, media validation, transfers, HTTP protection, free skips, restarts, profile relocation, and rejection of the retired local-wager API. `python3 scripts/package-smoke.py dist/clipclub` checks a packaged executable’s embedded validator and profile persistence. These use no funded wallets. Contract tests cover every monetary outcome, invalid proofs, timing, replay, deployment guards, and supply invariants. Rust mock-RPC tests cover receipt/chain validation, canonical blocks, ABI encoding, and pending-roll recovery; wallet tests cover exact precision, session checkout, and confirmation retries. `node scripts/session-integration.mjs dist/clipclub` exercises the complete queued-rating checkout against a disposable local EVM and three real app processes; it never contacts MegaETH or spends real funds. A funded testnet rehearsal is still required before operating a live token.

The local-chain checkout uses the built-in wallet. It verifies encrypted storage and backups, password unlock after restart, approval invalidation, local signing, and recovery while locked without consuming a second transaction nonce.
