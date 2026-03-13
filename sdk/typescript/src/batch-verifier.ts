import {
  createPublicClient,
  createWalletClient,
  http,
  encodeFunctionData,
  type PublicClient,
  type WalletClient,
  type TransactionReceipt,
  type Transport,
  type Chain,
  defineChain,
} from 'viem';
import { privateKeyToAccount } from 'viem/accounts';
import { batchVerifierAbi } from './batch-verifier-abi';
import type {
  Hex,
  BatchVerifierConfig,
  BatchVerifyInput,
  BatchVerifyResult,
  BatchSession,
  StepResult,
  BatchVerifyOptions,
  ProgressEvent,
} from './batch-verifier-types';

const LAYERS_PER_BATCH = 8;
const GROUPS_PER_FINALIZE_BATCH = 16;
const DEFAULT_GAS_LIMIT = 30_000_000n;
const DEFAULT_MAX_RETRIES = 3;
const DEFAULT_RETRY_DELAY_MS = 2000;

/**
 * Tracks timing for completed steps to estimate time remaining.
 * Exported for testing.
 */
export class ProgressTracker {
  private stepTimestamps: number[] = [];
  private startTime: number;

  constructor(startTime?: number) {
    this.startTime = startTime ?? Date.now();
  }

  /** Record that a step completed at the given timestamp (or now). */
  recordStep(timestamp?: number): void {
    this.stepTimestamps.push(timestamp ?? Date.now());
  }

  /** Number of completed steps. */
  get completedSteps(): number {
    return this.stepTimestamps.length;
  }

  /** Average milliseconds per step, or undefined if no steps completed. */
  get averageStepMs(): number | undefined {
    if (this.stepTimestamps.length === 0) return undefined;
    const elapsed = this.stepTimestamps[this.stepTimestamps.length - 1] - this.startTime;
    return elapsed / this.stepTimestamps.length;
  }

  /** Elapsed milliseconds since start. */
  get elapsedMs(): number {
    return Date.now() - this.startTime;
  }

  /**
   * Estimate time remaining in milliseconds.
   * Returns undefined if no steps completed yet or totalSteps is unknown.
   */
  estimateRemainingMs(remainingSteps: number): number | undefined {
    const avg = this.averageStepMs;
    if (avg === undefined || remainingSteps < 0) return undefined;
    return Math.round(avg * remainingSteps);
  }

  /** Reset for testing purposes. */
  getStartTime(): number {
    return this.startTime;
  }
}

export class BatchVerifier {
  private publicClient: PublicClient<Transport, Chain>;
  private walletClient: WalletClient<Transport, Chain>;
  private contractAddress: Hex;
  private gasLimit: bigint;
  private maxRetries: number;
  private retryDelayMs: number;
  private abortController: AbortController | null = null;

  constructor(config: BatchVerifierConfig) {
    const chain = defineChain({
      id: 31337,
      name: 'Local',
      nativeCurrency: { name: 'Ether', symbol: 'ETH', decimals: 18 },
      rpcUrls: { default: { http: [config.rpcUrl] } },
    });
    const transport = http(config.rpcUrl);
    const account = privateKeyToAccount(config.privateKey);

    this.publicClient = createPublicClient({ chain, transport });
    this.walletClient = createWalletClient({ chain, transport, account });
    this.contractAddress = config.contractAddress;
    this.gasLimit = config.gasLimit ?? DEFAULT_GAS_LIMIT;
    this.maxRetries = config.maxRetries ?? DEFAULT_MAX_RETRIES;
    this.retryDelayMs = config.retryDelayMs ?? DEFAULT_RETRY_DELAY_MS;
  }

  /**
   * Cancel an in-progress batch verification.
   * No further transactions will be submitted after the current one completes.
   * The partial result (with cancelled: true) will be returned from verifyBatch/resumeBatch.
   */
  cancel(): void {
    if (this.abortController) {
      this.abortController.abort();
    }
  }

