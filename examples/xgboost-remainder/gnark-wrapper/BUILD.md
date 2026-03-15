# Building gnark-wrapper

The `gnark-wrapper` binary wraps Remainder GKR proofs in a Groth16 SNARK for cheaper on-chain verification.

## Prerequisites

- Go 1.23+
- ~2GB RAM for circuit compilation

## Build

```bash
cd examples/xgboost-remainder/gnark-wrapper

# Build for current platform
go build -o gnark-wrapper .

# Cross-compile for Linux (CI/Docker)
GOOS=linux GOARCH=amd64 go build -o gnark-wrapper-linux-amd64 .

# Cross-compile for macOS ARM
GOOS=darwin GOARCH=arm64 go build -o gnark-wrapper-darwin-arm64 .
```

Or use the Makefile:

```bash
make build          # Current platform
make build-linux    # Linux amd64
make clean          # Remove binaries
make test           # Run tests
```

## Commands

```bash
# Simple circuit Groth16 proof
cat witness.json | ./gnark-wrapper prove-json

# DAG circuit Groth16 proof (XGBoost)
cat witness.json | ./gnark-wrapper prove-dag-json --config-json

# Show circuit info
./gnark-wrapper dag-info
```

## Dependencies

- `gnark` v0.11.0 — Groth16 prover/verifier
- `gnark-crypto` v0.14.0 — BN254 field arithmetic
- `icicle` v1.1.0 — GPU acceleration (optional, indirect)

## Generated Artifacts

After running `prove-dag-json`, the wrapper generates:
- `DAGRemainderGroth16Verifier.sol` — Solidity verifier with embedded verification key
- Groth16 proof bytes (printed to stdout)

## Notes

- The checked-in `gnark-wrapper` binary is arm64 macOS. CI and Docker need a Linux build.
- Proving keys (`proving_key.bin`, `verification_key.bin`) are generated on first run and cached.
- DAG circuit compilation takes ~30s; subsequent proofs use cached keys.
