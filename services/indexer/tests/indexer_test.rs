//! Integration tests for the tee-indexer service.
//!
//! Tests the Storage trait, SQLite backend, HTTP API, and event processing
//! through the public interface.

use alloy::primitives::{Address, B256};
use axum::body::Body;
use axum::http::StatusCode;
use http_body_util::BodyExt;
use std::sync::Arc;
use tee_watcher::TEEEvent;
use tower::ServiceExt;

// Re-use public types from the indexer binary crate.
// Since the indexer is a binary, we reference its lib-like public items
// via the module path established by the [[bin]] target.
// For integration tests of a binary crate, we include the source directly.
#[path = "../src/main.rs"]
#[allow(dead_code)]
mod indexer;

use indexer::{
    build_app, websocket::EventBroadcaster, HealthResponse, PaginatedResponse, ResultFilter,
    ResultRow, SqliteStorage, StatsResponse, Storage,
};

fn make_storage() -> Arc<dyn Storage> {
    Arc::new(SqliteStorage::open_in_memory().unwrap())
}

fn make_broadcaster() -> Arc<EventBroadcaster> {
    Arc::new(EventBroadcaster::with_max_connections(64, 100))
}

// ---------------------------------------------------------------------------
// Storage trait tests (through SqliteStorage)
// ---------------------------------------------------------------------------

#[test]
fn test_storage_insert_and_retrieve() {
    let s = make_storage();
    s.insert_result("0xdead", "0xmodel1", "0xinput1", "0xsubmitter1", 42)
        .unwrap();

    let row = s.get_result("0xdead").unwrap().unwrap();
    assert_eq!(row.id, "0xdead");
    assert_eq!(row.model_hash, "0xmodel1");
    assert_eq!(row.status, "submitted");
    assert_eq!(row.block_number, 42);
}

#[test]
fn test_storage_block_tracking() {
    let s = make_storage();

    // Default is 0
    assert_eq!(s.get_last_indexed_block(), 0);

    // Set and read back
    s.set_last_indexed_block(1000).unwrap();
    assert_eq!(s.get_last_indexed_block(), 1000);

    // Overwrite
    s.set_last_indexed_block(2000).unwrap();
    assert_eq!(s.get_last_indexed_block(), 2000);
}

#[test]
fn test_storage_event_full_lifecycle() {
    let s = make_storage();

    let rid = B256::from([0xAA; 32]);
    let mh = B256::from([0xBB; 32]);
    let ih = B256::from([0xCC; 32]);
    let submitter = Address::from([0x11; 20]);
    let challenger = Address::from([0x22; 20]);

    // Submit
    s.apply_event(&TEEEvent::ResultSubmitted {
        result_id: rid,
        model_hash: mh,
        input_hash: ih,
        submitter,
        block_number: 100,
    })
    .unwrap();

    let row = s.get_result(&format!("{rid:#x}")).unwrap().unwrap();
    assert_eq!(row.status, "submitted");

    // Challenge
    s.apply_event(&TEEEvent::ResultChallenged {
        result_id: rid,
        challenger,
    })
    .unwrap();

    let row = s.get_result(&format!("{rid:#x}")).unwrap().unwrap();
    assert_eq!(row.status, "challenged");
    assert!(row.challenger.is_some());

    // Resolve (prover wins)
    s.apply_event(&TEEEvent::DisputeResolved {
        result_id: rid,
        prover_won: true,
    })
    .unwrap();

    let row = s.get_result(&format!("{rid:#x}")).unwrap().unwrap();
    assert_eq!(row.status, "resolved");
}

#[test]
fn test_storage_event_expired_treated_as_finalized() {
    let s = make_storage();

    let rid = B256::from([0x55; 32]);
    s.apply_event(&TEEEvent::ResultSubmitted {
        result_id: rid,
        model_hash: B256::ZERO,
        input_hash: B256::ZERO,
        submitter: Address::ZERO,
        block_number: 1,
    })
    .unwrap();

    s.apply_event(&TEEEvent::ResultExpired { result_id: rid })
        .unwrap();

    let row = s.get_result(&format!("{rid:#x}")).unwrap().unwrap();
    assert_eq!(row.status, "finalized");
}

