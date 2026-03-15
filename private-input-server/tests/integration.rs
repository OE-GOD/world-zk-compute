//! Integration tests for the Private Input Server.
//!
//! These tests exercise the full HTTP layer via `tower::ServiceExt::oneshot`,
//! covering upload/download flows, access control validation, data integrity,
//! and file-backed storage persistence.

use alloy::primitives::Address;
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use private_input_server::{
    auth::AuthRequest, build_app, store, AppState, ErrorResponse, HealthResponse, UploadResponse,
};
use std::sync::Arc;
use tower::util::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build an in-memory test app with chain verification disabled.
fn test_app() -> axum::Router {
    let state = Arc::new(AppState {
        store: store::InputStore::new(),
        rpc_url: "http://localhost:8545".to_string(),
        engine_address: Address::ZERO,
        skip_chain_verification: true,
    });
    build_app(state)
}

/// Build a test app backed by the given `AppState` (for shared-state tests).
fn test_app_with_state(state: Arc<AppState>) -> axum::Router {
    build_app(state)
}

/// Create a valid `AuthRequest` signed by the given signer for the specified
/// request_id and current timestamp.
async fn make_auth(signer: &PrivateKeySigner, request_id: u64) -> AuthRequest {
    let address = signer.address();
    let now = current_timestamp();
    let message = format!(
        "world-zk-compute:fetch-input:{}:{}:{}",
        request_id,
        address.to_checksum(None),
        now,
    );
    let sig = signer.sign_message(message.as_bytes()).await.unwrap();
    AuthRequest {
        request_id,
        prover_address: address.to_checksum(None),
        timestamp: now,
        signature: format!("0x{}", hex::encode(sig.as_bytes())),
    }
}

/// Create an `AuthRequest` with an explicit timestamp (for expiry tests).
async fn make_auth_with_timestamp(
    signer: &PrivateKeySigner,
    request_id: u64,
    timestamp: u64,
) -> AuthRequest {
    let address = signer.address();
    let message = format!(
        "world-zk-compute:fetch-input:{}:{}:{}",
        request_id,
        address.to_checksum(None),
        timestamp,
    );
    let sig = signer.sign_message(message.as_bytes()).await.unwrap();
    AuthRequest {
        request_id,
        prover_address: address.to_checksum(None),
        timestamp,
        signature: format!("0x{}", hex::encode(sig.as_bytes())),
    }
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Helper to POST JSON to the fetch endpoint and return the response.
async fn post_fetch(
    app: axum::Router,
    request_id: u64,
    auth: &AuthRequest,
) -> axum::response::Response {
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri(format!("/inputs/{}", request_id))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(auth).unwrap()))
            .unwrap(),
    )
    .await
    .unwrap()
}

/// Helper to POST raw bytes to the upload endpoint and return the response.
async fn post_upload(
    app: axum::Router,
    request_id: u64,
    data: Vec<u8>,
) -> axum::response::Response {
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri(format!("/inputs/{}/upload", request_id))
            .header("content-type", "application/octet-stream")
            .body(Body::from(data))
            .unwrap(),
    )
    .await
    .unwrap()
}

/// Read the full body bytes from a response.
async fn body_bytes(response: axum::response::Response) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

// ===========================================================================
// 1. Upload / Download Flow
// ===========================================================================

#[tokio::test]
async fn test_upload_then_download_returns_same_bytes() {
    let state = Arc::new(AppState {
        store: store::InputStore::new(),
        rpc_url: "http://localhost:8545".to_string(),
        engine_address: Address::ZERO,
        skip_chain_verification: true,
    });
    let app = test_app_with_state(state);

    let payload = b"hello, world! this is private input data".to_vec();

    // Upload
    let upload_resp = post_upload(app.clone(), 1, payload.clone()).await;
    assert_eq!(upload_resp.status(), StatusCode::OK);

    let upload_body: UploadResponse =
        serde_json::from_slice(&body_bytes(upload_resp).await).unwrap();
    assert_eq!(upload_body.request_id, 1);
    assert_eq!(upload_body.size, payload.len());
    assert!(upload_body.digest.starts_with("0x"));

    // Download
    let signer = PrivateKeySigner::random();
    let auth = make_auth(&signer, 1).await;
    let fetch_resp = post_fetch(app.clone(), 1, &auth).await;
    assert_eq!(fetch_resp.status(), StatusCode::OK);

    let fetched = body_bytes(fetch_resp).await;
    assert_eq!(fetched, payload);
}

