//! Comprehensive integration tests for the tee-indexer service.
//!
//! Covers:
//! 1. REST endpoints (/results, /stats, /health) -- response format, status codes, headers
//! 2. WebSocket event subscription and filtering
//! 3. Database query correctness with in-memory SQLite
//! 4. Poll interval behavior (mocked)

#[path = "../src/main.rs"]
#[allow(dead_code)]
mod indexer;

use alloy::primitives::{Address, B256};
use axum::body::Body;
use axum::http::StatusCode;
use http_body_util::BodyExt;
use std::sync::Arc;
use tee_watcher::TEEEvent;
use tower::ServiceExt;

use indexer::{
    build_app,
    websocket::{EventBroadcaster, WsEvent},
    HealthResponse, ResultFilter, ResultRow, SqliteStorage, StatsResponse, Storage,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_storage() -> Arc<dyn Storage> {
    Arc::new(SqliteStorage::open_in_memory().unwrap())
}

fn make_broadcaster() -> Arc<EventBroadcaster> {
    Arc::new(EventBroadcaster::with_max_connections(64, 100))
}

/// Seed the storage with N results and return the storage handle.
fn seed_storage(n: usize) -> Arc<dyn Storage> {
    let s = make_storage();
    for i in 0..n {
        s.insert_result(
            &format!("0x{:04x}", i),
            &format!("0xmodel_{}", i % 3),
            &format!("0xinput_{}", i),
            &format!("0xsubmitter_{}", i % 2),
            (i * 10) as u64,
        )
        .unwrap();
    }
    s
}

/// Send a GET request to the app and return (status, body as Vec<u8>).
async fn get_request(app: axum::Router, uri: &str) -> (StatusCode, Vec<u8>) {
    let req = axum::http::Request::builder()
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, body)
}

// ===========================================================================
// 1. REST endpoint tests -- response format, status codes, headers
// ===========================================================================

mod rest_health {
    use super::*;

    #[tokio::test]
    async fn health_returns_200_with_ok_status() {
        let s = make_storage();
        let app = build_app(s, make_broadcaster());
        let (status, body) = get_request(app, "/health").await;

        assert_eq!(status, StatusCode::OK);
        let health: HealthResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(health.status, "ok");
        assert_eq!(health.last_indexed_block, 0);
        assert_eq!(health.total_results, 0);
    }

    #[tokio::test]
    async fn health_reflects_indexed_block_and_results() {
        let s = make_storage();
        s.set_last_indexed_block(12345).unwrap();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();
        s.insert_result("0x02", "0xm", "0xi", "0xa", 2).unwrap();
        s.insert_result("0x03", "0xm", "0xi", "0xa", 3).unwrap();

        let app = build_app(s, make_broadcaster());
        let (status, body) = get_request(app, "/health").await;

        assert_eq!(status, StatusCode::OK);
        let health: HealthResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(health.last_indexed_block, 12345);
        assert_eq!(health.total_results, 3);
    }

    #[tokio::test]
    async fn health_response_has_required_json_fields() {
        let s = make_storage();
        let app = build_app(s, make_broadcaster());
        let (_, body) = get_request(app, "/health").await;

        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(v.get("status").is_some(), "missing 'status' field");
        assert!(
            v.get("last_indexed_block").is_some(),
            "missing 'last_indexed_block' field"
        );
        assert!(
            v.get("total_results").is_some(),
            "missing 'total_results' field"
        );
    }

    #[tokio::test]
    async fn health_includes_x_api_version_header() {
        let s = make_storage();
        let app = build_app(s, make_broadcaster());

        let req = axum::http::Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        let version = resp.headers().get("x-api-version");
        assert!(version.is_some(), "missing x-api-version header");
        assert_eq!(version.unwrap().to_str().unwrap(), "v1");
    }
}

mod rest_stats {
    use super::*;

    #[tokio::test]
    async fn stats_returns_200_with_empty_db() {
        let s = make_storage();
        let app = build_app(s, make_broadcaster());
        let (status, body) = get_request(app, "/stats").await;

        assert_eq!(status, StatusCode::OK);
        let stats: StatsResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(stats.total_submitted, 0);
        assert_eq!(stats.total_challenged, 0);
        assert_eq!(stats.total_finalized, 0);
        assert_eq!(stats.total_resolved, 0);
    }

