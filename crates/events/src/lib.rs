//! Strongly-typed contract event decoding for TEEMLVerifier.
//!
//! Provides typed event structs, ABI-compatible topic hash constants, and a
//! [`parse_log`] function that decodes raw EVM log topics and data into a
//! [`TEEEvent`] enum.
//!
//! All event structs use plain byte arrays (`[u8; 32]`, `[u8; 20]`) and derive
//! `serde::Serialize` / `serde::Deserialize` for easy JSON round-tripping.

use alloy_primitives::keccak256;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Typed event structs
// ---------------------------------------------------------------------------

/// A new ML inference result was submitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultSubmitted {
    /// Unique result identifier (indexed topic 1).
    pub result_id: [u8; 32],
    /// Address of the submitter (indexed topic 3, left-padded to 32 bytes).
    pub submitter: [u8; 20],
    /// Hash of the ML model used (indexed topic 2).
    pub model_hash: [u8; 32],
    /// Hash of the input data (non-indexed, first 32 bytes of log data).
    pub input_hash: [u8; 32],
    /// Raw result bytes (non-indexed, remaining log data after input_hash).
    /// Empty if the log data contains only the input_hash.
    pub result: Vec<u8>,
    /// Stake amount in wei (from log data, zero if not present).
    pub stake: u64,
}

/// A submitted result was challenged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultChallenged {
    /// The challenged result identifier (indexed topic 1).
    pub result_id: [u8; 32],
    /// Address of the challenger (non-indexed, left-padded in log data).
    pub challenger: [u8; 20],
    /// Challenge bond in wei (from log data, zero if not present).
    pub bond: u64,
}

/// An unchallenged result was finalized after the challenge window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultFinalized {
    /// The finalized result identifier (indexed topic 1).
    pub result_id: [u8; 32],
    /// Whether the result was valid (from log data, defaults to true).
    pub is_valid: bool,
}

/// A dispute was resolved on-chain (by ZK proof or timeout).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisputeResolved {
    /// The disputed result identifier (indexed topic 1).
    pub result_id: [u8; 32],
    /// True if the original prover's result was validated.
    pub prover_won: bool,
}

/// A new TEE enclave was registered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnclaveRegistered {
    /// The enclave's ECDSA signing key address (indexed topic 1).
    pub enclave: [u8; 20],
    /// Hash of the enclave image, e.g. AWS Nitro PCR0 (non-indexed, in data).
    pub image_hash: [u8; 32],
}

/// A result expired without being challenged (challenge window elapsed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultExpired {
    /// The expired result identifier (indexed topic 1).
    pub result_id: [u8; 32],
}

/// A TEE enclave was revoked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnclaveRevoked {
    /// The revoked enclave's signing key address (indexed topic 1).
    pub enclave: [u8; 20],
}

// ---------------------------------------------------------------------------
// TEEEvent enum
// ---------------------------------------------------------------------------

/// Parsed TEEMLVerifier contract event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TEEEvent {
    ResultSubmitted(ResultSubmitted),
    ResultChallenged(ResultChallenged),
    ResultFinalized(ResultFinalized),
    ResultExpired(ResultExpired),
    DisputeResolved(DisputeResolved),
    EnclaveRegistered(EnclaveRegistered),
    EnclaveRevoked(EnclaveRevoked),
}

// ---------------------------------------------------------------------------
// ABI-compatible topic hash constants (compile-time keccak256)
// ---------------------------------------------------------------------------

/// Topic hash for `ResultSubmitted(bytes32,bytes32,bytes32,address)`.
pub const TOPIC_RESULT_SUBMITTED: [u8; 32] =
    const_keccak(b"ResultSubmitted(bytes32,bytes32,bytes32,address)");

/// Topic hash for `ResultChallenged(bytes32,address)`.
pub const TOPIC_RESULT_CHALLENGED: [u8; 32] = const_keccak(b"ResultChallenged(bytes32,address)");

/// Topic hash for `ResultFinalized(bytes32)`.
pub const TOPIC_RESULT_FINALIZED: [u8; 32] = const_keccak(b"ResultFinalized(bytes32)");

/// Topic hash for `DisputeResolved(bytes32,bool)`.
pub const TOPIC_DISPUTE_RESOLVED: [u8; 32] = const_keccak(b"DisputeResolved(bytes32,bool)");

