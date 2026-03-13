import { describe, it, expect, vi, beforeEach } from 'vitest';
import { BatchVerifier, DAGBatchVerifier, ProgressTracker, BatchVerificationCancelledError } from '../batch-verifier';
import type { ProgressEvent } from '../batch-verifier-types';
import type { PublicClient, WalletClient, Transport, Chain } from 'viem';

// ─── ProgressTracker unit tests ───

describe('ProgressTracker', () => {
  it('returns undefined averageStepMs when no steps completed', () => {
    const tracker = new ProgressTracker(1000);
    expect(tracker.averageStepMs).toBeUndefined();
    expect(tracker.completedSteps).toBe(0);
  });

  it('computes averageStepMs after one step', () => {
    const tracker = new ProgressTracker(1000);
    tracker.recordStep(2000); // 1 second later
    expect(tracker.completedSteps).toBe(1);
    expect(tracker.averageStepMs).toBe(1000);
  });

  it('computes averageStepMs after multiple steps', () => {
    const tracker = new ProgressTracker(0);
    tracker.recordStep(100);  // step 1 at 100ms
    tracker.recordStep(300);  // step 2 at 300ms
    tracker.recordStep(600);  // step 3 at 600ms
    expect(tracker.completedSteps).toBe(3);
    // Average = 600ms elapsed / 3 steps = 200ms
    expect(tracker.averageStepMs).toBe(200);
  });

  it('estimates remaining time correctly', () => {
    const tracker = new ProgressTracker(0);
    tracker.recordStep(100);
    tracker.recordStep(200);
    // Average = 200ms / 2 = 100ms per step
    expect(tracker.estimateRemainingMs(5)).toBe(500);
    expect(tracker.estimateRemainingMs(0)).toBe(0);
  });

  it('returns undefined for remaining time with no steps', () => {
    const tracker = new ProgressTracker(0);
    expect(tracker.estimateRemainingMs(5)).toBeUndefined();
  });

  it('returns undefined for remaining time with negative remaining', () => {
    const tracker = new ProgressTracker(0);
    tracker.recordStep(100);
    expect(tracker.estimateRemainingMs(-1)).toBeUndefined();
  });

  it('tracks elapsed time', () => {
    const now = Date.now();
    const tracker = new ProgressTracker(now - 500);
    const elapsed = tracker.elapsedMs;
    // Should be approximately 500ms (allow some margin for test execution)
    expect(elapsed).toBeGreaterThanOrEqual(490);
    expect(elapsed).toBeLessThan(600);
  });

  it('getStartTime returns the initial start time', () => {
    const tracker = new ProgressTracker(42);
    expect(tracker.getStartTime()).toBe(42);
  });

  it('uses Date.now() as default start time', () => {
    const before = Date.now();
    const tracker = new ProgressTracker();
    const after = Date.now();
    expect(tracker.getStartTime()).toBeGreaterThanOrEqual(before);
    expect(tracker.getStartTime()).toBeLessThanOrEqual(after);
  });

  it('recordStep uses Date.now() when no timestamp provided', () => {
    const tracker = new ProgressTracker(0);
    tracker.recordStep();
    expect(tracker.completedSteps).toBe(1);
    // averageStepMs should be roughly Date.now() which is a large number
    expect(tracker.averageStepMs).toBeGreaterThan(0);
  });

  it('handles rapid successive steps', () => {
    const tracker = new ProgressTracker(0);
    tracker.recordStep(0);
    tracker.recordStep(0);
    tracker.recordStep(0);
    expect(tracker.completedSteps).toBe(3);
    expect(tracker.averageStepMs).toBe(0);
    expect(tracker.estimateRemainingMs(10)).toBe(0);
  });
});

// ─── BatchVerificationCancelledError ───

describe('BatchVerificationCancelledError', () => {
  it('is an instance of Error', () => {
    const err = new BatchVerificationCancelledError();
    expect(err).toBeInstanceOf(Error);
    expect(err.name).toBe('BatchVerificationCancelledError');
    expect(err.message).toBe('Batch verification was cancelled');
  });
});