    #[tokio::test]
    async fn stats_counts_all_status_categories() {
        let s = make_storage();
        // 2 submitted, 1 challenged, 3 finalized, 1 resolved
        for i in 0..7u32 {
            s.insert_result(
                &format!("0x{:02x}", i),
                "0xm",
                "0xi",
                "0xa",
                i as u64,
            )
            .unwrap();
        }
        s.update_result_status("0x02", "challenged", Some("0xc"))
            .unwrap();
        s.update_result_status("0x03", "finalized", None).unwrap();
        s.update_result_status("0x04", "finalized", None).unwrap();
        s.update_result_status("0x05", "finalized", None).unwrap();
        s.update_result_status("0x06", "resolved", None).unwrap();

        let app = build_app(s, make_broadcaster());
        let (status, body) = get_request(app, "/stats").await;

        assert_eq!(status, StatusCode::OK);
        let stats: StatsResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(stats.total_submitted, 2); // 0x00, 0x01
        assert_eq!(stats.total_challenged, 1); // 0x02
        assert_eq!(stats.total_finalized, 3); // 0x03, 0x04, 0x05
        assert_eq!(stats.total_resolved, 1); // 0x06
    }

    #[tokio::test]
    async fn stats_response_has_required_json_fields() {
        let s = make_storage();
        let app = build_app(s, make_broadcaster());
        let (_, body) = get_request(app, "/stats").await;

        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(v.get("total_submitted").is_some());
        assert!(v.get("total_challenged").is_some());
        assert!(v.get("total_finalized").is_some());
        assert!(v.get("total_resolved").is_some());
    }

    #[tokio::test]
    async fn stats_via_versioned_api() {
        let s = seed_storage(3);
        s.update_result_status("0x0001", "finalized", None)
            .unwrap();

        let app = build_app(s, make_broadcaster());
        let (status, body) = get_request(app, "/api/v1/stats").await;

        assert_eq!(status, StatusCode::OK);
        let stats: StatsResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(stats.total_submitted, 2);
        assert_eq!(stats.total_finalized, 1);
    }
}

mod rest_results {
    use super::*;

