import assert from 'node:assert/strict';
import {timingSafeEqual, webcrypto} from 'node:crypto';
import test from 'node:test';
import worker from './worker.mjs';

// Node's WebCrypto omits the Workers extension; use Node's native constant-time primitive.
Object.defineProperty(globalThis, 'crypto', {value: {
  subtle: {digest: webcrypto.subtle.digest.bind(webcrypto.subtle), timingSafeEqual},
}, configurable: true});

class Bucket {
  objects = new Map();
  async put(key, text, options) {
    assert.equal(options.onlyIf.get('If-None-Match'), '*');
    if (this.objects.has(key)) return null;
    const object = {key, text, customMetadata: options.customMetadata, uploaded: new Date('2026-11-10T12:00:00Z')};
    this.objects.set(key, object);
    return object;
  }
  async get(key) {
    const object = this.objects.get(key);
    return object ? {...object, text: async () => object.text, body: new Response(object.text).body} : null;
  }
  async list(options) {
    assert.equal(options.prefix, 'reports/');
    assert.deepEqual(options.include, ['customMetadata']);
    const all = [...this.objects.values()].sort((a, b) => a.key.localeCompare(b.key));
    const offset = Number(options.cursor || 0), next = offset + options.limit;
    return {objects: all.slice(offset, next), truncated: next < all.length, cursor: String(next)};
  }
}
function fixture() {
  return {REPORTS: new Bucket(), UPLOAD_TOKEN: 'u'.repeat(64), READ_TOKEN: 'r'.repeat(64)};
}
function report(overrides = {}) {
  return {schema_version: 1, id: 'a'.repeat(32), generated_at_ms: 1794312000000,
    device: 'Test Mac', session: 'hotspot-a', suite: 'all', app_version: '0.1.3',
    platform: 'macos/aarch64', report: 'ClipClub diagnostics\nNo secrets.\n', ...overrides};
}
function request(env, method, path, body, token = method === 'PUT' ? env.UPLOAD_TOKEN : env.READ_TOKEN) {
  return worker.fetch(new Request('https://inbox.example' + path, {method, headers: {
    Authorization: 'Bearer ' + token, 'content-type': 'application/json',
  }, ...(body === undefined ? {} : {body: JSON.stringify(body)})}), env);
}

test('role separation and no report access without a key', async () => {
  const env = fixture(), data = report(), path = '/v1/reports/' + data.id;
  assert.equal((await request(env, 'PUT', path, data, env.READ_TOKEN)).status, 401);
  assert.equal((await request(env, 'PUT', path, data)).status, 201);
  for (const token of ['', 'wrong', env.UPLOAD_TOKEN]) {
    assert.equal((await request(env, 'GET', path, undefined, token)).status, 401);
    assert.equal((await request(env, 'GET', '/v1/reports', undefined, token)).status, 401);
  }
  const result = await request(env, 'GET', path);
  assert.deepEqual(await result.json(), data);
  assert.equal(result.headers.get('cache-control'), 'no-store');
  assert.equal(result.headers.get('x-content-type-options'), 'nosniff');
});

test('retries are idempotent and changed reports cannot overwrite an ID', async () => {
  const env = fixture(), data = report(), path = '/v1/reports/' + data.id;
  const responses = await Promise.all(Array.from({length: 12}, () => request(env, 'PUT', path, data)));
  assert.equal(responses.filter(response => response.status === 201).length, 1);
  assert.equal(responses.filter(response => response.status === 200).length, 11);
  assert.equal(env.REPORTS.objects.size, 1);
  assert.equal((await request(env, 'PUT', path, {...data, report: 'Changed'})).status, 409);
  assert.deepEqual(await (await request(env, 'GET', path)).json(), data);
});

test('invalid schema, mismatched IDs, unknown fields and oversized report text are rejected', async () => {
  const env = fixture(), path = '/v1/reports/' + 'a'.repeat(32);
  const invalid = [null, [], report({id: 'b'.repeat(32)}), report({schema_version: 2}),
    report({device: ''}), report({device: 'x'.repeat(81)}), report({session: 'x\n'}),
    report({suite: 'secret'}), report({generated_at_ms: Number.MAX_SAFE_INTEGER + 1}),
    report({generated_at_ms: -1}), report({report: ''}), report({report: 'é'.repeat(123000)}),
    {...report(), private_key: 'no'}, report({platform: ''})];
  for (const data of invalid) assert.equal((await request(env, 'PUT', path, data)).status, 400);
  assert.equal(env.REPORTS.objects.size, 0);
});

