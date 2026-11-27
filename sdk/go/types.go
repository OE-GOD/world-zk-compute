// Package worldzk provides response types for the World ZK Compute API.
package worldzk

import "strconv"

// HealthResponse is the aggregated health of all backend services
// (gateway `GET /health`).
type HealthResponse struct {
	// Status is "healthy", "degraded" or "unhealthy".
	Status       string          `json:"status"`
	Services     []ServiceHealth `json:"services,omitempty"`
	HealthyCount int             `json:"healthy_count"`
	TotalCount   int             `json:"total_count"`
}

// ServiceHealth describes the health of a single backend service.
type ServiceHealth struct {
	Name    string `json:"name"`
	Healthy bool   `json:"healthy"`
	// ResponseMs is how long the backend's /health took, when it answered.
	ResponseMs uint64 `json:"response_ms,omitempty"`
	// Error is set when the backend could not be reached.
	Error string `json:"error,omitempty"`
}

// ModelResponse is returned when a model is uploaded (`POST /models`).
type ModelResponse struct {
	ModelID string `json:"model_id"`
	// ModelHash is the proof generator's identifier: SHA-256 of the model after
	// it was parsed and re-serialized. It is neither the keccak256 nor the
	// SHA-256 of the uploaded file.
	ModelHash   string `json:"model_hash"`
	CircuitHash string `json:"circuit_hash"`
	Format      string `json:"format"`
	Active      bool   `json:"active"`
}

// ProveResponse is returned when a proof is generated (`POST /prove`).
type ProveResponse struct {
	// ProofID is the proof generator's own identifier. The proof registry does
	// not know it: use RegistryProofID with Verify and GetReceipt.
	ProofID     string `json:"proof_id"`
	ModelHash   string `json:"model_hash"`
	CircuitHash string `json:"circuit_hash"`
	// Output is the predicted label.
	Output string `json:"output"`
	// Mock is true when the generator runs with --mock and the bundle is a placeholder.
	Mock        bool    `json:"mock"`
	ProveTimeMs uint64  `json:"prove_time_ms"`
	ProofBundle *Bundle `json:"proof_bundle,omitempty"`
	// RegistryProofID is the ID under which the generator stored the proof in
	// the proof registry. Empty when the generator has no registry configured
	// or the hand-off failed.
	RegistryProofID string `json:"registry_proof_id,omitempty"`
}

// Bundle is the proof data inside a ProveResponse.
type Bundle struct {
	ProofHex        string `json:"proof_hex"`
	PublicInputsHex string `json:"public_inputs_hex"`
	ProofSizeBytes  uint64 `json:"proof_size_bytes"`
}

// VerifyResponse is the result of proof verification (`POST /proofs/{id}/verify`).
type VerifyResponse struct {
	Verified  bool   `json:"verified"`
	ReceiptID string `json:"receipt_id,omitempty"`
	Error     string `json:"error,omitempty"`
}

// ReceiptResponse is a verification receipt for a proof (`GET /proofs/{id}/receipt`).
type ReceiptResponse struct {
	ReceiptID       string `json:"receipt_id"`
	ProofID         string `json:"proof_id"`
	CircuitHash     string `json:"circuit_hash"`
	ModelHash       string `json:"model_hash"`
	Verified        bool   `json:"verified"`
	VerifiedAt      string `json:"verified_at"`
	VerifierVersion string `json:"verifier_version"`
	VerifierHost    string `json:"verifier_host"`
	Signature       string `json:"signature"`
	// Error is the reason the verification failed. Empty when it succeeded.
	Error string `json:"error,omitempty"`
}

// ProofSummary describes a single stored proof (an entry of `GET /proofs`).
type ProofSummary struct {
	// ProofID is the registry's proof ID (JSON field "id").
	ProofID     string `json:"id"`
	ModelHash   string `json:"model_hash"`
	CircuitHash string `json:"circuit_hash"`
	CreatedAt   string `json:"created_at"`
	// Verified is nil until the proof has been verified once.
	Verified       *bool   `json:"verified"`
	VerifiedAt     *string `json:"verified_at"`
	ProofSizeBytes uint64  `json:"proof_size_bytes"`
	Deleted        bool    `json:"deleted"`
}

// proofListResponse is the envelope `GET /proofs` answers with.
type proofListResponse struct {
	Proofs []ProofSummary `json:"proofs"`
	Count  int            `json:"count"`
}

// StatsResponse contains proof registry statistics (`GET /stats`).
type StatsResponse struct {
	TotalProofs       int    `json:"total_proofs"`
	ActiveProofs      int    `json:"active_proofs"`
	VerifiedCount     int    `json:"verified_count"`
	FailedCount       int    `json:"failed_count"`
	TotalStorageBytes uint64 `json:"total_storage_bytes"`
}

// APIError represents an error response from the gateway.
type APIError struct {
	StatusCode int
	Body       string
}

// Error implements the error interface.
func (e *APIError) Error() string {
	return "worldzk: HTTP " + strconv.Itoa(e.StatusCode) + ": " + e.Body
}
