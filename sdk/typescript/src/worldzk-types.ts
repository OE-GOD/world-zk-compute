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

/** Parameters for uploading a model via {@link WorldZK.uploadModel}. */
export interface UploadModelParams {
  /** Serialized model JSON (e.g. from `xgb.save_model()`). */
  model_json: string;
  /** Human-readable model name. */
  name?: string;
  /** Model format hint (e.g. `"xgboost"`, `"lightgbm"`). */
  format?: string;
}

/** Parameters for listing proofs via {@link WorldZK.listProofs}. */
export interface ListProofsParams {
  /** Filter by model ID. */
  model?: string;
  /** Maximum number of results. */
  limit?: number;
}

// ============================================================================
// Response types
// ============================================================================

/** Response from `GET /health`. */
export interface HealthCheckResponse {
  status: string;
  [key: string]: unknown;
}

/** Response from `POST /v1/models`. */
export interface UploadModelResponse {
  model_id: string;
  name?: string;
  format?: string;
  circuit_hash?: string;
  created_at?: string;
  [key: string]: unknown;
}

/** A single model entry returned by `GET /v1/models`. */
export interface ModelInfo {
  model_id: string;
  name?: string;
  format?: string;
  circuit_hash?: string;
  created_at?: string;
  [key: string]: unknown;
}

/** Response from `GET /v1/models`. */
export interface ListModelsResponse {
  models: ModelInfo[];
  [key: string]: unknown;
}

/** Response from `POST /v1/prove`. */
export interface ProveResponse {
  proof_id: string;
  model_id: string;
  status: string;
  result?: number | number[];
  proof?: string;
  created_at?: string;
  [key: string]: unknown;
}

/** Response from `POST /v1/proofs/:id/verify`. */
export interface VerifyResponse {
  proof_id: string;
  verified: boolean;
  circuit_hash?: string;
  verification_time_ms?: number;
  [key: string]: unknown;
}

/** Response from `GET /v1/proofs/:id/receipt`. */
export interface ReceiptResponse {
  proof_id: string;
  seal?: string;
  journal?: string;
  image_id?: string;
  [key: string]: unknown;
}

/** A single proof entry returned by `GET /v1/proofs`. */
export interface ProofInfo {
  proof_id: string;
  model_id: string;
  status: string;
  result?: number | number[];
  created_at?: string;
  [key: string]: unknown;
}

/** Response from `GET /v1/proofs`. */
export interface ListProofsResponse {
  proofs: ProofInfo[];
  [key: string]: unknown;
}

/** Response from `GET /v1/stats`. */
export interface StatsResponse {
  total_proofs?: number;
  total_models?: number;
  total_verifications?: number;
  uptime_seconds?: number;
  [key: string]: unknown;
}
