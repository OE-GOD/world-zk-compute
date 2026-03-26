/**
 * Anvil Integration Tests for TEEMLVerifier
 *
 * These tests spin up a local Anvil instance, deploy the TEEMLVerifier contract,
 * and exercise the full lifecycle: deploy, register enclave, submit result,
 * challenge, finalize, and event polling.
 *
 * The entire suite is skipped if `anvil` is not available in PATH.
 */
import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { execSync, spawn, type ChildProcess } from 'child_process';
import { readFileSync } from 'fs';
import { join } from 'path';
import {
  createPublicClient,
  createWalletClient,
  http,
  parseEther,
  encodePacked,
  keccak256,
  getAddress,
  type Hex,
  defineChain,
} from 'viem';
import { privateKeyToAccount } from 'viem/accounts';
import { TEEVerifier } from '../tee-verifier';
import { TEEEventWatcher } from '../tee-event-watcher';

// ---------------------------------------------------------------------------
// Check if anvil is available
// ---------------------------------------------------------------------------
let anvilAvailable = false;
try {
  execSync('which anvil', { stdio: 'ignore' });
  anvilAvailable = true;
} catch {
  anvilAvailable = false;
}

// ---------------------------------------------------------------------------
// Load contract artifact from forge output
// ---------------------------------------------------------------------------
const CONTRACTS_ROOT = join(__dirname, '..', '..', '..', '..', 'contracts');
let contractBytecode: Hex = '0x';
let contractAbi: readonly any[] = [];

try {
  const artifact = JSON.parse(
    readFileSync(join(CONTRACTS_ROOT, 'out', 'TEEMLVerifier.sol', 'TEEMLVerifier.json'), 'utf-8'),
  );
  contractBytecode = artifact.bytecode.object as Hex;
  contractAbi = artifact.abi;
} catch {
  // Will be caught in beforeAll when we check the bytecode
}

// ---------------------------------------------------------------------------
// Test accounts (Anvil default mnemonic deterministic keys)
// ---------------------------------------------------------------------------
const ADMIN_PRIVATE_KEY: Hex = '0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80';
const PROVER_PRIVATE_KEY: Hex = '0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d';
const CHALLENGER_PRIVATE_KEY: Hex = '0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a';
const ENCLAVE_PRIVATE_KEY: Hex = '0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6';

const adminAccount = privateKeyToAccount(ADMIN_PRIVATE_KEY);
const proverAccount = privateKeyToAccount(PROVER_PRIVATE_KEY);
const challengerAccount = privateKeyToAccount(CHALLENGER_PRIVATE_KEY);
const enclaveAccount = privateKeyToAccount(ENCLAVE_PRIVATE_KEY);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Find a free port by asking the OS. */
function getRandomPort(): number {
  // Use a port in the ephemeral range, add randomness to avoid collisions
  return 18545 + Math.floor(Math.random() * 10000);
}

/** Wait for Anvil's RPC to become responsive. */
async function waitForAnvil(rpcUrl: string, timeoutMs = 15000): Promise<void> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    try {
      const res = await fetch(rpcUrl, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ jsonrpc: '2.0', method: 'eth_blockNumber', params: [], id: 1 }),
      });
      if (res.ok) return;
    } catch {
      // not ready yet
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`Anvil did not start within ${timeoutMs}ms`);
}

/** Build an ECDSA attestation the way the contract expects it. */
async function buildAttestation(
  modelHash: Hex,
  inputHash: Hex,
  resultBytes: Hex,
  signerPrivateKey: Hex,
): Promise<Hex> {
  const resultHash = keccak256(resultBytes);
  const message = keccak256(encodePacked(['bytes32', 'bytes32', 'bytes32'], [modelHash, inputHash, resultHash]));
  // Sign with viem's signMessage which adds the EIP-191 prefix automatically
  const account = privateKeyToAccount(signerPrivateKey);
  const signature = await account.signMessage({ message: { raw: message } });
  return signature;
}

// ---------------------------------------------------------------------------
// Test suite
// ---------------------------------------------------------------------------

const describeIfAnvil = anvilAvailable ? describe : describe.skip;