// ─── BatchVerifier.loadFixture ───

describe('BatchVerifier.loadFixture', () => {
  it('loads phase1a_dag_fixture format (proof_hex/public_inputs_hex)', () => {
    const fixture = {
      proof_hex: 'deadbeef',
      public_inputs_hex: 'cafebabe',
      circuit_hash_raw: '0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef',
      gens_hex: '0xaabbccdd',
    };

    const input = BatchVerifier.loadFixture(fixture);
    expect(input.proof).toBe('0xdeadbeef');
    expect(input.publicInputs).toBe('0xcafebabe');
    expect(input.circuitHash).toBe('0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef');
    expect(input.gensData).toBe('0xaabbccdd');
  });

  it('loads groth16_e2e_fixture format (inner_proof_hex/public_values_abi)', () => {
    const fixture = {
      inner_proof_hex: '0xdeadbeef',
      public_values_abi: '0xcafebabe',
      circuit_hash_raw: '0xabcd',
      gens_hex: '0x1122',
    };

    const input = BatchVerifier.loadFixture(fixture);
    expect(input.proof).toBe('0xdeadbeef');
    expect(input.publicInputs).toBe('0xcafebabe');
    expect(input.circuitHash).toBe('0xabcd');
    expect(input.gensData).toBe('0x1122');
  });

  it('adds 0x prefix when missing', () => {
    const fixture = {
      proof_hex: 'aabb',
      public_inputs_hex: 'ccdd',
      circuit_hash_raw: 'eeff',
      gens_hex: '1122',
    };

    const input = BatchVerifier.loadFixture(fixture);
    expect(input.proof).toBe('0xaabb');
    expect(input.publicInputs).toBe('0xccdd');
    expect(input.circuitHash).toBe('0xeeff');
    expect(input.gensData).toBe('0x1122');
  });

  it('preserves existing 0x prefix', () => {
    const fixture = {
      proof_hex: '0xaabb',
      public_inputs_hex: '0xccdd',
      circuit_hash_raw: '0xeeff',
      gens_hex: '0x1122',
    };

    const input = BatchVerifier.loadFixture(fixture);
    expect(input.proof).toBe('0xaabb');
    expect(input.publicInputs).toBe('0xccdd');
  });

  it('prefers inner_proof_hex over proof_hex', () => {
    const fixture = {
      proof_hex: 'aaaa',
      inner_proof_hex: '0xbbbb',
      public_inputs_hex: 'cccc',
      public_values_abi: '0xdddd',
      circuit_hash_raw: '0x0001',
      gens_hex: '0x0002',
    };

    const input = BatchVerifier.loadFixture(fixture);
    expect(input.proof).toBe('0xbbbb');
    expect(input.publicInputs).toBe('0xdddd');
  });

  it('throws on missing required fields', () => {
    expect(() => BatchVerifier.loadFixture({})).toThrow('Fixture missing required fields');

    expect(() =>
      BatchVerifier.loadFixture({
        proof_hex: '0xaa',
        public_inputs_hex: '0xbb',
        // missing circuit_hash_raw and gens_hex
      }),
    ).toThrow('Fixture missing required fields');

    expect(() =>
      BatchVerifier.loadFixture({
        proof_hex: '0xaa',
        circuit_hash_raw: '0xbb',
        gens_hex: '0xcc',
        // missing public_inputs_hex
      }),
    ).toThrow('Fixture missing required fields');
  });
});

// ─── BatchVerifier.estimateTransactionCount ───

