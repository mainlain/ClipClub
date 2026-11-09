'use strict';
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const vm=require('node:vm');
const {test}=require('node:test');
const root=path.resolve(__dirname,'..');
const html=fs.readFileSync(path.join(root,'ui/index.html'),'utf8');
const source=fs.readFileSync(path.join(root,'ui/app.js'),'utf8');
const address='0x1111111111111111111111111111111111111111';
const contract='0x2222222222222222222222222222222222222222';
const hash='0x'+'a'.repeat(64);
const clip={beneficiary:'0x4444444444444444444444444444444444444444',id:'clip-a',title:'A clip',extension:'mp4',duration_secs:3,width:96,height:64};
function result(id='1',multiplier=9,media=clip){return {id,status:'settled',multiplier,wager_units:'2000000',payout_units:String(2000000*multiplier),round:'123',transaction_hash:hash,settlement_hash:hash,clip:media,message:null};}
function interfaceFixture(){
  const elements=new Map([...html.matchAll(/\bid="([^"]+)"/g)].map(([,id])=>[id,{
    innerHTML:'',textContent:'',value:id==='roll-wager'?'1':id==='library-sort'?'recency':'',
    paused:true,ended:false,loop:true,muted:false,volume:1,playCount:0,pauseCount:0,loadCount:0,
    listeners:{},addEventListener(name,fn){this.listeners[name]=fn;},
    showModal(){this.open=true;},close(){this.open=false;},
    classList:{toggle(){},add(){},remove(){}},
    play(){this.paused=false;this.playCount++;return Promise.resolve();},
    pause(){this.paused=true;this.pauseCount++;},load(){this.loadCount++;},removeAttribute(name){delete this[name];},
  }]));
  const walletCalls=[],calls=[],storage=new Map();
  const context=vm.createContext({
    document:{getElementById(id){assert.ok(elements.has(id),`Unknown element ${id}`);return elements.get(id);},querySelector(s){return s.startsWith('meta')?{content:'test-token'}:{addEventListener(){}};},querySelectorAll(){return [];},addEventListener(){}},
    window:{addEventListener(){},localStorage:{getItem:key=>storage.get(key),setItem:(key,value)=>storage.set(key,value),removeItem:key=>storage.delete(key)}},
    fetch(){return new Promise(()=>{});},FormData:class{},setTimeout(){},clearTimeout(){},setInterval(){},
  });
  vm.runInContext(source,context);
  const fixture={crypto:{local_wallet:{exists:true,address,unlocked:true},name:'좋아',symbol:'✨',decimals:6,configured:true,wallet:address,contract,chain_id:4326,chain_hex:'0x10e6',chain_name:'MegaETH',explorer_url:'https://mega.etherscan.io',balance_units:'100000000',total_supply_units:'1000000000',error:null},roll:null,library:[],queue:[],peers:[],addresses:[],session:false,profile_id:'peer-a'};
  context.fixture=fixture;context.address=address;vm.runInContext('state=fixture;page="roll";reviewNativeTransaction=async()=>"approval-1";',context);
  const app={context,fixture,elements,walletCalls,calls,storage,send:async()=>({hash}),receipt:async()=>({status:"0x1"}),run:code=>vm.runInContext(code,context),render:()=>vm.runInContext('renderRoll();',context)};
  app.respond=async(url,options)=>{
    calls.push({url,options});
    if(url==='/api/crypto/transaction')return {ok:true,json:async()=>({from:address,to:contract,data:'0xabcd',value:'0x0',chainId:'0x10e6'})};
    if(url==='/api/roll'){fixture.roll=result();return {ok:true,json:async()=>fixture.roll};}
    if(url==='/api/rate')return {ok:true,json:async()=>({score:2})};
    if(url==='/api/crypto/pending'){fixture.crypto.pending_transaction=JSON.parse(options.body);return {ok:true,json:async()=>({})};}
    if(url==='/api/wallet/send'){walletCalls.push({method:'send'});const sent=await app.send();return {ok:true,json:async()=>sent};}
    if(url==='/api/wallet/rebroadcast')return {ok:true,json:async()=>({hash})};
    if(url==='/api/wallet/rpc'){walletCalls.push(JSON.parse(options.body));const receipt=await app.receipt();return {ok:true,json:async()=>receipt};}
    if(url==='/api/state')return {ok:true,json:async()=>fixture};
    throw Error(`Unexpected API ${url}`);
  };
  context.fetch=(...args)=>app.respond(...args);
  return app;
}
async function until(predicate){for(let i=0;i<10&&!predicate();i++)await new Promise(setImmediate);assert.ok(predicate(),'Expected operation to reach its pending stage');}
function sends(app){return app.walletCalls.filter(call=>call.method==='send');}