#[test]
fn test_storage_stats_after_mixed_events() {
    let s = make_storage();

    // Insert 5 results
    for i in 0..5u8 {
        s.insert_result(&format!("0x{:02x}", i), "0xm", "0xi", "0xa", i as u64)
            .unwrap();
    }

    // Finalize 0x00, challenge 0x01, resolve 0x02
    s.update_result_status("0x00", "finalized", None).unwrap();
    s.update_result_status("0x01", "challenged", Some("0xc"))
        .unwrap();
    s.update_result_status("0x02", "resolved", None).unwrap();

    let stats = s.get_stats().unwrap();
    assert_eq!(stats.total_submitted, 2); // 0x03, 0x04
    assert_eq!(stats.total_finalized, 1); // 0x00
    assert_eq!(stats.total_challenged, 1); // 0x01
    assert_eq!(stats.total_resolved, 1); // 0x02
    assert_eq!(s.get_total_results(), 5);
}

#[test]
fn test_storage_list_filter_by_model_hash() {
    let s = make_storage();

    s.insert_result("0x01", "0xmodelA", "0xi", "0xa", 1)
        .unwrap();
    s.insert_result("0x02", "0xmodelB", "0xi", "0xa", 2)
        .unwrap();
    s.insert_result("0x03", "0xmodelA", "0xi", "0xa", 3)
        .unwrap();

    let results = s
        .list_results(&ResultFilter {
            status: None,
            submitter: None,
            model_hash: Some("0xmodelA".to_string()),
            limit: None,
            ..Default::default()
        })
        .unwrap();

    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|r| r.model_hash == "0xmodelA"));
}

#[test]
fn test_storage_query_by_result_id() {
    let s = make_storage();

    s.insert_result("result-001", "0xm", "0xi", "0xa", 10)
        .unwrap();
    s.insert_result("result-002", "0xm", "0xi", "0xa", 20)
        .unwrap();

    assert!(s.get_result("result-001").unwrap().is_some());
    assert!(s.get_result("result-002").unwrap().is_some());
    assert!(s.get_result("result-999").unwrap().is_none());
}

// ---------------------------------------------------------------------------
// HTTP API integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_api_health_reflects_storage_state() {
    let s = make_storage();
    s.set_last_indexed_block(500).unwrap();
    s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();
    s.insert_result("0x02", "0xm", "0xi", "0xa", 2).unwrap();

    let app = build_app(s, make_broadcaster());
    let req = axum::http::Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let health: HealthResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(health.last_indexed_block, 500);
    assert_eq!(health.total_results, 2);
}

#[tokio::test]
async fn test_api_stats_counts() {
    let s = make_storage();
    s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();
    s.insert_result("0x02", "0xm", "0xi", "0xa", 2).unwrap();
    s.insert_result("0x03", "0xm", "0xi", "0xa", 3).unwrap();
    s.update_result_status("0x01", "finalized", None).unwrap();
    s.update_result_status("0x02", "challenged", Some("0xc"))
        .unwrap();

    let app = build_app(s, make_broadcaster());
    let req = axum::http::Request::builder()
        .uri("/stats")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let stats: StatsResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(stats.total_submitted, 1);
    assert_eq!(stats.total_finalized, 1);
    assert_eq!(stats.total_challenged, 1);
}

#[tokio::test]
async fn test_api_get_result_by_id() {
    let s = make_storage();
    s.insert_result("unique-id-xyz", "0xm", "0xi", "0xsub", 77)
        .unwrap();

    let app = build_app(s, make_broadcaster());
    let req = axum::http::Request::builder()
        .uri("/results/unique-id-xyz")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let row: ResultRow = serde_json::from_slice(&body).unwrap();
    assert_eq!(row.id, "unique-id-xyz");
    assert_eq!(row.submitter, "0xsub");
    assert_eq!(row.block_number, 77);
}