/// Topic hash for `ResultExpired(bytes32)`.
pub const TOPIC_RESULT_EXPIRED: [u8; 32] = const_keccak(b"ResultExpired(bytes32)");

/// Topic hash for `EnclaveRegistered(address,bytes32)`.
pub const TOPIC_ENCLAVE_REGISTERED: [u8; 32] = const_keccak(b"EnclaveRegistered(address,bytes32)");

/// Topic hash for `EnclaveRevoked(address)`.
pub const TOPIC_ENCLAVE_REVOKED: [u8; 32] = const_keccak(b"EnclaveRevoked(address)");

// ---------------------------------------------------------------------------
// Log parsing
// ---------------------------------------------------------------------------

/// Parse raw EVM log topics and data into a [`TEEEvent`].
///
/// Returns `None` if:
/// - `topics` is empty
/// - `topics[0]` does not match any known event signature
/// - Required indexed topics or data bytes are missing
///
/// # Arguments
/// * `topics` - Slice of 32-byte topic hashes. `topics[0]` is the event selector.
/// * `data` - The non-indexed log data bytes.
pub fn parse_log(topics: &[[u8; 32]], data: &[u8]) -> Option<TEEEvent> {
    if topics.is_empty() {
        return None;
    }

    let selector = &topics[0];

    if selector == &TOPIC_RESULT_SUBMITTED {
        // topics: [selector, resultId, modelHash, submitter(address left-padded)]
        // data:   [inputHash (32 bytes), optional extra data, optional stake]
        if topics.len() < 4 || data.len() < 32 {
            return None;
        }
        let result_id = topics[1];
        let model_hash = topics[2];
        let submitter = addr_from_topic(&topics[3]);
        let mut input_hash = [0u8; 32];
        input_hash.copy_from_slice(&data[0..32]);

        let result = if data.len() > 64 {
            data[64..].to_vec()
        } else {
            Vec::new()
        };
        let stake = if data.len() >= 64 {
            read_u64_from_word(&data[32..64])
        } else {
            0
        };

        Some(TEEEvent::ResultSubmitted(ResultSubmitted {
            result_id,
            submitter,
            model_hash,
            input_hash,
            result,
            stake,
        }))
    } else if selector == &TOPIC_RESULT_CHALLENGED {
        // topics: [selector, resultId]
        // data:   [challenger (address, left-padded to 32 bytes), optional bond]
        if topics.len() < 2 || data.len() < 32 {
            return None;
        }
        let result_id = topics[1];
        let challenger = addr_from_data(&data[0..32]);
        let bond = if data.len() >= 64 {
            read_u64_from_word(&data[32..64])
        } else {
            0
        };

        Some(TEEEvent::ResultChallenged(ResultChallenged {
            result_id,
            challenger,
            bond,
        }))
    } else if selector == &TOPIC_RESULT_FINALIZED {
        // topics: [selector, resultId]
        // data:   [optional is_valid bool]
        if topics.len() < 2 {
            return None;
        }
        let result_id = topics[1];
        let is_valid = if data.len() >= 32 {
            data[31] != 0
        } else {
            true
        };

        Some(TEEEvent::ResultFinalized(ResultFinalized {
            result_id,
            is_valid,
        }))
    } else if selector == &TOPIC_RESULT_EXPIRED {
        // topics: [selector, resultId]
        if topics.len() < 2 {
            return None;
        }
        let result_id = topics[1];
        Some(TEEEvent::ResultExpired(ResultExpired { result_id }))
    } else if selector == &TOPIC_DISPUTE_RESOLVED {
        // topics: [selector, resultId]
        // data:   [proverWon (bool, ABI-encoded as uint256)]
        if topics.len() < 2 || data.len() < 32 {
            return None;
        }
        let result_id = topics[1];
        let prover_won = data[31] != 0;

        Some(TEEEvent::DisputeResolved(DisputeResolved {
            result_id,
            prover_won,
        }))
    } else if selector == &TOPIC_ENCLAVE_REGISTERED {
        // topics: [selector, enclaveKey (address, left-padded)]
        // data:   [enclaveImageHash (32 bytes)]
        if topics.len() < 2 || data.len() < 32 {
            return None;
        }
        let enclave = addr_from_topic(&topics[1]);
        let mut image_hash = [0u8; 32];
        image_hash.copy_from_slice(&data[0..32]);

        Some(TEEEvent::EnclaveRegistered(EnclaveRegistered {
            enclave,
            image_hash,
        }))
    } else if selector == &TOPIC_ENCLAVE_REVOKED {
        // topics: [selector, enclaveKey (address, left-padded)]
        if topics.len() < 2 {
            return None;
        }
        let enclave = addr_from_topic(&topics[1]);

        Some(TEEEvent::EnclaveRevoked(EnclaveRevoked { enclave }))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute the keccak256 topic hash for an event signature string.
///
/// # Example
/// ```
/// use tee_events::{event_signature, TOPIC_RESULT_FINALIZED};
/// assert_eq!(event_signature("ResultFinalized(bytes32)"), TOPIC_RESULT_FINALIZED);
/// ```
pub fn event_signature(name: &str) -> [u8; 32] {
    let hash = keccak256(name.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(hash.as_slice());
    out
}

/// Extract a 20-byte address from a 32-byte left-padded topic value.
fn addr_from_topic(topic: &[u8; 32]) -> [u8; 20] {
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&topic[12..32]);
    addr
}

/// Extract a 20-byte address from a 32-byte left-padded data slice.
fn addr_from_data(word: &[u8]) -> [u8; 20] {
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&word[12..32]);
    addr
}

/// Read a u64 from the last 8 bytes of a 32-byte ABI word (big-endian).
fn read_u64_from_word(word: &[u8]) -> u64 {
    if word.len() < 32 {
        return 0;
    }
    u64::from_be_bytes([
        word[24], word[25], word[26], word[27], word[28], word[29], word[30], word[31],
    ])
}

// ---------------------------------------------------------------------------
// Compile-time keccak256 (Keccak-f[1600])
// ---------------------------------------------------------------------------

const fn const_keccak(input: &[u8]) -> [u8; 32] {
    const RATE: usize = 136;
    let mut state = [0u64; 25];
    let mut buf = [0u8; RATE];
    let mut buf_len: usize = 0;

    let mut i = 0;
    while i < input.len() {
        buf[buf_len] = input[i];
        buf_len += 1;
        if buf_len == RATE {
            state = xor_and_permute(state, &buf);
            buf_len = 0;
        }
        i += 1;
    }

    buf[buf_len] = 0x01;
    buf_len += 1;
    while buf_len < RATE {
        buf[buf_len] = 0x00;
        buf_len += 1;
    }
    buf[RATE - 1] |= 0x80;
    state = xor_and_permute(state, &buf);

    let mut out = [0u8; 32];
    let mut j = 0;
    while j < 32 {
        let lane = j / 8;
        let byte_in_lane = j % 8;
        out[j] = (state[lane] >> (8 * byte_in_lane)) as u8;
        j += 1;
    }
    out
}

const fn xor_and_permute(mut state: [u64; 25], buf: &[u8; 136]) -> [u64; 25] {
    let lanes = 136 / 8;
    let mut i = 0;
    while i < lanes {
        let off = i * 8;
        let lane = (buf[off] as u64)
            | (buf[off + 1] as u64) << 8
            | (buf[off + 2] as u64) << 16
            | (buf[off + 3] as u64) << 24
            | (buf[off + 4] as u64) << 32
            | (buf[off + 5] as u64) << 40
            | (buf[off + 6] as u64) << 48
            | (buf[off + 7] as u64) << 56;
        state[i] ^= lane;
        i += 1;
    }
    keccak_f1600(state)
}

const fn keccak_f1600(mut state: [u64; 25]) -> [u64; 25] {
    const RC: [u64; 24] = [
        0x0000000000000001,
        0x0000000000008082,
        0x800000000000808A,
        0x8000000080008000,
        0x000000000000808B,
        0x0000000080000001,
        0x8000000080008081,
        0x8000000000008009,
        0x000000000000008A,
        0x0000000000000088,
        0x0000000080008009,
        0x000000008000000A,
        0x000000008000808B,
        0x800000000000008B,
        0x8000000000008089,
        0x8000000000008003,
        0x8000000000008002,
        0x8000000000000080,
        0x000000000000800A,
        0x800000008000000A,
        0x8000000080008081,
        0x8000000000008080,
        0x0000000080000001,
        0x8000000080008008,
    ];
    const ROTATIONS: [u32; 25] = [
        0, 1, 62, 28, 27, 36, 44, 6, 55, 20, 3, 10, 43, 25, 39, 41, 45, 15, 21, 8, 18, 2, 61, 56,
        14,
    ];
    const PI: [usize; 25] = [
        0, 10, 20, 5, 15, 16, 1, 11, 21, 6, 7, 17, 2, 12, 22, 23, 8, 18, 3, 13, 14, 24, 9, 19, 4,
    ];

    let mut round = 0;
    while round < 24 {
        let mut c = [0u64; 5];
        let mut x = 0;
        while x < 5 {
            c[x] = state[x] ^ state[x + 5] ^ state[x + 10] ^ state[x + 15] ^ state[x + 20];
            x += 1;
        }
        x = 0;
        while x < 5 {
            let d = c[(x + 4) % 5] ^ c[(x + 1) % 5].rotate_left(1);
            let mut y = 0;
            while y < 5 {
                state[x + 5 * y] ^= d;
                y += 1;
            }
            x += 1;
        }

        let mut tmp = [0u64; 25];
        let mut i = 0;
        while i < 25 {
            tmp[PI[i]] = state[i].rotate_left(ROTATIONS[i]);
            i += 1;
        }

        x = 0;
        while x < 5 {
            let mut y = 0;
            while y < 5 {
                state[x + 5 * y] =
                    tmp[x + 5 * y] ^ (!tmp[(x + 1) % 5 + 5 * y] & tmp[(x + 2) % 5 + 5 * y]);
                y += 1;
            }
            x += 1;
        }

        state[0] ^= RC[round];
        round += 1;
    }
    state
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a left-padded address topic from a 20-byte address.
    fn make_addr_topic(addr: &[u8; 20]) -> [u8; 32] {
        let mut topic = [0u8; 32];
        topic[12..32].copy_from_slice(addr);
        topic
    }

    /// Build ABI-encoded data for a left-padded address.
    fn make_addr_data(addr: &[u8; 20]) -> Vec<u8> {
        let mut buf = vec![0u8; 32];
        buf[12..32].copy_from_slice(addr);
        buf
    }

    /// Build ABI-encoded data for a bool value.
    fn make_bool_data(val: bool) -> Vec<u8> {
        let mut buf = vec![0u8; 32];
        if val {
            buf[31] = 1;
        }
        buf
    }

    // -----------------------------------------------------------------------
    // 1. Parse ResultSubmitted
    // -----------------------------------------------------------------------
    #[test]
    fn test_parse_result_submitted() {
        let result_id = [0x11u8; 32];
        let model_hash = [0x22u8; 32];
        let submitter = [0xAAu8; 20];
        let input_hash = [0x33u8; 32];

        let topics = vec![
            TOPIC_RESULT_SUBMITTED,
            result_id,
            model_hash,
            make_addr_topic(&submitter),
        ];
        let data = input_hash.to_vec();

        let event = parse_log(&topics, &data).expect("should parse");
        match event {
            TEEEvent::ResultSubmitted(e) => {
                assert_eq!(e.result_id, result_id);
                assert_eq!(e.model_hash, model_hash);
                assert_eq!(e.input_hash, input_hash);
                assert_eq!(e.submitter, submitter);
                assert!(e.result.is_empty());
                assert_eq!(e.stake, 0);
            }
            other => panic!("expected ResultSubmitted, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // 2. Parse ResultChallenged
    // -----------------------------------------------------------------------
    #[test]
    fn test_parse_result_challenged() {
        let result_id = [0x44u8; 32];
        let challenger = [0xBBu8; 20];

        let topics = vec![TOPIC_RESULT_CHALLENGED, result_id];
        let data = make_addr_data(&challenger);

        let event = parse_log(&topics, &data).expect("should parse");
        match event {
            TEEEvent::ResultChallenged(e) => {
                assert_eq!(e.result_id, result_id);
                assert_eq!(e.challenger, challenger);
                assert_eq!(e.bond, 0);
            }
            other => panic!("expected ResultChallenged, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // 3. Parse ResultFinalized
    // -----------------------------------------------------------------------
    #[test]
    fn test_parse_result_finalized() {
        let result_id = [0x55u8; 32];
        let topics = vec![TOPIC_RESULT_FINALIZED, result_id];

        let event = parse_log(&topics, &[]).expect("should parse");
        match event {
            TEEEvent::ResultFinalized(e) => {
                assert_eq!(e.result_id, result_id);
                assert!(e.is_valid, "default should be true");
            }
            other => panic!("expected ResultFinalized, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // 4. Parse DisputeResolved (prover won)
    // -----------------------------------------------------------------------
    #[test]
    fn test_parse_dispute_resolved_prover_won() {
        let result_id = [0x66u8; 32];
        let topics = vec![TOPIC_DISPUTE_RESOLVED, result_id];
        let data = make_bool_data(true);

        let event = parse_log(&topics, &data).expect("should parse");
        match event {
            TEEEvent::DisputeResolved(e) => {
                assert_eq!(e.result_id, result_id);
                assert!(e.prover_won);
            }
            other => panic!("expected DisputeResolved, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // 5. Parse DisputeResolved (challenger won)
    // -----------------------------------------------------------------------
    #[test]
    fn test_parse_dispute_resolved_challenger_won() {
        let result_id = [0x77u8; 32];
        let topics = vec![TOPIC_DISPUTE_RESOLVED, result_id];
        let data = make_bool_data(false);

        let event = parse_log(&topics, &data).expect("should parse");
        match event {
            TEEEvent::DisputeResolved(e) => {
                assert_eq!(e.result_id, result_id);
                assert!(!e.prover_won);
            }
            other => panic!("expected DisputeResolved, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // 6. Parse EnclaveRegistered
    // -----------------------------------------------------------------------
    #[test]
    fn test_parse_enclave_registered() {
        let enclave = [0xCCu8; 20];
        let image_hash = [0xDDu8; 32];

        let topics = vec![TOPIC_ENCLAVE_REGISTERED, make_addr_topic(&enclave)];
        let data = image_hash.to_vec();

        let event = parse_log(&topics, &data).expect("should parse");
        match event {
            TEEEvent::EnclaveRegistered(e) => {
                assert_eq!(e.enclave, enclave);
                assert_eq!(e.image_hash, image_hash);
            }
            other => panic!("expected EnclaveRegistered, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // 7. Parse EnclaveRevoked
    // -----------------------------------------------------------------------
    #[test]
    fn test_parse_enclave_revoked() {
        let enclave = [0xEEu8; 20];
        let topics = vec![TOPIC_ENCLAVE_REVOKED, make_addr_topic(&enclave)];

        let event = parse_log(&topics, &[]).expect("should parse");
        match event {
            TEEEvent::EnclaveRevoked(e) => {
                assert_eq!(e.enclave, enclave);
            }
            other => panic!("expected EnclaveRevoked, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // 8. Unknown topic returns None
    // -----------------------------------------------------------------------
    #[test]
    fn test_parse_unknown_topic_returns_none() {
        let unknown = event_signature("SomeUnknownEvent(uint256)");
        assert!(parse_log(&[unknown], &[]).is_none());
    }

    // -----------------------------------------------------------------------
    // 9. Empty topics returns None
    // -----------------------------------------------------------------------
    #[test]
    fn test_parse_empty_topics_returns_none() {
        assert!(parse_log(&[], &[]).is_none());
    }

    // -----------------------------------------------------------------------
    // 10. Insufficient data handling
    // -----------------------------------------------------------------------
    #[test]
    fn test_parse_insufficient_data_returns_none() {
        // ResultSubmitted needs 32 bytes of data (input_hash)
        let topics = vec![TOPIC_RESULT_SUBMITTED, [0x11; 32], [0x22; 32], [0u8; 32]];
        assert!(parse_log(&topics, &[0u8; 16]).is_none());

        // ResultChallenged needs 32 bytes of data (challenger address)
        let topics2 = vec![TOPIC_RESULT_CHALLENGED, [0x33; 32]];
        assert!(parse_log(&topics2, &[]).is_none());

        // DisputeResolved needs 32 bytes of data (proverWon bool)
        let topics3 = vec![TOPIC_DISPUTE_RESOLVED, [0x44; 32]];
        assert!(parse_log(&topics3, &[0u8; 5]).is_none());

        // EnclaveRegistered needs 32 bytes of data (image_hash)
        let topics4 = vec![TOPIC_ENCLAVE_REGISTERED, [0u8; 32]];
        assert!(parse_log(&topics4, &[0u8; 20]).is_none());
    }

    // -----------------------------------------------------------------------
    // 11. Missing indexed topics returns None
    // -----------------------------------------------------------------------
    #[test]
    fn test_parse_missing_topics_returns_none() {
        // ResultSubmitted needs 4 topics (selector + 3 indexed)
        let topics = vec![TOPIC_RESULT_SUBMITTED, [0x11; 32]];
        assert!(parse_log(&topics, &[0u8; 32]).is_none());

        // ResultFinalized needs 2 topics
        let topics2 = vec![TOPIC_RESULT_FINALIZED];
        assert!(parse_log(&topics2, &[]).is_none());

        // EnclaveRevoked needs 2 topics
        let topics3 = vec![TOPIC_ENCLAVE_REVOKED];
        assert!(parse_log(&topics3, &[]).is_none());
    }

    // -----------------------------------------------------------------------
    // 12. event_signature helper
    // -----------------------------------------------------------------------
    #[test]
    fn test_event_signature_helper() {
        assert_eq!(
            event_signature("ResultSubmitted(bytes32,bytes32,bytes32,address)"),
            TOPIC_RESULT_SUBMITTED,
        );
        assert_eq!(
            event_signature("ResultChallenged(bytes32,address)"),
            TOPIC_RESULT_CHALLENGED,
        );
        assert_eq!(
            event_signature("ResultFinalized(bytes32)"),
            TOPIC_RESULT_FINALIZED,
        );
        assert_eq!(
            event_signature("DisputeResolved(bytes32,bool)"),
            TOPIC_DISPUTE_RESOLVED,
        );
        assert_eq!(
            event_signature("EnclaveRegistered(address,bytes32)"),
            TOPIC_ENCLAVE_REGISTERED,
        );
        assert_eq!(
            event_signature("EnclaveRevoked(address)"),
            TOPIC_ENCLAVE_REVOKED,
        );
    }

    // -----------------------------------------------------------------------
    // 13. Topic hash constants are unique
    // -----------------------------------------------------------------------
    #[test]
    fn test_topic_hashes_are_unique() {
        let all = [
            TOPIC_RESULT_SUBMITTED,
            TOPIC_RESULT_CHALLENGED,
            TOPIC_RESULT_FINALIZED,
            TOPIC_DISPUTE_RESOLVED,
            TOPIC_ENCLAVE_REGISTERED,
            TOPIC_ENCLAVE_REVOKED,
        ];
        for i in 0..all.len() {
            for j in (i + 1)..all.len() {
                assert_ne!(all[i], all[j], "topics {} and {} collide", i, j);
            }
        }
    }

    // -----------------------------------------------------------------------
    // 14. Serde round-trip
    // -----------------------------------------------------------------------
    #[test]
    fn test_serde_roundtrip() {
        let event = TEEEvent::ResultSubmitted(ResultSubmitted {
            result_id: [0x11; 32],
            submitter: [0xAA; 20],
            model_hash: [0x22; 32],
            input_hash: [0x33; 32],
            result: vec![1, 2, 3],
            stake: 1000,
        });
        let json = serde_json::to_string(&event).expect("serialize");
        let parsed: TEEEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, parsed);
    }

    // -----------------------------------------------------------------------
    // 15. Extra trailing data is tolerated
    // -----------------------------------------------------------------------
    #[test]
    fn test_extra_data_tolerated() {
        let result_id = [0x01u8; 32];
        let model_hash = [0x02u8; 32];
        let submitter = [0x03u8; 20];
        let input_hash = [0x04u8; 32];

        let topics = vec![
            TOPIC_RESULT_SUBMITTED,
            result_id,
            model_hash,
            make_addr_topic(&submitter),
        ];
        let mut data = input_hash.to_vec();
        data.extend_from_slice(&[0xFF; 64]);

        let event = parse_log(&topics, &data).expect("should parse with extra data");
        match event {
            TEEEvent::ResultSubmitted(e) => {
                assert_eq!(e.input_hash, input_hash);
            }
            other => panic!("expected ResultSubmitted, got {:?}", other),
        }
    }
}
