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
cat witness.json | ./gnark-wrapper prove-dag-json

# Show DAG circuit info (reads the witness JSON from stdin)
./gnark-wrapper dag-info < witness.json
```

`--config-json` only affects `prove` and `prove-json`; `prove-dag-json` and
`dag-info` always take the circuit shape from the witness JSON.

## Dependencies

- `gnark` v0.11.0 — Groth16 prover/verifier
- `gnark-crypto` v0.14.0 — BN254 field arithmetic
- `icicle` v1.1.0 — GPU acceleration (optional, indirect)

## Generated Artifacts

After running `prove-dag-json`, the wrapper generates:
- `DAGRemainderGroth16Verifier.sol` — Solidity verifier with embedded verification key
- A JSON fixture (proof points and public inputs), printed to stdout

## Notes

- No binary is checked in; build one with the commands above.
- Every key comes from a one-party `groth16.Setup` run on the local machine.
- `prove-dag-json`, `prove-ec-json` and `prove-ec-chunked-json` run a fresh
  setup on every call, so each run has new keys and writes a new verifier
  `.sol` into the current directory, overwriting the previous one. Nothing is
  cached between runs.
- `prove` and `prove-json` load `proving_key.bin` and `verification_key.bin`
  written by `setup`. `prove-json` runs a fresh setup instead (and writes
  `RemainderGroth16Verifier.sol`) when `proving_key.bin` is missing or
  `--config-json` is given.
- DAG circuit compilation takes ~30s.
