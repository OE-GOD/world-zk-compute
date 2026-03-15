# deployments/

This directory contains **contract deployment records** -- JSON files that track deployed contract addresses, transaction hashes, and chain-specific configuration. It does **not** contain Kubernetes or Helm deployment manifests.

## Files

| File | Purpose |
|------|---------|
| `registry.json` | Canonical registry of all deployments across chains (localhost, Sepolia, Arbitrum Sepolia). Tracks contract addresses, versions, deploy tx hashes, block numbers, and cross-contract wiring. |
| `chains.json` | Chain configuration reference: RPC URLs, chain IDs, explorer URLs, gas limits, code size limits, and chain-specific notes. |
| `arbitrum-sepolia.json` | Arbitrum Sepolia deployment state including RemainderVerifier, TEEMLVerifier addresses, and DAG circuit registration status. |

## Usage

These files are read by SDK integration tests (e.g., `sdk/typescript/src/__tests__/sepolia-e2e.test.ts`) to resolve contract addresses automatically. They are also useful as a quick reference for which contracts are deployed where.

After deploying contracts, update the relevant JSON file with the new addresses and transaction details.

## Infrastructure Deployment

For actual infrastructure deployment configuration (Helm charts, Kubernetes manifests, Docker), see:

- `deploy/helm/worldzk/` -- Helm chart for the operator, indexer, and enclave services
- `deploy/k8s/` -- Raw Kubernetes manifests
- `deploy/docker/` -- Dockerfiles and compose configuration
