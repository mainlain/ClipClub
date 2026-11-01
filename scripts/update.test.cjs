'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const {test} = require('node:test');

const root = path.resolve(__dirname, '..');
const html = fs.readFileSync(path.join(root, 'ui/index.html'), 'utf8');
const source = fs.readFileSync(path.join(root, 'ui/app.js'), 'utf8');
const available = {
  current_version:'0.1.0', latest_version:'0.2.0', available:true, can_install:true,
  phase:'idle', status:'available', error:null,
  download_url:'https://github.com/mainlain/ClipClub/releases/download/v0.2.0/clipclub.zip',
  release_url:'https://github.com/mainlain/ClipClub/releases/tag/v0.2.0',
};
function interfaceFixture() {
  const elements = new Map([...html.matchAll(/\bid="([^"]+)"/g)].map(([,id]) => [id, {
    innerHTML:'', textContent:'', value:id==='library-sort'?'recency':'', hidden:false,
    listeners:{}, addEventListener(name,listener){this.listeners[name]=listener;},
    classList:{toggle(){},add(){},remove(){}},
  }]));
  const context = vm.createContext({
    document:{
      getElementById(id){assert.ok(elements.has(id),`Unknown element ${id}`);return elements.get(id);},
      querySelector(selector){return selector.startsWith('meta')?{content:'test-token'}:{addEventListener(){}};},
      querySelectorAll(){return [];},addEventListener(){},
    },
    window:{addEventListener(){}},
    // Startup stays pending; the backend supplies automatic checks via state.update.
    fetch(){return new Promise(()=>{});},
    FormData:class FormData {},setTimeout(){},clearTimeout(){},setInterval(){},
  });
  vm.runInContext(source,context);
  return {
    context,elements,
    run(code){return vm.runInContext(code,context);},
    render(update){context.updateFixture=update;vm.runInContext('renderUpdates(updateFixture);',context);},
  };
}

test('a new release shows the installed version and installation actions; polls announce it once',()=>{
  const app=interfaceFixture();let announcements=0;
  app.context.announce=()=>announcements++;
  app.run('toast=announce;');
  app.render(available);
  assert.equal(app.elements.get('app-version').textContent,'0.1.0');
  assert.equal(app.elements.get('about-version').textContent,'0.1.0 · experimental alpha');
  assert.equal(app.elements.get('update-status').textContent,'Version 0.2.0 is available.');
  assert.equal(app.elements.get('install-update').hidden,false);
  assert.equal(app.elements.get('install-update').disabled,false);
  assert.equal(app.elements.get('download-update').hidden,false);
  assert.equal(app.elements.get('update-release').hidden,false);
  assert.match(app.elements.get('update-install-help').textContent,/restarts clipclub/);
  app.render(available);app.render(available);
  assert.equal(announcements,1);
  app.render({...available,latest_version:'0.3.0'});
  assert.equal(announcements,2);
});

test('managed installs offer manual downloads and browser actions send only the approved target',async()=>{
  const app=interfaceFixture(),calls=[];
  app.render({...available,can_install:false});
  assert.equal(app.elements.get('install-update').hidden,true);
  assert.equal(app.elements.get('download-update').hidden,false);
  assert.match(app.elements.get('update-install-help').textContent,/replace the installed app/);
  app.context.fetch=async(url,options)=>{calls.push({url,options});return {ok:true,json:async()=>({ok:true})};};
  await app.elements.get('download-update').listeners.click();
  assert.equal(calls[0].url,'/api/update/open');
  assert.deepEqual(JSON.parse(calls[0].options.body),{target:'download'});
  assert.equal(calls[0].options.headers['x-clipcrop-token'],'test-token');
  await app.elements.get('update-release').listeners.click();
  assert.deepEqual(JSON.parse(calls[1].options.body),{target:'release'});
  await app.run('installUpdate()');
  assert.equal(calls.length,2,'managed installs cannot start in-app installation');
});

test('manual checks submit once and recover from an offline response',async()=>{
  const app=interfaceFixture();let rejectCheck,requests=0;
  app.render({...available,available:false,status:'current'});
  app.context.fetch=()=>{requests++;return new Promise((resolve,reject)=>{rejectCheck=reject;});};
  const pending=app.run('checkUpdates()');
  await app.run('checkUpdates()');
  assert.equal(requests,1);
  assert.equal(app.elements.get('check-updates').disabled,true);
  assert.equal(app.elements.get('check-updates').textContent,'Checking…');
  rejectCheck(Error('Offline'));
  await pending;
  assert.equal(app.elements.get('check-updates').disabled,false);
  assert.match(app.elements.get('update-status').textContent,/Could not check for updates\. Offline/);
  app.context.fetch=async()=>({ok:true,json:async()=>available});
  await app.run('checkUpdates()');
  assert.equal(app.elements.get('install-update').hidden,false);
  assert.equal(app.elements.get('update-status').textContent,'Version 0.2.0 is available.');
});

test('install requests submit once, stay disabled during download, and recover after a failure',async()=>{
  const app=interfaceFixture(),calls=[];let rejectInstall;
  app.render(available);
  app.context.fetch=(url,options)=>{calls.push({url,options});return new Promise((resolve,reject)=>{rejectInstall=reject;});};
  const pending=app.run('installUpdate()');
  await app.run('installUpdate()');
  await app.run('checkUpdates()');
  assert.equal(calls.length,1);
  assert.equal(calls[0].url,'/api/update/install');
  assert.deepEqual(JSON.parse(calls[0].options.body),{});
  assert.equal(app.elements.get('install-update').disabled,true);
  assert.equal(app.elements.get('download-update').disabled,true);
  assert.equal(app.elements.get('update-status').textContent,'Downloading update…');
  app.render(available);
  assert.equal(app.elements.get('update-status').textContent,'Downloading update…','a poll cannot undo an active install');
  rejectInstall(Error('Checksum mismatch'));
  await pending;
  assert.match(app.elements.get('update-status').textContent,/Update could not be installed\. Checksum mismatch/);
  assert.equal(app.elements.get('install-update').disabled,false);
  assert.equal(app.elements.get('check-updates').disabled,false);
});

test('successful installation keeps a restarting status and blocks further actions',async()=>{
  const app=interfaceFixture();let requests=0;
  app.render(available);
  app.context.fetch=async()=>{requests++;return {ok:true,json:async()=>({...available,phase:'installing'})};};
  await app.run('installUpdate()');
  assert.equal(app.elements.get('update-status').textContent,'Installing update and restarting clipclub…');
  assert.equal(app.elements.get('install-update').disabled,true);
  await app.run('installUpdate()');
  await app.run('openUpdate("download")');
  await app.run('checkUpdates()');
  assert.equal(requests,1);
});

test('current versions and releases without a matching asset hide unavailable actions',()=>{
  const app=interfaceFixture();
  app.render({...available,available:false,status:'current'});
  assert.equal(app.elements.get('update-status').textContent,'You have the latest release.');
  assert.equal(app.elements.get('install-update').hidden,true);
  assert.equal(app.elements.get('download-update').hidden,true);
  app.render({...available,can_install:false,download_url:null,latest_version:'<img onerror=alert(1)>'});
  assert.equal(app.elements.get('download-update').hidden,true);
  assert.equal(app.elements.get('update-release').hidden,false);
  assert.equal(app.elements.get('update-status').innerHTML,'','release metadata must never become HTML');
  assert.match(app.elements.get('update-status').textContent,/<img/);
});
