// Local review surface. Only a user's browser wallet can sign or broadcast.
import fs from 'node:fs';
import http from 'node:http';
import path from 'node:path';
import {randomBytes, timingSafeEqual} from 'node:crypto';
import {fileURLToPath} from 'node:url';
import {ContractFactory, getAddress, keccak256} from 'ethers';

const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const MAINNET_VERIFIER='0x7a53a6eFA81c426838fcf4824E6e207923969b36';
const TREASURY='0xcBBF8e516F9a3ad56Efe28A0aEBf69Edc4639637';
const GENESIS='1000000000000000';
export async function loadReview({deploymentPath=path.join(root,'deployment.json'),artifactPath=path.join(root,'artifacts/Joah.json')}={}){
  const deployment=JSON.parse(fs.readFileSync(deploymentPath,'utf8'));
  const artifact=JSON.parse(fs.readFileSync(artifactPath,'utf8'));
  if(deployment.chainId!==4326||BigInt(deployment.unsignedTransaction?.chainId||0)!==4326n)throw Error('The review surface requires MegaETH mainnet chain 4326.');
  if(getAddress(deployment.treasury)!==getAddress(TREASURY))throw Error('The treasury must be the reviewed recipient address.');
  if(deployment.genesisSupplyBaseUnits!==GENESIS)throw Error('The reviewed genesis supply is exactly 1,000,000,000 ✨ (six decimals).');
  if(getAddress(deployment.verifier)!==getAddress(MAINNET_VERIFIER))throw Error('The canonical MegaETH mainnet drand verifier is required.');
  const transaction=deployment.unsignedTransaction;
  if(transaction.to!=null||BigInt(transaction.value||0)!==0n)throw Error('The transaction must create a contract with no ETH value attached.');
  const factory=new ContractFactory(artifact.abi,artifact.bytecode);
  if(!factory.interface.getFunction('rateClips(bytes32,bytes32[],uint8[],bytes32)')||!factory.interface.getEvent('RatingsSettled(bytes32,address,uint256,uint256,bytes32)'))throw Error('Rebuild the contract with session batching before deployment.');
  const expected=await factory.getDeployTransaction(TREASURY,BigInt(GENESIS),MAINNET_VERIFIER);
  if(transaction.data?.toLowerCase()!==expected.data.toLowerCase())throw Error('Deployment initcode does not match the compiled contract and reviewed constructor arguments.');
  const bytecodeHash=keccak256(expected.data);
  if(deployment.bytecodeHash&&deployment.bytecodeHash!==bytecodeHash)throw Error('Deployment initcode hash does not match.');
  if(!/^\d+$/.test(deployment.remoteGasEstimate||''))throw Error('A valid prepared network gas estimate is required.');
  return {chainId:4326,chainHex:'0x10e6',chainName:'MegaETH Mainnet',rpcUrl:'https://mainnet.megaeth.com/rpc',explorerUrl:'https://mega.etherscan.io',
    tokenName:'좋아',tokenSymbol:'✨',decimals:6,treasuryEns:null,treasury:getAddress(TREASURY),genesisSupply:'1000000000',genesisSupplyBaseUnits:GENESIS,
    verifier:MAINNET_VERIFIER,compiler:deployment.compiler,evmVersion:deployment.evmVersion,optimizer:deployment.optimizer,
    remoteGasEstimate:deployment.remoteGasEstimate,bytecodeHash,creationBytecodeHash:keccak256(artifact.bytecode),
    unsignedTransaction:{chainId:'0x10e6',data:expected.data,value:'0x0'}};
}
function sameSecret(actual,expected){if(typeof actual!=='string')return false;const a=Buffer.from(actual),b=Buffer.from(expected);return a.length===b.length&&timingSafeEqual(a,b);}
export function createReviewServer(review,{sessionToken=randomBytes(32).toString('hex'),uiDir=path.join(root,'deploy-ui')}={}){
  let origin;
  const server=http.createServer((request,response)=>{
    const headers={'Content-Security-Policy':"default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; object-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",'X-Content-Type-Options':'nosniff','Referrer-Policy':'no-referrer','Cache-Control':'no-store'};
    const send=(status,content,type='text/plain; charset=utf-8')=>{response.writeHead(status,{...headers,'Content-Type':type});response.end(content);};
    const expectedHost=origin&&new URL(origin).host;
    if(request.headers.host!==expectedHost||request.headers.origin&&request.headers.origin!==origin){send(403,'Invalid local request.');return;}
    if(request.method!=='GET'){send(405,'Read-only review server.');return;}
    if(request.url==='/joah.png'){
      send(200,fs.readFileSync(path.join(root,'..','assets','joah-256.png')),'image/png');return;
    }
    if(request.url==='/api/deployment'){
      if(!sameSecret(request.headers['x-deployment-token'],sessionToken)){send(401,'Invalid review session.');return;}
      send(200,JSON.stringify(review),'application/json; charset=utf-8');return;
    }
    const file=request.url==='/'?'index.html':request.url==='/deploy.js'?'deploy.js':request.url==='/deploy.css'?'deploy.css':null;
    if(!file){send(404,'Not found.');return;}
    let content=fs.readFileSync(path.join(uiDir,file),'utf8');
    if(file==='index.html')content=content.replace('__SESSION_TOKEN__',sessionToken);
    send(200,content,file.endsWith('.js')?'text/javascript; charset=utf-8':file.endsWith('.css')?'text/css; charset=utf-8':'text/html; charset=utf-8');
  });
  server.on('listening',()=>{origin=`http://127.0.0.1:${server.address().port}`;});
  return {server,sessionToken,get origin(){return origin;}};
}
async function main(){
  const launch=JSON.parse(fs.readFileSync(path.join(root,'mainnet-launch.json'),'utf8'));
  if(launch.deploymentStatus==='on_hold')throw Error(`Deployment is on hold: ${launch.deploymentHoldReason}`);
  if(launch.deploymentStatus==='deployed')throw Error(`This launch is already deployed at ${launch.contractAddress}. Use the existing contract; do not deploy it again.`);
  const review=await loadReview();
  const session=createReviewServer(review);
  session.server.listen(0,'127.0.0.1',()=>{
    console.log(`좋아 mainnet deployment review: ${session.origin}`);
    console.log(`Treasury: ${review.treasury}; genesis: 1,000,000,000 ✨.`);
    console.log('Open this URL in your Ethereum wallet browser. Only your click and wallet approval can deploy; this server never signs or broadcasts.');
  });
  for(const signal of ['SIGINT','SIGTERM'])process.on(signal,()=>session.server.close(()=>process.exit(0)));
}
if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url))main().catch(error=>{console.error(error.message);process.exitCode=1;});
