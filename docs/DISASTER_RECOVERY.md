# Disaster Recovery Plan

## Recovery Time Objectives

| Component | RTO | RPO | Priority |
|-----------|-----|-----|----------|
| Verifier API | 5 min | 0 (stateless) | P0 |
| API Gateway | 5 min | 0 (stateless) | P0 |
| Proof Registry | 30 min | Last backup | P1 |
| Proof Generator | 15 min | 0 (stateless) | P1 |

## Backup Schedule

```bash
# Daily backup (cron)
0 2 * * * /opt/worldzk/scripts/backup.sh --output-dir /backups/worldzk

# Keep 30 days of backups
find /backups/worldzk -maxdepth 1 -mtime +30 -type d -exec rm -rf {} \;
```

## Recovery Procedures

### Scenario 1: Single Service Failure

```bash
# Restart the failed service
docker compose -f docker-compose.offchain.yml restart <service-name>

# Verify health
worldzk health
```

### Scenario 2: Data Loss (Registry)

```bash
# Stop services
docker compose -f docker-compose.offchain.yml down

# Restore from latest backup
./scripts/restore.sh /backups/worldzk/backup-latest

# Restart
docker compose -f docker-compose.offchain.yml up -d
```

### Scenario 3: Full Infrastructure Loss

```bash
# 1. Provision new infrastructure
cd deploy/terraform && terraform apply

# 2. Deploy services
cd deploy/helm && helm install worldzk .

# 3. Restore data
./scripts/restore.sh /backups/worldzk/backup-latest

# 4. Verify
worldzk health
```

### Scenario 4: On-Chain Contract Compromise

1. **Pause** contracts immediately: `admin-cli --contract <addr> pause`
2. Deploy new implementation behind UUPS proxy
3. Verify new implementation with test suite
4. Upgrade proxy: `admin-cli --contract <addr> upgrade <new-impl>`
5. Unpause: `admin-cli --contract <addr> unpause`

## Testing

Run DR drill quarterly:
```bash
./scripts/backup.sh --output-dir /tmp/dr-test
docker compose -f docker-compose.offchain.yml down
./scripts/restore.sh /tmp/dr-test/backup-*
docker compose -f docker-compose.offchain.yml up -d
worldzk health
```
