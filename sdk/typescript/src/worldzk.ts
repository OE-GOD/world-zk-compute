/**
 * World ZK Compute SDK - Simplified ML Inference Client
 *
 * High-level client for the ML inference API. Designed so Node.js developers
 * can integrate in three lines:
 *
 * ```typescript
 * import { WorldZK } from '@worldzk/sdk';
 * const zk = new WorldZK('http://localhost:8080', 'my-api-key');
 * const proof = await zk.prove('model-abc', [1.0, 2.0, 3.0]);
 * ```
 *
 * For the full-featured proving-network client (requests, provers, programs),
 * use {@link Client} instead.
 */

import type {
  WorldZKOptions,
  UploadModelParams,
  ListProofsParams,
  HealthCheckResponse,
  UploadModelResponse,
  ListModelsResponse,
  ProveResponse,
  VerifyResponse,
  ReceiptResponse,
  ListProofsResponse,
  StatsResponse,
} from './worldzk-types';

const DEFAULT_URL = 'http://localhost:8080';
const DEFAULT_TIMEOUT = 30_000;

/**
 * Simplified client for the World ZK Compute ML inference API.
 *
 * Wraps `/v1/models`, `/v1/prove`, `/v1/proofs`, and `/v1/stats` endpoints
 * behind a small, ergonomic surface.
 */
export class WorldZK {
  private readonly url: string;
  private readonly apiKey?: string;
  private readonly timeout: number;

  /**
   * @param urlOrOptions - Either a URL string or a {@link WorldZKOptions} object.
   * @param apiKey       - Optional bearer token (ignored when first arg is an options object).
   */
  constructor(urlOrOptions?: string | WorldZKOptions, apiKey?: string) {
    if (typeof urlOrOptions === 'object' && urlOrOptions !== null) {
      this.url = (urlOrOptions.url ?? DEFAULT_URL).replace(/\/$/, '');
      this.apiKey = urlOrOptions.apiKey;
      this.timeout = urlOrOptions.timeout ?? DEFAULT_TIMEOUT;
    } else {
      this.url = (urlOrOptions ?? DEFAULT_URL).replace(/\/$/, '');
      this.apiKey = apiKey;
      this.timeout = DEFAULT_TIMEOUT;
    }
  }

  // --------------------------------------------------------------------------
  // Internal helpers
  // --------------------------------------------------------------------------

  /**
   * Generic HTTP helper. Throws on non-2xx responses.
   * @internal
   */
  private async request<T = Record<string, unknown>>(
    method: string,
    path: string,
    body?: Record<string, unknown>,
  ): Promise<T> {
    const headers: Record<string, string> = {
      'Content-Type': 'application/json',
      'Accept': 'application/json',
    };
    if (this.apiKey) {
      headers['Authorization'] = `Bearer ${this.apiKey}`;
    }

    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), this.timeout);

    try {
      const resp = await fetch(`${this.url}${path}`, {
        method,
        headers,
        body: body ? JSON.stringify(body) : undefined,
        signal: controller.signal,
      });

      if (!resp.ok) {
        const text = await resp.text().catch(() => '');
        throw new Error(`WorldZK API error ${resp.status}: ${text}`);
      }

      return (await resp.json()) as T;
    } catch (err) {
      if (err instanceof Error && err.name === 'AbortError') {
        throw new Error(`WorldZK request timed out after ${this.timeout}ms`);
      }
      throw err;
    } finally {
      clearTimeout(timeoutId);
    }
  }

  // --------------------------------------------------------------------------
  // Public API
  // --------------------------------------------------------------------------

  /**
   * Health check.
   *
   * ```typescript
   * const h = await zk.health();
   * console.log(h.status); // "ok"
   * ```
   */
  async health(): Promise<HealthCheckResponse> {
    return this.request<HealthCheckResponse>('GET', '/health');
  }

  /**
   * Upload an ML model for ZK proving.
   *
   * @param modelJson - Serialized model JSON string.
   * @param name      - Optional human-readable name.
   * @param format    - Optional format hint (`"xgboost"`, `"lightgbm"`, etc.).
   *
   * ```typescript
   * const model = await zk.uploadModel(fs.readFileSync('model.json', 'utf-8'), 'my-model', 'xgboost');
   * console.log(model.model_id);
   * ```
   */
  async uploadModel(
    modelJson: string,
    name?: string,
    format?: string,
  ): Promise<UploadModelResponse> {
    const body: UploadModelParams = { model_json: modelJson };
    if (name !== undefined) body.name = name;
    if (format !== undefined) body.format = format;
    return this.request<UploadModelResponse>('POST', '/v1/models', body as unknown as Record<string, unknown>);
  }

  /**
   * List registered models.
   *
   * ```typescript
   * const { models } = await zk.listModels();
   * ```
   */
  async listModels(): Promise<ListModelsResponse> {
    return this.request<ListModelsResponse>('GET', '/v1/models');
  }

  /**
   * Run ZK inference on a model and produce a proof.
   *
   * @param modelId  - The model identifier (from {@link uploadModel}).
   * @param features - Numeric feature vector for inference.
   *
   * ```typescript
   * const result = await zk.prove('model-abc', [1.0, 2.5, 3.7]);
   * console.log(result.proof_id, result.result);
   * ```
   */
  async prove(modelId: string, features: number[]): Promise<ProveResponse> {
    return this.request<ProveResponse>('POST', '/v1/prove', {
      model_id: modelId,
      features,
    });
  }

  /**
   * Verify a previously generated proof.
   *
   * @param proofId - The proof identifier (from {@link prove}).
   *
   * ```typescript
   * const v = await zk.verify('proof-xyz');
   * console.log(v.verified); // true
   * ```
   */
  async verify(proofId: string): Promise<VerifyResponse> {
    return this.request<VerifyResponse>('POST', `/v1/proofs/${proofId}/verify`);
  }

  /**
   * Retrieve the raw receipt (seal + journal) for a proof.
   *
   * @param proofId - The proof identifier.
   *
   * ```typescript
   * const receipt = await zk.getReceipt('proof-xyz');
   * console.log(receipt.seal, receipt.journal);
   * ```
   */
  async getReceipt(proofId: string): Promise<ReceiptResponse> {
    return this.request<ReceiptResponse>('GET', `/v1/proofs/${proofId}/receipt`);
  }

  /**
   * List proofs with optional filters.
   *
   * @param opts - Optional filters: `model` (model ID) and `limit`.
   *
   * ```typescript
   * const { proofs } = await zk.listProofs({ model: 'model-abc', limit: 10 });
   * ```
   */
  async listProofs(opts?: ListProofsParams): Promise<ListProofsResponse> {
    const params = new URLSearchParams();
    if (opts?.model) params.set('model', opts.model);
    if (opts?.limit) params.set('limit', String(opts.limit));
    const qs = params.toString();
    const path = qs ? `/v1/proofs?${qs}` : '/v1/proofs';
    return this.request<ListProofsResponse>('GET', path);
  }

  /**
   * Get service statistics.
   *
   * ```typescript
   * const stats = await zk.stats();
   * console.log(stats.total_proofs);
   * ```
   */
  async stats(): Promise<StatsResponse> {
    return this.request<StatsResponse>('GET', '/v1/stats');
  }
}
