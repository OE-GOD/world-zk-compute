//! PostgreSQL storage backend for the tee-indexer.
//!
//! Implements the [`Storage`] trait defined in `main.rs` using `sqlx::PgPool`,
//! and provides additional event-table methods that match the schema in
//! `migrations/001_init.sql` (the `events` and `sync_state` tables).
//!
//! # Configuration
//!
//! Set `DB_TYPE=postgres` and `DATABASE_URL=postgres://user:pass@host/db` in the
//! environment to activate this backend.
//!
//! # Usage
//!
//! ```rust,ignore
//! use pg_storage::{PgStorage, DbType};
//!
//! let db_type = DbType::from_env();
//! let storage: Arc<dyn Storage> = match db_type {
//!     DbType::Postgres => {
//!         let url = std::env::var("DATABASE_URL")?;
//!         Arc::new(PgStorage::new(&url).await?)
//!     }
//!     DbType::Sqlite => Arc::new(SqliteStorage::open(&config.db_path)?),
//! };
//! ```
//!
//! # Schema
//!
//! The migration (`migrations/001_init.sql`) creates:
//!
//! - `events`        -- indexed on-chain events (block_number, tx_hash, log_index,
//!   event_type, result_id, data JSONB)
//! - `sync_state`    -- last synced block number
//! - `results`       -- indexed inference results (for the Storage trait)
//! - `indexer_state`  -- key-value bookkeeping (last indexed block, etc.)

use crate::{ResultFilter, ResultRow, StatsResponse, Storage};
use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::Row;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tee_watcher::TEEEvent;

// ---------------------------------------------------------------------------
// EventRow -- matches the `events` table from 001_init.sql
// ---------------------------------------------------------------------------

/// Row representation for the `events` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRow {
    pub id: i64,
    pub block_number: i64,
    pub tx_hash: String,
    pub log_index: i32,
    pub event_type: String,
    pub result_id: Option<String>,
    pub data: serde_json::Value,
    pub created_at: Option<String>,
}

// ---------------------------------------------------------------------------
// DB_TYPE configuration helper
// ---------------------------------------------------------------------------

/// The type of database backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbType {
    Sqlite,
    Postgres,
}

impl DbType {
    /// Read `DB_TYPE` from the environment. Defaults to `Sqlite` if the
    /// variable is unset or unrecognised.
    pub fn from_env() -> Self {
        match std::env::var("DB_TYPE")
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "postgres" | "postgresql" | "pg" => DbType::Postgres,
            _ => DbType::Sqlite,
        }
    }
}

// ---------------------------------------------------------------------------
// PgStorage
// ---------------------------------------------------------------------------

/// PostgreSQL-backed storage using `sqlx::PgPool`.
///
/// Wraps a connection pool (max 10 connections) and implements both:
/// - The [`Storage`] trait (for compatibility with the existing API handlers)
/// - Event-table methods matching `migrations/001_init.sql`
pub struct PgStorage {
    pool: PgPool,
    /// Cached copy of the last indexed block so `get_last_indexed_block()`
    /// (called on every poll cycle) avoids a DB round-trip.
    last_indexed_block: AtomicU64,
}

/// Configuration for the PostgreSQL connection pool.
///
/// All fields can be populated from environment variables via
/// [`PgStorageConfig::from_env()`]:
///
/// | Field              | Env Var                    | Default |
/// |--------------------|----------------------------|---------|
/// | `max_connections`  | `DB_MAX_CONNECTIONS`       | 10      |
/// | `min_connections`  | `DB_MIN_CONNECTIONS`       | 2       |
/// | `acquire_timeout`  | `DB_ACQUIRE_TIMEOUT_SECS`  | 30 s    |
/// | `max_lifetime`     | `DB_MAX_LIFETIME_SECS`     | 1800 s  |
/// | `idle_timeout`     | `DB_IDLE_TIMEOUT_SECS`     | 600 s   |
pub struct PgStorageConfig {
    /// Maximum number of connections in the pool (default: 10).
    pub max_connections: u32,
    /// Minimum number of idle connections to maintain (default: 2).
    pub min_connections: u32,
    /// Maximum time to wait when acquiring a connection from the pool
    /// (default: 30 seconds).
    pub acquire_timeout: Duration,
    /// Maximum lifetime of a connection before it is closed and replaced
    /// (default: 1800 seconds / 30 minutes).
    pub max_lifetime: Duration,
    /// Maximum time a connection can sit idle before being closed
    /// (default: 600 seconds / 10 minutes).
    pub idle_timeout: Duration,
}

impl Default for PgStorageConfig {
    fn default() -> Self {
        Self {
            max_connections: 10,
            min_connections: 2,
            acquire_timeout: Duration::from_secs(30),
            max_lifetime: Duration::from_secs(1800),
            idle_timeout: Duration::from_secs(600),
        }
    }
}