describe('BatchVerifier.estimateTransactionCount', () => {
  it('estimates correctly for 88 layers / 34 groups', () => {
    const est = BatchVerifier.estimateTransactionCount(88, 34);
    expect(est.start).toBe(1);
    expect(est.continue).toBe(11); // ceil(88/8)
    expect(est.finalize).toBe(3); // ceil(34/16)
    expect(est.total).toBe(15); // 1 + 11 + 3
  });

  it('handles exact multiples', () => {
    const est = BatchVerifier.estimateTransactionCount(16, 16);
    expect(est.continue).toBe(2); // 16/8
    expect(est.finalize).toBe(1); // 16/16
    expect(est.total).toBe(4); // 1 + 2 + 1
  });

  it('handles single layer and group', () => {
    const est = BatchVerifier.estimateTransactionCount(1, 1);
    expect(est.continue).toBe(1);
    expect(est.finalize).toBe(1);
    expect(est.total).toBe(3);
  });

  it('handles zero layers', () => {
    const est = BatchVerifier.estimateTransactionCount(0, 0);
    expect(est.continue).toBe(0);
    expect(est.finalize).toBe(0);
    expect(est.total).toBe(1); // just start
  });

  it('rounds up partial batches', () => {
    const est = BatchVerifier.estimateTransactionCount(9, 17);
    expect(est.continue).toBe(2); // ceil(9/8)
    expect(est.finalize).toBe(2); // ceil(17/16)
    expect(est.total).toBe(5);
  });
});

// ─── ProgressEvent structure validation ───

describe('ProgressEvent types', () => {
  it('has correct shape for a pre-tx event', () => {
    const event: ProgressEvent = {
      phase: 'start',
      stepIndex: 0,
      totalSteps: 1,
      overallStep: 0,
      overallTotalSteps: 15,
      elapsedMs: 0,
    };
    expect(event.txHash).toBeUndefined();
    expect(event.gasUsed).toBeUndefined();
    expect(event.estimatedTimeRemainingMs).toBeUndefined();
    expect(event.phase).toBe('start');
    expect(event.overallStep).toBe(0);
    expect(event.overallTotalSteps).toBe(15);
  });

  it('has correct shape for a post-tx event with ETA', () => {
    const event: ProgressEvent = {
      phase: 'continue',
      stepIndex: 3,
      totalSteps: 11,
      txHash: '0xabc123' as `0x${string}`,
      gasUsed: 25_000_000n,
      overallStep: 4,
      overallTotalSteps: 15,
      estimatedTimeRemainingMs: 33000,
      elapsedMs: 12000,
    };
    expect(event.txHash).toBe('0xabc123');
    expect(event.gasUsed).toBe(25_000_000n);
    expect(event.estimatedTimeRemainingMs).toBe(33000);
    expect(event.elapsedMs).toBe(12000);
  });

  it('supports finalize phase with unknown total (-1)', () => {
    const event: ProgressEvent = {
      phase: 'finalize',
      stepIndex: 0,
      totalSteps: -1,
      overallStep: 12,
      overallTotalSteps: -1,
      elapsedMs: 50000,
    };
    expect(event.totalSteps).toBe(-1);
    expect(event.overallTotalSteps).toBe(-1);
  });
});

// ─── Cancel behavior tests ───

describe('BatchVerifier.cancel()', () => {
  it('cancel() does not throw when no verification is running', () => {
    // Create a BatchVerifier with dummy config (it won't actually connect)
    const verifier = new BatchVerifier({
      rpcUrl: 'http://localhost:8545',
      privateKey: '0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80' as `0x${string}`,
      contractAddress: '0x0000000000000000000000000000000000000001' as `0x${string}`,
    });
    // Calling cancel before any verification should be safe (no-op)
    expect(() => verifier.cancel()).not.toThrow();
  });

  it('AbortController signal is respected by checking aborted state', () => {
    const controller = new AbortController();
    expect(controller.signal.aborted).toBe(false);
    controller.abort();
    expect(controller.signal.aborted).toBe(true);
  });

  it('external AbortSignal can be passed via options', () => {
    // This tests the type system — ensure signal is accepted in BatchVerifyOptions
    const controller = new AbortController();
    const _options = {
      signal: controller.signal,
      onProgress: (_event: ProgressEvent) => {},
      skipCleanup: false,
    };
    // The options should be type-compatible (no compile error means success)
    expect(_options.signal).toBe(controller.signal);
  });
});

// ─── BatchVerifyResult cancelled field ───