  async isCircuitActive(circuitHash: Hex): Promise<boolean> {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: batchVerifierAbi,
      functionName: 'isDAGCircuitActive',
      args: [circuitHash],
    }) as Promise<boolean>;
  }

  async getSession(sessionId: Hex): Promise<BatchSession> {
    const result = (await this.publicClient.readContract({
      address: this.contractAddress,
      abi: batchVerifierAbi,
      functionName: 'getDAGBatchSession',
      args: [sessionId],
    })) as [Hex, bigint, bigint, boolean, bigint, bigint];

    return {
      circuitHash: result[0],
      nextBatchIdx: result[1],
      totalBatches: result[2],
      finalized: result[3],
      finalizeInputIdx: result[4],
      finalizeGroupsDone: result[5],
    };
  }

  async verifyBatch(
    input: BatchVerifyInput,
    options?: BatchVerifyOptions,
  ): Promise<BatchVerifyResult> {
    // Create a new AbortController for this verification run.
    // Merge with any external signal from options.
    this.abortController = new AbortController();
    const internalSignal = this.abortController.signal;

    // If an external signal is provided, forward its abort to our internal controller.
    if (options?.signal) {
      if (options.signal.aborted) {
        this.abortController.abort();
      } else {
        options.signal.addEventListener('abort', () => this.abortController?.abort(), { once: true });
      }
    }

    const startTime = Date.now();
    const onProgress = options?.onProgress;
    const tracker = new ProgressTracker(startTime);
    let cancelled = false;

    const active = await this.isCircuitActive(input.circuitHash);
    if (!active) {
      throw new Error(
        `Circuit ${input.circuitHash} is not registered or active`,
      );
    }

    // Emit pre-start progress
    let overallStep = 0;
    // We don't know the total yet; we'll update after start tx.
    const emitProgress = (
      phase: ProgressEvent['phase'],
      stepIndex: number,
      totalSteps: number,
      overallTotalSteps: number,
      txHash?: Hex,
      gasUsed?: bigint,
    ) => {
      if (!onProgress) return;

      const remainingSteps = overallTotalSteps > 0
        ? overallTotalSteps - overallStep
        : -1;

      onProgress({
        phase,
        stepIndex,
        totalSteps,
        txHash,
        gasUsed,
        overallStep,
        overallTotalSteps,
        estimatedTimeRemainingMs: remainingSteps >= 0
          ? tracker.estimateRemainingMs(remainingSteps)
          : undefined,
        elapsedMs: Date.now() - startTime,
      });
    };

    // --- Start ---
    emitProgress('start', 0, 1, -1);

    this.checkAborted(internalSignal);
    const startReceipt = await this.sendTransaction(
      'startDAGBatchVerify',
      [input.proof, input.circuitHash, input.publicInputs, input.gensData],
    );
    tracker.recordStep();

    const sessionId = this.extractSessionIdFromLogs(startReceipt);
    const session = await this.getSession(sessionId);
    const totalBatches = Number(session.totalBatches);

    // Estimate finalize steps (unknown exactly, use groups heuristic if available).
    // We don't know numEvalGroups from on-chain, so estimate finalize = 3 (typical XGBoost).
    // This will self-correct once finalize steps happen.
    const estimatedFinalize = 3;
    const overallTotalWithCleanup = 1 + totalBatches + estimatedFinalize + (options?.skipCleanup ? 0 : 1);

    const startStep: StepResult = {
      txHash: startReceipt.transactionHash,
      gasUsed: startReceipt.gasUsed,
      blockNumber: startReceipt.blockNumber,
    };

    overallStep = 1;
    emitProgress('start', 0, 1, overallTotalWithCleanup, startStep.txHash, startStep.gasUsed);

    // --- Continue ---
    const continueSteps: StepResult[] = [];
    for (let i = 0; i < totalBatches; i++) {
      if (internalSignal.aborted) {
        cancelled = true;
        break;
      }

      emitProgress('continue', i, totalBatches, overallTotalWithCleanup);

      const receipt = await this.sendTransaction(
        'continueDAGBatchVerify',
        [sessionId, input.proof, input.publicInputs, input.gensData],
      );
      tracker.recordStep();

      const step: StepResult = {
        txHash: receipt.transactionHash,
        gasUsed: receipt.gasUsed,
        blockNumber: receipt.blockNumber,
      };
      continueSteps.push(step);

      overallStep = 1 + i + 1;
      emitProgress('continue', i, totalBatches, overallTotalWithCleanup, step.txHash, step.gasUsed);
    }

    // --- Finalize ---
    const finalizeSteps: StepResult[] = [];
    if (!cancelled) {
      let finalized = false;
      let finalizeIdx = 0;
      while (!finalized) {
        if (internalSignal.aborted) {
          cancelled = true;
          break;
        }

        emitProgress('finalize', finalizeIdx, -1, overallTotalWithCleanup);

        const receipt = await this.sendTransaction(
          'finalizeDAGBatchVerify',
          [sessionId, input.proof, input.publicInputs, input.gensData],
        );
        tracker.recordStep();

        const step: StepResult = {
          txHash: receipt.transactionHash,
          gasUsed: receipt.gasUsed,
          blockNumber: receipt.blockNumber,
        };
        finalizeSteps.push(step);

        const sessionState = await this.getSession(sessionId);
        finalized = sessionState.finalized;

        overallStep = 1 + totalBatches + finalizeIdx + 1;
        emitProgress(
          'finalize',
          finalizeIdx,
          finalized ? finalizeIdx + 1 : -1,
          overallTotalWithCleanup,
          step.txHash,
          step.gasUsed,
        );

        finalizeIdx++;
      }
    }

    // --- Cleanup ---
    let cleanupStep: StepResult | undefined;
    if (!cancelled && !options?.skipCleanup) {
      emitProgress('cleanup', 0, 1, overallTotalWithCleanup);

      const receipt = await this.sendTransaction(
        'cleanupDAGBatchSession',
        [sessionId],
      );
      tracker.recordStep();

      cleanupStep = {
        txHash: receipt.transactionHash,
        gasUsed: receipt.gasUsed,
        blockNumber: receipt.blockNumber,
      };

      overallStep++;
      emitProgress('cleanup', 0, 1, overallTotalWithCleanup, cleanupStep.txHash, cleanupStep.gasUsed);
    }

    const totalGasUsed =
      startStep.gasUsed +
      continueSteps.reduce((sum, s) => sum + s.gasUsed, 0n) +
      finalizeSteps.reduce((sum, s) => sum + s.gasUsed, 0n) +
      (cleanupStep?.gasUsed ?? 0n);

    this.abortController = null;

    return {
      sessionId,
      startStep,
      continueSteps,
      finalizeSteps,
      cleanupStep,
      totalGasUsed,
      durationMs: Date.now() - startTime,
      cancelled,
    };
  }

  async resumeBatch(
    sessionId: Hex,
    input: BatchVerifyInput,
    options?: BatchVerifyOptions,
  ): Promise<BatchVerifyResult> {
    this.abortController = new AbortController();
    const internalSignal = this.abortController.signal;

    if (options?.signal) {
      if (options.signal.aborted) {
        this.abortController.abort();
      } else {
        options.signal.addEventListener('abort', () => this.abortController?.abort(), { once: true });
      }
    }

    const startTime = Date.now();
    const onProgress = options?.onProgress;
    const tracker = new ProgressTracker(startTime);
    let cancelled = false;

    const session = await this.getSession(sessionId);

    if (session.circuitHash === '0x' + '0'.repeat(64)) {
      throw new Error(`Session ${sessionId} does not exist`);
    }

    const totalBatches = Number(session.totalBatches);
    const startBatchIdx = Number(session.nextBatchIdx);

    // Placeholder for start (already done)
    const startStep: StepResult = {
      txHash: '0x0' as Hex,
      gasUsed: 0n,
      blockNumber: 0n,
    };

    const remainingContinue = totalBatches - startBatchIdx;
    const estimatedFinalize = 3;
    const overallTotalSteps = remainingContinue + estimatedFinalize + (options?.skipCleanup ? 0 : 1);
    let overallStep = 0;

    const emitProgress = (
      phase: ProgressEvent['phase'],
      stepIndex: number,
      totalSteps: number,
      txHash?: Hex,
      gasUsed?: bigint,
    ) => {
      if (!onProgress) return;

      const remainingSteps = overallTotalSteps > 0
        ? overallTotalSteps - overallStep
        : -1;

      onProgress({
        phase,
        stepIndex,
        totalSteps,
        txHash,
        gasUsed,
        overallStep,
        overallTotalSteps,
        estimatedTimeRemainingMs: remainingSteps >= 0
          ? tracker.estimateRemainingMs(remainingSteps)
          : undefined,
        elapsedMs: Date.now() - startTime,
      });
    };

    // Continue remaining batches
    const continueSteps: StepResult[] = [];
    for (let i = startBatchIdx; i < totalBatches; i++) {
      if (internalSignal.aborted) {
        cancelled = true;
        break;
      }

      emitProgress('continue', i, totalBatches);

      const receipt = await this.sendTransaction(
        'continueDAGBatchVerify',
        [sessionId, input.proof, input.publicInputs, input.gensData],
      );
      tracker.recordStep();

      const step: StepResult = {
        txHash: receipt.transactionHash,
        gasUsed: receipt.gasUsed,
        blockNumber: receipt.blockNumber,
      };
      continueSteps.push(step);

      overallStep++;
      emitProgress('continue', i, totalBatches, step.txHash, step.gasUsed);
    }

    // Finalize (if not already finalized)
    const finalizeSteps: StepResult[] = [];
    if (!cancelled && !session.finalized) {
      let finalized = false;
      let finalizeIdx = 0;
      while (!finalized) {
        if (internalSignal.aborted) {
          cancelled = true;
          break;
        }

        emitProgress('finalize', finalizeIdx, -1);

        const receipt = await this.sendTransaction(
          'finalizeDAGBatchVerify',
          [sessionId, input.proof, input.publicInputs, input.gensData],
        );
        tracker.recordStep();

        const step: StepResult = {
          txHash: receipt.transactionHash,
          gasUsed: receipt.gasUsed,
          blockNumber: receipt.blockNumber,
        };
        finalizeSteps.push(step);

        const state = await this.getSession(sessionId);
        finalized = state.finalized;

        overallStep++;
        emitProgress(
          'finalize',
          finalizeIdx,
          finalized ? finalizeIdx + 1 : -1,
          step.txHash,
          step.gasUsed,
        );

        finalizeIdx++;
      }
    }

    // Cleanup
    let cleanupStep: StepResult | undefined;
    if (!cancelled && !options?.skipCleanup) {
      emitProgress('cleanup', 0, 1);

      const receipt = await this.sendTransaction(
        'cleanupDAGBatchSession',
        [sessionId],
      );
      tracker.recordStep();

      cleanupStep = {
        txHash: receipt.transactionHash,
        gasUsed: receipt.gasUsed,
        blockNumber: receipt.blockNumber,
      };

      overallStep++;
      emitProgress('cleanup', 0, 1, cleanupStep.txHash, cleanupStep.gasUsed);
    }

    const totalGasUsed =
      continueSteps.reduce((sum, s) => sum + s.gasUsed, 0n) +
      finalizeSteps.reduce((sum, s) => sum + s.gasUsed, 0n) +
      (cleanupStep?.gasUsed ?? 0n);

    this.abortController = null;

    return {
      sessionId,
      startStep,
      continueSteps,
      finalizeSteps,
      cleanupStep,
      totalGasUsed,
      durationMs: Date.now() - startTime,
      cancelled,
    };
  }

  static loadFixture(fixtureData: Record<string, unknown>): BatchVerifyInput {
    // Support both fixture formats:
    // - phase1a_dag_fixture: proof_hex, public_inputs_hex
    // - dag_groth16_e2e_fixture: inner_proof_hex, public_values_abi
    const proof = (fixtureData['inner_proof_hex'] ??
      fixtureData['proof_hex']) as string;
    const publicInputs = (fixtureData['public_values_abi'] ??
      fixtureData['public_inputs_hex']) as string;
    const circuitHash = fixtureData['circuit_hash_raw'] as string;
    const gensData = fixtureData['gens_hex'] as string;

    if (!proof || !publicInputs || !circuitHash || !gensData) {
      throw new Error(
        'Fixture missing required fields: proof, publicInputs, circuitHash, or gensData',
      );
    }

    return {
      proof: ensureHex(proof),
      circuitHash: ensureHex(circuitHash),
      publicInputs: ensureHex(publicInputs),
      gensData: ensureHex(gensData),
    };
  }

  static estimateTransactionCount(
    numComputeLayers: number,
    numEvalGroups: number,
  ): { start: number; continue: number; finalize: number; total: number } {
    const continueTxs = Math.ceil(numComputeLayers / LAYERS_PER_BATCH);
    const finalizeTxs = Math.ceil(numEvalGroups / GROUPS_PER_FINALIZE_BATCH);
    return {
      start: 1,
      continue: continueTxs,
      finalize: finalizeTxs,
      total: 1 + continueTxs + finalizeTxs,
    };
  }

  private checkAborted(signal: AbortSignal): void {
    if (signal.aborted) {
      throw new BatchVerificationCancelledError();
    }
  }

  private async sendTransaction(
    functionName: string,
    args: unknown[],
  ): Promise<TransactionReceipt> {
    let lastError: Error | undefined;

    for (let attempt = 0; attempt <= this.maxRetries; attempt++) {
      try {
        let gas: bigint;
        try {
          gas = await this.publicClient.estimateGas({
            account: this.walletClient.account!,
            to: this.contractAddress,
            data: this.encodeCallData(functionName, args),
          });
          // Add 10% buffer
          gas = (gas * 110n) / 100n;
        } catch {
          gas = this.gasLimit;
        }

        const hash = await this.walletClient.writeContract({
          address: this.contractAddress,
          abi: batchVerifierAbi,
          functionName,
          args,
          gas,
        } as any);

        const receipt = await this.publicClient.waitForTransactionReceipt({
          hash,
        });

        if (receipt.status === 'reverted') {
          throw new Error(`Transaction reverted: ${hash}`);
        }

        return receipt;
      } catch (error) {
        lastError = error as Error;
        const msg = lastError.message.toLowerCase();
        const isTransient =
          msg.includes('nonce') ||
          msg.includes('timeout') ||
          msg.includes('connection');

        if (!isTransient || attempt === this.maxRetries) {
          throw lastError;
        }

        await sleep(this.retryDelayMs * (attempt + 1));
      }
    }

    throw lastError!;
  }

  private encodeCallData(
    functionName: string,
    args: unknown[],
  ): Hex {
    return encodeFunctionData({
      abi: batchVerifierAbi,
      functionName,
      args,
    } as any);
  }

  private extractSessionIdFromLogs(receipt: TransactionReceipt): Hex {
    // Find DAGBatchSessionStarted event: 3 topics (signature + sessionId + circuitHash)
    for (const log of receipt.logs) {
      if (log.topics.length === 3 && log.address.toLowerCase() === this.contractAddress.toLowerCase()) {
        // topics[1] is the indexed sessionId
        return log.topics[1] as Hex;
      }
    }

    throw new Error(
      'DAGBatchSessionStarted event not found in transaction receipt',
    );
  }
}