describeIfAnvil('TEEMLVerifier Anvil Integration Tests', () => {
  let anvilProcess: ChildProcess;
  let rpcUrl: string;
  let port: number;
  let contractAddress: Hex;

  // Shared chain definition
  let chain: ReturnType<typeof defineChain>;

  beforeAll(async () => {
    // Verify we have the compiled artifact
    if (contractBytecode === '0x' || contractAbi.length === 0) {
      throw new Error(
        'Contract artifact not found. Run `cd contracts && forge build` first.',
      );
    }

    // Start Anvil
    port = getRandomPort();
    rpcUrl = `http://127.0.0.1:${port}`;

    anvilProcess = spawn('anvil', ['--port', String(port), '--silent'], {
      stdio: 'ignore',
      detached: false,
    });

    // In case anvil fails to start
    anvilProcess.on('error', (err) => {
      throw new Error(`Failed to start Anvil: ${err.message}`);
    });

    await waitForAnvil(rpcUrl);

    chain = defineChain({
      id: 31337,
      name: 'Anvil',
      nativeCurrency: { name: 'Ether', symbol: 'ETH', decimals: 18 },
      rpcUrls: { default: { http: [rpcUrl] } },
    });

    // Deploy TEEMLVerifier
    const walletClient = createWalletClient({
      chain,
      transport: http(rpcUrl),
      account: adminAccount,
    });
    const publicClient = createPublicClient({
      chain,
      transport: http(rpcUrl),
    });

    // Constructor: (address _admin, address _remainderVerifier)
    // Use a dummy address for remainderVerifier since we won't test actual ZK dispute resolution
    const dummyRemainderVerifier: Hex = '0x0000000000000000000000000000000000000001';

    const deployHash = await walletClient.deployContract({
      chain,
      abi: contractAbi,
      bytecode: contractBytecode,
      args: [adminAccount.address, dummyRemainderVerifier],
    });

    const receipt = await publicClient.waitForTransactionReceipt({ hash: deployHash });
    if (!receipt.contractAddress) {
      throw new Error('Contract deployment failed: no contractAddress in receipt');
    }
    contractAddress = receipt.contractAddress;
  }, 30000); // 30s timeout for beforeAll

  afterAll(async () => {
    if (anvilProcess) {
      anvilProcess.kill('SIGTERM');
      // Wait briefly for the process to exit
      await new Promise((r) => setTimeout(r, 500));
      if (!anvilProcess.killed) {
        anvilProcess.kill('SIGKILL');
      }
    }
  });

  // ------------------------------------------------------------------
  // Test: Deploy contract and verify admin
  // ------------------------------------------------------------------
  it('should deploy the contract and set the correct admin', async () => {
    const publicClient = createPublicClient({ chain, transport: http(rpcUrl) });

    const adminAddr = await publicClient.readContract({
      address: contractAddress,
      abi: contractAbi,
      functionName: 'admin',
    });

    expect(getAddress(adminAddr as string)).toBe(getAddress(adminAccount.address));
  });

  it('should report the correct remainderVerifier', async () => {
    const publicClient = createPublicClient({ chain, transport: http(rpcUrl) });

    const rv = await publicClient.readContract({
      address: contractAddress,
      abi: contractAbi,
      functionName: 'remainderVerifier',
    });

    expect(getAddress(rv as string)).toBe(getAddress('0x0000000000000000000000000000000000000001'));
  });

  it('should have correct default proverStake and challengeBondAmount', async () => {
    const publicClient = createPublicClient({ chain, transport: http(rpcUrl) });

    const stake = await publicClient.readContract({
      address: contractAddress,
      abi: contractAbi,
      functionName: 'proverStake',
    });
    const bond = await publicClient.readContract({
      address: contractAddress,
      abi: contractAbi,
      functionName: 'challengeBondAmount',
    });

    expect(stake).toBe(parseEther('0.1'));
    expect(bond).toBe(parseEther('0.1'));
  });

  // ------------------------------------------------------------------
  // Test: Register enclave via TEEVerifier SDK class
  // ------------------------------------------------------------------
  it('should register an enclave using the TEEVerifier SDK', async () => {
    const verifier = new TEEVerifier({
      rpcUrl,
      privateKey: ADMIN_PRIVATE_KEY,
      contractAddress,
    });

    const imageHash: Hex = '0x' + 'aa'.repeat(32) as Hex;
    const txHash = await verifier.registerEnclave(enclaveAccount.address, imageHash);
    expect(txHash).toMatch(/^0x[0-9a-f]{64}$/);

    // Verify via admin() that the contract is accessible
    const adminAddr = await verifier.admin();
    expect(getAddress(adminAddr)).toBe(getAddress(adminAccount.address));
  });

  // ------------------------------------------------------------------
  // Test: Submit result with stake
  // ------------------------------------------------------------------
  it('should submit a result with valid attestation and stake', async () => {
    // First register the enclave (using a fresh image hash to avoid "already registered")
    const adminWallet = createWalletClient({
      chain,
      transport: http(rpcUrl),
      account: adminAccount,
    });
    const publicClient = createPublicClient({ chain, transport: http(rpcUrl) });

    // Check if enclave is already registered (from prior test)
    const enclaveInfo = await publicClient.readContract({
      address: contractAddress,
      abi: contractAbi,
      functionName: 'enclaves',
      args: [enclaveAccount.address],
    }) as any;

    // If not yet registered, register
    if (!enclaveInfo || !enclaveInfo[0]) {
      const regHash = await adminWallet.writeContract({
        chain,
        address: contractAddress,
        abi: contractAbi,
        functionName: 'registerEnclave',
        args: [enclaveAccount.address, '0x' + 'aa'.repeat(32)],
      });
      await publicClient.waitForTransactionReceipt({ hash: regHash });
    }

    // Build the result submission
    const modelHash: Hex = '0x' + '11'.repeat(32) as Hex;
    const inputHash: Hex = '0x' + '22'.repeat(32) as Hex;
    const resultBytes: Hex = '0xdeadbeef';

    // Build attestation signed by enclave key
    const attestation = await buildAttestation(modelHash, inputHash, resultBytes, ENCLAVE_PRIVATE_KEY);

    // Submit via prover wallet
    const proverWallet = createWalletClient({
      chain,
      transport: http(rpcUrl),
      account: proverAccount,
    });

    const submitHash = await proverWallet.writeContract({
      chain,
      address: contractAddress,
      abi: contractAbi,
      functionName: 'submitResult',
      args: [modelHash, inputHash, resultBytes, attestation],
      value: parseEther('0.1'),
    });

    const submitReceipt = await publicClient.waitForTransactionReceipt({ hash: submitHash });
    expect(submitReceipt.status).toBe('success');

    // Compute the expected resultId
    const blockNumber = submitReceipt.blockNumber;
    const resultId = keccak256(
      encodePacked(
        ['address', 'bytes32', 'bytes32', 'uint256'],
        [proverAccount.address, modelHash, inputHash, blockNumber],
      ),
    );

    // Verify the result was stored
    const result = await publicClient.readContract({
      address: contractAddress,
      abi: contractAbi,
      functionName: 'getResult',
      args: [resultId],
    }) as any;

    expect(getAddress(result.submitter)).toBe(getAddress(proverAccount.address));
    expect(result.modelHash).toBe(modelHash);
    expect(result.inputHash).toBe(inputHash);
    expect(result.finalized).toBe(false);
    expect(result.challenged).toBe(false);
    expect(result.proverStakeAmount).toBe(parseEther('0.1'));
  });

  // ------------------------------------------------------------------
  // Test: Finalize after time warp
  // ------------------------------------------------------------------
  it('should finalize a result after the challenge window expires', async () => {
    const publicClient = createPublicClient({ chain, transport: http(rpcUrl) });
    const proverWallet = createWalletClient({
      chain,
      transport: http(rpcUrl),
      account: proverAccount,
    });

    // Submit a fresh result
    const modelHash: Hex = '0x' + '33'.repeat(32) as Hex;
    const inputHash: Hex = '0x' + '44'.repeat(32) as Hex;
    const resultBytes: Hex = '0xcafe';
    const attestation = await buildAttestation(modelHash, inputHash, resultBytes, ENCLAVE_PRIVATE_KEY);

    const submitHash = await proverWallet.writeContract({
      chain,
      address: contractAddress,
      abi: contractAbi,
      functionName: 'submitResult',
      args: [modelHash, inputHash, resultBytes, attestation],
      value: parseEther('0.1'),
    });
    const submitReceipt = await publicClient.waitForTransactionReceipt({ hash: submitHash });
    expect(submitReceipt.status).toBe('success');

    // Compute resultId
    const resultId = keccak256(
      encodePacked(
        ['address', 'bytes32', 'bytes32', 'uint256'],
        [proverAccount.address, modelHash, inputHash, submitReceipt.blockNumber],
      ),
    );

    // Time warp past CHALLENGE_WINDOW (1 hour = 3600 seconds)
    await fetch(rpcUrl, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        jsonrpc: '2.0',
        method: 'evm_increaseTime',
        params: [3601],
        id: 1,
      }),
    });
    // Mine a block to apply the time change
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

    // Get prover balance before finalize
    const balanceBefore = await publicClient.getBalance({ address: proverAccount.address });

    // Finalize using the TEEVerifier SDK
    const verifier = new TEEVerifier({
      rpcUrl,
      privateKey: PROVER_PRIVATE_KEY,
      contractAddress,
    });
    const finalizeTx = await verifier.finalize(resultId);
    expect(finalizeTx).toMatch(/^0x[0-9a-f]{64}$/);

    // Verify finalized state
    const result = await publicClient.readContract({
      address: contractAddress,
      abi: contractAbi,
      functionName: 'getResult',
      args: [resultId],
    }) as any;
    expect(result.finalized).toBe(true);

    // Verify isResultValid returns true
    const isValid = await publicClient.readContract({
      address: contractAddress,
      abi: contractAbi,
      functionName: 'isResultValid',
      args: [resultId],
    });
    expect(isValid).toBe(true);

    // Verify prover got stake back (balance should have increased, minus gas)
    const balanceAfter = await publicClient.getBalance({ address: proverAccount.address });
    // The difference should be close to 0.1 ETH (stake returned) minus gas cost
    // At minimum, balance should have increased because they got 0.1 ETH back
    // (gas cost is negligible compared to 0.1 ETH on Anvil)
    const diff = balanceAfter - balanceBefore;
    // diff should be positive (got stake back minus gas)
    expect(diff).toBeGreaterThan(parseEther('0.09'));
  });

  // ------------------------------------------------------------------
  // Test: Challenge flow
  // ------------------------------------------------------------------
  it('should allow challenging a submitted result within the window', async () => {
    const publicClient = createPublicClient({ chain, transport: http(rpcUrl) });
    const proverWallet = createWalletClient({
      chain,
      transport: http(rpcUrl),
      account: proverAccount,
    });

    // Submit a fresh result
    const modelHash: Hex = '0x' + '55'.repeat(32) as Hex;
    const inputHash: Hex = '0x' + '66'.repeat(32) as Hex;
    const resultBytes: Hex = '0xbabe';
    const attestation = await buildAttestation(modelHash, inputHash, resultBytes, ENCLAVE_PRIVATE_KEY);

    const submitHash = await proverWallet.writeContract({
      chain,
      address: contractAddress,
      abi: contractAbi,
      functionName: 'submitResult',
      args: [modelHash, inputHash, resultBytes, attestation],
      value: parseEther('0.1'),
    });
    const submitReceipt = await publicClient.waitForTransactionReceipt({ hash: submitHash });

    const resultId = keccak256(
      encodePacked(
        ['address', 'bytes32', 'bytes32', 'uint256'],
        [proverAccount.address, modelHash, inputHash, submitReceipt.blockNumber],
      ),
    );

    // Challenge via challenger wallet
    const challengerWallet = createWalletClient({
      chain,
      transport: http(rpcUrl),
      account: challengerAccount,
    });

    const challengeHash = await challengerWallet.writeContract({
      chain,
      address: contractAddress,
      abi: contractAbi,
      functionName: 'challenge',
      args: [resultId],
      value: parseEther('0.1'),
    });
    const challengeReceipt = await publicClient.waitForTransactionReceipt({ hash: challengeHash });
    expect(challengeReceipt.status).toBe('success');

    // Verify challenged state
    const result = await publicClient.readContract({
      address: contractAddress,
      abi: contractAbi,
      functionName: 'getResult',
      args: [resultId],
    }) as any;
    expect(result.challenged).toBe(true);
    expect(getAddress(result.challenger)).toBe(getAddress(challengerAccount.address));
    expect(result.challengeBond).toBe(parseEther('0.1'));
    expect(result.disputeDeadline).toBeGreaterThan(0n);
  });

  // ------------------------------------------------------------------
  // Test: Challenge with TEEVerifier SDK
  // ------------------------------------------------------------------
  it('should challenge using the TEEVerifier SDK class', async () => {
    const publicClient = createPublicClient({ chain, transport: http(rpcUrl) });
    const proverWallet = createWalletClient({
      chain,
      transport: http(rpcUrl),
      account: proverAccount,
    });

    // Submit a result
    const modelHash: Hex = '0x' + '77'.repeat(32) as Hex;
    const inputHash: Hex = '0x' + '88'.repeat(32) as Hex;
    const resultBytes: Hex = '0xface';
    const attestation = await buildAttestation(modelHash, inputHash, resultBytes, ENCLAVE_PRIVATE_KEY);

    const submitHash = await proverWallet.writeContract({
      chain,
      address: contractAddress,
      abi: contractAbi,
      functionName: 'submitResult',
      args: [modelHash, inputHash, resultBytes, attestation],
      value: parseEther('0.1'),
    });
    const submitReceipt = await publicClient.waitForTransactionReceipt({ hash: submitHash });

    const resultId = keccak256(
      encodePacked(
        ['address', 'bytes32', 'bytes32', 'uint256'],
        [proverAccount.address, modelHash, inputHash, submitReceipt.blockNumber],
      ),
    );

    // Challenge using the SDK
    const challengerVerifier = new TEEVerifier({
      rpcUrl,
      privateKey: CHALLENGER_PRIVATE_KEY,
      contractAddress,
    });

    const challengeTx = await challengerVerifier.challenge(resultId);
    expect(challengeTx).toMatch(/^0x[0-9a-f]{64}$/);

    // Verify state
    const result = await publicClient.readContract({
      address: contractAddress,
      abi: contractAbi,
      functionName: 'getResult',
      args: [resultId],
    }) as any;
    expect(result.challenged).toBe(true);
  });

  // ------------------------------------------------------------------
  // Test: Resolve dispute by timeout (challenger wins)
  // ------------------------------------------------------------------
  it('should resolve dispute by timeout when prover fails to provide ZK proof', async () => {
    const publicClient = createPublicClient({ chain, transport: http(rpcUrl) });
    const proverWallet = createWalletClient({
      chain,
      transport: http(rpcUrl),
      account: proverAccount,
    });

    // Submit
    const modelHash: Hex = '0x' + '99'.repeat(32) as Hex;
    const inputHash: Hex = '0x' + 'aa'.repeat(32) as Hex;
    const resultBytes: Hex = '0xdead';
    const attestation = await buildAttestation(modelHash, inputHash, resultBytes, ENCLAVE_PRIVATE_KEY);

    const submitHash = await proverWallet.writeContract({
      chain,
      address: contractAddress,
      abi: contractAbi,
      functionName: 'submitResult',
      args: [modelHash, inputHash, resultBytes, attestation],
      value: parseEther('0.1'),
    });
    const submitReceipt = await publicClient.waitForTransactionReceipt({ hash: submitHash });

    const resultId = keccak256(
      encodePacked(
        ['address', 'bytes32', 'bytes32', 'uint256'],
        [proverAccount.address, modelHash, inputHash, submitReceipt.blockNumber],
      ),
    );

    // Challenge
    const challengerWallet = createWalletClient({
      chain,
      transport: http(rpcUrl),
      account: challengerAccount,
    });

    const challengeHash = await challengerWallet.writeContract({
      chain,
      address: contractAddress,
      abi: contractAbi,
      functionName: 'challenge',
      args: [resultId],
      value: parseEther('0.1'),
    });
    await publicClient.waitForTransactionReceipt({ hash: challengeHash });

    // Record challenger balance before resolution
    const challengerBalanceBefore = await publicClient.getBalance({ address: challengerAccount.address });

    // Time warp past DISPUTE_WINDOW (24 hours = 86400 seconds)
    await fetch(rpcUrl, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        jsonrpc: '2.0',
        method: 'evm_increaseTime',
        params: [86401],
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

    // Resolve dispute by timeout (anyone can call this)
    const resolveHash = await challengerWallet.writeContract({
      chain,
      address: contractAddress,
      abi: contractAbi,
      functionName: 'resolveDisputeByTimeout',
      args: [resultId],
    });
    const resolveReceipt = await publicClient.waitForTransactionReceipt({ hash: resolveHash });
    expect(resolveReceipt.status).toBe('success');

    // Verify: dispute resolved, prover lost
    const disputeResolved = await publicClient.readContract({
      address: contractAddress,
      abi: contractAbi,
      functionName: 'disputeResolved',
      args: [resultId],
    });
    expect(disputeResolved).toBe(true);

    const proverWon = await publicClient.readContract({
      address: contractAddress,
      abi: contractAbi,
      functionName: 'disputeProverWon',
      args: [resultId],
    });
    expect(proverWon).toBe(false);

    // isResultValid should be false
    const isValid = await publicClient.readContract({
      address: contractAddress,
      abi: contractAbi,
      functionName: 'isResultValid',
      args: [resultId],
    });
    expect(isValid).toBe(false);

    // Challenger should have received the pot (prover stake + challenge bond = 0.2 ETH)
    const challengerBalanceAfter = await publicClient.getBalance({ address: challengerAccount.address });
    const diff = challengerBalanceAfter - challengerBalanceBefore;
    // They get 0.2 ETH (pot) minus gas used for the resolveDisputeByTimeout call
    expect(diff).toBeGreaterThan(parseEther('0.19'));
  });

  // ------------------------------------------------------------------
  // Test: Poll events using the full contract ABI (forge output)
  // ------------------------------------------------------------------
  it('should find ResultSubmitted events using the full contract ABI', async () => {
    // NOTE: The SDK's teeMLVerifierAbi has a mismatched ResultSubmitted event
    // signature (3 params vs 4 in the actual contract). This test uses the
    // full contract ABI from forge output to verify events are emitted correctly.
    const publicClient = createPublicClient({ chain, transport: http(rpcUrl) });

    const logs = await publicClient.getContractEvents({
      address: contractAddress,
      abi: contractAbi,
      eventName: 'ResultSubmitted',
      fromBlock: 0n,
      toBlock: 'latest',
    });

    expect(logs.length).toBeGreaterThanOrEqual(1);

    const firstLog = logs[0] as any;
    expect(firstLog.args.resultId).toMatch(/^0x[0-9a-f]{64}$/);
    expect(firstLog.args.modelHash).toMatch(/^0x[0-9a-f]{64}$/);
    expect(firstLog.args.inputHash).toMatch(/^0x[0-9a-f]{64}$/);
    // The full ABI also includes the submitter field
    expect(firstLog.args.submitter).toMatch(/^0x[0-9a-fA-F]{40}$/);
  });

  it('should find ResultSubmitted events via TEEEventWatcher', async () => {
    // After ABI codegen, the SDK's teeMLVerifierAbi now matches the contract's
    // event signatures, so the TEEEventWatcher should find ResultSubmitted events.
    const watcher = new TEEEventWatcher({
      rpcUrl,
      contractAddress,
    });

    const events = await watcher.getPastEvents('ResultSubmitted', 0n, 'latest');
    // Previous tests submitted multiple results, so we should find them
    expect(events.length).toBeGreaterThan(0);
  });

  it('should poll past events and find EnclaveRegistered', async () => {
    const watcher = new TEEEventWatcher({
      rpcUrl,
      contractAddress,
    });

    const events = await watcher.getPastEvents('EnclaveRegistered', 0n, 'latest');
    expect(events.length).toBeGreaterThanOrEqual(1);

    const first = events[0];
    expect(first.event).toBe('EnclaveRegistered');
    if (first.event === 'EnclaveRegistered') {
      expect(getAddress(first.data.enclaveKey)).toBe(getAddress(enclaveAccount.address));
    }
  });

  it('should poll past events and find ResultChallenged', async () => {
    const watcher = new TEEEventWatcher({
      rpcUrl,
      contractAddress,
    });

    const events = await watcher.getPastEvents('ResultChallenged', 0n, 'latest');
    expect(events.length).toBeGreaterThanOrEqual(1);

    const first = events[0];
    expect(first.event).toBe('ResultChallenged');
    if (first.event === 'ResultChallenged') {
      expect(first.data.resultId).toMatch(/^0x[0-9a-f]{64}$/);
      expect(first.data.challenger).toMatch(/^0x[0-9a-fA-F]{40}$/);
    }
  });

  it('should poll past events and find DisputeResolved', async () => {
    const watcher = new TEEEventWatcher({
      rpcUrl,
      contractAddress,
    });

    const events = await watcher.getPastEvents('DisputeResolved', 0n, 'latest');
    expect(events.length).toBeGreaterThanOrEqual(1);

    const first = events[0];
    expect(first.event).toBe('DisputeResolved');
    if (first.event === 'DisputeResolved') {
      expect(first.data.resultId).toMatch(/^0x[0-9a-f]{64}$/);
      expect(typeof first.data.proverWon).toBe('boolean');
    }
  });

  it('should poll all events without a filter', async () => {
    const watcher = new TEEEventWatcher({
      rpcUrl,
      contractAddress,
    });

    const events = await watcher.getPastEvents(undefined, 0n, 'latest');
    // We have at least: EnclaveRegistered + ResultChallenged + DisputeResolved + ResultFinalized
    // Note: ResultSubmitted events are NOT matched due to SDK ABI mismatch (see above test)
    expect(events.length).toBeGreaterThanOrEqual(3);

    // Check that we see a variety of event types
    const eventTypes = new Set(events.map((e) => e.event));
    expect(eventTypes.has('EnclaveRegistered')).toBe(true);
    // ResultChallenged and DisputeResolved are correctly defined in SDK ABI
    expect(eventTypes.has('ResultChallenged')).toBe(true);
    expect(eventTypes.has('DisputeResolved')).toBe(true);
  });

  // ------------------------------------------------------------------
  // Test: Pause and unpause
  // ------------------------------------------------------------------
  it('should pause and unpause the contract', async () => {
    const verifier = new TEEVerifier({
      rpcUrl,
      privateKey: ADMIN_PRIVATE_KEY,
      contractAddress,
    });

    // Pause
    await verifier.pause();
    const paused = await verifier.paused();
    expect(paused).toBe(true);

    // Unpause
    await verifier.unpause();
    const pausedAfter = await verifier.paused();
    expect(pausedAfter).toBe(false);
  });

  // ------------------------------------------------------------------
  // Test: Submission rejects unregistered enclave
  // ------------------------------------------------------------------
  it('should reject submission with unregistered enclave attestation', async () => {
    const proverWallet = createWalletClient({
      chain,
      transport: http(rpcUrl),
      account: proverAccount,
    });

    const modelHash: Hex = '0x' + 'bb'.repeat(32) as Hex;
    const inputHash: Hex = '0x' + 'cc'.repeat(32) as Hex;
    const resultBytes: Hex = '0x1234';

    // Sign with a random key that is NOT registered as an enclave
    const fakeEnclaveKey: Hex = '0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba';
    const attestation = await buildAttestation(modelHash, inputHash, resultBytes, fakeEnclaveKey);

    await expect(
      proverWallet.writeContract({
        chain,
        address: contractAddress,
        abi: contractAbi,
        functionName: 'submitResult',
        args: [modelHash, inputHash, resultBytes, attestation],
        value: parseEther('0.1'),
      }),
    ).rejects.toThrow();
  });

  // ------------------------------------------------------------------
  // Test: Submission rejects insufficient stake
  // ------------------------------------------------------------------
  it('should reject submission with insufficient prover stake', async () => {
    const proverWallet = createWalletClient({
      chain,
      transport: http(rpcUrl),
      account: proverAccount,
    });

    const modelHash: Hex = '0x' + 'dd'.repeat(32) as Hex;
    const inputHash: Hex = '0x' + 'ee'.repeat(32) as Hex;
    const resultBytes: Hex = '0xabcd';
    const attestation = await buildAttestation(modelHash, inputHash, resultBytes, ENCLAVE_PRIVATE_KEY);

    await expect(
      proverWallet.writeContract({
        chain,
        address: contractAddress,
        abi: contractAbi,
        functionName: 'submitResult',
        args: [modelHash, inputHash, resultBytes, attestation],
        value: parseEther('0.01'), // Below the 0.1 ETH minimum
      }),
    ).rejects.toThrow();
  });

  // ------------------------------------------------------------------
  // Test: Challenge rejects insufficient bond
  // ------------------------------------------------------------------
  it('should reject challenge with insufficient bond', async () => {
    const publicClient = createPublicClient({ chain, transport: http(rpcUrl) });
    const proverWallet = createWalletClient({
      chain,
      transport: http(rpcUrl),
      account: proverAccount,
    });

    // Submit a valid result first
    const modelHash: Hex = '0x' + 'f0'.repeat(32) as Hex;
    const inputHash: Hex = '0x' + 'f1'.repeat(32) as Hex;
    const resultBytes: Hex = '0xffff';
    const attestation = await buildAttestation(modelHash, inputHash, resultBytes, ENCLAVE_PRIVATE_KEY);

    const submitHash = await proverWallet.writeContract({
      chain,
      address: contractAddress,
      abi: contractAbi,
      functionName: 'submitResult',
      args: [modelHash, inputHash, resultBytes, attestation],
      value: parseEther('0.1'),
    });
    const submitReceipt = await publicClient.waitForTransactionReceipt({ hash: submitHash });

    const resultId = keccak256(
      encodePacked(
        ['address', 'bytes32', 'bytes32', 'uint256'],
        [proverAccount.address, modelHash, inputHash, submitReceipt.blockNumber],
      ),
    );

    // Try to challenge with insufficient bond
    const challengerWallet = createWalletClient({
      chain,
      transport: http(rpcUrl),
      account: challengerAccount,
    });

    await expect(
      challengerWallet.writeContract({
        chain,
        address: contractAddress,
        abi: contractAbi,
        functionName: 'challenge',
        args: [resultId],
        value: parseEther('0.01'), // Below the 0.1 ETH minimum
      }),
    ).rejects.toThrow();
  });

  // ------------------------------------------------------------------
  // Test: Non-admin cannot register enclave
  // ------------------------------------------------------------------
  it('should reject enclave registration from non-admin', async () => {
    const nonOwnerVerifier = new TEEVerifier({
      rpcUrl,
      privateKey: PROVER_PRIVATE_KEY,
      contractAddress,
    });

    const imageHash: Hex = '0x' + 'ff'.repeat(32) as Hex;
    await expect(
      nonOwnerVerifier.registerEnclave(
        '0x1111111111111111111111111111111111111111',
        imageHash,
      ),
    ).rejects.toThrow();
  });

  // ------------------------------------------------------------------
  // Test: getResult returns empty struct for unknown resultId
  // ------------------------------------------------------------------
  it('should return an empty result for unknown resultId', async () => {
    const verifier = new TEEVerifier({
      rpcUrl,
      privateKey: ADMIN_PRIVATE_KEY,
      contractAddress,
    });

    const unknownId: Hex = '0x' + '00'.repeat(32) as Hex;
    const result = await verifier.getResult(unknownId) as any;

    // submittedAt should be 0 for non-existent results
    expect(result.submittedAt).toBe(0n);
    expect(result.finalized).toBe(false);
    expect(result.challenged).toBe(false);
  });

  // ------------------------------------------------------------------
  // Test: isResultValid returns false for unknown resultId
  // ------------------------------------------------------------------
  it('should return false for isResultValid on unknown resultId', async () => {
    const verifier = new TEEVerifier({
      rpcUrl,
      privateKey: ADMIN_PRIVATE_KEY,
      contractAddress,
    });

    const unknownId: Hex = '0x' + '00'.repeat(32) as Hex;
    const isValid = await verifier.isResultValid(unknownId);
    expect(isValid).toBe(false);
  });
});