    #[tokio::test]
    async fn list_results_returns_200_with_empty_db() {
        let s = make_storage();
        let app = build_app(s, make_broadcaster());
        let (status, body) = get_request(app, "/results").await;

        assert_eq!(status, StatusCode::OK);
        let rows: Vec<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn list_results_returns_all_by_default() {
        let s = seed_storage(5);
        let app = build_app(s, make_broadcaster());
        let (status, body) = get_request(app, "/results").await;

        assert_eq!(status, StatusCode::OK);
        let rows: Vec<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(rows.len(), 5);
    }

    #[tokio::test]
    async fn list_results_ordered_by_block_number_desc() {
        let s = seed_storage(5);
        let app = build_app(s, make_broadcaster());
        let (_, body) = get_request(app, "/results").await;

        let rows: Vec<ResultRow> = serde_json::from_slice(&body).unwrap();
        for i in 0..rows.len() - 1 {
            assert!(
                rows[i].block_number >= rows[i + 1].block_number,
                "results not ordered by block_number DESC: {} < {}",
                rows[i].block_number,
                rows[i + 1].block_number
            );
        }
    }

    #[tokio::test]
    async fn list_results_filter_by_status() {
        let s = seed_storage(4);
        s.update_result_status("0x0001", "finalized", None)
            .unwrap();
        s.update_result_status("0x0003", "finalized", None)
            .unwrap();

        let app = build_app(s, make_broadcaster());
        let (status, body) = get_request(app, "/results?status=finalized").await;

        assert_eq!(status, StatusCode::OK);
        let rows: Vec<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.status == "finalized"));
    }

    #[tokio::test]
    async fn list_results_filter_by_submitter() {
        let s = seed_storage(6);
        // seed_storage uses submitter_{i%2}, so submitter_0 for even, submitter_1 for odd
        let app = build_app(s, make_broadcaster());
        let (status, body) =
            get_request(app, "/results?submitter=0xsubmitter_0").await;

        assert_eq!(status, StatusCode::OK);
        let rows: Vec<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(rows.len(), 3); // indices 0, 2, 4
        assert!(rows.iter().all(|r| r.submitter == "0xsubmitter_0"));
    }

    #[tokio::test]
    async fn list_results_filter_by_model_hash() {
        let s = seed_storage(9);
        // model_{i%3} => model_0 at 0,3,6; model_1 at 1,4,7; model_2 at 2,5,8
        let app = build_app(s, make_broadcaster());
        let (status, body) =
            get_request(app, "/results?model_hash=0xmodel_2").await;

        assert_eq!(status, StatusCode::OK);
        let rows: Vec<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|r| r.model_hash == "0xmodel_2"));
    }

    #[tokio::test]
    async fn list_results_with_limit() {
        let s = seed_storage(10);
        let app = build_app(s, make_broadcaster());
        let (status, body) = get_request(app, "/results?limit=3").await;

        assert_eq!(status, StatusCode::OK);
        let rows: Vec<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(rows.len(), 3);
    }

    #[tokio::test]
    async fn list_results_limit_capped_at_1000() {
        let s = seed_storage(5);
        let app = build_app(s, make_broadcaster());
        // Requesting limit=9999 should be internally capped; still returns only 5
        let (status, body) = get_request(app, "/results?limit=9999").await;

        assert_eq!(status, StatusCode::OK);
        let rows: Vec<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(rows.len(), 5);
    }

    #[tokio::test]
    async fn list_results_combined_filters() {
        let s = make_storage();
        s.insert_result("0x01", "0xm_a", "0xi", "0xalice", 10)
            .unwrap();
        s.insert_result("0x02", "0xm_a", "0xi", "0xbob", 20)
            .unwrap();
        s.insert_result("0x03", "0xm_b", "0xi", "0xalice", 30)
            .unwrap();
        s.update_result_status("0x01", "finalized", None).unwrap();
        s.update_result_status("0x02", "finalized", None).unwrap();

        let app = build_app(s, make_broadcaster());

        // Filter: finalized + model_hash=0xm_a => only 0x01 and 0x02
        let (status, body) =
            get_request(app.clone(), "/results?status=finalized&model_hash=0xm_a").await;
        assert_eq!(status, StatusCode::OK);
        let rows: Vec<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(rows.len(), 2);

        // Filter: submitter=0xalice + status=finalized => only 0x01
        let (status, body) = get_request(
            app,
            "/results?status=finalized&submitter=0xalice",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let rows: Vec<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "0x01");
    }

    #[tokio::test]
    async fn get_result_by_id_returns_200() {
        let s = make_storage();
        s.insert_result("my-result-id", "0xm", "0xi", "0xsub", 99)
            .unwrap();

        let app = build_app(s, make_broadcaster());
        let (status, body) = get_request(app, "/results/my-result-id").await;

        assert_eq!(status, StatusCode::OK);
        let row: ResultRow = serde_json::from_slice(&body).unwrap();
        assert_eq!(row.id, "my-result-id");
        assert_eq!(row.model_hash, "0xm");
        assert_eq!(row.input_hash, "0xi");
        assert_eq!(row.submitter, "0xsub");
        assert_eq!(row.status, "submitted");
        assert_eq!(row.block_number, 99);
        assert!(row.challenger.is_none());
    }

    #[tokio::test]
    async fn get_result_not_found_returns_404() {
        let s = make_storage();
        let app = build_app(s, make_broadcaster());
        let (status, body) = get_request(app, "/results/nonexistent").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(v.get("error").is_some(), "404 response should have 'error' field");
    }

    #[tokio::test]
    async fn get_result_response_has_all_fields() {
        let s = make_storage();
        s.insert_result("0xfull", "0xm", "0xi", "0xsub", 50)
            .unwrap();
        s.update_result_status("0xfull", "challenged", Some("0xchallenger"))
            .unwrap();

        let app = build_app(s, make_broadcaster());
        let (_, body) = get_request(app, "/results/0xfull").await;

        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let expected_fields = [
            "id",
            "model_hash",
            "input_hash",
            "output",
            "submitter",
            "status",
            "block_number",
            "timestamp",
            "challenger",
        ];
        for field in &expected_fields {
            assert!(v.get(field).is_some(), "missing field: {}", field);
        }
        assert_eq!(v["status"], "challenged");
        assert_eq!(v["challenger"], "0xchallenger");
    }

    #[tokio::test]
    async fn versioned_api_list_results() {
        let s = seed_storage(3);
        let app = build_app(s, make_broadcaster());
        let (status, body) = get_request(app, "/api/v1/results").await;

        assert_eq!(status, StatusCode::OK);
        let rows: Vec<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(rows.len(), 3);
    }

    #[tokio::test]
    async fn versioned_api_get_result_by_id() {
        let s = make_storage();
        s.insert_result("0xversioned", "0xm", "0xi", "0xa", 1)
            .unwrap();

        let app = build_app(s, make_broadcaster());
        let (status, body) =
            get_request(app, "/api/v1/results/0xversioned").await;

        assert_eq!(status, StatusCode::OK);
        let row: ResultRow = serde_json::from_slice(&body).unwrap();
        assert_eq!(row.id, "0xversioned");
    }

    #[tokio::test]
    async fn versioned_api_not_found() {
        let s = make_storage();
        let app = build_app(s, make_broadcaster());
        let (status, _) =
            get_request(app, "/api/v1/results/missing").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}