/**
 * Session state for a DAG batch verification, as returned by getSession().
 */
export interface DAGBatchSession {
  circuitHash: Hex;
  sender: Hex;
  nextBatchIdx: number;
  finalized: boolean;
}

/**
 * A viem-native DAG batch verifier that accepts PublicClient and WalletClient directly.
 *
 * This class provides fine-grained control over each verification step (start, continue,
 * finalize) as well as a convenience method (runFullVerification) that chains them all.
 *
 * @example
 * ```typescript
 * import { createPublicClient, createWalletClient, http } from 'viem';
 * import { privateKeyToAccount } from 'viem/accounts';
 * import { DAGBatchVerifier } from '@worldzk/sdk';
 *
 * const publicClient = createPublicClient({ chain, transport: http(rpcUrl) });
 * const walletClient = createWalletClient({ chain, transport: http(rpcUrl), account });
 *
 * const verifier = new DAGBatchVerifier(publicClient, walletClient, contractAddress);
 *
 * const result = await verifier.runFullVerification(circuitHash, proof, (step, idx) => {
 *   console.log(`${step} batch ${idx}`);
 * });
 * console.log(`Gas used: ${result.gasUsed}`);
 * ```
 */
export class DAGBatchVerifier {
  private client: PublicClient<Transport, Chain>;
  private walletClient: WalletClient<Transport, Chain>;
  private contractAddress: Hex;

