import fs from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import solc from 'solc';
import {Interface} from 'ethers';

export const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export function compile(extraSources = {}) {
  const input = {
    language: 'Solidity',
    sources: {'src/Joah.sol': {content: fs.readFileSync(path.join(root, 'src/Joah.sol'), 'utf8')}, ...extraSources},
    settings: {optimizer: {enabled: true, runs: 200}, evmVersion: 'shanghai',
      outputSelection: {'*': {'*': ['abi', 'evm.bytecode.object', 'evm.deployedBytecode.object']}}},
  };
  const output = JSON.parse(solc.compile(JSON.stringify(input), {import: name => {
    try {return {contents: fs.readFileSync(path.join(root, 'node_modules', name), 'utf8')};}
    catch {return {error: `Import unavailable: ${name}`};}
  }}));
  const errors = (output.errors || []).filter(error => error.severity === 'error');
  if (errors.length) throw Error(errors.map(error => error.formattedMessage).join('\n'));
  return output.contracts;
}

export function build({abiOnly = false} = {}) {
  const artifact = compile()['src/Joah.sol'].Joah;
  const contractInterface = new Interface(artifact.abi);
  const selectors = {functions: {}, events: {}};
  contractInterface.forEachFunction(fragment => selectors.functions[fragment.format('sighash')] = fragment.selector);
  contractInterface.forEachEvent(fragment => selectors.events[fragment.format('sighash')] = fragment.topicHash);
  if (!abiOnly) {
    fs.mkdirSync(path.join(root, 'artifacts'), {recursive: true});
    fs.writeFileSync(path.join(root, 'artifacts/Joah.json'), JSON.stringify({
      contractName: 'Joah', compiler: solc.version(), optimizer: {enabled: true, runs: 200}, evmVersion: 'shanghai',
      abi: artifact.abi, bytecode: `0x${artifact.evm.bytecode.object}`, deployedBytecode: `0x${artifact.evm.deployedBytecode.object}`,
    }, null, 2) + '\n');
  }
  fs.writeFileSync(path.join(root, 'abi-selectors.json'), JSON.stringify(selectors, null, 2) + '\n');
  console.log(`${abiOnly ? 'Checked' : 'Built'} Joah: ${artifact.evm.deployedBytecode.object.length / 2} runtime bytes; ABI selectors refreshed${abiOnly ? '; deployment artifacts unchanged' : ''}.`);
  return artifact;
}
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) build({abiOnly: process.argv.includes('--abi-only')});