// ===========================================================================
// 2. WebSocket event subscription tests
// ===========================================================================

mod websocket_tests {
    use super::*;
    use futures_util::StreamExt;
    use std::net::SocketAddr;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message;

    /// Start the app on a random port and return (addr, broadcaster).
    async fn start_server(
        storage: Arc<dyn Storage>,
    ) -> (SocketAddr, Arc<EventBroadcaster>) {
        let broadcaster = make_broadcaster();
        let app = build_app(storage, broadcaster.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (addr, broadcaster)
    }

    /// Extract text content from a tungstenite Message.
    fn extract_text(msg: Message) -> String {
        match msg {
            Message::Text(t) => t.to_string(),
            other => panic!("expected Text message, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn ws_connect_and_receive_event() {
        let s = make_storage();
        let (addr, broadcaster) = start_server(s).await;

        let url = format!("ws://{}/ws/events", addr);
        let (mut ws, _) = tokio_tungstenite::connect_async(&url)
            .await
            .unwrap();

        // Broadcast an event
        let event = WsEvent {
            event_type: "ResultSubmitted".to_string(),
            data: serde_json::json!({
                "result_id": "0xabc",
                "block_number": 42
            }),
        };
        broadcaster.broadcast(event.clone());

        // Read the event from the WebSocket
        let msg = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            ws.next(),
        )
        .await
        .expect("timeout waiting for ws message")
        .expect("stream ended")
        .expect("ws error");

        let text = extract_text(msg);
        let received: WsEvent = serde_json::from_str(&text).unwrap();
        assert_eq!(received.event_type, "ResultSubmitted");
        assert_eq!(received.data["result_id"], "0xabc");
    }

    #[tokio::test]
    async fn ws_subscribe_filters_events() {
        let s = make_storage();
        let (addr, broadcaster) = start_server(s).await;

        let url = format!("ws://{}/ws/events", addr);
        let (mut ws, _) = tokio_tungstenite::connect_async(&url)
            .await
            .unwrap();

        // Subscribe to only ResultChallenged events
        let subscribe_msg = serde_json::json!({
            "subscribe": ["ResultChallenged"]
        });

        use futures_util::SinkExt;
        ws.send(Message::Text(subscribe_msg.to_string().into()))
            .await
            .unwrap();

        // Give the server a moment to process the subscribe message
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Broadcast a ResultSubmitted event (should be filtered out)
        broadcaster.broadcast(WsEvent {
            event_type: "ResultSubmitted".to_string(),
            data: serde_json::json!({"result_id": "0x01"}),
        });

        // Broadcast a ResultChallenged event (should be received)
        broadcaster.broadcast(WsEvent {
            event_type: "ResultChallenged".to_string(),
            data: serde_json::json!({"result_id": "0x02", "challenger": "0xc"}),
        });

        // We should receive only the ResultChallenged event
        let msg = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            ws.next(),
        )
        .await
        .expect("timeout waiting for ws message")
        .expect("stream ended")
        .expect("ws error");

        let text = extract_text(msg);
        let received: WsEvent = serde_json::from_str(&text).unwrap();
        assert_eq!(received.event_type, "ResultChallenged");
        assert_eq!(received.data["result_id"], "0x02");
    }

    #[tokio::test]
    async fn ws_multiple_clients_receive_same_event() {
        let s = make_storage();
        let (addr, broadcaster) = start_server(s).await;
        let url = format!("ws://{}/ws/events", addr);

        let (mut ws1, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let (mut ws2, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

        let event = WsEvent {
            event_type: "ResultFinalized".to_string(),
            data: serde_json::json!({"result_id": "0xfin"}),
        };
        broadcaster.broadcast(event);

        for ws in [&mut ws1, &mut ws2] {
            let msg = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                ws.next(),
            )
            .await
            .expect("timeout")
            .expect("stream ended")
            .expect("ws error");

            let text = extract_text(msg);
            let received: WsEvent = serde_json::from_str(&text).unwrap();
            assert_eq!(received.event_type, "ResultFinalized");
        }
    }

    #[tokio::test]
    async fn ws_event_types_all_delivered() {
        let s = make_storage();
        let (addr, broadcaster) = start_server(s).await;
        let url = format!("ws://{}/ws/events", addr);

        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

        let event_types = vec![
            "ResultSubmitted",
            "ResultChallenged",
            "ResultFinalized",
            "ResultExpired",
            "DisputeResolved",
        ];

        for et in &event_types {
            broadcaster.broadcast(WsEvent {
                event_type: et.to_string(),
                data: serde_json::json!({"type": et}),
            });
        }

        for expected in &event_types {
            let msg = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                ws.next(),
            )
            .await
            .expect("timeout")
            .expect("stream ended")
            .expect("ws error");

            let text = extract_text(msg);
            let received: WsEvent = serde_json::from_str(&text).unwrap();
            assert_eq!(received.event_type, *expected);
        }
    }

    #[tokio::test]
    async fn ws_broadcaster_delivers_to_subscriber() {
        let broadcaster = EventBroadcaster::new(64);
        let mut rx = broadcaster.subscribe();

        let event = WsEvent {
            event_type: "ResultSubmitted".to_string(),
            data: serde_json::json!({"test": true}),
        };

        let sent = broadcaster.broadcast(event.clone());
        assert_eq!(sent, 1);

        let received = rx.try_recv().unwrap();
        assert_eq!(received, event);
    }

    #[tokio::test]
    async fn ws_broadcaster_no_receivers_returns_zero() {
        let broadcaster = EventBroadcaster::new(64);
        let event = WsEvent {
            event_type: "ResultSubmitted".to_string(),
            data: serde_json::json!({}),
        };
        let count = broadcaster.broadcast(event);
        assert_eq!(count, 0);
    }
}

// ===========================================================================
// 3. Database query correctness -- in-memory SQLite
// ===========================================================================

mod db_correctness {
    use super::*;