  constructor(
    client: PublicClient<Transport, Chain>,
    walletClient: WalletClient<Transport, Chain>,
    contractAddress: Hex,
  ) {
    this.client = client;
    this.walletClient = walletClient;
    this.contractAddress = contractAddress;
  }

  /**
   * Start a new DAG batch verification session.
   *
   * Sends the startDAGBatchVerify transaction and extracts the session ID
   * from the emitted DAGBatchSessionStarted event.
   */
  async startVerification(
    circuitHash: Hex,
    proof: Hex,
  ): Promise<{ sessionId: Hex; txHash: Hex }> {
    const hash = await this.walletClient.writeContract({
      address: this.contractAddress,
      abi: batchVerifierAbi,
      functionName: 'startDAGBatchVerify',
      args: [proof, circuitHash, '0x' as Hex, '0x' as Hex],
    } as any);

    const receipt = await this.client.waitForTransactionReceipt({ hash });
    if (receipt.status === 'reverted') {
      throw new Error(`startVerification reverted: ${hash}`);
    }

    const sessionId = this.extractSessionId(receipt);
    return { sessionId, txHash: hash };
  }

  /**
   * Continue a DAG batch verification session by processing the next batch of layers.
   *
   * Returns the transaction hash and the batch index that was processed.
   */
  async continueVerification(
    sessionId: Hex,
    proof: Hex,
  ): Promise<{ txHash: Hex; batchIdx: number }> {
    // Read current batch index before sending the transaction.
    const sessionBefore = await this.getSession(sessionId);
    if (!sessionBefore) {
      throw new Error(`Session ${sessionId} does not exist`);
    }
    const batchIdx = sessionBefore.nextBatchIdx;

    const hash = await this.walletClient.writeContract({
      address: this.contractAddress,
      abi: batchVerifierAbi,
      functionName: 'continueDAGBatchVerify',
      args: [sessionId, proof, '0x' as Hex, '0x' as Hex],
    } as any);

    const receipt = await this.client.waitForTransactionReceipt({ hash });
    if (receipt.status === 'reverted') {
      throw new Error(`continueVerification reverted at batch ${batchIdx}: ${hash}`);
    }

    return { txHash: hash, batchIdx };
  }

