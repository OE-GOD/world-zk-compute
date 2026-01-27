/**
 * World ZK Compute SDK - Client
 */

import { ApiError, NetworkError, TimeoutError } from './errors';
import {
  ApiErrorResponse,
  ClaimResponse,
  ClientOptions,
  CreateRequestParams,
  ExecutionRequest,
  HealthResponse,
  HealthStatus,
  ListProgramsParams,
  ListProversParams,
  ListRequestsParams,
  Pagination,
  PaginatedResponse,
  Program,
  ProofResult,
  ProofSubmissionResponse,
  ProofType,
  Prover,
  ProverStats,
  RegisterProverParams,
  Reputation,
  ReputationTier,
  RequestStatus,
  StatusResponse,
  SubmitProofParams,
  VerifyProofParams,
  VerifyProofResponse,
  WaitOptions,
} from './types';

const DEFAULT_BASE_URL = 'https://api.worldzk.compute/v1';
const DEFAULT_TIMEOUT = 30000;
const DEFAULT_MAX_RETRIES = 3;

/**
 * Utility to hash input data
 */
async function hashInput(data: Uint8Array): Promise<string> {
  const hashBuffer = await crypto.subtle.digest('SHA-256', data);
  const hashArray = Array.from(new Uint8Array(hashBuffer));
  return '0x' + hashArray.map((b) => b.toString(16).padStart(2, '0')).join('');
}

/**
 * Convert Uint8Array to base64
 */
function toBase64(data: Uint8Array): string {
  if (typeof Buffer !== 'undefined') {
    return Buffer.from(data).toString('base64');
  }
  return btoa(String.fromCharCode(...data));
}

/**
 * Convert base64 to Uint8Array
 */
function fromBase64(data: string): Uint8Array {
  if (typeof Buffer !== 'undefined') {
    return new Uint8Array(Buffer.from(data, 'base64'));
  }
  return new Uint8Array(
    atob(data)
      .split('')
      .map((c) => c.charCodeAt(0))
  );
}

/**
 * Sleep for specified milliseconds
 */
function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * Parse API response to ExecutionRequest
 */
function parseExecutionRequest(data: Record<string, unknown>): ExecutionRequest {
  let proof: ProofResult | undefined;
  if (data.proof && typeof data.proof === 'object') {
    const p = data.proof as Record<string, unknown>;
    proof = {
      seal: p.seal as string,
      journal: p.journal as string,
      proofType: (p.proof_type as ProofType) ?? ProofType.Groth16,
      imageId: p.image_id as string | undefined,
      verified: (p.verified as boolean) ?? false,
      verifiedAt: p.verified_at ? new Date(p.verified_at as string) : undefined,
    };
  }

  return {
    id: data.id as number,
    requester: data.requester as string,
    imageId: data.image_id as string,
    inputHash: data.input_hash as string,
    status: data.status as RequestStatus,
    createdAt: new Date(data.created_at as string),
    inputUrl: data.input_url as string | undefined,
    callbackContract: data.callback_contract as string | undefined,
    tip: BigInt(data.tip as string || '0'),
    claimedBy: data.claimed_by as string | undefined,
    claimDeadline: data.claim_deadline ? new Date(data.claim_deadline as string) : undefined,
    expiresAt: data.expires_at ? new Date(data.expires_at as string) : undefined,
    completedAt: data.completed_at ? new Date(data.completed_at as string) : undefined,
    proof,
  };
}

/**
 * Parse API response to Program
 */
function parseProgram(data: Record<string, unknown>): Program {
  return {
    imageId: data.image_id as string,
    name: (data.name as string) ?? '',
    owner: (data.owner as string) ?? '',
    isActive: (data.is_active as boolean) ?? true,
    description: data.description as string | undefined,
    sourceUrl: data.source_url as string | undefined,
    registeredAt: data.registered_at ? new Date(data.registered_at as string) : undefined,
    executionCount: (data.execution_count as number) ?? 0,
  };
}

/**
 * Parse API response to Prover
 */