test('Unicode labels use character limits and round-trip as metadata', async () => {
  const env = fixture(), data = report({device: '😀'.repeat(80)});
  assert.equal((await request(env, 'PUT', '/v1/reports/' + data.id, data)).status, 201);
  const page = await (await request(env, 'GET', '/v1/reports')).json();
  assert.equal(page.reports[0].device, data.device);
  assert.equal(page.reports[0].id, data.id);
  assert.equal(page.reports[0].generated_at_ms, data.generated_at_ms);
  assert.equal(page.cursor, null);
});

test('body limit applies to streams without a Content-Length', async () => {
  const env = fixture();
  const stream = new ReadableStream({start(controller) {
    controller.enqueue(new Uint8Array(140000)); controller.enqueue(new Uint8Array(140000)); controller.close();
  }});
  const result = await worker.fetch(new Request('https://inbox.example/v1/reports/' + 'a'.repeat(32), {
    method: 'PUT', headers: {Authorization: 'Bearer ' + env.UPLOAD_TOKEN, 'content-type': 'application/json'},
    body: stream, duplex: 'half',
  }), env);
  assert.equal(result.status, 413);
  assert.equal(env.REPORTS.objects.size, 0);
});

test('list pagination survives an empty filtered page', async () => {
  const env = fixture();
  for (let index = 0; index < 102; index++) {
    const data = report({id: index.toString(16).padStart(32, '0'), session: index === 101 ? 'target' : 'other'});
    assert.equal((await request(env, 'PUT', '/v1/reports/' + data.id, data)).status, 201);
  }
  const first = await (await request(env, 'GET', '/v1/reports?session=target')).json();
  assert.deepEqual(first.reports, []);
  assert.ok(first.cursor);
  const second = await (await request(env, 'GET', '/v1/reports?session=target&cursor=' + first.cursor)).json();
  assert.equal(second.reports.length, 1);
  assert.equal(second.reports[0].session, 'target');
  assert.equal(second.cursor, null);
});

test('bad JSON, unsupported media type, routes and methods return safe errors', async () => {
  const env = fixture(), path = '/v1/reports/' + 'a'.repeat(32);
  assert.equal((await worker.fetch(new Request('https://inbox.example' + path, {method: 'PUT', headers: {
    Authorization: 'Bearer ' + env.UPLOAD_TOKEN, 'content-type': 'application/json'}, body: '{'}), env)).status, 400);
  assert.equal((await worker.fetch(new Request('https://inbox.example' + path, {method: 'PUT', headers: {
    Authorization: 'Bearer ' + env.UPLOAD_TOKEN}, body: '{}'}), env)).status, 415);
  assert.equal((await request(env, 'POST', '/v1/reports')).status, 405);
  assert.equal((await request(env, 'GET', '/v1/reports/' + '../private')).status, 404);
  assert.equal((await request(env, 'GET', path)).status, 404);
});

test('missing/weak/equal secrets fail closed and storage failures do not leak details', async () => {
  const env = fixture();
  for (const extra of [{READ_TOKEN: ''}, {UPLOAD_TOKEN: env.READ_TOKEN}, {REPORTS: null}]) {
    assert.equal((await request({...env, ...extra}, 'GET', '/v1/reports')).status, 503);
  }
  env.REPORTS.list = async () => {throw Error('private-provider-credential');};
  const saved = console.error, logs = [];
  console.error = value => logs.push(value);
  try {
    const result = await request(env, 'GET', '/v1/reports');
    assert.equal(result.status, 503);
    assert.doesNotMatch(await result.text(), /private-provider/);
    assert.deepEqual(logs, ['{"event":"diagnostic_inbox_error"}']);
  } finally {console.error = saved;}
});

test('viewer shell has no private data, keys persist only in memory, and reports use textContent', async () => {
  const env = fixture();
  const shell = await request(env, 'GET', '/');
  assert.equal(shell.status, 200);
  assert.match(shell.headers.get('content-security-policy'), /script-src 'self'/);
  assert.doesNotMatch(await shell.text(), /u{32}|r{32}|<script[^>]*>[^<]/);
  const script = await (await request(env, 'GET', '/inbox.js')).text();
  assert.doesNotMatch(script, /localStorage|sessionStorage|innerHTML|document\.write|\.cookie/);
  assert.match(script, /\$\('report'\)\.textContent=data\.report/);
  assert.match(script, /key=''/);
});
