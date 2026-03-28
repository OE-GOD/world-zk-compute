//! Webhook notifications -- notify external URLs when proof events occur.
//!
//! Banks and other consumers register callback URLs to receive POST
//! notifications for events such as `proof.created`, `proof.verified`, and
//! `proof.failed`. Each delivery attempt includes an HMAC-SHA256 signature in
//! the `X-Webhook-Signature` header when a `secret` is configured, allowing
//! receivers to verify authenticity.
//!
//! Delivery uses fire-and-forget spawned tasks with exponential backoff retry
//! (up to 3 attempts).
//!
//! # Endpoints
//!
//! | Method   | Path                | Description              |
//! |----------|---------------------|--------------------------|
//! | `POST`   | `/v1/webhooks`      | Register a webhook       |
//! | `GET`    | `/v1/webhooks`      | List registered webhooks |
//! | `DELETE` | `/v1/webhooks/{id}` | Remove a webhook         |

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::sync::RwLock;

type HmacSha256 = Hmac<Sha256>;

/// Maximum number of delivery attempts per webhook event.
const MAX_ATTEMPTS: u32 = 3;

/// HTTP request timeout for each delivery attempt.
const DELIVERY_TIMEOUT_SECS: u64 = 10;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A registered webhook configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookRegistration {
    pub id: String,
    pub url: String,
    pub events: Vec<String>,
    /// HMAC-SHA256 signing secret. When present, each delivery includes an
    /// `X-Webhook-Signature` header with `sha256=<hex>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    pub active: bool,
}

/// Payload delivered to the webhook URL.
#[derive(Debug, Clone, Serialize)]
pub struct WebhookPayload {
    pub event: String,
    pub timestamp: String,
    pub data: serde_json::Value,
}

/// Request body for `POST /v1/webhooks`.
#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub url: String,
    pub events: Vec<String>,
    #[serde(default)]
    pub secret: Option<String>,
}

/// Response body for `POST /v1/webhooks`.
#[derive(Debug, Serialize)]
pub struct RegisterResponse {
    pub webhook: WebhookRegistration,
}

/// Response body for `GET /v1/webhooks`.
#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub webhooks: Vec<WebhookRegistration>,
}

/// Response body for `DELETE /v1/webhooks/{id}`.
#[derive(Debug, Serialize)]
pub struct DeleteResponse {
    pub deleted: bool,
}

/// Error response body.
#[derive(Debug, Serialize)]
pub struct WebhookError {
    pub error: String,
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// In-memory webhook registry.
///
/// Stores all registered webhooks and provides methods for CRUD operations
/// and event delivery. Uses `tokio::sync::RwLock` for async-safe access.
pub struct WebhookStore {
    hooks: RwLock<HashMap<String, WebhookRegistration>>,
    client: reqwest::Client,
}

impl WebhookStore {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(DELIVERY_TIMEOUT_SECS))
            .build()
            .expect("failed to build webhook HTTP client");

        Self {
            hooks: RwLock::new(HashMap::new()),
            client,
        }
    }

    /// Create a new `WebhookStore` with a custom `reqwest::Client` (useful for
    /// testing with mocked servers).
    #[cfg(test)]
    pub fn with_client(client: reqwest::Client) -> Self {
        Self {
            hooks: RwLock::new(HashMap::new()),
            client,
        }
    }

    /// Register a new webhook. Returns the full registration record.
    pub async fn register(
        &self,
        url: &str,
        events: Vec<String>,
        secret: Option<String>,
    ) -> WebhookRegistration {
        let reg = WebhookRegistration {
            id: format!("wh_{}", uuid::Uuid::new_v4()),
            url: url.to_string(),
            events,
            secret,
            active: true,
        };

        let mut hooks = self.hooks.write().await;
        hooks.insert(reg.id.clone(), reg.clone());
        reg
    }

    /// List all registered webhooks.
    ///
    /// Secrets are redacted in the returned list for safety.
    pub async fn list(&self) -> Vec<WebhookRegistration> {
        let hooks = self.hooks.read().await;
        let mut out: Vec<WebhookRegistration> = hooks
            .values()
            .map(|h| {
                let mut redacted = h.clone();
                if redacted.secret.is_some() {
                    redacted.secret = Some("***".to_string());
                }
                redacted
            })
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Delete a webhook by ID. Returns `true` if it existed.
    pub async fn delete(&self, id: &str) -> bool {
        let mut hooks = self.hooks.write().await;
        hooks.remove(id).is_some()
    }

    /// Fire a webhook event. Finds all matching active hooks and spawns
    /// delivery tasks with retry logic.
    pub async fn fire(&self, event: &str, data: serde_json::Value) {
        let hooks = self.hooks.read().await;
        let matching: Vec<WebhookRegistration> = hooks
            .values()
            .filter(|h| h.active && h.events.contains(&event.to_string()))
            .cloned()
            .collect();
        drop(hooks);

        for hook in matching {
            let client = self.client.clone();
            let payload = WebhookPayload {
                event: event.to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                data: data.clone(),
            };

            tokio::spawn(async move {
                deliver_with_retry(&client, &hook, &payload).await;
            });
        }
    }
}

