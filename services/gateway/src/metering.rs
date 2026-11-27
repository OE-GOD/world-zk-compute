//! Per-tenant usage metering for billing and capacity planning.
//!
//! Tracks proofs generated, proofs verified, and total requests per tenant per
//! day. A tenant is identified by a fingerprint of its API key (see
//! [`tenant_id`], which the audit logger uses too), never by the key itself.
//! Anonymous requests are grouped under `"anonymous"`.
//!
//! The in-memory store is suitable for single-instance deployments. For HA
//! setups, replace the backing `HashMap` with a durable store (e.g. Redis or
//! Postgres).
//!
//! # Admin endpoint
//!
//! `GET /admin/usage?tenant=X&from=2026-01-01&to=2026-03-27` returns filtered
//! usage records as JSON.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Query, Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
    Json,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

/// Tenant id of requests that carry no API key.
const ANONYMOUS: &str = "anonymous";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single usage record for one tenant on one calendar day.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct UsageRecord {
    pub tenant_id: String,
    pub date: String, // YYYY-MM-DD
    pub proofs_generated: u64,
    pub proofs_verified: u64,
    pub total_requests: u64,
}

/// Aggregate totals across all tenants and dates.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct UsageTotals {
    pub proofs_generated: u64,
    pub proofs_verified: u64,
    pub total_requests: u64,
    pub unique_tenants: usize,
}

/// The type of operation being metered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    ProofGenerated,
    ProofVerified,
    Other,
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// In-memory metering store keyed by `(tenant_id, date)`.
#[derive(Debug)]
pub struct MeteringStore {
    records: RwLock<HashMap<(String, String), UsageRecord>>,
}

impl Default for MeteringStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MeteringStore {
    pub fn new() -> Self {
        Self {
            records: RwLock::new(HashMap::new()),
        }
    }

    /// Record one request for the given tenant and operation.
    pub async fn record(&self, tenant_id: &str, operation: Operation) {
        let date = current_date();
        self.record_with_date(tenant_id, operation, &date).await;
    }

    /// Record with an explicit date string (useful for testing).
    pub async fn record_with_date(&self, tenant_id: &str, operation: Operation, date: &str) {
        let key = (tenant_id.to_string(), date.to_string());
        let mut records = self.records.write().await;
        let entry = records.entry(key).or_insert_with(|| UsageRecord {
            tenant_id: tenant_id.to_string(),
            date: date.to_string(),
            proofs_generated: 0,
            proofs_verified: 0,
            total_requests: 0,
        });

        entry.total_requests += 1;
        match operation {
            Operation::ProofGenerated => entry.proofs_generated += 1,
            Operation::ProofVerified => entry.proofs_verified += 1,
            Operation::Other => {}
        }
    }

