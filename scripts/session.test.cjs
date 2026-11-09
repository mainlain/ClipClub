'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const {test}=require('node:test');
const root=path.resolve(__dirname,'..'),html=fs.readFileSync(path.join(root,'ui/index.html'),'utf8'),source=fs.readFileSync(path.join(root,'ui/app.js'),'utf8');
const address='0x1111111111111111111111111111111111111111',contract='0x2222222222222222222222222222222222222222',recipient='0x4444444444444444444444444444444444444444';
const hash='0x'+'a'.repeat(64),sessionId='b'.repeat(64);
const clip=index=>({id:index.toString(16).padStart(64,'0'),beneficiary:recipient,title:`Clip ${index}`,extension:'mp4',duration_secs:3,width:96,height:64});
function fixture(count=5){
  const elements=new Map([...html.matchAll(/\bid="([^"]+)"/g)].map(([,id])=>[id,{id,value:id==='roll-wager'?'1':id==='library-sort'?'recency':'',textContent:'',innerHTML:'',muted:false,volume:1,listeners:{},addEventListener(name,fn){this.listeners[name]=fn;},showModal(){this.open=true;},close(){this.open=false;},classList:{toggle(){},add(){},remove(){}},pause(){this.paused=true;},load(){},removeAttribute(name){delete this[name];},play(){this.paused=false;return Promise.resolve();}}]));
  const scoreButtons=Array.from({length:10},(_,score)=>({dataset:{score:String(score)},disabled:false})),calls=[],walletCalls=[],storage=new Map();
  const state={crypto:{local_wallet:{exists:true,address,unlocked:true},name:'좋아',symbol:'✨',decimals:6,configured:true,wallet:address,contract,chain_id:4326,chain_hex:'0x10e6',chain_name:'MegaETH',explorer_url:'https://mega.etherscan.io',balance_units:'1000000000',total_supply_units:'1000000000000000',error:null,rating_session:null,pending_transaction:null},session:true,queue:Array.from({length:count},(_,index)=>clip(index+1)),settings:{bootstrap_peers:[]},library:[],peers:[],addresses:[],profile_id:'peer-a',temporary_seeds:0,roll:null};
  const context=vm.createContext({document:{body:{dataset:{page:'rate'}},getElementById(id){assert.ok(elements.has(id),id);return elements.get(id);},querySelector(selector){return selector.startsWith('meta')?{content:'token'}:{addEventListener(){},setAttribute(){},focus(){}};},querySelectorAll(selector){return selector==='[data-score]'?scoreButtons:selector==='.page'?['home','seed','rate','roll','settings'].map(page=>elements.get(`page-${page}`)):[];},addEventListener(){}},window:{addEventListener(){},scrollTo(){},localStorage:{getItem:key=>storage.get(key),setItem:(key,value)=>storage.set(key,value),removeItem:key=>storage.delete(key)}},fetch(){return new Promise(()=>{});},FormData:class{},setTimeout(){},clearTimeout(){},setInterval(){}});
  vm.runInContext(source,context);context.fixture=state;context.address=address;vm.runInContext('state=fixture;page="rate";current=fixture.queue[0];reviewNativeTransaction=async()=>"approval-1";',context);
  const app={state,context,elements,scoreButtons,calls,walletCalls,storage,send:async()=>({hash}),receipt:async()=>({status:"0x1"}),confirmed:false,confirmFailure:false,reverted:false,run:code=>vm.runInContext(code,context)};
  app.respond=async(url,options)=>{
    calls.push({url,options});const body=options?.body?JSON.parse(options.body):null;
    const ok=value=>({ok:true,json:async()=>value}),bad=message=>({ok:false,json:async()=>({error:message})});
    if(url==='/api/state')return ok(state);
    if(url==='/api/rate'){
      if(body.score>0){
        state.crypto.rating_session||={id:sessionId,ratings:[],total_units:'0',status:'draft'};
        const session=state.crypto.rating_session;if(session.status!=='draft'||session.ratings.length>=100)return bad('Frozen or full session');
        session.ratings.push({clip_id:body.id,score:body.score,title:state.queue[0].title});session.total_units=(BigInt(session.total_units)+BigInt(body.score)*1000000n).toString();state.temporary_seeds++;
      }
      state.queue.shift();return ok({queued:body.score>0,score:body.score});
    }
    if(url==='/api/crypto/transaction'){
      assert.equal(body.action,'rate_session');assert.deepEqual(Object.keys(body),['action','expected_session_id','expected_rating_count','expected_total_units'],'Only snapshot checks may accompany checkout');
      if(body.expected_session_id!==state.crypto.rating_session.id||body.expected_rating_count!==state.crypto.rating_session.ratings.length||body.expected_total_units!==state.crypto.rating_session.total_units)return bad('The queued session changed. Review the refreshed queue.');
      assert.ok(state.crypto.rating_session);state.crypto.rating_session.status='prepared';return ok({from:address,to:contract,data:'0x12345678'+sessionId+'0'.repeat(64),value:'0x0',chainId:'0x10e6'});
    }
    if(url==='/api/crypto/pending'){
      if(body){assert.equal(body.action,'rate_session');assert.equal(body.session_id,sessionId);state.crypto.pending_transaction=body;state.crypto.rating_session.status='submitted';}
      else{if(state.crypto.rating_session?.status==='submitted')return bad('Cannot clear a submitted rating session');state.crypto.pending_transaction=null;}
      return ok({});
    }
    if(url==='/api/rating-session/confirm'){
      if(app.confirmFailure)return bad('Temporary receipt import failure');
      if(app.reverted)return bad('Canonical receipt reverted');
      if(!app.confirmed){assert.ok(state.crypto.rating_session);state.crypto.balance_units=(BigInt(state.crypto.balance_units)-BigInt(state.crypto.rating_session.total_units)).toString();app.confirmed=true;}
      state.crypto.rating_session=null;state.crypto.pending_transaction=null;return ok({confirmed:true});
    }
    if(url==='/api/rating-session/unlock'){if(state.crypto.pending_transaction)return bad('Submitted hash is locked');state.crypto.rating_session.status='draft';return ok({});}
    if(url==='/api/rating-session/cancel'){if(state.crypto.pending_transaction||state.crypto.rating_session?.status!=='draft')return bad('Cannot discard submitted or prepared ratings');state.crypto.rating_session=null;return ok({});}
    if(url==='/api/rating-session/reset'){assert.ok(app.reverted,'Only a canonical reverted receipt can reset');assert.equal(body.transaction_hash,hash);state.crypto.rating_session.status='draft';state.crypto.pending_transaction=null;return ok({});}
    if(url==='/api/session'){state.session=body.active;return ok({});}
    if(url==='/api/wallet/send'){walletCalls.push({method:'send',body});return ok(await app.send());}
    if(url==='/api/wallet/rebroadcast'){walletCalls.push({method:'rebroadcast'});return ok({hash});}
    if(url==='/api/wallet/rpc'){walletCalls.push(body);return ok(await app.receipt());}
    throw Error(`Unexpected API ${url}`);
  };
  context.fetch=(...args)=>app.respond(...args);return app;
}
const sends=app=>app.walletCalls.filter(call=>call.method==='send');
async function until(predicate){for(let index=0;index<15&&!predicate();index++)await new Promise(setImmediate);assert.ok(predicate(),'Pending operation was not reached');}
async function queue(app,scores){for(const score of scores)await app.run(`rate(${score})`);}

test('many positive ratings queue and advance videos without any wallet request',async()=>{
  const app=fixture(),player=app.elements.get('player');app.run('current=null;render()');
  assert.equal(player.muted,false);assert.equal(player.volume,1);player.muted=true;player.volume=.35;
  await queue(app,[2,4,1]);assert.equal(app.walletCalls.length,0);assert.equal(app.state.queue.length,2);assert.equal(app.state.crypto.rating_session.ratings.length,3);assert.equal(app.state.crypto.rating_session.total_units,'7000000');
  assert.equal(player.muted,true);assert.equal(player.volume,.35);
  assert.equal(app.elements.get('rating-session-status').textContent,'Queued · not paid');assert.match(app.elements.get('reserved-balance').textContent,/7\.00 ✨ queued/);assert.equal(app.confirmed,false);
  for(const call of app.calls.filter(call=>call.url==='/api/rate'))assert.deepEqual(Object.keys(JSON.parse(call.options.body)),['id','score']);
});
test('one checkout pays all queued ratings with one send and a frozen session id',async()=>{
  const app=fixture();await queue(app,[2,4,1]);await app.run('settleRatingSession()');await app.run('settleRatingSession()');
  assert.equal(sends(app).length,1);assert.equal(app.confirmed,true);assert.equal(app.state.crypto.rating_session,null);assert.equal(app.state.crypto.balance_units,'993000000');
  assert.deepEqual(JSON.parse(app.calls.find(call=>call.url==='/api/crypto/transaction').options.body),{action:'rate_session',expected_session_id:sessionId,expected_rating_count:3,expected_total_units:'7000000'});
  const saved=app.calls.find(call=>call.url==='/api/crypto/pending'&&call.options.body!=='null');assert.deepEqual(JSON.parse(saved.options.body),{action:'rate_session',hash,session_id:sessionId});
});
test('a locked built-in wallet can queue ratings and opens unlock at checkout',async()=>{
  const app=fixture();app.state.crypto.local_wallet.unlocked=false;await queue(app,[2,3]);assert.equal(app.walletCalls.length,0);assert.equal(app.state.crypto.rating_session.total_units,'5000000');
  await app.run('settleRatingSession()');assert.equal(app.elements.get('wallet-dialog').open,true);assert.equal(app.state.crypto.rating_session.status,'draft');assert.equal(sends(app).length,0);assert.match(app.elements.get('rating-session-error').textContent,/Unlock your built-in wallet/);
});
test('explicit End session pauses first and checks out once; leaving Rate retains an unpaid draft',async()=>{
  const app=fixture();await queue(app,[2]);await app.run('toggleRatingSession()');assert.equal(app.state.session,false);assert.equal(sends(app).length,1);assert.ok(app.calls.findIndex(call=>call.url==='/api/session')<app.calls.findIndex(call=>call.url==='/api/crypto/transaction'));
  const leaving=fixture();await queue(leaving,[2]);await leaving.run('navigate("home")');assert.equal(leaving.state.session,false);assert.equal(sends(leaving).length,0);assert.equal(leaving.state.crypto.rating_session.total_units,'2000000');assert.equal(leaving.elements.get('rating-session-panel').hidden,false);
});
test('wallet rejection unlocks the same unpaid draft and preserves every rating',async()=>{
  const app=fixture();await queue(app,[2,3]);app.run('reviewNativeTransaction=async()=>{const error=Error("User rejected transaction");error.code=4001;throw error;}');
  await app.run('settleRatingSession()');assert.equal(app.state.crypto.rating_session.status,'draft');assert.equal(app.state.crypto.rating_session.ratings.length,2);assert.equal(app.state.crypto.pending_transaction,null);assert.equal(app.calls.filter(call=>call.url==='/api/rating-session/unlock').length,1);assert.equal(app.confirmed,false);
});
test('builder estimate failure leaves the unpaid draft without unlocking another preparation',async()=>{
  const app=fixture();await queue(app,[2]);const original=app.respond;app.respond=async(url,options)=>{if(url==='/api/crypto/transaction'){return {ok:false,json:async()=>({error:'Gas estimate failed'})};}return original(url,options);};
  await app.run('settleRatingSession()');assert.equal(app.state.crypto.rating_session.status,'draft');assert.equal(sends(app).length,0);assert.equal(app.state.crypto.rating_session.ratings.length,1);assert.equal(app.calls.filter(call=>call.url==='/api/rating-session/unlock').length,0);
});
test('unknown send failures freeze preparation and cannot silently discard or edit the batch',async()=>{
  const app=fixture();await queue(app,[2]);app.send=async()=>{throw Error('Wallet network response lost');};
  await app.run('settleRatingSession()');assert.equal(app.state.crypto.rating_session.status,'prepared');assert.equal(app.calls.filter(call=>call.url==='/api/rating-session/unlock').length,0);
  const count=app.calls.length;await app.run('rate(3)');await app.run('discardRatingSession()');assert.equal(app.calls.length,count);assert.equal(app.state.crypto.rating_session.ratings.length,1);
});
test('confirmation retry imports the submitted batch without another payment or edited ratings',async()=>{
  const app=fixture();await queue(app,[2,3]);app.confirmFailure=true;await app.run('settleRatingSession()');assert.equal(sends(app).length,1);assert.equal(app.state.crypto.rating_session.status,'submitted');
  const before=app.calls.length;await app.run('rate(1)');assert.equal(app.calls.length,before);app.confirmFailure=false;await app.run('settleRatingSession()');assert.equal(sends(app).length,1);assert.equal(app.state.crypto.rating_session,null);assert.equal(app.storage.size,0);
});
test('canonical reverted submission resets to the unpaid draft without generic pending clearing',async()=>{
  const app=fixture();await queue(app,[2,3]);app.reverted=true;app.receipt=async()=>({status:'0x0'});
  await app.run('settleRatingSession()');assert.equal(app.state.crypto.rating_session.status,'draft');assert.equal(app.state.crypto.rating_session.ratings.length,2);assert.equal(app.state.crypto.pending_transaction,null);assert.equal(app.calls.filter(call=>call.url==='/api/rating-session/reset').length,1);assert.equal(app.calls.filter(call=>call.url==='/api/crypto/pending'&&call.options.body==='null').length,0);
});
test('a full 100-rating batch checks out in one transaction and a 101st rating cannot queue',async()=>{
  const app=fixture(101);await queue(app,Array(100).fill(1));assert.equal(app.walletCalls.length,0);assert.equal(app.state.crypto.rating_session.ratings.length,100);const count=app.calls.length;await app.run('rate(1)');assert.equal(app.calls.length,count);assert.equal(app.scoreButtons[1].disabled,true);
  await app.run('settleRatingSession()');assert.equal(sends(app).length,1);assert.equal(app.state.crypto.rating_session,null);
});
test('queued reservations reduce available balance and prevent another rating or roll from overspending',async()=>{
  const app=fixture();app.state.crypto.balance_units='5000000';await queue(app,[3]);assert.equal(app.elements.get('balance').textContent,'2.00 ✨');assert.equal(app.scoreButtons[3].disabled,true);const before=app.calls.length;await app.run('rate(3)');assert.equal(app.calls.length,before);
  app.run('page="roll"');app.elements.get('roll-wager').value='3';await app.run('roll()');assert.equal(app.calls.length,before);assert.equal(app.walletCalls.length,0);
});
test('opening wallet management keeps queued ratings and the built-in account',async()=>{
  const app=fixture();await queue(app,[2]);const before=app.calls.length;
  await app.run('connectWallet()');assert.equal(app.elements.get('wallet-dialog').open,true);assert.equal(app.calls.length,before);assert.equal(app.state.crypto.wallet,address);assert.equal(app.state.crypto.rating_session.total_units,'2000000');
});
test('a submitted batch restored after restart only confirms its saved hash',async()=>{
  const app=fixture();await queue(app,[2,3]);app.state.crypto.rating_session.status='submitted';app.state.crypto.pending_transaction={action:'rate_session',hash,session_id:sessionId};app.run('pendingTransaction=null;renderWallet();');assert.equal(app.run('pendingTransaction.session_id'),sessionId);
  await app.run('retryTransaction()');assert.equal(sends(app).length,0);assert.equal(app.state.crypto.rating_session,null);
});
test('discard only clears an unsigned draft and free skips work with no selected wallet',async()=>{
  const app=fixture();await queue(app,[2]);await app.run('discardRatingSession()');assert.equal(app.state.crypto.rating_session,null);assert.equal(app.state.crypto.balance_units,'1000000000');assert.equal(app.walletCalls.length,0);
  app.state.crypto.wallet=null;await app.run('rate(0)');assert.equal(app.state.crypto.rating_session,null);assert.equal(app.walletCalls.length,0);
});
test('queue edits and a second checkout stay blocked while the wallet signature is pending',async()=>{
  const app=fixture();await queue(app,[2]);let resolveSend;app.send=()=>new Promise(resolve=>{resolveSend=resolve;});
  const checkout=app.run('settleRatingSession()');await until(()=>!!resolveSend);const before=app.calls.length;await app.run('rate(3)');await app.run('settleRatingSession()');await app.run('discardRatingSession()');assert.equal(app.calls.length,before);assert.equal(sends(app).length,1);assert.equal(app.state.crypto.rating_session.ratings.length,1);
  resolveSend({hash});await checkout;assert.equal(sends(app).length,1);
});

test('rapid End session clicks stop once and cannot restart seeding or send twice',async()=>{
  const app=fixture();await queue(app,[2]);await app.run('Promise.all([toggleRatingSession(),toggleRatingSession()])');
  assert.equal(app.state.session,false);assert.equal(app.calls.filter(call=>call.url==='/api/session').length,1);assert.equal(sends(app).length,1);
});

test('a stale checkout cannot approve calldata for a different frozen session',async()=>{
  const app=fixture();await queue(app,[2]);const original=app.respond;
  app.respond=async(url,options)=>{const response=await original(url,options);if(url==='/api/crypto/transaction')return {ok:true,json:async()=>({...await response.json(),data:'0x12345678'+'c'.repeat(64)+'0'.repeat(64)})};return response;};
  await app.run('settleRatingSession()');assert.equal(sends(app).length,0);assert.equal(app.state.crypto.rating_session.status,'draft');assert.match(app.elements.get('rating-session-error').textContent,/session changed before checkout/);
});

test('session review uses clip titles and escapes user-provided text',async()=>{
  const app=fixture();app.state.queue[0].title='Funny birds <script>unsafe</script>';await queue(app,[2]);
  assert.match(app.elements.get('rating-session-items').innerHTML,/Funny birds &lt;script&gt;unsafe&lt;\/script&gt;/);assert.doesNotMatch(app.elements.get('rating-session-items').innerHTML,/<script>/);
});
test('manual session recovery clears the hash newly persisted by the server even with no local pending record',async()=>{
  const app=fixture();await queue(app,[2,3]);app.state.crypto.rating_session.status='prepared';app.elements.get('rating-session-hash').value=hash;app.run('pendingTransaction=null;');
  const original=app.respond;app.respond=async(url,options)=>{const response=await original(url,options);if(url==='/api/rating-session/confirm')app.state.crypto.pending_transaction={action:'rate_session',hash,session_id:sessionId};return response;};
  await app.run('recoverRatingSession()');assert.equal(sends(app).length,0);assert.equal(app.state.crypto.pending_transaction,null);assert.equal(app.state.crypto.rating_session,null);assert.ok(app.calls.some(call=>call.url==='/api/crypto/pending'&&call.options.body==='null'));
});
test('a concurrent append causes the server snapshot check to reject before any wallet send',async()=>{
  const app=fixture();await queue(app,[2]);const original=app.respond;
  app.respond=async(url,options)=>{if(url==='/api/crypto/transaction'){app.state.crypto.rating_session.ratings.push({clip_id:clip(99).id,score:3,title:'Added in another window'});app.state.crypto.rating_session.total_units='5000000';}return original(url,options);};
  await app.run('settleRatingSession()');assert.equal(sends(app).length,0);assert.equal(app.state.crypto.rating_session.status,'draft');assert.match(app.elements.get('rating-session-error').textContent,/queued session changed/);
  const request=JSON.parse(app.calls.find(call=>call.url==='/api/crypto/transaction').options.body);assert.equal(request.expected_rating_count,1);assert.equal(request.expected_total_units,'2000000');
});

test('a builder rejection cannot unlock another window’s prepared wallet transaction',async()=>{
  const app=fixture();await queue(app,[2]);const original=app.respond;
  app.respond=async(url,options)=>{if(url==='/api/crypto/transaction'){app.state.crypto.rating_session.status='prepared';return {ok:false,json:async()=>({error:'Another window is preparing this session'})};}return original(url,options);};
  await app.run('settleRatingSession()');assert.equal(sends(app).length,0);assert.equal(app.state.crypto.rating_session.status,'prepared');assert.equal(app.calls.filter(call=>call.url==='/api/rating-session/unlock').length,0);assert.equal(app.elements.get('discard-rating-session').disabled,true);
});

test('a locked built-in wallet confirms a saved submitted session inside the app',async()=>{
  const app=fixture();await queue(app,[2]);app.state.crypto.rating_session.status='submitted';app.state.crypto.pending_transaction={action:'rate_session',hash,session_id:sessionId};app.state.crypto.local_wallet.unlocked=false;app.run('pendingTransaction=null;renderWallet();');
  await app.run('settleRatingSession()');assert.equal(app.state.crypto.rating_session,null);assert.equal(sends(app).length,0);assert.equal(app.state.crypto.local_wallet.unlocked,false);assert.equal(app.walletCalls.filter(call=>call.method==='rebroadcast').length,1);
});

test('a prepared session restored after restart cannot open a second wallet prompt',async()=>{
  const app=fixture();await queue(app,[2]);app.state.crypto.rating_session.status='prepared';app.run('renderWallet();');
  assert.equal(app.elements.get('settle-rating-session').disabled,true);assert.equal(app.elements.get('rating-session-recovery').open,true);assert.equal(app.elements.get('unlock-rating-session').hidden,false);
  const before=app.calls.length;await app.run('settleRatingSession()');await assert.rejects(app.run('sendCryptoTransaction("rate_session")'),/prepared session cannot open another/);
  assert.equal(app.calls.length,before);assert.equal(app.walletCalls.length,0);assert.equal(app.state.crypto.rating_session.status,'prepared');
});
test('explicitly confirming cancelled wallet approval returns the same unpaid draft to editing',async()=>{
  const app=fixture();await queue(app,[2,3]);app.state.crypto.rating_session.status='prepared';app.run('renderWallet();');
  await app.run('unlockRatingSession()');assert.equal(app.state.crypto.rating_session.status,'draft');assert.equal(app.state.crypto.rating_session.ratings.length,2);assert.equal(app.state.crypto.rating_session.total_units,'5000000');assert.equal(app.walletCalls.length,0);assert.equal(app.calls.filter(call=>call.url==='/api/rating-session/unlock').length,1);assert.equal(app.elements.get('unlock-rating-session').hidden,true);assert.equal(app.elements.get('discard-rating-session').disabled,false);
});
test('native prepared-session recovery remains visible and never resubmits approval',async()=>{
  const app=fixture();await queue(app,[2]);app.state.crypto.rating_session.status='prepared';app.run('renderWallet();');
  assert.equal(app.elements.get('rating-session-recovery').open,true);assert.equal(app.elements.get('rating-session-recover').disabled,false);assert.equal(app.elements.get('settle-rating-session').disabled,true);assert.match(app.elements.get('rating-session-help').textContent,/Reject any open wallet approval first/);
  app.elements.get('rating-session-hash').value=hash;await app.run('recoverRatingSession()');assert.equal(app.state.crypto.rating_session,null);assert.equal(app.walletCalls.length,0);
});


test('seeder reward previews allow ratings without prefunding and reserve only the score',async()=>{
  const app=fixture(),preview={seeders:5,population:100,reward_units:['0',...Array(9).fill('2570331')]};
  app.state.queue[0].beneficiary=null;app.state.queue[0].distribution_preview=preview;
  app.state.crypto.rating_credit_units='0';app.run('render()');
  assert.equal(app.scoreButtons[5].disabled,false);
  assert.doesNotMatch(app.elements.get('rating-recipient').textContent,/Fund your rating budget/);
  await app.run('rate(5)');
  assert.equal(app.state.crypto.rating_session.total_units,'5000000');assert.equal(app.walletCalls.length,0);
});

test('session review separates the rater charge from every seeder reward and escapes wallet text',async()=>{
  const app=fixture();await queue(app,[5]);const session=app.state.crypto.rating_session;
  session.payout_units='12851655';session.mint_required_units='7851655';
  session.ratings[0].distribution={population:100,reward_units:'2570331',recipients:Array.from({length:5},(_,i)=>({peer:`peer-${i}`,wallet:i?'0x'+String(i).repeat(40):'<unsafe wallet>'}))};
  app.run('render()');
  assert.match(app.elements.get('rating-session-summary').textContent,/5\.00 ✨ total/);
  assert.match(app.elements.get('rating-session-available').textContent,/12\.851655 ✨/);
  assert.match(app.elements.get('rating-session-available').textContent,/7\.851655 ✨ estimated new tokens minted/);
  assert.match(app.elements.get('rating-session-items').innerHTML,/2\.570331 ✨ each/);
  assert.match(app.elements.get('rating-session-items').innerHTML,/&lt;unsafe wallet&gt;/);
  await app.run('settleRatingSession()');assert.equal(sends(app).length,1);assert.equal(app.state.crypto.balance_units,'995000000');
});
