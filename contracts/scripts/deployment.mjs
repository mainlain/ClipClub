// Prepare a reviewable, unsigned deployment. This script never reads a key or
// signs/broadcasts a transaction. Its gas estimate comes from MegaETH itself.
import fs from 'node:fs';
import path from 'node:path';
import {ContractFactory, FetchRequest, JsonRpcProvider, getAddress, keccak256, parseUnits, toQuantity} from 'ethers';
import {build, root} from './build.mjs';

const chains = {
  4326: {rpc: 'https://mainnet.megaeth.com/rpc', verifier: '0x7a53a6eFA81c426838fcf4824E6e207923969b36'},
  6343: {rpc: 'https://carrot.megaeth.com/rpc', verifier: '0x4e1673dcAA38136b5032F27ef93423162aF977Cc'},
};
const launch = JSON.parse(fs.readFileSync(path.join(root, 'mainnet-launch.json'), 'utf8'));
if (launch.deploymentStatus === 'on_hold') throw Error(`Deployment is on hold: ${launch.deploymentHoldReason}`);
const chainId = Number(process.env.MEGAETH_CHAIN_ID || launch.chainId);
if (!chains[chainId]) throw Error('MEGAETH_CHAIN_ID must be 4326 or 6343.');
if (chainId === 6343 && (!process.env.JOAH_TREASURY || !process.env.JOAH_GENESIS_SUPPLY)) {
  throw Error('For testnet, explicitly set JOAH_TREASURY and JOAH_GENESIS_SUPPLY (whole tokens, up to 6 decimals).');
}
const treasury = getAddress(process.env.JOAH_TREASURY || launch.treasury);
const supplyTokens = process.env.JOAH_GENESIS_SUPPLY || launch.genesisSupplyTokens;
if (!/^(0|[1-9][0-9]*)(\.[0-9]{1,6})?$/.test(supplyTokens)) throw Error('Invalid genesis supply.');
const supply = parseUnits(supplyTokens, 6);
const request = new FetchRequest(process.env.MEGAETH_RPC_URL || chains[chainId].rpc);
request.timeout = 15_000;
const provider = new JsonRpcProvider(request, undefined, {cacheTimeout: -1});
try {
  if ((await provider.getNetwork()).chainId !== BigInt(chainId)) throw Error('RPC chain ID mismatch.');
  if ((await provider.getCode(chains[chainId].verifier)) === '0x') throw Error('No code at the canonical verifier address.');
  const artifact = build();
  const transaction = await new ContractFactory(artifact.abi, `0x${artifact.evm.bytecode.object}`).getDeployTransaction(treasury, supply, chains[chainId].verifier);
  const estimate = await provider.estimateGas({...transaction, from: treasury});
  const unsigned = {chainId: toQuantity(chainId), from: treasury, data: transaction.data,
    value: '0x0', gas: toQuantity(estimate + estimate / 5n)};
  const output = {chainId, tokenName: launch.name, tokenSymbol: launch.symbol, decimals: launch.decimals,
    treasury, treasuryEns: treasury.toLowerCase() === launch.treasury.toLowerCase() ? launch.treasuryEns : null,
    genesisSupplyTokens: supplyTokens, genesisSupplyBaseUnits: supply.toString(), verifier: chains[chainId].verifier,
    bytecodeHash: keccak256(transaction.data), creationBytecodeHash: keccak256(`0x${artifact.evm.bytecode.object}`),
    compiler: '0.8.30', optimizer: {enabled: true, runs: 200}, evmVersion: 'shanghai',
    remoteGasEstimate: estimate.toString(), unsignedTransaction: unsigned};
  fs.writeFileSync(path.join(root, 'deployment.json'), JSON.stringify(output, null, 2) + '\n');
  console.log(`Prepared deployment.json for MegaETH ${chainId}. Remote gas estimate ${estimate}; no transaction was sent.`);
} finally {provider.destroy();}
