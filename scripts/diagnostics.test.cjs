'use strict';
const assert=require('node:assert/strict');
const fs=require('node:fs');
const vm=require('node:vm');
const {test}=require('node:test');
const html=fs.readFileSync('ui/index.html','utf8');
const source=fs.readFileSync('ui/app.js','utf8');

function fixture(){
  const calls=[],timers=new Map(),downloads=[],urls=[],copied=[];
  let timerId=0;
  const elements=new Map([...html.matchAll(/\bid="([^"]+)"/g)].map(([,id])=>[id,{
    id,innerHTML:'',textContent:'',value:id==='diagnostics-p2p-port'?'42931':id==='diagnostics-torrent-port'?'42932':'',hidden:false,listeners:{},
    addEventListener(name,listener){this.listeners[name]=listener;},
    setAttribute(name,value){this[name]=value;},focus(){this.focused=true;},select(){this.selected=true;},
  }]));
  const context=vm.createContext({
    document:{
      body:{appendChild(link){downloads.push(link);}},
      getElementById(id){assert.ok(elements.has(id),id);return elements.get(id);},
      querySelector(){return{content:'local-token'};},querySelectorAll(){return[];},addEventListener(){},
      createElement(tag){assert.equal(tag,'a');return{click(){this.clicked=true;},remove(){this.removed=true;}};},
    },
    window:{addEventListener(){}},navigator:{clipboard:{async writeText(text){copied.push(text);}}},
    Blob,AbortController,
    URL:{createObjectURL(blob){urls.push(blob);return'blob:diagnostics';},revokeObjectURL(url){urls.push(url);}},
    fetch(url,options){calls.push({url,options});return new Promise(()=>{});},
    FormData:class{},setTimeout(fn,ms){const id=++timerId;timers.set(id,{fn,ms});return id;},clearTimeout(id){timers.delete(id);},setInterval(){},
  });
  vm.runInContext(source,context);
  context.fixtureState={p2p_port:43101,torrent_port:43102};
  vm.runInContext('state=fixtureState;renderDiagnostics();',context);
  const report={generated_at_ms:1794312000000,summary:'One video port needs attention.',checks:[
    {name:'Peer connection',status:'pass',detail:'TCP reachable; protocol not checked.'},
    {name:'Video transfer',status:'fail',detail:'Connection timed out.'},
    {name:'Discovery <script>',status:'warning',detail:'No observations <img onerror="x">.'},
  ],report:'ClipClub diagnostics\nPeer: 192.168.1.86\nVideo: timed out\n'};
  return{context,elements,calls,timers,downloads,urls,copied,report,
    run(code){return vm.runInContext(code,context);},
    respond(result=report){context.fetch=async(url,options)=>{calls.push({url,options});return{ok:true,json:async()=>result};};},
  };
}

test('diagnostics starts only on request and preserves edited ports and reports across polls',async()=>{
  const app=fixture();
  assert.deepEqual(app.calls.map(call=>call.url),['/api/state']);
  assert.equal(app.elements.get('diagnostics-p2p-port').value,'43101');
  assert.equal(app.elements.get('diagnostics-torrent-port').value,'43102');
  app.elements.get('diagnostics-peer').value=' 192.168.1.086 ';
  app.elements.get('diagnostics-p2p-port').value='42931';
  app.elements.get('diagnostics-torrent-port').value='42932';
  app.respond();await app.run('runDiagnostics()');
  const request=app.calls.at(-1);
  assert.equal(request.url,'/api/diagnostics');
  assert.equal(request.options.method,'POST');
  assert.equal(request.options.headers['x-clipcrop-token'],'local-token');
  assert.deepEqual(JSON.parse(request.options.body),{peer_ip:'192.168.1.86',p2p_port:42931,torrent_port:42932});
  assert.equal(app.elements.get('diagnostics-results').hidden,false);
  assert.equal(app.elements.get('diagnostics-report').value,app.report.report);
  assert.match(app.elements.get('diagnostics-checks').innerHTML,/&lt;script&gt;/);
  assert.doesNotMatch(app.elements.get('diagnostics-checks').innerHTML,/<script>|<img/);
  app.run('state={p2p_port:50000,torrent_port:50001};page="home";renderDiagnostics();page="diagnostics";renderDiagnostics();');
  assert.equal(app.elements.get('diagnostics-p2p-port').value,'42931');
  assert.equal(app.elements.get('diagnostics-report').value,app.report.report);
  assert.equal(app.calls.filter(call=>call.url==='/api/diagnostics').length,1);
});

