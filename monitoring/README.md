# Monitoring Stack

Alerting configuration for the world-zk-compute platform. This directory
contains Prometheus alerting rules, Alertmanager routing configuration, and
Grafana dashboards.

## Required Environment Variables

The `alertmanager.yml` file uses `${ENV_VAR}` references that must be
substituted before deployment (e.g., with `envsubst`). The following
variables are required:

| Variable | Description | Where to obtain |
|----------|-------------|-----------------|
| `PAGERDUTY_SERVICE_KEY` | PagerDuty Events API v1 Integration Key | PagerDuty > Services > your-service > Integrations > Events API v1 |
| `SLACK_WEBHOOK_URL_WARNINGS` | Slack Incoming Webhook URL for the `#world-zk-alerts` channel | Slack > Apps > Incoming Webhooks > Add New Webhook |
| `SLACK_WEBHOOK_URL_INFO` | Slack Incoming Webhook URL for the `#world-zk-info` channel | Slack > Apps > Incoming Webhooks > Add New Webhook |

Optional (uncomment the SMTP section in `alertmanager.yml` to use):

| Variable | Description |
|----------|-------------|
| `SMTP_PASSWORD` | Password for the SMTP relay configured in the global section |

## Deploying Alertmanager

### 1. Set environment variables

Export the required variables in your shell or CI secrets store:

```bash
export PAGERDUTY_SERVICE_KEY="your-pagerduty-integration-key"
export SLACK_WEBHOOK_URL_WARNINGS="<your-slack-warnings-webhook-url>"
export SLACK_WEBHOOK_URL_INFO="<your-slack-info-webhook-url>"
```

### 2. Substitute variables and write the config

Use `envsubst` to replace `${VAR}` references with actual values:

```bash
envsubst < monitoring/alertmanager.yml > /etc/alertmanager/alertmanager.yml
```

If you are using Docker Compose, you can mount the substituted file:

```bash
envsubst < monitoring/alertmanager.yml > ./alertmanager-rendered.yml
# then in docker-compose.yml:
#   volumes:
#     - ./alertmanager-rendered.yml:/etc/alertmanager/alertmanager.yml:ro
```

### 3. Validate the rendered config

Run the deploy-time validation to confirm all variables were substituted:

```bash
DEPLOY_CHECK=1 ./monitoring/validate-config.sh /etc/alertmanager/alertmanager.yml
```

This checks that no `${VAR}` references remain in the rendered file.

### 4. Start Alertmanager

```bash
alertmanager --config.file=/etc/alertmanager/alertmanager.yml
```

## Configuring Slack Webhooks

1. Go to your Slack workspace settings and navigate to **Apps > Incoming Webhooks**
   (or visit `https://<workspace>.slack.com/apps/A0F7XDUAZ-incoming-webhooks`).
2. Click **Add New Webhook to Workspace**.
3. Select the target channel (`#world-zk-alerts` for warnings, `#world-zk-info`
   for informational alerts).
4. Copy the generated Webhook URL. It will look like
   `https://hooks.slack.com/services/YOUR_TEAM/YOUR_BOT/YOUR_TOKEN`.
5. Set the URL as the corresponding environment variable
   (`SLACK_WEBHOOK_URL_WARNINGS` or `SLACK_WEBHOOK_URL_INFO`).

**Security note:** Webhook URLs are secrets. Never commit them to the repository.
Store them in your CI/CD secrets manager (e.g., GitHub Actions secrets, Vault,
AWS Secrets Manager).

## Configuring PagerDuty

1. In PagerDuty, navigate to **Services > Service Directory** and select (or
   create) the service for world-zk-compute alerts.
2. Go to the **Integrations** tab and click **Add Integration**.
3. Select **Events API v1** and click **Add**.
4. Copy the **Integration Key** (a 32-character hex string).
5. Set it as the `PAGERDUTY_SERVICE_KEY` environment variable.

The Alertmanager config routes `severity: critical` alerts to PagerDuty,
including `OperatorDown`, `EnclaveDown`, and `DisputeResolutionFailed`.

## Testing Alerting Locally

### Using amtool

