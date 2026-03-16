//! Lightweight database migration runner for the tee-indexer.
//!
//! Migrations are embedded at compile time via `include_str!` and tracked in a
//! `_migrations` table. On startup, [`run_migrations`] applies any unapplied
//! migrations in order.
//!
//! Supports both SQLite (`rusqlite::Connection`) and PostgreSQL (`sqlx::PgPool`)
//! backends.

use tracing::info;

/// A single embedded migration.
struct Migration {
    /// Numeric identifier used for ordering and tracking.
    id: i64,
    /// Human-readable name (e.g. "001_init").
    name: &'static str,
    /// The SQL to apply (the "up" script).
    up_sql: &'static str,
}

/// All known migrations, ordered by id. Add new entries here when creating
/// additional migration files.
static MIGRATIONS: &[Migration] = &[Migration {
    id: 1,
    name: "001_init",
    up_sql: include_str!("../migrations/001_init_up.sql"),
}];

// ---------------------------------------------------------------------------
// SQLite
// ---------------------------------------------------------------------------

/// Ensure the `_migrations` tracking table exists in SQLite.
fn ensure_migrations_table_sqlite(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
            id         INTEGER PRIMARY KEY,
            name       TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )?;
    Ok(())
}

/// Return the set of migration ids that have already been applied.
fn applied_migration_ids_sqlite(conn: &rusqlite::Connection) -> anyhow::Result<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT id FROM _migrations ORDER BY id")?;
    let ids: Vec<i64> = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

/// Run all pending migrations against a SQLite connection.
///
/// This is safe to call on every startup -- already-applied migrations are
/// skipped.
pub fn run_migrations_sqlite(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    ensure_migrations_table_sqlite(conn)?;
    let applied = applied_migration_ids_sqlite(conn)?;

    for migration in MIGRATIONS {
        if applied.contains(&migration.id) {
            continue;
        }
        info!(
            "applying migration {} ({}) [sqlite]",
            migration.id, migration.name
        );
        conn.execute_batch(migration.up_sql)?;
        conn.execute(
            "INSERT INTO _migrations (id, name) VALUES (?1, ?2)",
            rusqlite::params![migration.id, migration.name],
        )?;
        info!("migration {} applied successfully", migration.id);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// PostgreSQL
// ---------------------------------------------------------------------------

/// Run all pending migrations against a PostgreSQL pool.
///
/// Requires the `postgres` feature to be enabled.
#[cfg(feature = "postgres")]
pub async fn run_migrations_pg(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _migrations (
            id         INTEGER PRIMARY KEY,
            name       TEXT NOT NULL,
            applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )",
    )
    .execute(pool)
    .await?;

    let applied: Vec<i64> = sqlx::query_scalar("SELECT id FROM _migrations ORDER BY id")
        .fetch_all(pool)
        .await?;

    for migration in MIGRATIONS {
        if applied.contains(&migration.id) {
            continue;
        }
        info!(
            "applying migration {} ({}) [postgres]",
            migration.id, migration.name
        );
        sqlx::query(migration.up_sql).execute(pool).await?;
        sqlx::query("INSERT INTO _migrations (id, name) VALUES ($1, $2)")
            .bind(migration.id)
            .bind(migration.name)
            .execute(pool)
            .await?;
        info!("migration {} applied successfully", migration.id);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Convenience dispatcher
// ---------------------------------------------------------------------------

/// Run pending migrations for the given `db_type`.
///
/// - `"sqlite"` (or anything unrecognised) uses the SQLite path. The caller
///   must pass a valid `sqlite_conn`.
/// - `"postgres"` / `"postgresql"` / `"pg"` uses the PostgreSQL path. The
///   caller must pass a valid `pg_pool` (requires the `postgres` feature).
pub fn run_migrations(
    db_type: &str,
    sqlite_conn: Option<&rusqlite::Connection>,
    #[cfg(feature = "postgres")] pg_pool: Option<&sqlx::PgPool>,
) -> anyhow::Result<()> {
    match db_type.to_lowercase().as_str() {
        "postgres" | "postgresql" | "pg" => {
            #[cfg(feature = "postgres")]
            {
                let pool = pg_pool
                    .ok_or_else(|| anyhow::anyhow!("pg_pool required for postgres migrations"))?;
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(run_migrations_pg(pool))
                })?;
            }
            #[cfg(not(feature = "postgres"))]
            {
                anyhow::bail!(
                    "Postgres migrations require the 'postgres' feature. \
                     Rebuild with: cargo build --features postgres"
                );
            }
        }
        _ => {
            let conn = sqlite_conn
                .ok_or_else(|| anyhow::anyhow!("sqlite_conn required for sqlite migrations"))?;
            run_migrations_sqlite(conn)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migrations_table_created() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        ensure_migrations_table_sqlite(&conn).unwrap();

        // Table should exist and be queryable
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_run_migrations_sqlite_applies_all() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        run_migrations_sqlite(&conn).unwrap();

        // The _migrations table should record exactly one migration
        let applied = applied_migration_ids_sqlite(&conn).unwrap();
        assert_eq!(applied, vec![1]);

        // The results table should have been created by 001_init_up.sql
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM results", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);

        // The indexer_state table should have been created too
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM indexer_state", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_run_migrations_idempotent() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        run_migrations_sqlite(&conn).unwrap();
        // Running again should be a no-op
        run_migrations_sqlite(&conn).unwrap();

        let applied = applied_migration_ids_sqlite(&conn).unwrap();
        assert_eq!(applied, vec![1]);
    }

    #[test]
    fn test_run_migrations_dispatcher_sqlite() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        run_migrations(
            "sqlite",
            Some(&conn),
            #[cfg(feature = "postgres")]
            None,
        )
        .unwrap();

        let applied = applied_migration_ids_sqlite(&conn).unwrap();
        assert_eq!(applied, vec![1]);
    }

    #[test]
    fn test_run_migrations_dispatcher_missing_conn() {
        let result = run_migrations(
            "sqlite",
            None,
            #[cfg(feature = "postgres")]
            None,
        );
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("sqlite_conn required"));
    }

    #[test]
    fn test_migration_records_name_and_timestamp() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        run_migrations_sqlite(&conn).unwrap();

        let (name, applied_at): (String, String) = conn
            .query_row(
                "SELECT name, applied_at FROM _migrations WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(name, "001_init");
        assert!(!applied_at.is_empty());
    }

    #[test]
    fn test_embedded_sql_not_empty() {
        for m in MIGRATIONS {
            assert!(!m.up_sql.is_empty(), "migration {} has empty SQL", m.name);
            assert!(
                m.up_sql.contains("CREATE TABLE"),
                "migration {} should contain CREATE TABLE",
                m.name
            );
        }
    }
}
