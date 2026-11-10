// Private, dependency-free report inbox. Bind REPORTS (R2), UPLOAD_TOKEN, READ_TOKEN.
const MAX_BODY = 256 * 1024;
const MAX_REPORT = 240 * 1024;
const ID = /^[0-9a-f]{32}$/;
const SUITES = new Set(['all', 'device', 'network', 'media', 'wallet', 'checklist']);
const FIELDS = ['schema_version', 'id', 'generated_at_ms', 'device', 'session', 'suite', 'app_version', 'platform', 'report'];
const encoder = new TextEncoder();
const SECURITY = {
  'cache-control': 'no-store',
  'content-security-policy': "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
  'x-content-type-options': 'nosniff',
  'referrer-policy': 'no-referrer',
  'strict-transport-security': 'max-age=31536000',
};

function response(body, status = 200, type = 'application/json; charset=utf-8') {
  return new Response(type.startsWith('application/json') ? JSON.stringify(body) : body, {
    status, headers: {...SECURITY, 'content-type': type},
  });
}
class HttpError extends Error {
  constructor(status, message) { super(message); this.status = status; }
}
async function equalSecret(provided, expected) {
  const [a, b] = await Promise.all([
    crypto.subtle.digest('SHA-256', encoder.encode(provided)),
    crypto.subtle.digest('SHA-256', encoder.encode(expected)),
  ]);
  return crypto.subtle.timingSafeEqual(a, b);
}
function textField(value, max, required = false) {
  return typeof value === 'string' && [...value].length <= max &&
    (!required || value.trim().length > 0) && !/[\u0000-\u001f\u007f]/.test(value);
}
function validateReport(data, id) {
  if (!data || typeof data !== 'object' || Array.isArray(data) ||
      Object.keys(data).length !== FIELDS.length || FIELDS.some(key => !Object.hasOwn(data, key)) ||
      data.schema_version !== 1 || data.id !== id ||
      !Number.isSafeInteger(data.generated_at_ms) || data.generated_at_ms < 0 ||
      !textField(data.device, 80, true) || !textField(data.session, 80) ||
      !SUITES.has(data.suite) || !textField(data.app_version, 40, true) ||
      !textField(data.platform, 80, true) || typeof data.report !== 'string' ||
      !data.report.trim() || encoder.encode(data.report).byteLength > MAX_REPORT) {
    throw new HttpError(400, 'Invalid diagnostic report.');
  }
  return data;
}
async function boundedJson(request) {
  if (request.headers.get('content-type')?.split(';')[0].trim() !== 'application/json') {
    throw new HttpError(415, 'Use application/json.');
  }
  const length = Number(request.headers.get('content-length'));
  if (length > MAX_BODY) throw new HttpError(413, 'Report exceeds 256 KiB.');
  if (!request.body) throw new HttpError(400, 'A report is required.');
  const reader = request.body.getReader();
  const chunks = [];
  let size = 0;
  try {
    while (true) {
      const part = await reader.read();
      if (part.done) break;
      size += part.value.byteLength;
      if (size > MAX_BODY) {
        await reader.cancel();
        throw new HttpError(413, 'Report exceeds 256 KiB.');
      }
      chunks.push(part.value);
    }
  } finally { reader.releaseLock(); }
  const body = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) { body.set(chunk, offset); offset += chunk.byteLength; }
  try { return JSON.parse(new TextDecoder('utf-8', {fatal: true}).decode(body)); }
  catch { throw new HttpError(400, 'Invalid JSON.'); }
}
function metadata(data) {
  return Object.fromEntries(['device', 'session', 'suite', 'generated_at_ms', 'app_version', 'platform']
    .map(key => [key, String(data[key])]));
}
async function handle(request, env) {
  const url = new URL(request.url);
  if (request.method === 'GET') {
    if (url.pathname === '/') return response(HTML, 200, 'text/html; charset=utf-8');
    if (url.pathname === '/inbox.js') return response(SCRIPT, 200, 'text/javascript; charset=utf-8');
    if (url.pathname === '/inbox.css') return response(STYLE, 200, 'text/css; charset=utf-8');
  }
  const match = url.pathname.match(/^\/v1\/reports\/([0-9a-f]{32})$/);
  const listing = url.pathname === '/v1/reports';
  if (!match && !listing) throw new HttpError(404, 'Not found.');
  if ((listing && request.method !== 'GET') || (match && !['GET', 'PUT'].includes(request.method))) {
    throw new HttpError(405, 'Method not allowed.');
  }
  if (typeof env.READ_TOKEN !== 'string' || env.READ_TOKEN.length < 32 ||
      typeof env.UPLOAD_TOKEN !== 'string' || env.UPLOAD_TOKEN.length < 32 ||
      await equalSecret(env.READ_TOKEN, env.UPLOAD_TOKEN) || !env.REPORTS) {
    throw new HttpError(503, 'Inbox is not configured.');
  }
  const expected = request.method === 'PUT' ? env.UPLOAD_TOKEN : env.READ_TOKEN;
  const authorization = request.headers.get('authorization') || '';
  if (!await equalSecret(authorization, 'Bearer ' + expected)) throw new HttpError(401, 'Unauthorized.');
  if (listing) {
    const cursor = url.searchParams.get('cursor') || undefined;
    const session = url.searchParams.get('session');
    if ((cursor && cursor.length > 4096) || (session !== null && !textField(session, 80))) {
      throw new HttpError(400, 'Invalid report filter.');
    }
    const page = await env.REPORTS.list({prefix: 'reports/', limit: 100, cursor, include: ['customMetadata']});
    const reports = page.objects.filter(object => /^reports\/[0-9a-f]{32}\.json$/.test(object.key))
      .filter(object => session === null || object.customMetadata?.session === session)
      .map(object => ({
        id: object.key.slice(8, -5), ...object.customMetadata,
        generated_at_ms: Number(object.customMetadata?.generated_at_ms),
        uploaded_at: object.uploaded.toISOString(),
      }));
    return response({reports, cursor: page.truncated ? page.cursor : null});
  }
  const id = match[1], key = 'reports/' + id + '.json';
  if (request.method === 'GET') {
    const object = await env.REPORTS.get(key);
    if (!object) throw new HttpError(404, 'Report not found.');
    return new Response(object.body, {headers: {...SECURITY, 'content-type': 'application/json; charset=utf-8'}});
  }
  const data = validateReport(await boundedJson(request), id);
  const serialized = JSON.stringify(data);
  // Atomic create avoids overwrites, including two simultaneous retries.
  const stored = await env.REPORTS.put(key, serialized, {
    onlyIf: new Headers({'If-None-Match': '*'}),
    httpMetadata: {contentType: 'application/json; charset=utf-8', cacheControl: 'no-store'},
    customMetadata: metadata(data),
  });
  if (stored === null) {
    const existing = await env.REPORTS.get(key);
    if (!existing || await existing.text() !== serialized) throw new HttpError(409, 'Report ID already exists.');
  }
  return response({id}, stored === null ? 200 : 201);
}
export default {
  async fetch(request, env) {
    try { return await handle(request, env); }
    catch (error) {
      if (error instanceof HttpError) return response({error: error.message}, error.status);
      // Do not log request bodies, Authorization, provider errors, or private report metadata.
      console.error(JSON.stringify({event: 'diagnostic_inbox_error'}));
      return response({error: 'Inbox storage is temporarily unavailable.'}, 503);
    }
  },
};

