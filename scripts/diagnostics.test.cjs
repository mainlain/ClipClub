'use strict';
const assert=require('node:assert/strict');
const fs=require('node:fs');
const vm=require('node:vm');
const {test}=require('node:test');
const html=fs.readFileSync('ui/index.html','utf8');
const source=fs.readFileSync('ui/app.js','utf8');

function fixture({storage=new Map(),storageBlocked=false,codecSupport={mp4:'probably',webm:'maybe'}}={}){
  const calls=[],timers=new Map(),downloads=[],urls=[],copied=[];
  let timerId=0;
  const makeElement=id=>({
    id,innerHTML:'',textContent:'',value:id==='diagnostics-p2p-port'?'42931':id==='diagnostics-torrent-port'?'42932':id==='diagnostics-network-condition'?'unspecified':'',hidden:false,listeners:{},
    addEventListener(name,listener){this.listeners[name]=listener;},
    setAttribute(name,value){this[name]=value;},focus(){this.focused=true;},select(){this.selected=true;},
  });
  const elements=new Map([...html.matchAll(/\bid="([^"]+)"/g)].map(([,id])=>[id,makeElement(id)]));
  const suiteButtons=new Map([...html.matchAll(/data-diagnostic-suite="([^"]+)"/g)].map(([,suite])=>[suite,{...makeElement('suite-'+suite),dataset:{diagnosticSuite:suite}}]));
  elements.get('player').canPlayType=mime=>mime.includes('video/mp4')?codecSupport.mp4:codecSupport.webm;
  const getElement=id=>{
    if(!elements.has(id)&&/^diagnostic-step-/.test(id)&&elements.get('diagnostics-checklist').innerHTML.includes(`id="${id}"`))elements.set(id,makeElement(id));
    assert.ok(elements.has(id),id);return elements.get(id);
  };
  const context=vm.createContext({
    document:{
      body:{appendChild(link){downloads.push(link);}},
      getElementById:getElement,
      querySelector(){return{content:'local-token'};},querySelectorAll(selector){return selector==='[data-diagnostic-suite]'?[...suiteButtons.values()]:[];},addEventListener(){},
      createElement(tag){assert.equal(tag,'a');return{click(){this.clicked=true;},remove(){this.removed=true;}};},
    },
    window:{innerWidth:900,innerHeight:600,devicePixelRatio:2,addEventListener(){},localStorage:{
      getItem(key){if(storageBlocked)throw Error('Storage blocked');return storage.get(key)||null;},
      setItem(key,value){if(storageBlocked)throw Error('Storage blocked');storage.set(key,value);},
    }},navigator:{userAgent:'Fixture Webview/1.0',clipboard:{async writeText(text){copied.push(text);}}},
    Blob,AbortController,
    URL:{createObjectURL(blob){urls.push(blob);return'blob:diagnostics';},revokeObjectURL(url){urls.push(url);}},
    fetch(url,options){calls.push({url,options});return new Promise(()=>{});},
    FormData:class{},setTimeout(fn,ms){const id=++timerId;timers.set(id,{fn,ms});return id;},clearTimeout(id){timers.delete(id);},setInterval(){},
  });
  vm.runInContext(source,context);
  context.fixtureState={p2p_port:43101,torrent_port:43102};
  vm.runInContext('state=fixtureState;renderDiagnostics();',context);
  const report={suite:'all',generated_at_ms:1794312000000,summary:'One video port needs attention.',checks:[
    {category:'network',name:'Peer connection',status:'pass',detail:'TCP reachable; protocol not checked.'},
    {category:'network',name:'Video transfer',status:'fail',detail:'Connection timed out.'},
    {category:'network',name:'Discovery <script>',status:'warning',detail:'No observations <img onerror="x">.'},
  ],report:'ClipClub diagnostics\nPeer: 192.168.1.86\nVideo: timed out\n'};
  return{context,elements,calls,timers,downloads,urls,copied,report,suiteButtons,storage,
    run(code){return vm.runInContext(code,context);},
    respond(result=report){context.fetch=async(url,options)=>{calls.push({url,options});return{ok:true,json:async()=>JSON.parse(JSON.stringify(result))};};},
    select(suite){const button=suiteButtons.get(suite);assert.ok(button,suite);elements.get('diagnostics-menu').listeners.click({target:{closest(selector){assert.equal(selector,'[data-diagnostic-suite]');return button;}}});},
    input(id,value,event='input'){const element=getElement(id);element.value=value;element.listeners[event]();},
    step(id,value){const select=getElement('diagnostic-step-'+id);select.value=value;select.dataset={diagnosticStep:id};select.closest=selector=>{assert.equal(selector,'[data-diagnostic-step]');return select;};elements.get('diagnostics-checklist').listeners.change({target:select});},
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
  assert.deepEqual(JSON.parse(request.options.body),{suite:'all',peer_ip:'192.168.1.86',p2p_port:42931,torrent_port:42932});
  assert.equal(app.elements.get('diagnostics-results').hidden,false);
  const exported=app.elements.get('diagnostics-report').value;
  assert.ok(exported.includes(app.report.report));
  assert.match(exported,/Webview observations:\n/);
  assert.match(exported,/Fixture Webview\/1\.0.*900 × 600 CSS pixels; pixel ratio 2/);
  assert.match(exported,/Hands-on device checklist \(manually recorded\)/);
  assert.match(app.elements.get('diagnostics-checks').innerHTML,/&lt;script&gt;/);
  assert.doesNotMatch(app.elements.get('diagnostics-checks').innerHTML,/<script>|<img/);
  app.run('state={p2p_port:50000,torrent_port:50001};page="home";renderDiagnostics();page="diagnostics";renderDiagnostics();');
  assert.equal(app.elements.get('diagnostics-p2p-port').value,'42931');
  assert.equal(app.elements.get('diagnostics-report').value,exported);
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
  assert.ok([...app.suiteButtons.values()].every(button=>button.disabled));
  app.select('device');assert.equal(app.run('diagnosticsSuite'),'all');
  await app.run('runDiagnostics()');
  assert.equal(app.calls.filter(call=>call.url==='/api/diagnostics').length,1);
  reject(new Error('Local client disconnected.'));await pending;
  assert.equal(app.elements.get('run-diagnostics').disabled,false);
  assert.equal(app.elements.get('diagnostics-error').hidden,false);
  assert.match(app.elements.get('diagnostics-error').textContent,/disconnected/);
  assert.equal(app.elements.get('diagnostics-results').hidden,true);
  assert.ok([...app.suiteButtons.values()].every(button=>!button.disabled));
  assert.equal([...app.timers.values()].filter(timer=>timer.ms===40000).length,0);
});

test('failed reruns retain the prior report and malformed input never sends a probe',async()=>{
  const app=fixture();app.respond();await app.run('runDiagnostics()');
  const exported=app.elements.get('diagnostics-report').value;
  app.context.fetch=async()=>({ok:false,json:async()=>({error:'Diagnostics are already running.'})});
  await app.run('runDiagnostics()');
  assert.equal(app.elements.get('diagnostics-report').value,exported);
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
  const exported=app.elements.get('diagnostics-report').value;
  app.context.fetch=(_url,options)=>new Promise((_,reject)=>options.signal.addEventListener('abort',()=>{
    const error=new Error('Aborted');error.name='AbortError';reject(error);
  }));
  const pending=app.run('runDiagnostics()');
  const timeout=[...app.timers.values()].find(timer=>timer.ms===40000);assert.ok(timeout);timeout.fn();await pending;
  assert.equal(app.elements.get('run-diagnostics').disabled,false);
  assert.match(app.elements.get('diagnostics-error').textContent,/took too long/);
  assert.equal(app.elements.get('diagnostics-report').value,exported);
});

test('copy and save export exactly the report, with a selection fallback for native webviews',async()=>{
  const app=fixture();app.respond();await app.run('runDiagnostics()');
  const exported=app.elements.get('diagnostics-report').value;
  await app.elements.get('copy-diagnostics').listeners.click();
  assert.deepEqual(app.copied,[exported]);
  app.context.navigator.clipboard=undefined;
  await app.elements.get('copy-diagnostics').listeners.click();
  assert.equal(app.elements.get('diagnostics-report').focused,true);
  assert.equal(app.elements.get('diagnostics-report').selected,true);
  app.elements.get('save-diagnostics').listeners.click();
  assert.equal(app.downloads.length,1);
  assert.match(app.downloads[0].download,/^clipclub-diagnostics-.*\.txt$/);
  assert.equal(app.downloads[0].clicked,true);assert.equal(app.downloads[0].removed,true);
  assert.equal(await app.urls[0].text(),exported);
  assert.equal(app.urls[0].type,'text/plain;charset=utf-8');
  const cleanup=[...app.timers.values()].find(timer=>timer.ms===1000);assert.ok(cleanup);cleanup.fn();
  assert.equal(app.urls.at(-1),'blob:diagnostics');
});

test('blank target checks known peers without an explicit address',async()=>{
  const app=fixture();app.respond();await app.run('runDiagnostics()');
  assert.deepEqual(JSON.parse(app.calls.at(-1).options.body),{suite:'all',p2p_port:43101,torrent_port:43102});
});

test('submenu selection is idle until Run and preserves a separate report for each suite',async()=>{
  const app=fixture();app.respond();await app.run('runDiagnostics()');
  const baseline=app.elements.get('diagnostics-report').value,count=app.calls.length;
  app.select('device');
  assert.equal(app.calls.length,count);
  assert.equal(app.elements.get('diagnostics-results').hidden,true);
  assert.equal(app.elements.get('diagnostics-network-target').hidden,true);
  assert.equal(app.suiteButtons.get('device')['aria-pressed'],'true');
  assert.equal(app.suiteButtons.get('all')['aria-pressed'],'false');
  app.respond({suite:'device',generated_at_ms:1794312000001,summary:'Device checked.',checks:[{category:'device',name:'Storage access',status:'pass',detail:'Round trip succeeded.'}],report:'Device storage report\n'});
  await app.run('runDiagnostics()');
  assert.deepEqual(JSON.parse(app.calls.at(-1).options.body),{suite:'device'});
  const device=app.elements.get('diagnostics-report').value;
  assert.match(device,/Device storage report/);
  assert.doesNotMatch(device,/Peer: 192/);
  const withoutExportTime=text=>text.replace(/^Exported:.*$/m,'');
  app.select('all');assert.equal(withoutExportTime(app.elements.get('diagnostics-report').value),withoutExportTime(baseline));
  app.select('device');assert.equal(withoutExportTime(app.elements.get('diagnostics-report').value),withoutExportTime(device));
  assert.equal(app.calls.length,count+1);
});

test('each automated report keeps the device and network captured when its request began',async()=>{
  const app=fixture();
  app.input('diagnostics-device-label','Laptop A · baseline');
  app.input('diagnostics-network-condition','same-wifi','change');
  let finish;
  app.context.fetch=(url,options)=>{app.calls.push({url,options});return new Promise(resolve=>finish=()=>resolve({ok:true,json:async()=>JSON.parse(JSON.stringify(app.report))}));};
  const pending=app.run('runDiagnostics()');
  app.input('diagnostics-device-label','Laptop A · hotspot');
  app.input('diagnostics-network-condition','hotspot','change');
  finish();await pending;
  let exported=app.elements.get('diagnostics-report').value;
  assert.match(exported,/^Test device: Laptop A · baseline\nTest network: Same Wi-Fi\n/);
  assert.match(exported,/Checklist device: Laptop A · hotspot\nChecklist network: Phone hotspot/);
  app.select('network');app.respond({...app.report,suite:'network',report:'Hotspot network report\n'});await app.run('runDiagnostics()');
  exported=app.elements.get('diagnostics-report').value;
  assert.match(exported,/^Test device: Laptop A · hotspot\nTest network: Phone hotspot\n/);
  app.select('all');exported=app.elements.get('diagnostics-report').value;
  assert.match(exported,/^Test device: Laptop A · baseline\nTest network: Same Wi-Fi\n/);
  assert.match(exported,/Checklist device: Laptop A · hotspot\nChecklist network: Phone hotspot/);
  assert.doesNotMatch(exported,/Hotspot network report/);
  app.select('checklist');exported=app.elements.get('diagnostics-report').value;
  assert.match(exported,/^Test device: Laptop A · hotspot\nTest network: Phone hotspot\n/);
});

test('focused non-network suites ignore invalid hidden peer inputs',async()=>{
  const app=fixture();
  app.elements.get('diagnostics-peer').value='not an address';
  app.elements.get('diagnostics-p2p-port').value='0';
  app.elements.get('diagnostics-torrent-port').value='invalid';
  for(const suite of ['device','media','wallet']){
    app.select(suite);app.respond({...app.report,suite});await app.run('runDiagnostics()');
    assert.deepEqual(JSON.parse(app.calls.at(-1).options.body),{suite});
    assert.equal(app.elements.get('diagnostics-error').hidden,true);
    assert.equal(app.elements.get('diagnostics-peer').disabled,true);
  }
  const count=app.calls.length;app.select('network');await app.run('runDiagnostics()');
  assert.equal(app.calls.length,count);assert.match(app.elements.get('diagnostics-error').textContent,/IPv4/);
});

test('media capability observations distinguish reported support and a decode error from playback success',async()=>{
  const app=fixture({codecSupport:{mp4:'probably',webm:''}});
  app.elements.get('player').error={code:3};
  app.select('media');app.respond({...app.report,suite:'media'});await app.run('runDiagnostics()');
  const checks=JSON.parse(app.run('JSON.stringify(diagnosticsResult.checks)'));
  const mp4=checks.find(check=>check.name==='Playback capability · MP4 · H.264 + AAC');
  const webm=checks.find(check=>check.name==='Playback capability · WebM · VP9 + Opus');
  assert.equal(mp4.status,'info');assert.match(mp4.detail,/does not test decoding/);
  assert.equal(webm.status,'warning');assert.match(webm.detail,/does not advertise support/);
  assert.equal(checks.find(check=>check.name==='Current rating player').status,'warning');
  assert.match(app.elements.get('diagnostics-status').textContent,/capability warnings/);
  assert.match(app.elements.get('diagnostics-report').value,/decode error/);
});

test('reruns append one set of webview observations without changing the raw server fixture',async()=>{
  const app=fixture(),raw=JSON.stringify(app.report);
  app.respond();await app.run('runDiagnostics()');await app.run('runDiagnostics()');
  assert.equal(JSON.stringify(app.report),raw);
  assert.equal((app.elements.get('diagnostics-report').value.match(/Webview observations:/g)||[]).length,1);
  assert.equal(app.run('diagnosticsResult.checks.filter(check=>check.name==="Webview & display").length'),1);
});

test('manual checklist exports labeled results and notes without automated checks, and survives same-origin reload',async()=>{
  const app=fixture();app.select('checklist');
  assert.equal(app.calls.filter(call=>call.url==='/api/diagnostics').length,0);
  assert.equal(app.elements.get('diagnostics-form').hidden,true);
  assert.equal(app.elements.get('diagnostics-manual').hidden,false);
  assert.equal(app.elements.get('diagnostics-results').hidden,false);
  app.input('diagnostics-device-label','Laptop A · macOS');
  app.input('diagnostics-network-condition','hotspot','change');
  app.input('diagnostics-notes','WebM has no sound after waking.');
  app.step('discovery','pass');app.step('playback','fail');app.step('wallet','skip');
  const exported=app.elements.get('diagnostics-report').value;
  assert.match(exported,/Test device: Laptop A · macOS\nTest network: Phone hotspot/);
  assert.match(exported,/No automated checks are included/);
  assert.match(exported,/\[PASS\] Discover each other/);
  assert.match(exported,/\[FAIL\] Picture, sound & seeking/);
  assert.match(exported,/\[SKIPPED\] Wallet unlock & cancellation/);
  assert.match(exported,/WebM has no sound after waking/);
  assert.match(app.elements.get('diagnostics-checklist-progress').textContent,/2 \/ \d+ tested · 1 failed · 1 skipped/);
  assert.ok(app.elements.get('diagnostic-step-playback').focused);
  await app.elements.get('copy-diagnostics').listeners.click();assert.equal(app.copied.at(-1),exported);
  app.elements.get('save-diagnostics').listeners.click();assert.equal(await app.urls[0].text(),exported);
  assert.match(app.downloads[0].download,/^clipclub-diagnostics-checklist-/);
  const restored=fixture({storage:app.storage});restored.select('checklist');
  assert.equal(restored.elements.get('diagnostics-device-label').value,'Laptop A · macOS');
  assert.equal(restored.elements.get('diagnostics-network-condition').value,'hotspot');
  assert.equal(restored.elements.get('diagnostics-notes').value,'WebM has no sound after waking.');
  assert.deepEqual(JSON.parse(restored.run('JSON.stringify(diagnosticsChecklistResults)')),{discovery:'pass',playback:'fail',wallet:'skip'});
  assert.match(restored.elements.get('diagnostics-report').value,/\[FAIL\] Picture, sound & seeking/);
  restored.step('playback','');
  assert.match(restored.elements.get('diagnostics-report').value,/\[NOT TESTED\] Picture, sound & seeking/);
  assert.equal(JSON.parse(restored.storage.get('clipclub-diagnostic-notes')).steps.playback,undefined);
});

test('blocked local storage keeps checklist usable and clearly offers export',async()=>{
  const app=fixture({storageBlocked:true});app.select('checklist');
  app.input('diagnostics-notes','Observed while storage is blocked.');app.step('resume','fail');
  assert.match(app.elements.get('diagnostics-notes-status').textContent,/Local saving is unavailable/);
  const exported=app.elements.get('diagnostics-report').value;
  assert.match(exported,/Observed while storage is blocked/);assert.match(exported,/\[FAIL\] Sleep, wake & reconnect/);
  await app.elements.get('copy-diagnostics').listeners.click();assert.equal(app.copied.at(-1),exported);
  assert.equal(app.calls.filter(call=>call.url==='/api/diagnostics').length,0);
});