#[tokio::test]
async fn test_upload_empty_body_returns_400() {
    let app = test_app();
    let resp = post_upload(app, 7, vec![]).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let err: ErrorResponse = serde_json::from_slice(&body_bytes(resp).await).unwrap();
    assert!(
        err.error.contains("Empty body"),
        "unexpected error: {}",
        err.error
    );
}

#[tokio::test]
async fn test_upload_large_binary_blob() {
    let state = Arc::new(AppState {
        store: store::InputStore::new(),
        rpc_url: "http://localhost:8545".to_string(),
        engine_address: Address::ZERO,
        skip_chain_verification: true,
    });
    let app = test_app_with_state(state);

    // Upload a 1 MB blob of random-ish bytes
    let payload: Vec<u8> = (0..1_000_000u32).map(|i| (i % 256) as u8).collect();

    let upload_resp = post_upload(app.clone(), 100, payload.clone()).await;
    assert_eq!(upload_resp.status(), StatusCode::OK);

    let signer = PrivateKeySigner::random();
    let auth = make_auth(&signer, 100).await;
    let fetch_resp = post_fetch(app.clone(), 100, &auth).await;
    assert_eq!(fetch_resp.status(), StatusCode::OK);

    let fetched = body_bytes(fetch_resp).await;
    assert_eq!(fetched.len(), payload.len());
    assert_eq!(fetched, payload);
}

#[tokio::test]
async fn test_upload_overwrites_previous_data() {
    let state = Arc::new(AppState {
        store: store::InputStore::new(),
        rpc_url: "http://localhost:8545".to_string(),
        engine_address: Address::ZERO,
        skip_chain_verification: true,
    });
    let app = test_app_with_state(state);

    // Upload v1
    let resp1 = post_upload(app.clone(), 5, b"version1".to_vec()).await;
    assert_eq!(resp1.status(), StatusCode::OK);

    // Upload v2 for same request_id
    let resp2 = post_upload(app.clone(), 5, b"version2".to_vec()).await;
    assert_eq!(resp2.status(), StatusCode::OK);

    // Fetch should return v2
    let signer = PrivateKeySigner::random();
    let auth = make_auth(&signer, 5).await;
    let fetch_resp = post_fetch(app.clone(), 5, &auth).await;
    assert_eq!(fetch_resp.status(), StatusCode::OK);
    assert_eq!(body_bytes(fetch_resp).await, b"version2");
}

#[tokio::test]
async fn test_fetch_nonexistent_request_id_returns_404() {
    let app = test_app();
    let signer = PrivateKeySigner::random();
    let auth = make_auth(&signer, 999).await;
    let resp = post_fetch(app, 999, &auth).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let err: ErrorResponse = serde_json::from_slice(&body_bytes(resp).await).unwrap();
    assert!(err.error.contains("999"));
}

#[tokio::test]
async fn test_multiple_request_ids_are_independent() {
    let state = Arc::new(AppState {
        store: store::InputStore::new(),
        rpc_url: "http://localhost:8545".to_string(),
        engine_address: Address::ZERO,
        skip_chain_verification: true,
    });
    let app = test_app_with_state(state);

    post_upload(app.clone(), 10, b"data-for-10".to_vec()).await;
    post_upload(app.clone(), 20, b"data-for-20".to_vec()).await;

    let signer = PrivateKeySigner::random();

    let auth10 = make_auth(&signer, 10).await;
    let resp10 = post_fetch(app.clone(), 10, &auth10).await;
    assert_eq!(body_bytes(resp10).await, b"data-for-10");

    let auth20 = make_auth(&signer, 20).await;
    let resp20 = post_fetch(app.clone(), 20, &auth20).await;
    assert_eq!(body_bytes(resp20).await, b"data-for-20");
}

