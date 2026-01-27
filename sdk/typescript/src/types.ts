/**
 * World ZK Compute SDK - TypeScript Types
 */

// ============================================================================
// Enums
// ============================================================================

export enum RequestStatus {
  Pending = 'pending',
  Claimed = 'claimed',
  Completed = 'completed',
  Expired = 'expired',
  Cancelled = 'cancelled',
}

export enum ReputationTier {
  Unranked = 'unranked',
  Bronze = 'bronze',
  Silver = 'silver',
  Gold = 'gold',
  Platinum = 'platinum',
  Diamond = 'diamond',
}

export enum ProofType {
  Groth16 = 'groth16',
  Succinct = 'succinct',
  Fake = 'fake',
}

export enum HealthStatus {
  Healthy = 'healthy',
  Degraded = 'degraded',
  Unhealthy = 'unhealthy',
}

// ============================================================================
// Error Codes
// ============================================================================

export enum ErrorCode {
  // 1xxx - Client Errors
  InvalidRequest = 'WZK-1000',
  InvalidImageId = 'WZK-1001',
  InvalidInputHash = 'WZK-1002',
  InvalidInputUrl = 'WZK-1003',
  InvalidRequestId = 'WZK-1004',
  InvalidProof = 'WZK-1005',
  InvalidSignature = 'WZK-1006',
  InputTooLarge = 'WZK-1010',
  InputTooSmall = 'WZK-1011',
  RequestExpired = 'WZK-1020',
  RequestNotFound = 'WZK-1021',
  RequestAlreadyClaimed = 'WZK-1022',
  RequestNotClaimed = 'WZK-1023',
  RequestAlreadyCompleted = 'WZK-1024',
  Unauthorized = 'WZK-1030',
  Forbidden = 'WZK-1031',
  RateLimited = 'WZK-1040',
  QuotaExceeded = 'WZK-1041',

  // 2xxx - Server Errors
  InternalError = 'WZK-2000',
  DatabaseError = 'WZK-2001',
  CacheError = 'WZK-2002',
  ConfigurationError = 'WZK-2003',
  ServiceUnavailable = 'WZK-2010',
  ServiceOverloaded = 'WZK-2011',
  MaintenanceMode = 'WZK-2012',
  ShuttingDown = 'WZK-2013',

  // 3xxx - Proof Errors
  ProofGenerationFailed = 'WZK-3000',
  ProofVerificationFailed = 'WZK-3001',
  ProofTimeout = 'WZK-3002',
  ProofCancelled = 'WZK-3003',
  GuestProgramError = 'WZK-3010',
  GuestProgramPanic = 'WZK-3011',
  GuestProgramOom = 'WZK-3012',
  GuestProgramTimeout = 'WZK-3013',
  ImageNotFound = 'WZK-3020',
  ImageNotRegistered = 'WZK-3021',
  ImageInactive = 'WZK-3022',
  BonsaiError = 'WZK-3030',
  BonsaiTimeout = 'WZK-3031',
  BonsaiQuotaExceeded = 'WZK-3032',

  // 4xxx - Contract Errors
  ContractError = 'WZK-4000',
  TransactionFailed = 'WZK-4001',
  TransactionReverted = 'WZK-4002',
  InsufficientFunds = 'WZK-4003',
  GasEstimationFailed = 'WZK-4004',
  NonceError = 'WZK-4005',
  ChainUnavailable = 'WZK-4010',
  ChainReorg = 'WZK-4011',
  WrongChain = 'WZK-4012',

  // 5xxx - Network Errors
  NetworkError = 'WZK-5000',
  ConnectionTimeout = 'WZK-5001',
  ConnectionRefused = 'WZK-5002',
  DnsError = 'WZK-5003',
  TlsError = 'WZK-5004',
  IpfsError = 'WZK-5010',
  IpfsFetchFailed = 'WZK-5011',
  IpfsUploadFailed = 'WZK-5012',
  IpfsTimeout = 'WZK-5013',
  ExternalServiceError = 'WZK-5020',
}