impl Default for WebhookStore {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Delivery with retry
// ---------------------------------------------------------------------------

/// Attempt to deliver a webhook payload with up to `MAX_ATTEMPTS` retries
/// using exponential backoff (1s, 2s, 4s, ...).
async fn deliver_with_retry(
    client: &reqwest::Client,
    hook: &WebhookRegistration,
    payload: &WebhookPayload,
) {
    let body = match serde_json::to_vec(payload) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(webhook_id = %hook.id, error = %e, "failed to serialize webhook payload");
            return;
        }
    };

    for attempt in 0..MAX_ATTEMPTS {
        let mut request = client
            .post(&hook.url)
            .header("Content-Type", "application/json")
            .header("X-Webhook-Event", &payload.event)
            .header("X-Webhook-Id", &hook.id);

        // Add HMAC signature if a secret is configured.
        if let Some(secret) = &hook.secret {
            if let Ok(signature) = compute_signature(secret.as_bytes(), &body) {
                request = request.header("X-Webhook-Signature", format!("sha256={signature}"));
            }
        }

        match request.body(body.clone()).send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::info!(
                    webhook_id = %hook.id,
                    url = %hook.url,
                    status = resp.status().as_u16(),
                    attempt = attempt + 1,
                    "webhook delivered"
                );
                return;
            }
            Ok(resp) => {
                tracing::warn!(
                    webhook_id = %hook.id,
                    url = %hook.url,
                    status = resp.status().as_u16(),
                    attempt = attempt + 1,
                    "webhook delivery got non-success status"
                );
            }
            Err(e) => {
                tracing::warn!(
                    webhook_id = %hook.id,
                    url = %hook.url,
                    error = %e,
                    attempt = attempt + 1,
                    "webhook delivery failed"
                );
            }
        }

        // Exponential backoff before retrying (skip sleep on last attempt).
        if attempt + 1 < MAX_ATTEMPTS {
            let delay = std::time::Duration::from_secs(2u64.pow(attempt));
            tokio::time::sleep(delay).await;
        }
    }

    tracing::error!(
        webhook_id = %hook.id,
        url = %hook.url,
        "webhook delivery failed after {MAX_ATTEMPTS} attempts"
    );
}

/// Compute HMAC-SHA256 of `body` using `secret`, returning the hex-encoded
/// digest.
fn compute_signature(secret: &[u8], body: &[u8]) -> Result<String, hmac::digest::InvalidLength> {
    let mut mac = HmacSha256::new_from_slice(secret)?;
    mac.update(body);
    let result = mac.finalize();
    Ok(hex::encode(result.into_bytes()))
}

// ---------------------------------------------------------------------------
// Axum handlers
// ---------------------------------------------------------------------------

/// `POST /v1/webhooks` -- register a new webhook.
pub async fn register_handler(
    State(store): State<Arc<WebhookStore>>,
    Json(req): Json<RegisterRequest>,
) -> Result<(StatusCode, Json<RegisterResponse>), (StatusCode, Json<WebhookError>)> {
    // Validate URL.
    if req.url.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(WebhookError {
                error: "url is required".to_string(),
            }),
        ));
    }

    if !req.url.starts_with("http://") && !req.url.starts_with("https://") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(WebhookError {
                error: "url must start with http:// or https://".to_string(),
            }),
        ));
    }

    // Validate events.
    let valid_events = ["proof.created", "proof.verified", "proof.failed"];
    if req.events.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(WebhookError {
                error: "events must not be empty".to_string(),
            }),
        ));
    }
    for ev in &req.events {
        if !valid_events.contains(&ev.as_str()) {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(WebhookError {
                    error: format!(
                        "invalid event: {ev}. Valid events: {}",
                        valid_events.join(", ")
                    ),
                }),
            ));
        }
    }

    let webhook = store.register(&req.url, req.events, req.secret).await;

    Ok((StatusCode::CREATED, Json(RegisterResponse { webhook })))
}

