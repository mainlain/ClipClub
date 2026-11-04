import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import ganache from 'ganache';
import {AbiCoder, BrowserProvider, ContractFactory, MaxUint256, ZeroAddress, id, keccak256, toBeHex, zeroPadValue} from 'ethers';
import {compile, root} from '../scripts/build.mjs';

const sources = compile({'test/Mocks.sol': {content: fs.readFileSync(path.join(root, 'test/Mocks.sol'), 'utf8')}});
const abiCoder = AbiCoder.defaultAbiCoder();
const UNIT = 1_000_000n;
const GENESIS = 1_692_803_367n;
const signature = `0x${'00'.repeat(48)}`;
const peerCommitment = id('ClipClub peer identity');

async function fixture(t, supply = 1_000_000n * UNIT, chainId = 31337) {
  const transport = ganache.provider({logging: {quiet: true}, chain: {chainId, hardfork: 'shanghai'}, wallet: {totalAccounts: 4}});
  t.after(() => transport.disconnect());
  const provider = new BrowserProvider(transport, undefined, {cacheTimeout: -1, pollingInterval: 10});
  const owner = await provider.getSigner(0), user = await provider.getSigner(1), keeper = await provider.getSigner(2);
  async function deploy(source, name, args) {
    const artifact = sources[source][name];
    const contract = await new ContractFactory(artifact.abi, `0x${artifact.evm.bytecode.object}`, owner).deploy(...args);
    await contract.waitForDeployment();
    return contract;
  }
  const oracle = await deploy('test/Mocks.sol', 'MockDrand', []);
  const joah = await deploy('test/Mocks.sol', 'JoahHarness', [await owner.getAddress(), supply, await oracle.getAddress()]);
  return {provider, owner, user, keeper, oracle, joah, deploy};
}

function tier(ticket) {
  for (let k = 0; k <= 9; ++k) {const weight = 512 >> k; if (ticket < weight) return k; ticket -= weight;}
  throw Error('bad ticket');
}
function uniformTicket(entropy) {
  const threshold = (1n << 256n) % 1023n;
  let value = BigInt(entropy), counter = 0n;
  while (value < threshold) value = BigInt(keccak256(abiCoder.encode(['bytes32', 'uint256'], [entropy, ++counter])));
  return Number(value % 1023n);
}
async function entropyFor(f, rollId, multiplier) {
  const roll = await f.joah.rolls(rollId);
  const address = await f.joah.getAddress();
  for (let n = 1n; n < 100_000n; ++n) {
    const hash = zeroPadValue(toBeHex(n), 32);
    const entropy = keccak256(abiCoder.encode(
      ['bytes32', 'address', 'uint256', 'uint256', 'address', 'uint256', 'uint64'],
      [hash, address, 31337, rollId, roll.player, roll.wager, roll.round]
    ));
    if (tier(uniformTicket(entropy)) === multiplier) return hash;
  }
  throw Error('search exhausted');
}
async function publish(f, round) {
  await f.provider.send('evm_setTime', [Number((GENESIS + (round - 1n) * 3n) * 1000n)]);
  await f.provider.send('evm_mine', []);
}
async function tx(promise) {return (await promise).wait();}
async function minedRevert(promise) {
  const sent = await promise;
  let failure;
  try {await sent.wait();} catch (error) {failure = error;}
  assert.ok(failure?.receipt, 'expected an executed transaction to revert');
  assert.equal(failure.receipt.status, 0);
  assert.deepEqual(failure.receipt.logs, [], 'reverted rating batches must leave no logs');
  return failure.receipt;
}
function logs(f, receipt, name) {
  return receipt.logs.map(log => {try {return f.joah.interface.parseLog(log);} catch {return null;}}).filter(log => log?.name === name);
}

test('ERC20 metadata, explicit genesis grant, transfers and allowances', async t => {
  const f = await fixture(t);
  assert.equal(await f.joah.name(), '좋아');
  assert.equal(await f.joah.symbol(), '✨');
  assert.equal(await f.joah.decimals(), 6n);
  assert.equal(await f.joah.totalSupply(), 1_000_000n * UNIT);
  assert.equal(await f.joah.balanceOf(await f.user.getAddress()), 0n, 'new wallets get no mint');
  await tx(f.joah.approve(await f.user.getAddress(), 20n * UNIT));
  await tx(f.joah.connect(f.user).transferFrom(await f.owner.getAddress(), await f.keeper.getAddress(), 7n * UNIT));
  assert.equal(await f.joah.allowance(await f.owner.getAddress(), await f.user.getAddress()), 13n * UNIT);
  assert.equal(await f.joah.balanceOf(await f.keeper.getAddress()), 7n * UNIT);
  assert.equal(f.joah.interface.hasFunction('mint'), false);
});

