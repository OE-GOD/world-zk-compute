/**
 * World ZK Compute TypeScript SDK
 *
 * A TypeScript client library for the World ZK Compute decentralized proving network.
 *
 * @example
 * ```typescript
 * import { Client } from '@worldzk/sdk';
 *
 * const client = new Client({ apiKey: 'your-api-key' });
 *
 * // Submit a computation request
 * const request = await client.requests.create({
 *   imageId: '0x1234...',
 *   inputData: new TextEncoder().encode('hello world'),
 * });
 *
 * // Wait for completion
 * const result = await client.requests.wait(request.id);
 * console.log('Proof:', result.proof);
 * ```
 */

export { Client } from './client';
export { BatchVerifier, DAGBatchVerifier, ProgressTracker, BatchVerificationCancelledError } from './batch-verifier';
export type { DAGBatchSession } from './batch-verifier';
export { TEEVerifier } from './tee-verifier';
export type { TEEVerifierConfig } from './tee-verifier';
export { TEEEventWatcher } from './tee-event-watcher';
export type {
  TEEEventWatcherConfig,
  TEEEventName,
  TEEEventData,
  ResultSubmittedEvent,
  ResultChallengedEvent,
  ResultFinalizedEvent,
  ResultExpiredEvent,
  DisputeResolvedEvent,
  EnclaveRegisteredEvent,
  EnclaveRevokedEvent,
  WatchEventsOptions,
  WatchEventsCallback,
  WatchEventsErrorCallback,
  EventSubscription,
} from './tee-event-watcher';
export { teeMLVerifierAbi } from './tee-verifier-abi';
export {
  teeMLVerifierAbi as generatedTeeMLVerifierAbi,
  executionEngineAbi,
  programRegistryAbi,
  remainderVerifierAbi,
  proverRegistryAbi,
  proverReputationAbi,
} from './abi';
export {
  computeModelHash,
  computeInputHash,
  computeResultHash,
  computeInputHashFromJson,
  computeResultHashFromBytes,
  serializeF64List,
} from './hash';

export {
  WorldZKError,
  ApiError,
  ValidationError,
  AuthenticationError,
  NotFoundError,
  RateLimitError,
  ProofError,
  NetworkError,
  TimeoutError,
  ServiceHealthError,
} from './errors';

export {
  // Enums
  RequestStatus,
  ReputationTier,
  ProofType,
  HealthStatus,
  ErrorCode,
} from './types';

export type {
  // Interfaces
  ClientOptions,
  Pagination,
  PaginatedResponse,
  RetryInfo,
  ApiErrorResponse,
  ProofResult,
  ExecutionRequest,
  CreateRequestParams,
  ListRequestsParams,
  Program,
  ListProgramsParams,
  Reputation,
  ProverStats,
  Prover,
  ListProversParams,
  RegisterProverParams,
  HealthCheck,
  HealthResponse,
  StatusResponse,
  ClaimResponse,
  SubmitProofParams,
  ProofSubmissionResponse,
  VerifyProofParams,
  VerifyProofResponse,
  WaitOptions,
} from './types';

export {
  type Hex as BatchHex,
  type BatchVerifierConfig,
  type BatchVerifyInput,
  type BatchVerifyResult,
  type BatchSession,
  type StepResult,
  type ProgressEvent,
  type ProgressCallback,
  type BatchVerifyOptions,
} from './batch-verifier-types';

export {
  LightGBMConverter,
  ModelParseError,
  InputValidationError,
} from './lightgbm';
export type {
  TreeNode,
  LightGBMModel,
} from './lightgbm';

export { ExecutionEngineClient } from './execution-engine';
export {
  OnChainRequestStatus,
} from './execution-engine';
export type {
  OnChainExecutionRequest,
  SubmitRequestOptions,
  OnChainProverStats,
} from './execution-engine';

export { checkHealth } from './health';
export type {
  IndexerHealthResponse,
  CheckHealthOptions,
} from './health';

export { SEPOLIA_NETWORK, ANVIL_NETWORK, NETWORKS } from './networks';
export type { NetworkConfig } from './networks';
