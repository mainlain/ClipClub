# 좋아 (✨) on MegaETH

`src/Joah.sol` combines an OpenZeppelin ERC20 with fixed game rules. The name is
`좋아`, the symbol is `✨`, and one token has six decimal places. ClipClub uses a
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
deflationary in expectation. Existing total-payout semantics are preserved.
Worst-case pending profit reserves prevent supply overflow at settlement.

There is no timeout refund or alternate round: either could allow selective
loss cancellation. Reveal is permissionless and has no expiry. If quicknet or
the verifier stops, stakes remain locked until service returns. A production
operator should run a keeper for liveness; the application also supports user
settlement. Randomness cadence is three seconds; a new request normally waits
three to six seconds before proof availability, plus transaction delays.

## Ratings

`registerClip(bytes32)` permanently assigns the registering wallet as that
content ID's payment recipient. First registration wins. This is a payment
directory, not evidence of authorship; registration can be front-run or squatted.
Register before distributing a clip and check the recipient before paying.

`rateClips(bytes32,bytes32[],uint8[],bytes32)` settles an entire rating session
in one transaction. It accepts 1–100 distinct clips, matching scores of 1–9,
a nonzero session ID, and the SHA-256 commitment of the rater's peer identity.
The contract checks the aggregate balance, pays every registered recipient,
emits one `ClipRated` event per item and a final `RatingsSettled` summary, and
marks that wallet's session ID consumed. A wallet cannot pay itself or rate
the same clip twice. Any failed item reverts every payment, event, and replay
marker in that batch. Ratings transfer existing tokens without changing supply.
The single-clip `rateClip` method remains available for interoperability.

The app queues ratings locally without wallet prompts. Queued ratings reserve
only the displayed budget; tokens remain in the wallet until checkout. Ending
the session offers one payment approval. Navigating away or closing the app
keeps the unpaid draft. Rejecting approval preserves it; a submitted batch
cannot be edited or discarded. The app validates the successful canonical
receipt's exact item events, session ID, total, and peer commitment before
atomically saving all rating receipts. Retrying confirmation uses the same
transaction hash and cannot pay twice. A verified reverted transaction resets
the unchanged draft for another attempt. Zero is always a free local skip.
Peer-reported seeder counts cannot mint tokens or claim another peer's payment.

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
ERC20 transfers/allowances, rating transfers, atomic batches, aggregate balance checks, session replay protection, and the 100-item bound. Local tests do not reproduce
MegaEVM gas accounting or validate the real BLS implementation.

`npm run build` writes ignored artifacts and refreshes committed
`abi-selectors.json`, which Rust includes directly. Compiler is Solidity
0.8.30, optimizer 200 runs, Shanghai EVM target. Runtime is about 6.7 KiB.

An optional read-only integration check validates a live quicknet proof against
the canonical chain verifier. It needs network access and no funded wallet:

```sh
npm run check:verifier
npm run check:verifier -- --mainnet
```

Ganache and its transitive packages are development-only. No npm package ships
inside ClipClub's Rust application or is used to hold wallet keys.

## Prepare and deploy

**Deployment is intentionally on hold pending the corrected rating distribution formula.** `mainnet-launch.json` records this status, and both preparation and preview commands stop before proceeding. Current contract economics remain unchanged; logo work is complete.

No contract has been deployed by this change. A funded MegaETH wallet is needed
for deployment, clip registration, ratings, requests, and reveal transactions.

| Network | Chain ID | RPC | Immutable verifier |
| --- | ---: | --- | --- |
| Mainnet | 4326 | `https://mainnet.megaeth.com/rpc` | `0x7a53a6eFA81c426838fcf4824E6e207923969b36` |
| Testnet | 6343 | `https://carrot.megaeth.com/rpc` | `0x4e1673dcAA38136b5032F27ef93423162aF977Cc` |

The constructor rejects other chain IDs, with isolated chain 31337 as the sole
exception for mock-oracle tests. On MegaETH it requires the canonical verifier
address and nonempty code.

The selected launch in tracked `mainnet-launch.json` is **MegaETH mainnet
(4326)** with **1,000,000,000 ✨** (`1000000000000000` base units) sent to
**`0xcBBF8e516F9a3ad56Efe28A0aEBf69Edc4639637`**. The recipient is the
explicit wallet address selected for this launch; no ENS resolution is used.

Prepare this unsigned mainnet deployment with a remote gas estimate and 20%
margin, then open the local wallet review page:

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