test('deployment rejects unsupported chains, no-code verifiers and invalid treasury', async t => {
  const f = await fixture(t);
  await assert.rejects(f.deploy('src/Joah.sol', 'Joah', [ZeroAddress, 100n, await f.oracle.getAddress()]));
  await assert.rejects(f.deploy('src/Joah.sol', 'Joah', [await f.owner.getAddress(), 100n, await f.user.getAddress()]));
  const transport = ganache.provider({logging: {quiet: true}, chain: {chainId: 1, hardfork: 'shanghai'}});
  t.after(() => transport.disconnect());
  const provider = new BrowserProvider(transport);
  const owner = await provider.getSigner();
  const a = sources['src/Joah.sol'].Joah;
  await assert.rejects(new ContractFactory(a.abi, `0x${a.evm.bytecode.object}`, owner).deploy(await owner.getAddress(), 100n, await owner.getAddress()));
});

test('all 1023 tickets have the exact 512..1 distribution; rejection sampling is deterministic', async t => {
  const f = await fixture(t);
  assert.deepEqual([...(await f.joah.distribution())], [512n, 256n, 128n, 64n, 32n, 16n, 8n, 4n, 2n, 1n]);
  for (const n of [0n, 1n, 63n, 64n, 65n, MaxUint256]) {
    const entropy = zeroPadValue(toBeHex(n), 32);
    assert.equal(await f.joah.ticket(entropy), BigInt(uniformTicket(entropy)));
  }
});

test('commit escrows funds and fixes a future round, wager and identity; duplicate/premature/forged reveals fail', async t => {
  const f = await fixture(t), owner = await f.owner.getAddress();
  await assert.rejects(f.joah.requestRoll(0));
  await assert.rejects(f.joah.connect(f.user).requestRoll(UNIT));
  const receipt = await tx(f.joah.requestRoll(2n * UNIT));
  const roll = await f.joah.rolls(1);
  const block = await f.provider.getBlock(receipt.blockNumber);
  const currentRound = (BigInt(block.timestamp) - GENESIS) / 3n + 1n;
  assert.equal(roll.round, currentRound + 2n);
  assert.equal(roll.player, owner);
  assert.equal(roll.wager, 2n * UNIT);
  assert.equal(await f.joah.pendingRoll(owner), 1n);
  assert.equal(await f.joah.balanceOf(await f.joah.getAddress()), 2n * UNIT);
  assert.equal(await f.joah.totalSupply(), 1_000_000n * UNIT);
  await assert.rejects(f.joah.requestRoll(UNIT));
  await tx(f.oracle.set(roll.round, id('valid round')));
  await assert.rejects(f.joah.settleRoll(1, signature));
  await publish(f, roll.round);
  await assert.rejects(f.joah.settleRoll(1, '0x00'));
  await assert.rejects(f.joah.settleRoll(1, `0x${'ff'.repeat(48)}`));
  await assert.rejects(f.joah.settleRoll(2, signature));
  await tx(f.joah.connect(f.keeper).settleRoll(1, signature));
  assert.equal((await f.joah.rolls(1)).settled, true);
  assert.equal(await f.joah.pendingRoll(owner), 0n);
  await assert.rejects(f.joah.settleRoll(1, signature));
});

test('each payout burns losses, returns pushes, and mints only net winning profit; anyone settles', async t => {
  const f = await fixture(t), owner = await f.owner.getAddress(), wager = 2n * UNIT;
  for (let multiplier = 0; multiplier <= 9; ++multiplier) {
    const rollId = await f.joah.nextRollId();
    const beforeSupply = await f.joah.totalSupply(), beforeBalance = await f.joah.balanceOf(owner);
    const beforeBurned = await f.joah.totalBurned(), beforeMinted = await f.joah.totalWinMinted();
    await tx(f.joah.requestRoll(wager));
    const roll = await f.joah.rolls(rollId);
    await tx(f.oracle.set(roll.round, await entropyFor(f, rollId, multiplier)));
    await publish(f, roll.round);
    const receipt = await tx(f.joah.connect(f.keeper).settleRoll(rollId, signature));
    const event = receipt.logs.map(log => {try {return f.joah.interface.parseLog(log);} catch {return null;}}).find(log => log?.name === 'RollSettled');
    assert.equal(event.args.payout, wager * BigInt(multiplier));
    assert.equal(event.args.multiplier, BigInt(multiplier));
    const delta = wager * BigInt(multiplier - 1);
    assert.equal(await f.joah.balanceOf(owner), beforeBalance + delta);
    assert.equal(await f.joah.totalSupply(), beforeSupply + delta);
    assert.equal(await f.joah.totalBurned(), beforeBurned + (multiplier === 0 ? wager : 0n));
    assert.equal(await f.joah.totalWinMinted(), beforeMinted + (multiplier > 1 ? delta : 0n));
    assert.equal(await f.joah.reservedWinProfit(), 0n);
    assert.equal(await f.joah.balanceOf(await f.joah.getAddress()), 0n);
  }
});

