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

## Validation

Validation results and deliberate fork changes are recorded below after the recovery checks finish.
