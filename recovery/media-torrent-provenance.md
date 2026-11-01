# Media and torrent recovery provenance

The current project was read only. Only the temporary recovery tree and this report were written.

Both recovered files are byte-identical to the current project copies, whose modification times are October 28, before the cryptocurrency overhaul. These are shared subsystems that did not acquire cryptocurrency or update-protocol dependencies.

## Historical verification

- Original root build chat: `01a18554-54c3-74d0-8e81-19b0c68d1618`, creation turn `01a18554-94bd-7d61-9647-3f9f1709219a`.
- Original media agent chat: `01a18556-1137-7b40-b42e-416e43885679` (full retrieved history saved at `history/media-subagent.json`).
- Retrieved full initial file-add diffs for both source files (not truncated), and applied every recorded media/torrent update from the media agent in chronological order.
- Applied the root creation-history changes that bind media defaults to core constants and derive ffprobe limits from policy. Ran rustfmt against the recovered files in the temporary tree.
- Compared reconstruction against current files. Only four small cleanup differences remained: final `max_height` policy constant, removal of `return` in bundled probe branch, `#[cfg(test)]` on torrent `pause`, and removal of unused `resume` method. The last two and needless-return cleanup match the final original v0.1.0 Clippy-with-denied-warnings check. Current files retain original October 28 timestamps (media 19:04:37, torrent 19:01:49).
- Used these preserved current copies as final recovery files, including that original cleanup. No new implementation, consensus code, or cryptocurrency behavior was introduced.

## Result

`src/media.rs`: strict MP4/WebM validation with original file/duration/resolution policy, bundled ffprobe extraction, bounded process time/output.

`src/torrent.rs`: original deterministic single-file torrent protocol, canonical metainfo validation, upload/download sessions, existing unit/integration tests.

Root will run project-level compilation and integration using a separate build tree.
