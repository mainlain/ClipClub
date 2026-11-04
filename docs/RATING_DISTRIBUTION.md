# Rating distribution design intent

Clarified on 2026-11-03. This records the intended meaning of **N** for a future replacement of the rating/seeding distribution formula. The existing legacy points curve is not the intended future formula. The replacement formula and its funding rule remain undecided; deployment remains on hold.

## Meaning of N

**N represents an estimate of the total active user base.** The intended approximation is monthly active peer identities observed through the peer network, using a rolling 30-day window:

```text
N = max(20, 1 + distinct other peer IDs observed active during the last 30 days)
```

- **20 is only the minimum allowed value of N.** N grows with the estimated user base; it is neither fixed at 20 nor capped at 20.
- Include the local user's peer identity once (the `1` above). Count each other peer identity once, regardless of the number of announcements or connections.
- Receiving a fresh presence announcement qualifies as activity. Rating, uploading, or seeding a clip is not required to count toward N.
- Different apps may have different estimates because they observe different peers. This is acceptable; a globally agreed census is not required.
- One person may operate multiple peer identities. Counting those identities separately is acceptable; unique-human verification is not required for this estimate.

N is a population input. The number of peers currently seeding a particular clip is a separate quantity. Defining N does not select a payout curve or require the future formula to reach zero when the seeder count equals N.

## Current implementation and deferred work

The rolling 30-day estimate and intended minimum of 20 are **not implemented**. Currently, `src/app.rs` supplies `rt.peers.len() + 1`, and `src/core.rs` applies the legacy minimum of 3. That peer list is held in memory, removes directly disconnected peers immediately, and prunes entries last seen at least 65 seconds ago on a 20-second timer. It therefore measures recent observed presence rather than monthly activity.

A future implementation of the intended estimate needs persistent last-observed activity timestamps per peer identity. Disconnection must not erase monthly activity history, app restarts must preserve it, and an identity must expire from the estimate after 30 days without another observation. Current connectivity and clip availability must remain separate from that history.

This clarification changes documentation only. The existing legacy points calculation, token payments, peer retention behavior, and deployment hold remain unchanged. Formula replacement and implementation of the population estimate belong to a later task.
