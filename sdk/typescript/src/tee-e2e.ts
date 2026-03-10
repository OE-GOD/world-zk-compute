/**
 * TypeScript SDK — TEE Verifier E2E test
 *
 * Tests the full TEE lifecycle on a local Anvil instance:
 *   owner, pause/unpause, registerEnclave, submitResult,
 *   getResult, isResultValid, finalize, ownership transfer, revokeEnclave
 *
 * Usage:
 *   npx tsx src/tee-e2e.ts --rpc-url <url> --private-key <key> --contract <addr>
 */

import {
  createPublicClient,
  createWalletClient,
  http,
  keccak256,
  encodePacked,
  defineChain,
  type Hex,
} from 'viem';
import { privateKeyToAccount } from 'viem/accounts';
import { TEEVerifier } from './tee-verifier';

// ── Anvil well-known accounts ──────────────────────────────────────────────

const ENCLAVE_KEY =
  '0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d' as Hex;
const ENCLAVE_ADDR =
  '0x70997970C51812dc3A010C7d01b50e0d17dc79C8' as Hex;

const NEW_OWNER_KEY =
  '0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a' as Hex;
const NEW_OWNER_ADDR =
  '0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC' as Hex;

// ── Helpers ─────────────────────────────────────────────────────────────────

let passed = 0;
let failed = 0;

function ok(msg: string) {
  passed++;
  console.log(`  [OK] ${msg}`);
}
function fail(msg: string) {
  failed++;
  console.error(`  [FAIL] ${msg}`);
}

async function anvilIncreaseTime(rpcUrl: string, seconds: number) {
  await fetch(rpcUrl, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      jsonrpc: '2.0',
      method: 'evm_increaseTime',
      params: [seconds],
      id: 1,
    }),
  });
  await fetch(rpcUrl, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      jsonrpc: '2.0',
      method: 'evm_mine',
      params: [],
      id: 2,
    }),
  });
}

/**
 * Sign TEE attestation matching the contract verification logic:
 *   message = keccak256(encodePacked(modelHash, inputHash, keccak256(result)))
 *   then EIP-191 personal sign
 */
async function signAttestation(
  modelHash: Hex,
  inputHash: Hex,
  resultData: Hex,
  enclaveKey: Hex,
): Promise<Hex> {
  const resultHash = keccak256(resultData);
  const packed = encodePacked(
    ['bytes32', 'bytes32', 'bytes32'],
    [modelHash, inputHash, resultHash],
  );
  const messageHash = keccak256(packed);

  const account = privateKeyToAccount(enclaveKey);
  return account.signMessage({ message: { raw: messageHash } });
}

// ── Main ────────────────────────────────────────────────────────────────────