#[tokio::test]
async fn test_api_get_result_404() {
    let s = make_storage();
    let app = build_app(s, make_broadcaster());

    let req = axum::http::Request::builder()
        .uri("/results/does-not-exist")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_api_list_with_status_filter() {
    let s = make_storage();
    s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();
    s.insert_result("0x02", "0xm", "0xi", "0xa", 2).unwrap();
    s.update_result_status("0x01", "finalized", None).unwrap();

    let app = build_app(s, make_broadcaster());

    // Get only finalized
    let req = axum::http::Request::builder()
        .uri("/results?status=finalized")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
    assert_eq!(page.data.len(), 1);
    assert_eq!(page.data[0].id, "0x01");
    assert_eq!(page.data[0].status, "finalized");
}

// ---------------------------------------------------------------------------
// Config tests
// ---------------------------------------------------------------------------

#[test]
fn test_config_defaults() {
    // Clear env to test defaults
    std::env::remove_var("RPC_URL");
    std::env::remove_var("CONTRACT_ADDRESS");
    std::env::remove_var("DB_PATH");
    std::env::remove_var("PORT");
    std::env::remove_var("POLL_INTERVAL_SECS");

    let config = indexer::Config::from_env().unwrap();
    assert_eq!(config.rpc_url, "http://localhost:8545");
    assert_eq!(config.db_path, "./indexer.db");
    assert_eq!(config.port, 8081);
    assert_eq!(config.poll_interval_secs, 12);
}

// ---------------------------------------------------------------------------
// Versioned API endpoint tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_api_versioned_results() {
    let s = make_storage();
    s.insert_result("0xaa", "0xm", "0xi", "0xa", 10).unwrap();
    s.insert_result("0xbb", "0xm", "0xi", "0xa", 20).unwrap();

    let app = build_app(s, make_broadcaster());
    let req = axum::http::Request::builder()
        .uri("/api/v1/results")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify X-API-Version header is present and set to v1
    let version_header = resp.headers().get("x-api-version").expect("missing X-API-Version header");
    assert_eq!(version_header, "v1");

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
    assert_eq!(page.data.len(), 2);
}

#[tokio::test]
async fn test_api_versioned_stats() {
    let s = make_storage();
    s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();
    s.insert_result("0x02", "0xm", "0xi", "0xa", 2).unwrap();
    s.update_result_status("0x01", "finalized", None).unwrap();

    // Query both the versioned and unversioned endpoints with separate app
    // instances (oneshot consumes the router).
    let app_v1 = build_app(s.clone(), make_broadcaster());
    let req_v1 = axum::http::Request::builder()
        .uri("/api/v1/stats")
        .body(Body::empty())
        .unwrap();
    let resp_v1 = app_v1.oneshot(req_v1).await.unwrap();
    assert_eq!(resp_v1.status(), StatusCode::OK);
    assert_eq!(resp_v1.headers().get("x-api-version").unwrap(), "v1");
    let body_v1 = resp_v1.into_body().collect().await.unwrap().to_bytes();
    let stats_v1: StatsResponse = serde_json::from_slice(&body_v1).unwrap();

    let app_legacy = build_app(s, make_broadcaster());
    let req_legacy = axum::http::Request::builder()
        .uri("/stats")
        .body(Body::empty())
        .unwrap();
    let resp_legacy = app_legacy.oneshot(req_legacy).await.unwrap();
    let body_legacy = resp_legacy.into_body().collect().await.unwrap().to_bytes();
    let stats_legacy: StatsResponse = serde_json::from_slice(&body_legacy).unwrap();

    // Both endpoints must return identical data
    assert_eq!(stats_v1.total_submitted, stats_legacy.total_submitted);
    assert_eq!(stats_v1.total_finalized, stats_legacy.total_finalized);
    assert_eq!(stats_v1.total_challenged, stats_legacy.total_challenged);
    assert_eq!(stats_v1.total_resolved, stats_legacy.total_resolved);
}

#[tokio::test]
async fn test_api_versioned_result_by_id() {
    let s = make_storage();
    s.insert_result("id-versioned-test", "0xmodel", "0xinput", "0xsub", 55)
        .unwrap();

    let app = build_app(s, make_broadcaster());
    let req = axum::http::Request::builder()
        .uri("/api/v1/results/id-versioned-test")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers().get("x-api-version").unwrap(), "v1");

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let row: ResultRow = serde_json::from_slice(&body).unwrap();
    assert_eq!(row.id, "id-versioned-test");
    assert_eq!(row.model_hash, "0xmodel");
    assert_eq!(row.block_number, 55);
}

