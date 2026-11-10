# 좋아 (✨) on MegaETH

[`src/Joah.sol`](src/Joah.sol) combines an OpenZeppelin ERC20 with fixed game rules. The name is
`좋아`, the symbol is `✨`, and one token is 1,000,000 base units (six decimals). ClipClub uses a
fireworks logo. ETH remains the chain's gas currency.

There is one explicit genesis allocation to the constructor's treasury. Supply
and treasury are deployment choices; no wallet/peer receives a free grant. There
is no owner, upgrade mechanism, administrative mint, pause, or emergency refund.
Existing ClipClub points do not represent tokens and cannot be redeemed.

## Rolls

`requestRoll(wager)` escrows existing tokens in this contract and commits to
quicknet's current round + 2. Quicknet genesis is `1692803367`, period is three
seconds, and round `n` publishes at `genesis + (n - 1) * 3`. One unresolved roll is
allowed per wallet; `pendingRoll(address)` recovers it after an app restart.

After publication, anyone can call `settleRoll(id, signature)`. The immutable
MegaETH drand verifier validates the exact committed round with
`verifyNormalized(uint64,bytes)`. The contract checks the boolean and mixes the
returned chain-scoped hash with its address, chain ID, roll ID, player, amount,
and round. It uses deterministic rejection sampling to avoid modulo bias.
Compressed and uncompressed encodings produce the same normalized randomness.

| Total payout | Tickets out of 1023 | Supply change |
| --- | ---: | --- |
| 0× | 512 | Stake burned |
| 1× | 256 | Stake returned; neutral |
| 2×–9× | 128, 64, 32, 16, 8, 4, 2, 1 | Stake returned and net profit minted |

The expected total payout is `1013 / 1023` times the wager. Individual profitable
wins increase supply, losses reduce it, and the weighted game is slightly
deflationary in expectation.
Worst-case pending profit reserves prevent supply overflow at settlement.

There is no timeout refund or alternate round: either could allow selective
loss cancellation. Reveal is permissionless and has no expiry. If quicknet or
the verifier stops, stakes remain locked until service returns. A production
operator should run a keeper for liveness; the application also supports user
settlement. Randomness cadence is three seconds; a new request normally waits
three to six seconds before proof availability, plus transaction delays.

## Ratings

The app computes the selected seeder curve and six-decimal rewards locally, using a persistent 30-day population estimate (minimum 21) and registered permanent seeders. Each queued rating freezes its population, recipients and per-seeder reward. No formula or population census runs on-chain.

### Seeder registration

`registerSeeder(bytes32)` binds the calling wallet to the SHA-256 commitment of a peer identity. The authenticated peer advertises that wallet and its permanent clips. The app verifies this binding, excludes temporary seeds and the rater, and deduplicates wallets and peers. The contract does not independently prove seeding or Sybil resistance.

### Batch settlement and funding

`settleSeederRatings(bytes32,SeederRating[],bytes32)` settles up to 100 clips and 1,000 seeder payments atomically. A `SeederRating` contains the clip ID, score, local population, reward per seeder, sorted recipient wallets and matching peer commitments. The rater pays exactly the sum of scores in ✨.

For total charge **C**, seeder payout **P** and existing rating credit **B**, all in token base units:

```text
minted = max(0, P - C - B)
ratingCredit[rater] = max(0, B + C - P)
```

Unused contributions fund that wallet's future rewards. Insufficient rater balance or exhausted arithmetic capacity reverts every transfer, mint and replay marker. Credit belonging to another wallet and roll escrow remain untouched.

No prefunding or manual grant transaction is required. `totalRatingMinted` tracks cumulative seeder issuance. `SeederFunding` records the payout, minted amount and the rater's credit before/after. The app verifies this accounting from canonical events. Rating minting and roll commitments share capacity checks so new seeder issuance cannot consume supply headroom reserved for pending roll payouts.

### Trust model and receipts

Automatic rating minting is permissionless within contract bounds. The contract does not validate the locally evaluated curve, prove storage, require reward attestations, or apply a policy emissions schedule/cap. Client-side observations and formula enforcement can be bypassed by modified clients; registration, replay protection and basic reward/batch bounds still apply.

The contract enforces registered peer bindings, distinct sorted wallets, 1–9 scores, basic curve-domain/reward bounds, and per-wallet clip/session replay protection. It emits `SeederDistribution` and `SeederPaid` records, one `ClipRated` event per clip with the rater's charge, and a final `RatingsSettled` summary. The app verifies every recipient, reward, count, population, session, rater and peer commitment against the frozen draft before atomically committing receipts. Rejected approvals keep the unpaid draft; confirmation retries never send another payment.

`registerClip`, `rateClip` and `rateClips` remain as legacy single-recipient APIs for old drafts. They never mint seeder rewards. Archived signed points receipts retain their original calculation and cannot redeem tokens. See [the distribution design](../docs/RATING_DISTRIBUTION.md) for the formula and full trust model.

## Build and test

```sh
cd contracts
npm ci --ignore-scripts
npm run build
npm test
```

Tests execute the bytecode in a local EVM with a test-only oracle. They cover
all ten payout tiers and supply changes, the exact ticket distribution,
rejection sampling, future commitments, proof and replay failures, no-code/
wrong-network deployment rejection, pending recovery, overflow reserves,
ERC20 transfers/allowances, rating transfers, atomic batches, aggregate balance
checks, session replay protection, and the 100-item bound. Local tests do not reproduce
MegaEVM gas accounting or validate the real BLS implementation.