// ============================================================================
// Interfaces
// ============================================================================

export interface ClientOptions {
  baseUrl?: string;
  apiKey?: string;
  timeout?: number;
  maxRetries?: number;
}

export interface Pagination {
  total: number;
  limit: number;
  offset: number;
  hasMore: boolean;
}

export interface PaginatedResponse<T> {
  items: T[];
  pagination: Pagination;
}

export interface RetryInfo {
  retryable: boolean;
  retryAfterMs?: number;
  maxRetries?: number;
}

export interface ApiErrorResponse {
  code: string;
  message: string;
  details?: Record<string, unknown>;
  requestId?: string;
  traceId?: string;
  retry?: RetryInfo;
}

export interface ProofResult {
  seal: string; // base64
  journal: string; // base64
  proofType: ProofType;
  imageId?: string;
  verified: boolean;
  verifiedAt?: Date;
}

export interface ExecutionRequest {
  id: number;
  requester: string;
  imageId: string;
  inputHash: string;
  status: RequestStatus;
  createdAt: Date;
  inputUrl?: string;
  callbackContract?: string;
  tip: bigint;
  claimedBy?: string;
  claimDeadline?: Date;
  expiresAt?: Date;
  completedAt?: Date;
  proof?: ProofResult;
}

export interface CreateRequestParams {
  imageId: string;
  inputData?: Uint8Array;
  inputUrl?: string;
  inputHash?: string;
  callbackContract?: string;
  expirationSeconds?: number;
  tip?: bigint;
}

export interface ListRequestsParams {
  status?: RequestStatus;
  imageId?: string;
  limit?: number;
  offset?: number;
}

export interface Program {
  imageId: string;
  name: string;
  owner: string;
  isActive: boolean;
  description?: string;
  sourceUrl?: string;
  registeredAt?: Date;
  executionCount: number;
}

export interface ListProgramsParams {
  active?: boolean;
  limit?: number;
  offset?: number;
}

export interface Reputation {
  score: number; // 0-10000 basis points
  tier: ReputationTier;
  isSlashed: boolean;
  isBanned: boolean;
}

export interface ProverStats {
  totalJobs: number;
  completedJobs: number;
  failedJobs: number;
  abandonedJobs: number;
  avgProofTimeMs: number;
  totalEarnings: bigint;
}

export interface Prover {
  address: string;
  reputation: Reputation;
  stats: ProverStats;
  registeredAt?: Date;
  lastActiveAt?: Date;
}

export interface ListProversParams {
  tier?: ReputationTier;
  limit?: number;
  offset?: number;
}

export interface RegisterProverParams {
  name?: string;
  endpoint?: string;
}

export interface HealthCheck {
  status: 'pass' | 'warn' | 'fail';
  message?: string;
}

export interface HealthResponse {
  status: HealthStatus;
  timestamp: Date;
  version?: string;
  checks: Record<string, HealthCheck>;
}

export interface StatusResponse {
  status: string;
  queueDepth: number;
  activeJobs: number;
  totalCompleted: number;
  uptimeSeconds: number;
  proverCount: number;
}

export interface ClaimResponse {
  request: ExecutionRequest;
  inputData: string; // base64
  deadline: Date;
}

export interface SubmitProofParams {
  seal: Uint8Array;
  journal: Uint8Array;
}

export interface ProofSubmissionResponse {
  requestId: number;
  status: 'verified' | 'rejected';
  txHash?: string;
  payout: bigint;
  verificationTimeMs: number;
}

export interface VerifyProofParams {
  imageId: string;
  seal: Uint8Array;
  journal: Uint8Array;
}

export interface VerifyProofResponse {
  valid: boolean;
  imageId: string;
  journalHash: string;
  verificationTimeMs: number;
  error?: string;
}

export interface WaitOptions {
  timeout?: number; // ms
  pollInterval?: number; // ms
}