function parseProver(data: Record<string, unknown>): Prover {
  const rep = (data.reputation as Record<string, unknown>) ?? {};
  const stats = (data.stats as Record<string, unknown>) ?? {};

  return {
    address: data.address as string,
    reputation: {
      score: (rep.score as number) ?? 5000,
      tier: (rep.tier as ReputationTier) ?? ReputationTier.Unranked,
      isSlashed: (rep.is_slashed as boolean) ?? false,
      isBanned: (rep.is_banned as boolean) ?? false,
    },
    stats: {
      totalJobs: (stats.total_jobs as number) ?? 0,
      completedJobs: (stats.completed_jobs as number) ?? 0,
      failedJobs: (stats.failed_jobs as number) ?? 0,
      abandonedJobs: (stats.abandoned_jobs as number) ?? 0,
      avgProofTimeMs: (stats.avg_proof_time_ms as number) ?? 0,
      totalEarnings: BigInt((stats.total_earnings as string) ?? '0'),
    },
    registeredAt: data.registered_at ? new Date(data.registered_at as string) : undefined,
    lastActiveAt: data.last_active_at ? new Date(data.last_active_at as string) : undefined,
  };
}

/**
 * World ZK Compute API Client
 */
export class Client {
  private readonly baseUrl: string;
  private readonly apiKey?: string;
  private readonly timeout: number;
  private readonly maxRetries: number;

  readonly requests: RequestsClient;
  readonly programs: ProgramsClient;
  readonly provers: ProversClient;

  constructor(options: ClientOptions = {}) {
    this.baseUrl = (options.baseUrl ?? DEFAULT_BASE_URL).replace(/\/$/, '');
    this.apiKey = options.apiKey;
    this.timeout = options.timeout ?? DEFAULT_TIMEOUT;
    this.maxRetries = options.maxRetries ?? DEFAULT_MAX_RETRIES;

    this.requests = new RequestsClient(this);
    this.programs = new ProgramsClient(this);
    this.provers = new ProversClient(this);
  }

  /**
   * Make HTTP request
   */
  async request<T = Record<string, unknown>>(
    method: string,
    path: string,
    options: {
      params?: Record<string, string | number | boolean | undefined>;
      body?: Record<string, unknown>;
      retry?: boolean;
    } = {}
  ): Promise<T> {
    const { params, body, retry = true } = options;
    const attempts = retry ? this.maxRetries : 1;

    // Build URL with query params
    let url = `${this.baseUrl}${path}`;
    if (params) {
      const searchParams = new URLSearchParams();
      for (const [key, value] of Object.entries(params)) {
        if (value !== undefined) {
          searchParams.set(key, String(value));
        }
      }
      const queryString = searchParams.toString();
      if (queryString) {
        url += `?${queryString}`;
      }
    }

    // Build headers
    const headers: Record<string, string> = {
      'Content-Type': 'application/json',
      Accept: 'application/json',
      'User-Agent': 'worldzk-typescript/1.0.0',
    };
    if (this.apiKey) {
      headers['X-API-Key'] = this.apiKey;
    }

    let lastError: Error | undefined;

    for (let attempt = 0; attempt < attempts; attempt++) {
      const controller = new AbortController();
      const timeoutId = setTimeout(() => controller.abort(), this.timeout);

      try {
        const response = await fetch(url, {
          method,
          headers,
          body: body ? JSON.stringify(body) : undefined,
          signal: controller.signal,
        });

        clearTimeout(timeoutId);

        const responseData = response.headers.get('content-type')?.includes('application/json')
          ? await response.json()
          : {};

        if (!response.ok) {
          const error = ApiError.fromResponse(
            responseData as ApiErrorResponse,
            response.status
          );
          if (error.isRetryable && attempt < attempts - 1) {
            const delay = error.retryDelayMs ?? 1000;
            await sleep(delay);
            continue;
          }
          throw error;
        }

        return responseData as T;
      } catch (e) {
        clearTimeout(timeoutId);

        if (e instanceof ApiError) {
          throw e;
        }

        if (e instanceof Error) {
          if (e.name === 'AbortError') {
            lastError = new TimeoutError(`Request timed out after ${this.timeout}ms`, this.timeout);
          } else {
            lastError = new NetworkError('WZK-5000', `Network error: ${e.message}`, 0);
          }

          if (attempt < attempts - 1) {
            await sleep(1000);
            continue;
          }
        }

        throw lastError ?? e;
      }
    }

    throw lastError ?? new Error('Request failed');
  }