// ===========================================================================
// 2. Access Control Validation
// ===========================================================================

#[tokio::test]
async fn test_expired_timestamp_returns_401() {
    let state = Arc::new(AppState {
        store: store::InputStore::new(),
        rpc_url: "http://localhost:8545".to_string(),
        engine_address: Address::ZERO,
        skip_chain_verification: true,
    });
    let app = test_app_with_state(state);

    post_upload(app.clone(), 42, b"secret".to_vec()).await;

    let signer = PrivateKeySigner::random();
    // Timestamp 10 minutes in the past (max allowed is 300s)
    let old_timestamp = current_timestamp() - 600;
    let auth = make_auth_with_timestamp(&signer, 42, old_timestamp).await;

    let resp = post_fetch(app.clone(), 42, &auth).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let err: ErrorResponse = serde_json::from_slice(&body_bytes(resp).await).unwrap();
    assert!(
        err.error.contains("expired") || err.error.contains("Timestamp"),
        "unexpected error: {}",
        err.error
    );
}

#[tokio::test]
async fn test_future_timestamp_returns_401() {
    let state = Arc::new(AppState {
        store: store::InputStore::new(),
        rpc_url: "http://localhost:8545".to_string(),
        engine_address: Address::ZERO,
        skip_chain_verification: true,
    });
    let app = test_app_with_state(state);

    post_upload(app.clone(), 42, b"secret".to_vec()).await;

    let signer = PrivateKeySigner::random();
    // Timestamp 5 minutes in the future (max allowed is 60s)
    let future_timestamp = current_timestamp() + 300;
    let auth = make_auth_with_timestamp(&signer, 42, future_timestamp).await;

    let resp = post_fetch(app.clone(), 42, &auth).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let err: ErrorResponse = serde_json::from_slice(&body_bytes(resp).await).unwrap();
    assert!(
        err.error.contains("future") || err.error.contains("Timestamp"),
        "unexpected error: {}",
        err.error
    );
}

#[tokio::test]
async fn test_wrong_signer_returns_401() {
    let state = Arc::new(AppState {
        store: store::InputStore::new(),
        rpc_url: "http://localhost:8545".to_string(),
        engine_address: Address::ZERO,
        skip_chain_verification: true,
    });
    let app = test_app_with_state(state);

    post_upload(app.clone(), 42, b"secret".to_vec()).await;

    let actual_signer = PrivateKeySigner::random();
    let impersonated_signer = PrivateKeySigner::random();

    // Sign with actual_signer but claim to be impersonated_signer
    let now = current_timestamp();
    let impersonated_address = impersonated_signer.address();
    let message = format!(
        "world-zk-compute:fetch-input:{}:{}:{}",
        42,
        impersonated_address.to_checksum(None),
        now,
    );
    let sig = actual_signer
        .sign_message(message.as_bytes())
        .await
        .unwrap();

    let auth = AuthRequest {
        request_id: 42,
        prover_address: impersonated_address.to_checksum(None),
        timestamp: now,
        signature: format!("0x{}", hex::encode(sig.as_bytes())),
    };

    let resp = post_fetch(app.clone(), 42, &auth).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let err: ErrorResponse = serde_json::from_slice(&body_bytes(resp).await).unwrap();
    assert!(
        err.error.contains("mismatch") || err.error.contains("Address"),
        "unexpected error: {}",
        err.error
    );
}

#[tokio::test]
async fn test_mismatched_request_id_returns_400() {
    let state = Arc::new(AppState {
        store: store::InputStore::new(),
        rpc_url: "http://localhost:8545".to_string(),
        engine_address: Address::ZERO,
        skip_chain_verification: true,
    });
    let app = test_app_with_state(state);

    post_upload(app.clone(), 42, b"secret".to_vec()).await;

    let signer = PrivateKeySigner::random();
    // Auth body says request_id=42 but we POST to /inputs/99
    let auth = make_auth(&signer, 42).await;

    let resp = post_fetch(app.clone(), 99, &auth).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let err: ErrorResponse = serde_json::from_slice(&body_bytes(resp).await).unwrap();
    assert!(
        err.error.contains("doesn't match"),
        "unexpected error: {}",
        err.error
    );
}

