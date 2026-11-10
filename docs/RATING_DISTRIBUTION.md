# Rating distribution

The curve in [`src/rating_distribution.rs`](../src/rating_distribution.rs) determines rewards for permanent seeders. The rater pays exactly the rating score in existing 좋아 (✨) tokens. Unused rating contributions carry forward as wallet-scoped credit. Settlement mints any reward shortfall remaining after that credit is used. The token is deployed on MegaETH mainnet at [`0x98d49dC44f1CbbA5ABef438985b20A729dE4ab16`](https://mega.etherscan.io/address/0x98d49dC44f1CbbA5ABef438985b20A729dE4ab16); see [the verified deployment record](../contracts/mainnet-deployment.json).

## Selected curve

Let **x** be the number of distinct permanent seeders for a clip, and let **N** be the population estimate defined below. For `N > 20` and `1 <= x <= N`:

\[
s(x)=\frac{1}{1+e^{\frac{100}{N}(x-0.05N)}}
\]

\[
y(x)=1+99\frac{s(x)-s(N)}{s(1)-s(N)}
\]

The continuous curve decreases from exactly `y(1) = 100` to exactly `y(N) = 1`, with its inflection at `x = 0.05N`. For example, when `N = 100`, `y(5)` is approximately `51.406624` and `y(10)` is approximately `1.674728`.

For a rating **r**, **each seeder** receives `r × y(x) / 100` ✨. This amount is not divided by x. Rewards use six decimal places, rounding down to whole token base units (1 ✨ = 1,000,000 base units):

```text
reward_base_units = floor(r × y(x) / 100 × 1,000,000)
```

A lone seeder receives exactly the full rating; at `x = N`, each seeder receives exactly 1% of the rating. A zero rating is a free skip and pays zero. For example, a rating of 5 with `N = 100` pays each of five seeders `2.570331` ✨, and each of 100 seeders `0.05` ✨.

Floating-point rounding and conversion to whole base units can produce the minimum payout before `x = N`; the unique minimum at `x = N` is a property of the exact continuous formula.

## Local calculation and on-chain settlement

The desktop app calculates the curve and each seeder's six-decimal reward from locally observed seeder and population inputs. It freezes the population, reward and recipients when each rating is queued. Session checkout sends those frozen results in one on-chain `settleSeederRatings` transaction.

The contract enforces wallet authorization, registered seeder identities, distinct sorted recipient wallets, reward bounds, balances and replay protection. It does not evaluate the logistic curve or verify actual storage. Up to 100 ratings and 1,000 individual seeder payments can settle atomically in one session.

## Automatic minting and surplus credit

For a session, let **C** be the sum of rating scores in base units, **P** the sum of all seeder payouts, and **B** the rater's unused credit from previous rating contributions:

```text
minted = max(0, P - C - B)
new_credit = max(0, B + C - P)
```

Settlement debits exactly C from the rater, adds any required newly minted tokens, and pays P to the registered seeders atomically. No treasury grant, sponsor deposit or prefunding is required. The rater still needs the full rating cost in their wallet; unused credit funds seeder rewards rather than replacing that contribution.

A rating of 5 with five seeders and `N = 100` costs the rater 5 ✨ and pays 12.851655 ✨ in total. With no previous credit, settlement mints 7.851655 ✨. With 1 ✨ of credit, settlement consumes that credit and mints 6.851655 ✨. If payouts are below the contribution, the unspent existing tokens remain in the contract as that rater's credit for future rewards. The contract does not spend another wallet's credit or roll escrow.

Minting happens once per session after all distributions are validated. An invalid item, insufficient rater balance or exhausted uint256 capacity reverts contributions, minting, payouts and replay markers together. Capacity checks reserve headroom for the worst-case payouts of every already committed roll. `totalRatingMinted` tracks cumulative issuance, and `SeederFunding` records each session's total payout, minted amount and credit before/after. The app verifies those funding records as well as the exact seeder payments before committing receipts.

The rating formula and peer observations remain local inputs. The contract verifies wallet bindings, array and reward bounds, and replay protection; it does not independently prove actual storage or validate the logistic curve. Automatic rating minting is permissionless within those rules. No mint-role gate, attestation requirement, emissions schedule or policy supply cap is added; arithmetic overflow protection remains enforced. Modified clients can submit distributions that satisfy contract bounds without following the app's observations or formula.

## Recipients

A wallet calls `registerSeeder(SHA256(peer_id))`, then that authenticated peer advertises the wallet and its permanent clip IDs. The app checks the on-chain wallet-to-peer binding before accepting the wallet as a recipient. No first-claim clip registration is needed for the new seeder flow.

For the curve, x is the number of eligible, currently observed permanent seeders with registered recipient wallets, excluding the rater and the rater's wallet. Unregistered wallets, temporary session seeds, expired presence, and duplicate wallets do not qualify. Each peer and recipient wallet appears at most once per clip. Recipient lists and rewards are durable snapshots taken when queueing; a disconnect after queueing does not recalculate a signed payment. A changed on-chain wallet binding can cause settlement to revert and requires rebuilding the draft.

Session review displays each clip's recipients and per-seeder reward separately from the rater's cost. Confirmation verifies the exact distribution and payment events against the frozen local draft before committing signed rating receipts. Existing single-recipient contract methods remain available for legacy drafts and do not mint seeder subsidies.

## Meaning of N

**N represents an estimate of the total active user base.** The intended approximation is monthly active peer identities observed through the peer network, using a rolling 30-day window:

```text
N = max(21, 1 + distinct other peer IDs observed active during the last 30 days)
```

- **21 is the minimum integer value of N**, because the selected curve requires `N > 20`. N grows with the estimated user base; it is neither fixed at 21 nor capped at 21.
- Include the local user's peer identity once (the `1` above). Count each other peer identity once, regardless of the number of announcements or connections.
- Receiving a fresh presence announcement qualifies as activity. Rating, uploading, or seeding a clip is not required to count toward N.
- Different apps may have different estimates because they observe different peers. This is acceptable; a globally agreed census is not required.
- One person may operate multiple peer identities. Counting those identities separately is acceptable; unique-human verification is not required for this estimate.

N is a population input. The number of peers permanently seeding a particular clip is the separate input x. The selected curve reaches 1 when x equals N.

## Population persistence and receipt history

[`Store::observe_peer`](../src/core.rs) records a timestamp for each authenticated presence announcement in the profile's persistent `peer_activity` map. The estimate deduplicates peer identities, counts the local peer once, applies the minimum of 21, and excludes observations at least 30 days old. Disconnecting or pruning current connectivity does not erase monthly history. Restarting preserves it; profiles without stored observations start with an empty history.

Archived signed points receipts retain their original square-root calculation and signing bytes. They are not recalculated or converted into tokens. Paid rating receipts continue to bind scores to successful canonical contract events and the rater's peer identity.

## Validation

The end-to-end session test uses three local app processes and a disposable EVM, with persistent monthly history `N = 100`. It checks multiple permanent seeders, exact Rust/Solidity calldata agreement, fixed rater charges, automatic shortfall minting without prefunding, restart recovery, peer receipt propagation and duplicate confirmation without additional payment.
