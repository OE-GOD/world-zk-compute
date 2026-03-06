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
  phase: ProgressPhase;
  stepIndex: number;
  totalSteps: number;
  txHash?: Hex;
  gasUsed?: bigint;
}

export type ProgressCallback = (event: ProgressEvent) => void;

export interface BatchVerifyOptions {
  onProgress?: ProgressCallback;
  skipCleanup?: boolean;
}
