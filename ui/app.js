'use strict';
const $ = id => document.getElementById(id);
const token = document.querySelector('meta[name="api-token"]').content;
const esc = value => String(value).replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
let lastNotice=null;
let state=null, page='home', current=null, busy=false, uploading=false, settingsDirty=false, initialized=false, toastTimer;
let rollBusy=false, rollMediaKey='', rollStartedKey='';
let updateStatus=null, updateBusy=false, updateOpening=false, updateNoticeVersion=null, updateTask=null;
let refreshVersion=0;
let navigationQueue=Promise.resolve();
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
function fmtPoints(number){return Math.max(0,Number(number)).toLocaleString(undefined,{minimumFractionDigits:2,maximumFractionDigits:2});}
function duration(seconds){return `${Math.floor(seconds/60)}:${String(Math.floor(seconds%60)).padStart(2,'0')}`;}
function sortLibrary(library,sort){
  return [...library].sort((a,b)=>{
    const recent=(b.added_at_ms||0)-(a.added_at_ms||0);
    const tie=recent||a.clip.id.localeCompare(b.clip.id);
    if(sort==='points')return b.points_earned-a.points_earned||tie;
    if(sort==='rating')return (b.average_rating??-1)-(a.average_rating??-1)||tie;
    return tie;
  });
}
function renderLibrary(){
  if(!state)return;
  const library=sortLibrary(state.library,$('library-sort').value);
  const markup=library.length?library.map(({clip:c,seeding,points_earned,average_rating,rating_count})=>`<div class="library-row"><button class="clip-thumb" data-preview="${c.id}" aria-label="Preview ${esc(c.title)}">▷</button><div class="row-detail"><h3>${esc(c.title)}</h3><p>${esc(c.extension.toUpperCase())} · ${duration(c.duration_secs)} · ${c.width} × ${c.height} · ${(c.size/1e6).toFixed(2)} MB</p><p class="clip-stats"><span><strong>${fmtPoints(points_earned)}</strong> points earned</span><span title="Average of 1–9 ratings; skips are excluded.">${average_rating===null?'Unrated':`Rating <strong>${average_rating.toFixed(1)} / 9</strong> · ${rating_count} rating${rating_count===1?'':'s'}`}</span></p></div><span class="seeding-tag">${seeding?'● Seeding':'○ Unavailable'}</span><div class="row-actions"><button data-torrent="${c.id}">Torrent ↗</button><button data-remove="${c.id}">Remove</button></div></div>`).join(''):'<div class="library-empty"><span aria-hidden="true">▧</span><h3>No clips yet</h3></div>';
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
  const [integer,fraction]=fmtPoints(state.balance).split('.');
  $('balance').innerHTML=`${esc(integer)}<span>.${fraction||'00'}</span>`;
  $('profile-name').textContent=state.settings.display_name;
  $('seed-count').textContent=state.library.length;$('ready-count').textContent=state.queue.length;$('queue-total').textContent=state.queue.length;
  $('peer-status').textContent=state.peers.length?`${state.peers.length} peer${state.peers.length===1?'':'s'} connected`:'Finding peers…';
  $('library-cap').textContent=`${state.library.length} / 100 clips`;
  $('stat-seeds').innerHTML=`${state.library.filter(item=>item.seeding).length} <small>clips</small>`;
  $('stat-earned').textContent=fmtPoints(state.earned);
  $('stat-storage').innerHTML=`${(state.library.reduce((sum,item)=>sum+item.clip.size,0)/1e6).toFixed(1)} <small>MB</small>`;
  $('collection-count').textContent=state.library.length;
  renderLibrary();
  $('queue-slots').innerHTML=Array.from({length:10},(_,i)=>`<span class="${i<state.queue.length?'ready':i===state.queue.length&&state.downloading?'downloading':''}">${i<state.queue.length?'✓':i+1}</span>`).join('');
  const d=state.downloading;
  const downloadStatus=d?`Downloading · ${Math.min(100,Math.round(d.downloaded/Math.max(1,d.total)*100))}%`:state.queue.length>=10?'Queue full':!state.session&&!state.settings.prefetch?'Downloads paused':state.peers.length?'Waiting for more clips…':'Waiting for peers…';
  if($('download-status').textContent!==downloadStatus)$('download-status').textContent=downloadStatus;
  $('session-button').textContent=state.session?`End session${state.temporary_seeds?` · ${state.temporary_seeds} seeding`:''} →`:'Start session →';
  const next=state.queue[0]||null;
  if(current?.id!==next?.id){
    current=next;
    const player=$('player');player.pause();
    if(next){player.src=mediaUrl(next.id);player.muted=state.settings.muted;player.volume=state.settings.volume;player.load();}
    else{player.removeAttribute('src');player.load();}
  }
  const playing=!!current&&state.session&&page==='rate';
  $('player-shell').classList.toggle('has-video',playing);$('player-empty').hidden=playing;
  $('clip-details').hidden=!current;
  $('clip-title').textContent=current?current.title:'';
  $('clip-meta').textContent=current?`${current.extension.toUpperCase()} · ${duration(current.duration_secs)} · ${current.width} × ${current.height}`:'';
  if(playing && $('player').paused&&!$('player').error)$('player').play().catch(()=>{});
  $('waiting-title').textContent=state.session?'Waiting for clips':'Session paused';
  $('waiting-message').textContent=!state.session?'Start a session to watch and rate.':state.peers.length?'Waiting for a clip from your peers.':'Connect a peer to get started.';
  $('connect-peer').hidden=!state.session||state.peers.length>0;
  document.querySelectorAll('[data-score]').forEach(button=>button.disabled=busy||!playing||Number(button.dataset.score)>state.balance);
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
function fmtRollPoints(number){return Number(number).toLocaleString(undefined,{minimumFractionDigits:2,maximumFractionDigits:6});}
function rollWager(){
  const raw=$('roll-wager').value.trim();
  const value=Number(raw);
  if(!/^(?:\d+(?:\.\d{0,6})?|\.\d{1,6})$/.test(raw)||!Number.isFinite(value)||value<=0||!Number.isSafeInteger(Math.round(value*1e6)))return null;
  return value;
}
function renderRoll(){
  if(!state)return;
  const wager=rollWager();
  $('roll-button').disabled=rollBusy||wager===null||wager>state.balance;
  $('roll-button').textContent=rollBusy?'Rolling…':'Roll →';
  $('roll-wager').disabled=rollBusy;
  $('roll-wager').max=String(state.balance);
  const result=state.roll||null,clip=result?.clip||null;
  $('roll-result').hidden=!result;
  if(result){
    $('roll-multiplier').textContent=`${result.multiplier}×`;
    $('roll-result-title').textContent=result.multiplier===0?'No luck this time':result.multiplier===1?'Wager returned':`${fmtRollPoints(result.payout)} points returned`;
    $('roll-result-detail').textContent=`Wager ${fmtRollPoints(result.wager)} · Total payout ${fmtRollPoints(result.payout)}`;
  }
  const player=$('roll-player');
  player.loop=false;
  const key=result?`${result.id}:${clip?.id||''}`:'';
  if(key!==rollMediaKey){
    rollMediaKey=key;
    player.pause();
    if(clip){player.src=mediaUrl(clip.id);player.muted=state.settings.muted;player.volume=state.settings.volume;}
    else player.removeAttribute('src');
    player.load();
  }
  $('roll-player-shell').classList.toggle('has-video',!!clip);
  $('roll-player-empty').hidden=!!clip;
  $('roll-clip-details').hidden=!clip;
  $('roll-clip-title').textContent=clip?.title||'';
  $('roll-clip-meta').textContent=clip?`Rating ${result.multiplier} · ${duration(clip.duration_secs)} · ${clip.width} × ${clip.height}`:'';
  $('roll-waiting-title').textContent=!result?'Take a chance':result.multiplier===0?'0× · No clip':`${result.multiplier}× · Your roll`;
  $('roll-waiting-message').textContent=!result?'Wager points to roll a multiplier and discover a clip.':result.multiplier===0?'Your wager was lost. Roll again when you’re ready.':result.message||'Preparing a clip with this rating…';
  // A result may acquire its clip after downloading. Start it once when visible;
  // polling must never restart an ended clip or undo a manual pause.
  if(clip&&page==='roll'&&rollStartedKey!==key){rollStartedKey=key;player.play().catch(()=>{});}
}
async function roll(){
  if(rollBusy||page!=='roll'||!state)return;
  const wager=rollWager();
  if(wager===null||wager>state.balance){$('roll-error').textContent=wager===null?'Enter a positive wager with up to 6 decimal places.':'Your wager exceeds your points balance.';$('roll-error').hidden=false;return;}
  rollBusy=true;$('roll-error').hidden=true;renderRoll();
  try{
    const result=await api('roll',{wager});
    // Ignore state polls begun before this wager completed.
    refreshVersion++;
    // A poll can deliver the prepared clip before the original roll response.
    // Keep that newer playback state for this result instead of clearing it.
    state.roll=state.roll?.id===result.id&&state.roll.clip?state.roll:result;
    state.balance=result.balance;render();
    await refresh();
  }catch(error){$('roll-error').textContent=error.message;$('roll-error').hidden=false;}
  finally{rollBusy=false;renderRoll();}
}
async function refresh(){const version=++refreshVersion;try{const next=await api('state',undefined,'GET');if(version!==refreshVersion)return;state=next;render();}catch(error){if(initialized&&version===refreshVersion)$('peer-status').textContent='Client disconnected';}}
async function rate(score){
  if(busy||page!=='rate'||!current||!state?.session)return;
  busy=true;render();
  try{const result=await api('rate',{id:current.id,score});toast(score===0?'Passed. No points spent.':`${score} points given · ${result.seeders} seeder${result.seeders===1?'':'s'} earned ${fmtPoints(result.reward_per_seeder)} each.`);await refresh();}
  catch(error){toast(error.message);}finally{busy=false;render();}
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
$('rating-keys').innerHTML=Array.from({length:10},(_,score)=>`<button data-score="${score}" aria-label="Rate ${score}, spend ${score} points" disabled>${score}</button>`).join('');
$('rating-keys').addEventListener('click',event=>{const button=event.target.closest('[data-score]');if(button)rate(Number(button.dataset.score));});
$('roll-form').addEventListener('submit',event=>{event.preventDefault();roll();});
$('roll-wager').addEventListener('input',()=>{$('roll-error').hidden=true;renderRoll();});
$('roll-odds').innerHTML=Array.from({length:10},(_,k)=>`<div><strong>${k}×</strong><span>${(2**(9-k)/1023*100).toFixed(k>5?3:2)}%</span></div>`).join('');
$('roll-player').addEventListener('error',()=>{if(state?.roll?.clip){$('roll-error').textContent='Your system could not play this clip’s codec. Your roll and payout are saved.';$('roll-error').hidden=false;}});
document.addEventListener('keydown',event=>{if(event.repeat||event.ctrlKey||event.metaKey||event.altKey||event.target.isContentEditable||/INPUT|TEXTAREA|SELECT/.test(event.target.tagName)||$('preview-dialog').open)return;if(event.key==='Escape'&&page!=='home'){event.preventDefault();navigate('home');}else if(/^[0-9]$/.test(event.key)&&page==='rate'){event.preventDefault();rate(Number(event.key));}});
$('session-button').addEventListener('click',()=>queueNavigation(async()=>{if(page!=='rate'||!state)return;await api('session',{active:!state.session});await refresh();}));
$('dropzone').addEventListener('click',event=>{if(event.target!==$('file-input'))$('file-input').click();});
$('dropzone').addEventListener('keydown',event=>{if(event.key==='Enter'||event.key===' '){event.preventDefault();$('file-input').click();}});
$('file-input').addEventListener('change',event=>uploads([...event.target.files]));
for(const name of ['dragenter','dragover'])$('dropzone').addEventListener(name,event=>{event.preventDefault();$('dropzone').classList.add('dragover');});
for(const name of ['dragleave','drop'])$('dropzone').addEventListener(name,event=>{event.preventDefault();$('dropzone').classList.remove('dragover');});
$('dropzone').addEventListener('drop',event=>uploads([...event.dataTransfer.files]));
$('library-sort').addEventListener('change',renderLibrary);
$('library').addEventListener('click',async event=>{
  const preview=event.target.closest('[data-preview]'),remove=event.target.closest('[data-remove]'),torrent=event.target.closest('[data-torrent]');
  if(preview){const clip=state.library.find(item=>item.clip.id===preview.dataset.preview).clip;$('preview-title').textContent=clip.title;$('preview-video').src=mediaUrl(clip.id);$('preview-dialog').showModal();$('preview-video').play().catch(()=>{});}
  if(remove){remove.disabled=true;try{await api(`remove/${remove.dataset.remove}`,{});toast('Removed from your seed collection.');await refresh();}catch(error){toast(error.message);remove.disabled=false;}}
  if(torrent){try{const response=await fetch(`/api/torrent/${torrent.dataset.torrent}`,{headers:{'x-clipcrop-token':token}});if(!response.ok)throw Error('Could not export torrent.');const blob=URL.createObjectURL(await response.blob());const link=document.createElement('a');link.href=blob;link.download=`${torrent.dataset.torrent}.torrent`;link.click();setTimeout(()=>URL.revokeObjectURL(blob),30000);}catch(error){toast(error.message);}}
});
$('close-preview').addEventListener('click',()=>$('preview-dialog').close());
$('preview-dialog').addEventListener('close',()=>{$('preview-video').pause();$('preview-video').removeAttribute('src');$('preview-video').load();});
$('player').addEventListener('error',()=>{if(current)toast('Your system could not play this codec. Press 0 to pass without spending points.');});
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
(async()=>{await refresh();initialized=true;setInterval(refresh,1500);})();
