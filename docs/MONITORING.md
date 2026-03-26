# Monitoring Guide

This document describes how to set up Prometheus and Grafana for monitoring the World ZK Compute operator and verifier services.

## Architecture

```
Operator Service (:9090/metrics)  -->  Prometheus (:9091)  -->  Grafana (:3000)
                                         ^
Operator Health  (:9090/health)  --------+
```

The **operator service** exposes Prometheus-format metrics at `/metrics` and health checks at `/health` and `/ready`. Prometheus scrapes these endpoints on a 15-second interval and stores the time-series data. Grafana queries Prometheus to render dashboards.

The **verifier service** does not currently expose a `/metrics` endpoint; it only provides a `/health` endpoint returning circuit registration status.

## Prerequisites

- Docker and Docker Compose (recommended) or standalone binaries
- The operator service running and accessible on port 9090
- Network access from Prometheus to the operator, and from Grafana to Prometheus

## Quick Start with Docker Compose

Create a `docker-compose.monitoring.yml` in the repository root:

```yaml
version: "3.8"

services:
  prometheus:
    image: prom/prometheus:v2.51.0
    ports:
      - "9091:9090"
    volumes:
      - ./deploy/prometheus.yml:/etc/prometheus/prometheus.yml:ro
      - prometheus_data:/prometheus
    command:
      - "--config.file=/etc/prometheus/prometheus.yml"
      - "--storage.tsdb.retention.time=30d"
    restart: unless-stopped

  grafana:
    image: grafana/grafana:10.4.0
    ports:
      - "3000:3000"
    environment:
      - GF_SECURITY_ADMIN_USER=admin
      - GF_SECURITY_ADMIN_PASSWORD=worldzk
    volumes:
      - grafana_data:/var/lib/grafana
    depends_on:
      - prometheus
    restart: unless-stopped

volumes:
  prometheus_data:
  grafana_data:
```

Start the monitoring stack:

```bash
docker compose -f docker-compose.monitoring.yml up -d
```

## Manual Setup

### 1. Install Prometheus

Download from https://prometheus.io/download/ or use your package manager.

```bash
# macOS
brew install prometheus

# Linux (amd64)
wget https://github.com/prometheus/prometheus/releases/download/v2.51.0/prometheus-2.51.0.linux-amd64.tar.gz
tar xzf prometheus-2.51.0.linux-amd64.tar.gz
```

Start Prometheus with the provided configuration:

```bash
prometheus --config.file=deploy/prometheus.yml --web.listen-address=:9091
```

Note: The default Prometheus port is 9090, but the operator also uses 9090 for its metrics server. Use `--web.listen-address=:9091` to avoid conflicts, or adjust `deploy/prometheus.yml` if the operator runs on a different port.

### 2. Install Grafana

Download from https://grafana.com/grafana/download/ or use your package manager.

```bash
# macOS
brew install grafana

# Linux (Debian/Ubuntu)
sudo apt-get install -y grafana
```

Start Grafana:

```bash
# macOS
brew services start grafana

# Linux
sudo systemctl start grafana-server
```

Grafana is available at http://localhost:3000 (default credentials: admin / admin).

### 3. Configure the Prometheus Data Source in Grafana

1. Open Grafana at http://localhost:3000
2. Go to **Connections** > **Data Sources** > **Add data source**
3. Select **Prometheus**
4. Set the URL to `http://localhost:9091` (or `http://prometheus:9090` if using Docker Compose)
5. Click **Save & Test** to verify the connection

### 4. Import the Dashboard

1. In Grafana, go to **Dashboards** > **New** > **Import**
2. Click **Upload JSON file** and select `deploy/grafana-dashboard.json`
3. Select the Prometheus data source you configured in step 3
4. Click **Import**

The dashboard UID is `worldzk-operator-v1`. If you re-import, Grafana will offer to overwrite the existing dashboard.

## Prometheus Scrape Configuration

The scrape configuration lives in `deploy/prometheus.yml`:

```yaml
scrape_configs:
  - job_name: "worldzk-operator"
    metrics_path: "/metrics"
    static_configs:
      - targets: ["localhost:9090"]

  - job_name: "worldzk-operator-health"
    metrics_path: "/health"
    scrape_interval: 30s
    static_configs:
      - targets: ["localhost:9090"]
```

Update the `targets` if the operator runs on a different host or port. For Kubernetes deployments, see `deploy/k8s/overlays/monitoring/prometheus-config.yaml`.

## Exposed Metrics

The operator exposes metrics in Prometheus text exposition format at `GET /metrics`.

### Counters (monotonically increasing)

