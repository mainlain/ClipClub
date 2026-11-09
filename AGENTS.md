# Project policy

- Keep app versions in the `0.x.y` series until the user explicitly authorizes moving to `1.0.0`. Do not infer readiness for version 1 from feature completeness or release activity.
- Before `1.0.0`, updates do not require migration or backward compatibility for existing balances, wallets, profiles, or other application state. Do not add migration work solely to preserve state across pre-1.0 updates.
- Revisit migration and compatibility requirements when the user authorizes version 1.
