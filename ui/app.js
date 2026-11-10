'use strict';
const $ = id => document.getElementById(id);
const token = document.querySelector('meta[name="api-token"]').content;
const esc = value => String(value).replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
let lastNotice=null;
let state=null, page='home', current=null, busy=false, uploading=false, initialized=false, toastTimer;
let rollBusy=false, rollMediaKey='', rollStartedKey='';
let updateStatus=null, updateBusy=false, updateOpening=false, updateNoticeVersion=null, updateTask=null;
let refreshVersion=0;
let navigationQueue=Promise.resolve(),sessionToggleBusy=false,ratingRecoveryState=null;
let diagnosticsResult=null,diagnosticsBusy=false,diagnosticsError='',diagnosticsPortsReady=false;
const diagnosticsPortEdits=new Set();
let diagnosticsSuite='all',diagnosticsNotesLoaded=false,diagnosticsNotesSaved=true;
let diagnosticsInbox=null,diagnosticsInboxLoad=null,diagnosticsInboxLoaded=false,diagnosticsInboxDirty=false,diagnosticsInboxBusy=false;
let diagnosticsInboxCheckedAt=0,diagnosticsInboxError='',diagnosticsInboxLoadError='',diagnosticsInboxNotice='';
let diagnosticsInboxRevision=0;
const diagnosticsResults=new Map(),diagnosticsChecklistResults={};
const diagnosticSuites={
  all:{title:'All checks',help:'Run a baseline before testing another device or network. Checks profile access, media tools, peers, and the configured chain without sending transactions.'},
  device:{title:'Device & storage',help:'Compare app builds, platform, available disk space, and profile read/write access. A temporary test file is removed after the check.'},
  network:{title:'Discovery & peers',help:'Inspect discovery and authenticated peers, then test TCP listeners. Run on both devices to compare directions; reachable TCP alone does not prove a video transfer.'},
  media:{title:'Video & playback',help:'Check the media tool, download queue, and codecs reported by this webview. Use the checklist to verify real picture, sound, seeking, and transfers.'},
  wallet:{title:'Wallet & chain',help:'Inspect local wallet and pending activity, then make read-only RPC checks against the configured chain. These tests do not sign or send transactions.'},
  checklist:{title:'Device checklist',help:'Record the behavior you observed on this device.'},
};
const diagnosticSteps=[
  {id:'discovery',title:'Discover each other',detail:'On the same network, open both apps and wait at least 15 seconds. Confirm both show a connected peer. Record the other device and network.'},
  {id:'transfer-out',title:'Transfer A → B',detail:'Upload a small MP4 on this device. On the other device, open Rate and wait for the clip to download and play. TCP reachability alone is insufficient.'},
  {id:'transfer-in',title:'Transfer B → A',detail:'Upload a different clip on the other device. Confirm this device downloads it and plays it from Rate. Repeat using WebM.'},
  {id:'playback',title:'Picture, sound & seeking',detail:'Play MP4/H.264 + AAC and WebM/VP9 + Opus clips with known audio. Check sound, seeking, loop, pause/resume, and fullscreen. Record codec-specific failures.'},
  {id:'layout',title:'Window, display & keyboard',detail:'Resize to a narrow window, try display scaling or another monitor, and navigate menus by keyboard. Check focus, Escape, video controls, and that text stays readable.'},
  {id:'resume',title:'Sleep, wake & reconnect',detail:'Let this device sleep, wake it, and reopen Rate. Reconnect Wi-Fi or switch to a hotspot. Confirm peers reconnect and a new clip transfers; rerun network checks.'},
  {id:'restart',title:'Relaunch & library',detail:'Quit and reopen the app. Confirm uploaded clips resume seeding and another device can download a new clip. Confirm saved rating drafts remain available.'},
  {id:'wallet',title:'Wallet unlock & cancellation',detail:'After relaunch, confirm the wallet starts locked. Check wrong-password rejection and correct unlock. If a transaction is prepared, review details and cancel; confirm nothing is sent.'},
  {id:'offline',title:'Offline & recovery',detail:'Disconnect networking and check that menus stay usable and errors are clear. Reconnect, rerun diagnostics, and confirm a fresh transfer succeeds.'},
];
async function api(path, body, method='POST', signal) {
  const options={method,headers:{'x-clipcrop-token':token}};
  if(signal)options.signal=signal;
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
let walletBusy=false, transactionBusy=false, walletPhase='', pendingTransaction=null;
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
function localWallet(){return state?.crypto?.local_wallet||null;}
function nativeWalletSelected(){return !!localWallet()?.exists&&!!localWallet().address&&localWallet().address.toLowerCase()===state?.crypto?.wallet?.toLowerCase();}
function walletAvailable(){return !!state?.crypto?.configured&&!state.crypto.error&&nativeWalletSelected()&&!!localWallet().unlocked;}
function fmtEth(raw){
  let value;try{value=BigInt(raw??'0');}catch{return '—';}
  const scale=1000000000000000000n,whole=value/scale,fraction=(value%scale).toString().padStart(18,'0').replace(/0+$/,'');
  return `${whole}${fraction?`.${fraction}`:''} ETH`;
}
let walletFormBusy=false,nativeApproval=null,walletStartupPending=false;
function walletStartupRequired(){return !state||!!localWallet()&&!localWallet().unlocked;}
function syncStartupWallet(){
  if(walletStartupRequired()){
    walletStartupPending=true;
    if(!$('wallet-dialog').open)openNativeWallet();
  }else if(walletStartupPending){
    walletStartupPending=false;
    if($('wallet-dialog').open)$('wallet-dialog').close();
  }
}
function clearWalletSecrets(){for(const id of ['wallet-private-key','wallet-password','wallet-password-confirm','wallet-backup-json','wallet-backup-file'])$(id).value='';}
function renderNativeWallet(){
  const local=localWallet(),exists=!!local?.exists,unlocked=!!local?.unlocked;
  $('native-wallet-status').textContent=exists?`Built-in wallet · ${unlocked?'unlocked until you quit the app':'unlock when starting the app'}.`:'Set up your wallet when starting the app to pay and receive rewards.';
  $('native-gas-balance').textContent=nativeWalletSelected()?`Gas balance · ${fmtEth(state.crypto.native_balance_wei)} on ${state.crypto.chain_name}`:'';
  $('manage-wallet').textContent='Wallet details';$('manage-wallet').hidden=!unlocked;
  $('manage-wallet').disabled=walletBusy||transactionBusy||rollBusy||busy;
  $('copy-wallet-address').hidden=!exists;$('backup-wallet').hidden=!exists;
  $('backup-wallet').disabled=walletFormBusy;
  if(nativeApproval&&(!nativeWalletSelected()||!unlocked))finishNativeApproval(false);
  syncStartupWallet();
  if($('wallet-dialog').open)renderWalletDialog();
}
function renderWalletDialog(){
  const local=localWallet(),exists=!!local?.exists,unlocked=!!local?.unlocked,mode=$('wallet-mode').value||'create';
  $('wallet-dialog-title').textContent=unlocked?'Your built-in wallet':exists?'Welcome back':'Welcome to clipclub';
  $('wallet-dialog-help').textContent=unlocked?'Your private key is encrypted on this device. Your wallet stays unlocked until you quit the app.':exists?'Enter your password to unlock your wallet and open the app. It stays unlocked until you quit.':'Create, import, or restore a wallet to get started. Your password encrypts it on this device and unlocks it each time you open the app.';
  $('close-wallet').hidden=walletStartupRequired();$('close-wallet').disabled=walletFormBusy;
  $('wallet-dialog-account').hidden=!exists;$('wallet-receive-address').textContent=local?.address||'';
  $('wallet-setup-options').hidden=exists;
  $('wallet-private-key-row').hidden=exists||mode!=='import';$('wallet-restore-row').hidden=exists||mode!=='restore';
  $('wallet-password-row').hidden=unlocked;
  $('wallet-password').required=!unlocked;$('wallet-password').minLength=exists||mode==='restore'?1:12;
  $('wallet-password').autocomplete=exists||mode==='restore'?'current-password':'new-password';
  $('wallet-password-label').textContent=exists?'Wallet password':mode==='restore'?'Backup password':'New wallet password · at least 12 characters';
  $('wallet-password-confirm-row').hidden=exists||mode==='restore';$('wallet-password-confirm').required=!exists&&mode!=='restore';
  $('wallet-submit').hidden=unlocked;$('wallet-submit').textContent=walletFormBusy?'Working…':exists?'Unlock and continue':mode==='import'?'Import wallet':mode==='restore'?'Restore wallet':'Create wallet';
  $('wallet-submit').disabled=walletFormBusy;$('wallet-mode').disabled=walletFormBusy;
  $('wallet-dialog-backup').hidden=!exists;
  $('wallet-dialog-backup').disabled=walletFormBusy;
}
function openNativeWallet(){
  if(!state||walletFormBusy)return;
  clearWalletSecrets();$('wallet-form-error').hidden=true;renderWalletDialog();
  if(!$('wallet-dialog').open)$('wallet-dialog').showModal();
}
async function submitNativeWallet(event){
  event.preventDefault();if(walletFormBusy)return;
  const local=localWallet(),exists=!!local?.exists,mode=exists?'unlock':$('wallet-mode').value||'create',password=$('wallet-password').value;
  $('wallet-form-error').hidden=true;
  if(!password){$('wallet-form-error').textContent='Enter your wallet password.';$('wallet-form-error').hidden=false;return;}
  if(['create','import'].includes(mode)&&(password.length<12||password!==$('wallet-password-confirm').value)){$('wallet-form-error').textContent=password.length<12?'Use a password with at least 12 characters.':'The password confirmation does not match.';$('wallet-form-error').hidden=false;return;}
  let body={password};
  try{
    if(mode==='import'){body.private_key=$('wallet-private-key').value.trim();if(!/^(?:0x)?[a-fA-F0-9]{64}$/.test(body.private_key))throw Error('Enter a complete 32-byte Ethereum private key.');}
    if(mode==='restore'){const file=$('wallet-backup-file').files?.[0];if(file&&file.size>20000)throw Error('Choose a ClipClub wallet backup smaller than 20 KB.');body.backup=JSON.parse(file?await file.text():$('wallet-backup-json').value);}
    walletFormBusy=true;renderWalletDialog();clearWalletSecrets();
    await api(`wallet/${mode}`,body);body=null;await refresh();
    toast(mode==='unlock'?'Built-in wallet unlocked.':'Wallet saved on this device. Download an encrypted backup and keep its password safe.');
  }catch(error){$('wallet-form-error').textContent=error instanceof SyntaxError?'The encrypted backup is not valid JSON.':error.message;$('wallet-form-error').hidden=false;}
  finally{body=null;walletFormBusy=false;clearWalletSecrets();renderWalletDialog();renderWallet();}
}
async function backupNativeWallet(){
  if(walletFormBusy||!localWallet()?.exists)return;
  walletFormBusy=true;renderWalletDialog();
  try{
    const backup=await api('wallet/backup',{}),blob=new Blob([JSON.stringify(backup,null,2)],{type:'application/json'}),url=URL.createObjectURL(blob),link=document.createElement('a');
    link.href=url;link.download=`clipclub-wallet-${localWallet().address}.json`;document.body.appendChild(link);link.click();link.remove();setTimeout(()=>URL.revokeObjectURL(url),1000);
    toast('Encrypted backup downloaded. Keep its password separately; there is no password reset.');
  }catch(error){toast(error.message);}finally{walletFormBusy=false;renderWallet();}
}
function finishNativeApproval(approved){
  const approval=nativeApproval;if(!approval)return;nativeApproval=null;
  if($('wallet-approval-dialog').open)$('wallet-approval-dialog').close();approval.resolve(approved);
}
async function reviewNativeTransaction(transaction,action,fields){
  const approval=transaction.wallet_approval;
  if(!approval||typeof approval.id!=='string'||!approval.id||approval.action!==action||!/^\d+$/.test(String(approval.max_fee_wei)))throw Error('The built-in wallet did not return a valid transaction review.');
  if(nativeApproval)throw Error('Another transaction review is open.');
  $('wallet-review-action').textContent=({rate_session:'Settle rating session',request_roll:'Commit roll wager',settle_roll:'Settle committed roll',register_seeder:'Register seeder wallet',register_clip:'Register clip',rate_clip:'Pay clip rating'})[action]||action;
  $('wallet-review-network').textContent=`${state.crypto.chain_name} · ${state.crypto.chain_id}`;
  $('wallet-review-from').textContent=transaction.from;$('wallet-review-contract').textContent=transaction.to;
  const units=approval.token_units??(action==='rate_session'?queuedTokenUnits().toString():action==='request_roll'?fields.wager_units:action==='rate_clip'?(BigInt(fields.score)*TOKEN_SCALE).toString():'0');
  $('wallet-review-amount').textContent=action==='settle_roll'?'No new wager · settles the existing commitment':`${fmtTokens(units)} ✨${action==='request_roll'?' wager held on-chain':''}`;
  $('wallet-review-fee').textContent=fmtEth(approval.max_fee_wei);
  const expires=Date.now()+Math.max(0,Math.min(120,Number(approval.expires_in_seconds)||0))*1000;
  $('wallet-review-expiry').textContent='This review expires in two minutes. The actual gas charge can be lower.';$('wallet-approval-error').hidden=true;
  const accepted=await new Promise(resolve=>{nativeApproval={resolve,expires};$('wallet-approval-dialog').showModal();});
  if(!accepted){const error=Error('Transaction cancelled. No tokens were sent.');error.code=4001;throw error;}
  return approval.id;
}
function approveNativeTransaction(event){
  event.preventDefault();if(!nativeApproval)return;
  if(Date.now()>=nativeApproval.expires){$('wallet-approval-error').textContent='This transaction review expired. Cancel and prepare it again.';$('wallet-approval-error').hidden=false;return;}
  finishNativeApproval(true);
}
function currentTokenBalance(){try{return BigInt(state?.crypto?.balance_units||'0');}catch{return 0n;}}
function ratingSession(){return state?.crypto?.rating_session||null;}
function queuedRatingCount(){return ratingSession()?.ratings?.length||0;}
function queuedTokenUnits(){try{return BigInt(ratingSession()?.total_units||'0');}catch{return 0n;}}
function spendableTokenBalance(){const balance=currentTokenBalance(),reserved=queuedTokenUnits();return balance>reserved?balance-reserved:0n;}
function ratingWalletSelected(){return !!state?.crypto?.configured&&!state.crypto.error&&nativeWalletSelected();}
function ratingDraftEditable(){return !ratingSession()||ratingSession().status==='draft';}
function clipCanReceiveRating(clip){return clip?.distribution_preview?clip.distribution_preview.seeders>0:!!clip?.beneficiary&&clip.beneficiary.toLowerCase()!==state.crypto?.wallet?.toLowerCase();}
function seederPaymentsFit(){
  const preview=current?.distribution_preview;if(!preview)return true;
  const payments=(ratingSession()?.ratings||[]).reduce((sum,r)=>sum+(r.distribution?.recipients?.length||0),0)+preview.seeders;
  return payments<=1000;
}
function renderRatingSession(){
  if(!state)return;
  const session=ratingSession(),count=queuedRatingCount(),submitted=session?.status==='submitted',prepared=session?.status==='prepared';
  $('rating-session-panel').hidden=!count;
  $('rating-session-status').textContent=submitted?'Submitted · awaiting confirmation':prepared?'Prepared · not yet confirmed':'Queued · not paid';
  $('rating-session-summary').textContent=`${count} rating${count===1?'':'s'} · ${fmtTokens(queuedTokenUnits())} ✨ total`;
  $('rating-session-available').textContent=`${fmtTokens(spendableTokenBalance())} ✨ available after queued ratings. Up to 100 ratings settle together.${session?.payout_units?` Seeders receive ${fmtTokens(session.payout_units)} ✨; ${fmtTokens(session.mint_required_units)} ✨ estimated new tokens minted.`:''}`;
  $('rating-session-items').innerHTML=(session?.ratings||[]).map(item=>`<li><span class="session-item-title" title="${esc(item.clip_id)}">${esc(item.title||item.clip_id)}${item.distribution?`<details><summary>${item.distribution.recipients.length} permanent seeder${item.distribution.recipients.length===1?'':'s'} · ${fmtTokens(item.distribution.reward_units,0)} ✨ each</summary>${item.distribution.recipients.map(r=>`<div><code>${esc(r.wallet)}</code> · ${fmtTokens(item.distribution.reward_units,0)} ✨</div>`).join('')}</details>`:''}</span><span>${item.score} ✨</span></li>`).join('');
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
    if(record&&/^0x[a-fA-F0-9]{64}$/.test(record.hash)&&['request_roll','settle_roll','rate_clip','rate_session','register_clip','register_seeder'].includes(record.action))pendingTransaction={...record,chain_id:state.crypto.chain_id,contract:state.crypto.contract,wallet:state.crypto.wallet};
  }catch{}
}
function pendingServerRecord(record){const result={action:record.action,hash:record.hash};for(const key of ['wager_units','roll_id','clip_id','score','session_id'])if(record[key]!==undefined)result[key]=record[key];return result;}
async function forgetTransaction(){
  await api('crypto/pending',null);
  if(state?.crypto)state.crypto.pending_transaction=null;
  rememberTransaction(null);
}
async function requireWallet({readOnly=false}={}){
  const crypto=state?.crypto;
  if(!crypto?.configured)throw Error('The 좋아 contract has not been configured. See Settings for deployment status.');
  if(crypto.error)throw Error(crypto.error);
  if(!nativeWalletSelected()){openNativeWallet();throw Error('Set up your built-in wallet, then try this transaction again.');}
  if(!localWallet().unlocked&&!readOnly){openNativeWallet();throw Error('Unlock your built-in wallet, then try this transaction again.');}
}
async function connectWallet(){
  if(walletBusy||transactionBusy||rollBusy||busy)return;
  openNativeWallet();
}
function renderWallet(){
  if(!state)return;
  const crypto=state.crypto;
  restoreTransaction();
  $('balance').textContent=crypto?.wallet?`${fmtTokens(spendableTokenBalance())} ✨`:'— ✨';
  $('reserved-balance').hidden=!queuedRatingCount();$('reserved-balance').textContent=`${fmtTokens(queuedTokenUnits())} ✨ queued · not paid`;
  $('balance').title=crypto?.wallet?`Wallet balance ${fmtTokens(crypto.balance_units)} ✨; available after queued ratings`:'';
  $('connect-wallet').disabled=walletBusy||transactionBusy||rollBusy||busy;
  $('connect-wallet').title=nativeWalletSelected()&&!localWallet().unlocked?'Unlock wallet':'Wallet details';
  $('crypto-network').textContent=crypto?`${crypto.chain_name} · ${crypto.chain_id}`:'MegaETH';
  $('crypto-wallet').textContent=crypto?.wallet||'Disconnected';
  $('crypto-contract').textContent=crypto?.contract||'Not configured';
  $('seeder-status').textContent=crypto?.seeder_registered?'This wallet receives rewards for your permanent seeds.':'Register this wallet to receive permanent seeder rewards.';
  $('register-seeder').disabled=!walletAvailable()||walletBusy||transactionBusy||rollBusy||busy||!!pendingTransaction;
  $('register-seeder').hidden=!!crypto?.seeder_registered;
  $('rating-credit-status').textContent=`Unused rating credit · ${fmtTokens(crypto?.rating_credit_units||'0')} ✨. You pay only the rating score. Credit funds future seeder rewards; any remaining shortfall is minted automatically.`;
  $('wallet-status').textContent=walletPhase||crypto?.error||(!crypto?.configured?'The 좋아 token has not been deployed and configured for this app.':nativeWalletSelected()?(localWallet().unlocked?'Your built-in wallet is ready. Review each transaction before approving it.':'Your built-in wallet is locked. Unlock it to sign transactions.'):'Set up your built-in wallet to receive tokens and approve transactions.');
  renderNativeWallet();
  $('stat-earned').textContent=crypto?.configured?`${fmtTokens(crypto.total_supply_units)} ✨`:'—';
  $('pending-transaction').hidden=!pendingTransaction;
  $('pending-transaction-detail').textContent=pendingTransaction?`Unconfirmed ${pendingTransaction.action.replaceAll('_',' ')} · ${pendingTransaction.hash}. Retry confirmation before sending another transaction.`:'';
  $('retry-transaction').disabled=walletBusy||transactionBusy||rollBusy||busy;
  renderRatingSession();
}
async function waitForReceipt(hash){
  for(let attempt=0;attempt<120;attempt++){
    const receipt=await api('wallet/rpc',{method:'eth_getTransactionReceipt',params:[hash]});
    if(receipt){if(BigInt(receipt.status??'0x0')!==1n){const error=Error('The transaction reverted. Your wallet can show the reason.');error.code='TRANSACTION_REVERTED';throw error;}return receipt;}
    await new Promise(resolve=>setTimeout(resolve,1000));
  }
  throw Error('This transaction is still waiting for inclusion. Use Retry confirmation in Settings; do not submit it again.');
}
async function confirmTransaction(record){
  walletPhase='Waiting for transaction inclusion…';renderWallet();
  await waitForReceipt(record.hash);
  if(record.action==='request_roll'||record.action==='settle_roll'){
    const result=await api('roll',{transaction_hash:record.hash});refreshVersion++;
    state.roll=state.roll?.id===result.id&&state.roll.clip?{...result,clip:state.roll.clip}:result;
  }else if(record.action==='rate_session')await api('rating-session/confirm',{transaction_hash:record.hash});
  else if(record.action==='rate_clip')await api('rate',{id:record.clip_id,score:record.score,transaction_hash:record.hash});
  else if(['register_seeder'].includes(record.action))await api('crypto/confirm-account',{transaction_hash:record.hash,action:record.action});
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
    await requireWallet();
    const crypto={...state.crypto};
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
    const approval_id=await reviewNativeTransaction(transaction,action,fields);
    const result=await api('wallet/send',{approval_id}),hash=result.hash;
    if(result.broadcast_error)toast(`Transaction ${hash} is saved. Retry confirmation in Settings to resend the same signed transaction.`);
    if(!/^0x[a-fA-F0-9]{64}$/.test(hash||''))throw Error('The wallet did not return a valid transaction hash.');
    submittedHash=hash;
    const record={action,...fields,...(sessionId?{session_id:sessionId}:{}),hash,chain_id:crypto.chain_id,contract:crypto.contract,wallet:crypto.wallet};rememberTransaction(record);
    if(action==='rate_session'&&ratingSession()?.id===sessionId)state.crypto.rating_session.status='submitted';
    try{await api('crypto/pending',pendingServerRecord(record));state.crypto.pending_transaction=pendingServerRecord(record);}
    catch{toast(`Transaction ${hash} is saved on this device. The profile backup failed; keep the app open until confirmation.`);}
    await confirmTransaction(record);
  }catch(error){
    // A reverted transaction has no effects; a missing confirmation stays recoverable.
    if(error.code==='TRANSACTION_REVERTED')try{await resetRevertedTransaction(pendingTransaction);}catch{toast('The reverted transaction still needs server verification. Retry confirmation in Settings.');}
    else if(action==='rate_session'&&preparationOwned&&!submittedHash&&(!signatureRequested||Number(error.code)===4001||error.code==='ACTION_REJECTED')){
      try{await api('rating-session/unlock',{});await refresh();}catch{toast('Your unpaid session is still prepared. Retry checkout or recover its transaction.');}
    }else if(action==='rate_session'&&!preparationOwned&&!signatureRequested)await refresh();
    throw error;
  }finally{
    transactionBusy=false;walletPhase='';
    renderWallet();
  }
}
async function retryTransaction(){
  if(!pendingTransaction||walletBusy||transactionBusy||rollBusy||busy)return;
  walletBusy=true;transactionBusy=true;renderWallet();
  try{await requireWallet({readOnly:true});await api('wallet/rebroadcast',{});await confirmTransaction(pendingTransaction);toast('Transaction confirmed.');}
  catch(error){if(error.code==='TRANSACTION_REVERTED')try{await resetRevertedTransaction(pendingTransaction);}catch{}toast(error.message);}
  finally{walletPhase='';walletBusy=false;transactionBusy=false;render();}
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
  const markup=library.length?library.map(({clip:c,seeding,average_rating,rating_count})=>`<div class="library-row"><button class="clip-thumb" data-preview="${c.id}" aria-label="Preview ${esc(c.title)}">▷</button><div class="row-detail"><h3>${esc(c.title)}</h3><p>${esc(c.extension.toUpperCase())} · ${duration(c.duration_secs)} · ${c.width} × ${c.height} · ${(c.size/1e6).toFixed(2)} MB</p><p class="clip-stats"><span title="Average of 1–9 ratings; skips are excluded.">${average_rating===null?'Unrated':`Rating <strong>${average_rating.toFixed(1)} / 9</strong> · ${rating_count} rating${rating_count===1?'':'s'}`}</span></p></div><span class="seeding-tag">${seeding?'● Seeding':'○ Unavailable'}</span><div class="row-actions"><button data-remove="${c.id}">Remove</button></div></div>`).join(''):'<div class="library-empty"><span aria-hidden="true">▧</span><h3>No clips yet</h3></div>';
  if($('library').innerHTML!==markup)$('library').innerHTML=markup;
}
function queueNavigation(operation){
  // Session changes must finish in order, even when menu clicks arrive quickly.
  navigationQueue=navigationQueue.then(operation).catch(error=>toast(error.message));
  return navigationQueue;
}
function navigate(next){
  if(!['home','seed','rate','roll','settings','diagnostics'].includes(next))next='home';
  return queueNavigation(async()=>{
    if(walletStartupRequired())return;
    if(next===page)return;
    const previous=page;
    page=next;
    if(document.body)document.body.dataset.page=page;
    document.querySelectorAll('.page').forEach(el=>el.hidden=el.id!==`page-${page}`);
    $('back-home').hidden=page==='home';
    $('breadcrumb').textContent={home:'Home',seed:'Upload',rate:'Rate',roll:'Roll',settings:'Settings',diagnostics:'Diagnostics'}[page];
    if(page!=='rate')$('player').pause();
    if(page!=='roll')$('roll-player').pause();
    const focusTarget=document.querySelector(page==='home'?`[data-page="${previous}"]`:`#page-${page}`);
    focusTarget?.setAttribute?.('tabindex',page==='home'?'0':'-1');
    focusTarget?.focus?.({preventScroll:true});
    window.scrollTo?.({top:0});
    if(previous==='rate'){
      try{await api('session',{active:false});}catch(error){toast(error.message);}
    }
    if(page==='rate'){
      try{await api('session',{active:true});}catch(error){toast(error.message);}
    }
    await refresh();
  });
}
function loadDiagnosticNotes(){
  if(diagnosticsNotesLoaded)return;
  diagnosticsNotesLoaded=true;
  try{
    const saved=JSON.parse(window.localStorage?.getItem('clipclub-diagnostic-notes')||'null');
    if(!saved||typeof saved!=='object')return;
    if(typeof saved.device==='string')$('diagnostics-device-label').value=saved.device.slice(0,80);
    if(['unspecified','same-wifi','wired','hotspot','different-networks','offline'].includes(saved.network))$('diagnostics-network-condition').value=saved.network;
    if(typeof saved.notes==='string')$('diagnostics-notes').value=saved.notes.slice(0,2000);
    for(const step of diagnosticSteps)if(['pass','fail','skip'].includes(saved.steps?.[step.id]))diagnosticsChecklistResults[step.id]=saved.steps[step.id];
  }catch{diagnosticsNotesSaved=false;}
}
function saveDiagnosticNotes(){
  try{
    if(!window.localStorage)throw Error('Storage unavailable');
    window.localStorage.setItem('clipclub-diagnostic-notes',JSON.stringify({device:$('diagnostics-device-label').value.slice(0,80),network:$('diagnostics-network-condition').value,notes:$('diagnostics-notes').value.slice(0,2000),steps:diagnosticsChecklistResults}));
    diagnosticsNotesSaved=true;
  }catch{diagnosticsNotesSaved=false;}
  renderDiagnostics();
}
function setDiagnosticsSuite(suite){
  if(diagnosticsBusy||!Object.hasOwn(diagnosticSuites,suite))return;
  diagnosticsSuite=suite;diagnosticsResult=diagnosticsResults.get(suite)||null;diagnosticsError='';renderDiagnostics();
}
function clientDiagnosticChecks(suite){
  const checks=[];
  if(suite==='all'||suite==='device'){
    const width=window.innerWidth,height=window.innerHeight,scale=window.devicePixelRatio;
    checks.push({category:'device',name:'Webview & display',status:'info',detail:`${navigator.userAgent||'Webview identity unavailable'}. Viewport: ${width||'?'} × ${height||'?'} CSS pixels; pixel ratio ${scale||'?'}. Resize and display scaling still need a visual check.`});
  }
  if(suite==='all'||suite==='media'){
    const player=$('player');
    for(const [name,mime] of [['MP4 · H.264 + AAC','video/mp4; codecs="avc1.42E01E, mp4a.40.2"'],['WebM · VP9 + Opus','video/webm; codecs="vp9, opus"']]){
      let support='';try{support=player.canPlayType?.(mime)||'';}catch{}
      checks.push({category:'media',name:`Playback capability · ${name}`,status:support?'info':'warning',detail:support?`The webview reports “${support}” support. Confirm picture, sound, and seeking with a real clip; this does not test decoding.`:'The webview does not advertise support for this codec combination. Try a known sample and record the outcome in Device checklist.'});
    }
    const mediaError=player.error;
    checks.push({category:'media',name:'Current rating player',status:mediaError?'warning':'info',detail:mediaError?`The player reports media error ${mediaError.code}: ${{1:'playback aborted',2:'network error',3:'decode error',4:'source or codec unsupported'}[mediaError.code]||'unknown error'}. Record the clip and reproduction steps.`:`${player.videoWidth||0} × ${player.videoHeight||0} decoded pixels; ready state ${Number(player.readyState)||0}. No current player error is reported. This snapshot does not establish successful playback.`});
  }
  return checks;
}
function diagnosticChecklistText(){
  const labels={pass:'PASS',fail:'FAIL',skip:'SKIPPED'};
  return 'Hands-on device checklist (manually recorded):\n'+diagnosticSteps.map(step=>`[${labels[diagnosticsChecklistResults[step.id]]||'NOT TESTED'}] ${step.title}`).join('\n');
}
function diagnosticContext(){
  const networks={'same-wifi':'Same Wi-Fi',wired:'Ethernet / mixed wired and Wi-Fi',hotspot:'Phone hotspot','different-networks':'Different networks',offline:'Offline'};
  return {device:$('diagnostics-device-label').value.trim().slice(0,80)||'Not recorded',network:networks[$('diagnostics-network-condition').value]||'Not recorded'};
}
function diagnosticExportText(){
  if(!diagnosticsResult&&diagnosticsSuite!=='checklist')return '';
  const current=diagnosticContext(),{device,network}=diagnosticsResult?.context||current;
  const notes=$('diagnostics-notes').value.trim().slice(0,2000);
  const header=`Test device: ${device}\nTest network: ${network}\nExported: ${new Date().toISOString()}\n\n`;
  return header+(diagnosticsResult?diagnosticsResult.report:'ClipClub hands-on device test report\nNo automated checks are included in this checklist export.\n')+`\nChecklist device: ${current.device}\nChecklist network: ${current.network}\n`+diagnosticChecklistText()+'\n\nTest notes:\n'+(notes||'None recorded.')+'\n';
}
function acceptDiagnosticInbox(status){
  if(!status||typeof status.enabled!=='boolean'||typeof status.configured!=='boolean'||!Number.isInteger(status.pending)||status.pending<0)throw Error('The local client returned incomplete inbox settings.');
  diagnosticsInbox=status;diagnosticsInboxLoaded=true;diagnosticsInboxLoadError='';
  if(!diagnosticsInboxDirty){
    $('diagnostics-inbox-enabled').checked=status.enabled;
    for(const key of ['endpoint','device','session'])$(`diagnostics-inbox-${key}`).value=typeof status[key]==='string'?status[key]:'';
    if(!$('diagnostics-device-label').value&&status.device)$('diagnostics-device-label').value=status.device;
  }
}
function loadDiagnosticInbox(force=false){
  if(diagnosticsInboxLoad)return diagnosticsInboxLoad;
  if(!force&&diagnosticsInboxLoaded&&Date.now()-diagnosticsInboxCheckedAt<5000)return Promise.resolve();
  diagnosticsInboxCheckedAt=Date.now();
  const revision=diagnosticsInboxRevision;
  diagnosticsInboxLoad=(async()=>{
    try{const status=await api('diagnostics/inbox',undefined,'GET');if(revision===diagnosticsInboxRevision)acceptDiagnosticInbox(status);}
    catch(error){if(revision===diagnosticsInboxRevision)diagnosticsInboxLoadError=error.message;}
    finally{diagnosticsInboxLoad=null;renderDiagnosticInbox();}
  })();
  return diagnosticsInboxLoad;
}
function renderDiagnosticInbox(){
  const inbox=diagnosticsInbox,changing=diagnosticsInboxBusy||diagnosticsBusy;
  for(const id of ['diagnostics-inbox-enabled','diagnostics-inbox-endpoint','diagnostics-inbox-token','diagnostics-inbox-device','diagnostics-inbox-session'])$(id).disabled=changing;
  $('run-diagnostics').disabled=changing;
  $('save-diagnostics-inbox').disabled=changing||!diagnosticsInboxLoaded;
  $('save-diagnostics-inbox').textContent=diagnosticsInboxBusy?'Please wait…':'Save inbox settings';
  $('retry-diagnostics-inbox').disabled=changing||!inbox?.enabled||!inbox?.pending;
  $('send-diagnostics-inbox').disabled=changing||!inbox?.enabled||(!diagnosticsResult&&diagnosticsSuite!=='checklist');
  $('diagnostics-inbox-token-help').textContent=inbox?.configured?'An upload key is saved for this inbox address. Leave blank to keep it, or enter a new key when changing the address.':'Use the upload key supplied with your inbox.';
  let status=diagnosticsInboxLoaded?inbox.enabled?'Automatic uploads are on.':'Automatic uploads are off. Pending reports stay on this device.':'Loading inbox settings…';
  if(inbox){
    if(inbox.pending)status+=` ${inbox.pending} report${inbox.pending===1?'':'s'} waiting to upload.`;
    if(inbox.last_sent_at_ms){
      const sent=new Date(inbox.last_sent_at_ms);if(Number.isFinite(sent.getTime()))status+=` Last sent ${sent.toLocaleString()}.`;
    }
  }
  if(diagnosticsInboxNotice)status+=` ${diagnosticsInboxNotice}`;
  if(diagnosticsInboxDirty)status+=' Save your changes to apply them.';
  $('diagnostics-inbox-status').textContent=status;
  const error=diagnosticsInboxError||diagnosticsInboxLoadError||inbox?.error||'';
  $('diagnostics-inbox-error').textContent=error;$('diagnostics-inbox-error').hidden=!error;
}
async function saveDiagnosticInbox(event){
  event?.preventDefault();if(diagnosticsInboxBusy||diagnosticsBusy||!diagnosticsInboxLoaded)return;
  diagnosticsInboxRevision++;
  diagnosticsInboxBusy=true;diagnosticsInboxError='';diagnosticsInboxNotice='';renderDiagnosticInbox();
  const body={enabled:!!$('diagnostics-inbox-enabled').checked,endpoint:$('diagnostics-inbox-endpoint').value.trim(),device:$('diagnostics-inbox-device').value.trim().slice(0,80),session:$('diagnostics-inbox-session').value.trim().slice(0,80)};
  const uploadKey=$('diagnostics-inbox-token').value.trim();if(uploadKey)body.upload_token=uploadKey;
  // The upload key is sent only to the protected local API and never enters browser storage.
  $('diagnostics-inbox-token').value='';
  try{
    const status=await api('diagnostics/inbox',body);
    diagnosticsInboxDirty=false;acceptDiagnosticInbox(status);diagnosticsInboxNotice='Inbox settings saved.';
  }catch(error){diagnosticsInboxError=error.message;}
  finally{diagnosticsInboxBusy=false;renderDiagnosticInbox();}
}
async function sendDiagnosticInbox(snapshot){
  if(diagnosticsInboxBusy||!diagnosticsInbox?.enabled)return;
  const report=snapshot||{suite:diagnosticsSuite,generated_at_ms:diagnosticsResult?.generated_at_ms||Date.now(),report:$('diagnostics-report').value};
  if(!report.report)return;
  diagnosticsInboxBusy=true;diagnosticsInboxError='';diagnosticsInboxNotice='';renderDiagnosticInbox();
  try{
    const response=await api('diagnostics/inbox/reports',report);
    acceptDiagnosticInbox(response.status||response);
    diagnosticsInboxNotice='Report saved to the upload queue.';
  }catch(error){diagnosticsInboxError=`Could not queue this report: ${error.message} Your report is still available below; use Send report to inbox to try again.`;}
  finally{diagnosticsInboxBusy=false;renderDiagnosticInbox();}
}
async function retryDiagnosticInbox(){
  if(diagnosticsInboxBusy||!diagnosticsInbox?.enabled)return;
  diagnosticsInboxBusy=true;diagnosticsInboxError='';diagnosticsInboxNotice='';renderDiagnosticInbox();
  try{const response=await api('diagnostics/inbox/retry',{});acceptDiagnosticInbox(response.status||response);diagnosticsInboxNotice='Upload retry requested.';}
  catch(error){diagnosticsInboxError=error.message;}
  finally{diagnosticsInboxBusy=false;renderDiagnosticInbox();}
}
function renderDiagnostics(){
  loadDiagnosticNotes();
  if(page==='diagnostics'&&!diagnosticsInboxLoad&&!diagnosticsInboxBusy&&(!diagnosticsInboxCheckedAt||Date.now()-diagnosticsInboxCheckedAt>=5000))loadDiagnosticInbox();
  renderDiagnosticInbox();
  if(state&&!diagnosticsPortsReady){
    for(const [id,port] of [['diagnostics-p2p-port',state.p2p_port||42931],['diagnostics-torrent-port',state.torrent_port||42932]]){
      if(!diagnosticsPortEdits.has(id))$(id).value=String(port);
    }
    diagnosticsPortsReady=true;
  }
  const manual=diagnosticsSuite==='checklist',network=diagnosticsSuite==='all'||diagnosticsSuite==='network';
  $('diagnostics-form').hidden=manual;$('diagnostics-manual').hidden=!manual;
  $('diagnostics-network-target').hidden=!network;
  $('diagnostics-suite-title').textContent=diagnosticSuites[diagnosticsSuite].title;
  $('diagnostics-suite-help').textContent=diagnosticSuites[diagnosticsSuite].help;
  document.querySelectorAll('[data-diagnostic-suite]').forEach(button=>{button.setAttribute('aria-pressed',String(button.dataset.diagnosticSuite===diagnosticsSuite));button.disabled=diagnosticsBusy;});
  for(const id of ['diagnostics-peer','diagnostics-p2p-port','diagnostics-torrent-port'])$(id).disabled=diagnosticsBusy||!network;
  $('run-diagnostics').disabled=diagnosticsBusy||diagnosticsInboxBusy;
  $('run-diagnostics').textContent=diagnosticsBusy?'Testing…':'Run tests';
  $('diagnostics-form').setAttribute?.('aria-busy',String(diagnosticsBusy));
  $('diagnostics-status').textContent=diagnosticsBusy?`Running ${diagnosticSuites[diagnosticsSuite].title.toLowerCase()}…`:diagnosticsError?diagnosticsResult?'Tests could not complete. The previous report is shown below.':'Tests could not complete.':diagnosticsResult?.summary||'Ready to test.';
  $('diagnostics-error').hidden=!diagnosticsError;$('diagnostics-error').textContent=diagnosticsError;
  $('diagnostics-results').hidden=!diagnosticsResult&&!manual;
  $('copy-diagnostics').disabled=!diagnosticsResult&&!manual;$('save-diagnostics').disabled=!diagnosticsResult&&!manual;
  $('diagnostics-results-title').textContent=manual?'Checklist report':`${diagnosticSuites[diagnosticsSuite].title} results`;
  $('diagnostics-notes-status').textContent=diagnosticsNotesSaved?'Export your checklist and notes before quitting or starting a new test pass.':'Local saving is unavailable. Keep this window open and copy or save your report.';
  if(manual){
    const markup=diagnosticSteps.map(step=>`<li><div><label for="diagnostic-step-${step.id}">${esc(step.title)}</label><p>${esc(step.detail)}</p></div><select id="diagnostic-step-${step.id}" data-diagnostic-step="${step.id}" aria-label="${esc(step.title)} result">${[['','Not tested'],['pass','Passed'],['fail','Failed'],['skip','Skipped']].map(([value,label])=>`<option value="${value}"${(diagnosticsChecklistResults[step.id]||'')===value?' selected':''}>${label}</option>`).join('')}</select></li>`).join('');
    if($('diagnostics-checklist').innerHTML!==markup)$('diagnostics-checklist').innerHTML=markup;
    const values=Object.values(diagnosticsChecklistResults),tested=values.filter(value=>value==='pass'||value==='fail').length;
    $('diagnostics-checklist-progress').textContent=`${tested} / ${diagnosticSteps.length} tested · ${values.filter(value=>value==='fail').length} failed · ${values.filter(value=>value==='skip').length} skipped`;
  }
  if(!diagnosticsResult&&!manual)return;
  const generated=diagnosticsResult?new Date(diagnosticsResult.generated_at_ms):null;
  $('diagnostics-time').textContent=generated&&Number.isFinite(generated.getTime())?generated.toLocaleString():'';
  const labels={pass:'Passed',warning:'Check',fail:'Failed',info:'Info'};
  const markup=(diagnosticsResult?.checks||[]).map(check=>{
    const status=Object.hasOwn(labels,check.status)?check.status:'info';
    return `<li class="diagnostics-check diagnostics-${status}"><span class="diagnostics-badge">${labels[status]}</span><div><h3>${esc(check.name)}</h3><p>${esc(check.detail)}</p></div></li>`;
  }).join('');
  if($('diagnostics-checks').innerHTML!==markup)$('diagnostics-checks').innerHTML=markup;
  // Keep the generated export stable across routine state polling.
  const exportText=diagnosticExportText(),prior=$('diagnostics-report').value;
  if(prior.replace(/^Exported:.*$/m,'')!==exportText.replace(/^Exported:.*$/m,''))$('diagnostics-report').value=exportText;
}
async function runDiagnostics(event){
  event?.preventDefault();if(diagnosticsBusy||diagnosticsInboxBusy)return;
  diagnosticsError='';
  if(diagnosticsSuite==='checklist')return;
  const suite=diagnosticsSuite,network=suite==='all'||suite==='network';
  const peer=$('diagnostics-peer').value.trim(),body={suite};
  if(network&&peer){
    const parts=peer.split('.');
    if(parts.length!==4||parts.some(part=>!/^\d{1,3}$/.test(part)||Number(part)>255)){
      diagnosticsError='Enter the other device’s IPv4 address, such as 192.168.1.86.';renderDiagnostics();$('diagnostics-peer').focus?.();return;
    }
    body.peer_ip=parts.map(Number).join('.');
  }
  for(const [key,id] of network?[['p2p_port','diagnostics-p2p-port'],['torrent_port','diagnostics-torrent-port']]:[]){
    const text=$(id).value.trim(),number=Number(text);
    if(!/^\d+$/.test(text)||!Number.isInteger(number)||number<1||number>65535){
      diagnosticsError='Use TCP port numbers between 1 and 65535.';renderDiagnostics();$(id).focus?.();return;
    }
    body[key]=number;
  }
  const context=diagnosticContext();
  const inboxRequested=page==='diagnostics'||!!diagnosticsInboxLoad||diagnosticsInboxLoaded;
  diagnosticsBusy=true;renderDiagnostics();
  const controller=new AbortController(),timeout=setTimeout(()=>controller.abort(),40000);
  let completed=null;
  try{
    const result=await api('diagnostics',body,'POST',controller.signal);
    if(typeof result.report!=='string'||!Array.isArray(result.checks))throw Error('The client returned an incomplete diagnostics report.');
    const checks=clientDiagnosticChecks(suite);
    if(checks.length){
      result.checks.push(...checks);
      result.report+='\nWebview observations:\n'+checks.map(check=>`[${check.status.toUpperCase()}] ${check.name}\n${check.detail}\n`).join('\n');
      if(checks.some(check=>check.status==='warning'))result.summary+=' Review the webview capability warnings below.';
    }
    diagnosticsResult={...result,context};diagnosticsResults.set(suite,diagnosticsResult);
    // Snapshot the completed run before any inbox request or further navigation.
    completed={suite,generated_at_ms:result.generated_at_ms,report:diagnosticExportText()};
  }catch(error){diagnosticsError=error.name==='AbortError'?'Tests took too long. Keep ClipClub open and try again.':error.message;}
  finally{clearTimeout(timeout);renderDiagnostics();}
  try{
    if(completed){
      // Navigation does not cancel consent lookup for this completed snapshot.
      // Keep the run busy until it is queued so a second run cannot displace it.
      if(diagnosticsInboxLoad)await diagnosticsInboxLoad;
      else if(!diagnosticsInboxLoaded&&inboxRequested)await loadDiagnosticInbox();
      await sendDiagnosticInbox(completed);
    }
  }finally{diagnosticsBusy=false;renderDiagnostics();}
}
async function copyDiagnostics(){
  const text=$('diagnostics-report').value;if(!text)return;
  try{await navigator.clipboard.writeText(text);toast('Diagnostics report copied.');}
  catch{$('diagnostics-report').focus();$('diagnostics-report').select();toast('Report selected. Copy it with your keyboard or context menu.');}
}
function saveDiagnostics(){
  const text=$('diagnostics-report').value;if(!text)return;
  try{
    const blob=new Blob([text],{type:'text/plain;charset=utf-8'}),url=URL.createObjectURL(blob),link=document.createElement('a');
    const generated=diagnosticsResult?new Date(diagnosticsResult.generated_at_ms):new Date(),stamp=Number.isFinite(generated.getTime())?generated.toISOString().replace(/[:.]/g,'-'):'report';
    link.href=url;link.download=`clipclub-diagnostics-${diagnosticsSuite}-${stamp}.txt`;document.body.appendChild(link);link.click();link.remove();setTimeout(()=>URL.revokeObjectURL(url),1000);
  }catch{toast('Could not save the report. Use Copy report or select the report text.');}
}
function render(){
  if(!state)return;
  renderDiagnostics();
  if(state.notice&&state.notice!==lastNotice){lastNotice=state.notice;toast(state.notice);}
  renderWallet();
  $('seed-count').textContent=state.library.length;$('ready-count').textContent=state.queue.length;$('queue-total').textContent=state.queue.length;
  const peerStatus=state.peers.length?`${state.peers.length} peer${state.peers.length===1?'':'s'} connected`:'Finding peers…';
  $('peer-status').textContent=peerStatus;
  $('discovery-status').textContent=peerStatus;
  $('library-cap').textContent=`${state.library.length} / 100 clips`;
  $('stat-seeds').innerHTML=`${state.library.filter(item=>item.seeding).length} <small>clips</small>`;
  $('stat-storage').innerHTML=`${(state.library.reduce((sum,item)=>sum+item.clip.size,0)/1e6).toFixed(1)} <small>MB</small>`;
  renderLibrary();
  $('queue-slots').innerHTML=Array.from({length:10},(_,i)=>`<span class="${i<state.queue.length?'ready':i===state.queue.length&&state.downloading?'downloading':''}">${i<state.queue.length?'✓':i+1}</span>`).join('');
  const d=state.downloading;
  const downloadStatus=d?`Downloading · ${Math.min(100,Math.round(d.downloaded/Math.max(1,d.total)*100))}%`:state.queue.length>=10?'Queue full':state.peers.length?'Waiting for more clips…':'Waiting for peers…';
  if($('download-status').textContent!==downloadStatus)$('download-status').textContent=downloadStatus;
  $('session-button').disabled=sessionToggleBusy||busy||walletBusy||rollBusy||transactionBusy;
  $('session-button').textContent=state.session?`End session${state.temporary_seeds?` · ${state.temporary_seeds} seeding`:''} →`:'Start session →';
  const next=state.queue[0]||null;
  if(current?.id!==next?.id){
    current=next;
    const player=$('player');player.pause();
    if(next){player.src=mediaUrl(next.id);player.load();}
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
  $('waiting-message').textContent=!state.session?'Start a session to watch and rate.':state.peers.length?'Waiting for a clip from your peers.':'Finding peers automatically…';
  const preview=current?.distribution_preview;
  $('rating-recipient').textContent=!current?'':preview?preview.seeders?`${preview.seeders} eligible permanent seeder${preview.seeders===1?'':'s'} · rewards calculated locally.${!seederPaymentsFit()?' Settle the current session before adding more seeder payments.':''}`:'No registered permanent seeders are available for payment. You can skip for free.':clipCanReceiveRating(current)?`Recipient · ${current.beneficiary}`:'This clip needs a registered wallet before it can receive ✨. You can skip for free.';
  document.querySelectorAll('[data-score]').forEach(button=>{const score=Number(button.dataset.score);button.title=score>0&&preview?`${score} ✨ from your wallet; ${fmtTokens(preview.reward_units[score],0)} ✨ to each of ${preview.seeders} permanent seeders`:'';button.disabled=busy||rollBusy||walletBusy||transactionBusy||!playing||(score>0&&(!ratingWalletSelected()||!ratingDraftEditable()||queuedRatingCount()>=100||!clipCanReceiveRating(current)||!seederPaymentsFit()||BigInt(score)*TOKEN_SCALE>spendableTokenBalance()||!!pendingTransaction));});
  $('peer-id').textContent=state.profile_id;$('data-folder').textContent=state.data_folder;
  $('network-ports').textContent=`Peer TCP ${state.p2p_port} · BitTorrent TCP ${state.torrent_port}`;
  $('listen-addresses').innerHTML=state.addresses.length?state.addresses.map(address=>`<button type="button" data-address="${esc(address)}" title="Copy peer address">${esc(address)}</button>`).join(''):'Finding addresses…';
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
  const player=$('roll-player');player.loop=false;
  const key=result?`${result.id}:${clip?.id||''}`:'';
  if(key!==rollMediaKey){
    rollMediaKey=key;player.pause();
    if(clip){player.src=mediaUrl(clip.id);}
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

async function refresh(){const version=++refreshVersion;try{const next=await api('state',undefined,'GET');if(version!==refreshVersion)return;state=next;render();}catch(error){if(initialized&&version===refreshVersion){$('peer-status').textContent='Client disconnected';$('discovery-status').textContent='Client disconnected';}}}
async function rate(score){
  if(busy||rollBusy||walletBusy||transactionBusy||page!=='rate'||!current||!state?.session)return;
  const clipId=current.id;
  if(score>0){
    if(!ratingWalletSelected()){toast('Select a configured wallet before queueing paid ratings.');return;}
    if(!ratingDraftEditable()||pendingTransaction){toast('Confirm the frozen session transaction before queueing more ratings.');return;}
    if(queuedRatingCount()>=100){toast('This session has 100 ratings. Settle or discard it before queueing more.');return;}
    if(!clipCanReceiveRating(current)){toast('This clip needs an eligible permanent seeder wallet before you can queue ✨.');return;}
    if(!seederPaymentsFit()){toast('This session has reached its limit of 1000 seeder payments. Settle it before rating more clips.');return;}
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
document.querySelectorAll('button[data-page]').forEach(button=>button.addEventListener('click',()=>navigate(button.dataset.page)));
document.querySelectorAll('[data-goto]').forEach(button=>button.addEventListener('click',()=>navigate(button.dataset.goto)));
$('rating-keys').innerHTML=Array.from({length:10},(_,score)=>`<button data-score="${score}" aria-label="Rate ${score}, queue ${score} ✨" disabled>${score}</button>`).join('');
$('rating-keys').addEventListener('click',event=>{const button=event.target.closest('[data-score]');if(button)rate(Number(button.dataset.score));});
$('roll-form').addEventListener('submit',event=>{event.preventDefault();roll();});
$('settle-roll').addEventListener('click',settleRoll);
$('recover-roll').addEventListener('click',recoverRoll);
$('connect-wallet').addEventListener('click',connectWallet);
$('manage-wallet').addEventListener('click',openNativeWallet);
$('close-wallet').addEventListener('click',()=>{if(!walletStartupRequired()&&!walletFormBusy)$('wallet-dialog').close();});
$('wallet-dialog').addEventListener('cancel',event=>{if(walletStartupRequired()||walletFormBusy)event.preventDefault();});
$('wallet-dialog').addEventListener('close',()=>{clearWalletSecrets();if(walletStartupRequired())openNativeWallet();});
$('wallet-mode').addEventListener('change',()=>{clearWalletSecrets();$('wallet-form-error').hidden=true;renderWalletDialog();});
$('wallet-form').addEventListener('submit',submitNativeWallet);
for(const id of ['copy-wallet-address','wallet-dialog-copy'])$(id).addEventListener('click',()=>copy(localWallet()?.address));
for(const id of ['backup-wallet','wallet-dialog-backup'])$(id).addEventListener('click',backupNativeWallet);
$('wallet-approval-form').addEventListener('submit',approveNativeTransaction);
$('wallet-approval-cancel').addEventListener('click',()=>finishNativeApproval(false));
$('wallet-approval-dialog').addEventListener('cancel',event=>{event.preventDefault();finishNativeApproval(false);});
$('wallet-approval-dialog').addEventListener('close',()=>finishNativeApproval(false));
$('retry-transaction').addEventListener('click',retryTransaction);
$('roll-wager').addEventListener('input',()=>{$('roll-error').hidden=true;renderRoll();});
$('roll-odds').innerHTML=Array.from({length:10},(_,k)=>`<div><strong>${k}×</strong><span>${(2**(9-k)/1023*100).toFixed(k>5?3:2)}%</span></div>`).join('');
$('roll-player').addEventListener('error',()=>{if(state?.roll?.clip){$('roll-error').textContent='Your system could not play this clip’s codec. Your roll and payout are saved.';$('roll-error').hidden=false;}});
document.addEventListener('keydown',event=>{if(event.repeat||event.ctrlKey||event.metaKey||event.altKey||event.target.isContentEditable||/INPUT|TEXTAREA|SELECT/.test(event.target.tagName)||$('preview-dialog').open||$('wallet-dialog').open||$('wallet-approval-dialog').open)return;if(event.key==='Escape'&&page!=='home'){event.preventDefault();navigate('home');}else if(/^[0-9]$/.test(event.key)&&page==='rate'){event.preventDefault();rate(Number(event.key));}});
$('session-button').addEventListener('click',toggleRatingSession);
$('settle-rating-session').addEventListener('click',settleRatingSession);
$('discard-rating-session').addEventListener('click',discardRatingSession);
$('unlock-rating-session').addEventListener('click',unlockRatingSession);
$('rating-session-recover').addEventListener('click',recoverRatingSession);
$('register-seeder').addEventListener('click',async()=>{
  if(walletBusy||transactionBusy||rollBusy||busy)return;
  walletBusy=true;renderWallet();
  try{await sendCryptoTransaction('register_seeder');toast('Seeder wallet registered. Your permanent seeds can receive rewards.');}
  catch(error){toast(error.message);}finally{walletBusy=false;walletPhase='';render();}
});
$('dropzone').addEventListener('click',event=>{if(event.target!==$('file-input'))$('file-input').click();});
$('dropzone').addEventListener('keydown',event=>{if(event.key==='Enter'||event.key===' '){event.preventDefault();$('file-input').click();}});
$('file-input').addEventListener('change',event=>uploads([...event.target.files]));
for(const name of ['dragenter','dragover'])$('dropzone').addEventListener(name,event=>{event.preventDefault();$('dropzone').classList.add('dragover');});
for(const name of ['dragleave','drop'])$('dropzone').addEventListener(name,event=>{event.preventDefault();$('dropzone').classList.remove('dragover');});
$('dropzone').addEventListener('drop',event=>uploads([...event.dataTransfer.files]));
$('library-sort').addEventListener('change',renderLibrary);
$('library').addEventListener('click',async event=>{
  const preview=event.target.closest('[data-preview]'),remove=event.target.closest('[data-remove]');
  if(preview){const clip=state.library.find(item=>item.clip.id===preview.dataset.preview).clip;$('preview-title').textContent=clip.title;$('preview-video').src=mediaUrl(clip.id);$('preview-dialog').showModal();$('preview-video').play().catch(()=>{});}
  if(remove){remove.disabled=true;try{await api(`remove/${remove.dataset.remove}`,{});toast('Removed from your seed collection.');await refresh();}catch(error){toast(error.message);remove.disabled=false;}}
});
$('close-preview').addEventListener('click',()=>$('preview-dialog').close());
$('preview-dialog').addEventListener('close',()=>{$('preview-video').pause();$('preview-video').removeAttribute('src');$('preview-video').load();});
$('player').addEventListener('error',()=>{if(current)toast('Your system could not play this codec. Press 0 to pass without spending tokens.');});
$('copy-id').addEventListener('click',()=>copy(state.profile_id));
$('check-updates').addEventListener('click',checkUpdates);
$('install-update').addEventListener('click',installUpdate);
$('download-update').addEventListener('click',()=>openUpdate('download'));
$('update-release').addEventListener('click',()=>openUpdate('release'));
$('listen-addresses').addEventListener('click',event=>{const button=event.target.closest('[data-address]');if(button)copy(button.dataset.address);});
$('diagnostics-form').addEventListener('submit',runDiagnostics);
$('copy-diagnostics').addEventListener('click',copyDiagnostics);
$('save-diagnostics').addEventListener('click',saveDiagnostics);
$('diagnostics-inbox-form').addEventListener('submit',saveDiagnosticInbox);
$('send-diagnostics-inbox').addEventListener('click',()=>sendDiagnosticInbox());
$('retry-diagnostics-inbox').addEventListener('click',retryDiagnosticInbox);
for(const id of ['diagnostics-inbox-endpoint','diagnostics-inbox-token','diagnostics-inbox-device','diagnostics-inbox-session'])$(id).addEventListener('input',()=>{diagnosticsInboxDirty=true;renderDiagnosticInbox();});
$('diagnostics-inbox-enabled').addEventListener('change',()=>{diagnosticsInboxDirty=true;renderDiagnosticInbox();});
$('diagnostics-menu').addEventListener('click',event=>{const button=event.target.closest('[data-diagnostic-suite]');if(button)setDiagnosticsSuite(button.dataset.diagnosticSuite);});
$('diagnostics-checklist').addEventListener('change',event=>{
  const select=event.target.closest('[data-diagnostic-step]');if(!select)return;
  const id=select.dataset.diagnosticStep;
  if(!diagnosticSteps.some(step=>step.id===id)||!['','pass','fail','skip'].includes(select.value))return;
  if(select.value)diagnosticsChecklistResults[id]=select.value;else delete diagnosticsChecklistResults[id];
  saveDiagnosticNotes();$(select.id)?.focus?.({preventScroll:true});
});
for(const id of ['diagnostics-device-label','diagnostics-notes'])$(id).addEventListener('input',saveDiagnosticNotes);
$('diagnostics-network-condition').addEventListener('change',saveDiagnosticNotes);
for(const id of ['diagnostics-p2p-port','diagnostics-torrent-port'])$(id).addEventListener('input',()=>diagnosticsPortEdits.add(id));
window.addEventListener('pagehide',()=>{$('roll-player').pause();if(state?.session)fetch('/api/session',{method:'POST',headers:{'x-clipcrop-token':token,'Content-Type':'application/json'},body:'{"active":false}',keepalive:true}).catch(()=>{});});
(async()=>{await refresh();initialized=true;setInterval(refresh,1500);})();