test('money arithmetic preserves six decimals beyond the JavaScript safe integer range',()=>{
  const app=interfaceFixture();
  assert.equal(app.run("tokenUnits('9007199254740993.000001').toString()"),'9007199254740993000001');
  assert.equal(app.run("fmtTokens('9007199254740993000001')"),'9,007,199,254,740,993.000001');
  assert.equal(app.run("fmtTokens('1')"),'0.000001');
  for(const input of ['1e6','1.0000001','0','-1','Infinity',''])assert.equal(app.run(`tokenUnits(${JSON.stringify(input)})`),null);
});
test('Roll plays once and polls preserve an ended clip or manual pause',()=>{
  const app=interfaceFixture(),player=app.elements.get('roll-player');
  assert.doesNotMatch(html.match(/<video id="roll-player"[^>]*>/)[0],/\bloop\b/);
  assert.match(html.match(/<video id="player"[^>]*>/)[0],/\bloop\b/);
  app.fixture.roll=result();app.render();assert.equal(player.loop,false);assert.equal(player.playCount,1);assert.equal(player.muted,false);assert.equal(player.volume,1);
  const loads=player.loadCount;player.paused=true;player.ended=true;app.render();app.render();
  assert.equal(player.playCount,1);assert.equal(player.loadCount,loads);
  player.ended=false;app.render();assert.equal(player.playCount,1);
  player.muted=true;player.volume=.35;
  app.fixture.roll=result('2');app.render();assert.equal(player.playCount,2);assert.equal(player.muted,true);assert.equal(player.volume,.35);
});
test('zero burns the displayed stake, removes the clip, and a downloaded win starts once',()=>{
  const app=interfaceFixture(),player=app.elements.get('roll-player');app.fixture.roll=result();app.render();
  app.fixture.roll=result('2',0,null);app.render();assert.equal(player.src,undefined);assert.equal(app.elements.get('roll-player-empty').hidden,false);
  assert.match(app.elements.get('roll-result-detail').textContent,/2\.00 ✨ burned/);assert.match(app.elements.get('roll-waiting-title').textContent,/0×/);
  app.fixture.roll=result('3',9,null);app.render();assert.equal(player.playCount,1);
  app.fixture.roll.clip=clip;app.render();app.render();assert.equal(player.playCount,2);
});
test('pending commitment never reveals a multiplier and offers explicit settlement',()=>{
  const app=interfaceFixture();app.fixture.roll={...result(),status:'pending',multiplier:null,payout_units:'0',clip:null};app.render();
  assert.equal(app.elements.get('roll-multiplier').textContent,'…');assert.equal(app.elements.get('roll-button').disabled,true);
  assert.equal(app.elements.get('settle-roll').hidden,false);assert.match(app.elements.get('roll-result-detail').textContent,/locked on MegaETH/);
  assert.match(app.elements.get('roll-proof').innerHTML,/drand round 123/);assert.equal(app.elements.get('roll-player').playCount,0);
});
test('leaving Roll pauses playback and returning does not replay it',async()=>{
  const app=interfaceFixture(),player=app.elements.get('roll-player');app.fixture.roll=result();app.render();
  await app.run('navigate("seed")');assert.equal(player.paused,true);await app.run('navigate("roll")');assert.equal(player.playCount,1);
  app.run('page="seed"');app.fixture.roll=result('2');app.render();assert.equal(player.playCount,1);
  await app.run('navigate("roll")');assert.equal(player.playCount,2);
});
test('invalid wagers or insufficient balances never call wallet or API',async()=>{
  const app=interfaceFixture();
  for(const value of ['','0','-1','NaN','Infinity','1.0000001','1e2','101']){app.elements.get('roll-wager').value=value;await app.run('roll()');assert.equal(app.calls.length,0);assert.equal(app.walletCalls.length,0);}
  app.elements.get('roll-wager').value='.000001';app.render();assert.equal(app.elements.get('roll-button').disabled,false);
});
test('a saved address cannot sign without its built-in wallet',async()=>{
  const app=interfaceFixture();app.fixture.crypto.local_wallet={exists:false};
  await assert.rejects(app.run('sendCryptoTransaction("request_roll",{wager_units:"1000000"})'),/Set up your built-in wallet/);assert.equal(app.calls.length,0);assert.equal(sends(app).length,0);assert.equal(app.elements.get('wallet-dialog').open,true);
});
test('a transaction for the wrong chain never reaches wallet approval',async()=>{
  const app=interfaceFixture();app.respond=async()=>({ok:true,json:async()=>({from:address,to:contract,data:'0xabcd',value:'0x0',chainId:'0x1'})});
  await app.run('roll()');assert.equal(sends(app).length,0);assert.match(app.elements.get('roll-error').textContent,/does not match/);
});
test('transactions to a different contract are rejected before the wallet prompts',async()=>{
  const app=interfaceFixture();app.respond=async()=>({ok:true,json:async()=>({from:address,to:address,data:'0xabcd',value:'0x0',chainId:'0x10e6'})});
  await app.run('roll()');assert.equal(sends(app).length,0);assert.match(app.elements.get('roll-error').textContent,/does not match/);
});
test('one submission sends exact units and blocks duplicate sends until inclusion and confirmation',async()=>{
  const app=interfaceFixture();let resolveSend;
  app.elements.get('roll-wager').value='2.125';app.send=()=>new Promise(resolve=>{resolveSend=resolve;});
  const pending=app.run('roll()');await until(()=>!!resolveSend);await app.run('roll()');
  assert.equal(sends(app).length,1);assert.equal(app.elements.get('roll-button').disabled,true);assert.equal(app.elements.get('roll-wager').disabled,true);
  assert.deepEqual(JSON.parse(app.calls[0].options.body),{action:'request_roll',wager_units:'2125000'});
  assert.equal(app.calls.filter(call=>call.url==='/api/roll').length,0,'Server import must wait for inclusion');
  resolveSend({hash});await pending;
  assert.deepEqual(JSON.parse(app.calls.find(call=>call.url==='/api/roll').options.body),{transaction_hash:hash});
  assert.match(app.elements.get('roll-result-detail').textContent,/Total payout 18\.00 ✨/);assert.equal(app.elements.get('roll-player').playCount,1);assert.equal(app.elements.get('roll-wager').disabled,false);
});
test('settlement sends the committed roll id and does not choose a new wager',async()=>{
  const app=interfaceFixture();app.fixture.roll={...result('7'),status:'pending',multiplier:null,clip:null};
  await app.run('settleRoll()');assert.deepEqual(JSON.parse(app.calls.find(call=>call.url==='/api/crypto/transaction').options.body),{action:'settle_roll',roll_id:'7'});
  assert.equal(sends(app).length,1);
});
test('failed local confirmation retains the on-chain hash and retries without paying twice',async()=>{
  const app=interfaceFixture();const original=app.respond;let failConfirmation=true;
  app.respond=async(url,options)=>{
    if(url==='/api/roll'&&failConfirmation){app.calls.push({url,options});return {ok:false,json:async()=>({error:'Temporary local persistence failure'})};}return original(url,options);
  };
  await app.run('roll()');assert.equal(sends(app).length,1);assert.equal(app.run('pendingTransaction.hash'),hash);assert.equal(app.storage.size,1);
  failConfirmation=false;await app.run('retryTransaction()');assert.equal(sends(app).length,1);assert.equal(app.run('pendingTransaction'),null);assert.equal(app.storage.size,0);
  const confirmations=app.calls.filter(call=>call.url==='/api/roll');assert.equal(confirmations.length,2);for(const call of confirmations)assert.deepEqual(JSON.parse(call.options.body),{transaction_hash:hash});
});
test('a reverted transaction is never imported as a valid roll and clears recovery',async()=>{
  const app=interfaceFixture();app.receipt=async()=>({status:'0x0'});
  await app.run('roll()');assert.equal(app.calls.filter(call=>call.url==='/api/roll').length,0);assert.equal(app.run('pendingTransaction'),null);assert.match(app.elements.get('roll-error').textContent,/reverted/);
});
test('a roll can be recovered from its transaction hash without another wallet transaction',async()=>{
  const app=interfaceFixture();app.elements.get('recover-roll-hash').value=hash;await app.run('recoverRoll()');
  assert.equal(sends(app).length,0);assert.deepEqual(JSON.parse(app.calls[0].options.body),{transaction_hash:hash});assert.equal(app.run('state.roll.id'),'1');
});
test('free skip works without a wallet and cannot spend archived points',async()=>{
  const app=interfaceFixture();app.fixture.crypto.wallet=null;app.fixture.queue=[clip];app.fixture.session=true;app.run('page="rate";current=fixture.queue[0];');
  await app.run('rate(0)');assert.deepEqual(JSON.parse(app.calls.find(call=>call.url==='/api/rate').options.body),{id:clip.id,score:0});assert.equal(sends(app).length,0);
});
test('result arriving off-page stays paused and an older state poll cannot erase it',async()=>{
  const app=interfaceFixture();let resolveOld,stateCalls=0;const original=app.respond;
  app.respond=async(url,options)=>url==='/api/state'&&++stateCalls===1?new Promise(resolve=>{resolveOld=()=>resolve({ok:true,json:async()=>({...app.fixture,roll:null})});}):original(url,options);
  const oldPoll=app.run('refresh()');app.run('page="seed"');await app.run('sendCryptoTransaction("request_roll",{wager_units:"1000000"})');resolveOld();await oldPoll;
  assert.equal(app.run('state.roll.id'),'1');assert.equal(app.elements.get('roll-player').playCount,0);
});

