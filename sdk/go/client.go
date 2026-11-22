// Package worldzk provides a Go client for the World ZK Compute API gateway
// (services/gateway), which forwards to the proof generator, the proof
// registry and the verifier.
//
// Usage:
//
//	client := worldzk.New("http://localhost:8080", "my-api-key")
//	health, err := client.Health()
//	model, err := client.UploadModel(modelJSON, "my-model", "xgboost")
//	result, err := client.Prove(model.ModelID, []float64{1.0, 2.0, 3.0})
//	proofs, err := client.ListProofs("", 10)
//	verification, err := client.Verify(proofs[0].ProofID)
//	receipt, err := client.GetReceipt(proofs[0].ProofID)
//
// Verify and GetReceipt take a proof registry ID: an entry of ListProofs, or
// ProveResponse.RegistryProofID when the generator stored the proof.
// ProveResponse.ProofID is the generator's own ID and is unknown to the registry.
package worldzk

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"
)

// Client communicates with the World ZK Compute API gateway.
type Client struct {
	// URL is the base URL of the API gateway (e.g. "http://localhost:8080").
	URL string
	// APIKey is an optional Bearer token for authenticated endpoints.
	APIKey string
	// HTTP is the underlying HTTP client used for all requests.
	HTTP *http.Client
}

// New creates a Client pointing at the given gateway URL.
// Pass an empty string for apiKey if authentication is not required.
func New(url string, apiKey string) *Client {
	return &Client{
		URL:    strings.TrimRight(url, "/"),
		APIKey: apiKey,
		HTTP: &http.Client{
			Timeout: 120 * time.Second,
		},
	}
}

// Health checks the gateway health status.
func (c *Client) Health() (*HealthResponse, error) {
	var resp HealthResponse
	if err := c.doJSON("GET", "/health", nil, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

// DefaultModelName is the name UploadModel sends when it is given an empty one.
const DefaultModelName = "model"

// UploadModel uploads an ML model for proof generation.
//
// modelJSON is the raw JSON content of the model file. It is sent as a JSON
// object, which is what the proof generator parses; invalid JSON is rejected
// here, before any request is made.
// name is a human-readable name. The service requires one, so an empty name is
// sent as DefaultModelName.
// format is an optional format hint such as "xgboost" (pass "" to let the
// service detect it).
func (c *Client) UploadModel(modelJSON string, name string, format string) (*ModelResponse, error) {
	if !json.Valid([]byte(modelJSON)) {
		return nil, fmt.Errorf("worldzk: model is not valid JSON")
	}
	if name == "" {
		name = DefaultModelName
	}
	body := map[string]interface{}{
		"model_json": json.RawMessage(modelJSON),
		"name":       name,
	}
	if format != "" {
		body["format"] = format
	}

	var resp ModelResponse
	if err := c.doJSON("POST", "/models", body, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

// Prove generates a proof for the given model and input features.
func (c *Client) Prove(modelID string, features []float64) (*ProveResponse, error) {
	body := map[string]interface{}{
		"model_id": modelID,
		"features": features,
	}

	var resp ProveResponse
	if err := c.doJSON("POST", "/prove", body, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

// Verify verifies a stored proof by its proof registry ID.
func (c *Client) Verify(proofID string) (*VerifyResponse, error) {
	var resp VerifyResponse
	if err := c.doJSON("POST", fmt.Sprintf("/proofs/%s/verify", url.PathEscape(proofID)), map[string]interface{}{}, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

// GetReceipt retrieves the latest verification receipt for a proof. The
// registry answers 404 until the proof has been verified once.
func (c *Client) GetReceipt(proofID string) (*ReceiptResponse, error) {
	var resp ReceiptResponse
	if err := c.doJSON("GET", fmt.Sprintf("/proofs/%s/receipt", url.PathEscape(proofID)), nil, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

// ListProofs lists stored proofs, optionally filtered by model hash (the
// registry matches the model_hash a proof was stored with).
// Pass an empty string for model to list all proofs.
// Pass 0 for limit to use the server default.
func (c *Client) ListProofs(model string, limit int) ([]ProofSummary, error) {
	query := url.Values{}
	if limit > 0 {
		query.Set("limit", strconv.Itoa(limit))
	}
	if model != "" {
		query.Set("model", model)
	}
	path := "/proofs"
	if len(query) > 0 {
		path += "?" + query.Encode()
	}

	var resp proofListResponse
	if err := c.doJSON("GET", path, nil, &resp); err != nil {
		return nil, err
	}
	return resp.Proofs, nil
}

// Stats returns proof registry statistics.
func (c *Client) Stats() (*StatsResponse, error) {
	var resp StatsResponse
	if err := c.doJSON("GET", "/stats", nil, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}

// doJSON is the internal helper that builds the HTTP request, sets headers,
// executes the request, and decodes the JSON response.
func (c *Client) doJSON(method, path string, body interface{}, out interface{}) error {
	var reqBody io.Reader
	if body != nil {
		data, err := json.Marshal(body)
		if err != nil {
			return fmt.Errorf("worldzk: marshal request: %w", err)
		}
		reqBody = bytes.NewReader(data)
	}

	req, err := http.NewRequest(method, c.URL+path, reqBody)
	if err != nil {
		return fmt.Errorf("worldzk: build request: %w", err)
	}

	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	if c.APIKey != "" {
		req.Header.Set("Authorization", "Bearer "+c.APIKey)
	}

	resp, err := c.HTTP.Do(req)
	if err != nil {
		return fmt.Errorf("worldzk: request failed: %w", err)
	}
	defer resp.Body.Close()

	respData, err := io.ReadAll(resp.Body)
	if err != nil {
		return fmt.Errorf("worldzk: read response: %w", err)
	}

	if resp.StatusCode >= 400 {
		return &APIError{
			StatusCode: resp.StatusCode,
			Body:       string(respData),
		}
	}

	if out != nil && len(respData) > 0 {
		if err := json.Unmarshal(respData, out); err != nil {
			return fmt.Errorf("worldzk: decode response: %w", err)
		}
	}

	return nil
}