#[tokio::test]
async fn test_invalid_signature_hex_returns_401() {
    let state = Arc::new(AppState {
        store: store::InputStore::new(),
        rpc_url: "http://localhost:8545".to_string(),
        engine_address: Address::ZERO,
        skip_chain_verification: true,
    });
    let app = test_app_with_state(state);

    post_upload(app.clone(), 42, b"secret".to_vec()).await;

    let signer = PrivateKeySigner::random();
    let auth = AuthRequest {
        request_id: 42,
        prover_address: signer.address().to_checksum(None),
        timestamp: current_timestamp(),
        signature: "0xnothexatall".to_string(),
    };

    let resp = post_fetch(app.clone(), 42, &auth).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_signature_wrong_length_returns_401() {
    let state = Arc::new(AppState {
        store: store::InputStore::new(),
        rpc_url: "http://localhost:8545".to_string(),
        engine_address: Address::ZERO,
        skip_chain_verification: true,
    });
    let app = test_app_with_state(state);

    post_upload(app.clone(), 42, b"secret".to_vec()).await;

    let signer = PrivateKeySigner::random();
    let auth = AuthRequest {
        request_id: 42,
        prover_address: signer.address().to_checksum(None),
        timestamp: current_timestamp(),
        // Only 32 bytes instead of 65
        signature: format!("0x{}", "ab".repeat(32)),
    };

    let resp = post_fetch(app.clone(), 42, &auth).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let err: ErrorResponse = serde_json::from_slice(&body_bytes(resp).await).unwrap();
    assert!(
        err.error.contains("65"),
        "error should mention expected 65 bytes: {}",
        err.error
    );
}

#[tokio::test]
async fn test_invalid_address_format_returns_400() {
    let state = Arc::new(AppState {
        store: store::InputStore::new(),
        rpc_url: "http://localhost:8545".to_string(),
        engine_address: Address::ZERO,
        skip_chain_verification: true,
    });
    let app = test_app_with_state(state);

    post_upload(app.clone(), 42, b"secret".to_vec()).await;

    let auth = AuthRequest {
        request_id: 42,
        prover_address: "not-an-address".to_string(),
        timestamp: current_timestamp(),
        signature: format!("0x{}", "ab".repeat(65)),
    };

    let resp = post_fetch(app.clone(), 42, &auth).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ===========================================================================
// 3. Store and Retrieve Round-Trip (data integrity)
// ===========================================================================

#[tokio::test]
async fn test_upload_response_contains_correct_sha256_digest() {
    let state = Arc::new(AppState {
        store: store::InputStore::new(),
        rpc_url: "http://localhost:8545".to_string(),
        engine_address: Address::ZERO,
        skip_chain_verification: true,
    });
    let app = test_app_with_state(state);

    let payload = b"digest test data".to_vec();

    // Compute expected SHA-256
    use sha2::{Digest, Sha256};
    let expected_digest = {
        let mut hasher = Sha256::new();
        hasher.update(&payload);
        format!("0x{}", hex::encode(hasher.finalize()))
    };

    let resp = post_upload(app, 50, payload).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let upload: UploadResponse = serde_json::from_slice(&body_bytes(resp).await).unwrap();
    assert_eq!(upload.digest, expected_digest);
}

#[tokio::test]
async fn test_binary_data_preserved_through_round_trip() {
    let state = Arc::new(AppState {
        store: store::InputStore::new(),
        rpc_url: "http://localhost:8545".to_string(),
        engine_address: Address::ZERO,
        skip_chain_verification: true,
    });
    let app = test_app_with_state(state);

    // Include null bytes, high bytes, and other binary content
    let payload: Vec<u8> = (0u8..=255).collect();
    assert_eq!(payload.len(), 256);

    let upload_resp = post_upload(app.clone(), 77, payload.clone()).await;
    assert_eq!(upload_resp.status(), StatusCode::OK);

    let signer = PrivateKeySigner::random();
    let auth = make_auth(&signer, 77).await;
    let fetch_resp = post_fetch(app.clone(), 77, &auth).await;
    assert_eq!(fetch_resp.status(), StatusCode::OK);

    let fetched = body_bytes(fetch_resp).await;
    assert_eq!(fetched, payload, "binary data must survive round-trip");
}

#[tokio::test]
async fn test_fetch_returns_octet_stream_content_type() {
    let state = Arc::new(AppState {
        store: store::InputStore::new(),
        rpc_url: "http://localhost:8545".to_string(),
        engine_address: Address::ZERO,
        skip_chain_verification: true,
    });
    let app = test_app_with_state(state);

    post_upload(app.clone(), 60, b"some data".to_vec()).await;

    let signer = PrivateKeySigner::random();
    let auth = make_auth(&signer, 60).await;
    let resp = post_fetch(app.clone(), 60, &auth).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let ct = resp.headers().get("content-type").unwrap().to_str().unwrap();
    assert_eq!(ct, "application/octet-stream");
}

// ===========================================================================
// 4. File-Backed Storage (persist and recover)
// ===========================================================================

#[tokio::test]
async fn test_file_backed_storage_persists_across_stores() {
    let dir = tempfile::tempdir().unwrap();

    // Phase 1: Create a store, upload data, drop the store
    {
        let store1 = store::InputStore::with_storage_dir(dir.path().to_path_buf()).unwrap();
        store1.put(200, b"persistent payload".to_vec());
    }

    // Phase 2: Create a new store from the same directory and verify recovery
    {
        let store2 = store::InputStore::with_storage_dir(dir.path().to_path_buf()).unwrap();
        let recovered = store2.get(200);
        assert!(recovered.is_some(), "data should be recoverable from disk");
        assert_eq!(recovered.unwrap().data, b"persistent payload");
    }
}

#[tokio::test]
async fn test_file_backed_storage_via_http_round_trip() {
    let dir = tempfile::tempdir().unwrap();

    let payload = b"file-backed HTTP test".to_vec();

    // Upload via HTTP to a file-backed store
    {
        let state = Arc::new(AppState {
            store: store::InputStore::with_storage_dir(dir.path().to_path_buf()).unwrap(),
            rpc_url: "http://localhost:8545".to_string(),
            engine_address: Address::ZERO,
            skip_chain_verification: true,
        });
        let app = test_app_with_state(state);
        let resp = post_upload(app, 300, payload.clone()).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }
    // The state/store is dropped here.

    // Create a new app with a fresh store pointing at the same directory.
    // The file should still exist on disk and be recoverable.
    {
        let state = Arc::new(AppState {
            store: store::InputStore::with_storage_dir(dir.path().to_path_buf()).unwrap(),
            rpc_url: "http://localhost:8545".to_string(),
            engine_address: Address::ZERO,
            skip_chain_verification: true,
        });
        let app = test_app_with_state(state);

        let signer = PrivateKeySigner::random();
        let auth = make_auth(&signer, 300).await;
        let fetch_resp = post_fetch(app, 300, &auth).await;
        assert_eq!(fetch_resp.status(), StatusCode::OK);

        let fetched = body_bytes(fetch_resp).await;
        assert_eq!(fetched, payload, "data should survive store restart");
    }
}

#[tokio::test]
async fn test_file_backed_storage_writes_bin_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = store::InputStore::with_storage_dir(dir.path().to_path_buf()).unwrap();

    store.put(55, b"check disk file".to_vec());

    let file_path = dir.path().join("55.bin");
    assert!(file_path.exists(), "55.bin should exist on disk");
    let on_disk = std::fs::read(&file_path).unwrap();
    assert_eq!(on_disk, b"check disk file");
}

#[tokio::test]
async fn test_file_backed_storage_remove_deletes_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = store::InputStore::with_storage_dir(dir.path().to_path_buf()).unwrap();

    store.put(66, b"to be removed".to_vec());
    let file_path = dir.path().join("66.bin");
    assert!(file_path.exists());

    store.remove(66);
    assert!(!file_path.exists(), "file should be deleted after remove");
    assert!(store.get(66).is_none(), "get should return None after remove");
}