test('a port edit before the initial state arrives survives default initialization',()=>{
  const app=fixture();
  app.run('diagnosticsPortsReady=false;');
  app.elements.get('diagnostics-p2p-port').value='45000';
  app.elements.get('diagnostics-p2p-port').listeners.input();
  app.run('renderDiagnostics();');
  assert.equal(app.elements.get('diagnostics-p2p-port').value,'45000');
  assert.equal(app.elements.get('diagnostics-torrent-port').value,'43102');
});

test('an in-flight test blocks duplicate submissions and restores controls on failure',async()=>{
  const app=fixture();let reject;
  app.context.fetch=(url,options)=>{app.calls.push({url,options});return new Promise((_,fail)=>reject=fail);};
  const pending=app.run('runDiagnostics()');
  assert.equal(app.elements.get('run-diagnostics').disabled,true);
  assert.equal(app.elements.get('diagnostics-peer').disabled,true);
  await app.run('runDiagnostics()');
  assert.equal(app.calls.filter(call=>call.url==='/api/diagnostics').length,1);
  reject(new Error('Local client disconnected.'));await pending;
  assert.equal(app.elements.get('run-diagnostics').disabled,false);
  assert.equal(app.elements.get('diagnostics-error').hidden,false);
  assert.match(app.elements.get('diagnostics-error').textContent,/disconnected/);
  assert.equal(app.elements.get('diagnostics-results').hidden,true);
  assert.equal([...app.timers.values()].filter(timer=>timer.ms===25000).length,0);
});

test('failed reruns retain the prior report and malformed input never sends a probe',async()=>{
  const app=fixture();app.respond();await app.run('runDiagnostics()');
  app.context.fetch=async()=>({ok:false,json:async()=>({error:'Diagnostics are already running.'})});
  await app.run('runDiagnostics()');
  assert.equal(app.elements.get('diagnostics-report').value,app.report.report);
  assert.match(app.elements.get('diagnostics-status').textContent,/previous report/);
  const count=app.calls.length;
  for(const input of ['192.168.1.999','example.com','192.168.1']){
    app.elements.get('diagnostics-peer').value=input;await app.run('runDiagnostics()');
    assert.match(app.elements.get('diagnostics-error').textContent,/IPv4/);
  }
  app.elements.get('diagnostics-peer').value='';
  for(const port of ['0','65536','12.5','']){
    app.elements.get('diagnostics-p2p-port').value=port;await app.run('runDiagnostics()');
    assert.match(app.elements.get('diagnostics-error').textContent,/between 1 and 65535/);
  }
  assert.equal(app.calls.length,count);
});

test('a stalled request aborts and can be retried without losing the previous report',async()=>{
  const app=fixture();app.respond();await app.run('runDiagnostics()');
  app.context.fetch=(_url,options)=>new Promise((_,reject)=>options.signal.addEventListener('abort',()=>{
    const error=new Error('Aborted');error.name='AbortError';reject(error);
  }));
  const pending=app.run('runDiagnostics()');
  const timeout=[...app.timers.values()].find(timer=>timer.ms===25000);assert.ok(timeout);timeout.fn();await pending;
  assert.equal(app.elements.get('run-diagnostics').disabled,false);
  assert.match(app.elements.get('diagnostics-error').textContent,/took too long/);
  assert.equal(app.elements.get('diagnostics-report').value,app.report.report);
});

test('copy and save export exactly the report, with a selection fallback for native webviews',async()=>{
  const app=fixture();app.respond();await app.run('runDiagnostics()');
  await app.elements.get('copy-diagnostics').listeners.click();
  assert.deepEqual(app.copied,[app.report.report]);
  app.context.navigator.clipboard=undefined;
  await app.elements.get('copy-diagnostics').listeners.click();
  assert.equal(app.elements.get('diagnostics-report').focused,true);
  assert.equal(app.elements.get('diagnostics-report').selected,true);
  app.elements.get('save-diagnostics').listeners.click();
  assert.equal(app.downloads.length,1);
  assert.match(app.downloads[0].download,/^clipclub-diagnostics-.*\.txt$/);
  assert.equal(app.downloads[0].clicked,true);assert.equal(app.downloads[0].removed,true);
  assert.equal(await app.urls[0].text(),app.report.report);
  assert.equal(app.urls[0].type,'text/plain;charset=utf-8');
  const cleanup=[...app.timers.values()].find(timer=>timer.ms===1000);assert.ok(cleanup);cleanup.fn();
  assert.equal(app.urls.at(-1),'blob:diagnostics');
});

test('blank target checks known peers without an explicit address',async()=>{
  const app=fixture();app.respond();await app.run('runDiagnostics()');
  assert.deepEqual(JSON.parse(app.calls.at(-1).options.body),{p2p_port:43101,torrent_port:43102});
});