const HTML = `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>ClipClub diagnostic inbox</title><link rel="stylesheet" href="/inbox.css"><script src="/inbox.js" defer></script></head><body><main><header><span>ClipClub</span><h1>Diagnostic inbox</h1><p>Reports from your test computers, together in one private place.</p></header><form id="login"><label for="read-token">Read key</label><div class="row"><input id="read-token" type="password" autocomplete="off" required><button>Open inbox</button></div><p class="hint">The key stays in this tab’s memory. Closing or reloading the tab locks the inbox.</p></form><section id="inbox" hidden><div class="row"><form id="filter"><label for="session">Test session</label><div class="row"><input id="session" maxlength="80" placeholder="All sessions"><button>Refresh</button></div></form><button id="lock" type="button">Lock inbox</button></div><p id="count"></p><ol id="reports"></ol><button id="more" type="button" hidden>Load more</button><article id="detail" hidden><h2 id="detail-title"></h2><pre id="report"></pre></article></section><p id="status" role="status" aria-live="polite"></p></main></body></html>`;
const STYLE = `:root{color-scheme:dark;font:16px/1.5 system-ui,sans-serif;background:#111318;color:#edf0f6}*{box-sizing:border-box}body{margin:0}main{max-width:960px;margin:0 auto;padding:48px 24px}header span{color:#a9b8ff;font-weight:700}h1{font-size:36px;margin:12px 0}p{color:#b6bdce}form,section{margin-top:24px}label{display:block;margin-bottom:6px}input,button{font:inherit;border:1px solid #48516a;border-radius:8px;padding:10px 14px;background:#1b202b;color:inherit}input{min-width:0;flex:1}button{cursor:pointer}button:hover{border-color:#a9b8ff}button:disabled{opacity:.6;cursor:wait}.row{display:flex;gap:12px;align-items:end}.row form{flex:1;margin:0}.hint{font-size:13px}ol{list-style:none;padding:0}li{margin:10px 0}li button{display:block;width:100%;text-align:left}li small{display:block;color:#b6bdce;margin-top:4px}pre{white-space:pre-wrap;overflow-wrap:anywhere;background:#1b202b;padding:20px;border-radius:12px;font:13px/1.6 ui-monospace,monospace}[hidden]{display:none!important}#status{color:#c6d0ff}@media(max-width:560px){main{padding:28px 16px}.row{flex-wrap:wrap}.row input{width:100%}}`;
const SCRIPT = String.raw`'use strict';
const $=id=>document.getElementById(id);
let key='',cursor=null,loading=false,total=0,version=0;
function status(message){$('status').textContent=message;}
async function api(path){const r=await fetch(path,{headers:{Authorization:'Bearer '+key},cache:'no-store',redirect:'error'});if(!r.ok)throw Error(r.status===401?'The read key was rejected.':'The inbox could not load reports. Try again.');return r.json();}
function lock(){version++;key='';cursor=null;total=0;$('read-token').value='';$('reports').replaceChildren();$('report').textContent='';$('detail').hidden=true;$('inbox').hidden=true;$('login').hidden=false;status('Inbox locked.');}
async function load(reset){if(loading)return;loading=true;const turn=version;const filter=$('session').value.trim();try{if(reset){cursor=null;total=0;$('reports').replaceChildren();$('detail').hidden=true;}const params=new URLSearchParams();if(filter)params.set('session',filter);if(cursor)params.set('cursor',cursor);status('Loading reports…');const page=await api('/v1/reports?'+params);if(turn!==version)return;for(const item of page.reports){const li=document.createElement('li'),button=document.createElement('button'),small=document.createElement('small');button.type='button';button.textContent=item.device+' · '+item.suite;small.textContent=new Date(item.generated_at_ms).toLocaleString()+' · '+(item.session||'No test session')+' · '+item.app_version;button.append(small);button.addEventListener('click',async()=>{const selected=version;try{status('Opening report…');const data=await api('/v1/reports/'+encodeURIComponent(item.id));if(selected!==version)return;$('detail-title').textContent=data.device+' · '+data.suite;$('report').textContent=data.report;$('detail').hidden=false;status('Report opened.');}catch(e){if(selected===version)status(e.message);}});li.append(button);$('reports').append(li);total++;}cursor=page.cursor;$('more').hidden=!cursor;$('count').textContent=total+' report'+(total===1?'':'s')+' loaded';$('inbox').hidden=false;$('login').hidden=true;status(cursor?'More reports are available.':'All matching reports loaded.');}catch(e){if(turn===version)status(e.message);}finally{loading=false;}}
$('login').addEventListener('submit',event=>{event.preventDefault();key=$('read-token').value;$('read-token').value='';version++;load(true);});
$('filter').addEventListener('submit',event=>{event.preventDefault();load(true);});
$('more').addEventListener('click',()=>load(false));$('lock').addEventListener('click',lock);
`;