`amtool` is the Alertmanager CLI. Install it alongside Alertmanager or via
`go install github.com/prometheus/alertmanager/cmd/amtool@latest`.

**Check config syntax:**

```bash
# Validate the raw template (will show ${VAR} as literal strings but confirms YAML structure)
amtool check-config monitoring/alertmanager.yml

# Validate the rendered config (after envsubst)
amtool check-config /etc/alertmanager/alertmanager.yml
```

**Send a test alert to a running Alertmanager instance:**

```bash
# Fire a test critical alert
amtool alert add \
  --alertmanager.url=http://localhost:9093 \
  alertname=TestOperatorDown \
  severity=critical \
  service=operator \
  --annotation.summary="Test: operator is down" \
  --annotation.description="This is a test alert from amtool"

# Fire a test warning alert
amtool alert add \
  --alertmanager.url=http://localhost:9093 \
  alertname=TestHighLatency \
  severity=warning \
  service=operator \
  --annotation.summary="Test: high proving latency" \
  --annotation.description="This is a test warning from amtool"
```

**List active alerts:**

```bash
amtool alert query --alertmanager.url=http://localhost:9093
```

**Silence an alert (useful during maintenance):**

```bash
amtool silence add \
  --alertmanager.url=http://localhost:9093 \
  --duration=2h \
  --comment="Maintenance window" \
  alertname=TestOperatorDown
```

### Full local stack

To test the full alerting pipeline locally:

```bash
# 1. Render the config with test values
export PAGERDUTY_SERVICE_KEY="test-key-not-real"
export SLACK_WEBHOOK_URL_WARNINGS="https://httpbin.org/post"
export SLACK_WEBHOOK_URL_INFO="https://httpbin.org/post"
envsubst < monitoring/alertmanager.yml > /tmp/alertmanager-test.yml

# 2. Start Alertmanager
alertmanager --config.file=/tmp/alertmanager-test.yml --web.listen-address=:9093 &

# 3. Send a test alert
amtool alert add \
  --alertmanager.url=http://localhost:9093 \
  alertname=TestAlert severity=warning service=test \
  --annotation.summary="Local test alert"

# 4. Check the Alertmanager UI at http://localhost:9093
# 5. Verify routing with:
amtool config routes test --config.file=/tmp/alertmanager-test.yml \
  severity=critical service=operator
# Expected output: pagerduty-critical

amtool config routes test --config.file=/tmp/alertmanager-test.yml \
  severity=warning service=operator
# Expected output: slack-warnings

amtool config routes test --config.file=/tmp/alertmanager-test.yml \
  severity=info service=indexer
# Expected output: slack-info
```

## CI Validation

The `.github/workflows/monitoring-lint.yml` workflow runs on every push or PR
that touches `monitoring/` files. It performs:

1. **YAML syntax validation** -- ensures all `.yml` files parse correctly.
2. **Grafana JSON validation** -- ensures all dashboard JSON files are valid.
3. **Placeholder check** -- fails the build if any raw `<REPLACE_*>` strings
   are found in YAML config files. The committed config should use `${ENV_VAR}`
   references only.
4. **validate-config.sh** -- runs the validation script as an additional check.

If the CI check fails with a placeholder error, replace the offending
`<REPLACE_...>` string with the corresponding `${ENV_VAR}` reference as
documented in the table above.

## File Overview

| File | Purpose |
|------|---------|
| `alertmanager.yml` | Alertmanager routing, receivers, and inhibition rules |
| `alerting-rules.yml` | Prometheus alerting rules (operator, enclave, prover) |
| `alerting-rules-sepolia.yml` | Prometheus alerting rules for Sepolia testnet |
| `prometheus.yml` | Prometheus scrape configuration |
| `grafana-operator.json` | Grafana dashboard for operator service metrics |
| `grafana-enclave.json` | Grafana dashboard for enclave service metrics |
| `grafana-indexer.json` | Grafana dashboard for indexer service metrics |
| `grafana-sepolia.json` | Grafana dashboard for Sepolia contract metrics |
| `validate-config.sh` | Pre-deploy validation script for alertmanager config |