async function main() {
  const { parseArgs } = await import('node:util');

  const { values } = parseArgs({
    options: {
      'rpc-url': { type: 'string' },
      'private-key': { type: 'string' },
      contract: { type: 'string' },
    },
  });

  if (!values['rpc-url'] || !values['private-key'] || !values.contract) {
    console.error(
      'Usage: npx tsx src/tee-e2e.ts \\\n' +
        '  --rpc-url <url> --private-key <key> --contract <addr>',
    );
    process.exit(1);
  }

  const rpcUrl = values['rpc-url']!;
  const adminKey = values['private-key']! as Hex;
  const contractAddress = values.contract! as Hex;

  const adminAccount = privateKeyToAccount(adminKey);
  const adminAddr = adminAccount.address;

  const chain = defineChain({
    id: 31337,
    name: 'Anvil',
    nativeCurrency: { name: 'Ether', symbol: 'ETH', decimals: 18 },
    rpcUrls: { default: { http: [rpcUrl] } },
  });
  const publicClient = createPublicClient({ chain, transport: http(rpcUrl) });

  // ── Test 1: owner() ────────────────────────────────────────────────────

  console.log('--- Test 1: owner() ---');
  const verifier = new TEEVerifier({
    rpcUrl,
    privateKey: adminKey,
    contractAddress,
  });

  const owner = await verifier.owner();
  if (owner.toLowerCase() === adminAddr.toLowerCase()) {
    ok(`owner() = ${owner}`);
  } else {
    fail(`owner() expected ${adminAddr}, got ${owner}`);
  }

  // ── Test 2: paused() — should be false initially ───────────────────────

  console.log('--- Test 2: paused() ---');
  const isPaused = await verifier.paused();
  if (!isPaused) {
    ok('paused() = false (initial)');
  } else {
    fail(`paused() expected false, got ${isPaused}`);
  }

  // ── Test 3: pause / unpause ────────────────────────────────────────────

  console.log('--- Test 3: pause / unpause ---');
  let tx = await verifier.pause();
  ok(`pause() tx = ${tx.slice(0, 18)}...`);

  const pausedAfter = await verifier.paused();
  if (pausedAfter) {
    ok('paused() = true after pause()');
  } else {
    fail(`paused() expected true, got ${pausedAfter}`);
  }

  tx = await verifier.unpause();
  ok(`unpause() tx = ${tx.slice(0, 18)}...`);

  const unpausedAfter = await verifier.paused();
  if (!unpausedAfter) {
    ok('paused() = false after unpause()');
  } else {
    fail(`paused() expected false, got ${unpausedAfter}`);
  }

  // ── Test 4: registerEnclave ────────────────────────────────────────────

  console.log('--- Test 4: registerEnclave ---');
  const imageHash = keccak256(
    encodePacked(['string'], ['test-enclave-image-v1']),
  );
  tx = await verifier.registerEnclave(ENCLAVE_ADDR, imageHash);
  ok(`registerEnclave() tx = ${tx.slice(0, 18)}...`);

  // ── Test 5: submitResult ───────────────────────────────────────────────

  console.log('--- Test 5: submitResult ---');
  const modelHash = keccak256(
    encodePacked(['string'], ['xgboost-model-weights']),
  );
  const inputHash = keccak256(encodePacked(['string'], ['test-input-data']));
  const resultData = '0xdeadbeef' as Hex;

  const attestation = await signAttestation(
    modelHash,
    inputHash,
    resultData,
    ENCLAVE_KEY,
  );
  ok(`attestation signed (${attestation.length} chars)`);

  const preBlock = await publicClient.getBlockNumber();

  tx = await verifier.submitResult(
    modelHash,
    inputHash,
    resultData,
    attestation,
    '0.1',
  );
  ok(`submitResult() tx = ${tx.slice(0, 18)}...`);

  // Compute result ID
  const submitBlock = preBlock + 1n;
  const resultId = keccak256(
    encodePacked(
      ['address', 'bytes32', 'bytes32', 'uint256'],
      [adminAddr, modelHash, inputHash, submitBlock],
    ),
  );
  ok(`computed resultId = ${resultId.slice(0, 18)}...`);

  // ── Test 6: getResult ──────────────────────────────────────────────────

  console.log('--- Test 6: getResult ---');
  const result = await verifier.getResult(resultId);
  // viem returns the tuple as an object with named fields
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const r = result as any;
  const submitter = (r.submitter ?? r[1]) as string;
  const enclave = (r.enclave ?? r[0]) as string;
  const finalized = (r.finalized ?? r[11]) as boolean;
  const challenged = (r.challenged ?? r[12]) as boolean;

  if (submitter.toLowerCase() === adminAddr.toLowerCase()) {
    ok(`getResult().submitter = ${submitter}`);
  } else {
    fail(`getResult().submitter expected ${adminAddr}, got ${submitter}`);
  }

  if (enclave.toLowerCase() === ENCLAVE_ADDR.toLowerCase()) {
    ok(`getResult().enclave = ${enclave}`);
  } else {
    fail(`getResult().enclave expected ${ENCLAVE_ADDR}, got ${enclave}`);
  }

  if (!finalized) {
    ok('getResult().finalized = false (before finalization)');
  } else {
    fail('getResult().finalized expected false');
  }

  if (!challenged) {
    ok('getResult().challenged = false (no challenge)');
  } else {
    fail('getResult().challenged expected false');
  }

  // ── Test 7: isResultValid before finalization ──────────────────────────

  console.log('--- Test 7: isResultValid (before finalize) ---');
  const isValidBefore = await verifier.isResultValid(resultId);
  if (!isValidBefore) {
    ok('isResultValid() = false (not yet finalized)');
  } else {
    fail('isResultValid() expected false before finalize');
  }

  // ── Test 8: fast-forward + finalize ────────────────────────────────────

  console.log('--- Test 8: finalize ---');
  await anvilIncreaseTime(rpcUrl, 3601);
  ok('time fast-forwarded by 3601 seconds');

  tx = await verifier.finalize(resultId);
  ok(`finalize() tx = ${tx.slice(0, 18)}...`);

  // ── Test 9: isResultValid after finalization ───────────────────────────

  console.log('--- Test 9: isResultValid (after finalize) ---');
  const isValidAfter = await verifier.isResultValid(resultId);
  if (isValidAfter) {
    ok('isResultValid() = true (finalized)');
  } else {
    fail('isResultValid() expected true after finalize');
  }

  // ── Test 10: 2-step ownership transfer ─────────────────────────────────

  console.log('--- Test 10: 2-step ownership transfer ---');
  tx = await verifier.transferOwnership(NEW_OWNER_ADDR);
  ok(`transferOwnership() tx = ${tx.slice(0, 18)}...`);

  const pending = await verifier.pendingOwner();
  if (pending.toLowerCase() === NEW_OWNER_ADDR.toLowerCase()) {
    ok(`pendingOwner() = ${pending}`);
  } else {
    fail(`pendingOwner() expected ${NEW_OWNER_ADDR}, got ${pending}`);
  }

  // Accept from new owner
  const newOwnerVerifier = new TEEVerifier({
    rpcUrl,
    privateKey: NEW_OWNER_KEY,
    contractAddress,
  });
  tx = await newOwnerVerifier.acceptOwnership();
  ok(`acceptOwnership() tx = ${tx.slice(0, 18)}...`);

  const newOwner = await newOwnerVerifier.owner();
  if (newOwner.toLowerCase() === NEW_OWNER_ADDR.toLowerCase()) {
    ok(`owner() = ${newOwner} (after transfer)`);
  } else {
    fail(`owner() expected ${NEW_OWNER_ADDR}, got ${newOwner}`);
  }

  // Transfer back
  tx = await newOwnerVerifier.transferOwnership(adminAddr);
  ok(`transferOwnership() back tx = ${tx.slice(0, 18)}...`);
  tx = await verifier.acceptOwnership();
  ok(`acceptOwnership() by admin tx = ${tx.slice(0, 18)}...`);

  const restoredOwner = await verifier.owner();
  if (restoredOwner.toLowerCase() === adminAddr.toLowerCase()) {
    ok(`owner() = ${restoredOwner} (restored)`);
  } else {
    fail(`owner() expected ${adminAddr}, got ${restoredOwner}`);
  }

  // ── Test 11: revokeEnclave ─────────────────────────────────────────────

  console.log('--- Test 11: revokeEnclave ---');
  tx = await verifier.revokeEnclave(ENCLAVE_ADDR);
  ok(`revokeEnclave() tx = ${tx.slice(0, 18)}...`);

  // ── Summary ────────────────────────────────────────────────────────────

  console.log();
  if (failed > 0) {
    console.log(`RESULT: ${passed} passed, ${failed} failed`);
    process.exit(1);
  } else {
    console.log(`RESULT: ${passed} tests passed`);
  }
}

main().catch((err) => {
  console.error('E2E test failed:', (err as Error).message);
  process.exit(1);
});
