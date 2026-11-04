'use strict';
const $ = id => document.getElementById(id);
const token = document.querySelector('meta[name="api-token"]').content;
const esc = value => String(value).replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
let lastNotice=null;
let state=null, page='home', current=null, busy=false, uploading=false, settingsDirty=false, initialized=false, toastTimer;
let rollBusy=false, rollMediaKey='', rollStartedKey='';
let updateStatus=null, updateBusy=false, updateOpening=false, updateNoticeVersion=null, updateTask=null;
let refreshVersion=0;
let navigationQueue=Promise.resolve(),sessionToggleBusy=false,ratingRecoveryState=null;
async function api(path, body, method='POST') {
  const options={method,headers:{'x-clipcrop-token':token}};
  if(body instanceof FormData) options.body=body;
  else if(body!==undefined){options.headers['Content-Type']='application/json';options.body=JSON.stringify(body);}
  const response=await fetch(`/api/${path}`,options);
  let data;try{data=await response.json();}catch{throw new Error(response.status===413?'Video exceeds the 8 MB limit.':'The local client could not complete this request.');}
  if(!response.ok) throw new Error(data.error||'Request failed.');
  return data;
}
function toast(message){$('toast').textContent=message;$('toast').hidden=false;clearTimeout(toastTimer);toastTimer=setTimeout(()=>$('toast').hidden=true,6000);}
function mediaUrl(id){return `/api/video/${encodeURIComponent(id)}?token=${token}`;}
const TOKEN_SCALE=1000000n;
const MAX_TOKEN_UNITS=(1n<<256n)-1n;
let walletBusy=false, transactionBusy=false, deferredWalletAddress=undefined, walletAccount=null, walletPhase='', pendingTransaction=null;
function tokenUnits(raw){
  const text=String(raw??'').trim();
  if(!/^(?:\d+(?:\.\d{0,6})?|\.\d{1,6})$/.test(text))return null;
  const [whole,fraction='']=text.split('.');
  const amount=BigInt(whole||'0')*TOKEN_SCALE+BigInt(fraction.padEnd(6,'0'));
  return amount>0n&&amount<=MAX_TOKEN_UNITS?amount:null;
}
function fmtTokens(units,minimum=2){
  let value;try{value=BigInt(units??'0');}catch{return '—';}
  const negative=value<0n;value=negative?-value:value;
  const whole=(value/TOKEN_SCALE).toString().replace(/\B(?=(\d{3})+(?!\d))/g,',');
  const fraction=(value%TOKEN_SCALE).toString().padStart(6,'0').replace(/0+$/,'').padEnd(minimum,'0');
  return `${negative?'−':''}${whole}${fraction?`.${fraction}`:''}`;
}
function walletProvider(){return window.ethereum?.request?window.ethereum:null;}
function walletAvailable(){return !!state?.crypto?.configured&&!state.crypto.error&&!!state.crypto.wallet&&!!walletProvider()&&walletAccount?.toLowerCase()===state.crypto.wallet.toLowerCase();}
function currentTokenBalance(){try{return BigInt(state?.crypto?.balance_units||'0');}catch{return 0n;}}
function ratingSession(){return state?.crypto?.rating_session||null;}
function queuedRatingCount(){return ratingSession()?.ratings?.length||0;}
function queuedTokenUnits(){try{return BigInt(ratingSession()?.total_units||'0');}catch{return 0n;}}
function spendableTokenBalance(){const balance=currentTokenBalance(),reserved=queuedTokenUnits();return balance>reserved?balance-reserved:0n;}
function ratingWalletSelected(){return !!state?.crypto?.configured&&!state.crypto.error&&/^0x[a-fA-F0-9]{40}$/.test(state.crypto.wallet||'');}
function walletSelectionLocked(){return queuedRatingCount()>0||!!pendingTransaction;}
function ratingDraftEditable(){return !ratingSession()||ratingSession().status==='draft';}
function renderRatingSession(){
  if(!state)return;
  const session=ratingSession(),count=queuedRatingCount(),submitted=session?.status==='submitted',prepared=session?.status==='prepared';
  $('rating-session-panel').hidden=!count;
  $('rating-session-status').textContent=submitted?'Submitted · awaiting confirmation':prepared?'Prepared · not yet confirmed':'Queued · not paid';
  $('rating-session-summary').textContent=`${count} rating${count===1?'':'s'} · ${fmtTokens(queuedTokenUnits())} ✨ total`;
  $('rating-session-available').textContent=`${fmtTokens(spendableTokenBalance())} ✨ available after queued ratings. Up to 100 ratings settle together.`;
  $('rating-session-items').innerHTML=(session?.ratings||[]).map(item=>`<li><span class="session-item-title" title="${esc(item.clip_id)}">${esc(item.title||item.clip_id)}</span><span>${item.score} ✨</span></li>`).join('');
  $('settle-rating-session').disabled=busy||walletBusy||rollBusy||transactionBusy||!count||!ratingWalletSelected()||prepared&&!pendingTransaction||!!pendingTransaction&&!['rate_session'].includes(pendingTransaction.action);
  $('settle-rating-session').textContent=submitted||pendingTransaction?.action==='rate_session'?'Retry session confirmation':`Settle session · ${fmtTokens(queuedTokenUnits())} ✨`;
  $('discard-rating-session').disabled=busy||walletBusy||rollBusy||transactionBusy||!count||!ratingDraftEditable()||!!pendingTransaction;
  $('rating-session-recover').disabled=busy||walletBusy||rollBusy||transactionBusy||!count;
  $('unlock-rating-session').hidden=!prepared||!!pendingTransaction;
  $('unlock-rating-session').disabled=busy||walletBusy||rollBusy||transactionBusy;
  const recoveryState=session?`${session.id}:${session.status}`:null;
  if(recoveryState!==ratingRecoveryState){ratingRecoveryState=recoveryState;if(prepared||submitted)$('rating-session-recovery').open=true;}
  $('rating-session-help').textContent=submitted?'Your submitted batch is frozen. Confirm its transaction before continuing; retrying confirmation never pays again.':prepared?'Wallet approval is unresolved. Recover its transaction hash below if it was submitted. Reject any open wallet approval first. Then return this unpaid session to editing.':'Ratings are saved on this device and remain unpaid until one session transaction is confirmed. Leaving Rate keeps the queue.';
}
async function applyDeferredWallet(){
  if(deferredWalletAddress===undefined)return;
  const address=deferredWalletAddress;deferredWalletAddress=undefined;
  if(walletSelectionLocked()&&address?.toLowerCase()!==state.crypto?.wallet?.toLowerCase()){
    toast('Settle or discard the queued ratings and confirm pending transactions before switching wallets.');return;
  }
  pendingTransaction=null;
  try{await api('wallet',{address});await refresh();}catch(error){toast(error.message);}
}
async function resetRevertedTransaction(record){
  if(record?.action==='rate_session'){
    await api('rating-session/reset',{transaction_hash:record.hash});
    if(state?.crypto)state.crypto.pending_transaction=null;
    rememberTransaction(null);await refresh();
  }else await forgetTransaction();
}
async function settleRatingSession(){
  if(busy||walletBusy||rollBusy||transactionBusy||!queuedRatingCount())return;
  if(ratingSession().status==='prepared'&&!pendingTransaction){toast('This session already has an unresolved wallet approval. Recover its transaction or cancel that approval before returning to editing.');return;}
  if(!walletProvider()){await openWalletBrowser();toast('Your saved session is ready in the wallet browser. Settle it once or confirm its submitted transaction.');return;}
  if(pendingTransaction?.action==='rate_session'){await retryTransaction();return;}
  if(ratingSession().status==='submitted'){toast('Recover the submitted session transaction hash to confirm its payment.');return;}
  busy=true;$('rating-session-error').hidden=true;render();
  try{await sendCryptoTransaction('rate_session');toast('Session settled. All queued ratings are paid.');}
  catch(error){$('rating-session-error').textContent=error.message;$('rating-session-error').hidden=false;}
  finally{busy=false;walletPhase='';render();}
}
async function discardRatingSession(){
  if(busy||walletBusy||rollBusy||transactionBusy||!queuedRatingCount()||!ratingDraftEditable()||pendingTransaction)return;
  busy=true;render();
  try{await api('rating-session/cancel',{});refreshVersion++;await refresh();toast('Queued ratings discarded. No tokens were paid.');}
  catch(error){$('rating-session-error').textContent=error.message;$('rating-session-error').hidden=false;}
  finally{busy=false;render();}
}
async function unlockRatingSession(){
  if(busy||walletBusy||rollBusy||transactionBusy||ratingSession()?.status!=='prepared'||pendingTransaction)return;
  busy=true;$('rating-session-error').hidden=true;render();
  try{await api('rating-session/unlock',{});refreshVersion++;await refresh();toast('The unpaid session is ready for editing. No tokens were sent.');}
  catch(error){$('rating-session-error').textContent=error.message;$('rating-session-error').hidden=false;}
  finally{busy=false;render();}
}
async function recoverRatingSession(){
  if(busy||walletBusy||rollBusy||transactionBusy||!queuedRatingCount())return;
  const hash=$('rating-session-hash').value.trim();
  if(!/^0x[a-fA-F0-9]{64}$/.test(hash)){toast('Enter the session’s complete transaction hash.');return;}
  busy=true;render();
  try{
    await api('rating-session/confirm',{transaction_hash:hash});refreshVersion++;
    await forgetTransaction();
    await refresh();toast('Session payment confirmed. No new transaction was sent.');
  }catch(error){$('rating-session-error').textContent=error.message;$('rating-session-error').hidden=false;}
  finally{busy=false;render();}
}
async function toggleRatingSession(){
  if(sessionToggleBusy||busy||walletBusy||rollBusy||transactionBusy)return;
  sessionToggleBusy=true;
  return queueNavigation(async()=>{
    try{
      if(page!=='rate'||!state)return;
      render();const ending=state.session;await api('session',{active:!ending});await refresh();
      if(ending&&queuedRatingCount())await settleRatingSession();
    }finally{sessionToggleBusy=false;render();}
  });
}

