# Original pre-cryptocurrency Roll recovery

## Historical sources

- Roll root turn `01a1885d-6649-7f52-a382-ecebae6320b0`, staged `history/roll.json`.
- Roll ledger subagent `01a1885e-1646-7a11-ada8-ea5948a701f6`, full history `history/roll-ledger.json`.
- Roll UI subagent `01a1885e-4937-76c1-a510-7769ef9e373f`, full history `history/roll-ui.json`.
- Roll integration/docs subagent `01a1885e-867e-7471-9874-565f8dfd17c0`, full history `history/roll-integration.json`.

All twenty file-change patches/source additions are complete and untruncated. The only non-diff mutation is a fully recorded, parsed Python script that changes README and the check script. Read-only command outputs containing many source files were sometimes truncated; recovery does not depend on those truncated reads. Every source mutation used for recovery is intact.

`actions.json` records the 21 operations, source history, and exact original tool event IDs. Patches retain their original context and additions. `readme-check-initial.py` is the original shell-embedded Python script decoded with shlex and parsed as Python, not reauthored.

## Replay

Run `python3 extracted/roll/replay.py work` after the library-ratings restoration, before the menu update. The script only accepts a target inside the isolated staging directory, uses patch against the exact historical diffs, adds the recorded Roll test source, then formats affected Rust files. It restores the original target file before retrying a failed patch, so partial patches do not silently accumulate.

Validation used a separate source-only copy `roll-validation`; shared `work`, the current cryptocurrency project, and the previously recovered release were not modified. Every historical patch applied without a formatting retry. `replay-log.json` lists successful applications. The exact post-Roll files are copied under `final/`, with `final-sha256.json` for review.

## Recovered behavior and compatibility

- Application version remains 0.1.0 and state schema remains version 1.
- `RatingPayload` adds optional `roll`; None is omitted to preserve old rating JSON/signature bytes. Rolls and ratings use the same signed receipt chain, sender sequence and previous-receipt link. Roll receipts use a reserved zero content ID, score 0, no seeders, and population 3.
- Core samples a uniform integer ticket in 0..1023, mapping exact weights [512,256,128,64,32,16,8,4,2,1] to multipliers 0 through 9. Wagers are integer millionths of points; wager >0, <=balance, and <=u64::MAX/9. HTTP accepts the historical f64 wager, enforcing finite, positive, six-decimal precision. It rejects unknown request fields.
- A Roll immediately deducts the wager and adds wager×multiplier total payout. Zero loses the wager; one returns it. This is the original points economy, including creation/destruction of points, with no reserve pool, cryptocurrency supply, mining, or block delays.
- Rolls persist in the existing receipt ledger and replay over the unchanged `clipcrop/catalog/1` mesh Receipt message. No separate peer.rs or network changes are required. Validation checks signature, sender chain, wager, ticket range, field shape, insufficient balance and overflow; failed persistence restores state. As originally documented, modified clients can select their own signed tickets: peers cannot prove local randomness.
- Roll does not modify watched history or ratings/earnings statistics. Positive draws select randomly among available clips whose average positive rating rounds to the multiplier; watched and owned clips remain eligible. Unrated clips are excluded. Clip availability never changes draw or payout.
- App creates a separate bounded roll-cache and download session, validates downloaded/copied media and canonical torrent/content hashes, removes old files, supports cancellation/superseding and timeout cleanup, and preserves payouts if media fails.
- UI contains the original wager/odds/payout/result flow and a nonlooping Roll player. Ended/paused media is not restarted by polls or navigation. Stale polls cannot overwrite completed wager state, and a ready clip received in polling survives a later initial POST response.

## Checks completed

- Every mutation replayed successfully against a source-only copy of the recovered ratings stage.
- All 13 historical library and Roll UI tests passed, including all original race regressions.
- Independent historical comparison: reconstructed `src/core.rs` lines 1..870 matches exactly the untruncated reads made at the beginning of the subsequent cryptocurrency-overhaul chat (before that chat changed source). `core-boundary-comparison.diff` is empty. This covers all production core persistence/accounting/Roll methods; later lines are the restored historical unit tests.
- Backend compilation, full Rust tests, packaging and real network integration are left to the root recovery workflow.