  /**
   * Finalize a DAG batch verification session (may need multiple calls).
   *
   * Returns the transaction hash and whether the finalization is complete.
   */
  async finalizeVerification(
    sessionId: Hex,
    proof: Hex,
  ): Promise<{ txHash: Hex; isComplete: boolean }> {
    const hash = await this.walletClient.writeContract({
      address: this.contractAddress,
      abi: batchVerifierAbi,
      functionName: 'finalizeDAGBatchVerify',
      args: [sessionId, proof, '0x' as Hex, '0x' as Hex],
    } as any);

    const receipt = await this.client.waitForTransactionReceipt({ hash });
    if (receipt.status === 'reverted') {
      throw new Error(`finalizeVerification reverted: ${hash}`);
    }

    // Check session state to determine if finalization is complete.
    const session = await this.getSession(sessionId);
    const isComplete = session ? session.finalized : true;

    return { txHash: hash, isComplete };
  }

  /**
   * Query the on-chain state for a batch verification session.
   *
   * Returns null if the session does not exist (all-zero circuitHash).
   */
  async getSession(sessionId: Hex): Promise<DAGBatchSession | null> {
    const result = (await this.client.readContract({
      address: this.contractAddress,
      abi: batchVerifierAbi,
      functionName: 'getDAGBatchSession',
      args: [sessionId],
    })) as [Hex, bigint, bigint, boolean, bigint, bigint];

    const circuitHash = result[0];

    // A zero circuit hash means the session does not exist.
    if (circuitHash === ('0x' + '0'.repeat(64))) {
      return null;
    }

    return {
      circuitHash,
      sender: this.walletClient.account?.address as Hex ?? '0x0' as Hex,
      nextBatchIdx: Number(result[1]),
      finalized: result[3],
    };
  }