    /// Query usage records, optionally filtering by tenant and date range.
    ///
    /// - `tenant_id`: if `Some`, only return records for that tenant.
    /// - `from`: inclusive lower bound on date (YYYY-MM-DD).
    /// - `to`: inclusive upper bound on date (YYYY-MM-DD).
    ///
    /// Results are sorted by `(date, tenant_id)` for deterministic output.
    pub async fn query(
        &self,
        tenant_id: Option<&str>,
        from: Option<&str>,
        to: Option<&str>,
    ) -> Vec<UsageRecord> {
        let records = self.records.read().await;
        let mut results: Vec<UsageRecord> = records
            .values()
            .filter(|r| {
                if let Some(tid) = tenant_id {
                    if r.tenant_id != tid {
                        return false;
                    }
                }
                if let Some(f) = from {
                    if r.date.as_str() < f {
                        return false;
                    }
                }
                if let Some(t) = to {
                    if r.date.as_str() > t {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();

        results.sort_by(|a, b| (&a.date, &a.tenant_id).cmp(&(&b.date, &b.tenant_id)));
        results
    }

    /// Compute aggregate totals across all records.
    #[allow(dead_code)]
    pub async fn totals(&self) -> UsageTotals {
        let records = self.records.read().await;
        let mut totals = UsageTotals {
            proofs_generated: 0,
            proofs_verified: 0,
            total_requests: 0,
            unique_tenants: 0,
        };

        let mut tenants = std::collections::HashSet::new();
        for record in records.values() {
            totals.proofs_generated += record.proofs_generated;
            totals.proofs_verified += record.proofs_verified;
            totals.total_requests += record.total_requests;
            tenants.insert(&record.tenant_id);
        }
        totals.unique_tenants = tenants.len();
        totals
    }
}

// ---------------------------------------------------------------------------
// Tenant identity
// ---------------------------------------------------------------------------

/// The tenant id of a request: the first 16 hex digits of the SHA-256 of its
/// API key, or `"anonymous"` without one.
///
/// The key is the trimmed `Authorization` value after the `Bearer` scheme (in
/// any case) and the whitespace that follows it, or the whole trimmed value
/// for a request without that scheme. A bare scheme has no key, and neither
/// has a value that is not visible ASCII because it holds obs-text (a byte
/// from 0x80 to 0xFF): both are `"anonymous"`.
///
/// This accepts more forms than `auth_middleware`, which wants exactly
/// `"Bearer "` and the key: here the scheme may be in any case, the whitespace
/// may be longer, and the scheme may be missing. The request logger runs before
/// auth, so a request that auth rejects in one of those forms is still logged
/// under the fingerprint of the key it presented, with its 401.
pub fn tenant_id(headers: &HeaderMap) -> String {
    // HTTP/1 parsing trims the whitespace at the end of a header, so a request
    // sent with "Bearer " and no key arrives as "Bearer".
    let value = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim_ascii();

    let key = match value.split_at_checked("Bearer".len()) {
        Some((scheme, rest))
            if scheme.eq_ignore_ascii_case("Bearer")
                && (rest.is_empty() || rest.starts_with(|c: char| c.is_ascii_whitespace())) =>
        {
            rest.trim_ascii()
        }
        _ => value,
    };
    if key.is_empty() {
        return ANONYMOUS.to_string();
    }

    hex::encode(&Sha256::digest(key.as_bytes())[..8])
}

// ---------------------------------------------------------------------------
// Date helper
// ---------------------------------------------------------------------------

/// Returns today's date as YYYY-MM-DD using Hinnant's algorithm (no chrono
/// needed for this single call-site, but we use chrono elsewhere anyway).
fn current_date() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

// ---------------------------------------------------------------------------
// Path classification
// ---------------------------------------------------------------------------

/// Classify an HTTP request path into a metering operation.
///
/// - `/prove` or `/v1/prove` (and subpaths) -> ProofGenerated
/// - `/verify` or `/v1/verify` or `/proofs/*/verify` -> ProofVerified
/// - Everything else -> Other
pub fn classify_path(path: &str) -> Operation {
    let trimmed = path.trim_start_matches('/');

    // Strip optional "v1/" prefix.
    let trimmed = trimmed.strip_prefix("v1/").unwrap_or(trimmed);

    let first_segment = trimmed.split('/').next().unwrap_or("");

    match first_segment {
        "prove" => Operation::ProofGenerated,
        "verify" => Operation::ProofVerified,
        "proofs" => {
            // /proofs/*/verify counts as a verification
            if trimmed.ends_with("/verify") {
                Operation::ProofVerified
            } else {
                Operation::Other
            }
        }
        _ => Operation::Other,
    }
}

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

/// Axum middleware that meters every request.
///
/// Derives the tenant identifier from the request's API key ([`tenant_id`]) and
/// increments the appropriate counters.
pub async fn metering_middleware(
    State(store): State<Arc<MeteringStore>>,
    req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();

    let tenant_id = tenant_id(req.headers());

    let operation = classify_path(&path);

    // Run the downstream handler first so we only meter completed requests.
    let response = next.run(req).await;

    // Record usage (fire-and-forget -- the write lock is fast for in-memory).
    store.record(&tenant_id, operation).await;

    response
}

// ---------------------------------------------------------------------------
// Admin endpoint
// ---------------------------------------------------------------------------

/// Query parameters for `GET /admin/usage`.
#[derive(Debug, Deserialize)]
pub struct UsageQuery {
    pub tenant: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
}

/// Response body for `GET /admin/usage`.
#[derive(Debug, Serialize, Deserialize)]
pub struct UsageResponse {
    pub records: Vec<UsageRecord>,
    pub totals: UsageTotals,
}

/// `GET /admin/usage?tenant=X&from=YYYY-MM-DD&to=YYYY-MM-DD`
///
/// Returns usage records and aggregate totals. All query parameters are
/// optional; omitting them returns all records.
pub async fn usage_handler(
    State(store): State<Arc<MeteringStore>>,
    Query(params): Query<UsageQuery>,
) -> Result<Json<UsageResponse>, (StatusCode, String)> {
    // Validate date format if provided.
    for (name, val) in [("from", &params.from), ("to", &params.to)] {
        if let Some(d) = val {
            if chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_err() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("invalid {name} date: {d} (expected YYYY-MM-DD)"),
                ));
            }
        }
    }

    let records = store
        .query(
            params.tenant.as_deref(),
            params.from.as_deref(),
            params.to.as_deref(),
        )
        .await;

    // Compute filtered totals from the returned records.
    let mut totals = UsageTotals {
        proofs_generated: 0,
        proofs_verified: 0,
        total_requests: 0,
        unique_tenants: 0,
    };
    let mut tenants = std::collections::HashSet::new();
    for r in &records {
        totals.proofs_generated += r.proofs_generated;
        totals.proofs_verified += r.proofs_verified;
        totals.total_requests += r.total_requests;
        tenants.insert(&r.tenant_id);
    }
    totals.unique_tenants = tenants.len();

    Ok(Json(UsageResponse { records, totals }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- classify_path tests --

    #[test]
    fn test_classify_prove() {
        assert_eq!(classify_path("/prove"), Operation::ProofGenerated);
        assert_eq!(
            classify_path("/prove/some-model"),
            Operation::ProofGenerated
        );
        assert_eq!(classify_path("/v1/prove"), Operation::ProofGenerated);
        assert_eq!(
            classify_path("/v1/prove/some-model"),
            Operation::ProofGenerated
        );
    }

    #[test]
    fn test_classify_verify() {
        assert_eq!(classify_path("/verify"), Operation::ProofVerified);
        assert_eq!(classify_path("/v1/verify"), Operation::ProofVerified);
        assert_eq!(classify_path("/v1/verify/hybrid"), Operation::ProofVerified);
    }

    #[test]
    fn test_classify_proofs_verify() {
        assert_eq!(
            classify_path("/proofs/abc/verify"),
            Operation::ProofVerified
        );
        assert_eq!(
            classify_path("/v1/proofs/abc/verify"),
            Operation::ProofVerified
        );
    }

    #[test]
    fn test_classify_proofs_list() {
        assert_eq!(classify_path("/proofs"), Operation::Other);
        assert_eq!(classify_path("/v1/proofs"), Operation::Other);
        assert_eq!(classify_path("/proofs/abc"), Operation::Other);
    }

    #[test]
    fn test_classify_other() {
        assert_eq!(classify_path("/health"), Operation::Other);
        assert_eq!(classify_path("/models"), Operation::Other);
        assert_eq!(classify_path("/admin/usage"), Operation::Other);
        assert_eq!(classify_path("/stats"), Operation::Other);
    }

    // -- tenant_id tests --

    /// First 16 hex digits of SHA-256("tenant-abc").
    const TENANT_ABC_ID: &str = "f9b2fe86c1fed94a";

    fn id_of(authorization: &str) -> String {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, authorization.parse().unwrap());
        tenant_id(&headers)
    }

    #[test]
    fn test_tenant_id_is_the_fingerprint_of_the_key() {
        assert_eq!(id_of("Bearer tenant-abc"), TENANT_ABC_ID);
        assert_ne!(id_of("Bearer tenant-abd"), TENANT_ABC_ID);
    }

    #[test]
    fn test_tenant_id_ignores_the_case_of_the_scheme() {
        assert_eq!(id_of("bearer tenant-abc"), TENANT_ABC_ID);
        assert_eq!(id_of("BEARER tenant-abc"), TENANT_ABC_ID);
        // The key itself is case-sensitive.
        assert_ne!(id_of("Bearer TENANT-ABC"), TENANT_ABC_ID);
    }

    #[test]
    fn test_tenant_id_without_the_bearer_scheme_hashes_the_whole_value() {
        assert_eq!(id_of("tenant-abc"), TENANT_ABC_ID);

        let basic = id_of("Basic dXNlcjpwYXNz");
        assert_eq!(basic.len(), 16);
        assert_ne!(basic, id_of("Basic dXNlcjpwYXNx"));
    }

    #[test]
    fn test_tenant_id_anonymous_without_a_key() {
        assert_eq!(tenant_id(&HeaderMap::new()), "anonymous");
        assert_eq!(id_of(""), "anonymous");
        assert_eq!(id_of("Bearer "), "anonymous");
    }

    /// A header sent with no key: the forms in which hyper hands it over from
    /// HTTP/1 (the whitespace at the end is gone), and untrimmed forms that
    /// code in the same process can build.
    #[test]
    fn test_tenant_id_anonymous_for_a_bare_scheme_as_it_arrives_over_http1() {
        for value in ["Bearer", "bearer", "BEARER", "Bearer \t ", "  bearer  "] {
            assert_eq!(id_of(value), "anonymous", "{value:?}");
        }
    }

    #[test]
    fn test_tenant_id_anonymous_for_a_value_that_is_not_visible_ascii() {
        // HeaderValue accepts obs-text, but it is not a str, so there is no
        // key: neither as a lone Latin-1 byte nor as valid UTF-8.
        for bytes in [&b"Bearer caf\xe9"[..], &b"Bearer caf\xc3\xa9"[..]] {
            let mut headers = HeaderMap::new();
            let value = axum::http::HeaderValue::from_bytes(bytes).unwrap();
            headers.insert(header::AUTHORIZATION, value);
            assert_eq!(tenant_id(&headers), "anonymous", "{bytes:?}");
        }
    }

    #[test]
    fn test_tenant_id_trims_the_key() {
        // "Bearer   tenant-abc  " arrives over HTTP/1 as "Bearer   tenant-abc".
        for value in [
            "Bearer   tenant-abc",
            "Bearer   tenant-abc  ",
            "bearer\ttenant-abc",
            "  Bearer tenant-abc",
            "  tenant-abc  ",
        ] {
            assert_eq!(id_of(value), TENANT_ABC_ID, "{value:?}");
        }
    }

    #[test]
    fn test_tenant_id_needs_whitespace_after_the_scheme() {
        // Not the scheme: the whole value is the key.
        assert_eq!(id_of("Bearertenant-abc"), id_of("Bearertenant-abc  "));
        assert_ne!(id_of("Bearertenant-abc"), TENANT_ABC_ID);
        assert_ne!(id_of("Bearertenant-abc"), "anonymous");
    }

    // -- MeteringStore tests --

    #[tokio::test]
    async fn test_record_and_query_single_tenant() {
        let store = MeteringStore::new();

        store
            .record_with_date("tenant-a", Operation::ProofGenerated, "2026-03-01")
            .await;
        store
            .record_with_date("tenant-a", Operation::ProofGenerated, "2026-03-01")
            .await;
        store
            .record_with_date("tenant-a", Operation::ProofVerified, "2026-03-01")
            .await;

        let records = store.query(Some("tenant-a"), None, None).await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].proofs_generated, 2);
        assert_eq!(records[0].proofs_verified, 1);
        assert_eq!(records[0].total_requests, 3);
    }

