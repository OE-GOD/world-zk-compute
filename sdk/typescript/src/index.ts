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
} from './errors';

export {
  // Enums
  RequestStatus,
  ReputationTier,
  ProofType,
  HealthStatus,
  ErrorCode,
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
