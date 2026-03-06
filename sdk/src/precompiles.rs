/// SHA-256 precompile (~100x faster)
pub const SHA2_GIT: &str = "https://github.com/risc0/RustCrypto-hashes";
pub const SHA2_TAG: &str = "sha2-v0.10.8-risczero.0";

/// secp256k1 ECDSA precompile - Ethereum signatures (~100x faster)
pub const K256_GIT: &str = "https://github.com/risc0/RustCrypto-elliptic-curves";
pub const K256_TAG: &str = "k256-v0.13.4-risczero.1";

/// P-256 ECDSA precompile (~100x faster)
pub const P256_GIT: &str = "https://github.com/risc0/RustCrypto-elliptic-curves";
pub const P256_TAG: &str = "p256-v0.13.2-risczero.0";

/// RSA precompile (~100x faster)
pub const RSA_GIT: &str = "https://github.com/risc0/rust-rsa";
pub const RSA_TAG: &str = "rsa-v0.9.6-risczero.0";

/// Ed25519/curve25519 precompile (~100x faster)
pub const CURVE25519_GIT: &str = "https://github.com/risc0/curve25519-dalek";
pub const CURVE25519_TAG: &str = "curve25519-dalek-v4.1.3-risczero.0";

/// BLS12-381 precompile
pub const BLS12_381_GIT: &str = "https://github.com/risc0/bls12_381";
pub const BLS12_381_TAG: &str = "bls12_381-v0.8.0-risczero.0";

/// Generate Cargo.toml dependency line for a precompile
pub fn cargo_dep(name: &str, git: &str, tag: &str, features: &[&str]) -> String {
    if features.is_empty() {
        format!("{} = {{ git = \"{}\", tag = \"{}\" }}", name, git, tag)
    } else {
        format!(
            "{} = {{ git = \"{}\", tag = \"{}\", features = {:?} }}",
            name, git, tag, features
        )
    }
}

/// Print all precompile dependencies for copy-paste into Cargo.toml
pub fn print_all_deps() {
    println!("# RISC Zero Accelerated Crypto Precompiles");
    println!("# Add these to your guest's Cargo.toml for 10-100x speedup");
    println!();
    println!("# SHA-256 (~100x faster)");
    println!("{}", cargo_dep("sha2", SHA2_GIT, SHA2_TAG, &[]));
    println!();
    println!("# Ethereum ECDSA - secp256k1 (~100x faster)");
    println!(
        "{}",
        cargo_dep("k256", K256_GIT, K256_TAG, &["ecdsa", "unstable"])
    );
    println!();
    println!("# Standard ECDSA - P-256 (~100x faster)");
    println!("{}", cargo_dep("p256", P256_GIT, P256_TAG, &["ecdsa"]));
    println!();
    println!("# RSA (~100x faster)");
    println!("{}", cargo_dep("rsa", RSA_GIT, RSA_TAG, &["unstable"]));
    println!();
    println!("# Ed25519 (~100x faster)");
    println!(
        "{}",
        cargo_dep("curve25519-dalek", CURVE25519_GIT, CURVE25519_TAG, &[])
    );
    println!();
    println!("# BLS12-381");
    println!(
        "{}",
        cargo_dep("bls12_381", BLS12_381_GIT, BLS12_381_TAG, &[])
    );
}