  /**
   * Check service health
   */
  async health(): Promise<HealthResponse> {
    const data = await this.request<Record<string, unknown>>('GET', '/health');

    const checks: Record<string, { status: 'pass' | 'warn' | 'fail'; message?: string }> = {};
    if (data.checks && typeof data.checks === 'object') {
      for (const [name, check] of Object.entries(data.checks as Record<string, unknown>)) {
        const c = check as Record<string, unknown>;
        checks[name] = {
          status: (c.status as 'pass' | 'warn' | 'fail') ?? 'fail',
          message: c.message as string | undefined,
        };
      }
    }

    return {
      status: (data.status as HealthStatus) ?? HealthStatus.Unhealthy,
      timestamp: new Date((data.timestamp as string) ?? new Date().toISOString()),
      version: data.version as string | undefined,
      checks,
    };
  }

  /**
   * Get service status
   */
  async status(): Promise<StatusResponse> {
    const data = await this.request<Record<string, unknown>>('GET', '/status');

    return {
      status: (data.status as string) ?? 'unknown',
      queueDepth: (data.queue_depth as number) ?? 0,
      activeJobs: (data.active_jobs as number) ?? 0,
      totalCompleted: (data.total_completed as number) ?? 0,
      uptimeSeconds: (data.uptime_seconds as number) ?? 0,
      proverCount: (data.prover_count as number) ?? 0,
    };
  }
}

/**
 * Client for execution request operations
 */
class RequestsClient {
  constructor(private readonly client: Client) {}

  /**
   * List execution requests
   */
  async list(params: ListRequestsParams = {}): Promise<PaginatedResponse<ExecutionRequest>> {
    const data = await this.client.request<Record<string, unknown>>('GET', '/requests', {
      params: {
        status: params.status,
        imageId: params.imageId,
        limit: params.limit ?? 20,
        offset: params.offset ?? 0,
      },
    });

    const requests = ((data.requests as unknown[]) ?? []).map((r) =>
      parseExecutionRequest(r as Record<string, unknown>)
    );

    const pag = (data.pagination as Record<string, unknown>) ?? {};
    const pagination: Pagination = {
      total: (pag.total as number) ?? 0,
      limit: (pag.limit as number) ?? 20,
      offset: (pag.offset as number) ?? 0,
      hasMore: (pag.has_more as boolean) ?? false,
    };

    return { items: requests, pagination };
  }

  /**
   * Get execution request by ID
   */
  async get(requestId: number): Promise<ExecutionRequest> {
    const data = await this.client.request<Record<string, unknown>>(
      'GET',
      `/requests/${requestId}`
    );
    return parseExecutionRequest(data);
  }

  /**
   * Create a new execution request
   */
  async create(params: CreateRequestParams): Promise<ExecutionRequest> {
    if (!params.inputData && !params.inputUrl) {
      throw new Error('Either inputData or inputUrl is required');
    }

    const body: Record<string, unknown> = {
      image_id: params.imageId,
      expiration_seconds: params.expirationSeconds ?? 3600,
    };

    if (params.inputData) {
      body.input_data = toBase64(params.inputData);
      body.input_hash = params.inputHash ?? (await hashInput(params.inputData));
    } else if (params.inputUrl) {
      body.input_url = params.inputUrl;
      if (!params.inputHash) {
        throw new Error('inputHash is required when using inputUrl');
      }
      body.input_hash = params.inputHash;
    }

    if (params.callbackContract) {
      body.callback_contract = params.callbackContract;
    }
    if (params.tip) {
      body.tip = params.tip.toString();
    }

    const data = await this.client.request<Record<string, unknown>>('POST', '/requests', { body });
    return parseExecutionRequest(data);
  }

  /**
   * Cancel a pending execution request
   */
  async cancel(requestId: number): Promise<ExecutionRequest> {
    const data = await this.client.request<Record<string, unknown>>(
      'DELETE',
      `/requests/${requestId}`
    );
    return parseExecutionRequest(data);
  }

  /**
   * Claim an execution request (prover only)
   */
  async claim(requestId: number): Promise<ClaimResponse> {
    const data = await this.client.request<Record<string, unknown>>(
      'POST',
      `/requests/${requestId}/claim`
    );

    return {
      request: parseExecutionRequest((data.request as Record<string, unknown>) ?? data),
      inputData: data.input_data as string,
      deadline: new Date(data.deadline as string),
    };
  }