// ---------------------------------------------------------------------------
// Additional filter and ordering tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_api_list_filter_by_submitter() {
    let s = make_storage();
    s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();
    s.insert_result("0x02", "0xm", "0xi", "0xb", 2).unwrap();
    s.insert_result("0x03", "0xm", "0xi", "0xa", 3).unwrap();
    s.insert_result("0x04", "0xm", "0xi", "0xc", 4).unwrap();

    let app = build_app(s, make_broadcaster());
    let req = axum::http::Request::builder()
        .uri("/results?submitter=0xa")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
    assert_eq!(page.data.len(), 2);
    assert!(page.data.iter().all(|r| r.submitter == "0xa"));
}

#[tokio::test]
async fn test_api_list_with_limit() {
    let s = make_storage();
    for i in 0..5u8 {
        s.insert_result(&format!("0x{:02x}", i), "0xm", "0xi", "0xa", i as u64)
            .unwrap();
    }

    let app = build_app(s, make_broadcaster());
    let req = axum::http::Request::builder()
        .uri("/results?limit=2")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
    assert_eq!(page.data.len(), 2);
    assert_eq!(page.total, 5);
    assert!(page.has_more);
}

#[tokio::test]
async fn test_api_results_ordered_by_block_desc() {
    let s = make_storage();
    s.insert_result("0xold", "0xm", "0xi", "0xa", 5).unwrap();
    s.insert_result("0xnew", "0xm", "0xi", "0xa", 100).unwrap();
    s.insert_result("0xmid", "0xm", "0xi", "0xa", 50).unwrap();

    let app = build_app(s, make_broadcaster());
    let req = axum::http::Request::builder()
        .uri("/results")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();

    assert_eq!(page.data.len(), 3);
    // Newest first (highest block_number)
    assert_eq!(page.data[0].id, "0xnew");
    assert_eq!(page.data[0].block_number, 100);
    assert_eq!(page.data[1].id, "0xmid");
    assert_eq!(page.data[1].block_number, 50);
    assert_eq!(page.data[2].id, "0xold");
    assert_eq!(page.data[2].block_number, 5);
}

// ---------------------------------------------------------------------------
// Storage edge-case tests
// ---------------------------------------------------------------------------

#[test]
fn test_storage_duplicate_insert_ignored() {
    let s = make_storage();

    // First insert succeeds (returns 1 row affected)
    let n1 = s
        .insert_result("dup-id", "0xmodel1", "0xinput1", "0xsub1", 10)
        .unwrap();
    assert_eq!(n1, 1);

    // Second insert with same ID is silently ignored (INSERT OR IGNORE)
    let n2 = s
        .insert_result("dup-id", "0xmodel2", "0xinput2", "0xsub2", 20)
        .unwrap();
    assert_eq!(n2, 0);

    // Original row is preserved, not overwritten
    let row = s.get_result("dup-id").unwrap().unwrap();
    assert_eq!(row.model_hash, "0xmodel1");
    assert_eq!(row.submitter, "0xsub1");
    assert_eq!(row.block_number, 10);

    // Total count is still 1
    assert_eq!(s.get_total_results(), 1);
}

#[test]
fn test_storage_concurrent_access() {
    use std::thread;

    let s = make_storage();

    // Spawn multiple writer threads
    let mut handles = Vec::new();
    for i in 0..10u32 {
        let storage = s.clone();
        handles.push(thread::spawn(move || {
            storage
                .insert_result(
                    &format!("0x{:04x}", i),
                    "0xm",
                    "0xi",
                    "0xa",
                    i as u64,
                )
                .unwrap();
        }));
    }

    // Spawn concurrent reader threads
    for _ in 0..5 {
        let storage = s.clone();
        handles.push(thread::spawn(move || {
            let _ = storage.list_results(&ResultFilter {
                status: None,
                submitter: None,
                model_hash: None,
                limit: None,
                ..Default::default()
            });
            let _ = storage.get_stats();
        }));
    }

    for h in handles {
        h.join().expect("thread panicked");
    }

    // All 10 unique inserts should be present
    assert_eq!(s.get_total_results(), 10);
}

#[test]
fn test_storage_healthy_by_default() {
    let s = make_storage();
    // A freshly created storage instance should report healthy
    assert!(s.is_healthy());
}
