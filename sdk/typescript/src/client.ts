/**
 * World ZK Compute TypeScript SDK.
 *
 * @example
 * ```typescript
 * import { WorldZKClient } from '@worldzk/sdk';
 *
 * const client = new WorldZKClient({ gatewayUrl: 'http://localhost:8080' });
 * const result = await client.verify(bundle);
 * console.log(result.verified);
 * ```
 */

import type {
  BatchVerifyResult,
  ClientConfig,
  HealthStatus,
  ProofBundle,
  ProveRequest,
  ProveResult,
  SearchResult,
  SubmitResult,
  VerifyResult,
} from './types';

export class WorldZKClient {
  private gatewayUrl: string;
  private verifierUrl: string;
  private registryUrl: string;
  private generatorUrl: string;
  private timeout: number;
  private apiKey?: string;

  constructor(config: ClientConfig = {}) {
    this.gatewayUrl = (config.gatewayUrl || 'http://localhost:8080').replace(/\/$/, '');
    this.verifierUrl = (config.verifierUrl || `${this.gatewayUrl}`).replace(/\/$/, '');
    this.registryUrl = (config.registryUrl || `${this.gatewayUrl}`).replace(/\/$/, '');
    this.generatorUrl = (config.generatorUrl || `${this.gatewayUrl}`).replace(/\/$/, '');
    this.timeout = config.timeout || 120000;
    this.apiKey = config.apiKey;
  }

  /** Verify a proof bundle. */
  async verify(bundle: ProofBundle): Promise<VerifyResult> {
    return this.post(`${this.verifierUrl}/verify`, bundle);
  }

  /** Batch verify multiple proof bundles. */
  async verifyBatch(bundles: ProofBundle[]): Promise<BatchVerifyResult> {
    return this.post(`${this.verifierUrl}/verify/batch`, { bundles });
  }

  /** Submit a proof to the registry. */
  async submit(bundle: ProofBundle): Promise<SubmitResult> {
    return this.post(`${this.registryUrl}/proofs`, bundle);
  }

  /** Get a proof by ID. */
  async getProof(proofId: string): Promise<Record<string, unknown>> {
    return this.get(`${this.registryUrl}/proofs/${proofId}`);
  }

  /** Search proofs. */
  async search(params: {
    circuitHash?: string;
    modelHash?: string;
    limit?: number;
  } = {}): Promise<SearchResult> {
    const query = new URLSearchParams();
    if (params.circuitHash) query.set('circuit_hash', params.circuitHash);
    if (params.modelHash) query.set('model_hash', params.modelHash);
    if (params.limit) query.set('limit', String(params.limit));
    return this.get(`${this.registryUrl}/proofs?${query}`);
  }

  /** Generate a proof. */
  async prove(modelId: string, features: number[]): Promise<ProveResult> {
    return this.post(`${this.generatorUrl}/prove`, { model_id: modelId, features });
  }

  /** Check service health. */
  async health(): Promise<HealthStatus> {
    return this.get(`${this.gatewayUrl}/health`);
  }

  private async post<T>(url: string, body: unknown): Promise<T> {
    const headers: Record<string, string> = { 'Content-Type': 'application/json' };
    if (this.apiKey) headers['Authorization'] = `Bearer ${this.apiKey}`;

    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.timeout);

    try {
      const resp = await fetch(url, {
        method: 'POST',
        headers,
        body: JSON.stringify(body),
        signal: controller.signal,
      });
      if (!resp.ok) {
        const err = await resp.json().catch(() => ({ error: resp.statusText }));
        throw new Error(`HTTP ${resp.status}: ${(err as any).error || resp.statusText}`);
      }
      return resp.json() as Promise<T>;
    } finally {
      clearTimeout(timer);
    }
  }

  private async get<T>(url: string): Promise<T> {
    const headers: Record<string, string> = {};
    if (this.apiKey) headers['Authorization'] = `Bearer ${this.apiKey}`;

    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.timeout);

    try {
      const resp = await fetch(url, { headers, signal: controller.signal });
      if (!resp.ok) {
        const err = await resp.json().catch(() => ({ error: resp.statusText }));
        throw new Error(`HTTP ${resp.status}: ${(err as any).error || resp.statusText}`);
      }
      return resp.json() as Promise<T>;
    } finally {
      clearTimeout(timer);
    }
  }
}

export * from './types';