function storageKey(record){const crypto=record||state?.crypto;return crypto?`clipclub:transaction:${crypto.chain_id}:${crypto.contract}:${crypto.wallet}`:null;}
function rememberTransaction(record){
  const key=storageKey(record||pendingTransaction);pendingTransaction=record;
  try{if(key){if(record)window.localStorage?.setItem(key,JSON.stringify(record));else window.localStorage?.removeItem(key);}}catch{}
  renderWallet();
}
function restoreTransaction(){
  if(pendingTransaction||!state?.crypto?.wallet)return;
  try{
    const record=state.crypto.pending_transaction||JSON.parse(window.localStorage?.getItem(storageKey())||'null');
    if(record&&/^0x[a-fA-F0-9]{64}$/.test(record.hash)&&['request_roll','settle_roll','rate_clip','rate_session','register_clip'].includes(record.action))pendingTransaction={...record,chain_id:state.crypto.chain_id,contract:state.crypto.contract,wallet:state.crypto.wallet};
  }catch{}
}
function pendingServerRecord(record){const result={action:record.action,hash:record.hash};for(const key of ['wager_units','roll_id','clip_id','score','session_id'])if(record[key]!==undefined)result[key]=record[key];return result;}
async function forgetTransaction(){
  await api('crypto/pending',null);
  if(state?.crypto)state.crypto.pending_transaction=null;
  rememberTransaction(null);
}
async function openWalletBrowser(){try{await api('wallet/browser',{});toast('Opened the app in your default browser. Connect a MegaETH wallet there.');}catch(error){toast(error.message);}}
async function requireWallet(){
  const crypto=state?.crypto,provider=walletProvider();
  if(!crypto?.configured)throw Error('The 좋아 contract has not been configured. See Settings for deployment status.');
  if(crypto.error)throw Error(crypto.error);
  if(!provider)throw Error('Open the app in a browser with an Ethereum wallet, then connect it.');
  const accounts=await provider.request({method:'eth_accounts'});
  if(!accounts?.[0]||accounts[0].toLowerCase()!==crypto.wallet?.toLowerCase())throw Error('Connect the wallet you want to use before signing.');
  walletAccount=accounts[0];
  const chain=await provider.request({method:'eth_chainId'});
  if(BigInt(chain)!==BigInt(crypto.chain_id)){
    try{await provider.request({method:'wallet_switchEthereumChain',params:[{chainId:crypto.chain_hex}]});}
    catch(error){
      if(Number(error.code)!==4902)throw error;
      await provider.request({method:'wallet_addEthereumChain',params:[{chainId:crypto.chain_hex,chainName:crypto.chain_name,nativeCurrency:{name:'Ether',symbol:'ETH',decimals:18},rpcUrls:[crypto.rpc_url],blockExplorerUrls:[crypto.explorer_url]}]});
    }
  }
  if(BigInt(await provider.request({method:'eth_chainId'}))!==BigInt(crypto.chain_id))throw Error('Switch your wallet to the configured MegaETH network.');
  return provider;
}
async function connectWallet(){
  if(walletBusy||transactionBusy||rollBusy||busy)return;
  if(!walletProvider()){await openWalletBrowser();return;}
  walletBusy=true;renderWallet();
  try{
    const accounts=await walletProvider().request({method:'eth_requestAccounts'});
    if(!/^0x[a-fA-F0-9]{40}$/.test(accounts?.[0]||''))throw Error('No Ethereum wallet account was selected.');
    if(walletSelectionLocked()&&accounts[0].toLowerCase()!==state.crypto?.wallet?.toLowerCase())throw Error('Settle or discard queued ratings before switching wallets.');
    await api('wallet',{address:accounts[0]});walletAccount=accounts[0];
    await refresh();await requireWallet();toast('Wallet connected. Review token transactions in your wallet.');
  }catch(error){toast(error.message||'Wallet connection was cancelled.');}
  finally{walletBusy=false;render();}
}
function renderWallet(){
  if(!state)return;
  const crypto=state.crypto;
  restoreTransaction();
  $('balance').textContent=crypto?.wallet?`${fmtTokens(spendableTokenBalance())} ✨`:'— ✨';
  $('reserved-balance').hidden=!queuedRatingCount();$('reserved-balance').textContent=`${fmtTokens(queuedTokenUnits())} ✨ queued · not paid`;
  $('balance').title=crypto?.wallet?`Wallet balance ${fmtTokens(crypto.balance_units)} ✨; available after queued ratings`:'';
  $('connect-wallet').disabled=walletBusy||transactionBusy||rollBusy||busy;
  $('open-wallet-browser').disabled=transactionBusy||rollBusy||busy;
  $('connect-wallet').textContent=walletBusy?'Connecting…':walletAvailable()?`${walletAccount.slice(0,6)}…${walletAccount.slice(-4)}`:walletProvider()?'Connect wallet':'Open wallet browser ↗';
  $('crypto-network').textContent=crypto?`${crypto.chain_name} · ${crypto.chain_id}`:'MegaETH';
  $('crypto-wallet').textContent=crypto?.wallet||'Disconnected';
  $('crypto-contract').textContent=crypto?.contract||'Not configured';
  $('wallet-status').textContent=walletPhase||crypto?.error||(!crypto?.configured?'The 좋아 token has not been deployed and configured for this app.':!crypto.wallet?'Connect your wallet to see your ✨ balance and sign transactions.':!walletAvailable()?'This is the last selected wallet. Connect it in this browser to sign.':'Your wallet signs each transaction. Roll settlement uses verified drand randomness.');
  $('stat-earned').textContent=crypto?.configured?`${fmtTokens(crypto.total_supply_units)} ✨`:'—';
  $('pending-transaction').hidden=!pendingTransaction;
  $('pending-transaction-detail').textContent=pendingTransaction?`Unconfirmed ${pendingTransaction.action.replaceAll('_',' ')} · ${pendingTransaction.hash}. Retry confirmation before sending another transaction.`:'';
  $('retry-transaction').disabled=walletBusy||transactionBusy||rollBusy||busy;
  renderRatingSession();
}
async function waitForReceipt(hash,provider){
  for(let attempt=0;attempt<120;attempt++){
    const receipt=await provider.request({method:'eth_getTransactionReceipt',params:[hash]});
    if(receipt){if(BigInt(receipt.status??'0x0')!==1n){const error=Error('The transaction reverted. Your wallet can show the reason.');error.code='TRANSACTION_REVERTED';throw error;}return receipt;}
    await new Promise(resolve=>setTimeout(resolve,1000));
  }
  throw Error('This transaction is still waiting for inclusion. Use Retry confirmation in Settings; do not submit it again.');
}
async function confirmTransaction(record,provider){
  walletPhase='Waiting for transaction inclusion…';renderWallet();
  await waitForReceipt(record.hash,provider);
  if(record.action==='request_roll'||record.action==='settle_roll'){
    const result=await api('roll',{transaction_hash:record.hash});refreshVersion++;
    state.roll=state.roll?.id===result.id&&state.roll.clip?{...result,clip:state.roll.clip}:result;
  }else if(record.action==='rate_session')await api('rating-session/confirm',{transaction_hash:record.hash});
  else if(record.action==='rate_clip')await api('rate',{id:record.clip_id,score:record.score,transaction_hash:record.hash});
  await forgetTransaction();walletPhase='';await refresh();
}
async function sendCryptoTransaction(action,fields={}){
  if(transactionBusy)throw Error('Another token transaction is in progress.');
  if(pendingTransaction)throw Error('A previous transaction needs confirmation. Open Settings and select Retry confirmation.');
  let preparationOwned=false,signatureRequested=false,sessionId=null,submittedHash=null,sessionReview=null;
  if(action==='rate_session'&&ratingSession()?.status!=='draft')throw Error('A prepared session cannot open another wallet approval. Recover its transaction or cancel the existing approval first.');
  if(action==='rate_session'){sessionId=ratingSession()?.id;if(!/^[a-fA-F0-9]{64}$/.test(sessionId||''))throw Error('No saved rating session is available for checkout.');sessionReview={expected_session_id:sessionId,expected_rating_count:queuedRatingCount(),expected_total_units:queuedTokenUnits().toString()};}
  transactionBusy=true;renderWallet();
  try{
    const provider=await requireWallet(),crypto={...state.crypto};
    walletPhase='Preparing transaction…';renderWallet();
    const transaction=await api('crypto/transaction',{action,...(sessionReview||fields)});
    preparationOwned=action==='rate_session';
    if(preparationOwned&&ratingSession()?.id===sessionId){state.crypto.rating_session.status='prepared';renderWallet();}
    if(transaction.to?.toLowerCase()!==crypto.contract.toLowerCase()||transaction.from?.toLowerCase()!==crypto.wallet.toLowerCase()||BigInt(transaction.chainId)!==BigInt(crypto.chain_id)||BigInt(transaction.value||'0x0')!==0n||!/^0x[0-9a-fA-F]*$/.test(transaction.data||''))throw Error('The transaction does not match your selected MegaETH token and wallet.');
    if(action==='rate_session'&&transaction.data.slice(10,74).toLowerCase()!==sessionId.toLowerCase())throw Error('The queued session changed before checkout. Review the refreshed session and try again.');
    await requireWallet();
    if(state.crypto.wallet?.toLowerCase()!==crypto.wallet.toLowerCase()||state.crypto.contract?.toLowerCase()!==crypto.contract.toLowerCase()||state.crypto.chain_id!==crypto.chain_id)throw Error('The selected wallet or network changed. Review the selection and try again.');
    walletPhase='Approve the transaction in your wallet…';renderWallet();
    signatureRequested=true;
    const hash=await provider.request({method:'eth_sendTransaction',params:[transaction]});
    if(!/^0x[a-fA-F0-9]{64}$/.test(hash||''))throw Error('The wallet did not return a valid transaction hash.');
    submittedHash=hash;
    const record={action,...fields,...(sessionId?{session_id:sessionId}:{}),hash,chain_id:crypto.chain_id,contract:crypto.contract,wallet:crypto.wallet};rememberTransaction(record);
    if(action==='rate_session'&&ratingSession()?.id===sessionId)state.crypto.rating_session.status='submitted';
    try{await api('crypto/pending',pendingServerRecord(record));state.crypto.pending_transaction=pendingServerRecord(record);}
    catch{toast(`Transaction ${hash} is saved in this browser. The local backup failed; keep this browser open until confirmation.`);}
    await confirmTransaction(record,provider);
  }catch(error){
    // A reverted transaction has no effects; a missing confirmation stays recoverable.
    if(error.code==='TRANSACTION_REVERTED')try{await resetRevertedTransaction(pendingTransaction);}catch{toast('The reverted transaction still needs server verification. Retry confirmation in Settings.');}
    else if(action==='rate_session'&&preparationOwned&&!submittedHash&&(!signatureRequested||Number(error.code)===4001||error.code==='ACTION_REJECTED')){
      try{await api('rating-session/unlock',{});await refresh();}catch{toast('Your unpaid session is still prepared. Retry checkout or recover its transaction.');}
    }else if(action==='rate_session'&&!preparationOwned&&!signatureRequested)await refresh();
    throw error;
  }finally{
    transactionBusy=false;walletPhase='';
    await applyDeferredWallet();
    renderWallet();
  }
}
async function retryTransaction(){
  if(!pendingTransaction||walletBusy||transactionBusy||rollBusy||busy)return;
  walletBusy=true;transactionBusy=true;renderWallet();
  try{await confirmTransaction(pendingTransaction,await requireWallet());toast('Transaction confirmed.');}
  catch(error){if(error.code==='TRANSACTION_REVERTED')try{await resetRevertedTransaction(pendingTransaction);}catch{}toast(error.message);}
  finally{walletPhase='';walletBusy=false;transactionBusy=false;await applyDeferredWallet();render();}
}
async function recoverRoll(){
  if(rollBusy||busy||walletBusy||transactionBusy)return;
  const hash=$('recover-roll-hash').value.trim();
  if(!/^0x[a-fA-F0-9]{64}$/.test(hash)){toast('Enter a 0x-prefixed transaction hash with 64 hexadecimal characters.');return;}
  rollBusy=true;renderRoll();
  try{const result=await api('roll',{transaction_hash:hash});refreshVersion++;state.roll=result;if(pendingTransaction&&['request_roll','settle_roll'].includes(pendingTransaction.action)&&pendingTransaction.hash.toLowerCase()===hash.toLowerCase())await forgetTransaction();await refresh();toast('Roll loaded from MegaETH.');}
  catch(error){toast(error.message);}finally{rollBusy=false;renderRoll();}
}

