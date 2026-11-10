const assert=require('node:assert/strict');
const fs=require('node:fs');
const vm=require('node:vm');
const html=fs.readFileSync('ui/index.html','utf8');
const elements=new Map([...html.matchAll(/\bid="([^"]+)"/g)].map(([,id])=>[id,{id,innerHTML:'',textContent:'',value:id==='library-sort'?'recency':id==='roll-wager'?'1':'',hidden:false,listeners:{},classList:{toggle(){},add(){},remove(){}},addEventListener(name,fn){this.listeners[name]=fn;},pause(){this.paused=true;},load(){},play(){this.paused=false;return Promise.resolve();},removeAttribute(name){delete this[name];}}]));
let focused;
for(const element of elements.values()){
  element.setAttribute=function(name,value){this[name]=value;};
  element.focus=function(){focused=this.id;};
}
const menuButtons=new Map(['seed','rate','roll','settings','diagnostics'].map(page=>[page,{setAttribute(){},focus(){focused=`menu-${page}`;}}]));
const calls=[];
const context=vm.createContext({document:{body:{dataset:{page:'home'}},getElementById(id){assert.ok(elements.has(id),id);return elements.get(id);},querySelector(s){if(s.startsWith('meta'))return{content:'test-token'};if(s.startsWith('#'))return elements.get(s.slice(1))||null;const button=s.match(/^\[data-page="(\w+)"\]$/);return button?menuButtons.get(button[1]):null;},querySelectorAll(s){return s==='.page'?['home','seed','rate','roll','settings','diagnostics'].map(id=>elements.get('page-'+id)):[];},addEventListener(){}},window:{addEventListener(){},scrollTo(){}},fetch(url){calls.push(url);return new Promise(()=>{});},FormData:class{},setTimeout(){},clearTimeout(){},setInterval(){}});
vm.runInContext(fs.readFileSync('ui/app.js','utf8'),context);
assert.equal(vm.runInContext('page',context),'home');
assert.deepEqual(calls,['/api/state'],'startup must only load state, without opening a rating session');
const fixture={balance:100,library:[],queue:[],peers:[],addresses:[],session:false,profile_id:'peer-a',earned:0};
context.fixture=fixture;
vm.runInContext('state=fixture;initialized=true;',context);
const sessions=[];
context.fetch=async(url,options)=>{if(url==='/api/session'){fixture.session=JSON.parse(options.body).active;sessions.push(fixture.session);}return{ok:true,json:async()=>fixture};};
(async()=>{
  for(const page of ['seed','rate','roll','settings','diagnostics']){
    await vm.runInContext(`navigate("${page}")`,context);
    assert.equal(focused,`page-${page}`,'entering a screen must focus its accessible region');
    assert.equal(elements.get(`page-${page}`).hidden,false);
    await vm.runInContext('navigate("home")',context);
    assert.equal(focused,`menu-${page}`,'returning home must restore the originating menu button');
  }
  sessions.length=0;
  await vm.runInContext('Promise.all([navigate("rate"),navigate("home"),navigate("rate"),navigate("home")])',context);
  assert.deepEqual(sessions,[true,false,true,false]);assert.equal(fixture.session,false);assert.equal(context.document.body.dataset.page,'home');assert.equal(elements.get('page-home').hidden,false);assert.equal(elements.get('page-rate').hidden,true);assert.equal(elements.get('player').paused,true);assert.equal(focused,'menu-rate');
  console.log('Passed: screen navigation preserves keyboard focus; rapid navigation leaves Home visible with Rate stopped and playback paused.');
})().catch(error=>{console.error(error);process.exitCode=1;});
