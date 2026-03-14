pub mod metrics;
#[cfg(feature = "postgres")]
pub mod pg_storage;
pub mod routes;
pub mod websocket;

use alloy::primitives::Address;
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::Filter;
use axum::Router;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tee_watcher::{parse_log, TEEEvent};
use tracing::{error, info, warn};

use self::websocket::{EventBroadcaster, WsEvent};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct Config {
    pub rpc_url: String,
    pub contract_address: Address,
    pub db_path: String,
    pub db_type: String,
    pub database_url: Option<String>,
    pub port: u16,
    pub poll_interval_secs: u64,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let rpc_url =
            std::env::var("RPC_URL").unwrap_or_else(|_| "http://localhost:8545".to_string());
        let contract_address: Address = std::env::var("CONTRACT_ADDRESS")
            .unwrap_or_else(|_| "0x0000000000000000000000000000000000000000".to_string())
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid CONTRACT_ADDRESS"))?;
        let db_path = std::env::var("DB_PATH").unwrap_or_else(|_| "./indexer.db".to_string());
        let db_type = std::env::var("DB_TYPE").unwrap_or_else(|_| "sqlite".to_string());
        let database_url = std::env::var("DATABASE_URL").ok();
        let port: u16 = std::env::var("PORT")
            .unwrap_or_else(|_| "8081".to_string())
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid PORT"))?;
        let poll_interval_secs: u64 = std::env::var("POLL_INTERVAL_SECS")
            .unwrap_or_else(|_| "12".to_string())
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid POLL_INTERVAL_SECS"))?;
        Ok(Self {
            rpc_url,
            contract_address,
            db_path,
            db_type,
            database_url,
            port,
            poll_interval_secs,
        })
    }
}

// ---------------------------------------------------------------------------
// Storage trait
// ---------------------------------------------------------------------------

/// Trait abstracting the persistence layer so backends are swappable
/// (SQLite now, Postgres later).
pub trait Storage: Send + Sync {
    fn insert_result(
        &self,
        id: &str,
        model_hash: &str,
        input_hash: &str,
        submitter: &str,
        block_number: u64,
    ) -> anyhow::Result<usize>;

    fn update_result_status(
        &self,
        id: &str,
        status: &str,
        challenger: Option<&str>,
    ) -> anyhow::Result<usize>;

    fn get_result(&self, id: &str) -> anyhow::Result<Option<ResultRow>>;

    fn list_results(&self, filter: &ResultFilter) -> anyhow::Result<Vec<ResultRow>>;

    fn get_stats(&self) -> anyhow::Result<StatsResponse>;

    fn get_last_indexed_block(&self) -> u64;

    fn set_last_indexed_block(&self, block: u64) -> anyhow::Result<()>;

    fn get_total_results(&self) -> u64;

    fn apply_event(&self, event: &TEEEvent) -> anyhow::Result<()>;
}

// ---------------------------------------------------------------------------
// SQLite storage implementation
// ---------------------------------------------------------------------------

pub struct SqliteStorage {
    conn: Mutex<Connection>,
}

impl SqliteStorage {
    pub fn open(path: &str) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        let storage = Self {
            conn: Mutex::new(conn),
        };
        storage.init_tables()?;
        Ok(storage)
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        let storage = Self {
            conn: Mutex::new(conn),
        };
        storage.init_tables()?;
        Ok(storage)
    }

    fn lock(&self) -> anyhow::Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| anyhow::anyhow!("storage lock poisoned"))
    }

    fn init_tables(&self) -> anyhow::Result<()> {
        let conn = self.lock()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS results (
                id              TEXT PRIMARY KEY,
                model_hash      TEXT NOT NULL,
                input_hash      TEXT NOT NULL,
                output          TEXT NOT NULL DEFAULT '',
                submitter       TEXT NOT NULL,
                status          TEXT NOT NULL DEFAULT 'submitted',
                block_number    INTEGER NOT NULL DEFAULT 0,
                timestamp       INTEGER NOT NULL DEFAULT 0,
                challenger      TEXT
            );
            CREATE TABLE IF NOT EXISTS indexer_state (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )?;
        Ok(())
    }
}

