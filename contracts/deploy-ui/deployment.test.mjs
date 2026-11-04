import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import vm from 'node:vm';
import http from 'node:http';
import {once} from 'node:events';
import {fileURLToPath} from 'node:url';
import {createReviewServer,loadReview} from '../scripts/deploy-server.mjs';
import {compile} from '../scripts/build.mjs';
import {AbiCoder,ContractFactory,keccak256} from 'ethers';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const contractArtifact=compile()['src/Joah.sol'].Joah;
const artifact={abi:contractArtifact.abi,bytecode:`0x${contractArtifact.evm.bytecode.object}`};
const treasury='0xcBBF8e516F9a3ad56Efe28A0aEBf69Edc4639637',verifier='0x7a53a6eFA81c426838fcf4824E6e207923969b36';
const creation=await new ContractFactory(artifact.abi,artifact.bytecode).getDeployTransaction(treasury,1000000000000000n,verifier);
const prepared={chainId:4326,treasury,genesisSupplyBaseUnits:'1000000000000000',verifier,compiler:'0.8.30',evmVersion:'shanghai',optimizer:{enabled:true,runs:200},remoteGasEstimate:'60000000',bytecodeHash:keccak256(creation.data),unsignedTransaction:{...creation,chainId:'0x10e6',value:'0x0'}};
const fixtureFolder=fs.mkdtempSync(path.join(os.tmpdir(),'joah-review-fixture-'));
const deploymentPath=path.join(fixtureFolder,'deployment.json'),artifactPath=path.join(fixtureFolder,'artifact.json');
fs.writeFileSync(deploymentPath,JSON.stringify(prepared));fs.writeFileSync(artifactPath,JSON.stringify(artifact));
test.after(()=>fs.rmSync(fixtureFolder,{recursive:true,force:true}));
const review=await loadReview({deploymentPath,artifactPath});
const source=fs.readFileSync(path.join(root,'deploy-ui/deploy.js'),'utf8');
const html=fs.readFileSync(path.join(root,'deploy-ui/index.html'),'utf8');
const hash='0x'+'a'.repeat(64),deployer='0x1111111111111111111111111111111111111111',contract='0x4444444444444444444444444444444444444444';
function fixture(){
  const elements=new Map([...html.matchAll(/\bid="([^"]+)"/g)].map(([,id])=>[id,{value:'',textContent:'',disabled:false,hidden:false,listeners:{},classList:{toggle(){}},addEventListener(name,fn){this.listeners[name]=fn;}}]));
  const calls=[];const wallet={async request(request){calls.push(request);switch(request.method){case 'eth_accounts':case 'eth_requestAccounts':return [deployer];case 'eth_chainId':return '0x10e6';case 'eth_estimateGas':return '0x3938700';case 'eth_gasPrice':return '0x1';case 'eth_sendTransaction':return hash;case 'eth_getTransactionReceipt':return {status:'0x1',contractAddress:contract,blockNumber:'0x123'};case 'eth_getTransactionByHash':return {from:deployer,to:null,input:review.unsignedTransaction.data,value:'0x0',chainId:'0x10e6'};default:throw Error(request.method);}}};
  const context=vm.createContext({document:{getElementById:id=>elements.get(id),querySelector:()=>({content:'token'})},window:{ethereum:wallet,localStorage:{getItem(){return null;},setItem(){},removeItem(){}}},fetch:async()=>({ok:true,json:async()=>review}),setTimeout(){}});
  vm.runInContext(source,context);return {calls,elements,wallet,run:code=>vm.runInContext(code,context)};
}
async function ready(app){for(let i=0;i<10&&app.elements.get('prepared-gas').textContent==='';i++)await new Promise(setImmediate);}
test('prepared constructor, treasury and supply are checked before the review is exposed',async()=>{
  assert.equal(review.chainId,4326);assert.equal(review.genesisSupplyBaseUnits,'1000000000000000');
  assert.equal(review.treasury,treasury);
  const constructorArgs=AbiCoder.defaultAbiCoder().decode(['address','uint256','address'],`0x${review.unsignedTransaction.data.slice(artifact.bytecode.length)}`);
  assert.equal(constructorArgs[0],treasury);assert.equal(constructorArgs[1],1000000000000000n);assert.equal(constructorArgs[2],verifier);
  assert.match(html,/1,000,000,000 ✨/);assert.doesNotMatch(html,/vote4valentine|\.eth<\/strong>/);
  const folder=fs.mkdtempSync(path.join(os.tmpdir(),'joah-review-'));try{
    const original=prepared;
    // Predeployment changes must invalidate old unsigned constructor calldata,
    // even if its metadata and initcode hash have been replaced with current values.
    const oldTreasury='0x2333827F8098a6395321C3f4B2ff9C8b0b5c3252';
    const factory=new ContractFactory(artifact.abi,artifact.bytecode);
    for(const [recipient,units] of [[oldTreasury,1000000000000000n],[treasury,1000000000000n],[oldTreasury,1000000000000n]]){
      const stale=await factory.getDeployTransaction(recipient,units,verifier);
      const file=path.join(folder,'stale-deployment.json');
      fs.writeFileSync(file,JSON.stringify({...prepared,unsignedTransaction:{...prepared.unsignedTransaction,data:stale.data},bytecodeHash:keccak256(stale.data)}));
      await assert.rejects(loadReview({deploymentPath:file,artifactPath}),/initcode/);
    }
    for(const change of [{chainId:1},{treasury:deployer},{genesisSupplyBaseUnits:'999999'},{unsignedTransaction:{...original.unsignedTransaction,data:'0x00'}},{bytecodeHash:'0x'+'f'.repeat(64)}]){
      const file=path.join(folder,'deployment.json');fs.writeFileSync(file,JSON.stringify({...original,...change}));await assert.rejects(loadReview({deploymentPath:file,artifactPath}));
    }
  }finally{fs.rmSync(folder,{recursive:true,force:true});}
});
test('local review endpoint rejects cross-origin, DNS rebinding, missing token and writes',async()=>{
  const session=createReviewServer(review);session.server.listen(0,'127.0.0.1');await once(session.server,'listening');
  try{
    assert.equal((await fetch(session.origin)).status,200);
    assert.equal((await fetch(session.origin+'/api/deployment')).status,401);
    assert.equal((await fetch(session.origin+'/api/deployment',{headers:{'x-deployment-token':session.sessionToken}})).status,200);
    assert.equal(await new Promise((resolve,reject)=>http.get(session.origin,{headers:{Host:'attacker.example'}},response=>{response.resume();resolve(response.statusCode);}).on('error',reject)),403);
    assert.equal((await fetch(session.origin,{headers:{Origin:'https://attacker.example'}})).status,403);
    assert.equal((await fetch(session.origin+'/api/deployment',{method:'POST',headers:{'x-deployment-token':session.sessionToken}})).status,405);
    assert.equal((await fetch(session.origin+'/../deployment.json')).status,404);
    assert.match((await fetch(session.origin)).headers.get('content-security-policy'),/frame-ancestors 'none'/);
  }finally{await new Promise(resolve=>session.server.close(resolve));}
});
test('opening the page never requests accounts or signs; the deployer estimate uses the selected wallet',async()=>{
  const app=fixture();await ready(app);assert.equal(app.calls.length,0);assert.equal(app.elements.get('deploy').disabled,true);
  await app.run('connect()');assert.equal(app.elements.get('deployer').textContent,deployer);
  const estimates=app.calls.filter(call=>call.method==='eth_estimateGas');assert.equal(estimates.length,1);assert.equal(estimates[0].params[0].from,deployer);assert.notEqual(deployer,review.treasury);
  assert.equal(app.calls.filter(call=>call.method==='eth_sendTransaction').length,0);
});
test('only explicit deployment sends once with reviewed creation data and shows its receipt',async()=>{
  const app=fixture();await ready(app);await app.run('connect()');await app.run('deploy()');await app.run('deploy()');
  const sends=app.calls.filter(call=>call.method==='eth_sendTransaction');assert.equal(sends.length,1);assert.equal(sends[0].params[0].from,deployer);assert.equal(sends[0].params[0].data,review.unsignedTransaction.data);assert.equal(sends[0].params[0].chainId,'0x10e6');assert.equal(sends[0].params[0].to,undefined);
  assert.equal(app.run('deploymentRecord.treasury'),treasury);assert.equal(app.run('deploymentRecord.genesisSupplyBaseUnits'),'1000000000000000');assert.equal(app.run('Object.hasOwn(deploymentRecord,"treasuryEns")'),false);
  assert.equal(app.elements.get('result').hidden,false);assert.equal(app.elements.get('contract-address').textContent,contract);assert.match(app.elements.get('configuration').textContent,new RegExp(contract));
});
test('wallets which remain on another chain cannot deploy',async()=>{
  const app=fixture();await ready(app);const original=app.wallet.request;app.wallet.request=async request=>request.method==='eth_chainId'?'0x1':request.method==='wallet_switchEthereumChain'?null:original(request);
  await app.run('connect()');await app.run('deploy()');assert.equal(app.calls.filter(call=>call.method==='eth_sendTransaction').length,0);assert.match(app.elements.get('status').textContent,/must be on MegaETH mainnet/);
});
test('receipt recovery rejects a different contract initcode without sending a transaction',async()=>{
  const app=fixture();await ready(app);app.elements.get('recovery-hash').value=hash;const original=app.wallet.request;app.wallet.request=async request=>request.method==='eth_getTransactionByHash'?{to:null,input:'0x00',value:'0x0',chainId:'0x10e6'}:original(request);
  await app.run('recover()');assert.equal(app.calls.filter(call=>call.method==='eth_sendTransaction').length,0);assert.match(app.elements.get('status').textContent,/does not create the reviewed/);
});