impl PgStorageConfig {
    /// Read pool configuration from environment variables, falling back to
    /// defaults for any variable that is unset or unparseable.
    ///
    /// Logs a warning when a variable is set but cannot be parsed.
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            max_connections: Self::parse_env_u32("DB_MAX_CONNECTIONS", defaults.max_connections),
            min_connections: Self::parse_env_u32("DB_MIN_CONNECTIONS", defaults.min_connections),
            acquire_timeout: Duration::from_secs(Self::parse_env_u64(
                "DB_ACQUIRE_TIMEOUT_SECS",
                defaults.acquire_timeout.as_secs(),
            )),
            max_lifetime: Duration::from_secs(Self::parse_env_u64(
                "DB_MAX_LIFETIME_SECS",
                defaults.max_lifetime.as_secs(),
            )),
            idle_timeout: Duration::from_secs(Self::parse_env_u64(
                "DB_IDLE_TIMEOUT_SECS",
                defaults.idle_timeout.as_secs(),
            )),
        }
    }

    fn parse_env_u32(name: &str, default: u32) -> u32 {
        match std::env::var(name) {
            Ok(val) => match val.parse::<u32>() {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse {}={:?}: {} -- using default {}",
                        name,
                        val,
                        e,
                        default,
                    );
                    default
                }
            },
            Err(_) => default,
        }
    }

    fn parse_env_u64(name: &str, default: u64) -> u64 {
        match std::env::var(name) {
            Ok(val) => match val.parse::<u64>() {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse {}={:?}: {} -- using default {}",
                        name,
                        val,
                        e,
                        default,
                    );
                    default
                }
            },
            Err(_) => default,
        }
    }
}

impl PgStorage {
    /// Create a new `PgStorage` connected to the given `database_url`.
    ///
    /// The pool is configured with a maximum of 10 connections. On creation the
    /// required tables are ensured to exist via [`init_tables`], and the cached
    /// last-indexed-block is hydrated from the database.
    pub async fn new(database_url: &str) -> anyhow::Result<Self> {
        Self::connect_with_config(database_url, PgStorageConfig::default()).await
    }

    /// Alias for [`new`](Self::new) -- retained for backward compatibility.
    pub async fn connect(database_url: &str) -> anyhow::Result<Self> {
        Self::new(database_url).await
    }

    /// Connect to PostgreSQL with explicit pool configuration.
    pub async fn connect_with_config(
        database_url: &str,
        config: PgStorageConfig,
    ) -> anyhow::Result<Self> {
        tracing::info!(
            "PgStorage pool config: max_connections={}, min_connections={}, \
             acquire_timeout={}s, max_lifetime={}s, idle_timeout={}s",
            config.max_connections,
            config.min_connections,
            config.acquire_timeout.as_secs(),
            config.max_lifetime.as_secs(),
            config.idle_timeout.as_secs(),
        );

        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .min_connections(config.min_connections)
            .acquire_timeout(config.acquire_timeout)
            .max_lifetime(config.max_lifetime)
            .idle_timeout(config.idle_timeout)
            .connect(database_url)
            .await?;

        let storage = Self {
            pool,
            last_indexed_block: AtomicU64::new(0),
        };

        storage.init_tables().await?;
        storage.hydrate_last_indexed_block().await?;

        tracing::info!(
            "PgStorage connected to PostgreSQL (pool max={})",
            config.max_connections
        );
        Ok(storage)
    }

    /// Ensure all required tables exist. Covers both the migration schema
    /// (events + sync_state) and the results/indexer_state tables needed by
    /// the Storage trait.
    async fn init_tables(&self) -> anyhow::Result<()> {
        // Events table (from migration 001_init.sql)
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS events (
                id BIGSERIAL PRIMARY KEY,
                block_number BIGINT NOT NULL,
                tx_hash VARCHAR(66) NOT NULL,
                log_index INTEGER NOT NULL,
                event_type VARCHAR(50) NOT NULL,
                result_id VARCHAR(66),
                data JSONB NOT NULL,
                created_at TIMESTAMPTZ DEFAULT NOW(),
                UNIQUE(tx_hash, log_index)
            )",
        )
        .execute(&self.pool)
        .await?;

        for ddl in &[
            "CREATE INDEX IF NOT EXISTS idx_events_block ON events(block_number)",
            "CREATE INDEX IF NOT EXISTS idx_events_type ON events(event_type)",
            "CREATE INDEX IF NOT EXISTS idx_events_result_id ON events(result_id)",
        ] {
            sqlx::query(ddl).execute(&self.pool).await?;
        }

