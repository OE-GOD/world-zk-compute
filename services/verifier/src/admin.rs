//! Admin API endpoints for tenant management.

use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::tenant::TenantStore;

#[derive(Deserialize)]
pub struct CreateTenantRequest {
    pub id: String,
    pub name: String,
    #[serde(default = "default_rate_limit")]
    pub rate_limit: u32,
}

fn default_rate_limit() -> u32 {
    60
}

#[derive(Serialize)]
pub struct CreateTenantResponse {
    pub id: String,
    pub api_key: String,
    pub rate_limit: u32,
}

#[derive(Serialize)]
pub struct TenantListResponse {
    pub tenants: Vec<TenantInfo>,
}

#[derive(Serialize)]
pub struct TenantInfo {
    pub id: String,
    pub name: String,
    pub active: bool,
    pub rate_limit: u32,
    pub total_requests: u64,
}

#[derive(Serialize)]
pub struct AdminError {
    pub error: String,
}

pub async fn create_tenant(
    State(store): State<Arc<TenantStore>>,
    Json(req): Json<CreateTenantRequest>,
) -> Result<Json<CreateTenantResponse>, (StatusCode, Json<AdminError>)> {
    if req.id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(AdminError {
                error: "id is required".into(),
            }),
        ));
    }

    match store.register(&req.id, &req.name, req.rate_limit) {
        Ok(api_key) => Ok(Json(CreateTenantResponse {
            id: req.id,
            api_key,
            rate_limit: req.rate_limit,
        })),
        Err(e) => Err((
            StatusCode::CONFLICT,
            Json(AdminError { error: e }),
        )),
    }
}

pub async fn list_tenants(
    State(store): State<Arc<TenantStore>>,
) -> Json<TenantListResponse> {
    let tenants = store
        .list()
        .into_iter()
        .map(|t| TenantInfo {
            id: t.id,
            name: t.name,
            active: t.active,
            rate_limit: t.rate_limit,
            total_requests: t.total_requests,
        })
        .collect();
    Json(TenantListResponse { tenants })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_rate_limit() {
        assert_eq!(default_rate_limit(), 60);
    }
}