| Metric | Description |
|---|---|
| `operator_challenges_detected` | Total on-chain challenge events detected |
| `operator_proofs_submitted` | Total proofs submitted to resolve disputes |
| `operator_disputes_resolved` | Total disputes resolved successfully |
| `operator_disputes_failed` | Total disputes that failed (proof rejected or timeout) |
| `operator_stylus_disputes_resolved` | Disputes resolved via Stylus WASM path (<30M gas) |
| `operator_solidity_disputes_resolved` | Disputes resolved via Solidity path (>200M gas) |
| `operator_errors_total` | Total errors encountered across all operations |
| `operator_finalizations_total` | Total result finalizations processed |
| `webhook_failures_total` | Total webhook notification delivery failures |

### Gauges (current value)

| Metric | Description |
|---|---|
| `operator_active_disputes` | Current number of active disputes being tracked |
| `operator_uptime_seconds` | Operator process uptime in seconds |
| `operator_last_block_polled` | Last blockchain block number scanned |

### Health Endpoints

| Endpoint | Description |
|---|---|
| `GET /health` | Returns `{"status":"ok"}` (200) or `{"status":"shutting_down"}` (503) |
| `GET /ready` | Returns 200 only if `last_block_polled > 0` and not shutting down |
| `GET /metrics` | Prometheus text format |
| `GET /metrics/json` | JSON format (backward compatibility) |

## Dashboard Panels

The imported dashboard includes these panels organized into rows:

### Service Health (Row 1)
- **Operator Status** -- UP/DOWN indicator from Prometheus `up` metric
- **Operator Uptime** -- Process uptime duration
- **Active Disputes** -- Current dispute queue size with color thresholds
- **Last Block Polled** -- Most recent blockchain block scanned
- **Total Errors** -- Cumulative error count
- **Webhook Failures** -- Cumulative webhook delivery failure count

### Proof Throughput (Row 2)
- **Proofs Submitted per Minute** -- 5-minute rate of proof submissions
- **Challenges Detected per Minute** -- 5-minute rate of challenge events

### Dispute Resolution (Row 3)
- **Disputes: Resolved vs Failed** -- Stacked area chart of outcomes
- **Verification Path Breakdown** -- Stylus vs Solidity resolution counts

### Error Rates and Reliability (Row 4)
- **Dispute Failure Rate** -- Percentage of disputes that fail (threshold lines at 2% and 5%)
- **Error Rate per Minute** -- Errors and webhook failures per minute

### Active Disputes and Queue Depth (Row 5)
- **Active Disputes (Queue Depth)** -- Current dispute count with alert thresholds
- **Blockchain Polling Progress** -- Last block polled and block progress rate

### Gas Costs and Verification Path (Row 6)
- **Verification Path Distribution** -- Gauge showing Stylus/Solidity/Hybrid split
- **Gas Cost Indicator by Path (Rate)** -- Per-minute rates by verification path

### Operational Summary (Row 7)
- **Cumulative Counters** -- All counters in one time-series chart
- **Current Metric Values** -- Snapshot table of all current values

## Key Metrics to Watch

These are the most important metrics for day-to-day operations:

1. **`operator_active_disputes`** -- The dispute queue. If this grows without bound, the operator is falling behind.

2. **`rate(operator_disputes_failed[5m]) / rate(operator_disputes_resolved[5m])`** -- Failure ratio. Should be near zero. A spike indicates proof generation issues, chain congestion, or contract bugs.

3. **`deriv(operator_last_block_polled[5m])`** -- Block polling rate. If this drops to zero, the operator has stopped syncing (RPC failure, circuit breaker tripped, etc.).

4. **`up{job="worldzk-operator"}`** -- Basic availability. If 0, the operator process is down.

5. **`operator_stylus_disputes_resolved` vs `operator_solidity_disputes_resolved`** -- Indicates which on-chain verifier is being used. Stylus disputes cost ~3-6M gas; Solidity disputes cost ~200-250M gas.

## Alerting Rules

Add these rules to a Prometheus alerting rules file (e.g., `deploy/alert-rules.yml`) and reference it from `prometheus.yml`:

```yaml
# In prometheus.yml, add:
# rule_files:
#   - "alert-rules.yml"

groups:
  - name: worldzk-operator
    rules:
      # Operator is down
      - alert: OperatorDown
        expr: up{job="worldzk-operator"} == 0
        for: 2m
        labels:
          severity: critical
        annotations:
          summary: "World ZK Compute operator is down"
          description: "The operator service has not responded to scrapes for 2 minutes."

      # Dispute queue is growing
      - alert: DisputeQueueHigh
        expr: operator_active_disputes{job="worldzk-operator"} > 100
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "Dispute queue depth exceeds 100"
          description: "{{ $value }} active disputes. The operator may be falling behind on proof generation."

      # Dispute queue critical
      - alert: DisputeQueueCritical
        expr: operator_active_disputes{job="worldzk-operator"} > 500
        for: 2m
        labels:
          severity: critical
        annotations:
          summary: "Dispute queue depth exceeds 500"
          description: "{{ $value }} active disputes. Immediate investigation required."

      # High dispute failure rate
      - alert: DisputeFailureRateHigh
        expr: >
          rate(operator_disputes_failed{job="worldzk-operator"}[10m])
          / (rate(operator_disputes_resolved{job="worldzk-operator"}[10m])
             + rate(operator_disputes_failed{job="worldzk-operator"}[10m])) > 0.05
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "Dispute failure rate exceeds 5%"
          description: "{{ $value | humanizePercentage }} of disputes are failing over the last 10 minutes."

      # High error rate
      - alert: ErrorRateHigh
        expr: rate(operator_errors_total{job="worldzk-operator"}[5m]) * 60 > 10
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "Operator error rate exceeds 10/min"
          description: "{{ $value }} errors per minute over the last 5 minutes."

      # Block polling stalled
      - alert: BlockPollingStalledWarning
        expr: deriv(operator_last_block_polled{job="worldzk-operator"}[10m]) == 0
        for: 10m
        labels:
          severity: warning
        annotations:
          summary: "Operator has stopped polling new blocks"
          description: "No new blocks polled in the last 10 minutes. Check RPC connectivity and circuit breaker state."

      # Block polling stalled - critical
      - alert: BlockPollingStalledCritical
        expr: deriv(operator_last_block_polled{job="worldzk-operator"}[10m]) == 0
        for: 30m
        labels:
          severity: critical
        annotations:
          summary: "Operator block polling stalled for 30 minutes"
          description: "The operator has not advanced its block cursor in 30 minutes. Disputes may be missed."

      # Webhook delivery failures
      - alert: WebhookFailuresHigh
        expr: rate(webhook_failures_total{job="worldzk-operator"}[10m]) * 60 > 5
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "Webhook notification failure rate exceeds 5/min"
          description: "Webhook notifications are failing at {{ $value }}/min. Check notification endpoint availability."

      # No disputes resolved in a long time (while challenges exist)
      - alert: NoDisputeResolution
        expr: >
          increase(operator_challenges_detected{job="worldzk-operator"}[1h]) > 0
          and increase(operator_disputes_resolved{job="worldzk-operator"}[1h]) == 0
        for: 30m
        labels:
          severity: critical
        annotations:
          summary: "Challenges detected but no disputes resolved in 1 hour"
          description: "The operator is detecting challenges but not resolving any disputes. Proof generation may be broken."
```

### Setting Up Alertmanager

To receive alert notifications, configure Prometheus Alertmanager:

1. Install Alertmanager from https://prometheus.io/download/
2. Add Alertmanager to `prometheus.yml`:
   ```yaml
   alerting:
     alertmanagers:
       - static_configs:
           - targets: ["localhost:9093"]
   ```
3. Configure Alertmanager receivers (email, Slack, PagerDuty, etc.) in `alertmanager.yml`

### Grafana Alerts (Alternative)

Grafana can also evaluate alert rules directly. In the dashboard:

1. Click a panel title > **Edit**
2. Go to the **Alert** tab
3. Create alert conditions matching the Prometheus rules above
4. Configure notification channels under **Alerting** > **Contact points**

## Troubleshooting

### Prometheus shows no data for operator metrics

1. Verify the operator is running: `curl http://localhost:9090/health`
2. Verify metrics endpoint: `curl http://localhost:9090/metrics`
3. Check Prometheus targets page: http://localhost:9091/targets
4. Ensure `deploy/prometheus.yml` has the correct target address

### Grafana dashboard shows "No Data"

1. Verify the Prometheus data source is configured correctly in Grafana
2. Test the data source connection: **Connections** > **Data Sources** > **Prometheus** > **Save & Test**
3. Check that the job label matches: queries use `job="worldzk-operator"`
4. Verify time range -- the dashboard defaults to "Last 6 hours"

### Operator reports `shutting_down` on health endpoint

The operator sets this status during graceful shutdown. If unexpected:
1. Check the operator process logs for shutdown signals
2. Verify no external orchestrator (Kubernetes, systemd) is stopping the process
3. The `/ready` endpoint will also return 503 during shutdown

### Block polling appears stalled

1. Check operator logs for RPC errors
2. The circuit breaker may have tripped -- look for circuit breaker log messages
3. Verify the RPC endpoint is reachable from the operator host
4. Check if `operator_errors_total` is increasing (indicates repeated failures)