  /**
   * Submit a proof for a claimed request
   */
  async submitProof(
    requestId: number,
    params: SubmitProofParams
  ): Promise<ProofSubmissionResponse> {
    const body = {
      seal: toBase64(params.seal),
      journal: toBase64(params.journal),
    };

    const data = await this.client.request<Record<string, unknown>>(
      'POST',
      `/requests/${requestId}/proof`,
      { body }
    );

    return {
      requestId: data.request_id as number,
      status: data.status as 'verified' | 'rejected',
      txHash: data.tx_hash as string | undefined,
      payout: BigInt((data.payout as string) ?? '0'),
      verificationTimeMs: (data.verification_time_ms as number) ?? 0,
    };
  }

  /**
   * Wait for a request to complete
   */
  async wait(requestId: number, options: WaitOptions = {}): Promise<ExecutionRequest> {
    const timeout = options.timeout ?? 300000;
    const pollInterval = options.pollInterval ?? 2000;
    const startTime = Date.now();

    while (true) {
      const request = await this.get(requestId);

      if (
        request.status === RequestStatus.Completed ||
        request.status === RequestStatus.Expired ||
        request.status === RequestStatus.Cancelled
      ) {
        return request;
      }

      const elapsed = Date.now() - startTime;
      if (elapsed >= timeout) {
        throw new TimeoutError(
          `Request ${requestId} did not complete within ${timeout}ms`,
          timeout
        );
      }

      await sleep(pollInterval);
    }
  }
}

/**
 * Client for program operations
 */
class ProgramsClient {
  constructor(private readonly client: Client) {}

  /**
   * List registered programs
   */
  async list(params: ListProgramsParams = {}): Promise<PaginatedResponse<Program>> {
    const data = await this.client.request<Record<string, unknown>>('GET', '/programs', {
      params: {
        active: params.active,
        limit: params.limit ?? 20,
        offset: params.offset ?? 0,
      },
    });

    const programs = ((data.programs as unknown[]) ?? []).map((p) =>
      parseProgram(p as Record<string, unknown>)
    );

    const pag = (data.pagination as Record<string, unknown>) ?? {};
    const pagination: Pagination = {
      total: (pag.total as number) ?? 0,
      limit: (pag.limit as number) ?? 20,
      offset: (pag.offset as number) ?? 0,
      hasMore: (pag.has_more as boolean) ?? false,
    };

    return { items: programs, pagination };
  }

  /**
   * Get program by image ID
   */
  async get(imageId: string): Promise<Program> {
    const data = await this.client.request<Record<string, unknown>>('GET', `/programs/${imageId}`);
    return parseProgram(data);
  }
}

/**
 * Client for prover operations
 */
class ProversClient {
  constructor(private readonly client: Client) {}

  /**
   * List registered provers
   */
  async list(params: ListProversParams = {}): Promise<PaginatedResponse<Prover>> {
    const data = await this.client.request<Record<string, unknown>>('GET', '/provers', {
      params: {
        tier: params.tier,
        limit: params.limit ?? 20,
        offset: params.offset ?? 0,
      },
    });

    const provers = ((data.provers as unknown[]) ?? []).map((p) =>
      parseProver(p as Record<string, unknown>)
    );

    const pag = (data.pagination as Record<string, unknown>) ?? {};
    const pagination: Pagination = {
      total: (pag.total as number) ?? 0,
      limit: (pag.limit as number) ?? 20,
      offset: (pag.offset as number) ?? 0,
      hasMore: (pag.has_more as boolean) ?? false,
    };

    return { items: provers, pagination };
  }

  /**
   * Get prover by address
   */
  async get(address: string): Promise<Prover> {
    const data = await this.client.request<Record<string, unknown>>('GET', `/provers/${address}`);
    return parseProver(data);
  }

  /**
   * Register as a prover
   */
  async register(params: RegisterProverParams = {}): Promise<Prover> {
    const body: Record<string, unknown> = {};
    if (params.name) body.name = params.name;
    if (params.endpoint) body.endpoint = params.endpoint;

    const data = await this.client.request<Record<string, unknown>>('POST', '/provers/register', {
      body,
    });
    return parseProver(data);
  }
}