function duration(seconds){return `${Math.floor(seconds/60)}:${String(Math.floor(seconds%60)).padStart(2,'0')}`;}
function sortLibrary(library,sort){
  return [...library].sort((a,b)=>{
    const recent=(b.added_at_ms||0)-(a.added_at_ms||0);
    const tie=recent||a.clip.id.localeCompare(b.clip.id);
    if(sort==='rating')return (b.average_rating??-1)-(a.average_rating??-1)||tie;
    return tie;
  });
}
function renderLibrary(){
  if(!state)return;
  const library=sortLibrary(state.library,$('library-sort').value);
  const markup=library.length?library.map(({clip:c,seeding,beneficiary,average_rating,rating_count})=>`<div class="library-row"><button class="clip-thumb" data-preview="${c.id}" aria-label="Preview ${esc(c.title)}">▷</button><div class="row-detail"><h3>${esc(c.title)}</h3><p>${esc(c.extension.toUpperCase())} · ${duration(c.duration_secs)} · ${c.width} × ${c.height} · ${(c.size/1e6).toFixed(2)} MB</p><p class="clip-stats"><span title="Average of 1–9 ratings; skips are excluded.">${average_rating===null?'Unrated':`Rating <strong>${average_rating.toFixed(1)} / 9</strong> · ${rating_count} rating${rating_count===1?'':'s'}`}</span></p></div><span class="seeding-tag">${seeding?'● Seeding':'○ Unavailable'}</span><div class="row-actions">${beneficiary?`<span class="registered-token" title="${esc(beneficiary)}">✨ Registered</span>`:`<button data-register="${c.id}" ${walletAvailable()?'':'disabled'}>Register ✨ wallet</button>`}<button data-torrent="${c.id}">Torrent ↗</button><button data-remove="${c.id}">Remove</button></div></div>`).join(''):'<div class="library-empty"><span aria-hidden="true">▧</span><h3>No clips yet</h3></div>';
  if($('library').innerHTML!==markup)$('library').innerHTML=markup;
}
function queueNavigation(operation){
  // Session changes must finish in order, even when menu clicks arrive quickly.
  navigationQueue=navigationQueue.then(operation).catch(error=>toast(error.message));
  return navigationQueue;
}
function navigate(next){
  if(!['home','seed','rate','roll','settings'].includes(next))next='home';
  return queueNavigation(async()=>{
    if(next===page)return;
    const previous=page;
    page=next;
    if(document.body)document.body.dataset.page=page;
    document.querySelectorAll('.page').forEach(el=>el.hidden=el.id!==`page-${page}`);
    $('back-home').hidden=page==='home';
    $('breadcrumb').textContent={home:'Home',seed:'Upload',rate:'Rate',roll:'Roll',settings:'Settings'}[page];
    if(page!=='rate')$('player').pause();
    if(page!=='roll')$('roll-player').pause();
    const focusTarget=document.querySelector(page==='home'?`[data-page="${previous}"]`:`#page-${page} h1`);
    focusTarget?.setAttribute?.('tabindex',page==='home'?'0':'-1');
    focusTarget?.focus?.({preventScroll:true});
    window.scrollTo?.({top:0});
    if(previous==='rate'){
      try{await api('session',{active:false});}catch(error){toast(error.message);}
    }
    if(page==='rate'){
      try{await api('session',{active:true});}catch(error){toast(error.message);}
    }
    if(page==='settings')fillSettings(true);
    await refresh();
  });
}
function fillSettings(force=false){
  if(!state||(!force&&settingsDirty))return;
  $('display-name').value=state.settings.display_name;$('muted').checked=state.settings.muted;$('prefetch').checked=state.settings.prefetch;$('volume').value=state.settings.volume;$('bootstrap-peers').value=state.settings.bootstrap_peers.join('\n');
  document.querySelectorAll('#settings-form input, #settings-form textarea, #settings-form button').forEach(control=>control.disabled=false);
  if(force)settingsDirty=false;
}
function render(){
  if(!state)return;
  if(state.notice&&state.notice!==lastNotice){lastNotice=state.notice;toast(state.notice);}
  renderWallet();
  $('profile-name').textContent=state.settings.display_name;
  $('seed-count').textContent=state.library.length;$('ready-count').textContent=state.queue.length;$('queue-total').textContent=state.queue.length;
  $('peer-status').textContent=state.peers.length?`${state.peers.length} peer${state.peers.length===1?'':'s'} connected`:'Finding peers…';
  $('library-cap').textContent=`${state.library.length} / 100 clips`;
  $('stat-seeds').innerHTML=`${state.library.filter(item=>item.seeding).length} <small>clips</small>`;
  $('stat-storage').innerHTML=`${(state.library.reduce((sum,item)=>sum+item.clip.size,0)/1e6).toFixed(1)} <small>MB</small>`;
  $('collection-count').textContent=state.library.length;
  renderLibrary();
  $('queue-slots').innerHTML=Array.from({length:10},(_,i)=>`<span class="${i<state.queue.length?'ready':i===state.queue.length&&state.downloading?'downloading':''}">${i<state.queue.length?'✓':i+1}</span>`).join('');
  const d=state.downloading;
  const downloadStatus=d?`Downloading · ${Math.min(100,Math.round(d.downloaded/Math.max(1,d.total)*100))}%`:state.queue.length>=10?'Queue full':!state.session&&!state.settings.prefetch?'Downloads paused':state.peers.length?'Waiting for more clips…':'Waiting for peers…';
  if($('download-status').textContent!==downloadStatus)$('download-status').textContent=downloadStatus;
  $('session-button').disabled=sessionToggleBusy||busy||walletBusy||rollBusy||transactionBusy;
  $('session-button').textContent=state.session?`End session${state.temporary_seeds?` · ${state.temporary_seeds} seeding`:''} →`:'Start session →';
  const next=state.queue[0]||null;
  if(current?.id!==next?.id){
    current=next;
    const player=$('player');player.pause();
    if(next){player.src=mediaUrl(next.id);player.muted=state.settings.muted;player.volume=state.settings.volume;player.load();}
    else{player.removeAttribute('src');player.load();}
  }
  current=next;
  const playing=!!current&&state.session&&page==='rate';
  $('player-shell').classList.toggle('has-video',playing);$('player-empty').hidden=playing;
  $('clip-details').hidden=!current;
  $('clip-title').textContent=current?current.title:'';
  $('clip-meta').textContent=current?`${current.extension.toUpperCase()} · ${duration(current.duration_secs)} · ${current.width} × ${current.height}`:'';
  if(playing && $('player').paused&&!$('player').error)$('player').play().catch(()=>{});
  $('waiting-title').textContent=state.session?'Waiting for clips':'Session paused';
  $('waiting-message').textContent=!state.session?'Start a session to watch and rate.':state.peers.length?'Waiting for a clip from your peers.':'Connect a peer to get started.';
  $('connect-peer').hidden=!state.session||state.peers.length>0;
  const beneficiary=current?.beneficiary,ownClip=!!beneficiary&&beneficiary.toLowerCase()===state.crypto?.wallet?.toLowerCase();
  $('rating-recipient').textContent=!current?'':!beneficiary?'This clip needs a registered wallet before it can receive ✨. You can skip for free.':ownClip?'This clip is registered to your wallet. You can skip for free.':`Recipient · ${beneficiary}`;
  document.querySelectorAll('[data-score]').forEach(button=>{const score=Number(button.dataset.score);button.disabled=busy||rollBusy||walletBusy||transactionBusy||!playing||(score>0&&(!ratingWalletSelected()||!ratingDraftEditable()||queuedRatingCount()>=100||!beneficiary||ownClip||BigInt(score)*TOKEN_SCALE>spendableTokenBalance()||!!pendingTransaction));});
  $('peer-id').textContent=state.profile_id;$('data-folder').textContent=state.data_folder;
  $('network-ports').textContent=`Peer TCP ${state.p2p_port} · BitTorrent TCP ${state.torrent_port}`;
  $('listen-addresses').innerHTML=state.addresses.length?state.addresses.map(address=>`<button type="button" data-address="${esc(address)}" title="Copy peer address">${esc(address)}</button>`).join(''):'Finding addresses…';
  fillSettings();
  renderRoll();
  renderUpdates(state.update);
}
function renderUpdates(next){
  if(next)updateStatus=next;
  const update=updateStatus, phase=update?.phase==='installing'?'installing':updateTask==='install'?'downloading':update?.phase||'idle';
  const checking=updateTask==='check'||update?.status==='checking';
  const changing=updateBusy||checking||phase==='downloading'||phase==='installing';
  const available=!!update?.available;
  const version=update?.current_version||state?.version;
  if(version){$('app-version').textContent=version;$('about-version').textContent=`${version} · experimental alpha`;}
  let message='Updates are checked automatically when clipclub starts.';
  if(phase==='installing')message='Installing update and restarting clipclub…';
  else if(phase==='downloading')message='Downloading update…';
  else if(checking)message='Checking for updates…';
  else if(update?.error)message=update.error;
  else if(available)message=`Version ${update.latest_version} is available.${phase==='ready'?' Download complete.':''}`;
  else if(update?.status==='current')message='You have the latest release.';
  else if(update?.status==='unavailable')message='No published release yet.';
  $('update-status').textContent=message;
  $('update-status').classList.toggle('update-available',available);
  $('check-updates').disabled=changing||updateOpening;
  $('check-updates').textContent=checking?'Checking…':'Check for updates';
  $('install-update').hidden=!available||!update.can_install;
  $('install-update').disabled=changing||updateOpening;
  $('install-update').textContent=phase==='downloading'?'Downloading…':phase==='installing'?'Restarting…':'Install and restart';
  $('download-update').hidden=!available||!update.download_url;
  $('download-update').disabled=changing||updateOpening;
  $('update-release').hidden=!available||!update.release_url;
  $('update-release').disabled=changing||updateOpening;
  $('update-install-help').hidden=!available;
  $('update-install-help').textContent=update?.can_install?'Install and restart downloads the release and restarts clipclub. Your profile and clips stay in your data folder.':update?.download_url?'Download the new release, quit clipclub, then replace the installed app. Your profile and clips stay in your data folder.':'This release has no download for your platform. Open the release notes to see available packages.';
  if(available&&update.latest_version!==updateNoticeVersion){updateNoticeVersion=update.latest_version;toast(`clipclub ${update.latest_version} is available. Open Settings to update.`);}
}
async function checkUpdates(){
  if(updateBusy||updateOpening||updateStatus?.status==='checking'||['downloading','installing'].includes(updateStatus?.phase))return;
  updateBusy=true;updateTask='check';renderUpdates();
  try{renderUpdates(await api('update/check',{}));}
  catch(error){renderUpdates({...updateStatus,status:'unavailable',error:`Could not check for updates. ${error.message}`});}
  finally{updateBusy=false;updateTask=null;renderUpdates();}
}
async function openUpdate(target){
  if(updateOpening||updateBusy||updateStatus?.status==='checking'||!updateStatus?.available||['downloading','installing'].includes(updateStatus.phase))return;
  updateOpening=true;renderUpdates();
  try{await api('update/open',{target});}
  catch(error){toast(error.message);}
  finally{updateOpening=false;renderUpdates();}
}
async function installUpdate(){
  if(updateBusy||updateOpening||updateStatus?.status==='checking'||!updateStatus?.available||!updateStatus.can_install||['downloading','installing'].includes(updateStatus.phase))return;
  updateBusy=true;updateTask='install';renderUpdates({...updateStatus,phase:'downloading',error:null});
  try{renderUpdates(await api('update/install',{}));}
  catch(error){renderUpdates({...updateStatus,phase:'idle',error:`Update could not be installed. ${error.message}`});}
  finally{updateBusy=false;updateTask=null;renderUpdates();}
}
function rollWager(){return tokenUnits($('roll-wager').value);}
function renderRoll(){
  if(!state)return;
  const result=state.roll||null,pending=result?.status==='pending',clip=pending?null:result?.clip||null;
  if(pending)$('roll-wager').value=fmtTokens(result.wager_units).replace(/,/g,'');
  const wager=rollWager();
  $('roll-button').disabled=rollBusy||busy||walletBusy||pending||!!pendingTransaction||!walletAvailable()||wager===null||wager>spendableTokenBalance();
  $('roll-button').textContent=rollBusy?'Confirming…':pending?'Awaiting settlement':'Commit roll →';
  $('roll-wager').disabled=rollBusy||pending;
  $('settle-roll').hidden=!pending;
  $('settle-roll').disabled=rollBusy||!walletAvailable()||!!pendingTransaction;
  $('settle-roll').textContent=rollBusy?'Confirming…':'Settle with drand proof →';
  $('roll-result').hidden=!result;
  if(result){
    $('roll-multiplier').textContent=pending?'…':`${result.multiplier}×`;
    $('roll-result-title').textContent=pending?`Committed · drand round ${result.round}`:result.multiplier===0?'Wager burned':result.multiplier===1?'Wager returned':`${fmtTokens(result.payout_units)} ✨ returned`;
    $('roll-result-detail').textContent=pending?`${fmtTokens(result.wager_units)} ✨ locked on MegaETH. Settle after the randomness round is published.`:`Wager ${fmtTokens(result.wager_units)} ✨ · Total payout ${fmtTokens(result.payout_units)} ✨${result.multiplier>1?` · ${fmtTokens(BigInt(result.payout_units)-BigInt(result.wager_units))} ✨ minted`:result.multiplier===0?` · ${fmtTokens(result.wager_units)} ✨ burned`:' · Supply unchanged'}`;
  }
  const explorer=state.crypto?.explorer_url?.replace(/\/$/,'');
  const proofLinks=[];
  if(result&&explorer&&/^https:\/\//.test(explorer)){
    for(const [hash,label] of [[result.transaction_hash,'Commit transaction ↗'],[result.settlement_hash,'Settlement transaction ↗']])if(/^0x[a-fA-F0-9]{64}$/.test(hash||''))proofLinks.push(`<a href="${esc(explorer)}/tx/${hash}" target="_blank" rel="noopener noreferrer">${label}</a>`);
    if(/^\d+$/.test(String(result.round)))proofLinks.push(`<a href="https://api.drand.sh/v2/chains/52db9ba70e0cc0f6eaf7803dd07447a1f5477735fd3f661792ba94600c84e971/rounds/${result.round}" target="_blank" rel="noopener noreferrer">drand round ${result.round} ↗</a>`);
  }
  $('roll-proof').hidden=!proofLinks.length;$('roll-proof').innerHTML=proofLinks.join('');
  $('proof-browser').hidden=!result||!!walletProvider();$('proof-browser').disabled=rollBusy||busy||transactionBusy;
  const player=$('roll-player');player.loop=false;
  const key=result?`${result.id}:${clip?.id||''}`:'';
  if(key!==rollMediaKey){
    rollMediaKey=key;player.pause();
    if(clip){player.src=mediaUrl(clip.id);player.muted=state.settings.muted;player.volume=state.settings.volume;}
    else player.removeAttribute('src');
    player.load();
  }
  $('roll-player-shell').classList.toggle('has-video',!!clip);$('roll-player-empty').hidden=!!clip;
  $('roll-clip-details').hidden=!clip;$('roll-clip-title').textContent=clip?.title||'';
  $('roll-clip-meta').textContent=clip?`Rating ${result.multiplier} · ${duration(clip.duration_secs)} · ${clip.width} × ${clip.height}`:'';
  $('roll-waiting-title').textContent=!result?'Commit your roll':pending?'Waiting for verified randomness':result.multiplier===0?'0× · No clip':`${result.multiplier}× · Your roll`;
  $('roll-waiting-message').textContent=!result?'Wager ✨ to roll a multiplier and discover a clip.':pending?result.message||'Your wager and future round are fixed on-chain. Settle with the published drand proof.':result.multiplier===0?'Your stake was burned, reducing the token supply.':result.message||'Preparing a clip with this rating…';
  // Polls preserve both an ended clip and a manual pause.
  if(clip&&page==='roll'&&rollStartedKey!==key){rollStartedKey=key;player.play().catch(()=>{});}
}
async function roll(){
  if(rollBusy||busy||walletBusy||transactionBusy||page!=='roll'||!state)return;
  const wager=rollWager();
  if(wager===null||wager>spendableTokenBalance()){$('roll-error').textContent=wager===null?'Enter a positive wager with up to 6 decimal places.':'Your wager exceeds your ✨ balance.';$('roll-error').hidden=false;return;}
  if(state.roll?.status==='pending'){$('roll-error').textContent='Settle your committed roll before requesting another.';$('roll-error').hidden=false;return;}
  rollBusy=true;$('roll-error').hidden=true;renderRoll();
  try{await sendCryptoTransaction('request_roll',{wager_units:wager.toString()});}
  catch(error){$('roll-error').textContent=error.message;$('roll-error').hidden=false;}
  finally{walletPhase='';rollBusy=false;render();}
}
async function settleRoll(){
  if(rollBusy||busy||walletBusy||transactionBusy||state?.roll?.status!=='pending')return;
  rollBusy=true;$('roll-error').hidden=true;renderRoll();
  try{await sendCryptoTransaction('settle_roll',{roll_id:state.roll.id});}
  catch(error){$('roll-error').textContent=error.message;$('roll-error').hidden=false;}
  finally{walletPhase='';rollBusy=false;render();}
}

