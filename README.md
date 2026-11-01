# ClipCrop Points

Recovered latest pre-cryptocurrency v0.1.0 points fork. See [RECOVERY.md](RECOVERY.md) for source provenance, verification and the preserved Git baseline. This fork uses separate profile and application identities so it can coexist with the cryptocurrency project.

A minimalist Rust desktop app for sharing short videos through BitTorrent and spending points to rate them. There is no application account server or hosted video service. The interface and its assets are embedded in the executable.

This is a working experimental implementation. Points are a signed peer-to-peer ledger, not a cryptocurrency or a consensus-backed currency. See the network and trust limits below before using it with untrusted peers.

## Run

On macOS, open `dist/ClipCrop Points.app`, or run the executable:

```sh
./dist/clipcrop
```

On macOS, your profile is stored in `~/Library/Application Support/ClipCrop Points`. On Linux, every launch method uses `$XDG_DATA_HOME/clipcrop-points`, defaulting to `~/.local/share/clipcrop-points`; empty or relative `XDG_DATA_HOME` values use the default. Neither platform searches Downloads for a profile, and the app can be moved without moving this data. On other platforms, the executable defaults to an adjacent `clipcrop-points-data` directory; keep that directory when moving the app. Use `--data-dir PATH` to explicitly choose a different profile folder on any platform.

The profile contains your private identity, points history, settings, watched history, uploaded files, and cached videos. Back up the entire profile directory; creating a different identity starts a new 100-point profile. This fork starts a separate profile and imports no existing application data. To recover personal data from the original points release, use `--data-dir` with a separate copy of a verified version-1 points profile. Cryptocurrency profiles use a different state format and cannot be opened by this version. Do not share `identity.key` with other people.

The app closes its network sessions when you quit. It resumes your permanent library after reopening and revalidating the stored videos. It does not install a background service.

## Linux

Linux x86-64 builds target Ubuntu 22.04 or a compatible newer distribution with GTK 3 / WebKitGTK 4.1. The video validator is embedded; the OS supplies the desktop WebView and GStreamer playback codecs.

See [Linux run and build instructions](docs/LINUX.md) for runtime packages, portable data storage, and the optional application-menu launcher. The Linux release artifacts and build/test details are recorded alongside the packaged files.

## The four screens

The app opens in a compact, resizable window with four menu rows: Upload, Rate, Roll, and Settings. Points stay in the top-right corner, with peer status and your display name in the bottom corners. Use **← Menu** or Escape to return to the main menu. Rating sessions start when you enter Rate and end when you leave it.

- **Seed / upload:** select or drop MP4 or WebM videos. Files pass validation before they enter your library. New clips become torrents and begin seeding automatically. Uploading the exact same bytes joins the same deterministic torrent, even if the original filename differs. You can keep up to 100 permanent clips. Removing a clip stops its permanent seeding. Each clip shows your points earned, its average rating out of 9, and its rating count. Sort by recency (newest additions first), points (most earned first), or rating (highest average first, unrated last).
- **Rate:** videos loop until you press a number from 0–9 or use a rating button. The number is the points you spend. Zero records the clip as watched without paying anyone. Positive ratings temporarily seed the watched clip for the rest of the rating session; leaving Rate ends those temporary seeds. You cannot rate your own permanently seeded clips or rate the same content twice.
- **Roll:** wager points for a random integer multiplier from 0 through 9. A 0 loses the wager and plays no clip. Positive results pay `wager × multiplier` in total and play one random clip from that rating tier, without looping. Previously watched clips and clips in your own library are eligible.
- **Settings:** edit your peer display name, playback mute/volume, automatic prefetch, and saved bootstrap peer addresses. Network details show the local peer identity and connection addresses.

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

Validation checks real bytes and decoded frames with `ffprobe`, including container, streams, dimensions, frame timestamps, and duration. Only one video stream and at most one audio stream are accepted; attachments, subtitles, data streams, and unknown codecs are rejected. Network downloads receive the same checks as uploads. SHA-256 verifies content identity, and canonical single-file torrent metadata is checked before downloading. Paths and filenames from peers are never used as arbitrary filesystem destinations.