    #[test]
    fn insert_and_get_result_fields() {
        let s = make_storage();
        s.insert_result("id-1", "model-hash-1", "input-hash-1", "submitter-1", 100)
            .unwrap();

        let row = s.get_result("id-1").unwrap().unwrap();
        assert_eq!(row.id, "id-1");
        assert_eq!(row.model_hash, "model-hash-1");
        assert_eq!(row.input_hash, "input-hash-1");
        assert_eq!(row.submitter, "submitter-1");
        assert_eq!(row.status, "submitted");
        assert_eq!(row.block_number, 100);
        assert_eq!(row.output, "");
        assert_eq!(row.timestamp, 0);
        assert!(row.challenger.is_none());
    }

    #[test]
    fn insert_duplicate_is_ignored() {
        let s = make_storage();
        let n1 = s
            .insert_result("dup-id", "0xm", "0xi", "0xs", 1)
            .unwrap();
        assert_eq!(n1, 1);

        let n2 = s
            .insert_result("dup-id", "0xm2", "0xi2", "0xs2", 2)
            .unwrap();
        assert_eq!(n2, 0);

        // Original data preserved
        let row = s.get_result("dup-id").unwrap().unwrap();
        assert_eq!(row.model_hash, "0xm");
        assert_eq!(row.block_number, 1);
    }

    #[test]
    fn get_nonexistent_result_returns_none() {
        let s = make_storage();
        assert!(s.get_result("does-not-exist").unwrap().is_none());
    }

    #[test]
    fn update_status_with_challenger() {
        let s = make_storage();
        s.insert_result("r1", "0xm", "0xi", "0xs", 1).unwrap();

        s.update_result_status("r1", "challenged", Some("0xchallenger"))
            .unwrap();

        let row = s.get_result("r1").unwrap().unwrap();
        assert_eq!(row.status, "challenged");
        assert_eq!(row.challenger.as_deref(), Some("0xchallenger"));
    }

    #[test]
    fn update_status_without_challenger_preserves_existing() {
        let s = make_storage();
        s.insert_result("r1", "0xm", "0xi", "0xs", 1).unwrap();
        s.update_result_status("r1", "challenged", Some("0xchallenger"))
            .unwrap();

        // Now resolve without specifying challenger -- existing challenger preserved
        s.update_result_status("r1", "resolved", None).unwrap();

        let row = s.get_result("r1").unwrap().unwrap();
        assert_eq!(row.status, "resolved");
        assert_eq!(
            row.challenger.as_deref(),
            Some("0xchallenger"),
            "challenger should be preserved when not explicitly set"
        );
    }

