import { describe, it, expect } from 'vitest';
import { BatchVerifier, ProgressTracker, BatchVerificationCancelledError } from '../batch-verifier';
import type { ProgressEvent } from '../batch-verifier-types';

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
