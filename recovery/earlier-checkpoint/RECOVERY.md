# v0.1.0 points recovery

Recovered on 2026-10-31 into a separate project. The original cryptocurrency project at `/Users/sean/Downloads/clipcrop` was read only during this recovery.

This is a buildable recovery of the points-era application. It is not a claim of a byte-for-byte reproduction of every original source file or executable. The original complete source checkout and Cargo.lock were not preserved.

## Evidence and provenance

- Reference release: `clipcrop-0.1.0-linux-x86_64.tar.gz`. Its embedded build report identifies v0.1.0, and its executable SHA-256 is `f0c25337d71906e5344b4b31e24b8e841c8cf52ff2895068072b39c0b2eb6309`.
- `ui/index.html`, `ui/app.js`, and `ui/style.css`: restored from `.build/library-ratings-before`; all three complete byte sequences occur unchanged inside the original Linux executable.
- `README.md`: the preserved backup is byte identical to the README distributed with the reference release. The working fork adds its own recovery notice and isolated paths.
- `src/core.rs`, `src/app.rs`, `scripts/check.sh`, `scripts/integration.py`: preserved complete points-era backup. Historical reconstruction independently confirms all production ledger methods, apart from a test-only gate on an unused helper. App startup relocation changes match the recorded final Linux patch. One original app creation record is truncated, so its full original source identity cannot be independently proved from that record alone.
- `src/peer.rs`: restored from complete historical add/update diffs, including the original `clipcrop/catalog/1` topic and signed receipt wire protocol.
- `src/media.rs`, `src/torrent.rs`: verified against original source additions, patches and final build cleanups. The preserved shared modules contain no cryptocurrency dependencies.
- `src/http.rs`: original points routes and functions restored from the historical creation block and fixes. The history redacted the random access-token expression; its unchanged implementation was recovered from the surviving source.
- `src/main.rs`: launcher reconstructed from historical source plus preserved profile path/window functions. The historical GTK container fix and safe profile paths were checked against the original release's documentation and binary markers. Profile regression tests from the preserved launcher were retained.
- `Cargo.toml` and `scripts/package.sh`: original v0.1.0 readback. No build.rs was present in that release.
- `Cargo.lock`: reconstructed by pruning the surviving lockfile against the historical manifest, offline. Its 633 third-party package name/version pairs exactly match the original Linux release's dependency inventory. The original lockfile bytes are unavailable.
- Original Linux packaging, documentation, icon helpers, assets, license files and validator build recipe were recovered from original history or preserved shared files.

The restored application starts each new identity at 100 points, spends 0-9 points per rating, pays the original signed-receipt seeder rewards, supports video uploads/playback/BitTorrent, and leaves Roll as the original coming-soon screen. It contains no active cryptocurrency chain, mining, wallet, or paid Roll implementation.

## Snapshots

The first Git commit and `v0.1.0-recovered-baseline` tag preserve the recovered historical baseline before intentional fork isolation. The working version then changes only fork identity/profile separation and the test fixture's unique temporary directories; these changes are recorded in Git.

Do not launch the historical baseline with its default profile path: it used the same ClipCrop profile name as later releases. The normal recovered fork uses its own profile location. No current or historical user profile/identity was copied into this project.

## Intentional fork differences

- macOS default profile: `~/Library/Application Support/ClipCrop Points`; Linux: `$XDG_DATA_HOME/clipcrop-points` or `~/.local/share/clipcrop-points`; other platforms: adjacent `clipcrop-points-data`. The current cryptocurrency profile is never auto-imported.
- Default peer/torrent ports are 42931/42932, allowing the two apps to run together. Explicit CLI ports and profiles remain available.
- The macOS bundle is `ClipCrop Points.app`, identifier `local.clipcrop.points`. Linux packaging/launchers use the `clipcrop-points` identity. The embedded historical UI stays byte identical to the original release.
- The test-only fixture uses unique temporary directories instead of timestamp names; the old parallel tests collided. Production points rules are unchanged.

## Validation

- All 29 Rust tests pass, including the real peer-discovery and BitTorrent socket tests. Tests also pass in parallel after the fixture-only path fix.
- Desktop compilation, JavaScript syntax, shell syntax and embedded Linux packaging Python syntax pass.
- The full three-peer integration passes: 13 uploads, MP4/WebM validation, malformed/oversized file rejection, ten-clip queue refill, range playback, zero-point passes, 100-point starts, lone/multiple-seeder payouts, duplicate handling, promotion, session cleanup, restart, profile relocation and profile locking.
- Machine-readable provenance is in `recovery/evidence.json`; test results are in `recovery/unit-tests.txt` and `recovery/integration.txt`.
- The macOS release app is rebuilt with its embedded validator and its ad-hoc signature verifies. Packaged smoke tests pass: 100-point starts, no cryptocurrency actions, saved identity/settings, and restart. Disposable test profiles were removed.
- Integrity verification confirms all 2,883 checked files in the original project are byte unchanged. Build/toolchain caches were copied into this separate project; the original caches were not used for build outputs.
- A full source archive and a Git bundle are saved in `snapshots/`; Git includes the recovered historical baseline tag and a separate fork-isolation commit.
- Linux source, documentation and packaging scripts are recovered; a new Linux fork binary has not been built during this recovery.

## Build this recovered project

The local `.build/cargo` and `.build/rustup` toolchain/cache copies allow offline rebuilding on this Mac. The original cryptocurrency project is not a build dependency.

```sh
CARGO_NET_OFFLINE=true ./scripts/check.sh
CARGO_NET_OFFLINE=true ./scripts/package.sh
```

Open `dist/ClipCrop Points.app` to run the isolated points fork. The full integration test can be repeated with `python3 scripts/integration.py target/debug/clipcrop .build/ffmpeg`. `recovery/package-smoke.py` accepts the path to a packaged executable and tests only disposable profiles.

