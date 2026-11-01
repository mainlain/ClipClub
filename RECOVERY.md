# Latest v0.1.0 points recovery

This project restores the latest identifiable pre-cryptocurrency v0.1.0: clip ratings and library sorting, working points Roll, and the compact four-row menu.

The recovered features use complete archived source patches. The final UI and available pre-conversion ledger/API source were independently compared. The original complete checkout and lockfile were unavailable, so this is not a byte-identical reproduction of every original source file or executable. The reconstructed lockfile matches the original release's 633 third-party package names and versions.

## Verification

- All 38 Rust tests and 13 library/Roll UI tests passed.
- Three-peer integration, navigation, packaged payout/restart checks, and macOS signature verification passed.
- Current test records and source hashes are in `recovery/`; screenshots are in `recovery/latest-v0.1.0/`.
- The latest source archive is `snapshots/clipcrop-points-latest-v0.1.0-source.tar.gz`, with its checksum alongside it.

## Separate application data

The macOS app is `dist/ClipCrop Points.app`, using `~/Library/Application Support/ClipCrop Points`. Linux uses `$XDG_DATA_HOME/clipcrop-points` or `~/.local/share/clipcrop-points`. Default peer and BitTorrent ports are 42931 and 42932. Existing cryptocurrency profiles are incompatible and are never imported automatically.

## Rebuild

The local toolchain and dependency cache support offline rebuilding on this Mac:

```sh
CARGO_NET_OFFLINE=true ./scripts/check.sh
CARGO_NET_OFFLINE=true ./scripts/package.sh
python3 scripts/integration.py target/debug/clipcrop .build/ffmpeg
```

Linux source and packaging are present; a new Linux fork binary has not been built.