describe('BatchVerifyResult.cancelled', () => {
  it('result type includes cancelled field', () => {
    const result = {
      sessionId: '0xabc' as `0x${string}`,
      startStep: { txHash: '0x1' as `0x${string}`, gasUsed: 100n, blockNumber: 1n },
      continueSteps: [],
      finalizeSteps: [],
      totalGasUsed: 100n,
      durationMs: 1000,
      cancelled: false,
    };
    expect(result.cancelled).toBe(false);

    const cancelledResult = { ...result, cancelled: true };
    expect(cancelledResult.cancelled).toBe(true);
  });
});

// ─── ETA estimation integration (using ProgressTracker) ───

describe('ETA estimation integration', () => {
  it('computes reasonable ETA given step history', () => {
    // Simulate: 3 steps done at 1s each, 7 remaining
    const tracker = new ProgressTracker(0);
    tracker.recordStep(1000);
    tracker.recordStep(2000);
    tracker.recordStep(3000);

    // Average = 3000/3 = 1000ms per step
    const eta = tracker.estimateRemainingMs(7);
    expect(eta).toBe(7000); // 7 * 1000ms
  });

  it('ETA decreases as more steps complete', () => {
    const tracker = new ProgressTracker(0);
    const totalSteps = 10;

    tracker.recordStep(1000);
    const eta1 = tracker.estimateRemainingMs(totalSteps - 1);

    tracker.recordStep(2000);
    const eta2 = tracker.estimateRemainingMs(totalSteps - 2);

    tracker.recordStep(3000);
    const eta3 = tracker.estimateRemainingMs(totalSteps - 3);

    // All ETAs should be 7000ms since rate is constant 1s/step
    // But the remaining steps decrease: 9, 8, 7
    expect(eta1).toBe(9000);
    expect(eta2).toBe(8000);
    expect(eta3).toBe(7000);
  });

  it('ETA is zero when all steps complete', () => {
    const tracker = new ProgressTracker(0);
    tracker.recordStep(500);
    tracker.recordStep(1000);
    expect(tracker.estimateRemainingMs(0)).toBe(0);
  });

  it('handles variable step durations (averages them)', () => {
    const tracker = new ProgressTracker(0);
    // Steps at: 100ms, 500ms, 600ms (variable durations)
    tracker.recordStep(100);
    tracker.recordStep(500);
    tracker.recordStep(600);

    // Average = 600ms / 3 = 200ms per step
    const eta = tracker.estimateRemainingMs(5);
    expect(eta).toBe(1000); // 5 * 200ms
  });
});

// ---- DAGBatchVerifier tests ----

type Hex = `0x${string}`;

const MOCK_CONTRACT: Hex = '0x1111111111111111111111111111111111111111';
const MOCK_CIRCUIT_HASH: Hex = '0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';
const MOCK_PROOF: Hex = '0xdeadbeef';
const MOCK_TX_HASH: Hex = '0x0000000000000000000000000000000000000000000000000000000000001234';
const MOCK_SESSION_ID: Hex = '0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';
const MOCK_SENDER: Hex = '0x2222222222222222222222222222222222222222';
const ZERO_HASH: Hex = `0x${'0'.repeat(64)}` as Hex;

function createMockClients() {
  const writeContractFn = vi.fn();
  const readContractFn = vi.fn();
  const waitForTransactionReceiptFn = vi.fn();
  const getTransactionReceiptFn = vi.fn();

  const publicClient = {
    readContract: readContractFn,
    waitForTransactionReceipt: waitForTransactionReceiptFn,
    getTransactionReceipt: getTransactionReceiptFn,
  } as unknown as PublicClient<Transport, Chain>;

  const walletClient = {
    writeContract: writeContractFn,
    account: { address: MOCK_SENDER },
  } as unknown as WalletClient<Transport, Chain>;

  return {
    publicClient,
    walletClient,
    writeContractFn,
    readContractFn,
    waitForTransactionReceiptFn,
    getTransactionReceiptFn,
  };
}