async function refresh(){const version=++refreshVersion;try{const next=await api('state',undefined,'GET');if(version!==refreshVersion)return;state=next;render();}catch(error){if(initialized&&version===refreshVersion)$('peer-status').textContent='Client disconnected';}}
async function rate(score){
  if(busy||rollBusy||walletBusy||transactionBusy||page!=='rate'||!current||!state?.session)return;
  const clipId=current.id;
  if(score>0){
    if(!ratingWalletSelected()){toast('Select a configured wallet before queueing paid ratings.');return;}
    if(!ratingDraftEditable()||pendingTransaction){toast('Confirm the frozen session transaction before queueing more ratings.');return;}
    if(queuedRatingCount()>=100){toast('This session has 100 ratings. Settle or discard it before queueing more.');return;}
    if(!current.beneficiary||current.beneficiary.toLowerCase()===state.crypto.wallet.toLowerCase()){toast('This clip needs a registered wallet other than your own before you can queue ✨.');return;}
    if(BigInt(score)*TOKEN_SCALE>spendableTokenBalance()){toast('Your available ✨ balance is already reserved by queued ratings.');return;}
  }
  busy=true;render();
  try{
    await api('rate',{id:clipId,score});refreshVersion++;
    toast(score===0?'Passed. No tokens spent.':`${score} ✨ queued · not paid. Settle the session once when you finish.`);await refresh();
  }catch(error){toast(error.message);}finally{busy=false;render();}
}
async function uploads(files){
  if(uploading)return;
  uploading=true;const feedback=$('upload-feedback');feedback.hidden=false;
  for(const file of files){
    if(!/\.(mp4|webm)$/i.test(file.name)){toast(`${file.name}: only MP4 and WebM videos are accepted.`);continue;}
    if(!file.size||file.size>(state?.limits?.max_bytes||8000000)){toast(`${file.name}: videos must be between 1 byte and 8 MB.`);continue;}
    feedback.textContent=`Checking ${file.name}…`;
    try{const body=new FormData();body.append('video',file);const result=await api('upload',body);feedback.textContent=result.duplicate?`${file.name} · Joined existing swarm`:`${file.name} · Seeding`;await refresh();}
    catch(error){feedback.textContent=`${file.name}: ${error.message}`;toast(error.message);}
  }
  uploading=false;$('file-input').value='';
}
async function copy(text){try{await navigator.clipboard.writeText(text);toast('Copied to clipboard.');}catch{toast('Select and copy the address shown in Settings.');}}
document.querySelector('.brand').addEventListener('click',event=>{event.preventDefault();navigate('home');});
document.querySelectorAll('button[data-page]').forEach(button=>button.addEventListener('click',()=>navigate(button.dataset.page)));
document.querySelectorAll('[data-goto]').forEach(button=>button.addEventListener('click',()=>navigate(button.dataset.goto)));
$('rating-keys').innerHTML=Array.from({length:10},(_,score)=>`<button data-score="${score}" aria-label="Rate ${score}, queue ${score} ✨" disabled>${score}</button>`).join('');
$('rating-keys').addEventListener('click',event=>{const button=event.target.closest('[data-score]');if(button)rate(Number(button.dataset.score));});
$('roll-form').addEventListener('submit',event=>{event.preventDefault();roll();});
$('settle-roll').addEventListener('click',settleRoll);
$('recover-roll').addEventListener('click',recoverRoll);
$('connect-wallet').addEventListener('click',connectWallet);
$('open-wallet-browser').addEventListener('click',openWalletBrowser);
$('proof-browser').addEventListener('click',openWalletBrowser);
$('retry-transaction').addEventListener('click',retryTransaction);
$('roll-wager').addEventListener('input',()=>{$('roll-error').hidden=true;renderRoll();});
$('roll-odds').innerHTML=Array.from({length:10},(_,k)=>`<div><strong>${k}×</strong><span>${(2**(9-k)/1023*100).toFixed(k>5?3:2)}%</span></div>`).join('');
$('roll-player').addEventListener('error',()=>{if(state?.roll?.clip){$('roll-error').textContent='Your system could not play this clip’s codec. Your roll and payout are saved.';$('roll-error').hidden=false;}});
document.addEventListener('keydown',event=>{if(event.repeat||event.ctrlKey||event.metaKey||event.altKey||event.target.isContentEditable||/INPUT|TEXTAREA|SELECT/.test(event.target.tagName)||$('preview-dialog').open)return;if(event.key==='Escape'&&page!=='home'){event.preventDefault();navigate('home');}else if(/^[0-9]$/.test(event.key)&&page==='rate'){event.preventDefault();rate(Number(event.key));}});
$('session-button').addEventListener('click',toggleRatingSession);
$('settle-rating-session').addEventListener('click',settleRatingSession);
$('discard-rating-session').addEventListener('click',discardRatingSession);
$('unlock-rating-session').addEventListener('click',unlockRatingSession);
$('rating-session-recover').addEventListener('click',recoverRatingSession);
$('dropzone').addEventListener('click',event=>{if(event.target!==$('file-input'))$('file-input').click();});
$('dropzone').addEventListener('keydown',event=>{if(event.key==='Enter'||event.key===' '){event.preventDefault();$('file-input').click();}});
$('file-input').addEventListener('change',event=>uploads([...event.target.files]));
for(const name of ['dragenter','dragover'])$('dropzone').addEventListener(name,event=>{event.preventDefault();$('dropzone').classList.add('dragover');});
for(const name of ['dragleave','drop'])$('dropzone').addEventListener(name,event=>{event.preventDefault();$('dropzone').classList.remove('dragover');});
$('dropzone').addEventListener('drop',event=>uploads([...event.dataTransfer.files]));
$('library-sort').addEventListener('change',renderLibrary);
$('library').addEventListener('click',async event=>{
  const preview=event.target.closest('[data-preview]'),remove=event.target.closest('[data-remove]'),torrent=event.target.closest('[data-torrent]'),register=event.target.closest('[data-register]');
  if(register){if(walletBusy||rollBusy||busy)return;walletBusy=true;register.disabled=true;try{await sendCryptoTransaction('register_clip',{clip_id:register.dataset.register});toast('Clip wallet registered on MegaETH.');await refresh();}catch(error){toast(error.message);}finally{walletPhase='';walletBusy=false;render();}}
  if(preview){const clip=state.library.find(item=>item.clip.id===preview.dataset.preview).clip;$('preview-title').textContent=clip.title;$('preview-video').src=mediaUrl(clip.id);$('preview-dialog').showModal();$('preview-video').play().catch(()=>{});}
  if(remove){remove.disabled=true;try{await api(`remove/${remove.dataset.remove}`,{});toast('Removed from your seed collection.');await refresh();}catch(error){toast(error.message);remove.disabled=false;}}
  if(torrent){try{const response=await fetch(`/api/torrent/${torrent.dataset.torrent}`,{headers:{'x-clipcrop-token':token}});if(!response.ok)throw Error('Could not export torrent.');const blob=URL.createObjectURL(await response.blob());const link=document.createElement('a');link.href=blob;link.download=`${torrent.dataset.torrent}.torrent`;link.click();setTimeout(()=>URL.revokeObjectURL(blob),30000);}catch(error){toast(error.message);}}
});
$('close-preview').addEventListener('click',()=>$('preview-dialog').close());
$('preview-dialog').addEventListener('close',()=>{$('preview-video').pause();$('preview-video').removeAttribute('src');$('preview-video').load();});
$('player').addEventListener('error',()=>{if(current)toast('Your system could not play this codec. Press 0 to pass without spending tokens.');});
document.querySelectorAll('#settings-form input, #settings-form textarea, #settings-form button').forEach(control=>control.disabled=true);
$('settings-form').addEventListener('input',()=>settingsDirty=true);
$('settings-form').addEventListener('submit',async event=>{event.preventDefault();if(!state){toast('Settings are still loading. Try again in a moment.');return;}try{await api('settings',{display_name:$('display-name').value.trim(),muted:$('muted').checked,prefetch:$('prefetch').checked,volume:Number($('volume').value),bootstrap_peers:$('bootstrap-peers').value.split('\n').map(s=>s.trim()).filter(Boolean)});settingsDirty=false;$('save-status').textContent='Saved';await refresh();}catch(error){toast(error.message);}});
$('copy-id').addEventListener('click',()=>copy(state.profile_id));
$('check-updates').addEventListener('click',checkUpdates);
$('install-update').addEventListener('click',installUpdate);
$('download-update').addEventListener('click',()=>openUpdate('download'));
$('update-release').addEventListener('click',()=>openUpdate('release'));
$('listen-addresses').addEventListener('click',event=>{const button=event.target.closest('[data-address]');if(button)copy(button.dataset.address);});
window.addEventListener('pagehide',()=>{$('roll-player').pause();if(state?.session)fetch('/api/session',{method:'POST',headers:{'x-clipcrop-token':token,'Content-Type':'application/json'},body:'{"active":false}',keepalive:true}).catch(()=>{});});
async function syncWalletAccount(){
  try{const accounts=await walletProvider()?.request({method:'eth_accounts'});walletAccount=accounts?.[0]||null;render();}catch{walletAccount=null;}
}
if(walletProvider()?.on){
  walletProvider().on('accountsChanged',async accounts=>{
    walletAccount=accounts?.[0]||null;
    if(transactionBusy){deferredWalletAddress=walletAccount;renderWallet();return;}
    if(walletSelectionLocked()&&walletAccount?.toLowerCase()!==state.crypto?.wallet?.toLowerCase()){render();toast('Settle or discard queued ratings and confirm pending transactions before switching wallets.');return;}
    pendingTransaction=null;
    try{await api('wallet',{address:walletAccount});await refresh();}catch(error){toast(error.message);}
  });
  walletProvider().on('chainChanged',()=>{walletPhase='';refresh();});
}
(async()=>{await refresh();initialized=true;await syncWalletAccount();setInterval(refresh,1500);})();