The validator has a timeout, bounded output and allocation settings, a demuxer allowlist, and a file-only protocol allowlist. These measures are defense in depth, not an operating-system sandbox or a guarantee against decoder vulnerabilities. The player uses the OS WebView, so codec availability still depends on the platform. A valid MP4/WebM file may use a codec unavailable in that WebView. There is no transcoding or separately bundled playback engine.

## Points

Every identity starts with **100 points**. A rating costs the rater its score, from 0 through 9.

Roll accepts positive wagers up to your balance, in millionths of a point. The wager is deducted once and the total payout is `wager × k`: rolling 1 returns the wager, while rolling 9 returns nine times the wager. Each multiplier has probability `P(k) = 2^(-k) × (512 / 1023)` for `k = 0…9`. The exact outcome weights are 512, 256, 128, 64, 32, 16, 8, 4, 2, and 1 out of 1023.

For a positive roll, the clip tier is its average positive rating rounded to the nearest integer. Unrated clips are excluded. Selection includes previously watched clips and your own clips; playing a roll does not submit a rating or change watched history. If no clip in that tier is available, the result and payout still stand and the interface explains why no clip can play. Rolls share the signed receipt history with ratings, so point changes persist across restarts and can be validated by peers.

Library ratings are the arithmetic mean of the positive 1–9 scores in this peer's validated receipts, regardless of who earned the payout. A 0 remains a free skip and is excluded from the average and rating count; clips with no positive ratings show “Unrated.” Per-clip points count only rewards earned by your identity for that content. Both values include existing receipts and survive restarts. Ratings reflect the receipts this peer has received, so disconnected peers can temporarily show different values.

Recency uses the date a clip was added to your permanent library, preserved across restarts, duplicate uploads, and profile moves. Older profiles without addition dates remain compatible; those clips sort behind dated additions, with a stable content-ID order for ties. Points and rating ties use recency, then content ID.

A lone seeder receives 100% of the rating. Otherwise, each eligible seeder receives:

```text
reward = score × sqrt(1 − (n / max(N, 3))²)
```

Here `n` is the observed seeder count for that clip and `N` is the observed user count, including the rater. This uses `p = 2` and `q = 0.5`. Rewards are computed separately for each seeder, so the total payout can exceed the rater's spend. Fractions persist in millionths of a point, rounded down per reward. For example, a score of 5 with two seeders and three users gives each seeder 3.726779 points, while deducting 5 from the rater.

Each receipt is signed by its rater and includes its score, recipient snapshot, observed population, and a link to the rater's previous receipt. Peers reject modified signatures, replayed receipts, repeated ratings, broken sender history, self-ratings, and spends exceeding the balance in their available ledger. State corruption produces an error instead of silently granting another 100 points.

### Network and trust limits

Discovery uses libp2p mDNS on a local network and direct TCP bootstrap addresses for other networks. BitTorrent transfers use `librqbit`; catalog metadata and signed rating receipts use the authenticated libp2p mesh. Peers behind restrictive NAT may need reachable bootstrap peers and port forwarding. There is no hosted relay or promised automatic NAT traversal.

There is no global census or consensus algorithm. Seeder presence and the population `N` are observations, and peers can disagree while disconnected. Signed receipts identify their author but do not prove actual seeding or honest population reports. Roll draws are generated locally and recorded in signed receipts; peers can verify the recorded multiplier and payout, but cannot prove the draw was random or prevent a modified client from choosing outcomes. New identities each receive their own initial grant, so Sybil identities, collusion, partitioned double-spends, and dishonest reward snapshots remain open problems. Do not treat points as having monetary value. Wider deployment needs an explicit identity and consensus policy.

Duplicate detection is exact-byte detection. Re-encoding, editing, or changing embedded metadata creates different content and a new torrent. Perceptual duplicate detection is not implemented.

