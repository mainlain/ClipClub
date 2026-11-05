'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const {test}=require('node:test');
const root=path.resolve(__dirname,'..'),html=fs.readFileSync(path.join(root,'ui/index.html'),'utf8'),source=fs.readFileSync(path.join(root,'ui/app.js'),'utf8');
const address='0x1111111111111111111111111111111111111111',contract='0x2222222222222222222222222222222222222222',browserAddress='0x3333333333333333333333333333333333333333';
const hash='0x'+'a'.repeat(64),sessionId='b'.repeat(64),privateKey='0x'+'9'.repeat(64),password='a long wallet password';
function fixture({exists=true,unlocked=true}={}){
  const elements=new Map([...html.matchAll(/\bid="([^"]+)"/g)].map(([,id])=>[id,{id,value:id==='wallet-mode'?'create':id==='library-sort'?'recency':id==='roll-wager'?'1':'',textContent:'',innerHTML:'',open:false,listeners:{},addEventListener(name,fn){this.listeners[name]=fn;},showModal(){this.open=true;},close(){this.open=false;this.listeners.close?.({});},classList:{toggle(){},add(){},remove(){}},pause(){},load(){},removeAttribute(name){delete this[name];},play(){return Promise.resolve();}}]));
  const calls=[],events={},documentEvents={},walletCalls=[],storage=new Map(),downloads=[];
  const state={crypto:{local_wallet:{exists,address:exists?address:null,unlocked:exists&&unlocked,auto_lock_seconds:900},native_balance_wei:'1234567890123456789',name:'좋아',symbol:'✨',decimals:6,configured:true,wallet:exists?address:null,contract,chain_id:4326,chain_hex:'0x10e6',chain_name:'MegaETH',balance_units:'1000000000',total_supply_units:'1000000000000000',error:null,rating_session:null,pending_transaction:null},session:false,queue:[],settings:{display_name:'Peer',muted:true,volume:.5,bootstrap_peers:[],prefetch:true},library:[],peers:[],addresses:[],profile_id:'peer-a',temporary_seeds:0,roll:null};
  const provider={on(name,fn){events[name]=fn;},async request(request){walletCalls.push(request);if(['eth_accounts','eth_requestAccounts'].includes(request.method))return[browserAddress];if(request.method==='eth_chainId')return'0x10e6';throw Error(`Unexpected browser request ${request.method}`);}};
  const context=vm.createContext({document:{body:{dataset:{page:'settings'},appendChild(){}},getElementById(id){assert.ok(elements.has(id),id);return elements.get(id);},querySelector(selector){return selector.startsWith('meta')?{content:'token'}:{addEventListener(){},setAttribute(){},focus(){}};},querySelectorAll(selector){return selector==='.page'?['home','seed','rate','roll','settings'].map(page=>elements.get(`page-${page}`)):[];},addEventListener(name,fn){documentEvents[name]=fn;},createElement(){return{click(){downloads.push(this);},remove(){}};}},window:{ethereum:provider,addEventListener(){},scrollTo(){},localStorage:{getItem:key=>storage.get(key),setItem:(key,value)=>storage.set(key,value),removeItem:key=>storage.delete(key)}},fetch(){return new Promise(()=>{});},FormData:class{},Blob,URL:{createObjectURL(blob){downloads.blob=blob;return'blob:wallet-backup';},revokeObjectURL(){}},setTimeout(){},clearTimeout(){},setInterval(){}});
  vm.runInContext(source,context);context.fixture=state;vm.runInContext('state=fixture;page="settings";',context);
  const app={state,elements,context,calls,events,documentEvents,walletCalls,storage,downloads,run:code=>vm.runInContext(code,context),respond:null};
  app.respond=async(url,options)=>{
    const body=options?.body?JSON.parse(options.body):null;calls.push({url,body});
    const ok=value=>({ok:true,json:async()=>value});
    if(url==='/api/state')return ok(state);
    if(['/api/wallet/create','/api/wallet/import','/api/wallet/restore','/api/wallet/unlock'].includes(url)){state.crypto.local_wallet={exists:true,address,unlocked:true,auto_lock_seconds:900};state.crypto.wallet=address;return ok({address});}
    if(url==='/api/wallet/lock'){state.crypto.local_wallet.unlocked=false;return ok({});}
    if(url==='/api/wallet/backup')return ok({version:1,address,ciphertext:'encrypted-keystore-only'});
    if(url==='/api/wallet'){state.crypto.wallet=body.address;return ok({});}
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
test('browser account changes leave the selected native wallet and approvals alone',async()=>{
  const app=fixture();await app.events.accountsChanged([browserAddress]);await app.events.accountsChanged([]);assert.equal(app.state.crypto.wallet,address);assert.equal(app.calls.length,0);assert.equal(app.run('walletProvider()===nativeWalletProvider'),true);
});
test('expired native reviews cannot approve and dialog dismissal clears import secrets',async()=>{
  const app=fixture(),payment=app.run('sendCryptoTransaction("register_seeder")');await until(()=>app.elements.get('wallet-approval-dialog').open);app.run('nativeApproval.expires=0');app.elements.get('wallet-approval-form').listeners.submit(submitEvent);assert.match(app.elements.get('wallet-approval-error').textContent,/expired/);assert.equal(app.calls.some(call=>call.url==='/api/wallet/send'),false);app.elements.get('wallet-approval-cancel').listeners.click();await assert.rejects(payment,/cancelled/);
  app.run('openNativeWallet()');app.elements.get('wallet-private-key').value=privateKey;app.elements.get('wallet-password').value=password;app.elements.get('wallet-dialog').close();assert.equal(app.elements.get('wallet-private-key').value,'');assert.equal(app.elements.get('wallet-password').value,'');
});

test('wallet dialogs keep Escape and number keys inside the modal',()=>{
  const app=fixture();app.run('page="rate"');
  for(const id of ['wallet-dialog','wallet-approval-dialog']){app.elements.get(id).open=true;for(const key of ['Escape','3']){let prevented=false;app.documentEvents.keydown({key,target:{tagName:'BUTTON'},preventDefault(){prevented=true;}});assert.equal(prevented,false);assert.equal(app.run('page'),'rate');assert.equal(app.calls.length,0);}app.elements.get(id).open=false;}
});