test('wallet connection and other token actions stay locked while a signature is pending',async()=>{
  const app=interfaceFixture();let resolveSend;
  app.send=()=>new Promise(resolve=>{resolveSend=resolve;});
  const pending=app.run('roll()');await until(()=>!!resolveSend);
  await app.run('connectWallet()');assert.equal(app.elements.get('wallet-dialog').open,undefined);
  assert.equal(app.elements.get('connect-wallet').disabled,true);
  await assert.rejects(app.run('sendCryptoTransaction("register_clip",{clip_id:"clip-a"})'),/Another token transaction/);
  resolveSend({hash});await pending;assert.equal(sends(app).length,1);
});
test('a locked built-in wallet cannot start a roll transaction',async()=>{
  const app=interfaceFixture();app.fixture.crypto.local_wallet.unlocked=false;
  await assert.rejects(app.run('sendCryptoTransaction("request_roll",{wager_units:"1000000"})'),/Unlock your built-in wallet/);
  assert.equal(app.calls.length,0);assert.equal(sends(app).length,0);assert.equal(app.elements.get('wallet-dialog').open,true);
});
test('unregistered clips and self-owned beneficiaries cannot prompt a token payment',async()=>{
  const app=interfaceFixture();app.fixture.queue=[{...clip,beneficiary:null}];app.fixture.session=true;app.run('page="rate";current=fixture.queue[0];');
  await app.run('rate(2)');assert.equal(app.calls.length,0);assert.equal(sends(app).length,0);
  app.fixture.queue[0].beneficiary=address;await app.run('rate(2)');assert.equal(app.calls.length,0);assert.equal(sends(app).length,0);
});

