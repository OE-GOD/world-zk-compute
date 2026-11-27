/**
 * World ZK Compute SDK - ML Inference API Types
 *
 * Types for the simplified WorldZK client that wraps the /v1/ ML inference
 * endpoints (model upload, prove, verify, receipts).
 */

// ============================================================================
// Request types
// ============================================================================

/** Options for constructing a {@link WorldZK} client. */
export interface WorldZKOptions {
  /** Base URL of the inference service (default: `"http://localhost:8080"`). */
  url?: string;
  /** Optional bearer token for authentication. */
  apiKey?: string;
  /** Request timeout in milliseconds (default: 30 000). */
  timeout?: number;
}

/** Request body of `POST /v1/models` (what {@link WorldZK.uploadModel} sends). */
export interface UploadModelParams {
  /** The model file contents as a JSON value (the service rejects a string here). */
  model_json: unknown;
  /** Human-readable model name. Required by the proof-generator. */
  name: string;
  /** Model format hint (e.g. `"xgboost"`, `"lightgbm"`). Defaults to `"auto"` in the service. */
  format?: string;
}

/** Parameters for listing proofs via {@link WorldZK.listProofs}. */
export interface ListProofsParams {
  /** Filter by model hash. */
  model?: string;
  /** Maximum number of results. */
  limit?: number;
}

// ============================================================================
// Response types
// ============================================================================

/** Health of one backend, as reported by the gateway. */
export interface ServiceHealth {
  name: string;
  healthy: boolean;
  response_ms?: number;
  [key: string]: unknown;
}

/** Response from `GET /health` on the gateway. */
export interface HealthCheckResponse {
  /**
   * `"healthy"` when every backend answers, `"degraded"` when only some do,
   * `"unhealthy"` when none does.
   */
  status: string;
  services?: ServiceHealth[];
  healthy_count?: number;
  total_count?: number;
  [key: string]: unknown;
}

/** Response from `POST /v1/models`. */
export interface UploadModelResponse {
  model_id: string;
  model_hash: string;
  circuit_hash: string;
  format: string;
  active: boolean;
  [key: string]: unknown;
}

/** A single model entry returned by `GET /v1/models`. */
export interface ModelInfo {
  /** Model identifier (the `model_id` returned by the upload). */
  id: string;
  name: string;
  format: string;
  model_hash: string;
  circuit_hash: string;
  active: boolean;
  created_at: string;
  file_size_bytes?: number;
  [key: string]: unknown;
}

/** Response from `GET /v1/models`. */
export interface ListModelsResponse {
  models: ModelInfo[];
  [key: string]: unknown;
}

/** Proof bundle inside a {@link ProveResponse}. */
export interface ProofBundle {
  proof_hex: string;
  public_inputs_hex: string;
  [key: string]: unknown;
}

/** Response from `POST /v1/prove`. */
export interface ProveResponse {
  /**
   * The proof-generator's own identifier. It is NOT a proof-registry id:
   * {@link WorldZK.verify} and {@link WorldZK.getReceipt} need
   * `registry_proof_id`.
   */
  proof_id: string;
  model_hash: string;
  circuit_hash: string;
  /** Predicted output label. */
  output: string;
  /** True when the generator runs in mock mode and computed no proof. */
  mock: boolean;
  prove_time_ms: number;
  proof_bundle: ProofBundle;
  /** Id under which the proof-registry stored the proof, when it did. */
  registry_proof_id?: string;
  [key: string]: unknown;
}

/** Response from `POST /v1/proofs/:id/verify`. */
export interface VerifyResponse {
  verified: boolean;
  /** Id of the signed receipt the registry issued for this verification. */
  receipt_id: string;
  error?: string;
  [key: string]: unknown;
}

/** Response from `GET /v1/proofs/:id/receipt`: the registry's signed verification receipt. */
export interface ReceiptResponse {
  receipt_id: string;
  proof_id: string;
  circuit_hash: string;
  model_hash: string;
  verified: boolean;
  verified_at: string;
  verifier_version?: string;
  verifier_host?: string;
  /** Hex signature of the receipt by the registry's signing key. */
  signature: string;
  [key: string]: unknown;
}

/** A single proof entry returned by `GET /v1/proofs`. */
export interface ProofInfo {
  /** Proof-registry identifier. */
  id: string;
  model_hash: string;
  circuit_hash: string;
  created_at: string;
  verified?: boolean;
  verified_at?: string;
  proof_size_bytes?: number;
  deleted?: boolean;
  [key: string]: unknown;
}

/** Response from `GET /v1/proofs`. */
export interface ListProofsResponse {
  proofs: ProofInfo[];
  count: number;
  [key: string]: unknown;
}

/** Response from `GET /v1/stats` (proof-registry statistics). */
export interface StatsResponse {
  total_proofs: number;
  active_proofs?: number;
  verified_count?: number;
  failed_count?: number;
  total_storage_bytes?: number;
  [key: string]: unknown;
}
