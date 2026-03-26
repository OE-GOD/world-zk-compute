use std::sync::Arc;

use axum::{
    extract::Request,
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

use crate::tenant::TenantStore;

/// Shared auth state injected into the middleware.
///
/// Supports two modes:
/// 1. **Legacy mode**: A static list of API keys (`api_keys`).
/// 2. **Tenant mode**: Dynamic lookup via `TenantStore`.
///
/// If both are configured, the middleware checks legacy keys first, then tenants.
/// If neither is configured, auth is disabled (open access).
#[derive(Clone)]
pub struct AuthState {
    /// Allowed API keys (legacy). Empty means legacy auth is disabled.
    pub api_keys: Arc<Vec<String>>,
    /// Tenant store for dynamic API key lookup.
    pub tenant_store: Option<Arc<TenantStore>>,
}

#[derive(Serialize)]
struct AuthError {
    error: String,
}

impl AuthState {
    pub fn new(api_keys: Vec<String>) -> Self {
        Self {
            api_keys: Arc::new(api_keys),
            tenant_store: None,
        }
    }

    pub fn with_tenant_store(mut self, store: Arc<TenantStore>) -> Self {
        self.tenant_store = Some(store);
        self
    }

    /// Returns true if auth is disabled (no legacy keys AND no tenant store with tenants).
    pub fn is_open(&self) -> bool {
        if !self.api_keys.is_empty() {
            return false;
        }
        match &self.tenant_store {
            Some(store) => store.active_count() == 0,
            None => true,
        }
    }

    /// Validates the API key. Checks legacy keys first, then tenant store.
    /// Returns the tenant ID if found via tenant store, or "legacy" for static keys.
    pub fn validate_key(&self, key: &str) -> Option<String> {
        // Check legacy keys first
        if self.api_keys.iter().any(|k| k == key) {
            return Some("legacy".to_string());
        }
        // Check tenant store
        if let Some(store) = &self.tenant_store {
            if let Some(tenant) = store.get_by_key(key) {
                return Some(tenant.id);
            }
        }
        None
    }
}

impl std::fmt::Debug for AuthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthState")
            .field("legacy_keys", &self.api_keys.len())
            .field(
                "tenant_store",
                &self.tenant_store.as_ref().map(|s| s.count()),
            )
            .finish()
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
/// - If `AuthState` has no keys and no tenants configured, all requests pass through.
/// - Requests to `/health` always pass through (no auth required).
/// - All other requests must include a valid `X-API-Key` header.
/// - When a tenant key is validated, the tenant's request count is incremented.
pub async fn auth_middleware(
    axum::extract::State(state): axum::extract::State<AuthState>,
    request: Request,
    next: Next,
) -> Response {
    // /health is always public
    if request.uri().path() == "/health" {
        return next.run(request).await;
    }

    // If no API keys configured (neither legacy nor tenant), auth is disabled
    if state.is_open() {
        return next.run(request).await;
    }

    // Check X-API-Key header
    let key = request
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok());

    match key {
        Some(k) => match state.validate_key(k) {
            Some(tenant_id) => {
                // Record the request if from a tenant (not legacy)
                if tenant_id != "legacy" {
                    if let Some(store) = &state.tenant_store {
                        store.record_request(k);
                    }
                }
                next.run(request).await
            }
            None => (
                StatusCode::UNAUTHORIZED,
                Json(AuthError {
                    error: "Invalid API key".to_string(),
                }),
            )
                .into_response(),
        },
        None => (
            StatusCode::UNAUTHORIZED,
            Json(AuthError {
                error: "Missing X-API-Key header".to_string(),
            }),
        )
            .into_response(),
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
        assert_eq!(state.validate_key("key1"), Some("legacy".to_string()));
        assert_eq!(state.validate_key("key2"), Some("legacy".to_string()));
        assert_eq!(state.validate_key("key3"), None);
    }

    #[test]
    fn test_auth_state_with_tenant_store() {
        let store = Arc::new(TenantStore::new());
        let tenant = store.create_with_id("acme", "Acme Corp", 100).unwrap();

        let state = AuthState::new(vec![]).with_tenant_store(store);
        assert!(!state.is_open());

        let result = state.validate_key(&tenant.api_key);
        assert_eq!(result, Some("acme".to_string()));

        assert_eq!(state.validate_key("nonexistent"), None);
    }

    #[test]
    fn test_auth_state_legacy_and_tenant() {
        let store = Arc::new(TenantStore::new());
        let tenant = store.create_with_id("acme", "Acme Corp", 100).unwrap();

        let state = AuthState::new(vec!["legacy-key".into()]).with_tenant_store(store);
        assert!(!state.is_open());

        // Legacy key works
        assert_eq!(state.validate_key("legacy-key"), Some("legacy".to_string()));

        // Tenant key works
        assert_eq!(
            state.validate_key(&tenant.api_key),
            Some("acme".to_string())
        );

        // Unknown key fails
        assert_eq!(state.validate_key("unknown"), None);
    }

    #[test]
    fn test_auth_state_open_with_empty_tenant_store() {
        let store = Arc::new(TenantStore::new());
        let state = AuthState::new(vec![]).with_tenant_store(store);
        // No active tenants = open
        assert!(state.is_open());
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
