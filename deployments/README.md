# deployments/

This directory contains **contract deployment records** -- JSON files that track deployed contract addresses, transaction hashes, and chain-specific configuration. It does **not** contain Kubernetes or Helm deployment manifests.

## Files

| File | Purpose |
|------|---------|
| `registry.json` | Canonical registry of all deployments across chains (localhost, World Chain Sepolia; all address fields are currently empty). Tracks contract addresses, versions, deploy tx hashes, block numbers, and cross-contract wiring. |
| `chains.json` | Chain configuration reference: RPC URLs, chain IDs, explorer URLs, gas limits, code size limits, and chain-specific notes. |
| `arbitrum-sepolia.json` | Arbitrum Sepolia deployment state: RemainderVerifier, TEEMLVerifier addresses, and DAG circuit registration status (empty template). |
| `world-chain-sepolia.json` | World Chain Sepolia deployment state (empty template). |

## Usage

`chains.json` is read by the deploy scripts (`scripts/deploy.sh`, `scripts/deploy-multichain.sh`). `scripts/deploy.sh` writes its result to `deployments/<chain-id>.json`, which SDK integration tests (e.g., `sdk/typescript/src/__tests__/sepolia-e2e.test.ts`) and `scripts/verify-deployment.sh` read to resolve contract addresses automatically.

After deploying contracts, update the relevant JSON file with the new addresses and transaction details.

## Infrastructure Deployment

For actual infrastructure deployment configuration (Helm charts, Kubernetes manifests, Docker), see:

- `deploy/helm/worldzk/` -- Helm chart for the operator, indexer, and enclave services
- `deploy/k8s/` -- Raw Kubernetes manifests
- `deploy/docker/` -- Docker build targets (`Makefile.docker`)
