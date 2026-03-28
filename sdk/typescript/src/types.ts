/** Proof bundle for verification. */
export interface ProofBundle {
  proof_hex: string;
  gens_hex: string;
  dag_circuit_description: Record<string, unknown>;
  public_inputs_hex?: string;
  model_hash?: string;
  timestamp?: number;
  prover_version?: string;
  circuit_hash?: string;
}

/** Verification result. */
export interface VerifyResult {
  verified: boolean;
  circuit_hash?: string;
  error?: string;
}

/** Batch verification result. */
export interface BatchVerifyResult {
  results: Array<{
    index: number;
    verified: boolean;
    circuit_hash: string;
    error?: string;
  }>;
  total: number;
  valid: number;
}

/** Proof submission result. */
export interface SubmitResult {
  id: string;
  content_hash: string;
}

/** Proof search result. */
export interface SearchResult {
  proofs: Array<{
    id: string;
    circuit_hash?: string;
    model_hash?: string;
    submitted_at: number;
    verified?: boolean;
    content_hash: string;
  }>;
  total: number;
}

/** Prove request. */
export interface ProveRequest {
  model_id: string;
  features: number[];
}

/** Prove result. */
export interface ProveResult {
  proof_id: string;
  model_id: string;
  predicted_class: number;
  proof_bundle: ProofBundle;
  prove_time_ms: number;
}

/** Health status. */
export interface HealthStatus {
  status: string;
  backends?: Record<string, string>;
}

/** Client configuration. */
export interface ClientConfig {
  gatewayUrl?: string;
  verifierUrl?: string;
  registryUrl?: string;
  generatorUrl?: string;
  timeout?: number;
  apiKey?: string;
}