  /**
   * Run a full verification from start through all continue and finalize steps.
   *
   * The optional onProgress callback is invoked before each step with a description
   * string and the current batch index.
   */
  async runFullVerification(
    circuitHash: Hex,
    proof: Hex,
    onProgress?: (step: string, batchIdx: number) => void,
  ): Promise<{ txHashes: Hex[]; gasUsed: bigint }> {
    const txHashes: Hex[] = [];
    let totalGas = 0n;

    // Step 1: Start
    onProgress?.('start', 0);
    const { sessionId, txHash: startHash } = await this.startVerification(circuitHash, proof);
    txHashes.push(startHash);
    const startReceipt = await this.client.getTransactionReceipt({ hash: startHash });
    totalGas += startReceipt.gasUsed;

    // Step 2: Read session to determine how many continue batches are needed.
    const sessionState = await this.getRawSession(sessionId);
    const totalBatches = sessionState ? Number(sessionState.totalBatches) : 0;
    const startBatchIdx = sessionState ? Number(sessionState.nextBatchIdx) : 0;

    for (let i = startBatchIdx; i < totalBatches; i++) {
      onProgress?.('continue', i);
      const continueResult = await this.continueVerification(sessionId, proof);
      txHashes.push(continueResult.txHash);
      const continueReceipt = await this.client.getTransactionReceipt({ hash: continueResult.txHash });
      totalGas += continueReceipt.gasUsed;
    }

    // Step 3: Finalize until complete.
    let isComplete = false;
    let finalizeIdx = 0;
    while (!isComplete) {
      onProgress?.('finalize', finalizeIdx);
      const finalizeResult = await this.finalizeVerification(sessionId, proof);
      txHashes.push(finalizeResult.txHash);
      const finalizeReceipt = await this.client.getTransactionReceipt({ hash: finalizeResult.txHash });
      totalGas += finalizeReceipt.gasUsed;
      isComplete = finalizeResult.isComplete;
      finalizeIdx++;
    }

    return { txHashes, gasUsed: totalGas };
  }

