# Project policy

- Keep app versions in the `0.x.y` series until the user explicitly authorizes moving to `1.0.0`. Do not infer readiness for version 1 from feature completeness or release activity.
- Before `1.0.0`, updates do not require migration or backward compatibility for existing balances, wallets, profiles, or other application state. Do not add migration work solely to preserve state across pre-1.0 updates.
- Revisit migration and compatibility requirements when the user authorizes version 1.

# Remote diagnostics

- When investigating reports from other test computers, use `python3 scripts/diagnostic-inbox.py pull` if the private reader is configured, then inspect `.build/diagnostics/`. See `docs/DIAGNOSTICS_INBOX.md` for setup and session filtering.
- Inbox credentials and local setup pages live under ignored `.build/`. Never print, commit, or include them in diagnostic reports. Test computers receive only the upload key; the read key stays on the development computer.
