# Points core/app fidelity audit

Read-only audit of recovered source, original saved backup, original creation/core-agent/Linux packaging history, and cryptocurrency-conversion history. Current project source was not changed.

## Core

The original core add diff is truncated at20,000characters, but this occurs inside tests, after all production code. Extracted the complete production prefix and applied every historical production update (exec-a7ae3b35-22f2-483f-8039-19ba5065350f, exec-4ad00a47-c148-4920-9f18-49961c805d84, exec-e7151de2-458d-455d-80ad-4a7d0f2cad04), using exact before/context text. Normalized with the original Rustfmt toolchain. The result matches the saved backup production code exactly, with one qualification: the backup gates the obsolete add_library helper with #[cfg(test)]. That helper is unused by the app; actual uploads and startup use add_verified_library. No production ledger behavior differs.

All ten core test names match the original release-era test list. Some original test bodies are beyond the history's initial20,000character truncation; those are preserved from the complete on-disk backup instead of reconstructed from incomplete logs.

Verified production policies: version1state;100point initial grant; signed causal RatingReceipt chains; spendscore0..9; lone seeder receives full spend; multiple seeders each receive curve-derived reward; read-only persisted balances recomputed from receipts; duplicate/self/overspend checks; atomic rollback; catalog/file limits.

Evidence script and derived source: history/audit-core-recovery.py and history/core-derived-production.rs. These are audit outputs only and do not replace recovered source.

## App

The saved app.rs source is complete. Original creation's large shell source command is truncated in the download method, so a byte-for-byte comparison of the entire app file to that one history item is unavailable. Available original follow-up reads and source mutation records agree with the backup. In particular the final Linux portability fix exec-c55e2178-35e9-4fe9-b6e6-de56ba06804e is present: source files are revalidated, local dimensions/duration updated from bytes, and managed paths rebuilt after profile relocation.

Receipt gossip/replay, points balance/earnings state output, rating-session temporary seeds, upload/download canonical metadata repair, and shutdown APIs are points-era code. No active coin, blockchain, mining, or paid-Roll code remains in core.rs/app.rs. Roll is the original coming-soon UI placeholder, as confirmed separately by original release embedded UI equality.

The archived Linux package's embedded three UI files and README were confirmed byte-identical to this backup by root. That provides strong provenance for the backup's release-era context, though source identity of app.rs cannot be proven from binary strings alone.

## Scope of confidence

Recovered behavior is the original v0.1.0 points application with directly preserved source and historical source recovery. It is not a claim of byte-identical release executable reproduction: compiler/build environment and platform differ. No unsupported cryptocurrency rollback or current-profile conversion is part of this recovery.

Recovered src/core.rs SHA-256: 149c7ffb00b7c5e4bf55186e05fbc800695d350cc97e842bd95903eeb049d821

Recovered src/app.rs SHA-256: d138337e6aad013fac798091d4fa4b91ce0e95bea7b15c54b475e9ca0fa3ae26
