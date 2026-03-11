export type Hex = `0x${string}`;

export interface BatchVerifierConfig {
  rpcUrl: string;
  privateKey: Hex;
  contractAddress: Hex;
  gasLimit?: bigint;
  maxRetries?: number;
  retryDelayMs?: number;
}

export interface BatchVerifyInput {
  proof: Hex;
  circuitHash: Hex;
  publicInputs: Hex;
  gensData: Hex;
}

export interface StepResult {
  txHash: Hex;
  gasUsed: bigint;
  blockNumber: bigint;
}

export interface BatchVerifyResult {
  sessionId: Hex;
  startStep: StepResult;
  continueSteps: StepResult[];
  finalizeSteps: StepResult[];
  cleanupStep?: StepResult;
  totalGasUsed: bigint;
  durationMs: number;
  cancelled: boolean;
}

export interface BatchSession {
  circuitHash: Hex;
  nextBatchIdx: bigint;
  totalBatches: bigint;
  finalized: boolean;
  finalizeInputIdx: bigint;
  finalizeGroupsDone: bigint;
}

export type ProgressPhase = 'start' | 'continue' | 'finalize' | 'cleanup';

export interface ProgressEvent {
  /** Current phase of the batch verification. */
  phase: ProgressPhase;
  /** Zero-based index within the current phase. */
  stepIndex: number;
  /** Total steps in the current phase (-1 if unknown, e.g. finalize before completion). */
  totalSteps: number;
  /** Transaction hash, present after the tx is confirmed. */
  txHash?: Hex;
  /** Gas used by this step, present after the tx is confirmed. */
  gasUsed?: bigint;
  /** Overall step index across all phases (0-based). */
  overallStep: number;
  /** Total steps across all phases (-1 if unknown). */
  overallTotalSteps: number;
  /** Estimated milliseconds remaining, based on average tx time so far. Undefined before first tx completes. */
  estimatedTimeRemainingMs?: number;
  /** Elapsed milliseconds since batch verification started. */
  elapsedMs: number;
}

export type ProgressCallback = (event: ProgressEvent) => void;

export interface BatchVerifyOptions {
  onProgress?: ProgressCallback;
  skipCleanup?: boolean;
  /** AbortSignal to cancel the batch verification. When aborted, no further transactions are submitted. */
  signal?: AbortSignal;
}