// ===========================================================================
// 5. Health Check
// ===========================================================================

#[tokio::test]
async fn test_health_reports_zero_inputs_initially() {
    let app = test_app();
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let health: HealthResponse = serde_json::from_slice(&body_bytes(resp).await).unwrap();
    assert_eq!(health.status, "healthy");
    assert_eq!(health.inputs_stored, 0);
}

#[tokio::test]
async fn test_health_reports_correct_input_count() {
    let state = Arc::new(AppState {
        store: store::InputStore::new(),
        rpc_url: "http://localhost:8545".to_string(),
        engine_address: Address::ZERO,
        skip_chain_verification: true,
    });
    let app = test_app_with_state(state);

    // Upload 3 inputs
    post_upload(app.clone(), 1, b"a".to_vec()).await;
    post_upload(app.clone(), 2, b"b".to_vec()).await;
    post_upload(app.clone(), 3, b"c".to_vec()).await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let health: HealthResponse = serde_json::from_slice(&body_bytes(resp).await).unwrap();
    assert_eq!(health.inputs_stored, 3);
}

// ===========================================================================
// 6. Edge Cases
// ===========================================================================

#[tokio::test]
async fn test_fetch_with_malformed_json_returns_error() {
    let state = Arc::new(AppState {
        store: store::InputStore::new(),
        rpc_url: "http://localhost:8545".to_string(),
        engine_address: Address::ZERO,
        skip_chain_verification: true,
    });
    let app = test_app_with_state(state);

    post_upload(app.clone(), 42, b"data".to_vec()).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/inputs/42")
                .header("content-type", "application/json")
                .body(Body::from(b"this is not json".to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Axum returns 422 Unprocessable Entity for JSON parse failures
    assert!(
        resp.status() == StatusCode::BAD_REQUEST
            || resp.status() == StatusCode::UNPROCESSABLE_ENTITY,
        "expected 400 or 422, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn test_different_signers_can_fetch_same_input() {
    let state = Arc::new(AppState {
        store: store::InputStore::new(),
        rpc_url: "http://localhost:8545".to_string(),
        engine_address: Address::ZERO,
        skip_chain_verification: true,
    });
    let app = test_app_with_state(state);

    post_upload(app.clone(), 42, b"shared input".to_vec()).await;

    // Two different signers can both fetch (since on-chain check is skipped)
    let signer_a = PrivateKeySigner::random();
    let auth_a = make_auth(&signer_a, 42).await;
    let resp_a = post_fetch(app.clone(), 42, &auth_a).await;
    assert_eq!(resp_a.status(), StatusCode::OK);
    assert_eq!(body_bytes(resp_a).await, b"shared input");

    let signer_b = PrivateKeySigner::random();
    let auth_b = make_auth(&signer_b, 42).await;
    let resp_b = post_fetch(app.clone(), 42, &auth_b).await;
    assert_eq!(resp_b.status(), StatusCode::OK);
    assert_eq!(body_bytes(resp_b).await, b"shared input");
}

#[tokio::test]
async fn test_timestamp_at_boundary_is_accepted() {
    let state = Arc::new(AppState {
        store: store::InputStore::new(),
        rpc_url: "http://localhost:8545".to_string(),
        engine_address: Address::ZERO,
        skip_chain_verification: true,
    });
    let app = test_app_with_state(state);

    post_upload(app.clone(), 42, b"data".to_vec()).await;

    let signer = PrivateKeySigner::random();
    // Timestamp exactly 299 seconds ago should still be accepted (max is 300)
    let boundary_ts = current_timestamp() - 299;
    let auth = make_auth_with_timestamp(&signer, 42, boundary_ts).await;

    let resp = post_fetch(app.clone(), 42, &auth).await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "timestamp 299s ago should be within the 300s window"
    );
}