fn row_to_result(row: &rusqlite::Row) -> rusqlite::Result<ResultRow> {
    Ok(ResultRow {
        id: row.get(0)?,
        model_hash: row.get(1)?,
        input_hash: row.get(2)?,
        output: row.get(3)?,
        submitter: row.get(4)?,
        status: row.get(5)?,
        block_number: row.get(6)?,
        timestamp: row.get(7)?,
        challenger: row.get(8)?,
    })
}

impl Storage for SqliteStorage {
    fn insert_result(
        &self,
        id: &str,
        model_hash: &str,
        input_hash: &str,
        submitter: &str,
        block_number: u64,
    ) -> anyhow::Result<usize> {
        let conn = self.lock()?;
        Ok(conn.execute(
            "INSERT OR IGNORE INTO results (id, model_hash, input_hash, submitter, status, block_number)
             VALUES (?1, ?2, ?3, ?4, 'submitted', ?5)",
            params![id, model_hash, input_hash, submitter, block_number],
        )?)
    }

    fn update_result_status(
        &self,
        id: &str,
        status: &str,
        challenger: Option<&str>,
    ) -> anyhow::Result<usize> {
        let conn = self.lock()?;
        Ok(if let Some(c) = challenger {
            conn.execute(
                "UPDATE results SET status = ?1, challenger = ?2 WHERE id = ?3",
                params![status, c, id],
            )?
        } else {
            conn.execute(
                "UPDATE results SET status = ?1 WHERE id = ?2",
                params![status, id],
            )?
        })
    }