    #[tokio::test]
    async fn test_record_multiple_tenants() {
        let store = MeteringStore::new();

        store
            .record_with_date("tenant-a", Operation::ProofGenerated, "2026-03-01")
            .await;
        store
            .record_with_date("tenant-b", Operation::ProofVerified, "2026-03-01")
            .await;
        store
            .record_with_date("tenant-b", Operation::Other, "2026-03-01")
            .await;

        // Query all
        let all = store.query(None, None, None).await;
        assert_eq!(all.len(), 2);

        // Query specific
        let a_only = store.query(Some("tenant-a"), None, None).await;
        assert_eq!(a_only.len(), 1);
        assert_eq!(a_only[0].proofs_generated, 1);

        let b_only = store.query(Some("tenant-b"), None, None).await;
        assert_eq!(b_only.len(), 1);
        assert_eq!(b_only[0].proofs_verified, 1);
        assert_eq!(b_only[0].total_requests, 2);
    }

    #[tokio::test]
    async fn test_query_date_range() {
        let store = MeteringStore::new();

        store
            .record_with_date("t1", Operation::ProofGenerated, "2026-03-01")
            .await;
        store
            .record_with_date("t1", Operation::ProofGenerated, "2026-03-15")
            .await;
        store
            .record_with_date("t1", Operation::ProofGenerated, "2026-03-27")
            .await;

        // From only
        let results = store.query(None, Some("2026-03-10"), None).await;
        assert_eq!(results.len(), 2);

        // To only
        let results = store.query(None, None, Some("2026-03-15")).await;
        assert_eq!(results.len(), 2);

        // Both
        let results = store
            .query(None, Some("2026-03-10"), Some("2026-03-20"))
            .await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].date, "2026-03-15");
    }

    #[tokio::test]
    async fn test_query_empty_results() {
        let store = MeteringStore::new();
        let results = store.query(Some("nonexistent"), None, None).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_totals() {
        let store = MeteringStore::new();

        store
            .record_with_date("t1", Operation::ProofGenerated, "2026-03-01")
            .await;
        store
            .record_with_date("t1", Operation::ProofGenerated, "2026-03-02")
            .await;
        store
            .record_with_date("t2", Operation::ProofVerified, "2026-03-01")
            .await;
        store
            .record_with_date("t2", Operation::Other, "2026-03-01")
            .await;

        let totals = store.totals().await;
        assert_eq!(totals.proofs_generated, 2);
        assert_eq!(totals.proofs_verified, 1);
        assert_eq!(totals.total_requests, 4);
        assert_eq!(totals.unique_tenants, 2);
    }

    #[tokio::test]
    async fn test_records_sorted_by_date_then_tenant() {
        let store = MeteringStore::new();

        store
            .record_with_date("beta", Operation::Other, "2026-03-02")
            .await;
        store
            .record_with_date("alpha", Operation::Other, "2026-03-02")
            .await;
        store
            .record_with_date("alpha", Operation::Other, "2026-03-01")
            .await;

        let records = store.query(None, None, None).await;
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].date, "2026-03-01");
        assert_eq!(records[0].tenant_id, "alpha");
        assert_eq!(records[1].date, "2026-03-02");
        assert_eq!(records[1].tenant_id, "alpha");
        assert_eq!(records[2].date, "2026-03-02");
        assert_eq!(records[2].tenant_id, "beta");
    }

    #[tokio::test]
    async fn test_other_operation_increments_only_total() {
        let store = MeteringStore::new();

        store
            .record_with_date("t1", Operation::Other, "2026-03-01")
            .await;
        store
            .record_with_date("t1", Operation::Other, "2026-03-01")
            .await;

        let records = store.query(Some("t1"), None, None).await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].proofs_generated, 0);
        assert_eq!(records[0].proofs_verified, 0);
        assert_eq!(records[0].total_requests, 2);
    }

    // -- Middleware integration test --

    #[tokio::test]
    async fn test_metering_middleware_integration() {
        use axum::{body::Body, middleware, routing::any, Router};
        use tower::ServiceExt;

        let store = Arc::new(MeteringStore::new());

        async fn ok_handler() -> &'static str {
            "ok"
        }

        let app = Router::new()
            .route("/prove", any(ok_handler))
            .route("/verify", any(ok_handler))
            .route("/proofs/{id}/verify", any(ok_handler))
            .route("/models", any(ok_handler))
            .layer(middleware::from_fn_with_state(
                store.clone(),
                metering_middleware,
            ))
            .with_state(store.clone());

        // Proof generation
        let _ = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method(axum::http::Method::POST)
                    .uri("/prove")
                    .header("Authorization", "Bearer tenant-abc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // Verification
        let _ = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method(axum::http::Method::POST)
                    .uri("/verify")
                    .header("Authorization", "Bearer tenant-abc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // Proof subpath verification
        let _ = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method(axum::http::Method::POST)
                    .uri("/proofs/xyz/verify")
                    .header("Authorization", "Bearer tenant-abc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // Other request (anonymous)
        let _ = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/models")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let today = current_date();

        // Check tenant-abc
        let records = store.query(Some(TENANT_ABC_ID), None, None).await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].date, today);
        assert_eq!(records[0].proofs_generated, 1);
        assert_eq!(records[0].proofs_verified, 2);
        assert_eq!(records[0].total_requests, 3);

        // Check anonymous
        let records = store.query(Some("anonymous"), None, None).await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].total_requests, 1);
        assert_eq!(records[0].proofs_generated, 0);

        // Global totals
        let totals = store.totals().await;
        assert_eq!(totals.total_requests, 4);
        assert_eq!(totals.proofs_generated, 1);
        assert_eq!(totals.proofs_verified, 2);
        assert_eq!(totals.unique_tenants, 2);
    }

    // -- Admin endpoint test --

    #[tokio::test]
    async fn test_usage_handler() {
        use axum::{body::Body, routing::get, Router};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let store = Arc::new(MeteringStore::new());

        // Seed data
        store
            .record_with_date("t1", Operation::ProofGenerated, "2026-03-01")
            .await;
        store
            .record_with_date("t1", Operation::ProofVerified, "2026-03-01")
            .await;
        store
            .record_with_date("t2", Operation::ProofGenerated, "2026-03-02")
            .await;

        let app = Router::new()
            .route("/admin/usage", get(usage_handler))
            .with_state(store.clone());

        // Query all
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/admin/usage")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: UsageResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.records.len(), 2);
        assert_eq!(json.totals.total_requests, 3);
        assert_eq!(json.totals.unique_tenants, 2);

        // Query specific tenant
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/admin/usage?tenant=t1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: UsageResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.records.len(), 1);
        assert_eq!(json.records[0].tenant_id, "t1");
        assert_eq!(json.records[0].proofs_generated, 1);
        assert_eq!(json.records[0].proofs_verified, 1);

        // Query with date range
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/admin/usage?from=2026-03-02&to=2026-03-02")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: UsageResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.records.len(), 1);
        assert_eq!(json.records[0].tenant_id, "t2");
    }

    #[tokio::test]
    async fn test_usage_handler_bad_date() {
        use axum::{body::Body, routing::get, Router};
        use tower::ServiceExt;

        let store = Arc::new(MeteringStore::new());
        let app = Router::new()
            .route("/admin/usage", get(usage_handler))
            .with_state(store);

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/admin/usage?from=not-a-date")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
