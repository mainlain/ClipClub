// Optional live, read-only smoke test; does not need a wallet or spend ETH.
import {Contract, FetchRequest, JsonRpcProvider} from 'ethers';
const mainnet = process.argv.includes('--mainnet');
const chainId = mainnet ? 4326n : 6343n;
const url = mainnet ? 'https://mainnet.megaeth.com/rpc' : 'https://carrot.megaeth.com/rpc';
const address = mainnet ? '0x7a53a6eFA81c426838fcf4824E6e207923969b36' : '0x4e1673dcAA38136b5032F27ef93423162aF977Cc';
const request = new FetchRequest(url); request.timeout = 15_000;
const provider = new JsonRpcProvider(request, undefined, {cacheTimeout: -1});
try {
  if ((await provider.getNetwork()).chainId !== chainId) throw Error('RPC chain ID mismatch.');
  if ((await provider.getCode(address)) === '0x') throw Error('Verifier code absent.');
  const response = await fetch('https://api.drand.sh/v2/chains/52db9ba70e0cc0f6eaf7803dd07447a1f5477735fd3f661792ba94600c84e971/rounds/latest', {signal: AbortSignal.timeout(15_000)});
  if (!response.ok) throw Error(`drand HTTP ${response.status}`);
  const beacon = await response.json();
  if (!Number.isSafeInteger(beacon.round) || !/^[0-9a-f]{96}$/i.test(beacon.signature)) throw Error('Unexpected drand beacon.');
  const oracle = new Contract(address, ['function verifyNormalized(uint64,bytes) view returns(bool,bytes32,bytes32)'], provider);
  const result = await oracle.verifyNormalized(beacon.round, `0x${beacon.signature}`);
  if (!result[0] || /^0x0+$/.test(result[2])) throw Error('Live drand beacon failed verification.');
  console.log(`MegaETH ${chainId}: canonical verifier accepted quicknet round ${beacon.round}; chain-scoped hash ${result[2]}. No transaction sent.`);
} finally {provider.destroy();}
