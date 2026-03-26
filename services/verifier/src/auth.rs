use std::sync::Arc;

use axum::{
    extract::Request,
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

/// Shared auth state injected into the middleware.
#[derive(Clone, Debug)]
pub struct AuthState {
    /// Allowed API keys. Empty means auth is disabled.
    pub api_keys: Arc<Vec<String>>,
}

#[derive(Serialize)]
struct AuthError {
    error: String,
}

impl AuthState {
    pub fn new(api_keys: Vec<String>) -> Self {
        Self {
            api_keys: Arc::new(api_keys),
        }
    }

    /// Returns true if auth is disabled (no keys configured).
    pub fn is_open(&self) -> bool {
        self.api_keys.is_empty()
    }

    /// Validates the API key. Returns the key if valid, or None.
    pub fn validate_key(&self, key: &str) -> bool {
        self.api_keys.iter().any(|k| k == key)
    }
}

/// Extract the API key identifier from the request, or the client IP as fallback.
/// Used by rate limiting to identify the caller.
pub fn extract_client_id(headers: &HeaderMap) -> String {
    if let Some(key) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        format!("key:{key}")
    } else {
        // Fall back to forwarded IP or a default
        headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .map(|s| {
                // Take the first IP if multiple are present
                s.split(',').next().unwrap_or("unknown").trim().to_string()
            })
            .map(|ip| format!("ip:{ip}"))
            .unwrap_or_else(|| "ip:unknown".to_string())
    }
}

/// Axum middleware function for API key authentication.
///
/// - If `AuthState` has no keys configured, all requests pass through.
/// - Requests to `/health` always pass through (no auth required).
/// - All other requests must include a valid `X-API-Key` header.
pub async fn auth_middleware(
    axum::extract::State(state): axum::extract::State<AuthState>,
    request: Request,
    next: Next,
) -> Response {
    // /health is always public
    if request.uri().path() == "/health" {
        return next.run(request).await;
    }

    // If no API keys configured, auth is disabled
    if state.is_open() {
        return next.run(request).await;
    }

    // Check X-API-Key header
    let key = request
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok());

    match key {
        Some(k) if state.validate_key(k) => next.run(request).await,
        Some(_) => {
            // Key present but invalid
            (
                StatusCode::UNAUTHORIZED,
                Json(AuthError {
                    error: "Invalid API key".to_string(),
                }),
            )
                .into_response()
        }
        None => {
            // No key provided
            (
                StatusCode::UNAUTHORIZED,
                Json(AuthError {
                    error: "Missing X-API-Key header".to_string(),
                }),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_state_open() {
        let state = AuthState::new(vec![]);
        assert!(state.is_open());
    }

    #[test]
    fn test_auth_state_with_keys() {
        let state = AuthState::new(vec!["key1".into(), "key2".into()]);
        assert!(!state.is_open());
        assert!(state.validate_key("key1"));
        assert!(state.validate_key("key2"));
        assert!(!state.validate_key("key3"));
    }

    #[test]
    fn test_extract_client_id_from_api_key() {
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", "my-secret-key".parse().unwrap());
        assert_eq!(extract_client_id(&headers), "key:my-secret-key");
    }

    #[test]
    fn test_extract_client_id_from_ip() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "1.2.3.4, 5.6.7.8".parse().unwrap());
        assert_eq!(extract_client_id(&headers), "ip:1.2.3.4");
    }

    #[test]
    fn test_extract_client_id_unknown() {
        let headers = HeaderMap::new();
        assert_eq!(extract_client_id(&headers), "ip:unknown");
    }
}
