#!/usr/bin/env node
// Real ClipClub processes + a disposable local EVM. No public RPC, wallet key,
// real-chain transaction, or existing profile is used.
// Usage: node scripts/session-integration.mjs [dist/clipclub] [.build/ffmpeg]
//        node scripts/session-integration.mjs --fixture-only
import assert from 'node:assert/strict';
import {createHash, randomBytes} from 'node:crypto';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import {spawn, spawnSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import ganache from '../contracts/node_modules/ganache/dist/node/core.js';
import {ContractFactory, FetchRequest, JsonRpcProvider} from '../contracts/node_modules/ethers/lib.esm/index.js';
import {compile} from '../contracts/scripts/build.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const fixtureOnly = process.argv.includes('--fixture-only');
const args = process.argv.slice(2).filter(arg => arg !== '--fixture-only');
const binary = path.resolve(args[0] || path.join(root, 'dist/clipclub'));
const ffmpeg = path.resolve(args[1] || path.join(root, '.build/ffmpeg'));
const work = fs.mkdtempSync(path.join(os.tmpdir(), 'clipclub-session-e2e-'));
const verifier = '0x7a53a6eFA81c426838fcf4824E6e207923969b36';
const UNIT = 1_000_000n;
const clients = [];
const abort = new AbortController();
const timeout = setTimeout(() => abort.abort(Error('Session integration exceeded 180 seconds')), 180_000);
const signalHandlers = ['SIGINT', 'SIGTERM'].map(signal => {
  const handler = () => abort.abort(Error(`Interrupted by ${signal}`));
  process.once(signal, handler);
  return [signal, handler];
});
let server, provider, environment, succeeded = false;

function sleep(ms) {return new Promise(resolve => setTimeout(resolve, ms));}
async function until(check, description, milliseconds = 40_000) {
  const deadline = Date.now() + milliseconds;
  let last;
  while (Date.now() < deadline) {
    abort.signal.throwIfAborted();
    try {const value = await check(); if (value) return value;} catch (error) {
      if (error.fatal) throw error;
      last = error.message;
    }
    await sleep(150);
  }
  throw Error(`${description} timed out${last ? `: ${last}` : ''}`);
}
async function freePort() {
  const socket = net.createServer();
  await new Promise((resolve, reject) => {socket.once('error', reject); socket.listen(0, '127.0.0.1', resolve);});
  const port = socket.address().port;
  await new Promise((resolve, reject) => socket.close(error => error ? reject(error) : resolve()));
  return port;
}
async function mine(promise) {return (await promise).wait();}
function saved(client) {return JSON.parse(fs.readFileSync(path.join(client.folder, 'state.json'), 'utf8'));}
function localReceipts(client) {return saved(client).receipts.filter(receipt => receipt.payload.sender === client.identity);}

class Client {
  constructor(name) {this.name = name; this.folder = path.join(work, name); clients.push(this);}
  checkAlive() {
    if (this.spawnError || this.child?.exitCode !== null || this.child?.signalCode !== null) {
      throw Object.assign(Error(`${this.name} exited: ${this.spawnError?.message || this.child.exitCode || this.child.signalCode}; see ${this.log}`), {fatal: true});
    }
  }
  async start() {
    this.peerPort ||= await freePort();
    this.torrentPort ||= await freePort();
    const previousPort = this.httpPort;
    do {this.httpPort = await freePort();} while (this.httpPort === previousPort);
    this.url = `http://127.0.0.1:${this.httpPort}`;
    this.log = path.join(work, `${this.name}.log`);
    const descriptor = fs.openSync(this.log, 'a');
    this.spawnError = null;
    this.child = spawn(binary, ['--headless', '--data-dir', this.folder, '--http-port', String(this.httpPort), '--p2p-port', String(this.peerPort), '--torrent-port', String(this.torrentPort)], {env: environment, stdio: ['ignore', descriptor, descriptor]});
    fs.closeSync(descriptor);
    this.child.once('error', error => {this.spawnError = error;});
    await until(async () => {
      this.checkAlive();
      const response = await fetch(this.url, {signal: AbortSignal.any([abort.signal, AbortSignal.timeout(2_000)])});
      const html = await response.text();
      this.token = html.match(/name="api-token" content="([a-f0-9]+)"/)?.[1];
      return response.ok && this.token;
    }, `${this.name} startup`);
    this.identity = (await this.state()).profile_id;
  }
  async stop() {
    const child = this.child;
    if (!child || this.spawnError || child.exitCode !== null || child.signalCode !== null) return;
    const exited = new Promise(resolve => child.once('exit', resolve));
    child.kill('SIGINT');
    let stopTimer;
    const stopped = await Promise.race([exited.then(() => true), new Promise(resolve => {stopTimer = setTimeout(() => resolve(false), 8_000);})]);
    clearTimeout(stopTimer);
    if (!stopped) {
      child.kill('SIGKILL');
      await exited;
    }
  }
  async api(route, data) {
    this.checkAlive();
    const options = {headers: {'x-clipcrop-token': this.token}, signal: AbortSignal.any([abort.signal, AbortSignal.timeout(15_000)])};
    if (data !== undefined) {
      options.method = 'POST';
      if (data instanceof FormData) options.body = data;
      else {options.headers['Content-Type'] = 'application/json'; options.body = JSON.stringify(data);}
    }
    const response = await fetch(`${this.url}/api/${route}`, options);
    const text = await response.text();
    if (!response.ok) throw Error(`${this.name} ${route}: HTTP ${response.status} ${text}`);
    return JSON.parse(text);
  }
  state() {return this.api('state');}
  async upload(filename) {
    const data = new FormData();
    data.append('video', new Blob([fs.readFileSync(filename)], {type: 'video/mp4'}), path.basename(filename));
    return (await this.api('upload', data)).clip;
  }
}

async function setupEvm() {
  const source = fs.readFileSync(path.join(root, 'contracts/test/Mocks.sol'), 'utf8');
  const compiled = compile({'test/Mocks.sol': {content: source}});
  server = ganache.server({logging: {quiet: true}, chain: {chainId: 4326, networkId: 4326, hardfork: 'shanghai'}, wallet: {totalAccounts: 4}});
  await server.listen(0, '127.0.0.1');
  const rpc = `http://127.0.0.1:${server.address().port}`;
  const request = new FetchRequest(rpc); request.timeout = 10_000;
  provider = new JsonRpcProvider(request, undefined, {cacheTimeout: -1, pollingInterval: 10});
  assert.equal((await provider.getNetwork()).chainId, 4326n);
  const mockCode = `0x${compiled['test/Mocks.sol'].MockDrand.evm.deployedBytecode.object}`;
  assert.equal(await provider.send('evm_setAccountCode', [verifier, mockCode]), true);
  assert.equal(await provider.getCode(verifier), mockCode);
  const owner = await provider.getSigner(0), rater = await provider.getSigner(1), seeder = await provider.getSigner(2);
  const artifact = compiled['src/Joah.sol'].Joah;
  const token = await new ContractFactory(artifact.abi, `0x${artifact.evm.bytecode.object}`, owner).deploy(await owner.getAddress(), 1_000n * UNIT, verifier);
  await token.waitForDeployment();
  await mine(token.transfer(await rater.getAddress(), 100n * UNIT));
  assert.equal(await token.verifier(), verifier);
  assert.equal(await token.balanceOf(await rater.getAddress()), 100n * UNIT);
  return {rpc, owner, rater, seeder, token};
}

async function run() {
  if (!fixtureOnly) for (const filename of [binary, ffmpeg, path.join(root, 'vendor/ffprobe')]) fs.accessSync(filename, fs.constants.X_OK);
  const {rpc, owner, rater, seeder, token} = await setupEvm();
  if (fixtureOnly) {console.log('PASS: local chain 4326, canonical-address mock verifier, guarded Joah deployment and funded test rater.'); return;}
  const contract = await token.getAddress(), ownerAddress = await owner.getAddress(), raterAddress = await rater.getAddress();
  environment = {...process.env, CLIPCLUB_MEGAETH_CHAIN_ID: '4326', CLIPCLUB_MEGAETH_RPC_URL: rpc, CLIPCLUB_JOAH_CONTRACT: contract, CLIPCROP_FFPROBE: path.join(root, 'vendor/ffprobe')};
  // Background release checks must fail locally instead of using a public service.
  for (const key of ['HTTPS_PROXY', 'HTTP_PROXY', 'ALL_PROXY', 'https_proxy', 'http_proxy', 'all_proxy']) environment[key] = 'http://127.0.0.1:9';
  environment.NO_PROXY = environment.no_proxy = '127.0.0.1,localhost,::1';
  const alice = new Client('Alice'), bob = new Client('Bob'), charlie = new Client('Charlie');
  await alice.start(); await bob.start(); await charlie.start();
  // Seed a persistent monthly-history fixture of valid peer IDs, independently
  // of currently connected peers. This lets the integration exercise excess
  // seeder rewards at N=100 with only three real desktop processes.
  await bob.stop();
  const history = saved(bob); history.peer_activity = {};
  const alphabet = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
  for (let i = 0; i < 97; ++i) {
    let n = BigInt('0x1220' + randomBytes(32).toString('hex')), peer = '';
    while(n) {peer = alphabet[Number(n % 58n)] + peer; n /= 58n;}
    history.peer_activity[peer] = Date.now();
  }
  for (const peer of [alice.identity,charlie.identity]) history.peer_activity[peer] = Date.now();
  fs.writeFileSync(path.join(bob.folder,'state.json'),JSON.stringify(history));
  await bob.start();
  assert.equal((await bob.state()).population,100);
  await alice.api('wallet', {address: ownerAddress});
  await bob.api('wallet', {address: raterAddress});
  await charlie.api('wallet',{address:await seeder.getAddress()});
  for (const [client,signer] of [[alice,owner],[charlie,seeder]]) {
    const registration = await client.api('crypto/transaction',{action:'register_seeder'});
    const sent = await signer.sendTransaction({to:registration.to,data:registration.data});
    await client.api('crypto/pending',{action:'register_seeder',hash:sent.hash});
    await sent.wait();
    await client.api('crypto/confirm-account',{action:'register_seeder',transaction_hash:sent.hash});
    await client.api('crypto/pending',null);
    assert.equal((await client.state()).crypto.seeder_registered,true);
  }
  assert.equal(await token.ratingCredit(raterAddress),0n);
  const settings = (await bob.state()).settings;
  settings.display_name = 'Session test Bob';
  settings.bootstrap_peers = [alice,charlie].map(client=>`/ip4/127.0.0.1/tcp/${client.peerPort}/p2p/${client.identity}`);
  await bob.api('settings', settings);
  await until(async () => (await alice.state()).peers.some(peer => peer.id === bob.identity) && (await bob.state()).peers.some(peer => peer.id === alice.identity), 'mutual peer discovery');
  for (const [i, color] of ['0x278065', '0xdb653d'].entries()) {
    const filename = path.join(work, `session-clip-${i}.mp4`);
    const result = spawnSync(ffmpeg, ['-v', 'error', '-f', 'lavfi', '-i', `color=c=${color}:s=96x64:r=10`, '-t', '0.3', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-metadata', `title=ClipClub-session-${i}`, '-y', filename], {encoding: 'utf8', timeout: 20_000});
    assert.equal(result.status, 0, result.stderr || result.error?.message);
    const clip = await alice.upload(filename);
    await charlie.upload(filename);
    assert.equal(await token.clipBeneficiary(`0x${clip.id}`), '0x0000000000000000000000000000000000000000', 'seeder payments need no first-claim clip registration');
  }
  await bob.api('session', {active: true});
  await until(async () => {const state=await bob.state();return state.queue.length >= 2 && state.queue.every(c=>c.distribution_preview.seeders===2);}, 'two real transferred clips with two permanent seeders', 60_000);
  const ratings = [];
  for (const score of [5, 7]) {
    const state = await bob.state(), clip = state.queue[0];
    assert.ok(clip, 'next clip must be playable before queuing');
    assert.equal(clip.distribution_preview.population,100);assert.equal(clip.distribution_preview.seeders,2);
    const response = await bob.api('rate', {id: clip.id, score});
    assert.equal(response.queued, true); assert.equal(response.paid, false);
    ratings.push({clip_id: clip.id, score});
    assert.equal(await token.hasRated(`0x${clip.id}`, raterAddress), false);
    assert.equal(await token.balanceOf(raterAddress), 100n * UNIT, 'queuing a rating must not transfer tokens');
    assert.equal(localReceipts(bob).length, 0, 'an unpaid draft must not create signed paid receipts');
    assert.equal(saved(bob).watched.includes(clip.id), false);
  }
  const draft = (await bob.state()).crypto.rating_session;
  assert.equal(draft.status, 'draft'); assert.equal(draft.total_units, '12000000');
  assert.deepEqual(draft.ratings.map(({clip_id, score}) => ({clip_id, score})), ratings);
  for (const rating of draft.ratings) {
    assert.equal(rating.distribution.population,100);assert.equal(rating.distribution.recipients.length,2);
    assert.deepEqual(new Set(rating.distribution.recipients.map(r=>r.peer)),new Set([alice.identity,charlie.identity]));
    const n=100, logistic=x=>1/(1+Math.exp(100*(x/n-0.05)));
    const percentage=1+99*(logistic(2)-logistic(n))/(logistic(1)-logistic(n));
    assert.equal(rating.distribution.reward_units,String(Math.floor(rating.score*1_000_000*percentage/100)));
  }
  const perSeederPayout = draft.ratings.reduce((sum,r)=>sum+BigInt(r.distribution.reward_units),0n);
  const expectedMint=2n*perSeederPayout-12n*UNIT;
  assert.ok(expectedMint>0n,'the session must mint a genuine reward shortfall');
  assert.equal(draft.mint_required_units,expectedMint.toString());
  await bob.api('session', {active: false});
  const identity = bob.identity, origin = bob.url;
  await bob.stop(); await bob.start();
  assert.equal(bob.identity, identity); assert.notEqual(bob.url, origin);
  assert.deepEqual((await bob.state()).crypto.rating_session, draft, 'the exact unpaid draft must survive a new local origin');
  assert.equal(localReceipts(bob).length, 0);
  assert.equal(await token.balanceOf(raterAddress), 100n * UNIT);

  const transaction = await bob.api('crypto/transaction', {action: 'rate_session', expected_session_id: draft.id, expected_rating_count: draft.ratings.length, expected_total_units: draft.total_units});
  const commitment = `0x${createHash('sha256').update(bob.identity).digest('hex')}`;
  assert.equal(transaction.data, token.interface.encodeFunctionData('settleSeederRatings', [`0x${draft.id}`, draft.ratings.map(r=>({clipId:`0x${r.clip_id}`,score:r.score,population:r.distribution.population,reward:r.distribution.reward_units,recipients:r.distribution.recipients.map(s=>s.wallet),peers:r.distribution.recipients.map(s=>'0x'+createHash('sha256').update(s.peer).digest('hex'))})), commitment]));
  assert.equal(transaction.from.toLowerCase(), raterAddress.toLowerCase());
  assert.equal(transaction.to.toLowerCase(), contract.toLowerCase());
  assert.equal(BigInt(transaction.chainId), 4326n);
  assert.equal((await bob.state()).crypto.rating_session.status, 'prepared');
  const nonce = await provider.getTransactionCount(raterAddress);
  const sent = await rater.sendTransaction({to: transaction.to, data: transaction.data, value: BigInt(transaction.value), chainId: 4326n});
  await bob.api('crypto/pending', {action: 'rate_session', hash: sent.hash, session_id: draft.id});
  const receipt = await sent.wait(); assert.equal(receipt.status, 1);
  assert.equal(await token.balanceOf(raterAddress), 88n * UNIT);
  assert.equal(await token.balanceOf(ownerAddress), 900n*UNIT+perSeederPayout);
  assert.equal(await token.balanceOf(await seeder.getAddress()),perSeederPayout);
  assert.equal(await token.ratingCredit(raterAddress),0n);
  assert.equal(await token.totalRatingMinted(),expectedMint);
  assert.equal(await token.balanceOf(contract),0n);
  assert.equal(await token.totalSupply(), 1_000n * UNIT+expectedMint);
  assert.equal(await token.settledSession(raterAddress, `0x${draft.id}`), true);
  for (const rating of ratings) assert.equal(await token.hasRated(`0x${rating.clip_id}`, raterAddress), true);
  // Exercise recovery after the chain has paid but before local receipts exist.
  await bob.stop(); await bob.start();
  const submitted = (await bob.state()).crypto;
  assert.equal(submitted.rating_session.id, draft.id);
  assert.equal(submitted.rating_session.status, 'submitted');
  assert.equal(submitted.pending_transaction.hash, sent.hash.toLowerCase());
  assert.equal(localReceipts(bob).length, 0);
  const confirmation = await bob.api('rating-session/confirm', {transaction_hash: sent.hash});
  assert.equal(confirmation.confirmed, true); assert.equal(confirmation.count, 2);
  assert.equal(confirmation.total_units, '12000000');
  await bob.api('crypto/pending', null);
  const confirmed = (await bob.state()).crypto;
  assert.equal(confirmed.rating_session, null); assert.equal(confirmed.pending_transaction, null);
  const paid = localReceipts(bob);
  assert.equal(paid.length, 2); assert.equal(new Set(paid.map(item => item.id)).size, 2);
  for (const [i, item] of paid.entries()) {
    assert.equal(item.payload.clip_id, ratings[i].clip_id); assert.equal(item.payload.score, ratings[i].score);
    assert.equal(item.payload.payment.transaction_hash, sent.hash.toLowerCase());
    assert.equal(item.payload.payment.wallet, raterAddress.toLowerCase());
    assert.equal(item.payload.payment.contract, contract.toLowerCase());
    assert.equal(item.payload.payment.chain_id, 4326);
    assert.equal(item.payload.sequence, i + 1); assert.equal(item.payload.previous, i ? paid[i - 1].id : null);
    assert.ok(item.public_key && item.signature); assert.ok(saved(bob).watched.includes(item.payload.clip_id));
  }
  await bob.api('rating-session/confirm', {transaction_hash: sent.hash});
  assert.deepEqual(localReceipts(bob), paid, 'confirmation retry must not append duplicate receipts');
  assert.equal(await token.balanceOf(raterAddress), 88n * UNIT);
  assert.equal(await provider.getTransactionCount(raterAddress), nonce + 1, 'a full session and all retries must use exactly one wallet transaction');
  await until(() => saved(alice).receipts.filter(item => item.payload.sender === bob.identity && item.payload.payment?.transaction_hash === sent.hash.toLowerCase()).length === 2, 'both signed ratings gossip to the real peer');
  const library = (await alice.state()).library;
  for (const rating of ratings) {
    const row = library.find(item => item.clip.id === rating.clip_id);
    assert.equal(row.average_rating, rating.score); assert.equal(row.rating_count, 1);
  }
  console.log('PASS: three real peers, persistent monthly N=100 and MP4 transfers; registered permanent seeder snapshots survive restart; exact Solidity ABI matches Rust; one batch charges only scores while only the excess rewards are minted without prefunding; receipt recovery, gossip and retries preserve every balance.');
}

try {await run(); succeeded = true;}
catch (error) {console.error(error.stack || error); console.error(`Session integration logs retained: ${work}`); process.exitCode = 1;}
finally {
  clearTimeout(timeout);
  for (const [signal, handler] of signalHandlers) process.removeListener(signal, handler);
  for (const client of clients.toReversed()) {try {await client.stop();} catch (error) {console.error(`Cleanup ${client.name}: ${error.message}`);}}
  provider?.destroy();
  if (server) {try {await server.close();} catch (error) {console.error(`Cleanup EVM: ${error.message}`);}}
  if (succeeded) fs.rmSync(work, {recursive: true, force: true});
}