    #[test]
    fn list_results_empty_filter_returns_all() {
        let s = seed_storage(5);
        let results = s
            .list_results(&ResultFilter {
                status: None,
                submitter: None,
                model_hash: None,
                limit: None,
            })
            .unwrap();
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn list_results_default_limit_is_50() {
        // If we insert more than 50, the default limit should cap it
        let s = make_storage();
        for i in 0..60 {
            s.insert_result(
                &format!("r{:04}", i),
                "0xm",
                "0xi",
                "0xa",
                i as u64,
            )
            .unwrap();
        }

        let results = s
            .list_results(&ResultFilter {
                status: None,
                submitter: None,
                model_hash: None,
                limit: None,
            })
            .unwrap();
        assert_eq!(results.len(), 50, "default limit should be 50");
    }

    #[test]
    fn stats_all_zeros_on_empty_db() {
        let s = make_storage();
        let stats = s.get_stats().unwrap();
        assert_eq!(stats.total_submitted, 0);
        assert_eq!(stats.total_challenged, 0);
        assert_eq!(stats.total_finalized, 0);
        assert_eq!(stats.total_resolved, 0);
    }

    #[test]
    fn total_results_reflects_inserts() {
        let s = make_storage();
        assert_eq!(s.get_total_results(), 0);

        s.insert_result("r1", "0xm", "0xi", "0xa", 1).unwrap();
        assert_eq!(s.get_total_results(), 1);

        s.insert_result("r2", "0xm", "0xi", "0xa", 2).unwrap();
        assert_eq!(s.get_total_results(), 2);

        // Duplicate insert should not increase count
        s.insert_result("r1", "0xm2", "0xi2", "0xa2", 3).unwrap();
        assert_eq!(s.get_total_results(), 2);
    }

    #[test]
    fn last_indexed_block_default_is_zero() {
        let s = make_storage();
        assert_eq!(s.get_last_indexed_block(), 0);
    }

    #[test]
    fn last_indexed_block_round_trip() {
        let s = make_storage();
        s.set_last_indexed_block(999).unwrap();
        assert_eq!(s.get_last_indexed_block(), 999);

        s.set_last_indexed_block(0).unwrap();
        assert_eq!(s.get_last_indexed_block(), 0);

        s.set_last_indexed_block(u64::MAX).unwrap();
        assert_eq!(s.get_last_indexed_block(), u64::MAX);
    }

    #[test]
    fn is_healthy_returns_true_by_default() {
        let s = make_storage();
        assert!(s.is_healthy());
    }

    #[test]
    fn apply_event_result_submitted() {
        let s = make_storage();
        let rid = B256::from([0x11; 32]);

        s.apply_event(&TEEEvent::ResultSubmitted {
            result_id: rid,
            model_hash: B256::from([0x22; 32]),
            input_hash: B256::from([0x33; 32]),
            submitter: Address::from([0x44; 20]),
            block_number: 50,
        })
        .unwrap();

        let row = s.get_result(&format!("{rid:#x}")).unwrap().unwrap();
        assert_eq!(row.status, "submitted");
        assert_eq!(row.block_number, 50);
    }

    #[test]
    fn apply_event_result_challenged() {
        let s = make_storage();
        let rid = B256::from([0x11; 32]);

        s.apply_event(&TEEEvent::ResultSubmitted {
            result_id: rid,
            model_hash: B256::ZERO,
            input_hash: B256::ZERO,
            submitter: Address::ZERO,
            block_number: 1,
        })
        .unwrap();

        let challenger = Address::from([0x99; 20]);
        s.apply_event(&TEEEvent::ResultChallenged {
            result_id: rid,
            challenger,
        })
        .unwrap();

        let row = s.get_result(&format!("{rid:#x}")).unwrap().unwrap();
        assert_eq!(row.status, "challenged");
        assert!(row.challenger.is_some());
    }

    #[test]
    fn apply_event_result_finalized() {
        let s = make_storage();
        let rid = B256::from([0xAA; 32]);

        s.apply_event(&TEEEvent::ResultSubmitted {
            result_id: rid,
            model_hash: B256::ZERO,
            input_hash: B256::ZERO,
            submitter: Address::ZERO,
            block_number: 1,
        })
        .unwrap();

        s.apply_event(&TEEEvent::ResultFinalized { result_id: rid })
            .unwrap();

        let row = s.get_result(&format!("{rid:#x}")).unwrap().unwrap();
        assert_eq!(row.status, "finalized");
    }

    #[test]
    fn apply_event_result_expired_maps_to_finalized() {
        let s = make_storage();
        let rid = B256::from([0xBB; 32]);

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
    fn apply_event_dispute_resolved() {
        let s = make_storage();
        let rid = B256::from([0xCC; 32]);

        s.apply_event(&TEEEvent::ResultSubmitted {
            result_id: rid,
            model_hash: B256::ZERO,
            input_hash: B256::ZERO,
            submitter: Address::ZERO,
            block_number: 1,
        })
        .unwrap();

        s.apply_event(&TEEEvent::DisputeResolved {
            result_id: rid,
            prover_won: false,
        })
        .unwrap();

        let row = s.get_result(&format!("{rid:#x}")).unwrap().unwrap();
        assert_eq!(row.status, "resolved");
    }

    #[test]
    fn full_event_lifecycle_submit_challenge_resolve() {
        let s = make_storage();
        let rid = B256::from([0xDD; 32]);
        let mh = B256::from([0xEE; 32]);
        let ih = B256::from([0xFF; 32]);
        let sub = Address::from([0x01; 20]);
        let chal = Address::from([0x02; 20]);

        // Submit
        s.apply_event(&TEEEvent::ResultSubmitted {
            result_id: rid,
            model_hash: mh,
            input_hash: ih,
            submitter: sub,
            block_number: 100,
        })
        .unwrap();
        assert_eq!(
            s.get_result(&format!("{rid:#x}")).unwrap().unwrap().status,
            "submitted"
        );

        // Challenge
        s.apply_event(&TEEEvent::ResultChallenged {
            result_id: rid,
            challenger: chal,
        })
        .unwrap();
        assert_eq!(
            s.get_result(&format!("{rid:#x}")).unwrap().unwrap().status,
            "challenged"
        );

        // Resolve
        s.apply_event(&TEEEvent::DisputeResolved {
            result_id: rid,
            prover_won: true,
        })
        .unwrap();
        assert_eq!(
            s.get_result(&format!("{rid:#x}")).unwrap().unwrap().status,
            "resolved"
        );

        // Verify stats
        let stats = s.get_stats().unwrap();
        assert_eq!(stats.total_resolved, 1);
        assert_eq!(stats.total_submitted, 0);
    }

    #[test]
    fn multiple_results_different_statuses() {
        let s = make_storage();

        // Insert several results and transition them through different states
        for i in 0..10u8 {
            let rid = B256::from([i; 32]);
            s.apply_event(&TEEEvent::ResultSubmitted {
                result_id: rid,
                model_hash: B256::ZERO,
                input_hash: B256::ZERO,
                submitter: Address::ZERO,
                block_number: i as u64,
            })
            .unwrap();
        }

        // Finalize first 3
        for i in 0..3u8 {
            let rid = B256::from([i; 32]);
            s.apply_event(&TEEEvent::ResultFinalized { result_id: rid })
                .unwrap();
        }

        // Challenge next 2
        for i in 3..5u8 {
            let rid = B256::from([i; 32]);
            s.apply_event(&TEEEvent::ResultChallenged {
                result_id: rid,
                challenger: Address::from([0xFF; 20]),
            })
            .unwrap();
        }

        // Resolve one of the challenged
        let rid = B256::from([3u8; 32]);
        s.apply_event(&TEEEvent::DisputeResolved {
            result_id: rid,
            prover_won: true,
        })
        .unwrap();

        let stats = s.get_stats().unwrap();
        assert_eq!(stats.total_submitted, 5); // indices 5-9
        assert_eq!(stats.total_finalized, 3); // indices 0-2
        assert_eq!(stats.total_challenged, 1); // index 4
        assert_eq!(stats.total_resolved, 1); // index 3
        assert_eq!(s.get_total_results(), 10);
    }
}

// ===========================================================================
// 4. Poll interval behavior
// ===========================================================================

mod poll_behavior {
    use super::*;

    #[test]
    fn config_default_poll_interval() {
        std::env::remove_var("POLL_INTERVAL_SECS");
        std::env::remove_var("RPC_URL");
        std::env::remove_var("CONTRACT_ADDRESS");
        std::env::remove_var("DB_PATH");
        std::env::remove_var("PORT");

        let config = indexer::Config::from_env().unwrap();
        assert_eq!(
            config.poll_interval_secs, 12,
            "default poll interval should be 12 seconds"
        );
    }

    #[test]
    fn config_custom_poll_interval() {
        std::env::set_var("POLL_INTERVAL_SECS", "30");
        let config = indexer::Config::from_env().unwrap();
        assert_eq!(config.poll_interval_secs, 30);
        std::env::remove_var("POLL_INTERVAL_SECS");
    }

    #[test]
    fn config_invalid_poll_interval_errors() {
        std::env::set_var("POLL_INTERVAL_SECS", "not-a-number");
        let result = indexer::Config::from_env();
        assert!(result.is_err());
        std::env::remove_var("POLL_INTERVAL_SECS");
    }

    /// Verify that storage can persist block state correctly, simulating
    /// what poll_and_index does after processing events.
    #[test]
    fn poll_state_tracking_via_storage() {
        let s = make_storage();

        // Simulate poll cycle 1: blocks 0..100
        assert_eq!(s.get_last_indexed_block(), 0);
        s.insert_result("r1", "0xm", "0xi", "0xa", 50).unwrap();
        s.set_last_indexed_block(101).unwrap();
        assert_eq!(s.get_last_indexed_block(), 101);

        // Simulate poll cycle 2: blocks 101..200
        s.insert_result("r2", "0xm", "0xi", "0xa", 150).unwrap();
        s.set_last_indexed_block(201).unwrap();
        assert_eq!(s.get_last_indexed_block(), 201);

        assert_eq!(s.get_total_results(), 2);
    }

    /// Verify that apply_event followed by storage queries produce
    /// consistent state, simulating the poll loop behavior.
    #[test]
    fn poll_apply_events_and_query() {
        let s = make_storage();

        let events = vec![
            TEEEvent::ResultSubmitted {
                result_id: B256::from([1; 32]),
                model_hash: B256::from([2; 32]),
                input_hash: B256::from([3; 32]),
                submitter: Address::from([4; 20]),
                block_number: 100,
            },
            TEEEvent::ResultSubmitted {
                result_id: B256::from([5; 32]),
                model_hash: B256::from([6; 32]),
                input_hash: B256::from([7; 32]),
                submitter: Address::from([8; 20]),
                block_number: 101,
            },
            TEEEvent::ResultChallenged {
                result_id: B256::from([1; 32]),
                challenger: Address::from([9; 20]),
            },
        ];

        for event in &events {
            s.apply_event(event).unwrap();
        }

        s.set_last_indexed_block(102).unwrap();

        assert_eq!(s.get_total_results(), 2);
        assert_eq!(s.get_last_indexed_block(), 102);

        let stats = s.get_stats().unwrap();
        assert_eq!(stats.total_submitted, 1);
        assert_eq!(stats.total_challenged, 1);
    }

    /// Test that the event-to-websocket conversion produces correct event types
    /// by broadcasting events and verifying they are received via the broadcast channel.
    #[test]
    fn broadcast_all_event_types() {
        let broadcaster = EventBroadcaster::new(64);
        let mut rx = broadcaster.subscribe();

        let event_types = [
            "ResultSubmitted",
            "ResultChallenged",
            "ResultFinalized",
            "ResultExpired",
            "DisputeResolved",
        ];

        for et in &event_types {
            broadcaster.broadcast(WsEvent {
                event_type: et.to_string(),
                data: serde_json::json!({}),
            });
        }

        for expected in &event_types {
            let received = rx.try_recv().unwrap();
            assert_eq!(received.event_type, *expected);
        }
    }

    /// Verify that the broadcaster active_connections tracking works correctly.
    #[test]
    fn broadcaster_active_connections_tracking() {
        let broadcaster = EventBroadcaster::with_max_connections(64, 5);
        assert_eq!(broadcaster.active_connections(), 0);
        assert_eq!(broadcaster.max_connections(), 5);
    }
}

// ===========================================================================
// Additional: SQLite persistence to file (tempfile)
// ===========================================================================

mod sqlite_file_persistence {
    use super::*;

    #[test]
    fn open_and_persist_to_file() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        // Insert data
        {
            let s = SqliteStorage::open(db_path.to_str().unwrap()).unwrap();
            s.insert_result("r1", "0xm", "0xi", "0xa", 42).unwrap();
            s.set_last_indexed_block(100).unwrap();
        }

        // Reopen and verify
        {
            let s = SqliteStorage::open(db_path.to_str().unwrap()).unwrap();
            let row = s.get_result("r1").unwrap().unwrap();
            assert_eq!(row.id, "r1");
            assert_eq!(row.block_number, 42);
            assert_eq!(s.get_last_indexed_block(), 100);
        }
    }

    #[test]
    fn open_creates_tables_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test2.db");

        // Open twice -- should not fail
        let _s1 = SqliteStorage::open(db_path.to_str().unwrap()).unwrap();
        let _s2 = SqliteStorage::open(db_path.to_str().unwrap()).unwrap();
    }
}
