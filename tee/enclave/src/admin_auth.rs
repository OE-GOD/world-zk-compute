//! Admin API key comparison.
//!
//! The key is kept as its SHA-256 digest only, so handlers cannot compare the
//! key itself: [`AdminKey::matches`] is the one way to check a candidate.

use sha2::{Digest, Sha256};

/// The configured admin API key, held as its SHA-256 digest.
pub struct AdminKey([u8; 32]);

impl AdminKey {
    pub fn new(key: &str) -> Self {
        Self(Sha256::digest(key.as_bytes()).into())
    }

    /// Whether `candidate` is the admin key. The digests are compared without
    /// an early exit, so the time taken does not depend on where they differ.
    pub fn matches(&self, candidate: &str) -> bool {
        digests_equal(&self.0, &Sha256::digest(candidate.as_bytes()).into())
    }
}

fn digests_equal(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let difference = a
        .iter()
        .zip(b)
        .fold(0u8, |acc, (x, y)| std::hint::black_box(acc | (x ^ y)));
    difference == 0
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_only_the_exact_key_matches() {
        let key = AdminKey::new("correct-key");
        assert!(key.matches("correct-key"));

        for wrong in [
            "",
            "correct-ke",
            "correct-key2",
            "correct-kez",
            "dorrect-key",
            "Correct-key",
            " correct-key",
            "correct-key\0",
        ] {
            assert!(!key.matches(wrong), "{wrong:?} must not match");
        }
    }

    #[test]
    fn test_key_is_held_as_its_digest() {
        // SHA-256("correct-key")
        assert_eq!(
            hex::encode(AdminKey::new("correct-key").0),
            "ddb0fd2dede48502669718e09ef1447dba46f3d3822e9fbf05af11d874a0f23b"
        );
    }

    #[test]
    fn test_every_byte_of_the_digest_counts() {
        let digest = [0xa5u8; 32];
        assert!(digests_equal(&digest, &digest));
        for i in 0..32 {
            for bit in 0..8 {
                let mut other = digest;
                other[i] ^= 1 << bit;
                assert!(!digests_equal(&digest, &other), "byte {i}, bit {bit}");
            }
        }
        // Differences in two bytes do not cancel out.
        let mut other = digest;
        other[0] ^= 0xff;
        other[31] ^= 0xff;
        assert!(!digests_equal(&digest, &other));
    }
}
