// Package worldzk provides response types for the World ZK Compute API.
package worldzk

// HealthResponse is the aggregated health of all backend services.
type HealthResponse struct {
	Status       string            `json:"status"`
	Services     []ServiceHealth   `json:"services,omitempty"`
	Backends     map[string]string `json:"backends,omitempty"`
	HealthyCount int               `json:"healthy_count,omitempty"`
	TotalCount   int               `json:"total_count,omitempty"`
}

// ServiceHealth describes the health of a single backend service.
type ServiceHealth struct {
	Name    string `json:"name"`
	Status  string `json:"status"`
	Latency int    `json:"latency_ms,omitempty"`
}

// ModelResponse is returned when a model is uploaded.
type ModelResponse struct {
	ModelID   string `json:"model_id"`
	ModelHash string `json:"model_hash"`
	Name      string `json:"name,omitempty"`
	Format    string `json:"format,omitempty"`
}

// ProveResponse is returned when a proof is generated.
type ProveResponse struct {
	ProofID      string  `json:"proof_id"`
	ModelID      string  `json:"model_id"`
	ModelHash    string  `json:"model_hash,omitempty"`
	CircuitHash  string  `json:"circuit_hash,omitempty"`
	Output       string  `json:"output,omitempty"`
	ProveTimeMs  uint64  `json:"prove_time_ms,omitempty"`
	ProofBundle  *Bundle `json:"proof_bundle,omitempty"`
}

// Bundle is a self-contained proof bundle.
type Bundle struct {
	ProofHex              string                 `json:"proof_hex"`
	GensHex               string                 `json:"gens_hex"`
	DAGCircuitDescription map[string]interface{} `json:"dag_circuit_description,omitempty"`
	PublicInputsHex       string                 `json:"public_inputs_hex,omitempty"`
	ModelHash             string                 `json:"model_hash,omitempty"`
	CircuitHash           string                 `json:"circuit_hash,omitempty"`
	ProverVersion         string                 `json:"prover_version,omitempty"`
	Timestamp             *uint64                `json:"timestamp,omitempty"`
}

// VerifyResponse is the result of proof verification.
type VerifyResponse struct {
	Verified  bool   `json:"verified"`
	ReceiptID string `json:"receipt_id,omitempty"`
	Error     string `json:"error,omitempty"`
}

// ReceiptResponse is a verification receipt for a proof.
type ReceiptResponse struct {
	ReceiptID  string `json:"receipt_id"`
	ProofID    string `json:"proof_id"`
	Verified   bool   `json:"verified"`
	VerifiedAt string `json:"verified_at"`
	Signature  string `json:"signature"`
}

// ProofSummary describes a single proof entry in a list response.
type ProofSummary struct {
	ProofID     string `json:"proof_id"`
	ModelID     string `json:"model_id,omitempty"`
	ModelHash   string `json:"model_hash,omitempty"`
	CircuitHash string `json:"circuit_hash,omitempty"`
	CreatedAt   string `json:"created_at,omitempty"`
	Verified    *bool  `json:"verified,omitempty"`
}

// StatsResponse contains proof registry statistics.
type StatsResponse struct {
	TotalProofs    int            `json:"total_proofs"`
	TotalModels    int            `json:"total_models"`
	TotalVerified  int            `json:"total_verified"`
	ProofsByModel  map[string]int `json:"proofs_by_model,omitempty"`
	AvgProveTimeMs float64        `json:"avg_prove_time_ms,omitempty"`
}

// APIError represents an error response from the gateway.
type APIError struct {
	StatusCode int
	Body       string
}

// Error implements the error interface.
func (e *APIError) Error() string {
	return "worldzk: HTTP " + intToStr(e.StatusCode) + ": " + e.Body
}

// intToStr converts an int to a string without importing strconv.
func intToStr(n int) string {
	if n == 0 {
		return "0"
	}
	neg := false
	if n < 0 {
		neg = true
		n = -n
	}
	digits := make([]byte, 0, 10)
	for n > 0 {
		digits = append(digits, byte('0'+n%10))
		n /= 10
	}
	if neg {
		digits = append(digits, '-')
	}
	// reverse
	for i, j := 0, len(digits)-1; i < j; i, j = i+1, j-1 {
		digits[i], digits[j] = digits[j], digits[i]
	}
	return string(digits)
}
