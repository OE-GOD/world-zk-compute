//! Admin API endpoints for tenant management.
//!
//! All admin endpoints are protected by a separate admin key (`X-Admin-Key` header).
//! The admin key is configured via the `VERIFIER_ADMIN_KEY` environment variable.

use axum::{
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::tenant::TenantStore;

// -- Admin auth state --

/// Shared state for admin authentication.
#[derive(Clone, Debug)]
pub struct AdminAuthState {
    /// The admin API key. If empty, admin endpoints are disabled.
    pub admin_key: String,
}

impl AdminAuthState {
    pub fn new(admin_key: String) -> Self {
        Self { admin_key }
    }

    /// Returns true if admin auth is disabled (no key configured).
    pub fn is_disabled(&self) -> bool {
        self.admin_key.is_empty()
    }
}

/// Middleware that enforces admin key authentication on admin routes.
///
/// Checks the `X-Admin-Key` header. If no admin key is configured,
/// admin endpoints return 403 Forbidden (disabled by design).
pub async fn admin_auth_middleware(
    axum::extract::State(state): axum::extract::State<AdminAuthState>,
    request: Request,
    next: Next,
) -> Response {
    if state.is_disabled() {
        return (
            StatusCode::FORBIDDEN,
            Json(AdminError {
                error: "Admin API is disabled (no VERIFIER_ADMIN_KEY configured)".to_string(),
            }),
        )
            .into_response();
    }

    let key = request
        .headers()
        .get("x-admin-key")
        .and_then(|v| v.to_str().ok());

    match key {
        Some(k) if k == state.admin_key => next.run(request).await,
        Some(_) => (
            StatusCode::UNAUTHORIZED,
            Json(AdminError {
                error: "Invalid admin key".to_string(),
            }),
        )
            .into_response(),
        None => (
            StatusCode::UNAUTHORIZED,
            Json(AdminError {
                error: "Missing X-Admin-Key header".to_string(),
            }),
        )
            .into_response(),
    }
}

// -- Request/Response types --

#[derive(Deserialize)]
pub struct CreateTenantRequest {
    /// Optional explicit tenant ID. If not provided, one is generated.
    #[serde(default)]
    pub id: Option<String>,
    /// Display name for the tenant (required).
    pub name: String,
    /// Rate limit in requests per minute. Default: 60.
    #[serde(default = "default_rate_limit")]
    pub rate_limit_rpm: u32,
}

fn default_rate_limit() -> u32 {
    60
}

#[derive(Serialize)]
pub struct CreateTenantResponse {
    pub id: String,
    pub name: String,
    pub api_key: String,
    pub rate_limit_rpm: u32,
    pub created_at: String,
}

#[derive(Serialize)]
pub struct TenantListResponse {
    pub tenants: Vec<TenantInfo>,
    pub total: usize,
    pub active: usize,
}

#[derive(Serialize)]
pub struct TenantInfo {
    pub id: String,
    pub name: String,
    pub active: bool,
    pub rate_limit_rpm: u32,
    pub total_requests: u64,
    pub created_at: String,
    pub last_request_at: Option<String>,
}

#[derive(Serialize)]
pub struct UsageResponse {
    pub tenant_id: String,
    pub name: String,
    pub total_requests: u64,
    pub last_request_at: Option<String>,
    pub rate_limit_rpm: u32,
    pub active: bool,
}

#[derive(Serialize)]
pub struct RevokeResponse {
    pub id: String,
    pub status: &'static str,
}

#[derive(Serialize)]
pub struct AdminError {
    pub error: String,
}

// -- Handlers --

/// POST /admin/tenants -- Create a new tenant.
///
/// Returns the generated API key. This is the only time the key is returned.
pub async fn create_tenant(
    State(store): State<Arc<TenantStore>>,
    Json(req): Json<CreateTenantRequest>,
) -> Result<(StatusCode, Json<CreateTenantResponse>), (StatusCode, Json<AdminError>)> {
    if req.name.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(AdminError {
                error: "name is required".into(),
            }),
        ));
    }

    let result = if let Some(ref id) = req.id {
        if id.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(AdminError {
                    error: "id must not be empty if provided".into(),
                }),
            ));
        }
        store.create_with_id(id, &req.name, req.rate_limit_rpm)
    } else {
        store.create(&req.name, req.rate_limit_rpm)
    };

    match result {
        Ok(tenant) => Ok((
            StatusCode::CREATED,
            Json(CreateTenantResponse {
                id: tenant.id,
                name: tenant.name,
                api_key: tenant.api_key,
                rate_limit_rpm: tenant.rate_limit_rpm,
                created_at: tenant.created_at,
            }),
        )),
        Err(e) => Err((StatusCode::CONFLICT, Json(AdminError { error: e }))),
    }
}

/// GET /admin/tenants -- List all tenants.
pub async fn list_tenants(
    State(store): State<Arc<TenantStore>>,
) -> Json<TenantListResponse> {
    let tenants = store.list();
    let total = tenants.len();
    let active = tenants.iter().filter(|t| t.active).count();
    let infos = tenants
        .into_iter()
        .map(|t| TenantInfo {
            id: t.id,
            name: t.name,
            active: t.active,
            rate_limit_rpm: t.rate_limit_rpm,
            total_requests: t.total_requests,
            created_at: t.created_at,
            last_request_at: t.last_request_at,
        })
        .collect();
    Json(TenantListResponse {
        tenants: infos,
        total,
        active,
    })
}

/// DELETE /admin/tenants/:id -- Revoke a tenant.
pub async fn revoke_tenant(
    State(store): State<Arc<TenantStore>>,
    Path(tenant_id): Path<String>,
) -> Result<Json<RevokeResponse>, (StatusCode, Json<AdminError>)> {
    if store.revoke(&tenant_id) {
        Ok(Json(RevokeResponse {
            id: tenant_id,
            status: "revoked",
        }))
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(AdminError {
                error: format!("tenant '{}' not found", tenant_id),
            }),
        ))
    }
}

/// GET /admin/tenants/:id/usage -- Get tenant usage stats.
pub async fn tenant_usage(
    State(store): State<Arc<TenantStore>>,
    Path(tenant_id): Path<String>,
) -> Result<Json<UsageResponse>, (StatusCode, Json<AdminError>)> {
    match store.usage(&tenant_id) {
        Some(usage) => Ok(Json(UsageResponse {
            tenant_id: usage.tenant_id,
            name: usage.name,
            total_requests: usage.total_requests,
            last_request_at: usage.last_request_at,
            rate_limit_rpm: usage.rate_limit_rpm,
            active: usage.active,
        })),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(AdminError {
                error: format!("tenant '{}' not found", tenant_id),
            }),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_rate_limit() {
        assert_eq!(default_rate_limit(), 60);
    }

    #[test]
    fn test_admin_auth_state() {
        let state = AdminAuthState::new("secret".into());
        assert!(!state.is_disabled());

        let empty = AdminAuthState::new(String::new());
        assert!(empty.is_disabled());
    }
}
