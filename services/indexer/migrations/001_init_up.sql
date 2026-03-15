-- Migration 001: Initial schema for tee-indexer
--
-- Creates the core tables used by both SQLite and PostgreSQL backends.

CREATE TABLE IF NOT EXISTS results (
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
);