test('normalized alternative signature encodings produce the same result and wrong-round entropy cannot settle', async t => {
  const f = await fixture(t);
  await tx(f.joah.requestRoll(UNIT));
  const roll = await f.joah.rolls(1);
  await tx(f.oracle.set(roll.round + 1n, id('wrong round')));
  await publish(f, roll.round);
  await assert.rejects(f.joah.settleRoll(1, signature));
  await tx(f.oracle.set(roll.round, id('correct round')));
  const snapshot = await f.provider.send('evm_snapshot', []);
  await tx(f.joah.settleRoll(1, signature));
  const first = await f.joah.rolls(1);
  await f.provider.send('evm_revert', [snapshot]);
  await tx(f.joah.settleRoll(1, `0x${'00'.repeat(96)}`));
  assert.equal((await f.joah.rolls(1)).multiplier, first.multiplier);
});

test('pending reserves prevent accepted maximum payouts from overflowing supply', async t => {
  const f = await fixture(t, MaxUint256 - 100n);
  await tx(f.joah.transfer(await f.user.getAddress(), 20n));
  await tx(f.joah.requestRoll(10n)); // reserves80; only20 headroom remains
  await assert.rejects(f.joah.connect(f.user).requestRoll(3n));
  await tx(f.joah.connect(f.user).requestRoll(2n));
  assert.equal(await f.joah.reservedWinProfit(), 96n);
  const roll = await f.joah.rolls(1);
  await tx(f.oracle.set(roll.round, await entropyFor(f, 1n, 9)));
  await publish(f, roll.round);
  await tx(f.joah.settleRoll(1, signature));
  assert.equal(await f.joah.totalSupply(), MaxUint256 - 20n);
  assert.equal(await f.joah.reservedWinProfit(), 16n);
});

test('ratings pay the immutable registered recipient without minting; self/duplicate/invalid ratings fail', async t => {
  const f = await fixture(t), clip = id('content hash');
  await tx(f.joah.connect(f.user).registerClip(clip));
  await assert.rejects(f.joah.registerClip(clip));
  await assert.rejects(f.joah.registerClip(zeroPadValue('0x00', 32)));
  await assert.rejects(f.joah.connect(f.user).rateClip(clip, 1, peerCommitment));
  await assert.rejects(f.joah.rateClip(clip, 0, peerCommitment));
  await assert.rejects(f.joah.rateClip(clip, 10, peerCommitment));
  await assert.rejects(f.joah.rateClip(id('unregistered'), 1, peerCommitment));
  await assert.rejects(f.joah.rateClip(clip, 7, zeroPadValue('0x00', 32)));
  const supply = await f.joah.totalSupply();
  const receipt = await tx(f.joah.rateClip(clip, 7, peerCommitment));
  assert.equal(await f.joah.balanceOf(await f.user.getAddress()), 7n * UNIT);
  assert.equal(await f.joah.totalSupply(), supply);
  assert.equal(await f.joah.hasRated(clip, await f.owner.getAddress()), true);
  assert.equal(await f.joah.clipBeneficiary(clip), await f.user.getAddress());
  const rating = f.joah.interface.parseLog(receipt.logs.at(-1));
  assert.equal(rating.name, 'ClipRated');
  assert.equal(rating.args.peerCommitment, peerCommitment, 'the wallet transaction binds payment to the intended peer');
  await assert.rejects(f.joah.rateClip(clip, 9, peerCommitment));
});

