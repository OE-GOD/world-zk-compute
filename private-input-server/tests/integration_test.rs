use alloy::primitives::Address;
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use private_input_server::{
    auth::AuthRequest, build_app, store, AppState, ErrorResponse, HealthResponse, UploadResponse,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tower::util::ServiceExt;

fn test_app() -> (axum::Router, Arc<AppState>) {
    let state = Arc::new(AppState {
        store: store::InputStore::new(),
        rpc_url: "http://localhost:8545".to_string(),
        engine_address: Address::ZERO,
        skip_chain_verification: true,
    });
    (build_app(state.clone()), state)
}

fn upload_request(request_id: u64, body: &[u8]) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/inputs/{}/upload", request_id))
        .header("content-type", "application/octet-stream")
        .body(Body::from(body.to_vec()))
        .unwrap()
}

async fn make_auth_request(
    signer: &PrivateKeySigner,
    request_id: u64,
) -> AuthRequest {
    let address = signer.address();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let message = format!(
        "world-zk-compute:fetch-input:{}:{}:{}",
        request_id,
        address.to_checksum(None),
        now
    );
    let sig = signer.sign_message(message.as_bytes()).await.unwrap();
    AuthRequest {
        request_id,
        prover_address: address.to_checksum(None),
        timestamp: now,
        signature: format!("0x{}", hex::encode(sig.as_bytes())),
    }
}

fn fetch_request(request_id: u64, auth: &AuthRequest) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/inputs/{}", request_id))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(auth).unwrap()))
        .unwrap()
}

async fn collect_body(resp: axum::response::Response) -> Vec<u8> {
    resp.into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec()
}

#[tokio::test]
async fn test_upload_returns_digest() {
    let (app, _) = test_app();
    let data = b"test payload for digest verification";
    let resp = app.oneshot(upload_request(42, data)).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = collect_body(resp).await;
    let upload: UploadResponse = serde_json::from_slice(&body).unwrap();

    assert_eq!(upload.request_id, 42);
    assert_eq!(upload.size, data.len());

    let expected_digest = {
        let mut h = Sha256::new();
        h.update(data);
        format!("0x{}", hex::encode(h.finalize()))
    };
    assert_eq!(upload.digest, expected_digest);
}

#[tokio::test]
async fn test_upload_overwrites_previous() {
    let (app, state) = test_app();

    let _ = app.clone().oneshot(upload_request(7, b"first")).await.unwrap();
    let _ = app.oneshot(upload_request(7, b"second")).await.unwrap();

    let signer = PrivateKeySigner::random();
    let auth = make_auth_request(&signer, 7).await;
    let app2 = build_app(state);
    let resp = app2.oneshot(fetch_request(7, &auth)).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = collect_body(resp).await;
    assert_eq!(body, b"second");
}

#[tokio::test]
async fn test_fetch_request_id_mismatch() {
    let (app, state) = test_app();
    let _ = app.oneshot(upload_request(10, b"data")).await.unwrap();

    let signer = PrivateKeySigner::random();
    let auth = make_auth_request(&signer, 99).await;

    let app2 = build_app(state);
    let resp = app2.oneshot(fetch_request(10, &auth)).await.unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = collect_body(resp).await;
    let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
    assert!(err.error.contains("doesn't match"));
}

#[tokio::test]
async fn test_fetch_with_expired_timestamp() {
    let (app, state) = test_app();
    let _ = app.oneshot(upload_request(5, b"data")).await.unwrap();

    let signer = PrivateKeySigner::random();
    let address = signer.address();
    let old_timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - 600; // 10 minutes ago

    let message = format!(
        "world-zk-compute:fetch-input:{}:{}:{}",
        5,
        address.to_checksum(None),
        old_timestamp
    );
    let sig = signer.sign_message(message.as_bytes()).await.unwrap();
    let auth = AuthRequest {
        request_id: 5,
        prover_address: address.to_checksum(None),
        timestamp: old_timestamp,
        signature: format!("0x{}", hex::encode(sig.as_bytes())),
    };

    let app2 = build_app(state);
    let resp = app2.oneshot(fetch_request(5, &auth)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_fetch_wrong_signer() {
    let (app, state) = test_app();
    let _ = app.oneshot(upload_request(5, b"data")).await.unwrap();

    let signer1 = PrivateKeySigner::random();
    let signer2 = PrivateKeySigner::random();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Sign with signer1 but claim to be signer2
    let message = format!(
        "world-zk-compute:fetch-input:{}:{}:{}",
        5,
        signer2.address().to_checksum(None),
        now
    );
    let sig = signer1.sign_message(message.as_bytes()).await.unwrap();
    let auth = AuthRequest {
        request_id: 5,
        prover_address: signer2.address().to_checksum(None),
        timestamp: now,
        signature: format!("0x{}", hex::encode(sig.as_bytes())),
    };

    let app2 = build_app(state);
    let resp = app2.oneshot(fetch_request(5, &auth)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body = collect_body(resp).await;
    let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
    assert!(err.error.contains("mismatch"));
}

#[tokio::test]
async fn test_health_reflects_store_size() {
    let (app, state) = test_app();

    for id in [1, 2, 3] {
        let _ = app
            .clone()
            .oneshot(upload_request(id, format!("data-{}", id).as_bytes()))
            .await
            .unwrap();
    }

    let health_app = build_app(state);
    let resp = health_app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = collect_body(resp).await;
    let health: HealthResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(health.inputs_stored, 3);
}

#[tokio::test]
async fn test_upload_empty_body_rejected() {
    let (app, _) = test_app();
    let resp = app.oneshot(upload_request(1, b"")).await.unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = collect_body(resp).await;
    let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(err.error, "Empty body");
}

#[tokio::test]
async fn test_concurrent_uploads() {
    let (_, state) = test_app();

    let mut handles = Vec::new();
    for id in 0..10u64 {
        let st = state.clone();
        handles.push(tokio::spawn(async move {
            let app = build_app(st);
            let data = format!("payload-{}", id);
            let resp = app
                .oneshot(upload_request(id, data.as_bytes()))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    assert_eq!(state.store.len(), 10);
}

#[tokio::test]
async fn test_file_backed_store_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let data = b"persistent integration test data";

    // Upload with first store instance
    {
        let store = store::InputStore::with_storage_dir(dir.path().to_path_buf()).unwrap();
        let state = Arc::new(AppState {
            store,
            rpc_url: "http://localhost:8545".to_string(),
            engine_address: Address::ZERO,
            skip_chain_verification: true,
        });
        let app = build_app(state);
        let resp = app.oneshot(upload_request(55, data)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // Create new store from same directory and fetch
    let store2 = store::InputStore::with_storage_dir(dir.path().to_path_buf()).unwrap();
    let state2 = Arc::new(AppState {
        store: store2,
        rpc_url: "http://localhost:8545".to_string(),
        engine_address: Address::ZERO,
        skip_chain_verification: true,
    });

    let signer = PrivateKeySigner::random();
    let auth = make_auth_request(&signer, 55).await;
    let app2 = build_app(state2);
    let resp = app2.oneshot(fetch_request(55, &auth)).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = collect_body(resp).await;
    assert_eq!(body, data);
}
