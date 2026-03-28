package worldzk

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

// newTestServer creates an httptest.Server that routes to a handler map.
func newTestServer(routes map[string]http.HandlerFunc) *httptest.Server {
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Try exact path match first, then path without query.
		key := r.Method + " " + r.URL.Path
		if handler, ok := routes[key]; ok {
			handler(w, r)
			return
		}
		http.NotFound(w, r)
	}))
}

func writeJSON(w http.ResponseWriter, v interface{}) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(v)
}

// --- Health ---

func TestHealth(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"GET /health": func(w http.ResponseWriter, r *http.Request) {
			writeJSON(w, HealthResponse{
				Status:       "healthy",
				HealthyCount: 3,
				TotalCount:   3,
			})
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	resp, err := c.Health()
	if err != nil {
		t.Fatalf("Health() error: %v", err)
	}
	if resp.Status != "healthy" {
		t.Errorf("expected status 'healthy', got %q", resp.Status)
	}
	if resp.HealthyCount != 3 {
		t.Errorf("expected healthy_count 3, got %d", resp.HealthyCount)
	}
}

// --- UploadModel ---

func TestUploadModel(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"POST /models": func(w http.ResponseWriter, r *http.Request) {
			body, _ := io.ReadAll(r.Body)
			var req map[string]interface{}
			json.Unmarshal(body, &req)

			if req["model_json"] == nil {
				http.Error(w, "missing model_json", 400)
				return
			}
			if req["name"] != "test-model" {
				http.Error(w, "unexpected name", 400)
				return
			}
			if req["format"] != "xgboost" {
				http.Error(w, "unexpected format", 400)
				return
			}

			writeJSON(w, ModelResponse{
				ModelID:   "model-123",
				ModelHash: "0xabc",
				Name:      "test-model",
				Format:    "xgboost",
			})
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	resp, err := c.UploadModel(`{"trees":[]}`, "test-model", "xgboost")
	if err != nil {
		t.Fatalf("UploadModel() error: %v", err)
	}
	if resp.ModelID != "model-123" {
		t.Errorf("expected model_id 'model-123', got %q", resp.ModelID)
	}
	if resp.ModelHash != "0xabc" {
		t.Errorf("expected model_hash '0xabc', got %q", resp.ModelHash)
	}
}

func TestUploadModelOptionalFields(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"POST /models": func(w http.ResponseWriter, r *http.Request) {
			body, _ := io.ReadAll(r.Body)
			var req map[string]interface{}
			json.Unmarshal(body, &req)

			// name and format should be omitted when empty
			if _, ok := req["name"]; ok {
				http.Error(w, "name should be omitted when empty", 400)
				return
			}
			if _, ok := req["format"]; ok {
				http.Error(w, "format should be omitted when empty", 400)
				return
			}

			writeJSON(w, ModelResponse{
				ModelID:   "model-456",
				ModelHash: "0xdef",
			})
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	resp, err := c.UploadModel(`{"trees":[]}`, "", "")
	if err != nil {
		t.Fatalf("UploadModel() error: %v", err)
	}
	if resp.ModelID != "model-456" {
		t.Errorf("expected model_id 'model-456', got %q", resp.ModelID)
	}
}

// --- Prove ---

func TestProve(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"POST /prove": func(w http.ResponseWriter, r *http.Request) {
			body, _ := io.ReadAll(r.Body)
			var req map[string]interface{}
			json.Unmarshal(body, &req)

			if req["model_id"] != "model-123" {
				http.Error(w, "unexpected model_id", 400)
				return
			}
			features, ok := req["features"].([]interface{})
			if !ok || len(features) != 3 {
				http.Error(w, "expected 3 features", 400)
				return
			}

			writeJSON(w, ProveResponse{
				ProofID:     "proof-abc",
				ModelID:     "model-123",
				Output:      "1",
				ProveTimeMs: 1234,
			})
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	resp, err := c.Prove("model-123", []float64{1.0, 2.0, 3.0})
	if err != nil {
		t.Fatalf("Prove() error: %v", err)
	}
	if resp.ProofID != "proof-abc" {
		t.Errorf("expected proof_id 'proof-abc', got %q", resp.ProofID)
	}
	if resp.ProveTimeMs != 1234 {
		t.Errorf("expected prove_time_ms 1234, got %d", resp.ProveTimeMs)
	}
}

// --- Verify ---

func TestVerify(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"POST /proofs/proof-abc/verify": func(w http.ResponseWriter, r *http.Request) {
			writeJSON(w, VerifyResponse{
				Verified:  true,
				ReceiptID: "rcpt-001",
			})
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	resp, err := c.Verify("proof-abc")
	if err != nil {
		t.Fatalf("Verify() error: %v", err)
	}
	if !resp.Verified {
		t.Error("expected verified=true")
	}
	if resp.ReceiptID != "rcpt-001" {
		t.Errorf("expected receipt_id 'rcpt-001', got %q", resp.ReceiptID)
	}
}

// --- GetReceipt ---

func TestGetReceipt(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"GET /proofs/proof-abc/receipt": func(w http.ResponseWriter, r *http.Request) {
			writeJSON(w, ReceiptResponse{
				ReceiptID:  "rcpt-001",
				ProofID:    "proof-abc",
				Verified:   true,
				VerifiedAt: "2026-03-28T12:00:00Z",
				Signature:  "0xsig",
			})
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	resp, err := c.GetReceipt("proof-abc")
	if err != nil {
		t.Fatalf("GetReceipt() error: %v", err)
	}
	if resp.ReceiptID != "rcpt-001" {
		t.Errorf("expected receipt_id 'rcpt-001', got %q", resp.ReceiptID)
	}
	if !resp.Verified {
		t.Error("expected verified=true")
	}
	if resp.Signature != "0xsig" {
		t.Errorf("expected signature '0xsig', got %q", resp.Signature)
	}
}

// --- ListProofs ---

func TestListProofs(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"GET /proofs": func(w http.ResponseWriter, r *http.Request) {
			query := r.URL.Query()
			if query.Get("model") != "model-123" {
				http.Error(w, "expected model=model-123", 400)
				return
			}
			if query.Get("limit") != "10" {
				http.Error(w, "expected limit=10", 400)
				return
			}

			writeJSON(w, []ProofSummary{
				{ProofID: "proof-1", ModelID: "model-123"},
				{ProofID: "proof-2", ModelID: "model-123"},
			})
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	proofs, err := c.ListProofs("model-123", 10)
	if err != nil {
		t.Fatalf("ListProofs() error: %v", err)
	}
	if len(proofs) != 2 {
		t.Fatalf("expected 2 proofs, got %d", len(proofs))
	}
	if proofs[0].ProofID != "proof-1" {
		t.Errorf("expected proof_id 'proof-1', got %q", proofs[0].ProofID)
	}
}

func TestListProofsNoFilter(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"GET /proofs": func(w http.ResponseWriter, r *http.Request) {
			query := r.URL.Query()
			if query.Get("model") != "" {
				http.Error(w, "model should not be set", 400)
				return
			}
			writeJSON(w, []ProofSummary{})
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	proofs, err := c.ListProofs("", 0)
	if err != nil {
		t.Fatalf("ListProofs() error: %v", err)
	}
	if len(proofs) != 0 {
		t.Errorf("expected 0 proofs, got %d", len(proofs))
	}
}

// --- Stats ---

func TestStats(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"GET /stats": func(w http.ResponseWriter, r *http.Request) {
			writeJSON(w, StatsResponse{
				TotalProofs:   100,
				TotalModels:   5,
				TotalVerified: 95,
			})
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	resp, err := c.Stats()
	if err != nil {
		t.Fatalf("Stats() error: %v", err)
	}
	if resp.TotalProofs != 100 {
		t.Errorf("expected total_proofs 100, got %d", resp.TotalProofs)
	}
	if resp.TotalModels != 5 {
		t.Errorf("expected total_models 5, got %d", resp.TotalModels)
	}
}

// --- Auth header ---

func TestAuthHeader(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"GET /health": func(w http.ResponseWriter, r *http.Request) {
			auth := r.Header.Get("Authorization")
			if auth != "Bearer secret-key" {
				http.Error(w, "missing or wrong auth: "+auth, 401)
				return
			}
			writeJSON(w, HealthResponse{Status: "healthy"})
		},
	})
	defer srv.Close()

	c := New(srv.URL, "secret-key")
	resp, err := c.Health()
	if err != nil {
		t.Fatalf("Health() with auth error: %v", err)
	}
	if resp.Status != "healthy" {
		t.Errorf("expected status 'healthy', got %q", resp.Status)
	}
}

func TestNoAuthHeaderWhenEmpty(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"GET /health": func(w http.ResponseWriter, r *http.Request) {
			auth := r.Header.Get("Authorization")
			if auth != "" {
				http.Error(w, "unexpected auth header: "+auth, 400)
				return
			}
			writeJSON(w, HealthResponse{Status: "healthy"})
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	_, err := c.Health()
	if err != nil {
		t.Fatalf("Health() without auth error: %v", err)
	}
}

// --- Error handling ---

func TestHTTPError(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"GET /health": func(w http.ResponseWriter, r *http.Request) {
			http.Error(w, `{"error":"unauthorized"}`, 401)
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	_, err := c.Health()
	if err == nil {
		t.Fatal("expected error for 401 response")
	}

	apiErr, ok := err.(*APIError)
	if !ok {
		t.Fatalf("expected *APIError, got %T: %v", err, err)
	}
	if apiErr.StatusCode != 401 {
		t.Errorf("expected status 401, got %d", apiErr.StatusCode)
	}
	if !strings.Contains(apiErr.Body, "unauthorized") {
		t.Errorf("expected body to contain 'unauthorized', got %q", apiErr.Body)
	}
}

func TestHTTPNotFound(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{})
	defer srv.Close()

	c := New(srv.URL, "")
	_, err := c.Health()
	if err == nil {
		t.Fatal("expected error for 404 response")
	}
	apiErr, ok := err.(*APIError)
	if !ok {
		t.Fatalf("expected *APIError, got %T: %v", err, err)
	}
	if apiErr.StatusCode != 404 {
		t.Errorf("expected status 404, got %d", apiErr.StatusCode)
	}
}

func TestServerError(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"POST /prove": func(w http.ResponseWriter, r *http.Request) {
			http.Error(w, `{"error":"internal server error"}`, 500)
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	_, err := c.Prove("model-123", []float64{1.0})
	if err == nil {
		t.Fatal("expected error for 500 response")
	}
	apiErr, ok := err.(*APIError)
	if !ok {
		t.Fatalf("expected *APIError, got %T: %v", err, err)
	}
	if apiErr.StatusCode != 500 {
		t.Errorf("expected status 500, got %d", apiErr.StatusCode)
	}
}

// --- Content-Type header ---

func TestContentTypeForPOST(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"POST /prove": func(w http.ResponseWriter, r *http.Request) {
			ct := r.Header.Get("Content-Type")
			if ct != "application/json" {
				http.Error(w, "expected Content-Type: application/json, got: "+ct, 400)
				return
			}
			writeJSON(w, ProveResponse{ProofID: "proof-1"})
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	_, err := c.Prove("model-1", []float64{1.0})
	if err != nil {
		t.Fatalf("Prove() error: %v", err)
	}
}

func TestNoContentTypeForGET(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"GET /stats": func(w http.ResponseWriter, r *http.Request) {
			ct := r.Header.Get("Content-Type")
			if ct != "" {
				http.Error(w, "unexpected Content-Type on GET: "+ct, 400)
				return
			}
			writeJSON(w, StatsResponse{TotalProofs: 1})
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	_, err := c.Stats()
	if err != nil {
		t.Fatalf("Stats() error: %v", err)
	}
}

// --- URL trailing slash ---

func TestURLTrailingSlash(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"GET /health": func(w http.ResponseWriter, r *http.Request) {
			writeJSON(w, HealthResponse{Status: "ok"})
		},
	})
	defer srv.Close()

	// Pass URL with trailing slash; client should trim it
	c := New(srv.URL+"/", "")
	resp, err := c.Health()
	if err != nil {
		t.Fatalf("Health() error: %v", err)
	}
	if resp.Status != "ok" {
		t.Errorf("expected status 'ok', got %q", resp.Status)
	}
}

// --- APIError string ---

func TestAPIErrorString(t *testing.T) {
	e := &APIError{StatusCode: 403, Body: "forbidden"}
	s := e.Error()
	if !strings.Contains(s, "403") {
		t.Errorf("expected error string to contain '403', got %q", s)
	}
	if !strings.Contains(s, "forbidden") {
		t.Errorf("expected error string to contain 'forbidden', got %q", s)
	}
}

// --- Custom HTTP client ---

func TestCustomHTTPClient(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"GET /health": func(w http.ResponseWriter, r *http.Request) {
			writeJSON(w, HealthResponse{Status: "ok"})
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	c.HTTP = &http.Client{Timeout: 5 * time.Second}

	resp, err := c.Health()
	if err != nil {
		t.Fatalf("Health() error: %v", err)
	}
	if resp.Status != "ok" {
		t.Errorf("expected status 'ok', got %q", resp.Status)
	}
}
