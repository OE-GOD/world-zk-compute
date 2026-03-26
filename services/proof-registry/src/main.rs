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

mod db;
mod routes;

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

    let db = ProofDb::new(&db_path, &storage_dir).unwrap_or_else(|e| {
        eprintln!("Failed to initialize database: {e}");
        std::process::exit(1);
    });

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
        let db = ProofDb::new(db_path.to_str().unwrap(), storage_dir.to_str().unwrap()).unwrap();

        // Leak the tempdir to keep it alive for the duration of the test.
        std::mem::forget(tmp);

        Arc::new(AppState {
            db: Mutex::new(db),
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
        let db =
            ProofDb::new(db_path.to_str().unwrap(), storage_dir.to_str().unwrap()).unwrap();
        std::mem::forget(tmp);

        let state = Arc::new(AppState {
            db: Mutex::new(db),
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
}