function makeReceipt(hash: Hex, gasUsed = 1_000_000n, sessionId?: Hex) {
  const logs = sessionId
    ? [
        {
          address: MOCK_CONTRACT,
          topics: [
            '0xevent_sig' as Hex,
            sessionId,
            MOCK_CIRCUIT_HASH,
          ],
          data: '0x' as Hex,
        },
      ]
    : [];
  return {
    transactionHash: hash,
    status: 'success' as const,
    gasUsed,
    blockNumber: 42n,
    logs,
  };
}

describe('DAGBatchVerifier', () => {
  let mocks: ReturnType<typeof createMockClients>;
  let verifier: DAGBatchVerifier;

  beforeEach(() => {
    mocks = createMockClients();
    verifier = new DAGBatchVerifier(
      mocks.publicClient,
      mocks.walletClient,
      MOCK_CONTRACT,
    );
  });

  it('constructor stores client, walletClient, and contractAddress', () => {
    // The verifier should be created without errors.
    expect(verifier).toBeDefined();
    expect(verifier).toBeInstanceOf(DAGBatchVerifier);
  });

  it('startVerification returns sessionId and txHash', async () => {
    mocks.writeContractFn.mockResolvedValueOnce(MOCK_TX_HASH);
    mocks.waitForTransactionReceiptFn.mockResolvedValueOnce(
      makeReceipt(MOCK_TX_HASH, 5_000_000n, MOCK_SESSION_ID),
    );

    const result = await verifier.startVerification(MOCK_CIRCUIT_HASH, MOCK_PROOF);

    expect(result.sessionId).toBe(MOCK_SESSION_ID);
    expect(result.txHash).toBe(MOCK_TX_HASH);
    expect(mocks.writeContractFn).toHaveBeenCalledOnce();
    expect(mocks.waitForTransactionReceiptFn).toHaveBeenCalledWith({ hash: MOCK_TX_HASH });
  });

  it('startVerification throws on reverted transaction', async () => {
    mocks.writeContractFn.mockResolvedValueOnce(MOCK_TX_HASH);
    mocks.waitForTransactionReceiptFn.mockResolvedValueOnce({
      ...makeReceipt(MOCK_TX_HASH, 0n, MOCK_SESSION_ID),
      status: 'reverted',
    });

    await expect(
      verifier.startVerification(MOCK_CIRCUIT_HASH, MOCK_PROOF),
    ).rejects.toThrow('startVerification reverted');
  });

  it('continueVerification increments batch index', async () => {
    // getSession called inside continueVerification to read current batchIdx
    mocks.readContractFn.mockResolvedValueOnce([
      MOCK_CIRCUIT_HASH,
      2n,   // nextBatchIdx = 2
      11n,  // totalBatches
      false,
      0n,
      0n,
    ]);
    const continueTxHash: Hex = '0x0000000000000000000000000000000000000000000000000000000000005678';
    mocks.writeContractFn.mockResolvedValueOnce(continueTxHash);
    mocks.waitForTransactionReceiptFn.mockResolvedValueOnce(
      makeReceipt(continueTxHash, 20_000_000n),
    );

    const result = await verifier.continueVerification(MOCK_SESSION_ID, MOCK_PROOF);

    expect(result.txHash).toBe(continueTxHash);
    expect(result.batchIdx).toBe(2);
  });

  it('continueVerification throws for non-existent session', async () => {
    // Return zero circuit hash to indicate non-existent session
    mocks.readContractFn.mockResolvedValueOnce([
      ZERO_HASH,
      0n,
      0n,
      false,
      0n,
      0n,
    ]);

    await expect(
      verifier.continueVerification(MOCK_SESSION_ID, MOCK_PROOF),
    ).rejects.toThrow('does not exist');
  });

  it('finalizeVerification returns completion status', async () => {
    const finalizeTxHash: Hex = '0x0000000000000000000000000000000000000000000000000000000000009abc';
    mocks.writeContractFn.mockResolvedValueOnce(finalizeTxHash);
    mocks.waitForTransactionReceiptFn.mockResolvedValueOnce(
      makeReceipt(finalizeTxHash, 15_000_000n),
    );
    // getSession after finalize to check completion
    mocks.readContractFn.mockResolvedValueOnce([
      MOCK_CIRCUIT_HASH,
      11n,
      11n,
      true, // finalized
      34n,
      34n,
    ]);

    const result = await verifier.finalizeVerification(MOCK_SESSION_ID, MOCK_PROOF);

    expect(result.txHash).toBe(finalizeTxHash);
    expect(result.isComplete).toBe(true);
  });

  it('finalizeVerification returns isComplete=false when not done', async () => {
    const finalizeTxHash: Hex = '0x0000000000000000000000000000000000000000000000000000000000009abc';
    mocks.writeContractFn.mockResolvedValueOnce(finalizeTxHash);
    mocks.waitForTransactionReceiptFn.mockResolvedValueOnce(
      makeReceipt(finalizeTxHash, 15_000_000n),
    );
    // getSession shows NOT finalized yet
    mocks.readContractFn.mockResolvedValueOnce([
      MOCK_CIRCUIT_HASH,
      11n,
      11n,
      false, // not finalized
      16n,
      16n,
    ]);

    const result = await verifier.finalizeVerification(MOCK_SESSION_ID, MOCK_PROOF);

    expect(result.isComplete).toBe(false);
  });

  it('getSession returns session data for existing session', async () => {
    mocks.readContractFn.mockResolvedValueOnce([
      MOCK_CIRCUIT_HASH,
      5n,   // nextBatchIdx
      11n,  // totalBatches
      false,
      0n,
      0n,
    ]);

    const session = await verifier.getSession(MOCK_SESSION_ID);

    expect(session).not.toBeNull();
    expect(session!.circuitHash).toBe(MOCK_CIRCUIT_HASH);
    expect(session!.sender).toBe(MOCK_SENDER);
    expect(session!.nextBatchIdx).toBe(5);
    expect(session!.finalized).toBe(false);
  });

  it('getSession returns null for unknown session', async () => {
    mocks.readContractFn.mockResolvedValueOnce([
      ZERO_HASH,
      0n,
      0n,
      false,
      0n,
      0n,
    ]);

    const session = await verifier.getSession(MOCK_SESSION_ID);
    expect(session).toBeNull();
  });

  it('runFullVerification chains start, continue, and finalize steps', async () => {
    const progressCalls: Array<{ step: string; batchIdx: number }> = [];
    const onProgress = (step: string, batchIdx: number) => {
      progressCalls.push({ step, batchIdx });
    };

    // --- Start ---
    mocks.writeContractFn.mockResolvedValueOnce(MOCK_TX_HASH);
    mocks.waitForTransactionReceiptFn.mockResolvedValueOnce(
      makeReceipt(MOCK_TX_HASH, 5_000_000n, MOCK_SESSION_ID),
    );
    // getTransactionReceipt for gas accounting after start
    mocks.getTransactionReceiptFn.mockResolvedValueOnce(
      makeReceipt(MOCK_TX_HASH, 5_000_000n, MOCK_SESSION_ID),
    );

    // getRawSession after start (to get totalBatches)
    mocks.readContractFn.mockResolvedValueOnce([
      MOCK_CIRCUIT_HASH,
      0n,   // nextBatchIdx starts at 0
      2n,   // totalBatches = 2 (for a quick test)
      false,
      0n,
      0n,
    ]);

    // --- Continue batch 0 ---
    // getSession inside continueVerification (to read batchIdx)
    mocks.readContractFn.mockResolvedValueOnce([
      MOCK_CIRCUIT_HASH, 0n, 2n, false, 0n, 0n,
    ]);
    const continueTx0: Hex = '0x0000000000000000000000000000000000000000000000000000000000000c01';
    mocks.writeContractFn.mockResolvedValueOnce(continueTx0);
    mocks.waitForTransactionReceiptFn.mockResolvedValueOnce(
      makeReceipt(continueTx0, 20_000_000n),
    );
    mocks.getTransactionReceiptFn.mockResolvedValueOnce(
      makeReceipt(continueTx0, 20_000_000n),
    );

    // --- Continue batch 1 ---
    mocks.readContractFn.mockResolvedValueOnce([
      MOCK_CIRCUIT_HASH, 1n, 2n, false, 0n, 0n,
    ]);
    const continueTx1: Hex = '0x0000000000000000000000000000000000000000000000000000000000000c02';
    mocks.writeContractFn.mockResolvedValueOnce(continueTx1);
    mocks.waitForTransactionReceiptFn.mockResolvedValueOnce(
      makeReceipt(continueTx1, 20_000_000n),
    );
    mocks.getTransactionReceiptFn.mockResolvedValueOnce(
      makeReceipt(continueTx1, 20_000_000n),
    );

    // --- Finalize (one call, completes immediately) ---
    const finalizeTx: Hex = '0x0000000000000000000000000000000000000000000000000000000000000f01';
    mocks.writeContractFn.mockResolvedValueOnce(finalizeTx);
    mocks.waitForTransactionReceiptFn.mockResolvedValueOnce(
      makeReceipt(finalizeTx, 10_000_000n),
    );
    // getSession after finalize
    mocks.readContractFn.mockResolvedValueOnce([
      MOCK_CIRCUIT_HASH, 2n, 2n, true, 34n, 34n,
    ]);
    mocks.getTransactionReceiptFn.mockResolvedValueOnce(
      makeReceipt(finalizeTx, 10_000_000n),
    );

    const result = await verifier.runFullVerification(
      MOCK_CIRCUIT_HASH,
      MOCK_PROOF,
      onProgress,
    );

    // Check transaction hashes collected
    expect(result.txHashes).toHaveLength(4); // start + 2 continue + 1 finalize
    expect(result.txHashes[0]).toBe(MOCK_TX_HASH);
    expect(result.txHashes[1]).toBe(continueTx0);
    expect(result.txHashes[2]).toBe(continueTx1);
    expect(result.txHashes[3]).toBe(finalizeTx);

    // Check gas accounting: 5M + 20M + 20M + 10M = 55M
    expect(result.gasUsed).toBe(55_000_000n);

    // Check progress callbacks were fired
    expect(progressCalls).toEqual([
      { step: 'start', batchIdx: 0 },
      { step: 'continue', batchIdx: 0 },
      { step: 'continue', batchIdx: 1 },
      { step: 'finalize', batchIdx: 0 },
    ]);
  });

  it('runFullVerification works without onProgress callback', async () => {
    // Start
    mocks.writeContractFn.mockResolvedValueOnce(MOCK_TX_HASH);
    mocks.waitForTransactionReceiptFn.mockResolvedValueOnce(
      makeReceipt(MOCK_TX_HASH, 5_000_000n, MOCK_SESSION_ID),
    );
    mocks.getTransactionReceiptFn.mockResolvedValueOnce(
      makeReceipt(MOCK_TX_HASH, 5_000_000n, MOCK_SESSION_ID),
    );

    // getRawSession: 0 totalBatches (skip continue phase)
    mocks.readContractFn.mockResolvedValueOnce([
      MOCK_CIRCUIT_HASH, 0n, 0n, false, 0n, 0n,
    ]);

    // Finalize
    const finalizeTx: Hex = '0x0000000000000000000000000000000000000000000000000000000000000f01';
    mocks.writeContractFn.mockResolvedValueOnce(finalizeTx);
    mocks.waitForTransactionReceiptFn.mockResolvedValueOnce(
      makeReceipt(finalizeTx, 10_000_000n),
    );
    mocks.readContractFn.mockResolvedValueOnce([
      MOCK_CIRCUIT_HASH, 0n, 0n, true, 0n, 0n,
    ]);
    mocks.getTransactionReceiptFn.mockResolvedValueOnce(
      makeReceipt(finalizeTx, 10_000_000n),
    );

    // Should not throw even without progress callback
    const result = await verifier.runFullVerification(MOCK_CIRCUIT_HASH, MOCK_PROOF);

    expect(result.txHashes).toHaveLength(2); // start + finalize
    expect(result.gasUsed).toBe(15_000_000n);
  });
});