/// `GET /v1/webhooks` -- list all registered webhooks.
pub async fn list_handler(
    State(store): State<Arc<WebhookStore>>,
) -> Json<ListResponse> {
    let webhooks = store.list().await;
    Json(ListResponse { webhooks })
}

/// `DELETE /v1/webhooks/{id}` -- remove a registered webhook.
pub async fn delete_handler(
    State(store): State<Arc<WebhookStore>>,
    Path(id): Path<String>,
) -> Result<Json<DeleteResponse>, (StatusCode, Json<WebhookError>)> {
    let deleted = store.delete(&id).await;
    if deleted {
        Ok(Json(DeleteResponse { deleted: true }))
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(WebhookError {
                error: format!("webhook not found: {id}"),
            }),
        ))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::routing::{delete, get, post};
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Build a minimal router with webhook endpoints.
    fn webhook_app(store: Arc<WebhookStore>) -> Router {
        Router::new()
            .route("/v1/webhooks", post(register_handler).get(list_handler))
            .route("/v1/webhooks/{id}", delete(delete_handler))
            .with_state(store)
    }

    // -- Registration tests --

    #[tokio::test]
    async fn test_register_webhook() {
        let store = Arc::new(WebhookStore::new());
        let app = webhook_app(store);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/v1/webhooks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_string(&serde_json::json!({
                            "url": "https://example.com/hook",
                            "events": ["proof.created", "proof.verified"]
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::CREATED);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: RegisterResponse = serde_json::from_slice(&body).unwrap();
        assert!(json.webhook.id.starts_with("wh_"));
        assert_eq!(json.webhook.url, "https://example.com/hook");
        assert_eq!(json.webhook.events.len(), 2);
        assert!(json.webhook.active);
    }

    #[tokio::test]
    async fn test_register_webhook_with_secret() {
        let store = Arc::new(WebhookStore::new());
        let app = webhook_app(store);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/v1/webhooks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_string(&serde_json::json!({
                            "url": "https://example.com/hook",
                            "events": ["proof.failed"],
                            "secret": "my-secret-key"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: RegisterResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.webhook.secret.as_deref(), Some("my-secret-key"));
    }

    #[tokio::test]
    async fn test_register_webhook_invalid_url() {
        let store = Arc::new(WebhookStore::new());
        let app = webhook_app(store);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/v1/webhooks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_string(&serde_json::json!({
                            "url": "not-a-url",
                            "events": ["proof.created"]
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_register_webhook_empty_events() {
        let store = Arc::new(WebhookStore::new());
        let app = webhook_app(store);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/v1/webhooks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_string(&serde_json::json!({
                            "url": "https://example.com/hook",
                            "events": []
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_register_webhook_invalid_event() {
        let store = Arc::new(WebhookStore::new());
        let app = webhook_app(store);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/v1/webhooks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_string(&serde_json::json!({
                            "url": "https://example.com/hook",
                            "events": ["proof.created", "invalid.event"]
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // -- List tests --

    #[tokio::test]
    async fn test_list_webhooks_empty() {
        let store = Arc::new(WebhookStore::new());
        let app = webhook_app(store);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/v1/webhooks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: ListResponse = serde_json::from_slice(&body).unwrap();
        assert!(json.webhooks.is_empty());
    }

    #[tokio::test]
    async fn test_list_webhooks_redacts_secrets() {
        let store = Arc::new(WebhookStore::new());
        store
            .register(
                "https://example.com/hook",
                vec!["proof.created".to_string()],
                Some("super-secret".to_string()),
            )
            .await;

        let app = webhook_app(store);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/v1/webhooks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: ListResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.webhooks.len(), 1);
        // Secret must be redacted.
        assert_eq!(json.webhooks[0].secret.as_deref(), Some("***"));
    }

    // -- Delete tests --

    #[tokio::test]
    async fn test_delete_webhook() {
        let store = Arc::new(WebhookStore::new());
        let reg = store
            .register(
                "https://example.com/hook",
                vec!["proof.created".to_string()],
                None,
            )
            .await;

        let app = webhook_app(store);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method("DELETE")
                    .uri(&format!("/v1/webhooks/{}", reg.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: DeleteResponse = serde_json::from_slice(&body).unwrap();
        assert!(json.deleted);
    }

    #[tokio::test]
    async fn test_delete_webhook_not_found() {
        let store = Arc::new(WebhookStore::new());
        let app = webhook_app(store);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method("DELETE")
                    .uri("/v1/webhooks/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // -- Store unit tests --

    #[tokio::test]
    async fn test_store_register_and_list() {
        let store = WebhookStore::new();
        store
            .register(
                "https://a.com/hook",
                vec!["proof.created".to_string()],
                None,
            )
            .await;
        store
            .register(
                "https://b.com/hook",
                vec!["proof.verified".to_string()],
                None,
            )
            .await;

        let list = store.list().await;
        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn test_store_delete() {
        let store = WebhookStore::new();
        let reg = store
            .register(
                "https://a.com/hook",
                vec!["proof.created".to_string()],
                None,
            )
            .await;

        assert!(store.delete(&reg.id).await);
        assert!(!store.delete(&reg.id).await); // second delete returns false
        assert!(store.list().await.is_empty());
    }

    // -- Fire + delivery tests --

    #[tokio::test]
    async fn test_fire_delivers_to_matching_hooks() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&mock_server)
            .await;

        let store = Arc::new(WebhookStore::new());
        store
            .register(
                &format!("{}/hook", mock_server.uri()),
                vec!["proof.created".to_string()],
                None,
            )
            .await;

        store
            .fire(
                "proof.created",
                serde_json::json!({"proof_id": "p-123"}),
            )
            .await;

        // Allow the spawned task time to deliver.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // wiremock will panic in drop if expectation was not met.
    }

    #[tokio::test]
    async fn test_fire_skips_non_matching_events() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&mock_server)
            .await;

        let store = Arc::new(WebhookStore::new());
        store
            .register(
                &format!("{}/hook", mock_server.uri()),
                vec!["proof.created".to_string()],
                None,
            )
            .await;

        // Fire a different event type.
        store
            .fire(
                "proof.verified",
                serde_json::json!({"proof_id": "p-456"}),
            )
            .await;

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    #[tokio::test]
    async fn test_fire_retries_on_failure() {
        let mock_server = MockServer::start().await;
        // First two calls return 500, third returns 200.
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(2)
            .mount(&mock_server)
            .await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock_server)
            .await;

        let store = Arc::new(WebhookStore::new());
        store
            .register(
                &format!("{}/hook", mock_server.uri()),
                vec!["proof.failed".to_string()],
                None,
            )
            .await;

        store
            .fire(
                "proof.failed",
                serde_json::json!({"error": "timeout"}),
            )
            .await;

        // Wait for retries (backoff: 1s, 2s).
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;

        // Verify 3 requests were made (2 failures + 1 success).
        let received = mock_server.received_requests().await.unwrap();
        assert_eq!(received.len(), 3);
    }

    #[tokio::test]
    async fn test_fire_includes_hmac_signature() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&mock_server)
            .await;

        let store = Arc::new(WebhookStore::new());
        store
            .register(
                &format!("{}/hook", mock_server.uri()),
                vec!["proof.created".to_string()],
                Some("test-secret".to_string()),
            )
            .await;

        store
            .fire(
                "proof.created",
                serde_json::json!({"proof_id": "p-789"}),
            )
            .await;

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        let requests = mock_server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);

        let sig_header = requests[0]
            .headers
            .get("X-Webhook-Signature")
            .expect("X-Webhook-Signature header must be present");

        let sig_str = sig_header.to_str().unwrap();
        assert!(
            sig_str.starts_with("sha256="),
            "signature must start with sha256="
        );

        // Verify the signature matches what we expect.
        let body = &requests[0].body;
        let expected_sig = compute_signature(b"test-secret", body).unwrap();
        assert_eq!(sig_str, format!("sha256={expected_sig}"));
    }

    // -- HMAC unit test --

    #[test]
    fn test_compute_signature() {
        let sig = compute_signature(b"secret", b"hello").unwrap();
        // Known HMAC-SHA256 of "hello" with key "secret".
        assert_eq!(sig.len(), 64); // 32 bytes = 64 hex chars
        assert!(!sig.is_empty());

        // Same input should give same output.
        let sig2 = compute_signature(b"secret", b"hello").unwrap();
        assert_eq!(sig, sig2);

        // Different secret should give different output.
        let sig3 = compute_signature(b"other-secret", b"hello").unwrap();
        assert_ne!(sig, sig3);
    }
}