test('one atomic session pays multiple recipients and emits exact per-clip and summary receipts without minting', async t => {
  const f = await fixture(t), owner = await f.owner.getAddress(), user = await f.user.getAddress(), keeper = await f.keeper.getAddress();
  const clips = [id('batch clip one'), id('batch clip two'), id('batch clip three')];
  await tx(f.joah.connect(f.user).registerClip(clips[0]));
  await tx(f.joah.connect(f.keeper).registerClip(clips[1]));
  await tx(f.joah.connect(f.user).registerClip(clips[2]));
  const session = id('immutable local session'), scores = [2, 7, 9], supply = await f.joah.totalSupply();
  const receipt = await tx(f.joah.rateClips(session, clips, scores, peerCommitment));
  const ratings = logs(f, receipt, 'ClipRated'), settlements = logs(f, receipt, 'RatingsSettled');
  assert.equal(ratings.length, 3);
  assert.equal(logs(f, receipt, 'Transfer').length, 3);
  for (let i = 0; i < clips.length; ++i) {
    assert.equal(ratings[i].args.clipId, clips[i]);
    assert.equal(ratings[i].args.rater, owner);
    assert.equal(ratings[i].args.score, BigInt(scores[i]));
    assert.equal(ratings[i].args.amount, BigInt(scores[i]) * UNIT);
    assert.equal(ratings[i].args.peerCommitment, peerCommitment);
    assert.equal(await f.joah.hasRated(clips[i], owner), true);
  }
  assert.equal(settlements.length, 1);
  const summary = settlements[0].args;
  assert.equal(summary.sessionId, session);
  assert.equal(summary.rater, owner);
  assert.equal(summary.count, 3n);
  assert.equal(summary.amount, 18n * UNIT);
  assert.equal(summary.peerCommitment, peerCommitment);
  assert.equal(f.joah.interface.parseLog(receipt.logs.at(-1)).name, 'RatingsSettled');
  assert.equal(await f.joah.balanceOf(owner), supply - 18n * UNIT);
  assert.equal(await f.joah.balanceOf(user), 11n * UNIT);
  assert.equal(await f.joah.balanceOf(keeper), 7n * UNIT);
  assert.equal(await f.joah.totalSupply(), supply);
  assert.equal(await f.joah.totalBurned(), 0n);
  assert.equal(await f.joah.totalWinMinted(), 0n);
  assert.equal(await f.joah.settledSession(owner, session), true);
  // Reusing the session with different clips is also rejected, rather than
  // relying only on the individual clip replay guards.
  const fresh = id('fresh clip after settlement');
  await tx(f.joah.connect(f.user).registerClip(fresh));
  await minedRevert(f.joah.rateClips(session, [fresh], [1], peerCommitment, {gasLimit: 3_000_000}));
  assert.equal(await f.joah.hasRated(fresh, owner), false);
  await assert.rejects(f.joah.rateClip(clips[0], 1, peerCommitment));
  await minedRevert(f.joah.rateClips(id('replacement session'), clips, scores, peerCommitment, {gasLimit: 3_000_000}));
  assert.equal(await f.joah.settledSession(owner, id('replacement session')), false);
});

test('session metadata rejects zero IDs, zero peer commitments, empty/mismatched/over-limit arrays and remains retryable', async t => {
  const f = await fixture(t), owner = await f.owner.getAddress(), clip = id('metadata validation clip'), session = id('retryable session');
  const zero = zeroPadValue('0x00', 32), supply = await f.joah.totalSupply();
  await tx(f.joah.connect(f.user).registerClip(clip));
  const tooMany = Array.from({length: 101}, (_, i) => id(`over-limit ${i}`));
  const cases = [
    [zero, [clip], [1], peerCommitment],
    [session, [clip], [1], zero],
    [session, [], [], peerCommitment],
    [session, [clip], [], peerCommitment],
    [session, [clip], [1, 2], peerCommitment],
    [session, tooMany, Array(101).fill(1), peerCommitment],
  ];
  for (const args of cases) {
    await minedRevert(f.joah.rateClips(...args, {gasLimit: 3_000_000}));
    assert.equal(await f.joah.settledSession(owner, session), false);
    assert.equal(await f.joah.hasRated(clip, owner), false);
    assert.equal(await f.joah.balanceOf(owner), supply);
    assert.equal(await f.joah.balanceOf(await f.user.getAddress()), 0n);
  }
  // A rejected attempt does not consume its ID; a one-item batch is valid.
  await tx(f.joah.rateClips(session, [clip], [1], peerCommitment));
  assert.equal(await f.joah.settledSession(owner, session), true);
});

