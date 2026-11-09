'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const {test}=require('node:test');
const root=path.resolve(__dirname,'..'),html=fs.readFileSync(path.join(root,'ui/index.html'),'utf8'),source=fs.readFileSync(path.join(root,'ui/app.js'),'utf8');
const address='0x1111111111111111111111111111111111111111',contract='0x2222222222222222222222222222222222222222',browserAddress='0x3333333333333333333333333333333333333333';
const hash='0x'+'a'.repeat(64),sessionId='b'.repeat(64),privateKey='0x'+'9'.repeat(64),password='a long wallet password';
function fixture({exists=true,unlocked=true}={}){
  const elements=new Map([...html.matchAll(/\bid="([^"]+)"/g)].map(([,id])=>[id,{id,value:id==='wallet-mode'?'create':id==='library-sort'?'recency':id==='roll-wager'?'1':'',textContent:'',innerHTML:'',open:false,listeners:{},addEventListener(name,fn){this.listeners[name]=fn;},showModal(){this.open=true;},close(){this.open=false;this.listeners.close?.({});},classList:{toggle(){},add(){},remove(){}},pause(){},load(){},removeAttribute(name){delete this[name];},play(){return Promise.resolve();}}]));
  const calls=[],events={},documentEvents={},walletCalls=[],storage=new Map(),downloads=[];
  let initialStateRequest;
  const state={crypto:{local_wallet:{exists,address:exists?address:null,unlocked:exists&&unlocked},native_balance_wei:'1234567890123456789',name:'좋아',symbol:'✨',decimals:6,configured:true,wallet:exists?address:null,contract,chain_id:4326,chain_hex:'0x10e6',chain_name:'MegaETH',balance_units:'1000000000',total_supply_units:'1000000000000000',error:null,rating_session:null,pending_transaction:null},session:false,queue:[],library:[],peers:[],addresses:[],profile_id:'peer-a',temporary_seeds:0,roll:null};
  const provider={on(name,fn){events[name]=fn;},async request(request){walletCalls.push(request);if(['eth_accounts','eth_requestAccounts'].includes(request.method))return[browserAddress];if(request.method==='eth_chainId')return'0x10e6';throw Error(`Unexpected browser request ${request.method}`);}};
  const context=vm.createContext({document:{body:{dataset:{page:'settings'},appendChild(){}},getElementById(id){assert.ok(elements.has(id),id);return elements.get(id);},querySelector(selector){return selector.startsWith('meta')?{content:'token'}:{addEventListener(){},setAttribute(){},focus(){}};},querySelectorAll(selector){return selector==='.page'?['home','seed','rate','roll','settings'].map(page=>elements.get(`page-${page}`)):[];},addEventListener(name,fn){documentEvents[name]=fn;},createElement(){return{click(){downloads.push(this);},remove(){}};}},window:{ethereum:provider,addEventListener(){},scrollTo(){},localStorage:{getItem:key=>storage.get(key),setItem:(key,value)=>storage.set(key,value),removeItem:key=>storage.delete(key)}},fetch(){return new Promise(resolve=>{initialStateRequest=resolve;});},FormData:class{},Blob,URL:{createObjectURL(blob){downloads.blob=blob;return'blob:wallet-backup';},revokeObjectURL(){}},setTimeout(){},clearTimeout(){},setInterval(){}});
  const startup=vm.runInContext(source,context);context.fixture=state;vm.runInContext('state=fixture;page="settings";',context);
  const app={state,elements,context,calls,events,documentEvents,walletCalls,storage,downloads,run:code=>vm.runInContext(code,context),respond:null,async start(){this.run('state=null;page="home";document.body.dataset.page="home";');initialStateRequest(await this.respond('/api/state'));await startup;}};
  app.respond=async(url,options)=>{
    const body=options?.body?JSON.parse(options.body):null;calls.push({url,body});
    const ok=value=>({ok:true,json:async()=>value});
    if(url==='/api/state')return ok(state);
    if(['/api/wallet/create','/api/wallet/import','/api/wallet/restore','/api/wallet/unlock'].includes(url)){state.crypto.local_wallet={exists:true,address,unlocked:true};state.crypto.wallet=address;return ok({address});}
    if(url==='/api/wallet/backup')return ok({version:1,address,ciphertext:'encrypted-keystore-only'});
    if(url==='/api/crypto/transaction'){
      if(body.action==='rate_session')state.crypto.rating_session.status='prepared';
      return ok({from:address,to:contract,data:body.action==='rate_session'?'0x12345678'+sessionId+'0'.repeat(64):'0xabcd',value:'0x0',chainId:'0x10e6',wallet_approval:{id:'approval-1',gas_limit:30000,gas_price_wei:'1000000',max_fee_wei:'30000000000',expires_in_seconds:120,action:body.action}});
    }
    if(url==='/api/wallet/send')return ok({hash});
    if(url==='/api/wallet/rebroadcast')return ok({hash});
    if(url==='/api/wallet/rpc')return ok({status:'0x1'});
    if(url==='/api/crypto/pending'){state.crypto.pending_transaction=body;return ok({});}
    if(url==='/api/crypto/confirm-account')return ok({confirmed:true});
    if(url==='/api/rating-session/unlock'){state.crypto.rating_session.status='draft';return ok({});}
    if(url==='/api/rating-session/confirm'){state.crypto.rating_session=null;return ok({});}
    throw Error(`Unexpected API ${url}`);
  };
  context.fetch=(...args)=>app.respond(...args);return app;
}
async function until(predicate){for(let index=0;index<20&&!predicate();index++)await new Promise(setImmediate);assert.ok(predicate(),'Expected pending operation');}
const submitEvent={preventDefault(){}};
function queued(app){app.state.crypto.rating_session={id:sessionId,status:'draft',ratings:[{clip_id:'c'.repeat(64),score:3,title:'Clip'}],total_units:'3000000'};}

test('navigation waits for startup wallet status before entering the app',async()=>{
  const app=fixture();app.run('state=null;page="home"');await app.run('navigate("rate")');await app.run('connectWallet()');
  assert.equal(app.run('page'),'home');assert.equal(app.elements.get('wallet-dialog').open,false);assert.equal(app.calls.length,0);
});
test('first launch requires wallet setup on Home, including when the token network is unavailable',async()=>{
  const app=fixture({exists:false});app.state.crypto.error='Network unavailable';await app.start();
  assert.equal(app.elements.get('wallet-dialog').open,true);assert.equal(app.elements.get('wallet-dialog-title').textContent,'Welcome to clipclub');assert.equal(app.elements.get('close-wallet').hidden,true);assert.equal(app.elements.get('manage-wallet').hidden,true);
  let cancelled=false;app.elements.get('wallet-dialog').listeners.cancel({preventDefault(){cancelled=true;}});assert.equal(cancelled,true);
  app.elements.get('close-wallet').listeners.click();assert.equal(app.elements.get('wallet-dialog').open,true);
  await app.run('navigate("settings")');assert.equal(app.run('page'),'home');assert.equal(app.calls.some(call=>call.url==='/api/session'),false);
});
test('startup setup preserves entered secrets across polling and continues after creation, import, or restore',async()=>{
  for(const mode of ['create','import','restore']){
    const app=fixture({exists:false});await app.start();app.elements.get('wallet-mode').value=mode;
    app.elements.get('wallet-password').value=password;app.elements.get('wallet-password-confirm').value=password;app.elements.get('wallet-private-key').value=privateKey;app.elements.get('wallet-backup-json').value=JSON.stringify({version:1,ciphertext:'encrypted'});
    await app.run('refresh()');assert.equal(app.elements.get('wallet-password').value,password);assert.equal(app.elements.get('wallet-dialog').open,true);
    await app.elements.get('wallet-form').listeners.submit(submitEvent);
    assert.equal(app.elements.get('wallet-dialog').open,false);assert.equal(app.state.crypto.local_wallet.unlocked,true);assert.equal(app.run('page'),'home');assert.equal(app.storage.size,0);
    await app.run('navigate("settings")');assert.equal(app.run('page'),'settings');assert.equal(app.elements.get('wallet-dialog').open,false);
  }
});
test('returning users unlock once at launch and can reopen wallet details without another prompt',async()=>{
  const app=fixture({unlocked:false});await app.start();assert.equal(app.elements.get('wallet-dialog').open,true);assert.equal(app.elements.get('wallet-setup-options').hidden,true);assert.equal(app.elements.get('wallet-submit').textContent,'Unlock and continue');
  app.elements.get('wallet-password').value=password;await app.elements.get('wallet-form').listeners.submit(submitEvent);
  assert.equal(app.calls.filter(call=>call.url==='/api/wallet/unlock').length,1);assert.equal(app.elements.get('wallet-dialog').open,false);assert.equal(app.elements.get('wallet-password').value,'');
  await app.run('connectWallet()');assert.equal(app.elements.get('wallet-dialog').open,true);assert.equal(app.elements.get('wallet-password-row').hidden,true);assert.equal(app.elements.get('close-wallet').hidden,false);
  let cancelled=false;app.elements.get('wallet-dialog').listeners.cancel({preventDefault(){cancelled=true;}});assert.equal(cancelled,false);
  app.elements.get('close-wallet').listeners.click();await app.run('refresh()');assert.equal(app.elements.get('wallet-dialog').open,false);assert.match(app.elements.get('native-wallet-status').textContent,/unlocked until you quit/);
});
test('failed startup unlock keeps the app gated and clears the password for retry',async()=>{
  const app=fixture({unlocked:false});await app.start();const respond=app.respond;
  app.respond=async(url,options)=>url==='/api/wallet/unlock'?{ok:false,json:async()=>({error:'Incorrect password'})}:respond(url,options);
  app.elements.get('wallet-password').value='incorrect password';await app.elements.get('wallet-form').listeners.submit(submitEvent);
  assert.equal(app.elements.get('wallet-dialog').open,true);assert.equal(app.elements.get('wallet-password').value,'');assert.equal(app.state.crypto.local_wallet.unlocked,false);assert.match(app.elements.get('wallet-form-error').textContent,/Incorrect password/);
  await app.run('refresh()');assert.match(app.elements.get('wallet-form-error').textContent,/Incorrect password/);await app.run('navigate("rate")');assert.equal(app.run('page'),'home');
});
test('loading an already unlocked app session does not prompt for a password',async()=>{
  const app=fixture();await app.start();assert.equal(app.elements.get('wallet-dialog').open,false);await app.run('refresh()');assert.equal(app.elements.get('wallet-dialog').open,false);assert.equal(app.calls.some(call=>call.url.startsWith('/api/wallet/')),false);
});
test('the balance action opens wallet details and keeps queued funds visible across refreshes',async()=>{
  const app=fixture();queued(app);await app.start();
  assert.equal(app.elements.get('balance').textContent,'997.00 ✨');
  assert.equal(app.elements.get('reserved-balance').hidden,false);
  assert.match(app.elements.get('reserved-balance').textContent,/3\.00 ✨ queued/);
  await app.elements.get('connect-wallet').listeners.click();
  assert.equal(app.elements.get('wallet-dialog').open,true);
  assert.equal(app.elements.get('wallet-dialog-account').hidden,false);
  app.elements.get('close-wallet').listeners.click();
  app.state.crypto.balance_units='2000000000';await app.run('refresh()');
  assert.equal(app.elements.get('balance').textContent,'1,997.00 ✨');
  assert.equal(app.elements.get('wallet-dialog').open,false);
});
test('automatic discovery shows connection progress without manual peer controls',async()=>{
  const app=fixture();app.state.session=true;await app.start();
  for(const id of ['bootstrap-peers','connect-peer','settings-form','save-status'])assert.equal(app.elements.has(id),false);
  assert.equal(app.elements.get('peer-status').textContent,'Finding peers…');
  assert.equal(app.elements.get('discovery-status').textContent,'Finding peers…');
  assert.equal(app.elements.get('waiting-message').textContent,'Finding peers automatically…');
  app.state.peers=[{id:'peer-b'}];app.state.addresses=['/ip4/192.168.1.20/tcp/42931/p2p/peer-a'];await app.run('refresh()');
  assert.equal(app.elements.get('peer-status').textContent,'1 peer connected');
  assert.equal(app.elements.get('discovery-status').textContent,'1 peer connected');
  assert.equal(app.elements.get('waiting-message').textContent,'Waiting for a clip from your peers.');
  assert.equal(app.elements.get('peer-id').textContent,'peer-a');
  assert.match(app.elements.get('listen-addresses').innerHTML,/data-address="\/ip4\/192\.168\.1\.20\/tcp\/42931\/p2p\/peer-a"/);
  assert.equal(app.calls.some(call=>call.url==='/api/settings'),false);
  app.respond=async()=>{throw Error('Local interface unavailable');};await app.run('refresh()');
  assert.equal(app.elements.get('peer-status').textContent,'Client disconnected');
  assert.equal(app.elements.get('discovery-status').textContent,'Client disconnected');
});

test('the main wallet action opens native setup without asking a browser wallet',async()=>{
  const app=fixture({exists:false});await app.run('connectWallet()');assert.equal(app.elements.get('wallet-dialog').open,true);assert.equal(app.elements.get('wallet-submit').textContent,'Create wallet');assert.equal(app.calls.length,0);assert.equal(app.walletCalls.length,0);
});
test('create and import clear passwords and keys from the form and do not use browser storage',async()=>{
  for(const mode of ['create','import']){
    const app=fixture({exists:false});app.elements.get('wallet-mode').value=mode;app.elements.get('wallet-password').value=password;app.elements.get('wallet-password-confirm').value=password;app.elements.get('wallet-private-key').value=privateKey;
    await app.elements.get('wallet-form').listeners.submit(submitEvent);
    const call=app.calls.find(call=>call.url===`/api/wallet/${mode}`);assert.equal(call.body.password,password);if(mode==='import')assert.equal(call.body.private_key,privateKey);else assert.equal(call.body.private_key,undefined);
    for(const id of ['wallet-private-key','wallet-password','wallet-password-confirm'])assert.equal(app.elements.get(id).value,'');assert.equal(app.storage.size,0);assert.equal(app.state.crypto.local_wallet.unlocked,true);
  }
});
test('short or mismatched passwords and malformed imports never reach wallet APIs',async()=>{
  const app=fixture({exists:false});app.elements.get('wallet-password').value='short';app.elements.get('wallet-password-confirm').value='short';await app.elements.get('wallet-form').listeners.submit(submitEvent);assert.equal(app.calls.length,0);assert.match(app.elements.get('wallet-form-error').textContent,/12 characters/);
  app.elements.get('wallet-password').value=password;app.elements.get('wallet-password-confirm').value='different';await app.elements.get('wallet-form').listeners.submit(submitEvent);assert.equal(app.calls.length,0);
  app.elements.get('wallet-password-confirm').value=password;app.elements.get('wallet-mode').value='import';app.elements.get('wallet-private-key').value='bad';await app.elements.get('wallet-form').listeners.submit(submitEvent);assert.equal(app.calls.length,0);assert.equal(app.elements.get('wallet-private-key').value,'');
});
test('encrypted backups download only the server keystore and restoration uses its password',async()=>{
  const app=fixture();await app.run('backupNativeWallet()');assert.equal(app.downloads.length,1);assert.match(app.downloads[0].download,/clipclub-wallet-0x/);const exported=await app.downloads.blob.text();assert.match(exported,/encrypted-keystore-only/);assert.doesNotMatch(exported,/a long wallet password|999999999999/);
  const restored=fixture({exists:false});restored.elements.get('wallet-mode').value='restore';restored.elements.get('wallet-password').value=password;restored.elements.get('wallet-backup-json').value=exported;await restored.elements.get('wallet-form').listeners.submit(submitEvent);
  const call=restored.calls.find(call=>call.url==='/api/wallet/restore');assert.equal(call.body.backup.ciphertext,'encrypted-keystore-only');assert.equal(call.body.password,password);assert.equal(restored.elements.get('wallet-backup-json').value,'');
});
test('native transactions wait for explicit review approval and send only its opaque approval id',async()=>{
  const app=fixture(),payment=app.run('sendCryptoTransaction("register_seeder")');await until(()=>app.elements.get('wallet-approval-dialog').open);
  assert.equal(app.calls.some(call=>call.url==='/api/wallet/send'),false);assert.equal(app.walletCalls.length,0);assert.equal(app.elements.get('wallet-review-from').textContent,address);assert.equal(app.elements.get('wallet-review-contract').textContent,contract);assert.equal(app.elements.get('wallet-review-fee').textContent,'0.00000003 ETH');
  app.elements.get('wallet-approval-form').listeners.submit(submitEvent);await payment;
  assert.deepEqual(app.calls.find(call=>call.url==='/api/wallet/send').body,{approval_id:'approval-1'});assert.equal(app.calls.filter(call=>call.url==='/api/wallet/send').length,1);assert.equal(app.walletCalls.length,0);
});
test('cancelling native session review keeps the same unpaid draft and sends no transaction',async()=>{
  const app=fixture();queued(app);const checkout=app.run('settleRatingSession()');await until(()=>app.elements.get('wallet-approval-dialog').open);assert.equal(app.elements.get('wallet-review-amount').textContent,'3.00 ✨');
  app.elements.get('wallet-approval-cancel').listeners.click();await checkout;assert.equal(app.state.crypto.rating_session.status,'draft');assert.equal(app.state.crypto.rating_session.ratings.length,1);assert.equal(app.calls.some(call=>call.url==='/api/wallet/send'),false);
});
test('locking during a review cancels it and an old approval click cannot send',async()=>{
  const app=fixture(),payment=app.run('sendCryptoTransaction("register_seeder")');await until(()=>app.elements.get('wallet-approval-dialog').open);app.state.crypto.local_wallet.unlocked=false;app.run('renderWallet()');
  await assert.rejects(payment,/cancelled/);app.elements.get('wallet-approval-form').listeners.submit(submitEvent);assert.equal(app.calls.some(call=>call.url==='/api/wallet/send'),false);
});
test('a locked wallet can rebroadcast and confirm a saved payment without another signature',async()=>{
  const app=fixture({unlocked:false});app.state.crypto.pending_transaction={action:'register_seeder',hash};app.run('renderWallet()');await app.run('retryTransaction()');
  assert.equal(app.calls.filter(call=>call.url==='/api/wallet/rebroadcast').length,1);assert.equal(app.calls.filter(call=>call.url==='/api/wallet/rpc').length,1);assert.equal(app.calls.some(call=>call.url==='/api/wallet/send'),false);assert.equal(app.elements.get('wallet-approval-dialog').open,false);assert.equal(app.state.crypto.pending_transaction,null);assert.equal(app.walletCalls.length,0);
});
test('injected browser wallets receive no listeners or requests',async()=>{
  const app=fixture();await app.run('connectWallet()');assert.equal(app.elements.get('wallet-dialog').open,true);assert.deepEqual(Object.keys(app.events),[]);assert.equal(app.walletCalls.length,0);assert.equal(app.state.crypto.wallet,address);
});
test('expired native reviews cannot approve and dialog dismissal clears import secrets',async()=>{
  const app=fixture(),payment=app.run('sendCryptoTransaction("register_seeder")');await until(()=>app.elements.get('wallet-approval-dialog').open);app.run('nativeApproval.expires=0');app.elements.get('wallet-approval-form').listeners.submit(submitEvent);assert.match(app.elements.get('wallet-approval-error').textContent,/expired/);assert.equal(app.calls.some(call=>call.url==='/api/wallet/send'),false);app.elements.get('wallet-approval-cancel').listeners.click();await assert.rejects(payment,/cancelled/);
  app.run('openNativeWallet()');app.elements.get('wallet-private-key').value=privateKey;app.elements.get('wallet-password').value=password;app.elements.get('wallet-dialog').close();assert.equal(app.elements.get('wallet-private-key').value,'');assert.equal(app.elements.get('wallet-password').value,'');
});

test('wallet dialogs keep Escape and number keys inside the modal',()=>{
  const app=fixture();app.run('page="rate"');
  for(const id of ['wallet-dialog','wallet-approval-dialog']){app.elements.get(id).open=true;for(const key of ['Escape','3']){let prevented=false;app.documentEvents.keydown({key,target:{tagName:'BUTTON'},preventDefault(){prevented=true;}});assert.equal(prevented,false);assert.equal(app.run('page'),'rate');assert.equal(app.calls.length,0);}app.elements.get(id).open=false;}
});
