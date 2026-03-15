# Signature-Verified Detection

A RISC Zero guest program that verifies an ECDSA signature on input data before running anomaly detection. This ensures the data provenance is cryptographically authenticated inside the proof.

## What It Does

1. **Verifies an Ethereum ECDSA signature** over the input data using RISC Zero's accelerated secp256k1 precompile (approximately 100x faster than software implementation).
2. **Derives the signer's Ethereum address** from the public key and checks it matches the expected signer.
3. **Runs anomaly detection** only if the signature is valid and the signer matches.
4. **Commits a verified output** that includes signature validity, signer address, detection results, and data hash.

The proof guarantees both data authenticity (correct signature from expected source) and correct computation (detection ran faithfully).

## Use Cases

- Verify that an orb operator signed biometric data before processing.
- Chain-of-custody verification for sensitive data pipelines.
- Ensure only data from trusted sources is analyzed.

## Project Structure

```
signature-verified/
  methods/
    guest/
      Cargo.toml       # Guest dependencies (risc0-zkvm, k256, sha2, sha3)
      src/main.rs       # Signature verification + anomaly detection
```

## Prerequisites

- Rust toolchain (stable)
- RISC Zero toolchain:
  ```bash
  curl -L https://risczero.com/install | bash
  rzup install
  ```

## Build

```bash
cd examples/signature-verified
cargo build --release
```

## Precompiles Used

| Precompile | Purpose | Speedup |
|-----------|---------|---------|
| SHA-256 | Hashing data before signature verification | ~100x |
| secp256k1 | Ethereum ECDSA signature verification | ~100x |

These precompiles use the same Rust API (`sha2::Sha256`, `k256::ecdsa`) but execute via accelerated circuits inside the zkVM.

## Input / Output

**Input** (`SignedDetectionInput`):
- `data_points` -- feature vectors to analyze
- `threshold` -- detection sensitivity
- `signature` -- 64-byte ECDSA signature (r || s)
- `signer_pubkey` -- 33-byte compressed secp256k1 public key
- `expected_signer` -- 20-byte Ethereum address to verify against

**Output** (`VerifiedOutput`):
- `signature_valid` -- whether the ECDSA signature verified
- `signer_address` -- Ethereum address derived from the public key
- `signer_matches` -- whether the derived address matches the expected signer
- `data_hash` -- SHA-256 hash of the signed data
- `anomalies_found` / `flagged_ids` / `risk_score` -- detection results

## See Also

- [RISC Zero precompiles](https://dev.risczero.com/api/zkvm/precompiles)
- [World ZK Compute project root](../../)