test('any invalid or duplicate batch item rolls back earlier marks, all payments, session state and logs', async t => {
  const f = await fixture(t), owner = await f.owner.getAddress(), user = await f.user.getAddress();
  const valid = id('valid first item'), other = id('valid second item'), self = id('self recipient'), prior = id('previously rated');
  for (const clip of [valid, other, prior]) await tx(f.joah.connect(f.user).registerClip(clip));
  await tx(f.joah.registerClip(self));
  await tx(f.joah.rateClip(prior, 1, peerCommitment));
  const ownerBalance = await f.joah.balanceOf(owner), userBalance = await f.joah.balanceOf(user), supply = await f.joah.totalSupply();
  const cases = [
    [[valid, other], [2, 0]],
    [[valid, other], [2, 10]],
    [[valid, id('unregistered batch item')], [2, 1]],
    [[valid, zeroPadValue('0x00', 32)], [2, 1]],
    [[valid, self], [2, 1]],
    [[valid, valid], [2, 3]],
    [[valid, prior], [2, 1]],
  ];
  for (const [i, [clipIds, scores]] of cases.entries()) {
    const session = id(`invalid atomic session ${i}`);
    await minedRevert(f.joah.rateClips(session, clipIds, scores, peerCommitment, {gasLimit: 3_000_000}));
    assert.equal(await f.joah.hasRated(valid, owner), false);
    assert.equal(await f.joah.hasRated(other, owner), false);
    assert.equal(await f.joah.hasRated(prior, owner), true);
    assert.equal(await f.joah.settledSession(owner, session), false);
    assert.equal(await f.joah.balanceOf(owner), ownerBalance);
    assert.equal(await f.joah.balanceOf(user), userBalance);
    assert.equal(await f.joah.totalSupply(), supply);
  }
});

test('aggregate session balance is guarded and a funded retry settles the same frozen session exactly once', async t => {
  const f = await fixture(t, 10n * UNIT), owner = await f.owner.getAddress(), user = await f.user.getAddress(), rater = await f.keeper.getAddress();
  const clips = [id('aggregate cost one'), id('aggregate cost two')], session = id('needs one more token');
  await tx(f.joah.registerClip(clips[0]));
  await tx(f.joah.connect(f.user).registerClip(clips[1]));
  await tx(f.joah.transfer(rater, 7n * UNIT));
  await minedRevert(f.joah.connect(f.keeper).rateClips(session, clips, [4, 4], peerCommitment, {gasLimit: 3_000_000}));
  assert.equal(await f.joah.balanceOf(rater), 7n * UNIT);
  assert.equal(await f.joah.balanceOf(owner), 3n * UNIT);
  assert.equal(await f.joah.balanceOf(user), 0n);
  assert.equal(await f.joah.settledSession(rater, session), false);
  for (const clip of clips) assert.equal(await f.joah.hasRated(clip, rater), false);
  await tx(f.joah.transfer(rater, UNIT));
  await tx(f.joah.connect(f.keeper).rateClips(session, clips, [4, 4], peerCommitment));
  assert.equal(await f.joah.balanceOf(rater), 0n);
  assert.equal(await f.joah.balanceOf(owner), 6n * UNIT);
  assert.equal(await f.joah.balanceOf(user), 4n * UNIT);
  assert.equal(await f.joah.totalSupply(), 10n * UNIT);
  assert.equal(await f.joah.settledSession(rater, session), true);
});

test('exactly 100 session items succeed and session replay protection is scoped to each wallet', async t => {
  const f = await fixture(t), owner = await f.owner.getAddress(), user = await f.user.getAddress(), keeper = await f.keeper.getAddress();
  assert.equal(await f.joah.MAX_RATINGS_PER_SESSION(), 100n);
  const clips = Array.from({length: 100}, (_, i) => id(`maximum-size session clip ${i}`));
  const scores = clips.map((_, i) => i % 9 + 1), session = id('shared session identifier');
  for (const clip of clips) await tx(f.joah.connect(f.user).registerClip(clip));
  const amount = scores.reduce((sum, score) => sum + BigInt(score) * UNIT, 0n), supply = await f.joah.totalSupply();
  const receipt = await tx(f.joah.rateClips(session, clips, scores, peerCommitment));
  assert.equal(logs(f, receipt, 'ClipRated').length, 100);
  assert.equal(logs(f, receipt, 'Transfer').length, 100);
  assert.equal(logs(f, receipt, 'RatingsSettled')[0].args.amount, amount);
  assert.equal(logs(f, receipt, 'RatingsSettled')[0].args.count, 100n);
  assert.equal(await f.joah.balanceOf(user), amount);
  assert.equal(await f.joah.balanceOf(owner), supply - amount);
  assert.equal(await f.joah.totalSupply(), supply);
  assert.equal(await f.joah.hasRated(clips[0], owner), true);
  assert.equal(await f.joah.hasRated(clips[99], owner), true);
  // Another wallet may use the same random session identifier independently.
  await tx(f.joah.transfer(keeper, UNIT));
  await tx(f.joah.connect(f.keeper).rateClips(session, [clips[0]], [1], peerCommitment));
  assert.equal(await f.joah.settledSession(owner, session), true);
  assert.equal(await f.joah.settledSession(keeper, session), true);
  assert.equal(await f.joah.balanceOf(user), amount + UNIT);
});