test('pending transactions restore from server state after a new browser origin or app restart',async()=>{
  const app=interfaceFixture();app.fixture.crypto.pending_transaction={action:'request_roll',hash,wager_units:'1000000'};
  app.run('pendingTransaction=null;renderWallet();');assert.equal(app.run('pendingTransaction.hash'),hash);assert.equal(app.storage.size,0);
  await app.run('retryTransaction()');assert.equal(sends(app).length,0);assert.equal(app.fixture.crypto.pending_transaction,null);assert.equal(app.run('pendingTransaction'),null);
});
test('the local journal receives the hash before inclusion is queried and is cleared after import',async()=>{
  const app=interfaceFixture();const order=[],originalRespond=app.respond;
  app.respond=async(url,options)=>{order.push(url==='/api/crypto/pending'?JSON.parse(options.body)?'persist hash':'clear hash':url);return originalRespond(url,options);};
  await app.run('roll()');assert.ok(order.indexOf('persist hash')<order.indexOf('/api/wallet/rpc'));assert.ok(order.indexOf('/api/roll')<order.indexOf('clear hash'));
  const save=app.calls.find(call=>call.url==='/api/crypto/pending'&&call.options.body!=='null');assert.deepEqual(JSON.parse(save.options.body),{action:'request_roll',hash,wager_units:'1000000'});
});