        // Sync state table
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS sync_state (
                id INTEGER PRIMARY KEY DEFAULT 1,
                last_block BIGINT NOT NULL DEFAULT 0,
                updated_at TIMESTAMPTZ DEFAULT NOW()
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query("INSERT INTO sync_state (id, last_block) VALUES (1, 0) ON CONFLICT DO NOTHING")
            .execute(&self.pool)
            .await?;

        // Results table (Storage trait)
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS results (
                id TEXT PRIMARY KEY,
                model_hash TEXT NOT NULL,
                input_hash TEXT NOT NULL,
                output TEXT NOT NULL DEFAULT '',
                submitter TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'submitted',
                block_number BIGINT NOT NULL DEFAULT 0,
                timestamp BIGINT NOT NULL DEFAULT 0,
                challenger TEXT
            )",
        )
        .execute(&self.pool)
        .await?;

        // Indexer state table (Storage trait)
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS indexer_state (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Run the initial migration from the SQL file. This is a convenience
    /// method for deployments that want to auto-migrate without running the
    /// SQL file manually.
    pub async fn run_migrations(&self) -> anyhow::Result<()> {
        sqlx::query(include_str!("../migrations/001_init.sql"))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Read the last indexed block from the database and cache it.
    async fn hydrate_last_indexed_block(&self) -> anyhow::Result<()> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM indexer_state WHERE key = 'last_indexed_block'")
                .fetch_optional(&self.pool)
                .await?;

        let block = row.and_then(|(v,)| v.parse::<u64>().ok()).unwrap_or(0);

        self.last_indexed_block.store(block, Ordering::SeqCst);
        Ok(())
    }

    /// Get a reference to the underlying connection pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    // -----------------------------------------------------------------------
    // Event-table methods (matching the migration schema)
    // -----------------------------------------------------------------------

    /// Insert an event into the `events` table. Duplicates (same tx_hash +
    /// log_index) are silently ignored via `ON CONFLICT DO NOTHING`.
    pub async fn insert_event(
        &self,
        block_number: i64,
        tx_hash: &str,
        log_index: i32,
        event_type: &str,
        result_id: Option<&str>,
        data: &serde_json::Value,
    ) -> anyhow::Result<u64> {
        let result = sqlx::query(
            "INSERT INTO events (block_number, tx_hash, log_index, event_type, result_id, data)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (tx_hash, log_index) DO NOTHING",
        )
        .bind(block_number)
        .bind(tx_hash)
        .bind(log_index)
        .bind(event_type)
        .bind(result_id)
        .bind(data)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Retrieve all events associated with a given `result_id`, ordered by
    /// block number ascending.
    pub async fn get_events_by_result_id(&self, result_id: &str) -> anyhow::Result<Vec<EventRow>> {
        let rows = sqlx::query(
            "SELECT id, block_number, tx_hash, log_index, event_type, result_id, data,
                    created_at::text
             FROM events
             WHERE result_id = $1
             ORDER BY block_number ASC, log_index ASC",
        )
        .bind(result_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(pg_row_to_event).collect())
    }

    /// Retrieve events of a given type starting from `from_block`, ordered by
    /// block number ascending.
    pub async fn get_events_by_type(
        &self,
        event_type: &str,
        from_block: i64,
    ) -> anyhow::Result<Vec<EventRow>> {
        let rows = sqlx::query(
            "SELECT id, block_number, tx_hash, log_index, event_type, result_id, data,
                    created_at::text
             FROM events
             WHERE event_type = $1 AND block_number >= $2
             ORDER BY block_number ASC, log_index ASC",
        )
        .bind(event_type)
        .bind(from_block)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(pg_row_to_event).collect())
    }

    /// Get the latest synced block from the `sync_state` table, or `None` if
    /// the table has not been seeded.
    pub async fn get_latest_block(&self) -> anyhow::Result<Option<u64>> {
        let row = sqlx::query("SELECT last_block FROM sync_state WHERE id = 1")
            .fetch_optional(&self.pool)
            .await?;

        row.map(|r| {
            let v: i64 = r.get(0);
            u64::try_from(v).context("negative block number in sync_state")
        })
        .transpose()
    }

    /// Update the latest synced block in the `sync_state` table.
    pub async fn set_latest_block(&self, block_number: u64) -> anyhow::Result<()> {
        let block_i64 =
            i64::try_from(block_number).context("block number overflow in set_latest_block")?;
        sqlx::query("UPDATE sync_state SET last_block = $1, updated_at = NOW() WHERE id = 1")
            .bind(block_i64)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Async implementations for Storage trait methods
    // -----------------------------------------------------------------------

    async fn insert_result_async(
        &self,
        id: &str,
        model_hash: &str,
        input_hash: &str,
        submitter: &str,
        block_number: u64,
    ) -> anyhow::Result<usize> {
        let result = sqlx::query(
            "INSERT INTO results (id, model_hash, input_hash, submitter, status, block_number)
             VALUES ($1, $2, $3, $4, 'submitted', $5)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(id)
        .bind(model_hash)
        .bind(input_hash)
        .bind(submitter)
        .bind(i64::try_from(block_number).context("block number overflow in insert_result")?)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() as usize)
    }

    async fn update_result_status_async(
        &self,
        id: &str,
        status: &str,
        challenger: Option<&str>,
    ) -> anyhow::Result<usize> {
        let result = if let Some(c) = challenger {
            sqlx::query("UPDATE results SET status = $1, challenger = $2 WHERE id = $3")
                .bind(status)
                .bind(c)
                .bind(id)
                .execute(&self.pool)
                .await?
        } else {
            sqlx::query("UPDATE results SET status = $1 WHERE id = $2")
                .bind(status)
                .bind(id)
                .execute(&self.pool)
                .await?
        };

        Ok(result.rows_affected() as usize)
    }

    async fn get_result_async(&self, id: &str) -> anyhow::Result<Option<ResultRow>> {
        let row = sqlx::query(
            "SELECT id, model_hash, input_hash, output, submitter, status,
                    block_number, timestamp, challenger
             FROM results WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| ResultRow {
            id: r.get(0),
            model_hash: r.get(1),
            input_hash: r.get(2),
            output: r.get(3),
            submitter: r.get(4),
            status: r.get(5),
            block_number: r.get(6),
            timestamp: r.get(7),
            challenger: r.get(8),
        }))
    }

    async fn list_results_async(&self, filter: &ResultFilter) -> anyhow::Result<Vec<ResultRow>> {
        let mut sql = String::from(
            "SELECT id, model_hash, input_hash, output, submitter, status,
                    block_number, timestamp, challenger
             FROM results WHERE true",
        );

        let mut bind_values: Vec<String> = Vec::new();
        let mut param_count: usize = 0;

        if let Some(ref status) = filter.status {
            bind_values.push(status.clone());
            param_count += 1;
            sql.push_str(&format!(" AND status = ${param_count}"));
        }
        if let Some(ref submitter) = filter.submitter {
            bind_values.push(submitter.clone());
            param_count += 1;
            sql.push_str(&format!(" AND submitter = ${param_count}"));
        }
        if let Some(ref model_hash) = filter.model_hash {
            bind_values.push(model_hash.clone());
            param_count += 1;
            sql.push_str(&format!(" AND model_hash = ${param_count}"));
        }

        // Cursor-based pagination: skip rows up to and including the cursor ID.
        // Works with the default sort order (block_number DESC, id ASC).
        if let Some(ref after_id) = filter.after_id {
            bind_values.push(after_id.clone());
            param_count += 1;
            let p = param_count;
            sql.push_str(&format!(
                " AND (block_number < (SELECT block_number FROM results WHERE id = ${p}) \
                 OR (block_number = (SELECT block_number FROM results WHERE id = ${p}) \
                 AND id > ${p}))"
            ));
        }

        // Sorting: whitelist allowed columns to prevent SQL injection.
        let order_col = match filter.sort_by.as_deref() {
            Some("block_number") => "block_number",
            Some("submitted_at") => "timestamp",
            Some("status") => "status",
            Some("submitter") => "submitter",
            _ => "block_number",
        };
        let order_dir = match filter.sort_order.as_deref() {
            Some("asc") => "ASC",
            Some("desc") => "DESC",
            _ => "DESC",
        };
        sql.push_str(&format!(" ORDER BY {order_col} {order_dir}, id ASC"));

        // Clamp limit to [1, 1000] with default 100 to prevent abuse.
        let limit = i64::from(filter.limit.unwrap_or(100).clamp(1, 1000));
        param_count += 1;
        sql.push_str(&format!(" LIMIT ${param_count}"));

        // Offset-based pagination (mutually exclusive with after_id).
        // Use saturating conversion to prevent overflow; floor at 0.
        let offset = if filter.after_id.is_none() {
            i64::from(filter.offset.unwrap_or(0))
        } else {
            0
        };
        if offset > 0 {
            param_count += 1;
            sql.push_str(&format!(" OFFSET ${param_count}"));
        }

        let mut query = sqlx::query(&sql);
        for val in &bind_values {
            query = query.bind(val.clone());
        }
        query = query.bind(limit);
        if offset > 0 {
            query = query.bind(offset);
        }

        let rows = query.fetch_all(&self.pool).await?;
        Ok(rows
            .iter()
            .map(|r| ResultRow {
                id: r.get(0),
                model_hash: r.get(1),
                input_hash: r.get(2),
                output: r.get(3),
                submitter: r.get(4),
                status: r.get(5),
                block_number: r.get(6),
                timestamp: r.get(7),
                challenger: r.get(8),
            })
            .collect())
    }

    async fn count_results_async(&self, filter: &ResultFilter) -> anyhow::Result<u64> {
        let mut sql = String::from("SELECT COUNT(*) FROM results WHERE true");
        let mut bind_values: Vec<String> = Vec::new();
        let mut param_count: usize = 0;

        if let Some(ref status) = filter.status {
            bind_values.push(status.clone());
            param_count += 1;
            sql.push_str(&format!(" AND status = ${param_count}"));
        }
        if let Some(ref submitter) = filter.submitter {
            bind_values.push(submitter.clone());
            param_count += 1;
            sql.push_str(&format!(" AND submitter = ${param_count}"));
        }
        if let Some(ref model_hash) = filter.model_hash {
            bind_values.push(model_hash.clone());
            param_count += 1;
            sql.push_str(&format!(" AND model_hash = ${param_count}"));
        }

        let mut query = sqlx::query(&sql);
        for val in &bind_values {
            query = query.bind(val.clone());
        }

        let row = query.fetch_one(&self.pool).await?;
        let count: i64 = row.get(0);
        u64::try_from(count).context("negative count in count_results")
    }

    async fn get_stats_async(&self) -> anyhow::Result<StatsResponse> {
        let row = sqlx::query(
            "SELECT
                COALESCE(SUM(CASE WHEN status = 'submitted'  THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status = 'challenged' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status = 'finalized'  THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status = 'resolved'   THEN 1 ELSE 0 END), 0)
             FROM results",
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(StatsResponse {
            total_submitted: u64::try_from(row.get::<i64, _>(0))
                .context("negative submitted count")?,
            total_challenged: u64::try_from(row.get::<i64, _>(1))
                .context("negative challenged count")?,
            total_finalized: u64::try_from(row.get::<i64, _>(2))
                .context("negative finalized count")?,
            total_resolved: u64::try_from(row.get::<i64, _>(3))
                .context("negative resolved count")?,
        })
    }

    async fn set_last_indexed_block_async(&self, block: u64) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO indexer_state (key, value) VALUES ('last_indexed_block', $1)
             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
        )
        .bind(block.to_string())
        .execute(&self.pool)
        .await?;

        self.last_indexed_block.store(block, Ordering::SeqCst);
        Ok(())
    }

