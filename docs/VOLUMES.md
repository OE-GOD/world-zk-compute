# Deployment Volumes Guide

This document covers storage requirements for each service — which need persistent volumes, which are stateless, and how to configure them.

## Service Storage Requirements

| Service | Storage | Type | Data |
|---------|---------|------|------|
| **Operator** | Persistent | File | State file for crash recovery |
| **Indexer** | Persistent | SQLite/PostgreSQL | Indexed events database |
| **Enclave** | Stateless | — | Model loaded at startup |
| **Prover** | Stateless (cache optional) | File | Proof cache (optional) |
| **Admin CLI** | Stateless | — | CLI tool, no persistent state |

## Operator

The operator stores crash recovery state in a file (default: `./operator-state.json`).

**Required volume:** Mount a persistent volume at the state file path.

```yaml
# Docker Compose
volumes:
  - operator-data:/data
environment:
  - STATE_FILE=/data/operator-state.json

# Kubernetes
volumeMounts:
  - name: operator-data
    mountPath: /data
volumes:
  - name: operator-data
    persistentVolumeClaim:
      claimName: operator-pvc
```

**Backup:** Copy `operator-state.json` periodically. The file is small (< 1 MB typically) and contains pending dispute state. Loss means re-scanning from last known block.

## Indexer

The indexer stores events in SQLite (default) or PostgreSQL.

### SQLite (Default)

**Required volume:** Mount a persistent volume at the database path.

```yaml
# Docker Compose
volumes:
  - indexer-data:/data
environment:
  - DB_PATH=/data/indexer.db
  - DB_TYPE=sqlite

# Kubernetes
volumeMounts:
  - name: indexer-data
    mountPath: /data
```

**Backup:** Copy `indexer.db` file. The database can be rebuilt from chain by re-indexing from block 0, but this is slow.

### PostgreSQL

```yaml
environment:
  - DB_TYPE=postgres
  - DATABASE_URL=postgresql://user:pass@postgres:5432/indexer
```

No local volume needed — data lives in the PostgreSQL instance. Use standard PostgreSQL backup procedures.

## Enclave (TEE)

Stateless. The ML model is loaded at startup from `MODEL_PATH`.

```yaml
# Docker Compose
volumes:
  - ./models:/app/model:ro  # Read-only model mount
environment:
  - MODEL_PATH=/app/model/model.json
```

No persistent volume needed. The enclave generates ephemeral signing keys on startup (or from `ENCLAVE_PRIVATE_KEY` env var). Attestation documents are cached in memory.

## Prover

Stateless by default. An optional proof cache directory can be mounted for faster re-proving.

```yaml
# Docker Compose (optional cache)
volumes:
  - prover-cache:/cache
environment:
  - PROOF_CACHE_DIR=/cache
```

The prover loads guest programs from `programs/` (bundled in the container image). No persistent state is required.

## Kubernetes PVC Examples

```yaml
# Operator PVC
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: operator-pvc
spec:
  accessModes: [ReadWriteOnce]
  resources:
    requests:
      storage: 100Mi

# Indexer PVC (SQLite)
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: indexer-pvc
spec:
  accessModes: [ReadWriteOnce]
  resources:
    requests:
      storage: 1Gi
```

## Backup Procedures

| Service | Method | Frequency | Recovery |
|---------|--------|-----------|----------|
| Operator | Copy state file | Every 5 min | Restore file, service resumes from last state |
| Indexer (SQLite) | Copy `.db` file | Hourly | Restore file, or re-index from chain |
| Indexer (Postgres) | `pg_dump` | Hourly | `pg_restore`, or re-index from chain |

## Docker Compose Reference

```yaml
volumes:
  operator-data:
  indexer-data:

services:
  operator:
    volumes:
      - operator-data:/data
    environment:
      - STATE_FILE=/data/operator-state.json

  indexer:
    volumes:
      - indexer-data:/data
    environment:
      - DB_PATH=/data/indexer.db

  enclave:
    volumes:
      - ./models:/app/model:ro

  prover:
    # No persistent volume needed
```
