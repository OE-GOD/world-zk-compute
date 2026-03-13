-- Migration 001: Initial schema for tee-indexer PostgreSQL backend
--
-- Tables:
--   events     - stores indexed on-chain events
--   sync_state - tracks the last synced block number
--
-- To apply manually:
--   psql $DATABASE_URL -f migrations/001_init.sql

CREATE TABLE IF NOT EXISTS events (
    id BIGSERIAL PRIMARY KEY,
    block_number BIGINT NOT NULL,
    tx_hash VARCHAR(66) NOT NULL,
    log_index INTEGER NOT NULL,
    event_type VARCHAR(50) NOT NULL,
    result_id VARCHAR(66),
    data JSONB NOT NULL,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    UNIQUE(tx_hash, log_index)
);

CREATE INDEX idx_events_block ON events(block_number);
CREATE INDEX idx_events_type ON events(event_type);
CREATE INDEX idx_events_result_id ON events(result_id);

CREATE TABLE IF NOT EXISTS sync_state (
    id INTEGER PRIMARY KEY DEFAULT 1,
    last_block BIGINT NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ DEFAULT NOW()
);

INSERT INTO sync_state (id, last_block) VALUES (1, 0) ON CONFLICT DO NOTHING;