  /**
   * Read the raw session data including totalBatches (internal helper).
   */
  private async getRawSession(
    sessionId: Hex,
  ): Promise<{
    circuitHash: Hex;
    nextBatchIdx: bigint;
    totalBatches: bigint;
    finalized: boolean;
    finalizeInputIdx: bigint;
    finalizeGroupsDone: bigint;
  } | null> {
    const result = (await this.client.readContract({
      address: this.contractAddress,
      abi: batchVerifierAbi,
      functionName: 'getDAGBatchSession',
      args: [sessionId],
    })) as [Hex, bigint, bigint, boolean, bigint, bigint];

    const circuitHash = result[0];
    if (circuitHash === ('0x' + '0'.repeat(64))) {
      return null;
    }

    return {
      circuitHash,
      nextBatchIdx: result[1],
      totalBatches: result[2],
      finalized: result[3],
      finalizeInputIdx: result[4],
      finalizeGroupsDone: result[5],
    };
  }

  private extractSessionId(receipt: TransactionReceipt): Hex {
    for (const log of receipt.logs) {
      if (
        log.topics.length === 3 &&
        log.address.toLowerCase() === this.contractAddress.toLowerCase()
      ) {
        return log.topics[1] as Hex;
      }
    }
    throw new Error('DAGBatchSessionStarted event not found in transaction receipt');
  }
}

export class BatchVerificationCancelledError extends Error {
  constructor() {
    super('Batch verification was cancelled');
    this.name = 'BatchVerificationCancelledError';
  }
}

