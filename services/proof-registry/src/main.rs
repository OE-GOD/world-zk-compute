//! Proof registry service — store, index, search, and verify proof bundles.
//!
//! ## Environment Variables
//!
//! | Variable | Description | Default |
//! |---|---|---|
//! | `PORT` | Server listen port | 3001 |
//! | `PROOF_DB_PATH` | Path to SQLite database file | `./proofs.db` |
//! | `PROOF_STORAGE_DIR` | Directory for proof bundle JSON files | `./proof-store/` |
//! | `REGISTRY_API_KEYS` | Comma-separated API keys (empty = no auth) | (none) |
//! | `REGISTRY_SIGNING_KEY` | Hex-encoded secp256k1 private key for receipt signing | (auto-gen) |
//! | `TRANSPARENCY_DB_PATH` | Path to transparency log SQLite database | `./transparency.db` |

mod db;
pub mod receipt;
mod routes;
#[cfg(feature = "s3")]
pub mod s3;
pub mod sign;
pub mod storage;
mod transparency;

use std::env;
use std::sync::Arc;

use axum::{
    middleware,
    routing::{delete, get, post},
    Router,
};
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;

use db::ProofDb;
use routes::AppState;

fn build_app_with_state(state: Arc<AppState>) -> Router {
    let protected = Router::new()
        .route("/proofs", post(routes::submit_proof))
        .route("/proofs", get(routes::search_proofs))
        .route("/proofs/:id", get(routes::get_proof))
        .route("/proofs/:id/verify", post(routes::verify_proof))
        .route("/proofs/:id/receipt", get(routes::get_receipt))
        .route("/proofs/:id", delete(routes::delete_proof))
        .route("/stats", get(routes::stats))
        .route("/transparency/root", get(routes::transparency_root))
        .route(
            "/transparency/proof/:index",
            get(routes::transparency_proof),
        )
        .route("/transparency/verify", post(routes::transparency_verify))
        .route("/transparency/entries", get(routes::transparency_entries))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            routes::auth_middleware,
        ))
        .with_state(state.clone());

    let health_router = Router::new()
        .route("/health", get(routes::health))
        .with_state(state);

    Router::new()
        .merge(health_router)
        .merge(protected)
        .layer(CorsLayer::permissive())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let port: u16 = env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3001);

    let db_path = env::var("PROOF_DB_PATH").unwrap_or_else(|_| "./proofs.db".to_string());

    let storage_dir =
        env::var("PROOF_STORAGE_DIR").unwrap_or_else(|_| "./proof-store/".to_string());

    let api_keys: Vec<String> = env::var("REGISTRY_API_KEYS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    // Select storage backend based on environment variables.
    // If S3_BUCKET is set (and the s3 feature is enabled), use S3 with Object Lock;
    // otherwise use local filesystem with append-only WORM semantics.
    let blob_store: Arc<dyn storage::ProofStorage> = {
        #[cfg(feature = "s3")]
        {
            if let Ok(bucket) = env::var("S3_BUCKET") {
                let region =
                    env::var("S3_REGION").unwrap_or_else(|_| "us-east-1".to_string());
                let mode = env::var("S3_RETENTION_MODE")
                    .unwrap_or_else(|_| "GOVERNANCE".to_string());
                let years: u32 = env::var("S3_RETENTION_YEARS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(7);
                let prefix =
                    env::var("S3_PREFIX").unwrap_or_else(|_| "proofs/".to_string());

                let s3 = s3::S3Storage::new(&bucket, &region, &mode, years, &prefix)
                    .await
                    .unwrap_or_else(|e| {
                        eprintln!("Failed to initialize S3 storage: {e}");
                        std::process::exit(1);
                    });

                eprintln!(
                    "Storage backend: S3 (bucket={bucket}, region={region}, \
                     mode={mode}, retention={years}y)"
                );
                Arc::new(s3) as Arc<dyn storage::ProofStorage>
            } else {
                let local = storage::LocalStorage::new(&storage_dir).unwrap_or_else(|e| {
                    eprintln!("Failed to initialize local storage: {e}");
                    std::process::exit(1);
                });
                eprintln!("Storage backend: local (dir={})", storage_dir);
                Arc::new(local) as Arc<dyn storage::ProofStorage>
            }
        }
        #[cfg(not(feature = "s3"))]
        {
            let local = storage::LocalStorage::new(&storage_dir).unwrap_or_else(|e| {
                eprintln!("Failed to initialize local storage: {e}");
                std::process::exit(1);
            });
            eprintln!("Storage backend: local (dir={})", storage_dir);
            Arc::new(local) as Arc<dyn storage::ProofStorage>
        }
    };

    let db = ProofDb::with_storage(&db_path, blob_store).unwrap_or_else(|e| {
        eprintln!("Failed to initialize database: {e}");
        std::process::exit(1);
    });

    let tlog_path =
        env::var("TRANSPARENCY_DB_PATH").unwrap_or_else(|_| "./transparency.db".to_string());
    let tlog = transparency::TransparencyLog::new(&tlog_path).unwrap_or_else(|e| {
        eprintln!("Failed to initialize transparency log: {e}");
        std::process::exit(1);
    });

    let signing_key_path =
        env::var("REGISTRY_KEY_FILE").unwrap_or_else(|_| "./registry_signing_key.hex".to_string());
    let signing_key = sign::load_or_generate_key(&signing_key_path).unwrap_or_else(|e| {
        eprintln!("Failed to load signing key: {e}");
        std::process::exit(1);
    });

    let vk_hex = sign::verifying_key_hex(&sign::verifying_key(&signing_key));
    eprintln!("Receipt signing public key: {vk_hex}");

    if api_keys.is_empty() {
        eprintln!("API key auth disabled (no REGISTRY_API_KEYS set)");
    } else {
        eprintln!(
            "API key auth enabled ({} key(s) configured)",
            api_keys.len()
        );
    }

    let state = Arc::new(AppState {
        db: Mutex::new(db),
        transparency_log: Mutex::new(tlog),
        signing_key,
        api_keys,
    });

    let app = build_app_with_state(state);
    let addr = format!("0.0.0.0:{port}");
    eprintln!("Proof registry listening on {addr}");

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn make_test_state() -> Arc<AppState> {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let storage_dir = tmp.path().join("bundles");
        let tlog_path = tmp.path().join("transparency.db");
        let db = ProofDb::new(db_path.to_str().unwrap(), storage_dir.to_str().unwrap()).unwrap();
        let tlog = transparency::TransparencyLog::new(tlog_path.to_str().unwrap()).unwrap();

        // Deterministic test signing key.
        let signing_key = k256::ecdsa::SigningKey::from_slice(&[42u8; 32]).unwrap();

        // Leak the tempdir to keep it alive for the duration of the test.
        std::mem::forget(tmp);

        Arc::new(AppState {
            db: Mutex::new(db),
            transparency_log: Mutex::new(tlog),
            signing_key,
            api_keys: vec![],
        })
    }

    fn make_test_bundle_json() -> serde_json::Value {
        let proof_data: Vec<u8> = (0..256).map(|i| (i % 256) as u8).collect();
        let gens_data: Vec<u8> = (0..128).map(|i| ((i * 7) % 256) as u8).collect();

        serde_json::json!({
            "proof_hex": format!("0x{}", hex::encode(&proof_data)),
            "public_inputs_hex": "",
            "gens_hex": format!("0x{}", hex::encode(&gens_data)),
            "dag_circuit_description": {
                "num_compute_layers": 4,
                "layer_types": [0, 1, 0, 1],
            },
            "model_hash": "0xabcdef1234567890",
            "timestamp": 1700000000u64,
            "prover_version": "0.1.0-test",
            "circuit_hash": "0xdeadbeef",
        })
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let state = make_test_state();
        let app = build_app_with_state(state);

        let req = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["db_healthy"], true);
    }

    #[tokio::test]
    async fn test_submit_and_get_proof() {
        let state = make_test_state();
        let app = build_app_with_state(state);

        let bundle_json = make_test_bundle_json();

        // Submit a proof.
        let req = Request::builder()
            .method("POST")
            .uri("/proofs")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&bundle_json).unwrap()))
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let submit_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(submit_resp["status"], "stored");
        let proof_id = submit_resp["id"].as_str().unwrap().to_string();

        // Retrieve the proof.
        let req = Request::builder()
            .uri(format!("/proofs/{proof_id}"))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let get_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(get_resp["id"], proof_id);
        assert_eq!(get_resp["model_hash"], "0xabcdef1234567890");
    }

    #[tokio::test]
    async fn test_search_proofs() {
        let state = make_test_state();
        let app = build_app_with_state(state);

        let bundle_json = make_test_bundle_json();

        // Submit two proofs.
        for _ in 0..2 {
            let req = Request::builder()
                .method("POST")
                .uri("/proofs")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&bundle_json).unwrap()))
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::CREATED);
        }

        // Search all.
        let req = Request::builder()
            .uri("/proofs?limit=50")
            .body(Body::empty())
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let search_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(search_resp["count"], 2);

        // Search by model hash.
        let req = Request::builder()
            .uri("/proofs?model=0xabcdef1234567890")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let search_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(search_resp["count"], 2);
    }

    #[tokio::test]
    async fn test_delete_proof() {
        let state = make_test_state();
        let app = build_app_with_state(state);

        let bundle_json = make_test_bundle_json();

        // Submit.
        let req = Request::builder()
            .method("POST")
            .uri("/proofs")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&bundle_json).unwrap()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let submit_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let proof_id = submit_resp["id"].as_str().unwrap().to_string();

        // Delete.
        let req = Request::builder()
            .method("DELETE")
            .uri(format!("/proofs/{proof_id}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Should not be found.
        let req = Request::builder()
            .uri(format!("/proofs/{proof_id}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_stats_endpoint() {
        let state = make_test_state();
        let app = build_app_with_state(state);

        let req = Request::builder()
            .uri("/stats")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let stats: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(stats["total_proofs"], 0);
        assert_eq!(stats["active_proofs"], 0);
    }

    #[tokio::test]
    async fn test_get_nonexistent_proof() {
        let state = make_test_state();
        let app = build_app_with_state(state);

        let req = Request::builder()
            .uri("/proofs/nonexistent-id")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_auth_required_when_keys_set() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let storage_dir = tmp.path().join("bundles");
        let tlog_path = tmp.path().join("transparency.db");
        let db = ProofDb::new(db_path.to_str().unwrap(), storage_dir.to_str().unwrap()).unwrap();
        let tlog = transparency::TransparencyLog::new(tlog_path.to_str().unwrap()).unwrap();
        let signing_key = k256::ecdsa::SigningKey::from_slice(&[42u8; 32]).unwrap();
        std::mem::forget(tmp);

        let state = Arc::new(AppState {
            db: Mutex::new(db),
            transparency_log: Mutex::new(tlog),
            signing_key,
            api_keys: vec!["test-key-123".to_string()],
        });

        let app = build_app_with_state(state);

        // Request without key should fail.
        let req = Request::builder()
            .uri("/stats")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // Request with wrong key should fail.
        let req = Request::builder()
            .uri("/stats")
            .header("x-api-key", "wrong-key")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // Request with correct key should succeed.
        let req = Request::builder()
            .uri("/stats")
            .header("x-api-key", "test-key-123")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Health endpoint should always be public.
        let req = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // -- Transparency log integration tests --

    #[tokio::test]
    async fn test_transparency_root_empty() {
        let state = make_test_state();
        let app = build_app_with_state(state);

        let req = Request::builder()
            .uri("/transparency/root")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["tree_size"], 0);
        // Empty tree root is all zeros.
        assert_eq!(
            json["root"],
            "0000000000000000000000000000000000000000000000000000000000000000"
        );
    }

    #[tokio::test]
    async fn test_transparency_auto_append_on_submit() {
        let state = make_test_state();
        let app = build_app_with_state(state);

        let bundle_json = make_test_bundle_json();

        // Submit a proof.
        let req = Request::builder()
            .method("POST")
            .uri("/proofs")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&bundle_json).unwrap()))
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let submit_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(submit_resp["transparency_index"], 0);
        assert!(submit_resp["proof_hash"].as_str().unwrap().len() == 64);

        // Check the root is no longer zero.
        let req = Request::builder()
            .uri("/transparency/root")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let root_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(root_resp["tree_size"], 1);
        assert_ne!(
            root_resp["root"],
            "0000000000000000000000000000000000000000000000000000000000000000"
        );
    }

    #[tokio::test]
    async fn test_transparency_inclusion_proof_roundtrip() {
        let state = make_test_state();
        let app = build_app_with_state(state);

        let bundle_json = make_test_bundle_json();

        // Submit three proofs.
        for _ in 0..3 {
            let req = Request::builder()
                .method("POST")
                .uri("/proofs")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&bundle_json).unwrap()))
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::CREATED);
        }

        // Get inclusion proof for leaf 1.
        let req = Request::builder()
            .uri("/transparency/proof/1")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let proof_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(proof_resp["index"], 1);
        assert_eq!(proof_resp["tree_size"], 3);
        assert!(!proof_resp["proof"].as_array().unwrap().is_empty());

        // Verify the inclusion proof via the verify endpoint.
        let verify_req = serde_json::json!({
            "index": proof_resp["index"],
            "leaf_hash": proof_resp["leaf_hash"],
            "proof": proof_resp["proof"],
            "root": proof_resp["root"],
            "tree_size": proof_resp["tree_size"],
        });

        let req = Request::builder()
            .method("POST")
            .uri("/transparency/verify")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&verify_req).unwrap()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let verify_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(verify_resp["valid"], true);
    }

    #[tokio::test]
    async fn test_transparency_verify_invalid_proof() {
        let state = make_test_state();
        let app = build_app_with_state(state);

        // Submit a proof to have something in the log.
        let bundle_json = make_test_bundle_json();
        let req = Request::builder()
            .method("POST")
            .uri("/proofs")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&bundle_json).unwrap()))
            .unwrap();
        app.clone().oneshot(req).await.unwrap();

        // Try to verify with a wrong leaf hash.
        let verify_req = serde_json::json!({
            "index": 0,
            "leaf_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "proof": [],
            "root": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "tree_size": 1,
        });

        let req = Request::builder()
            .method("POST")
            .uri("/transparency/verify")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&verify_req).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let verify_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(verify_resp["valid"], false);
    }

    #[tokio::test]
    async fn test_transparency_entries() {
        let state = make_test_state();
        let app = build_app_with_state(state);

        let bundle_json = make_test_bundle_json();

        // Submit five proofs.
        for _ in 0..5 {
            let req = Request::builder()
                .method("POST")
                .uri("/proofs")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&bundle_json).unwrap()))
                .unwrap();
            app.clone().oneshot(req).await.unwrap();
        }

        // Get entries 2..4.
        let req = Request::builder()
            .uri("/transparency/entries?from=2&count=2")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let entries_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(entries_resp["total"], 5);
        let entries = entries_resp["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["index"], 2);
        assert_eq!(entries[1]["index"], 3);

        // Get all entries.
        let req = Request::builder()
            .uri("/transparency/entries")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let entries_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(entries_resp["entries"].as_array().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn test_transparency_proof_nonexistent() {
        let state = make_test_state();
        let app = build_app_with_state(state);

        let req = Request::builder()
            .uri("/transparency/proof/999")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_transparency_root_changes_after_append() {
        let state = make_test_state();
        let app = build_app_with_state(state);

        let bundle_json = make_test_bundle_json();

        // Submit first proof.
        let req = Request::builder()
            .method("POST")
            .uri("/proofs")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&bundle_json).unwrap()))
            .unwrap();
        app.clone().oneshot(req).await.unwrap();

        // Get root after first.
        let req = Request::builder()
            .uri("/transparency/root")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let root1: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // Submit second proof (different bundle to get different hash).
        let bundle_json2 = serde_json::json!({
            "proof_hex": "0xdeadbeef",
            "public_inputs_hex": "",
            "gens_hex": "0xcafebabe",
            "dag_circuit_description": {"num_compute_layers": 1},
            "model_hash": "0x1111",
            "timestamp": 1700000001u64,
            "prover_version": "0.1.1-test",
            "circuit_hash": "0x2222",
        });

        let req = Request::builder()
            .method("POST")
            .uri("/proofs")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&bundle_json2).unwrap()))
            .unwrap();
        app.clone().oneshot(req).await.unwrap();

        // Get root after second.
        let req = Request::builder()
            .uri("/transparency/root")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let root2: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // Root must change.
        assert_ne!(root1["root"], root2["root"]);
        assert_eq!(root2["tree_size"], 2);
    }
}