    fn get_result(&self, id: &str) -> anyhow::Result<Option<ResultRow>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, model_hash, input_hash, output, submitter, status, block_number, timestamp, challenger
             FROM results WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], row_to_result)?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    fn list_results(&self, filter: &ResultFilter) -> anyhow::Result<Vec<ResultRow>> {
        let conn = self.lock()?;
        let mut sql = String::from(
            "SELECT id, model_hash, input_hash, output, submitter, status, block_number, timestamp, challenger
             FROM results WHERE 1=1",
        );
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(ref status) = filter.status {
            param_values.push(Box::new(status.clone()));
            sql.push_str(&format!(" AND status = ?{}", param_values.len()));
        }
        if let Some(ref submitter) = filter.submitter {
            param_values.push(Box::new(submitter.clone()));
            sql.push_str(&format!(" AND submitter = ?{}", param_values.len()));
        }
        if let Some(ref model_hash) = filter.model_hash {
            param_values.push(Box::new(model_hash.clone()));
            sql.push_str(&format!(" AND model_hash = ?{}", param_values.len()));
        }

        sql.push_str(" ORDER BY block_number DESC");

        let limit = filter.limit.unwrap_or(50).min(1000);
        param_values.push(Box::new(limit as i64));
        sql.push_str(&format!(" LIMIT ?{}", param_values.len()));

        let mut stmt = conn.prepare(&sql)?;
        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(params_ref.as_slice(), row_to_result)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    fn get_stats(&self) -> anyhow::Result<StatsResponse> {
        let conn = self.lock()?;
        let total_submitted: i64 = conn.query_row(
            "SELECT COUNT(*) FROM results WHERE status = 'submitted'",
            [],
            |r| r.get(0),
        )?;
        let total_challenged: i64 = conn.query_row(
            "SELECT COUNT(*) FROM results WHERE status = 'challenged'",
            [],
            |r| r.get(0),
        )?;
        let total_finalized: i64 = conn.query_row(
            "SELECT COUNT(*) FROM results WHERE status = 'finalized'",
            [],
            |r| r.get(0),
        )?;
        let total_resolved: i64 = conn.query_row(
            "SELECT COUNT(*) FROM results WHERE status = 'resolved'",
            [],
            |r| r.get(0),
        )?;
        Ok(StatsResponse {
            total_submitted: total_submitted as u64,
            total_challenged: total_challenged as u64,
            total_finalized: total_finalized as u64,
            total_resolved: total_resolved as u64,
        })
    }

    fn get_last_indexed_block(&self) -> u64 {
        let conn = match self.lock() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("DB lock poisoned in get_last_indexed_block: {}", e);
                return 0;
            }
        };
        conn.query_row(
            "SELECT value FROM indexer_state WHERE key = 'last_indexed_block'",
            [],
            |r| {
                let v: String = r.get(0)?;
                Ok(v.parse::<u64>().unwrap_or(0))
            },
        )
        .unwrap_or(0)
    }

    fn set_last_indexed_block(&self, block: u64) -> anyhow::Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT OR REPLACE INTO indexer_state (key, value) VALUES ('last_indexed_block', ?1)",
            params![block.to_string()],
        )?;
        Ok(())
    }

    fn get_total_results(&self) -> u64 {
        let conn = match self.lock() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("DB lock poisoned in get_total_results: {}", e);
                return 0;
            }
        };
        conn.query_row("SELECT COUNT(*) FROM results", [], |r| {
            let v: i64 = r.get(0)?;
            Ok(v as u64)
        })
        .unwrap_or(0)
    }

    fn apply_event(&self, event: &TEEEvent) -> anyhow::Result<()> {
        match event {
            TEEEvent::ResultSubmitted {
                result_id,
                model_hash,
                input_hash,
                submitter,
                block_number,
            } => {
                self.insert_result(
                    &format!("{result_id:#x}"),
                    &format!("{model_hash:#x}"),
                    &format!("{input_hash:#x}"),
                    &format!("{submitter:#x}"),
                    *block_number,
                )?;
            }
            TEEEvent::ResultChallenged {
                result_id,
                challenger,
            } => {
                self.update_result_status(
                    &format!("{result_id:#x}"),
                    "challenged",
                    Some(&format!("{challenger:#x}")),
                )?;
            }
            TEEEvent::ResultFinalized { result_id } => {
                self.update_result_status(&format!("{result_id:#x}"), "finalized", None)?;
            }
            TEEEvent::ResultExpired { result_id } => {
                self.update_result_status(&format!("{result_id:#x}"), "finalized", None)?;
            }
            TEEEvent::DisputeResolved {
                result_id,
                prover_won: _,
            } => {
                self.update_result_status(&format!("{result_id:#x}"), "resolved", None)?;
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Event watcher
// ---------------------------------------------------------------------------

/// Convert a [`TEEEvent`] into a [`WsEvent`] for WebSocket broadcasting.
fn tee_event_to_ws_event(event: &TEEEvent) -> WsEvent {
    match event {
        TEEEvent::ResultSubmitted {
            result_id,
            model_hash,
            input_hash,
            submitter,
            block_number,
        } => WsEvent {
            event_type: "ResultSubmitted".to_string(),
            data: serde_json::json!({
                "result_id": format!("{result_id:#x}"),
                "model_hash": format!("{model_hash:#x}"),
                "input_hash": format!("{input_hash:#x}"),
                "submitter": format!("{submitter:#x}"),
                "block_number": block_number,
            }),
        },
        TEEEvent::ResultChallenged {
            result_id,
            challenger,
        } => WsEvent {
            event_type: "ResultChallenged".to_string(),
            data: serde_json::json!({
                "result_id": format!("{result_id:#x}"),
                "challenger": format!("{challenger:#x}"),
            }),
        },
        TEEEvent::ResultFinalized { result_id } => WsEvent {
            event_type: "ResultFinalized".to_string(),
            data: serde_json::json!({
                "result_id": format!("{result_id:#x}"),
            }),
        },
        TEEEvent::ResultExpired { result_id } => WsEvent {
            event_type: "ResultExpired".to_string(),
            data: serde_json::json!({
                "result_id": format!("{result_id:#x}"),
            }),
        },
        TEEEvent::DisputeResolved {
            result_id,
            prover_won,
        } => WsEvent {
            event_type: "DisputeResolved".to_string(),
            data: serde_json::json!({
                "result_id": format!("{result_id:#x}"),
                "prover_won": prover_won,
            }),
        },
    }
}

async fn poll_and_index(
    rpc_url: &str,
    contract_address: Address,
    storage: &dyn Storage,
    broadcaster: &EventBroadcaster,
) -> anyhow::Result<()> {
    let from_block = storage.get_last_indexed_block();

    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    let latest = provider.get_block_number().await?;

    if from_block > latest {
        return Ok(());
    }

    let filter = Filter::new()
        .address(contract_address)
        .from_block(from_block)
        .to_block(latest);

    let logs = provider.get_logs(&filter).await?;
    let events: Vec<TEEEvent> = logs.iter().filter_map(parse_log).collect();

    if !events.is_empty() {
        info!(
            "indexed {} events in blocks {}..{}",
            events.len(),
            from_block,
            latest
        );
    }

    for event in &events {
        if let Err(e) = storage.apply_event(event) {
            error!("failed to apply event: {}", e);
        }
        // Broadcast to WebSocket clients
        let ws_event = tee_event_to_ws_event(event);
        broadcaster.broadcast(ws_event);
    }
    storage.set_last_indexed_block(latest + 1)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// API types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultRow {
    pub id: String,
    pub model_hash: String,
    pub input_hash: String,
    pub output: String,
    pub submitter: String,
    pub status: String,
    pub block_number: i64,
    pub timestamp: i64,
    pub challenger: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ResultFilter {
    pub status: Option<String>,
    pub submitter: Option<String>,
    pub model_hash: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsResponse {
    pub total_submitted: u64,
    pub total_challenged: u64,
    pub total_finalized: u64,
    pub total_resolved: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub last_indexed_block: u64,
    pub total_results: u64,
}

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

#[derive(Clone)]
#[allow(dead_code)]
pub(crate) struct AppState {
    pub(crate) storage: Arc<dyn Storage>,
    pub(crate) broadcaster: Arc<EventBroadcaster>,
}

// ---------------------------------------------------------------------------
// App builder (delegates to routes module)
// ---------------------------------------------------------------------------

pub fn build_app(storage: Arc<dyn Storage>, broadcaster: Arc<EventBroadcaster>) -> Router {
    routes::build_app(storage, broadcaster)
}

// ---------------------------------------------------------------------------
// Graceful shutdown
// ---------------------------------------------------------------------------

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let Ok(mut sigterm) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        else {
            tracing::warn!("failed to install SIGTERM handler, using ctrl-c only");
            ctrl_c.await.ok();
            return;
        };
        tokio::select! {
            _ = ctrl_c => { info!("received SIGINT, shutting down"); }
            _ = sigterm.recv() => { info!("received SIGTERM, shutting down"); }
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
        info!("received SIGINT, shutting down");
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = Config::from_env()?;

    info!("tee-indexer starting");
    info!("  rpc_url:          {}", config.rpc_url);
    info!("  contract_address: {:#x}", config.contract_address);
    info!("  db_type:          {}", config.db_type);
    info!("  db_path:          {}", config.db_path);
    info!("  port:             {}", config.port);

    let storage: Arc<dyn Storage> = match config.db_type.to_lowercase().as_str() {
        "postgres" | "postgresql" | "pg" => {
            let url = config
                .database_url
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("DATABASE_URL is required when DB_TYPE=postgres"))?;
            let redacted = &url[..url.find('@').unwrap_or(url.len()).min(30)];
            info!("  database_url:     {}...", redacted);
            #[cfg(feature = "postgres")]
            {
                Arc::new(pg_storage::PgStorage::new(url).await?) as Arc<dyn Storage>
            }
            #[cfg(not(feature = "postgres"))]
            {
                let _ = redacted;
                anyhow::bail!(
                    "Postgres backend requires the 'postgres' feature. \
                     Rebuild with: cargo build --features postgres"
                );
            }
        }
        _ => {
            info!("  storage backend:  sqlite ({})", config.db_path);
            Arc::new(SqliteStorage::open(&config.db_path)?)
        }
    };
    let broadcaster = Arc::new(EventBroadcaster::new(256));

    info!("  max_ws_connections: {}", broadcaster.max_connections());

    let app = build_app(storage.clone(), broadcaster.clone());

    // Spawn the event polling loop
    let poll_storage = storage.clone();
    let poll_broadcaster = broadcaster.clone();
    let poll_rpc = config.rpc_url.clone();
    let poll_addr = config.contract_address;
    let poll_interval = config.poll_interval_secs;
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(poll_interval));
        loop {
            interval.tick().await;
            if let Err(e) = poll_and_index(
                &poll_rpc,
                poll_addr,
                poll_storage.as_ref(),
                &poll_broadcaster,
            )
            .await
            {
                warn!("poll error: {}", e);
            }
        }
    });

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", config.port)).await?;
    info!("listening on 0.0.0.0:{}", config.port);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("tee-indexer stopped");
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::B256;
    use axum::body::Body;
    use axum::http::StatusCode;
    use http_body_util::BodyExt;
    use tee_watcher::{
        topic_dispute_resolved, topic_result_challenged, topic_result_expired,
        topic_result_finalized, topic_result_submitted,
    };
    use tower::ServiceExt;

    fn test_storage() -> Arc<dyn Storage> {
        Arc::new(SqliteStorage::open_in_memory().unwrap())
    }

    fn test_broadcaster() -> Arc<EventBroadcaster> {
        Arc::new(EventBroadcaster::with_max_connections(64, 100))
    }

    // -- Database CRUD tests --

    #[test]
    fn test_sqlite_insert_and_get_result() {
        let s = test_storage();
        s.insert_result("0xabc", "0xmodel", "0xinput", "0xsubmitter", 100)
            .unwrap();

        let row = s.get_result("0xabc").unwrap().unwrap();
        assert_eq!(row.id, "0xabc");
        assert_eq!(row.model_hash, "0xmodel");
        assert_eq!(row.input_hash, "0xinput");
        assert_eq!(row.submitter, "0xsubmitter");
        assert_eq!(row.status, "submitted");
        assert_eq!(row.block_number, 100);
        assert!(row.challenger.is_none());
    }

    #[test]
    fn test_get_result_not_found() {
        let s = test_storage();
        let row = s.get_result("0xnonexistent").unwrap();
        assert!(row.is_none());
    }

    #[test]
    fn test_update_result_status() {
        let s = test_storage();
        s.insert_result("0xabc", "0xmodel", "0xinput", "0xsubmitter", 100)
            .unwrap();

        s.update_result_status("0xabc", "challenged", Some("0xchallenger"))
            .unwrap();
        let row = s.get_result("0xabc").unwrap().unwrap();
        assert_eq!(row.status, "challenged");
        assert_eq!(row.challenger.as_deref(), Some("0xchallenger"));

        s.update_result_status("0xabc", "resolved", None).unwrap();
        let row = s.get_result("0xabc").unwrap().unwrap();
        assert_eq!(row.status, "resolved");
        assert_eq!(row.challenger.as_deref(), Some("0xchallenger"));
    }

    #[test]
    fn test_insert_duplicate_ignored() {
        let s = test_storage();
        let n1 = s
            .insert_result("0xabc", "0xmodel", "0xinput", "0xsubmitter", 100)
            .unwrap();
        assert_eq!(n1, 1);
        let n2 = s
            .insert_result("0xabc", "0xmodel2", "0xinput2", "0xsubmitter2", 200)
            .unwrap();
        assert_eq!(n2, 0);
        let row = s.get_result("0xabc").unwrap().unwrap();
        assert_eq!(row.model_hash, "0xmodel");
    }

    #[test]
    fn test_list_results_with_filters() {
        let s = test_storage();
        s.insert_result("0x01", "0xm1", "0xi1", "0xalice", 10)
            .unwrap();
        s.insert_result("0x02", "0xm2", "0xi2", "0xbob", 20)
            .unwrap();
        s.insert_result("0x03", "0xm1", "0xi3", "0xalice", 30)
            .unwrap();
        s.update_result_status("0x02", "finalized", None).unwrap();

        let all = s
            .list_results(&ResultFilter {
                status: None,
                submitter: None,
                model_hash: None,
                limit: None,
            })
            .unwrap();
        assert_eq!(all.len(), 3);

        let finalized = s
            .list_results(&ResultFilter {
                status: Some("finalized".to_string()),
                submitter: None,
                model_hash: None,
                limit: None,
            })
            .unwrap();
        assert_eq!(finalized.len(), 1);
        assert_eq!(finalized[0].id, "0x02");

        let alice = s
            .list_results(&ResultFilter {
                status: None,
                submitter: Some("0xalice".to_string()),
                model_hash: None,
                limit: None,
            })
            .unwrap();
        assert_eq!(alice.len(), 2);

        let m1 = s
            .list_results(&ResultFilter {
                status: None,
                submitter: None,
                model_hash: Some("0xm1".to_string()),
                limit: None,
            })
            .unwrap();
        assert_eq!(m1.len(), 2);

        let limited = s
            .list_results(&ResultFilter {
                status: None,
                submitter: None,
                model_hash: None,
                limit: Some(2),
            })
            .unwrap();
        assert_eq!(limited.len(), 2);
    }

    // -- Stats tests --

    #[test]
    fn test_stats_computation() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();
        s.insert_result("0x02", "0xm", "0xi", "0xa", 2).unwrap();
        s.insert_result("0x03", "0xm", "0xi", "0xa", 3).unwrap();
        s.insert_result("0x04", "0xm", "0xi", "0xa", 4).unwrap();

        s.update_result_status("0x02", "challenged", Some("0xc"))
            .unwrap();
        s.update_result_status("0x03", "finalized", None).unwrap();
        s.update_result_status("0x04", "resolved", None).unwrap();

        let stats = s.get_stats().unwrap();
        assert_eq!(stats.total_submitted, 1);
        assert_eq!(stats.total_challenged, 1);
        assert_eq!(stats.total_finalized, 1);
        assert_eq!(stats.total_resolved, 1);
    }

    // -- Indexer state tests --

    #[test]
    fn test_last_indexed_block() {
        let s = test_storage();
        assert_eq!(s.get_last_indexed_block(), 0);
        s.set_last_indexed_block(42).unwrap();
        assert_eq!(s.get_last_indexed_block(), 42);
        s.set_last_indexed_block(100).unwrap();
        assert_eq!(s.get_last_indexed_block(), 100);
    }

    #[test]
    fn test_total_results() {
        let s = test_storage();
        assert_eq!(s.get_total_results(), 0);
        s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();
        s.insert_result("0x02", "0xm", "0xi", "0xa", 2).unwrap();
        assert_eq!(s.get_total_results(), 2);
    }

    // -- Event topic tests --

    #[test]
    fn test_topic_hashes_unique() {
        let t1 = topic_result_submitted();
        let t2 = topic_result_challenged();
        let t3 = topic_result_finalized();
        let t4 = topic_result_expired();
        let t5 = topic_dispute_resolved();
        let all = [t1, t2, t3, t4, t5];
        for i in 0..all.len() {
            for j in (i + 1)..all.len() {
                assert_ne!(all[i], all[j], "topics {} and {} collide", i, j);
            }
        }
    }

    // -- Apply event tests --

    #[test]
    fn test_apply_event_lifecycle() {
        let s = test_storage();

        let rid = B256::from([0x01; 32]);
        let mh = B256::from([0x02; 32]);
        let ih = B256::from([0x03; 32]);
        let sub = Address::from([0x04; 20]);
        let chal = Address::from([0x05; 20]);

        s.apply_event(&TEEEvent::ResultSubmitted {
            result_id: rid,
            model_hash: mh,
            input_hash: ih,
            submitter: sub,
            block_number: 10,
        })
        .unwrap();
        let row = s.get_result(&format!("{rid:#x}")).unwrap().unwrap();
        assert_eq!(row.status, "submitted");

        s.apply_event(&TEEEvent::ResultChallenged {
            result_id: rid,
            challenger: chal,
        })
        .unwrap();
        let row = s.get_result(&format!("{rid:#x}")).unwrap().unwrap();
        assert_eq!(row.status, "challenged");
        assert!(row.challenger.is_some());

        s.apply_event(&TEEEvent::DisputeResolved {
            result_id: rid,
            prover_won: true,
        })
        .unwrap();
        let row = s.get_result(&format!("{rid:#x}")).unwrap().unwrap();
        assert_eq!(row.status, "resolved");
    }

    #[test]
    fn test_apply_event_finalize() {
        let s = test_storage();

        let rid = B256::from([0x10; 32]);
        s.apply_event(&TEEEvent::ResultSubmitted {
            result_id: rid,
            model_hash: B256::ZERO,
            input_hash: B256::ZERO,
            submitter: Address::ZERO,
            block_number: 5,
        })
        .unwrap();

        s.apply_event(&TEEEvent::ResultFinalized { result_id: rid })
            .unwrap();
        let row = s.get_result(&format!("{rid:#x}")).unwrap().unwrap();
        assert_eq!(row.status, "finalized");
    }

    // -- HTTP endpoint tests --

    #[tokio::test]
    async fn test_health_endpoint() {
        let s = test_storage();
        s.set_last_indexed_block(999).unwrap();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();
        let app = build_app(s, test_broadcaster());

        let req = axum::http::Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let health: HealthResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(health.status, "ok");
        assert_eq!(health.last_indexed_block, 999);
        assert_eq!(health.total_results, 1);
    }

    #[tokio::test]
    async fn test_stats_endpoint() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();
        s.insert_result("0x02", "0xm", "0xi", "0xa", 2).unwrap();
        s.update_result_status("0x02", "finalized", None).unwrap();
        let app = build_app(s, test_broadcaster());

        let req = axum::http::Request::builder()
            .uri("/stats")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let stats: StatsResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(stats.total_submitted, 1);
        assert_eq!(stats.total_finalized, 1);
    }

    #[tokio::test]
    async fn test_get_result_endpoint() {
        let s = test_storage();
        s.insert_result("0xabc123", "0xm", "0xi", "0xa", 42)
            .unwrap();
        let app = build_app(s, test_broadcaster());

        let req = axum::http::Request::builder()
            .uri("/results/0xabc123")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let row: ResultRow = serde_json::from_slice(&body).unwrap();
        assert_eq!(row.id, "0xabc123");
        assert_eq!(row.block_number, 42);
    }

    #[tokio::test]
    async fn test_get_result_not_found_endpoint() {
        let s = test_storage();
        let app = build_app(s, test_broadcaster());

        let req = axum::http::Request::builder()
            .uri("/results/0xnonexistent")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_list_results_endpoint() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xalice", 1).unwrap();
        s.insert_result("0x02", "0xm", "0xi", "0xbob", 2).unwrap();
        s.update_result_status("0x02", "finalized", None).unwrap();
        let app = build_app(s, test_broadcaster());

        let req = axum::http::Request::builder()
            .uri("/results")
            .body(Body::empty())
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let rows: Vec<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(rows.len(), 2);

        let req = axum::http::Request::builder()
            .uri("/results?status=finalized")
            .body(Body::empty())
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let rows: Vec<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "0x02");

        let req = axum::http::Request::builder()
            .uri("/results?submitter=0xalice")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let rows: Vec<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "0x01");
    }
}
