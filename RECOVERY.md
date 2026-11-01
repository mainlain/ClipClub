# Latest v0.1.0 points recovery

Updated on 2026-10-31. This project restores the latest identifiable points-era v0.1.0 before the cryptocurrency overhaul. The cryptocurrency project at `/Users/sean/Downloads/clipcrop` remains unchanged.

The earlier recovery restored a pre-feature Linux checkpoint. Its complete source, app backup and Git tags remain available. This working version adds the three subsequent revisions found in archived chats: clip ratings and library sorting, working points Roll, and the compact home menu.

## Restored behavior

- New identities start with 100 points. Ratings cost 0–9 points and retain the original signed-receipt seeder rewards.
- The library displays points earned, average positive rating and rating count, with recency/points/rating sorting. Zero skips do not affect averages.
- Roll accepts a positive points wager. It draws k=0–9 with weights `[512,256,128,64,32,16,8,4,2,1]/1023` and returns `wager × k` after deducting the stake. A zero loses the stake; one returns it. These are the original variable-supply points rules, with no cryptocurrency pool or mining.
- Positive rolls select a random clip by rounded average rating. Watched and owned clips are eligible; clips play once. Payouts stand when no matching clip is available.
- The compact 480 × 620 window opens on four horizontal menu rows. Menu/Escape returns home and ends rating sessions.

## Historical source evidence

All changes in the following three revisions were recovered from complete source patches and writing commands. No feature was redesigned for this recovery:

1. `Add clip rating tracking and sorting` (`01a1884f-91ea-76f1-98d3-8c24b66fc12a`): all ten original source/test/documentation diffs.
2. `Add multiplier-based clip rolls` (`01a1885d-64c6-7f72-be2c-4d5d4c916ee5`): the parent and all three original ledger/UI/integration agent histories, replayed as 21 recorded actions.
3. `Simplify the main menu UI` (`01a18869-5435-77d2-bd8c-fe14c5180739`): full HTML/documentation transform, complete CSS rewrite, navigation patches and original navigation smoke check.

The final three UI files match independent reconstruction from historical source. The final HTML also matches its complete historical readback. The core production source through line 870 is byte-for-line identical to the source read immediately before the cryptocurrency conversion. The full HTTP source is byte identical to that pre-conversion readback; available independent app source slices also match. Detailed audit, manifests and hashes are in `recovery/latest-v0.1.0/`.

The original complete checkout and lockfile were not retained, so this is not a claim of byte-identical reproduction of every original source file or executable. The underlying early-checkpoint provenance and known reconstruction limits are retained in `recovery/earlier-checkpoint/RECOVERY.md` and the other recovery audit files. The dependency lock still contains the exact 633 third-party package name/version pairs recorded in the original Linux release.

## Isolated fork identity

- macOS profile: `~/Library/Application Support/ClipCrop Points`.
- Linux profile: `$XDG_DATA_HOME/clipcrop-points`, defaulting to `~/.local/share/clipcrop-points`.
- Default peer/BitTorrent ports: 42931/42932.
- macOS bundle: `ClipCrop Points.app`, identifier `local.clipcrop.points`; Linux packaging/launchers use `clipcrop-points`.
- Explicit CLI data directories remain supported. No existing user profile or identity was imported. Use a separate copy when opening an old points profile; cryptocurrency profiles are incompatible.
- The test-only fixture retains unique temporary directories to avoid collisions. Production points behavior is historical.

## Checkpoints and backups

Git preserves the earlier recovery (`v0.1.0-recovered-baseline`, `v0.1.0-points-recovered`) and each recovered late revision (`v0.1.0-ratings-recovered`, `v0.1.0-roll-recovered`, `v0.1.0-compact-menu-recovered`). The final verified state is tagged `v0.1.0-latest-points-recovered`.

`snapshots/` contains the earlier source/history archives, an earlier app ZIP, and the new latest-v0.1.0 source archive and Git bundle. Checksums accompany them. Do not launch the historical baseline tag with its original default profile; normal working builds retain the isolated fork paths above.

## Validation

The restored backend passes all 38 Rust tests, including real peer discovery and BitTorrent transfers. All 13 original library/Roll UI tests pass. The original navigation smoke also passes, including rapid navigation/session cleanup. The full three-peer integration also passes: 13 uploads, file validation and rejection, queue refill, rating averages and recency, lone/multiple payouts, duplicate handling, watched/owned Roll playback, validated remote Roll downloads, mixed signed history, balance persistence and profile locking.

The rebuilt macOS release signature verifies. Packaged smoke checks pass for 100-point starts, signed Roll payouts, sorting/menu markup, embedded validator, client-result injection rejection and identity/settings/balance persistence. The browser preview verified the final menu, Roll odds/wager controls and library sorting options; screenshots are retained here. Preview servers and disposable profiles were stopped/removed.

All 2,883 checked files in the original cryptocurrency project remain byte unchanged. Test results are retained in `recovery/`.

## Rebuild

The copied `.build/cargo`, `.build/rustup` and dependency cache allow offline rebuilding on this Mac without the cryptocurrency project:

```sh
CARGO_NET_OFFLINE=true ./scripts/check.sh
CARGO_NET_OFFLINE=true ./scripts/package.sh
python3 scripts/integration.py target/debug/clipcrop .build/ffmpeg
```

Open `dist/ClipCrop Points.app`. Linux source and packaging are recovered, but a new Linux fork binary has not been built during this recovery.
