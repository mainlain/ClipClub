# Rating distribution design intent

Updated on 2026-11-03. The selected seeder distribution curve and per-seeder reward are implemented as tested pure helpers in `src/rating_distribution.rs`. The curve output is a percentage of the rating paid to each permanent seeder. Deployment remains on hold pending integration and a seeder funding/recipient rule.

## Selected curve

Let **x** be the number of distinct permanent seeders for a clip, and let **N** be the population estimate defined below. For `N > 20` and `1 <= x <= N`:

\[
s(x)=\frac{1}{1+e^{\frac{100}{N}(x-0.05N)}}
\]

\[
y(x)=1+99\frac{s(x)-s(N)}{s(1)-s(N)}
\]

The continuous curve decreases from exactly `y(1) = 100` to exactly `y(N) = 1`, with its inflection at `x = 0.05N`. For example, when `N = 100`, `y(5)` is approximately `51.406624` and `y(10)` is approximately `1.674728`.

For a rating r, **each seeder** receives `r × y(x) / 100` points. This amount is not divided by x. Rewards use six decimal places, rounding down to whole micropoints:

```text
reward_micropoints = floor(r × y(x) / 100 × 1,000,000)
```

A lone seeder receives exactly the full rating; at `x = N`, each seeder receives exactly 1% of the rating. A zero rating is a free skip and pays zero. For example, a rating of 5 with `N = 100` pays each of five seeders `2.570331` points, and each of 100 seeders `0.05` points.

Floating-point rounding and conversion to whole micropoints can produce the minimum payout before `x = N`; the unique minimum at `x = N` is a property of the exact continuous formula.

## Meaning of N

**N represents an estimate of the total active user base.** The intended approximation is monthly active peer identities observed through the peer network, using a rolling 30-day window:

```text
N = max(21, 1 + distinct other peer IDs observed active during the last 30 days)
```

- **21 is the minimum integer value of N**, because the selected curve requires `N > 20`. This supersedes the earlier minimum of 20. N grows with the estimated user base; it is neither fixed at 21 nor capped at 21.
- Include the local user's peer identity once (the `1` above). Count each other peer identity once, regardless of the number of announcements or connections.
- Receiving a fresh presence announcement qualifies as activity. Rating, uploading, or seeding a clip is not required to count toward N.
- Different apps may have different estimates because they observe different peers. This is acceptable; a globally agreed census is not required.
- One person may operate multiple peer identities. Counting those identities separately is acceptable; unique-human verification is not required for this estimate.

N is a population input. The number of peers permanently seeding a particular clip is the separate input x. The selected curve reaches 1 when x equals N.

## Current implementation and deferred work

The rolling 30-day estimate and its minimum of 21 are **not implemented**. Currently, `src/app.rs` supplies `rt.peers.len() + 1`, and `src/core.rs` applies the legacy minimum of 3. That peer list is held in memory, removes directly disconnected peers immediately, and prunes entries last seen at least 65 seconds ago on a 20-second timer. It therefore measures recent observed presence rather than monthly activity.

A future implementation of the intended estimate needs persistent last-observed activity timestamps per peer identity. Disconnection must not erase monthly activity history, app restarts must preserve it, and an identity must expire from the estimate after 30 days without another observation. Current connectivity and clip availability must remain separate from that history.

The helpers do not recalculate archived signed receipts: their square-root points curve must remain available to preserve historical balances and earnings. Active on-chain ratings still transfer the rating amount to the registered clip wallet. Connecting the selected reward calculation to new seeder payments requires funding and recipient integration, plus implementation of the population estimate. Peer retention behavior and the deployment hold remain unchanged.