`npm run build` writes ignored artifacts and refreshes committed
`abi-selectors.json`, which Rust includes directly. Compiler is Solidity
0.8.30, optimizer 200 runs, Shanghai EVM target.

An optional read-only integration check validates a live quicknet proof against
the canonical chain verifier. It needs network access and no funded wallet:

```sh
npm run check:verifier
npm run check:verifier -- --mainnet
```

Ganache and its transitive packages are development-only. No npm package ships
inside ClipClub's Rust application or is used to hold wallet keys.

## Deployment record and tools

### Mainnet deployment

**좋아 was deployed on MegaETH mainnet on 2026-11-04 at [`0x98d49dC44f1CbbA5ABef438985b20A729dE4ab16`](https://mega.etherscan.io/address/0x98d49dC44f1CbbA5ABef438985b20A729dE4ab16).** [Transaction `0x776686e4…441211f`](https://mega.etherscan.io/tx/0x776686e41987d79d2eda546eab5c74e9fd38f3e6ad1678b1195194c87441211f) was included in block 31,031,878. Full initcode, canonical inclusion, token metadata, immutable verifier and the genesis mint were independently checked through the public mainnet RPC. See [the public deployment record](mainnet-deployment.json).

[`mainnet-launch.json`](mainnet-launch.json) records the deployed configuration. Deployment commands reject a second deployment of this launch. The genesis supply went to the treasury; seeder reward shortfalls are minted automatically during session settlement. A funded MegaETH wallet is needed for seeder registration, ratings, requests, and reveal transactions.

Source verification has exact creation and runtime matches on [Sourcify](https://repo.sourcify.dev/4326/0x98d49dC44f1CbbA5ABef438985b20A729dE4ab16), an exact source match on [Mega Etherscan](https://mega.etherscan.io/address/0x98d49dC44f1CbbA5ABef438985b20A729dE4ab16#code), and verified status on [MegaETH Blockscout](https://megaeth.blockscout.com/address/0x98d49dC44f1CbbA5ABef438985b20A729dE4ab16?tab=contract). The compiler is Solidity `v0.8.30+commit.73712a01`, with optimizer 200 runs and Shanghai EVM target.

### Network configuration

| Network | Chain ID | RPC | Immutable verifier |
| --- | ---: | --- | --- |
| Mainnet | 4326 | `https://mainnet.megaeth.com/rpc` | `0x7a53a6eFA81c426838fcf4824E6e207923969b36` |
| Testnet | 6343 | `https://carrot.megaeth.com/rpc` | `0x4e1673dcAA38136b5032F27ef93423162aF977Cc` |

The constructor rejects other chain IDs, with isolated chain 31337 as the sole
exception for mock-oracle tests. On MegaETH it requires the canonical verifier
address and nonempty code.

The selected launch in [`mainnet-launch.json`](mainnet-launch.json) is **MegaETH mainnet
(4326)** with **1,000,000,000 ✨** (`1000000000000000` base units) sent to
**`0xcBBF8e516F9a3ad56Efe28A0aEBf69Edc4639637`**. The recipient is the
explicit wallet address selected for this launch; no ENS resolution is used.

### Deployment tools

The completed mainnet launch used these commands to prepare unsigned initcode with a remote gas estimate and 20% margin, then open the local wallet review page. They now reject repeating the deployed launch:

```sh
npm run deployment
npm run deploy:wallet
```

The review page displays the selected chain, treasury and supply and asks the
connected browser wallet to sign the deployment. It never requests a private
key. `deploy:review` is an alias for the same local review page.

For a separate testnet deployment, provide both recipient and supply explicitly:

```sh
JOAH_TREASURY=0xYourTreasury JOAH_GENESIS_SUPPLY=1000000 \
  MEGAETH_CHAIN_ID=6343 npm run deployment
```

The testnet values above are examples. Environment overrides remain available;
testnet never silently inherits the selected mainnet allocation. Preparation
creates ignored `deployment.json` and never loads a key or broadcasts.
The artifact includes token metadata and the Keccak-256 hash of the complete
deployment calldata, including the constructor allocation.
Submit its `unsignedTransaction` through a wallet after reviewing the chain,
treasury, supply, and bytecode. A Foundry keystore deployment is also possible
after `npm ci`; use the target RPC's estimates and verify the resulting source:

```sh
forge create src/Joah.sol:Joah --rpc-url https://carrot.megaeth.com/rpc \
  --account your-keystore --broadcast \
  --constructor-args 0xYourTreasury 1000000000000 0x4e1673dcAA38136b5032F27ef93423162aF977Cc
```

Do not use generic local-EVM gas figures as MegaETH gas limits. Verify compiler,
optimizer, constructor arguments and source on the explorer. Configure ClipClub
with the resulting contract address after the deployment is confirmed.

## Sources

- [MegaETH developer skill](https://github.com/megaeth-labs/skills/blob/main/skills/megaeth-developer-skills/SKILL.md)
- [Official drand integration reference](https://github.com/megaeth-labs/skills/blob/main/skills/megaeth-developer-skills/references/vrf-drand.md)
- [MegaETH VRF documentation](https://docs.megaeth.com/developer-docs/vrf)
- [Verifier implementation](https://github.com/Zodomo/DrandVerifier/blob/main/src/oracles/DrandOracleQuicknet.sol)
- [MegaETH contract and gas guidance](https://github.com/megaeth-labs/skills/blob/main/skills/megaeth-developer-skills/references/deploy-contracts.md)
- [ENS deployment addresses](https://docs.ens.domains/learn/deployments/)