    async fn get_total_results_async(&self) -> u64 {
        let row: Result<(i64,), _> = sqlx::query_as("SELECT COUNT(*) FROM results")
            .fetch_one(&self.pool)
            .await;

        row.ok().and_then(|(c,)| u64::try_from(c).ok()).unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Helper: convert a sqlx::postgres::PgRow to EventRow
// ---------------------------------------------------------------------------

fn pg_row_to_event(row: &sqlx::postgres::PgRow) -> EventRow {
    EventRow {
        id: row.get(0),
        block_number: row.get(1),
        tx_hash: row.get(2),
        log_index: row.get(3),
        event_type: row.get(4),
        result_id: row.get(5),
        data: row.get(6),
        created_at: row.get(7),
    }
}

// ---------------------------------------------------------------------------
// Storage trait implementation
// ---------------------------------------------------------------------------
//
// The `Storage` trait is synchronous. Since `sqlx` is async, we bridge using
// `tokio::task::block_in_place` + `Handle::current().block_on()`.
//
// `block_in_place` is safe to call from within a multi-threaded tokio runtime
// (the indexer uses `#[tokio::main]` which defaults to multi-threaded). It
// moves the current task off the worker thread so that `block_on` does not
// deadlock.

impl Storage for PgStorage {
    fn insert_result(
        &self,
        id: &str,
        model_hash: &str,
        input_hash: &str,
        submitter: &str,
        block_number: u64,
    ) -> anyhow::Result<usize> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.insert_result_async(
                id,
                model_hash,
                input_hash,
                submitter,
                block_number,
            ))
        })
    }

    fn update_result_status(
        &self,
        id: &str,
        status: &str,
        challenger: Option<&str>,
    ) -> anyhow::Result<usize> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(self.update_result_status_async(id, status, challenger))
        })
    }

    fn get_result(&self, id: &str) -> anyhow::Result<Option<ResultRow>> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.get_result_async(id))
        })
    }

    fn list_results(&self, filter: &ResultFilter) -> anyhow::Result<Vec<ResultRow>> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.list_results_async(filter))
        })
    }

    fn count_results(&self, filter: &ResultFilter) -> anyhow::Result<u64> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.count_results_async(filter))
        })
    }

    fn get_stats(&self) -> anyhow::Result<StatsResponse> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.get_stats_async())
        })
    }

    fn get_last_indexed_block(&self) -> u64 {
        self.last_indexed_block.load(Ordering::SeqCst)
    }

    fn set_last_indexed_block(&self, block: u64) -> anyhow::Result<()> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.set_last_indexed_block_async(block))
        })
    }

    fn get_total_results(&self) -> u64 {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.get_total_results_async())
        })
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Unit tests (no PostgreSQL instance required)
    // -----------------------------------------------------------------------

    #[test]
    fn test_db_type_defaults_to_sqlite() {
        std::env::remove_var("DB_TYPE");
        assert_eq!(DbType::from_env(), DbType::Sqlite);
    }

    #[test]
    fn test_db_type_recognises_postgres_variants() {
        for variant in &[
            "postgres",
            "postgresql",
            "pg",
            "POSTGRES",
            "PostgreSQL",
            "PG",
        ] {
            std::env::set_var("DB_TYPE", variant);
            assert_eq!(
                DbType::from_env(),
                DbType::Postgres,
                "failed for variant: {variant}"
            );
        }
        std::env::remove_var("DB_TYPE");
    }

    #[test]
    fn test_db_type_unknown_falls_back_to_sqlite() {
        std::env::set_var("DB_TYPE", "mysql");
        assert_eq!(DbType::from_env(), DbType::Sqlite);
        std::env::remove_var("DB_TYPE");
    }

    #[test]
    fn test_db_type_equality_and_debug() {
        let a = DbType::Postgres;
        let b = DbType::Postgres;
        let c = DbType::Sqlite;
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(format!("{:?}", a), "Postgres");
        assert_eq!(format!("{:?}", c), "Sqlite");
    }

    #[test]
    fn test_event_row_serialization_roundtrip() {
        let event = EventRow {
            id: 1,
            block_number: 12345,
            tx_hash: "0xabc123".to_string(),
            log_index: 0,
            event_type: "ResultSubmitted".to_string(),
            result_id: Some("0xdef456".to_string()),
            data: serde_json::json!({"submitter": "0xalice", "model_hash": "0xm1"}),
            created_at: Some("2026-01-01T00:00:00Z".to_string()),
        };
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: EventRow = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, 1);
        assert_eq!(deserialized.block_number, 12345);
        assert_eq!(deserialized.tx_hash, "0xabc123");
        assert_eq!(deserialized.event_type, "ResultSubmitted");
        assert_eq!(deserialized.result_id, Some("0xdef456".to_string()));
    }

    #[test]
    fn test_event_row_with_null_result_id() {
        let event = EventRow {
            id: 2,
            block_number: 99,
            tx_hash: "0x000".to_string(),
            log_index: 3,
            event_type: "Unknown".to_string(),
            result_id: None,
            data: serde_json::json!({}),
            created_at: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"result_id\":null"));
        let back: EventRow = serde_json::from_str(&json).unwrap();
        assert!(back.result_id.is_none());
        assert!(back.created_at.is_none());
    }

    #[test]
    fn test_event_row_data_field_complex_json() {
        let data = serde_json::json!({
            "result_id": "0xabc",
            "model_hash": "0xdef",
            "nested": {
                "array": [1, 2, 3],
                "flag": true
            }
        });
        let event = EventRow {
            id: 10,
            block_number: 500,
            tx_hash: "0xtx".to_string(),
            log_index: 1,
            event_type: "ResultSubmitted".to_string(),
            result_id: Some("0xabc".to_string()),
            data: data.clone(),
            created_at: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: EventRow = serde_json::from_str(&json).unwrap();
        assert_eq!(back.data, data);
        assert_eq!(back.data["nested"]["array"][1], 2);
    }

    #[test]
    fn test_atomic_last_indexed_block_caching() {
        let atomic = AtomicU64::new(0);
        assert_eq!(atomic.load(Ordering::SeqCst), 0);

        atomic.store(42, Ordering::SeqCst);
        assert_eq!(atomic.load(Ordering::SeqCst), 42);

        atomic.store(u64::MAX, Ordering::SeqCst);
        assert_eq!(atomic.load(Ordering::SeqCst), u64::MAX);
    }

    #[test]
    fn test_default_config() {
        let config = PgStorageConfig::default();
        assert_eq!(config.max_connections, 10);
        assert_eq!(config.min_connections, 2);
        assert_eq!(config.acquire_timeout, Duration::from_secs(30));
        assert_eq!(config.max_lifetime, Duration::from_secs(1800));
        assert_eq!(config.idle_timeout, Duration::from_secs(600));
    }

    #[test]
    fn test_custom_config() {
        let config = PgStorageConfig {
            max_connections: 20,
            min_connections: 5,
            acquire_timeout: Duration::from_secs(60),
            max_lifetime: Duration::from_secs(3600),
            idle_timeout: Duration::from_secs(300),
        };
        assert_eq!(config.max_connections, 20);
        assert_eq!(config.min_connections, 5);
        assert_eq!(config.acquire_timeout, Duration::from_secs(60));
        assert_eq!(config.max_lifetime, Duration::from_secs(3600));
        assert_eq!(config.idle_timeout, Duration::from_secs(300));
    }

    /// All `from_env` tests in a single function to avoid env-var race
    /// conditions when tests run in parallel.
    #[test]
    fn test_config_from_env() {
        let env_vars = [
            "DB_MAX_CONNECTIONS",
            "DB_MIN_CONNECTIONS",
            "DB_ACQUIRE_TIMEOUT_SECS",
            "DB_MAX_LIFETIME_SECS",
            "DB_IDLE_TIMEOUT_SECS",
        ];

        // --- defaults (all vars cleared) ---
        for var in &env_vars {
            std::env::remove_var(var);
        }
        let config = PgStorageConfig::from_env();
        assert_eq!(config.max_connections, 10);
        assert_eq!(config.min_connections, 2);
        assert_eq!(config.acquire_timeout, Duration::from_secs(30));
        assert_eq!(config.max_lifetime, Duration::from_secs(1800));
        assert_eq!(config.idle_timeout, Duration::from_secs(600));

        // --- full custom values ---
        std::env::set_var("DB_MAX_CONNECTIONS", "25");
        std::env::set_var("DB_MIN_CONNECTIONS", "5");
        std::env::set_var("DB_ACQUIRE_TIMEOUT_SECS", "10");
        std::env::set_var("DB_MAX_LIFETIME_SECS", "900");
        std::env::set_var("DB_IDLE_TIMEOUT_SECS", "120");
        let config = PgStorageConfig::from_env();
        assert_eq!(config.max_connections, 25);
        assert_eq!(config.min_connections, 5);
        assert_eq!(config.acquire_timeout, Duration::from_secs(10));
        assert_eq!(config.max_lifetime, Duration::from_secs(900));
        assert_eq!(config.idle_timeout, Duration::from_secs(120));

        // --- invalid values fall back to defaults ---
        std::env::set_var("DB_MAX_CONNECTIONS", "not_a_number");
        std::env::set_var("DB_ACQUIRE_TIMEOUT_SECS", "abc");
        std::env::remove_var("DB_MIN_CONNECTIONS");
        std::env::remove_var("DB_MAX_LIFETIME_SECS");
        std::env::remove_var("DB_IDLE_TIMEOUT_SECS");
        let config = PgStorageConfig::from_env();
        assert_eq!(config.max_connections, 10);
        assert_eq!(config.acquire_timeout, Duration::from_secs(30));
        assert_eq!(config.min_connections, 2);

        // --- partial override ---
        for var in &env_vars {
            std::env::remove_var(var);
        }
        std::env::set_var("DB_MAX_CONNECTIONS", "50");
        std::env::set_var("DB_IDLE_TIMEOUT_SECS", "300");
        let config = PgStorageConfig::from_env();
        assert_eq!(config.max_connections, 50);
        assert_eq!(config.min_connections, 2);
        assert_eq!(config.acquire_timeout, Duration::from_secs(30));
        assert_eq!(config.idle_timeout, Duration::from_secs(300));

        // --- clean up ---
        for var in &env_vars {
            std::env::remove_var(var);
        }
    }

    #[test]
    fn test_event_row_default_values() {
        let event = EventRow {
            id: 0,
            block_number: 0,
            tx_hash: String::new(),
            log_index: 0,
            event_type: String::new(),
            result_id: None,
            data: serde_json::Value::Null,
            created_at: None,
        };
        assert_eq!(event.id, 0);
        assert!(event.created_at.is_none());
        assert!(event.result_id.is_none());

        // Verify JSON round-trip with default/null values
        let json = serde_json::to_string(&event).unwrap();
        let back: EventRow = serde_json::from_str(&json).unwrap();
        assert_eq!(back.block_number, 0);
        assert_eq!(back.data, serde_json::Value::Null);
    }

    #[test]
    fn test_event_row_clone_and_debug() {
        let event = EventRow {
            id: 42,
            block_number: 100,
            tx_hash: "0xhash".to_string(),
            log_index: 2,
            event_type: "ResultFinalized".to_string(),
            result_id: Some("0xresult".to_string()),
            data: serde_json::json!({"key": "value"}),
            created_at: Some("now".to_string()),
        };
        let cloned = event.clone();
        assert_eq!(cloned.id, event.id);
        assert_eq!(cloned.tx_hash, event.tx_hash);

        let debug_str = format!("{:?}", event);
        assert!(debug_str.contains("EventRow"));
        assert!(debug_str.contains("42"));
    }

    // -----------------------------------------------------------------------
    // Integration tests requiring a live PostgreSQL instance.
    //
    // Gated behind `#[ignore]`. To run:
    //
    //   DATABASE_URL=postgres://user:pass@localhost/indexer_test \
    //     cargo test --package tee-indexer pg_storage -- --ignored
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[ignore = "requires a running PostgreSQL instance (set DATABASE_URL)"]
    async fn test_pg_insert_and_get_event() {
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must be set for PG integration tests");
        let pg = PgStorage::new(&url).await.unwrap();

        sqlx::query("DELETE FROM events")
            .execute(pg.pool())
            .await
            .unwrap();

        let data = serde_json::json!({"submitter": "0xalice"});
        let inserted = pg
            .insert_event(
                100,
                "0xtxhash1",
                0,
                "ResultSubmitted",
                Some("0xresult1"),
                &data,
            )
            .await
            .unwrap();
        assert_eq!(inserted, 1);

        let events = pg.get_events_by_result_id("0xresult1").await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].block_number, 100);
        assert_eq!(events[0].event_type, "ResultSubmitted");
        assert_eq!(events[0].tx_hash, "0xtxhash1");
    }

    #[tokio::test]
    #[ignore = "requires a running PostgreSQL instance (set DATABASE_URL)"]
    async fn test_pg_duplicate_event_ignored() {
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must be set for PG integration tests");
        let pg = PgStorage::new(&url).await.unwrap();

        sqlx::query("DELETE FROM events")
            .execute(pg.pool())
            .await
            .unwrap();

        let data = serde_json::json!({});
        pg.insert_event(1, "0xdup_tx", 0, "ResultSubmitted", Some("0xr"), &data)
            .await
            .unwrap();
        let dup = pg
            .insert_event(1, "0xdup_tx", 0, "ResultChallenged", Some("0xr"), &data)
            .await
            .unwrap();
        assert_eq!(dup, 0, "duplicate (tx_hash, log_index) should be ignored");
    }

    #[tokio::test]
    #[ignore = "requires a running PostgreSQL instance (set DATABASE_URL)"]
    async fn test_pg_get_events_by_type() {
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must be set for PG integration tests");
        let pg = PgStorage::new(&url).await.unwrap();

        sqlx::query("DELETE FROM events")
            .execute(pg.pool())
            .await
            .unwrap();

        let data = serde_json::json!({});
        pg.insert_event(10, "0xt1", 0, "ResultSubmitted", Some("0xr1"), &data)
            .await
            .unwrap();
        pg.insert_event(20, "0xt2", 0, "ResultChallenged", Some("0xr2"), &data)
            .await
            .unwrap();
        pg.insert_event(30, "0xt3", 0, "ResultSubmitted", Some("0xr3"), &data)
            .await
            .unwrap();

        let submitted = pg.get_events_by_type("ResultSubmitted", 0).await.unwrap();
        assert_eq!(submitted.len(), 2);

        let from_25 = pg.get_events_by_type("ResultSubmitted", 25).await.unwrap();
        assert_eq!(from_25.len(), 1);
        assert_eq!(from_25[0].block_number, 30);
    }

    #[tokio::test]
    #[ignore = "requires a running PostgreSQL instance (set DATABASE_URL)"]
    async fn test_pg_sync_state() {
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must be set for PG integration tests");
        let pg = PgStorage::new(&url).await.unwrap();

        let block = pg.get_latest_block().await.unwrap();
        assert!(block.is_some(), "sync_state should be seeded with 0");

        pg.set_latest_block(42).await.unwrap();
        let block = pg.get_latest_block().await.unwrap();
        assert_eq!(block, Some(42));

        pg.set_latest_block(100).await.unwrap();
        let block = pg.get_latest_block().await.unwrap();
        assert_eq!(block, Some(100));
    }

    #[tokio::test]
    #[ignore = "requires a running PostgreSQL instance (set DATABASE_URL)"]
    async fn test_pg_storage_trait_insert_and_get() {
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must be set for PG integration tests");
        let pg = PgStorage::new(&url).await.unwrap();

        sqlx::query("DELETE FROM results")
            .execute(pg.pool())
            .await
            .unwrap();

        pg.insert_result_async("0xpg_abc", "0xmodel", "0xinput", "0xsubmitter", 100)
            .await
            .unwrap();

        let row = pg.get_result_async("0xpg_abc").await.unwrap().unwrap();
        assert_eq!(row.id, "0xpg_abc");
        assert_eq!(row.model_hash, "0xmodel");
        assert_eq!(row.status, "submitted");
        assert_eq!(row.block_number, 100);
    }

    #[tokio::test]
    #[ignore = "requires a running PostgreSQL instance (set DATABASE_URL)"]
    async fn test_pg_storage_trait_stats() {
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must be set for PG integration tests");
        let pg = PgStorage::new(&url).await.unwrap();

        sqlx::query("DELETE FROM results")
            .execute(pg.pool())
            .await
            .unwrap();

        pg.insert_result_async("0xst1", "0xm", "0xi", "0xa", 1)
            .await
            .unwrap();
        pg.insert_result_async("0xst2", "0xm", "0xi", "0xa", 2)
            .await
            .unwrap();
        pg.update_result_status_async("0xst2", "finalized", None)
            .await
            .unwrap();

        let stats = pg.get_stats_async().await.unwrap();
        assert_eq!(stats.total_submitted, 1);
        assert_eq!(stats.total_finalized, 1);
        assert_eq!(stats.total_challenged, 0);
    }
}
