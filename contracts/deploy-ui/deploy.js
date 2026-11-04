'use strict';
const $=id=>document.getElementById(id);
const token=document.querySelector('meta[name="deployment-token"]').content;
const EXPECTED_TREASURY='0xcbbf8e516f9a3ad56efe28a0aebf69edc4639637';
const EXPECTED_GENESIS_UNITS='1000000000000000';
let review=null,account=null,estimatedGas=null,gasLimit=null,busy=false,submittedHash=null,deploymentRecord=null;
const provider=()=>window.ethereum?.request?window.ethereum:null;
function show(message,error=false){$('status').textContent=message;$('status').classList.toggle('error',error);}
function formatEther(units){const value=BigInt(units);const whole=value/1000000000000000000n;const fraction=(value%1000000000000000000n).toString().padStart(18,'0').replace(/0+$/,'');return `${whole}${fraction?`.${fraction}`:''} ETH`;}
function deploymentKey(){return `joah:mainnet-deployment:${review?.bytecodeHash}`;}
function storeHash(hash){submittedHash=hash;try{if(hash)window.localStorage.setItem(deploymentKey(),hash);else window.localStorage.removeItem(deploymentKey());}catch{}renderControls();}
function renderControls(){
  $('connect').disabled=busy||!review||!provider();$('recover').disabled=busy||!review||!provider();
  $('deploy').disabled=busy||!review||!account||!estimatedGas||!!submittedHash||!!deploymentRecord;
  $('connect').textContent=busy?'Waiting for wallet…':submittedHash?'Connect wallet':'Connect wallet & estimate gas';
  $('deploy').textContent=submittedHash?'Deployment submitted':'Deploy 좋아 on MegaETH';
}
async function ensureChain(){
  const wallet=provider();if(!wallet)throw Error('Open this local URL in a browser with an Ethereum wallet.');
  if(BigInt(await wallet.request({method:'eth_chainId'}))!==4326n){
    try{await wallet.request({method:'wallet_switchEthereumChain',params:[{chainId:'0x10e6'}]});}
    catch(error){if(Number(error.code)!==4902)throw error;await wallet.request({method:'wallet_addEthereumChain',params:[{chainId:'0x10e6',chainName:'MegaETH Mainnet',nativeCurrency:{name:'Ether',symbol:'ETH',decimals:18},rpcUrls:[review.rpcUrl],blockExplorerUrls:[review.explorerUrl]}]});}
  }
  if(BigInt(await wallet.request({method:'eth_chainId'}))!==4326n)throw Error('Your wallet must be on MegaETH mainnet, chain 4326.');
  return wallet;
}
function creationTransaction(from){return {from,data:review.unsignedTransaction.data,value:'0x0',chainId:'0x10e6'};}
async function estimateFor(wallet,from){
  const estimate=BigInt(await wallet.request({method:'eth_estimateGas',params:[creationTransaction(from)]}));
  if(estimate<=0n)throw Error('The wallet returned an invalid deployment gas estimate.');
  estimatedGas=estimate;gasLimit=estimate+estimate/5n;
  $('wallet-gas').textContent=`${estimate.toLocaleString()} gas · limit ${gasLimit.toLocaleString()}`;
  try{const price=BigInt(await wallet.request({method:'eth_gasPrice'}));$('gas-cost').textContent=`${formatEther(estimate*price)} · current gas price`;}catch{$('gas-cost').textContent='Review the fee in your wallet';}
}
async function connect(){
  if(busy)return;busy=true;renderControls();
  try{
    const wallet=provider();if(!wallet)throw Error('Open this local URL in a browser with an Ethereum wallet.');
    const accounts=await wallet.request({method:'eth_requestAccounts'});
    if(!/^0x[0-9a-fA-F]{40}$/.test(accounts?.[0]||''))throw Error('Select a wallet account to pay the deployment gas.');
    const selected=accounts[0];await ensureChain();account=selected;$('deployer').textContent=selected;
    if(!submittedHash){show('Estimating gas from the selected deployer…');await estimateFor(wallet,selected);show('Ready for your review. Click Deploy only when you want to open the wallet’s mainnet transaction approval.');}
    else show(`Deployment ${submittedHash} has already been submitted. Check its receipt below; do not deploy again.`);
  }catch(error){account=null;estimatedGas=null;show(error.message||'Wallet connection was cancelled.',true);}
  finally{busy=false;renderControls();}
}
async function checkTransaction(hash,wallet){
  const transaction=await wallet.request({method:'eth_getTransactionByHash',params:[hash]});
  if(!transaction)return null;
  if(transaction.to!=null||transaction.input?.toLowerCase()!==review.unsignedTransaction.data.toLowerCase()||BigInt(transaction.value||0)!==0n||transaction.chainId&&BigInt(transaction.chainId)!==4326n)throw Error('This transaction does not create the reviewed 좋아 contract on MegaETH mainnet.');
  return transaction;
}
async function pollDeployment(hash,wallet){
  $('recovery-hash').value=hash;
  for(let attempt=0;attempt<180;attempt++){
    const receipt=await wallet.request({method:'eth_getTransactionReceipt',params:[hash]});
    if(receipt){
      if(BigInt(receipt.status||0)!==1n){const error=Error('Deployment reverted. No contract was created. Review the transaction in your wallet before trying again.');error.code='REVERTED';throw error;}
      if(!/^0x[0-9a-fA-F]{40}$/.test(receipt.contractAddress||''))throw Error('The receipt did not contain a deployed contract address.');
      const transaction=await checkTransaction(hash,wallet);if(!transaction)throw Error('The wallet could not load the deployment transaction to check its bytecode. Retry receipt recovery.');
      if(BigInt(await wallet.request({method:'eth_chainId'}))!==4326n)throw Error('The wallet network changed. Switch to MegaETH mainnet and recover the receipt.');
      deploymentRecord={chainId:4326,name:'좋아',symbol:'✨',decimals:6,treasury:review.treasury,genesisSupplyBaseUnits:review.genesisSupplyBaseUnits,
        verifier:review.verifier,contractAddress:receipt.contractAddress,transactionHash:hash,deployer:transaction.from,blockNumber:receipt.blockNumber,bytecodeHash:review.bytecodeHash,confirmation:'Wallet RPC reports inclusion; not Ethereum L1 finality.'};
      $('contract-address').textContent=receipt.contractAddress;$('transaction-link').textContent=hash;$('transaction-link').href=`${review.explorerUrl}/tx/${hash}`;
      $('configuration').textContent=`CLIPCLUB_MEGAETH_CHAIN_ID=4326 \\\nCLIPCLUB_JOAH_CONTRACT=${receipt.contractAddress} \\\n./target/release/clipclub`;
      $('result').hidden=false;show(`Deployment included. 좋아 is at ${receipt.contractAddress}. Download the record below.`);renderControls();return;
    }
    show(`Waiting for deployment inclusion… ${hash}. You can recover this hash if the browser closes.`);
    await new Promise(resolve=>setTimeout(resolve,1000));
  }
  throw Error('The deployment is still awaiting a receipt. Keep its transaction hash and use Check deployment receipt.');
}
async function deploy(){
  if(busy||submittedHash||deploymentRecord||!review||!account)return;
  busy=true;renderControls();
  try{
    const selected=account,wallet=await ensureChain();
    const accounts=await wallet.request({method:'eth_accounts'});if(accounts?.[0]?.toLowerCase()!==selected.toLowerCase())throw Error('Your selected wallet changed. Connect and review the deployer again.');
    show('Refreshing the gas estimate for your deployer…');await estimateFor(wallet,selected);
    if(BigInt(await wallet.request({method:'eth_chainId'}))!==4326n||(await wallet.request({method:'eth_accounts'}))?.[0]?.toLowerCase()!==selected.toLowerCase())throw Error('The wallet network or account changed before deployment. Connect again.');
    show('Approve the contract creation in your wallet. It deploys on MegaETH mainnet and spends ETH for gas.');
    const hash=await wallet.request({method:'eth_sendTransaction',params:[{...creationTransaction(selected),gas:`0x${gasLimit.toString(16)}`} ]});
    if(!/^0x[0-9a-fA-F]{64}$/.test(hash||''))throw Error('The wallet did not return a valid transaction hash. Check your wallet before retrying deployment.');
    storeHash(hash);await pollDeployment(hash,wallet);
  }catch(error){if(error.code==='REVERTED'){storeHash(null);estimatedGas=null;}show(error.message||'Wallet deployment was cancelled.',true);}
  finally{busy=false;renderControls();}
}
async function recover(){
  if(busy||!review)return;
  const hash=$('recovery-hash').value.trim();if(!/^0x[0-9a-fA-F]{64}$/.test(hash)){show('Enter a complete 0x-prefixed deployment transaction hash.',true);return;}
  busy=true;renderControls();
  try{const wallet=await ensureChain();const transaction=await checkTransaction(hash,wallet);if(!transaction)throw Error('The wallet cannot find this transaction on MegaETH mainnet yet.');storeHash(hash);await pollDeployment(hash,wallet);}
  catch(error){if(error.code==='REVERTED'){storeHash(null);estimatedGas=null;}show(error.message,true);}
  finally{busy=false;renderControls();}
}
$('connect').addEventListener('click',connect);$('deploy').addEventListener('click',deploy);$('recover').addEventListener('click',recover);
$('download').addEventListener('click',()=>{if(!deploymentRecord)return;const url=URL.createObjectURL(new Blob([JSON.stringify(deploymentRecord,null,2)+'\n'],{type:'application/json'}));const anchor=document.createElement('a');anchor.href=url;anchor.download='joah-mainnet-deployment.json';anchor.click();setTimeout(()=>URL.revokeObjectURL(url),1000);});
function invalidateWallet(){account=null;estimatedGas=null;$('deployer').textContent='Connect again to confirm the selected wallet';$('wallet-gas').textContent='Connect to estimate';$('gas-cost').textContent='—';renderControls();if(!busy&&!deploymentRecord)show('The wallet account or network changed. Connect and review it again.');}
// Some injected wallet proxies throw when optional event APIs are accessed.
// Deployment still rechecks the selected account and chain before sending.
try{const wallet=provider(),on=wallet?.on;if(typeof on==='function'){on.call(wallet,'accountsChanged',invalidateWallet);on.call(wallet,'chainChanged',invalidateWallet);}}catch{}
(async()=>{
  try{
    const response=await fetch('/api/deployment',{headers:{'x-deployment-token':token}});if(!response.ok)throw Error('Could not load the checked deployment. Restart the local review server.');
    review=await response.json();if(review.chainId!==4326||review.genesisSupplyBaseUnits!==EXPECTED_GENESIS_UNITS||review.treasury.toLowerCase()!==EXPECTED_TREASURY)throw Error('The deployment does not match the reviewed mainnet launch.');
    $('treasury').textContent=review.treasury;$('verifier').textContent=review.verifier;$('bytecode-hash').textContent=review.bytecodeHash;$('prepared-gas').textContent=BigInt(review.remoteGasEstimate).toLocaleString();$('compiler').textContent=`Solidity ${review.compiler} · ${review.evmVersion} · optimizer ${review.optimizer.runs} runs`;
    $('unsigned-data').textContent=JSON.stringify(review.unsignedTransaction,null,2);
    let saved=null;try{saved=window.localStorage.getItem(deploymentKey());}catch{}if(/^0x[0-9a-fA-F]{64}$/.test(saved||'')){submittedHash=saved;$('recovery-hash').value=saved;show(`A deployment was already submitted: ${saved}. Check its receipt below before doing anything else.`);}else show('Deployment checked. Connect your wallet when you are ready to estimate its gas.');
    if(!provider())$('wallet-help').textContent='Open this local URL in Chrome, Firefox, or another browser with an Ethereum wallet. This preview cannot access a browser extension wallet.';
    renderControls();
  }catch(error){show(error.message,true);}
})();
