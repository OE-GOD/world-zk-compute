-- Migration 001 rollback: drop initial schema tables

DROP TABLE IF EXISTS indexer_state;
DROP TABLE IF EXISTS results;
