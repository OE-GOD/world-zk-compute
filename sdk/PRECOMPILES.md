# Precompiles Guide: 10-100x Faster Proving

Precompiles are hardware-accelerated circuits built directly into the RISC Zero zkVM. Using them can speed up cryptographic operations by **10-100x**.

## Quick Start

Replace standard crypto crates with RISC Zero's accelerated versions in your **guest** program:

```toml
# methods/guest/Cargo.toml

[dependencies]
# Instead of standard crates, use RISC Zero's accelerated versions:

# SHA-256 (accelerated)
sha2 = { git = "https://github.com/risc0/RustCrypto-hashes", tag = "sha2-v0.10.8-risczero.0" }

# Ethereum signatures - secp256k1 (accelerated)
k256 = { git = "https://github.com/risc0/RustCrypto-elliptic-curves", tag = "k256-v0.13.4-risczero.1", features = ["ecdsa", "unstable"] }

# RSA signatures (accelerated)
rsa = { git = "https://github.com/risc0/rust-rsa", tag = "rsa-v0.9.6-risczero.0", features = ["unstable"] }

# Ed25519 signatures (accelerated)
curve25519-dalek = { git = "https://github.com/risc0/curve25519-dalek", tag = "curve25519-dalek-v4.1.3-risczero.0" }

# BLS signatures (accelerated)
bls12_381 = { git = "https://github.com/risc0/bls12_381", tag = "bls12_381-v0.8.0-risczero.0" }
```

## Performance Comparison

| Operation | Standard Cycles | Precompile Cycles | Speedup |
|-----------|-----------------|-------------------|---------|
| SHA-256 (1KB) | ~500K | ~5K | **100x** |
| ECDSA Verify (secp256k1) | ~10M | ~100K | **100x** |
| RSA-2048 Verify | ~50M | ~500K | **100x** |
| Ed25519 Verify | ~5M | ~50K | **100x** |

## Available Precompiles

### 1. SHA-256 (Hashing)

```toml
# Cargo.toml
sha2 = { git = "https://github.com/risc0/RustCrypto-hashes", tag = "sha2-v0.10.8-risczero.0" }
```

```rust
// guest/src/main.rs
use sha2::{Sha256, Digest};

fn hash_data(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    result.into()
}
```

### 2. ECDSA secp256k1 (Ethereum Signatures)

```toml
# Cargo.toml
k256 = { git = "https://github.com/risc0/RustCrypto-elliptic-curves", tag = "k256-v0.13.4-risczero.1", features = ["ecdsa", "unstable"] }
```

```rust
// guest/src/main.rs
use k256::ecdsa::{Signature, VerifyingKey, signature::Verifier};

fn verify_eth_signature(
    message: &[u8],
    signature: &Signature,
    pubkey: &VerifyingKey,
) -> bool {
    pubkey.verify(message, signature).is_ok()
}
```

### 3. RSA Signatures

```toml
# Cargo.toml
rsa = { git = "https://github.com/risc0/rust-rsa", tag = "rsa-v0.9.6-risczero.0", features = ["unstable"] }
```

```rust
// guest/src/main.rs
use rsa::{RsaPublicKey, pkcs1v15::VerifyingKey, signature::Verifier};
use sha2::Sha256;

fn verify_rsa_signature(
    message: &[u8],
    signature: &[u8],
    pubkey: &RsaPublicKey,
) -> bool {
    let verifying_key = VerifyingKey::<Sha256>::new(pubkey.clone());
    let sig = rsa::pkcs1v15::Signature::try_from(signature).unwrap();
    verifying_key.verify(message, &sig).is_ok()
}
```

### 4. Ed25519 Signatures

```toml
# Cargo.toml
ed25519-dalek = { git = "https://github.com/risc0/curve25519-dalek", tag = "curve25519-dalek-v4.1.3-risczero.0" }
```

```rust
// guest/src/main.rs
use ed25519_dalek::{Signature, VerifyingKey, Verifier};

fn verify_ed25519(
    message: &[u8],
    signature: &Signature,
    pubkey: &VerifyingKey,
) -> bool {
    pubkey.verify(message, signature).is_ok()
}
```

### 5. BLS12-381 (BLS Signatures)

```toml
# Cargo.toml
bls12_381 = { git = "https://github.com/risc0/bls12_381", tag = "bls12_381-v0.8.0-risczero.0" }
```

### 6. P-256 ECDSA (Standard ECDSA)

```toml
# Cargo.toml
p256 = { git = "https://github.com/risc0/RustCrypto-elliptic-curves", tag = "p256-v0.13.2-risczero.0", features = ["ecdsa"] }
```

## Complete Example: Signature-Verified Detection

Here's a complete example showing how to verify signatures before processing detection data:

```rust
// methods/guest/src/main.rs
#![no_main]
#![no_std]

extern crate alloc;
use alloc::vec::Vec;

use risc0_zkvm::guest::env;
use k256::ecdsa::{Signature, VerifyingKey, signature::Verifier};
use sha2::{Sha256, Digest};

risc0_zkvm::guest::entry!(main);

#[derive(serde::Deserialize)]
struct SignedInput {
    /// The actual detection data
    data: Vec<DataPoint>,
    /// Signature over the data
    signature: [u8; 64],
    /// Public key of the data source
    pubkey: [u8; 33],
}

#[derive(serde::Deserialize)]
struct DataPoint {
    id: [u8; 32],
    value: u64,
}

#[derive(serde::Serialize)]
struct Output {
    data_hash: [u8; 32],
    signature_valid: bool,
    anomalies_found: usize,
}

fn main() {
    // Read signed input
    let input: SignedInput = env::read();

    // Hash the data (ACCELERATED - 100x faster)
    let data_bytes = bincode::serialize(&input.data).unwrap();
    let data_hash: [u8; 32] = {
        let mut hasher = Sha256::new();
        hasher.update(&data_bytes);
        hasher.finalize().into()
    };

    // Verify signature (ACCELERATED - 100x faster)
    let signature = Signature::from_slice(&input.signature).unwrap();
    let pubkey = VerifyingKey::from_sec1_bytes(&input.pubkey).unwrap();
    let signature_valid = pubkey.verify(&data_hash, &signature).is_ok();

    // Only process if signature is valid
    let anomalies_found = if signature_valid {
        detect_anomalies(&input.data)
    } else {
        0
    };

    // Commit output
    env::commit(&Output {
        data_hash,
        signature_valid,
        anomalies_found,
    });
}

fn detect_anomalies(data: &[DataPoint]) -> usize {
    // Your detection logic here
    data.iter().filter(|p| p.value > 1000).count()
}
```

## Cargo.toml for the Example

```toml
[package]
name = "signature-verified-detector"
version = "0.1.0"
edition = "2021"

[dependencies]
risc0-zkvm = { version = "1.2", default-features = false, features = ["std"] }
serde = { version = "1.0", default-features = false, features = ["derive", "alloc"] }
bincode = { version = "1.3", default-features = false }

# ACCELERATED CRYPTO - Use RISC Zero's precompile versions!
sha2 = { git = "https://github.com/risc0/RustCrypto-hashes", tag = "sha2-v0.10.8-risczero.0" }
k256 = { git = "https://github.com/risc0/RustCrypto-elliptic-curves", tag = "k256-v0.13.4-risczero.1", features = ["ecdsa", "unstable"] }
```

## Monitoring Precompile Usage

Set the `RISC0_INFO` environment variable to see which precompiles are being used:

```bash
RISC0_INFO=1 cargo run --release
```

Output will show:
```
[risc0] Precompile usage:
  sha256: 15 calls, 75K cycles saved
  secp256k1_verify: 3 calls, 29.7M cycles saved
  Total cycles saved: 29.775M
```

## Common Patterns

### Pattern 1: Hash-then-Verify

```rust
// Hash data, then verify signature over hash
let hash = Sha256::digest(&data);
let valid = pubkey.verify(&hash, &signature).is_ok();
```

### Pattern 2: Merkle Proof Verification

```rust
// Verify merkle proof with accelerated hashing
fn verify_merkle_proof(leaf: &[u8], proof: &[[u8; 32]], root: &[u8; 32]) -> bool {
    let mut current = Sha256::digest(leaf);
    for sibling in proof {
        let mut hasher = Sha256::new();
        if current.as_slice() < sibling {
            hasher.update(&current);
            hasher.update(sibling);
        } else {
            hasher.update(sibling);
            hasher.update(&current);
        }
        current = hasher.finalize();
    }
    current.as_slice() == root
}
```

### Pattern 3: Batch Signature Verification

```rust
// Verify multiple signatures efficiently
fn verify_batch(items: &[(Message, Signature, VerifyingKey)]) -> Vec<bool> {
    items.iter()
        .map(|(msg, sig, pk)| pk.verify(msg, sig).is_ok())
        .collect()
}
```

## What NOT to Use Precompiles For

1. **Secret Key Operations** - Precompiles don't guarantee constant-time execution
2. **Key Generation** - Generate keys outside the zkVM
3. **Random Number Generation** - Use deterministic inputs instead

## Troubleshooting

### "feature unstable required"

Add the `unstable` feature:
```toml
k256 = { ..., features = ["ecdsa", "unstable"] }
```

### "no_std compatibility"

Ensure you have:
```toml
[dependencies]
serde = { ..., default-features = false, features = ["derive", "alloc"] }
```

### Version Mismatch

Ensure risc0-zkvm version matches precompile tags:
- risc0-zkvm 1.2.x → Use `*-risczero.0` or `*-risczero.1` tags

## Reference: All Precompile Crates

| Crate | Tag | Notes |
|-------|-----|-------|
| sha2 | sha2-v0.10.8-risczero.0 | SHA-256/512 |
| k256 | k256-v0.13.4-risczero.1 | secp256k1 (Ethereum) |
| p256 | p256-v0.13.2-risczero.0 | P-256/secp256r1 |
| rsa | rsa-v0.9.6-risczero.0 | RSA-2048/4096 |
| curve25519-dalek | curve25519-dalek-v4.1.3-risczero.0 | Ed25519/X25519 |
| bls12_381 | bls12_381-v0.8.0-risczero.0 | BLS signatures |
| blst | blst-v0.3.14-risczero.0 | Alternative BLS |

## Summary

1. **Replace standard crates** with RISC Zero's git versions
2. **Use the same API** - no code changes needed
3. **Get 10-100x speedup** on cryptographic operations
4. **Monitor with RISC0_INFO=1** to verify acceleration