function ensureHex(value: string): Hex {
  if (value.startsWith('0x')) return value as Hex;
  return `0x${value}`;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// --- CLI ---
async function main() {
  const { parseArgs } = require('node:util') as typeof import('node:util');
  const fs = require('node:fs') as typeof import('node:fs');

  const { values } = parseArgs({
    options: {
      'rpc-url': { type: 'string' },
      'private-key': { type: 'string' },
      contract: { type: 'string' },
      fixture: { type: 'string' },
      'gas-limit': { type: 'string' },
      resume: { type: 'string' },
      'skip-cleanup': { type: 'boolean', default: false },
    },
  });

  if (!values['rpc-url'] || !values['private-key'] || !values.contract || !values.fixture) {
    console.error(
      'Usage: npx ts-node src/batch-verifier.ts \\\n' +
        '  --rpc-url <url> --private-key <key> --contract <addr> --fixture <path> \\\n' +
        '  [--gas-limit <gas>] [--resume <sessionId>] [--skip-cleanup]',
    );
    process.exit(1);
  }

  const fixtureData = JSON.parse(fs.readFileSync(values.fixture!, 'utf-8'));
  const input = BatchVerifier.loadFixture(fixtureData);

  const gasLimit = values['gas-limit']
    ? BigInt(values['gas-limit'])
    : undefined;

  const verifier = new BatchVerifier({
    rpcUrl: values['rpc-url']!,
    privateKey: values['private-key']! as Hex,
    contractAddress: values.contract! as Hex,
    gasLimit,
  });

  // Allow Ctrl+C to cancel gracefully
  process.on('SIGINT', () => {
    console.log('\nCancelling batch verification...');
    verifier.cancel();
  });

  const onProgress: import('./batch-verifier-types').ProgressCallback = (event) => {
    const gas = event.gasUsed ? ` | gas: ${event.gasUsed.toLocaleString()}` : '';
    const tx = event.txHash ? ` | tx: ${event.txHash.slice(0, 10)}...` : '';
    const step =
      event.totalSteps > 0
        ? `${event.stepIndex + 1}/${event.totalSteps}`
        : `${event.stepIndex + 1}`;
    const overall = event.overallTotalSteps > 0
      ? ` (${event.overallStep}/${event.overallTotalSteps})`
      : '';
    const eta = event.estimatedTimeRemainingMs !== undefined
      ? ` | ETA: ${(event.estimatedTimeRemainingMs / 1000).toFixed(1)}s`
      : '';
    console.log(`[${event.phase}] step ${step}${overall}${gas}${tx}${eta}`);
  };

  try {
    let result: BatchVerifyResult;

    if (values.resume) {
      console.log(`Resuming session: ${values.resume}`);
      result = await verifier.resumeBatch(
        values.resume as Hex,
        input,
        { onProgress, skipCleanup: values['skip-cleanup'] },
      );
    } else {
      const estimate = BatchVerifier.estimateTransactionCount(
        fixtureData.dag_circuit_description?.numComputeLayers ?? 88,
        fixtureData.config?.num_input_groups ?? 34,
      );
      console.log(
        `Estimated transactions: ${estimate.total} (1 start + ${estimate.continue} continue + ${estimate.finalize} finalize)`,
      );

      result = await verifier.verifyBatch(input, {
        onProgress,
        skipCleanup: values['skip-cleanup'],
      });
    }

    console.log('\n--- Batch Verification Complete ---');
    console.log(`Session ID:     ${result.sessionId}`);
    console.log(`Total gas used: ${result.totalGasUsed.toLocaleString()}`);
    console.log(`Duration:       ${(result.durationMs / 1000).toFixed(1)}s`);
    console.log(`Transactions:   ${1 + result.continueSteps.length + result.finalizeSteps.length + (result.cleanupStep ? 1 : 0)}`);
    if (result.cancelled) {
      console.log('Status:         CANCELLED (partial verification)');
    }
  } catch (error) {
    console.error('Batch verification failed:', (error as Error).message);
    process.exit(1);
  }
}

// Run CLI when executed directly
if (require.main === module) {
  main();
}