The embedded UI is served only on `127.0.0.1`, with a fresh per-process access token. The local HTTP service is an internal part of the desktop process. It is not a public application backend. Video uploads and the player communicate with that local service; video bytes move between users through BitTorrent.

## Connect two peers

On the same local network, open two instances with separate data directories and ports. mDNS discovery should connect them automatically:

```sh
./dist/clipcrop --data-dir /tmp/clipcrop-alice --p2p-port 42931 --torrent-port 42932
./dist/clipcrop --data-dir /tmp/clipcrop-bob --p2p-port 42933 --torrent-port 42934
```

The commands run in separate terminals. For peers on different networks, save the other peer's full reachable multiaddress in Settings, for example:

```text
/ip4/192.168.1.20/tcp/42931/p2p/<other-peer-id>
```

Use a reachable public address when crossing networks. The default peer mesh port is TCP **42931**; the default torrent listening port is TCP **42932**. The HTTP interface binds an ephemeral loopback port by default. CLI options are `--data-dir PATH`, `--p2p-port PORT`, `--torrent-port PORT`, `--http-port PORT`, and `--headless`. A data directory is locked to prevent two processes from spending through the same local profile at once.

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

`package.sh` requires Python 3 to collect dependency notices and the Xcode Swift tools on macOS to render the original vector app icon. It uses an installed Cargo, or the workspace-local `.build/cargo/bin/cargo` toolchain if Cargo is not on `PATH`. You can select Cargo explicitly with `CLIPCROP_CARGO`. `CLIPCROP_FFPROBE` must be an absolute path to an executable for the same target as the app; the script resolves the path before building. For a cross-build, set `CLIPCROP_TARGET` and provide the matching probe. A bundled build extracts its verified embedded probe into the profile directory on first launch. No user-installed FFmpeg is required for that release.

Packaging creates the direct executable, a macOS app bundle on macOS, and license notices in `dist`. These are local development packages; the app is not notarized. The default vendored probe is a GPLv3-or-later third-party build. Its exact version, checksum, source links, and redistribution caveats are in [THIRD_PARTY.md](THIRD_PARTY.md). The packaging script collects notices; it does not establish that all corresponding-source obligations have been fulfilled for a public binary release.

For backend-only operation, which needs no desktop WebView dependencies:

```sh
CLIPCROP_FFPROBE="$PWD/vendor/ffprobe" cargo run --no-default-features -- --headless --data-dir /tmp/clipcrop-test
```

Run the checks with:

```sh
./scripts/check.sh
```

This runs formatting when `rustfmt` is installed, all backend targets/tests, a desktop compile check, and JavaScript syntax checks when Node is available. Tests cover rating and roll math, signatures and persistence, roll playback behavior, file limits and malformed media, deterministic torrents and unsafe metadata, and peer identity/message validation. A separate real multi-process smoke test is needed for network behavior across your actual devices, routers, and OS codecs.

## Source layout

| Path | Responsibility |
| --- | --- |
| `src/main.rs` / `src/app.rs` | Desktop lifecycle, local API, prefetch and rating sessions |
| `src/core.rs` | Models, durable state, signed receipt ledger and payout math |
| `src/media.rs` | Bounded media validation and portable validator installation |
| `src/torrent.rs` | Deterministic torrent creation and BitTorrent sessions |
| `src/peer.rs` | Identity, discovery, authenticated gossip and presence |
| `ui/` | Embedded minimalist interface and native WebView video element |

ClipCrop's original source is MIT licensed. Third-party components retain their own licenses.

## Validation

The Rust tests cover profile-location and startup-access regressions, including XDG data paths and explicit overrides; one additional socket-transfer test is ignored by default. `python3 scripts/integration.py` also covers a real three-app, 13-video integration run: a ten-video ready queue, both payout cases, rejection of oversized/fake/over-resolution/over-duration uploads, and automatic seeding resume after restart.
