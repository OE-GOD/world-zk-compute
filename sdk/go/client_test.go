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

// writeRaw answers with a JSON body exactly as a service sent it.
func writeRaw(w http.ResponseWriter, body string) {
	w.Header().Set("Content-Type", "application/json")
	io.WriteString(w, body)
}

func writeJSON(w http.ResponseWriter, v interface{}) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(v)
}

// The JSON bodies in these tests were captured from the real services
// (api-gateway in front of proof-generator --mock, proof-registry and
// zkml-verifier-service), so the client is checked against the wire format
// and not against its own structs.

// --- Health ---

func TestHealth(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"GET /health": func(w http.ResponseWriter, r *http.Request) {
			writeRaw(w, `{"status":"degraded","services":[`+
				`{"name":"generator","healthy":true,"response_ms":2},`+
				`{"name":"registry","healthy":false,"error":"connection refused"},`+
				`{"name":"verifier","healthy":true,"response_ms":0}],`+
				`"healthy_count":2,"total_count":3}`)
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	resp, err := c.Health()
	if err != nil {
		t.Fatalf("Health() error: %v", err)
	}
	if resp.Status != "degraded" {
		t.Errorf("expected status 'degraded', got %q", resp.Status)
	}
	if resp.HealthyCount != 2 || resp.TotalCount != 3 {
		t.Errorf("expected 2 of 3 healthy, got %d of %d", resp.HealthyCount, resp.TotalCount)
	}
	if len(resp.Services) != 3 {
		t.Fatalf("expected 3 services, got %d", len(resp.Services))
	}
	generator, registry := resp.Services[0], resp.Services[1]
	if generator.Name != "generator" || !generator.Healthy || generator.ResponseMs != 2 {
		t.Errorf("unexpected generator health: %+v", generator)
	}
	if registry.Healthy || registry.Error != "connection refused" {
		t.Errorf("unexpected registry health: %+v", registry)
	}
}

// --- UploadModel ---

const uploadModelResponse = `{"model_id":"8614598b-e20f-40f9-ba09-ceb74b340199",` +
	`"model_hash":"0xd2f805d74af95f367236a0e636abdaf8c066221e5ac8524b4cecfb2a992f7f68",` +
	`"circuit_hash":"0xf9076219ddb4ab48717b00aa8e40029832f587efe045c4a0b337e17b117627b7",` +
	`"format":"xgboost","active":true}`

func TestUploadModel(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"POST /models": func(w http.ResponseWriter, r *http.Request) {
			body, _ := io.ReadAll(r.Body)
			var req map[string]interface{}
			json.Unmarshal(body, &req)

			// The proof generator parses model_json as a JSON object. Sent as a
			// string it is rejected (or, with a format hint, stored as a model
			// that is one JSON string).
			model, ok := req["model_json"].(map[string]interface{})
			if !ok {
				http.Error(w, "model_json must be a JSON object", 400)
				return
			}
			if _, ok := model["trees"]; !ok {
				http.Error(w, "model_json lost its content", 400)
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

			writeRaw(w, uploadModelResponse)
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	resp, err := c.UploadModel(`{"trees":[]}`, "test-model", "xgboost")
	if err != nil {
		t.Fatalf("UploadModel() error: %v", err)
	}
	if resp.ModelID != "8614598b-e20f-40f9-ba09-ceb74b340199" {
		t.Errorf("unexpected model_id %q", resp.ModelID)
	}
	if resp.ModelHash != "0xd2f805d74af95f367236a0e636abdaf8c066221e5ac8524b4cecfb2a992f7f68" {
		t.Errorf("unexpected model_hash %q", resp.ModelHash)
	}
	if resp.CircuitHash != "0xf9076219ddb4ab48717b00aa8e40029832f587efe045c4a0b337e17b117627b7" {
		t.Errorf("unexpected circuit_hash %q", resp.CircuitHash)
	}
	if resp.Format != "xgboost" || !resp.Active {
		t.Errorf("unexpected format/active: %q %v", resp.Format, resp.Active)
	}
}

func TestUploadModelOptionalFields(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"POST /models": func(w http.ResponseWriter, r *http.Request) {
			body, _ := io.ReadAll(r.Body)
			var req map[string]interface{}
			json.Unmarshal(body, &req)

			// The service answers 422 "missing field `name`" without a name, so
			// an empty one is replaced by the default. format may be left out.
			if req["name"] != DefaultModelName {
				http.Error(w, "name must always be sent", 422)
				return
			}
			if _, ok := req["format"]; ok {
				http.Error(w, "format should be omitted when empty", 400)
				return
			}

			writeRaw(w, uploadModelResponse)
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	resp, err := c.UploadModel(`{"trees":[]}`, "", "")
	if err != nil {
		t.Fatalf("UploadModel() error: %v", err)
	}
	if resp.ModelID != "8614598b-e20f-40f9-ba09-ceb74b340199" {
		t.Errorf("unexpected model_id %q", resp.ModelID)
	}
}

func TestUploadModelRejectsInvalidJSON(t *testing.T) {
	called := false
	srv := newTestServer(map[string]http.HandlerFunc{
		"POST /models": func(w http.ResponseWriter, r *http.Request) {
			called = true
			writeRaw(w, uploadModelResponse)
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	if _, err := c.UploadModel(`{"trees":[`, "broken", ""); err == nil {
		t.Fatal("expected an error for a model that is not valid JSON")
	}
	if called {
		t.Error("no request should be sent for an invalid model")
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

			writeRaw(w, `{"proof_id":"5679ff2d-7546-4dbc-8480-6533cca99521",`+
				`"model_hash":"0xd2f805d74af95f367236a0e636abdaf8c066221e5ac8524b4cecfb2a992f7f68",`+
				`"circuit_hash":"0xf9076219ddb4ab48717b00aa8e40029832f587efe045c4a0b337e17b117627b7",`+
				`"output":"approved","mock":true,"prove_time_ms":1234,`+
				`"proof_bundle":{"proof_hex":"0x43e85f3c","public_inputs_hex":"0xb71d7b68","proof_size_bytes":32}}`)
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	resp, err := c.Prove("model-123", []float64{1.0, 2.0, 3.0})
	if err != nil {
		t.Fatalf("Prove() error: %v", err)
	}
	if resp.ProofID != "5679ff2d-7546-4dbc-8480-6533cca99521" {
		t.Errorf("unexpected proof_id %q", resp.ProofID)
	}
	if resp.Output != "approved" || !resp.Mock {
		t.Errorf("unexpected output/mock: %q %v", resp.Output, resp.Mock)
	}
	if resp.ProveTimeMs != 1234 {
		t.Errorf("expected prove_time_ms 1234, got %d", resp.ProveTimeMs)
	}
	if resp.ProofBundle == nil || resp.ProofBundle.ProofHex != "0x43e85f3c" ||
		resp.ProofBundle.PublicInputsHex != "0xb71d7b68" || resp.ProofBundle.ProofSizeBytes != 32 {
		t.Errorf("unexpected proof bundle: %+v", resp.ProofBundle)
	}
	// The generator leaves registry_proof_id out when it did not store the proof.
	if resp.RegistryProofID != "" {
		t.Errorf("expected no registry proof ID, got %q", resp.RegistryProofID)
	}
}

func TestProveRegistryProofID(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"POST /prove": func(w http.ResponseWriter, r *http.Request) {
			writeRaw(w, `{"proof_id":"gen-1","model_hash":"0x01","circuit_hash":"0x02","output":"approved",`+
				`"mock":false,"prove_time_ms":7,`+
				`"proof_bundle":{"proof_hex":"0x00","public_inputs_hex":"0x00","proof_size_bytes":1},`+
				`"registry_proof_id":"f2917f9a-981a-49ce-a905-19bcc2ea3050"}`)
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	resp, err := c.Prove("model-123", []float64{1.0})
	if err != nil {
		t.Fatalf("Prove() error: %v", err)
	}
	if resp.RegistryProofID != "f2917f9a-981a-49ce-a905-19bcc2ea3050" {
		t.Errorf("unexpected registry proof ID %q", resp.RegistryProofID)
	}
}

// --- Verify ---

func TestVerify(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"POST /proofs/7fd37292-2de3-4e3d-8431-ec4e52b3346e/verify": func(w http.ResponseWriter, r *http.Request) {
			writeRaw(w, `{"verified":true,"receipt_id":"f86d8359-39ea-4fd2-8299-cda5458a4f75"}`)
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	resp, err := c.Verify("7fd37292-2de3-4e3d-8431-ec4e52b3346e")
	if err != nil {
		t.Fatalf("Verify() error: %v", err)
	}
	if !resp.Verified {
		t.Error("expected verified=true")
	}
	if resp.ReceiptID != "f86d8359-39ea-4fd2-8299-cda5458a4f75" {
		t.Errorf("unexpected receipt_id %q", resp.ReceiptID)
	}
	if resp.Error != "" {
		t.Errorf("expected no error, got %q", resp.Error)
	}
}

// A proof the verifier rejects is a 200 with verified=false and the reason.
func TestVerifyFailed(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"POST /proofs/fe6e34c6-8083-4438-929d-2add0a741b2a/verify": func(w http.ResponseWriter, r *http.Request) {
			writeRaw(w, `{"verified":false,"receipt_id":"a84af8e4-46ba-4441-b86c-a683095c490d",`+
				`"error":"verification failed: sumcheck failed"}`)
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	resp, err := c.Verify("fe6e34c6-8083-4438-929d-2add0a741b2a")
	if err != nil {
		t.Fatalf("Verify() error: %v", err)
	}
	if resp.Verified {
		t.Error("expected verified=false")
	}
	if resp.ReceiptID != "a84af8e4-46ba-4441-b86c-a683095c490d" {
		t.Errorf("unexpected receipt_id %q", resp.ReceiptID)
	}
	if resp.Error != "verification failed: sumcheck failed" {
		t.Errorf("unexpected error %q", resp.Error)
	}
}

// An ID is one path segment: "/" and "?" in it must not start a new segment
// or a query string.
func TestProofIDIsEscaped(t *testing.T) {
	var paths, queries []string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		paths = append(paths, r.Method+" "+r.URL.EscapedPath())
		queries = append(queries, r.URL.RawQuery)
		writeRaw(w, `{}`)
	}))
	defer srv.Close()

	c := New(srv.URL, "")
	if _, err := c.Verify("a/b?c"); err != nil {
		t.Fatalf("Verify() error: %v", err)
	}
	if _, err := c.GetReceipt("a/b?c"); err != nil {
		t.Fatalf("GetReceipt() error: %v", err)
	}

	want := []string{"POST /proofs/a%2Fb%3Fc/verify", "GET /proofs/a%2Fb%3Fc/receipt"}
	if len(paths) != len(want) {
		t.Fatalf("expected %d requests, got %v", len(want), paths)
	}
	for i := range want {
		if paths[i] != want[i] || queries[i] != "" {
			t.Errorf("request %d: got %q with query %q, want %q with no query", i, paths[i], queries[i], want[i])
		}
	}
}

// --- GetReceipt ---

func TestGetReceipt(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"GET /proofs/proof-abc/receipt": func(w http.ResponseWriter, r *http.Request) {
			writeRaw(w, `{"receipt_id":"a550145b-62ba-489f-98b6-6af4a603af00","proof_id":"proof-abc",`+
				`"circuit_hash":"0x8e7c338859ba0bcb6911e6b68794f5449c0bb36a0e2ce47cb5dc96e8eb56e909",`+
				`"model_hash":"","verified":true,"verified_at":"2026-11-22T20:57:57.510666+00:00",`+
				`"verifier_version":"0.1.0","verifier_host":"unknown","signature":"06c7e070136ee214"}`)
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	resp, err := c.GetReceipt("proof-abc")
	if err != nil {
		t.Fatalf("GetReceipt() error: %v", err)
	}
	if resp.ReceiptID != "a550145b-62ba-489f-98b6-6af4a603af00" {
		t.Errorf("unexpected receipt_id %q", resp.ReceiptID)
	}
	if resp.ProofID != "proof-abc" {
		t.Errorf("unexpected proof_id %q", resp.ProofID)
	}
	if !resp.Verified {
		t.Error("expected verified=true")
	}
	if resp.CircuitHash != "0x8e7c338859ba0bcb6911e6b68794f5449c0bb36a0e2ce47cb5dc96e8eb56e909" {
		t.Errorf("unexpected circuit_hash %q", resp.CircuitHash)
	}
	if resp.VerifiedAt != "2026-11-22T20:57:57.510666+00:00" {
		t.Errorf("unexpected verified_at %q", resp.VerifiedAt)
	}
	if resp.VerifierVersion != "0.1.0" || resp.VerifierHost != "unknown" {
		t.Errorf("unexpected verifier: %q %q", resp.VerifierVersion, resp.VerifierHost)
	}
	if resp.Signature != "06c7e070136ee214" {
		t.Errorf("unexpected signature %q", resp.Signature)
	}
	// The registry leaves "error" out of the receipt of a verified proof.
	if resp.Error != "" {
		t.Errorf("expected no error, got %q", resp.Error)
	}
}

// The receipt of a rejected proof carries the reason and no circuit hash.
func TestGetReceiptFailed(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"GET /proofs/fe6e34c6-8083-4438-929d-2add0a741b2a/receipt": func(w http.ResponseWriter, r *http.Request) {
			writeRaw(w, `{"receipt_id":"a84af8e4-46ba-4441-b86c-a683095c490d",`+
				`"proof_id":"fe6e34c6-8083-4438-929d-2add0a741b2a","circuit_hash":"",`+
				`"model_hash":"0xabc def&x=1","verified":false,`+
				`"verified_at":"2026-11-23T01:19:20.292088+00:00","verifier_version":"0.1.0",`+
				`"verifier_host":"unknown","signature":"b13d7e0e5f7b61d98b0cbfb07526840910fd2fb9b202be0fc383c6d2c3e09226`+
				`62b41172fb11f161327589beb78fabdc42239975510d63aeedd28704028e0158",`+
				`"error":"verification failed: sumcheck failed"}`)
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	resp, err := c.GetReceipt("fe6e34c6-8083-4438-929d-2add0a741b2a")
	if err != nil {
		t.Fatalf("GetReceipt() error: %v", err)
	}
	if resp.Verified {
		t.Error("expected verified=false")
	}
	if resp.Error != "verification failed: sumcheck failed" {
		t.Errorf("unexpected error %q", resp.Error)
	}
	if resp.CircuitHash != "" {
		t.Errorf("expected no circuit_hash, got %q", resp.CircuitHash)
	}
	if resp.ModelHash != "0xabc def&x=1" {
		t.Errorf("unexpected model_hash %q", resp.ModelHash)
	}
}

// --- ListProofs ---

// The registry answers with an envelope, not a bare array.
const listProofsResponse = `{"proofs":[` +
	`{"id":"f2917f9a-981a-49ce-a905-19bcc2ea3050","model_hash":"0xabc","circuit_hash":"0x8e7c",` +
	`"created_at":"2026-11-22T20:57:12.814955+00:00","verified":true,` +
	`"verified_at":"2026-11-22T20:57:57.511569+00:00","proof_size_bytes":350655,"deleted":false},` +
	`{"id":"0b1c","model_hash":"0xabc","circuit_hash":"0x8e7c",` +
	`"created_at":"2026-11-22T20:58:00.000000+00:00","verified":null,` +
	`"verified_at":null,"proof_size_bytes":32,"deleted":false}],"count":2}`

func TestListProofs(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"GET /proofs": func(w http.ResponseWriter, r *http.Request) {
			query := r.URL.Query()
			if query.Get("model") != "0xabc" {
				http.Error(w, "expected model=0xabc", 400)
				return
			}
			if query.Get("limit") != "10" {
				http.Error(w, "expected limit=10", 400)
				return
			}

			writeRaw(w, listProofsResponse)
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	proofs, err := c.ListProofs("0xabc", 10)
	if err != nil {
		t.Fatalf("ListProofs() error: %v", err)
	}
	if len(proofs) != 2 {
		t.Fatalf("expected 2 proofs, got %d", len(proofs))
	}
	first, second := proofs[0], proofs[1]
	if first.ProofID != "f2917f9a-981a-49ce-a905-19bcc2ea3050" {
		t.Errorf("unexpected proof ID %q", first.ProofID)
	}
	if first.Verified == nil || !*first.Verified || first.VerifiedAt == nil {
		t.Errorf("first proof should be verified: %+v", first)
	}
	if first.ProofSizeBytes != 350655 || first.ModelHash != "0xabc" {
		t.Errorf("unexpected first proof: %+v", first)
	}
	if first.CreatedAt != "2026-11-22T20:57:12.814955+00:00" {
		t.Errorf("unexpected created_at %q", first.CreatedAt)
	}
	if second.Verified != nil || second.VerifiedAt != nil {
		t.Errorf("second proof was never verified: %+v", second)
	}
	// The registry lists only proofs that were not deleted.
	if first.Deleted || second.Deleted {
		t.Errorf("no listed proof is deleted: %+v %+v", first, second)
	}
}

// Every field of a listed proof has a place in ProofSummary. This also covers
// fields such as "deleted" whose value on the wire is the Go zero value.
func TestListProofsKnowsEveryField(t *testing.T) {
	dec := json.NewDecoder(strings.NewReader(listProofsResponse))
	dec.DisallowUnknownFields()
	var resp proofListResponse
	if err := dec.Decode(&resp); err != nil {
		t.Fatalf("decoding the registry's list: %v", err)
	}
}

func TestListProofsNoFilter(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"GET /proofs": func(w http.ResponseWriter, r *http.Request) {
			if r.URL.RawQuery != "" {
				http.Error(w, "no query expected", 400)
				return
			}
			writeRaw(w, `{"proofs":[],"count":0}`)
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

func TestListProofsEscapesModel(t *testing.T) {
	const model = "a&limit=1 b"
	srv := newTestServer(map[string]http.HandlerFunc{
		"GET /proofs": func(w http.ResponseWriter, r *http.Request) {
			query := r.URL.Query()
			if query.Get("model") != model || query.Get("limit") != "" {
				http.Error(w, "model was not escaped: "+r.URL.RawQuery, 400)
				return
			}
			writeRaw(w, `{"proofs":[],"count":0}`)
		},
	})
	defer srv.Close()

	c := New(srv.URL, "")
	if _, err := c.ListProofs(model, 0); err != nil {
		t.Fatalf("ListProofs() error: %v", err)
	}
}

// --- Stats ---

func TestStats(t *testing.T) {
	srv := newTestServer(map[string]http.HandlerFunc{
		"GET /stats": func(w http.ResponseWriter, r *http.Request) {
			writeRaw(w, `{"total_proofs":100,"active_proofs":98,"verified_count":95,`+
				`"failed_count":3,"total_storage_bytes":350655}`)
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
	if resp.ActiveProofs != 98 {
		t.Errorf("expected active_proofs 98, got %d", resp.ActiveProofs)
	}
	if resp.VerifiedCount != 95 || resp.FailedCount != 3 {
		t.Errorf("expected 95 verified and 3 failed, got %d and %d", resp.VerifiedCount, resp.FailedCount)
	}
	if resp.TotalStorageBytes != 350655 {
		t.Errorf("expected total_storage_bytes 350655, got %d", resp.TotalStorageBytes)
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
