/// Proof decoding from ABI-encoded bytes.
/// Ported from RemainderVerifier.sol decode functions.
///
/// The proof is a flat byte array where every field occupies 32 bytes (big-endian U256).
/// Variable-length sections are length-prefixed: a U256 count followed by that many items.

use alloc::vec::Vec;

use crate::ec::{G1Point, PODPProof, PedersenGens, ProofOfProduct};
use crate::field::U256;
use crate::gkr::{CommittedLayerProof, DAGInputLayerProof, GKRProof, PublicValueClaim};
use crate::sumcheck::CommittedSumcheckProof;

// ============================================================
// ProofDecoder
// ============================================================

/// Streaming decoder that walks through a flat byte slice, reading 32-byte
/// big-endian fields sequentially.
pub struct ProofDecoder<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> ProofDecoder<'a> {
    /// Create a new decoder starting at offset 0.
    pub fn new(data: &'a [u8]) -> Self {
        ProofDecoder { data, offset: 0 }
    }

    /// Current byte offset into the data slice.
    #[inline]
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Set the byte offset explicitly (used for two-pass decoding).
    #[inline]
    pub fn set_offset(&mut self, offset: usize) {
        self.offset = offset;
    }

    // --------------------------------------------------------
    // Primitive readers
    // --------------------------------------------------------

    /// Read 32 big-endian bytes and return a `U256`. Advances offset by 32.
    /// Public variant for use by external decoders (e.g. circuit description).
    #[inline]
    pub fn read_u256_public(&mut self) -> U256 {
        self.read_u256()
    }

    /// Read 32 big-endian bytes and return a `U256`. Advances offset by 32.
    fn read_u256(&mut self) -> U256 {
        let start = self.offset;
        let end = start + 32;
        verify!(
            end <= self.data.len(),
            "read_u256: out of bounds"
        );
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&self.data[start..end]);
        self.offset = end;
        U256::from_be_bytes(&bytes)
    }

    /// Read a G1 affine point (two consecutive U256 values: x, y). Advances offset by 64.
    fn read_g1(&mut self) -> G1Point {
        let x = self.read_u256();
        let y = self.read_u256();
        G1Point { x, y }
    }

    /// Read a U256 and interpret it as a `usize` length/count field.
    /// Only the lowest 64 bits are used; panics if the value exceeds usize::MAX.
    fn read_usize(&mut self) -> usize {
        let v = self.read_u256();
        // For length fields the value is always small; only limb 0 is nonzero.
        verify!(
            v.0[1] == 0 && v.0[2] == 0 && v.0[3] == 0,
            "read_usize: value too large for usize"
        );
        v.0[0] as usize
    }

    // --------------------------------------------------------
    // Composite decoders
    // --------------------------------------------------------

    /// Decode a PODP proof.
    ///
    /// Layout: commitD(G1) | commitDDotA(G1) | numZ(u256) | z_vector[numZ](u256 each) | zDelta(u256) | zBeta(u256)
    fn decode_podp(&mut self) -> PODPProof {
        let commit_d = self.read_g1();
        let commit_d_dot_a = self.read_g1();

        let num_z = self.read_usize();
        let mut z_vector = Vec::with_capacity(num_z);
        for _ in 0..num_z {
            z_vector.push(self.read_u256());
        }

        let z_delta = self.read_u256();
        let z_beta = self.read_u256();

        PODPProof {
            commit_d,
            commit_d_dot_a,
            z_vector,
            z_delta,
            z_beta,
        }
    }

    /// Decode a ProofOfProduct.
    ///
    /// Layout: alpha(G1) | beta(G1) | delta(G1) | z1(u256) | z2(u256) | z3(u256) | z4(u256) | z5(u256)
    fn decode_pop(&mut self) -> ProofOfProduct {
        let alpha = self.read_g1();
        let beta = self.read_g1();
        let delta = self.read_g1();
        let z1 = self.read_u256();
        let z2 = self.read_u256();
        let z3 = self.read_u256();
        let z4 = self.read_u256();
        let z5 = self.read_u256();

        ProofOfProduct {
            alpha,
            beta,
            delta,
            z1,
            z2,
            z3,
            z4,
            z5,
        }
    }

    /// Decode a single committed layer proof.
    ///
    /// Layout:
    ///   sum(G1)
    ///   numMessages(u256) | messages[numMessages](G1 each)
    ///   PODP
    ///   numCommits(u256) | commitments[numCommits](G1 each)
    ///   numPops(u256) | pops[numPops](ProofOfProduct each)
    ///   hasAgg(u256) | if 1: numCoeffs(u256) skip coeffs*64, numOpenings(u256) skip openings*128, numEq(u256) skip eq*96
    fn decode_committed_layer_proof(&mut self) -> CommittedLayerProof {
        // Sumcheck proof: sum (G1)
        let sum = self.read_g1();

        // Sumcheck proof: messages (G1[])
        let num_messages = self.read_usize();
        let mut messages = Vec::with_capacity(num_messages);
        for _ in 0..num_messages {
            messages.push(self.read_g1());
        }

        // Sumcheck proof: PODP
        let podp = self.decode_podp();

        let sumcheck_proof = CommittedSumcheckProof {
            sum,
            messages,
            podp,
        };

        // Post-sumcheck commitments (G1[])
        let num_commits = self.read_usize();
        let mut commitments = Vec::with_capacity(num_commits);
        for _ in 0..num_commits {
            commitments.push(self.read_g1());
        }

        // ProofOfProduct entries
        let num_pops = self.read_usize();
        let mut pops = Vec::with_capacity(num_pops);
        for _ in 0..num_pops {
            pops.push(self.decode_pop());
        }

        // Claim aggregation (optional) -- skip if present
        let has_agg = self.read_usize();
        if has_agg == 1 {
            // Skip coefficients: numCoeffs * 64 bytes (each is a G1 point)
            let num_coeffs = self.read_usize();
            self.offset += num_coeffs * 64;

            // Skip openings: numOpenings * (64 + 64) bytes
            let num_openings = self.read_usize();
            self.offset += num_openings * 128;

            // Skip equality proofs: numEq * (64 + 32) bytes
            let num_eq = self.read_usize();
            self.offset += num_eq * 96;
        }

        CommittedLayerProof {
            sumcheck_proof,
            commitments,
            pops,
        }
    }

    /// Decode a DAG input layer proof.
    ///
    /// Layout:
    ///   numRows(u256) | commitment_rows[numRows](G1 each)
    ///   numEvals(u256) | per eval: PODP + comEval(G1)
    fn decode_dag_input_layer_proof(&mut self) -> DAGInputLayerProof {
        // Commitment rows
        let num_rows = self.read_usize();
        let mut commitment_rows = Vec::with_capacity(num_rows);
        for _ in 0..num_rows {
            commitment_rows.push(self.read_g1());
        }

        // Evaluation proofs (decode ALL)
        let num_evals = self.read_usize();
        let mut podps = Vec::with_capacity(num_evals);
        let mut com_evals = Vec::with_capacity(num_evals);
        for _ in 0..num_evals {
            podps.push(self.decode_podp());
            com_evals.push(self.read_g1());
        }

        DAGInputLayerProof {
            commitment_rows,
            podps,
            com_evals,
        }
    }

    /// Decode public value claims from the current offset.
    ///
    /// Layout per claim: value(32) | blinding(32) | commitment(G1, 64) | numPoint(u256) | point[numPoint](32 each)
    /// The point coordinates are skipped (resolved from circuit topology at verification time).
    fn decode_public_value_claims(&mut self) -> Vec<PublicValueClaim> {
        let num_claims = self.read_usize();
        let mut claims = Vec::with_capacity(num_claims);
        for _ in 0..num_claims {
            let value = self.read_u256();
            let blinding = self.read_u256();
            let commitment = self.read_g1();

            // Skip point coordinates (not needed; resolved from circuit topology)
            let num_point = self.read_usize();
            self.offset += num_point * 32;

            claims.push(PublicValueClaim {
                value,
                blinding,
                commitment,
            });
        }
        claims
    }

    /// Skip a single claim section (used for FS claims).
    ///
    /// Layout: value(32) | blinding(32) | commitment(64) | numPoint(u256) | point[numPoint](32 each)
    fn skip_claim(&mut self) {
        self.offset += 32; // value
        self.offset += 32; // blinding
        self.offset += 64; // commitment (G1)
        let num_point = self.read_usize();
        self.offset += num_point * 32;
    }

    // --------------------------------------------------------
    // Top-level decoders
    // --------------------------------------------------------

    /// Decode the common proof prefix shared by regular and DAG modes.
    ///
    /// Returns `(pub_claims_offset)` -- the byte offset where public value claims start.
    ///
    /// After this call, `self.offset` points to the DAG input proofs section.
    fn decode_proof_common(&mut self, gkr_proof: &mut GKRProof) -> usize {
        // 1. Skip circuit hash (32 bytes)
        self.offset += 32;

        // 2. Skip public inputs section (decoded separately via decode_embedded_public_inputs)
        let num_pub_input_sections = self.read_usize();
        for _ in 0..num_pub_input_sections {
            let cnt = self.read_usize();
            self.offset += cnt * 32;
        }

        // 3. Decode output claim commitments (G1 points)
        let num_output_proofs = self.read_usize();
        let mut output_claim_commitments = Vec::with_capacity(num_output_proofs);
        for _ in 0..num_output_proofs {
            output_claim_commitments.push(self.read_g1());
        }
        gkr_proof.output_claim_commitments = output_claim_commitments;

        // 4. Decode committed layer proofs
        let num_layer_proofs = self.read_usize();
        let mut layer_proofs = Vec::with_capacity(num_layer_proofs);
        for _ in 0..num_layer_proofs {
            layer_proofs.push(self.decode_committed_layer_proof());
        }
        gkr_proof.layer_proofs = layer_proofs;

        // 5. Skip Fiat-Shamir claims section
        let num_fs_claims = self.read_usize();
        for _ in 0..num_fs_claims {
            self.skip_claim();
        }

        // 6. Save offset for public value claims (DAG mode decodes these)
        let pub_claims_offset = self.offset;

        // 7. Skip public value claims section (regular mode ignores them;
        //    DAG mode reads them separately at pub_claims_offset)
        let num_pub_claims = self.read_usize();
        for _ in 0..num_pub_claims {
            self.skip_claim();
        }

        pub_claims_offset
    }

    /// Decode a full proof for DAG verification.
    ///
    /// Returns:
    ///   - `GKRProof`: output claim commitments + committed layer proofs
    ///   - `Vec<U256>`: embedded public input values (from the proof binary)
    ///   - `Vec<DAGInputLayerProof>`: per-input-layer eval proofs
    ///   - `Vec<PublicValueClaim>`: Pedersen openings for public input atoms
    pub fn decode_proof_for_dag(
        &mut self,
    ) -> (
        GKRProof,
        Vec<U256>,
        Vec<DAGInputLayerProof>,
        Vec<PublicValueClaim>,
    ) {
        // First extract embedded public inputs (needs its own pass from start)
        let embedded = Self::extract_embedded_public_inputs(self.data);

        // Now decode the common proof structure
        let mut gkr_proof = GKRProof {
            output_claim_commitments: Vec::new(),
            layer_proofs: Vec::new(),
        };
        let pub_claims_offset = self.decode_proof_common(&mut gkr_proof);

        // Decode public value claims at the saved offset
        // We need a sub-decoder positioned at pub_claims_offset
        let mut claims_decoder = ProofDecoder::new(self.data);
        claims_decoder.set_offset(pub_claims_offset);
        let public_value_claims = claims_decoder.decode_public_value_claims();

        // self.offset now points past the public value claims section;
        // decode DAG input proofs
        let num_input_proofs = self.read_usize();
        let mut dag_input_proofs = Vec::with_capacity(num_input_proofs);
        for _ in 0..num_input_proofs {
            dag_input_proofs.push(self.decode_dag_input_layer_proof());
        }

        (gkr_proof, embedded, dag_input_proofs, public_value_claims)
    }

    /// Extract the embedded public input values from the proof binary.
    ///
    /// These are stored right after the circuit hash as:
    ///   numSections(u256), per section: count(u256) + count*32 bytes of Fr values.
    ///
    /// Uses a two-pass approach: first count total elements, then extract.
    pub fn decode_embedded_public_inputs(data: &[u8]) -> Vec<U256> {
        Self::extract_embedded_public_inputs(data)
    }

    /// Internal helper for embedded public input extraction.
    fn extract_embedded_public_inputs(data: &[u8]) -> Vec<U256> {
        let mut dec = ProofDecoder::new(data);

        // Skip circuit hash (32 bytes)
        dec.offset += 32;

        // Read number of sections
        let num_sections = dec.read_usize();

        // First pass: count total elements
        let saved_offset = dec.offset;
        let mut total_elements: usize = 0;
        for _ in 0..num_sections {
            let cnt = dec.read_usize();
            dec.offset += cnt * 32;
            total_elements += cnt;
        }

        // Second pass: extract elements
        dec.offset = saved_offset;
        let mut result = Vec::with_capacity(total_elements);
        for _ in 0..num_sections {
            let cnt = dec.read_usize();
            for _ in 0..cnt {
                result.push(dec.read_u256());
            }
        }

        result
    }

    /// Decode Pedersen generators from a separate byte slice.
    ///
    /// Format: numGens(u256) | G1[numGens] message gens | G1 scalar_gen | G1 blinding_gen
    pub fn decode_pedersen_gens(data: &[u8]) -> PedersenGens {
        let mut dec = ProofDecoder::new(data);

        if data.is_empty() {
            return PedersenGens {
                message_gens: Vec::new(),
                scalar_gen: G1Point::INFINITY,
                blinding_gen: G1Point::INFINITY,
            };
        }

        let num_gens = dec.read_usize();
        let mut message_gens = Vec::with_capacity(num_gens);
        for _ in 0..num_gens {
            message_gens.push(dec.read_g1());
        }

        let scalar_gen = dec.read_g1();
        let blinding_gen = dec.read_g1();

        PedersenGens {
            message_gens,
            scalar_gen,
            blinding_gen,
        }
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: encode a U256 as 32 big-endian bytes and append to a buffer.
    fn push_u256(buf: &mut Vec<u8>, val: U256) {
        buf.extend_from_slice(&val.to_be_bytes());
    }

    /// Helper: encode a usize as a U256 and append.
    fn push_usize(buf: &mut Vec<u8>, val: usize) {
        push_u256(buf, U256::from_u64(val as u64));
    }

    /// Helper: encode a G1 point and append.
    fn push_g1(buf: &mut Vec<u8>, p: &G1Point) {
        push_u256(buf, p.x);
        push_u256(buf, p.y);
    }

    fn sample_point(seed: u64) -> G1Point {
        G1Point {
            x: U256::from_u64(seed * 100 + 1),
            y: U256::from_u64(seed * 100 + 2),
        }
    }

    #[test]
    fn test_read_u256() {
        let val = U256::from_u64(42);
        let bytes = val.to_be_bytes();
        let mut dec = ProofDecoder::new(&bytes);
        let result = dec.read_u256();
        assert_eq!(result, val);
        assert_eq!(dec.offset(), 32);
    }

    #[test]
    fn test_read_g1() {
        let p = sample_point(1);
        let mut buf = Vec::new();
        push_g1(&mut buf, &p);
        let mut dec = ProofDecoder::new(&buf);
        let result = dec.read_g1();
        assert_eq!(result, p);
        assert_eq!(dec.offset(), 64);
    }

    #[test]
    fn test_read_usize() {
        let mut buf = Vec::new();
        push_usize(&mut buf, 7);
        let mut dec = ProofDecoder::new(&buf);
        assert_eq!(dec.read_usize(), 7);
    }

    #[test]
    fn test_decode_podp() {
        let mut buf = Vec::new();
        let commit_d = sample_point(1);
        let commit_d_dot_a = sample_point(2);
        push_g1(&mut buf, &commit_d);
        push_g1(&mut buf, &commit_d_dot_a);
        push_usize(&mut buf, 3); // numZ
        let z0 = U256::from_u64(10);
        let z1 = U256::from_u64(20);
        let z2 = U256::from_u64(30);
        push_u256(&mut buf, z0);
        push_u256(&mut buf, z1);
        push_u256(&mut buf, z2);
        let z_delta = U256::from_u64(40);
        let z_beta = U256::from_u64(50);
        push_u256(&mut buf, z_delta);
        push_u256(&mut buf, z_beta);

        let mut dec = ProofDecoder::new(&buf);
        let podp = dec.decode_podp();
        assert_eq!(podp.commit_d, commit_d);
        assert_eq!(podp.commit_d_dot_a, commit_d_dot_a);
        assert_eq!(podp.z_vector.len(), 3);
        assert_eq!(podp.z_vector[0], z0);
        assert_eq!(podp.z_vector[1], z1);
        assert_eq!(podp.z_vector[2], z2);
        assert_eq!(podp.z_delta, z_delta);
        assert_eq!(podp.z_beta, z_beta);
    }

    #[test]
    fn test_decode_pop() {
        let mut buf = Vec::new();
        let alpha = sample_point(1);
        let beta = sample_point(2);
        let delta = sample_point(3);
        push_g1(&mut buf, &alpha);
        push_g1(&mut buf, &beta);
        push_g1(&mut buf, &delta);
        for i in 1..=5u64 {
            push_u256(&mut buf, U256::from_u64(i * 111));
        }

        let mut dec = ProofDecoder::new(&buf);
        let pop = dec.decode_pop();
        assert_eq!(pop.alpha, alpha);
        assert_eq!(pop.beta, beta);
        assert_eq!(pop.delta, delta);
        assert_eq!(pop.z1, U256::from_u64(111));
        assert_eq!(pop.z2, U256::from_u64(222));
        assert_eq!(pop.z3, U256::from_u64(333));
        assert_eq!(pop.z4, U256::from_u64(444));
        assert_eq!(pop.z5, U256::from_u64(555));
    }

    #[test]
    fn test_decode_pedersen_gens() {
        let mut buf = Vec::new();
        push_usize(&mut buf, 2); // 2 message gens
        let g0 = sample_point(10);
        let g1 = sample_point(11);
        push_g1(&mut buf, &g0);
        push_g1(&mut buf, &g1);
        let scalar_gen = sample_point(20);
        let blinding_gen = sample_point(30);
        push_g1(&mut buf, &scalar_gen);
        push_g1(&mut buf, &blinding_gen);

        let gens = ProofDecoder::decode_pedersen_gens(&buf);
        assert_eq!(gens.message_gens.len(), 2);
        assert_eq!(gens.message_gens[0], g0);
        assert_eq!(gens.message_gens[1], g1);
        assert_eq!(gens.scalar_gen, scalar_gen);
        assert_eq!(gens.blinding_gen, blinding_gen);
    }

    #[test]
    fn test_decode_pedersen_gens_empty() {
        let gens = ProofDecoder::decode_pedersen_gens(&[]);
        assert_eq!(gens.message_gens.len(), 0);
        assert!(gens.scalar_gen.is_infinity());
        assert!(gens.blinding_gen.is_infinity());
    }

    #[test]
    fn test_decode_embedded_public_inputs() {
        let mut buf = Vec::new();
        // Circuit hash (32 bytes)
        push_u256(&mut buf, U256::from_u64(0xABCD));

        // 2 sections
        push_usize(&mut buf, 2);

        // Section 0: 3 elements
        push_usize(&mut buf, 3);
        push_u256(&mut buf, U256::from_u64(100));
        push_u256(&mut buf, U256::from_u64(200));
        push_u256(&mut buf, U256::from_u64(300));

        // Section 1: 2 elements
        push_usize(&mut buf, 2);
        push_u256(&mut buf, U256::from_u64(400));
        push_u256(&mut buf, U256::from_u64(500));

        let result = ProofDecoder::decode_embedded_public_inputs(&buf);
        assert_eq!(result.len(), 5);
        assert_eq!(result[0], U256::from_u64(100));
        assert_eq!(result[1], U256::from_u64(200));
        assert_eq!(result[2], U256::from_u64(300));
        assert_eq!(result[3], U256::from_u64(400));
        assert_eq!(result[4], U256::from_u64(500));
    }

    #[test]
    fn test_decode_public_value_claims() {
        let mut buf = Vec::new();
        // 2 claims
        push_usize(&mut buf, 2);

        // Claim 0
        push_u256(&mut buf, U256::from_u64(11)); // value
        push_u256(&mut buf, U256::from_u64(22)); // blinding
        push_g1(&mut buf, &sample_point(1)); // commitment
        push_usize(&mut buf, 2); // numPoint
        push_u256(&mut buf, U256::from_u64(0)); // point[0] (skipped)
        push_u256(&mut buf, U256::from_u64(0)); // point[1] (skipped)

        // Claim 1
        push_u256(&mut buf, U256::from_u64(33)); // value
        push_u256(&mut buf, U256::from_u64(44)); // blinding
        push_g1(&mut buf, &sample_point(2)); // commitment
        push_usize(&mut buf, 0); // numPoint (none)

        let mut dec = ProofDecoder::new(&buf);
        let claims = dec.decode_public_value_claims();
        assert_eq!(claims.len(), 2);
        assert_eq!(claims[0].value, U256::from_u64(11));
        assert_eq!(claims[0].blinding, U256::from_u64(22));
        assert_eq!(claims[0].commitment, sample_point(1));
        assert_eq!(claims[1].value, U256::from_u64(33));
        assert_eq!(claims[1].blinding, U256::from_u64(44));
        assert_eq!(claims[1].commitment, sample_point(2));
    }

    #[test]
    fn test_decode_dag_input_layer_proof() {
        let mut buf = Vec::new();

        // 2 commitment rows
        push_usize(&mut buf, 2);
        push_g1(&mut buf, &sample_point(1));
        push_g1(&mut buf, &sample_point(2));

        // 1 eval proof
        push_usize(&mut buf, 1);
        // PODP for eval 0
        push_g1(&mut buf, &sample_point(10)); // commitD
        push_g1(&mut buf, &sample_point(11)); // commitDDotA
        push_usize(&mut buf, 2); // numZ
        push_u256(&mut buf, U256::from_u64(7));
        push_u256(&mut buf, U256::from_u64(8));
        push_u256(&mut buf, U256::from_u64(9)); // zDelta
        push_u256(&mut buf, U256::from_u64(10)); // zBeta
                                                 // comEval for eval 0
        push_g1(&mut buf, &sample_point(20));

        let mut dec = ProofDecoder::new(&buf);
        let proof = dec.decode_dag_input_layer_proof();
        assert_eq!(proof.commitment_rows.len(), 2);
        assert_eq!(proof.commitment_rows[0], sample_point(1));
        assert_eq!(proof.commitment_rows[1], sample_point(2));
        assert_eq!(proof.podps.len(), 1);
        assert_eq!(proof.podps[0].z_vector.len(), 2);
        assert_eq!(proof.com_evals.len(), 1);
        assert_eq!(proof.com_evals[0], sample_point(20));
    }

    /// Build a minimal but structurally complete proof blob for DAG decoding.
    fn build_minimal_dag_proof() -> Vec<u8> {
        let mut buf = Vec::new();

        // 1. Circuit hash
        push_u256(&mut buf, U256::from_u64(0xDEAD));

        // 2. Public input sections: 1 section with 2 elements
        push_usize(&mut buf, 1);
        push_usize(&mut buf, 2);
        push_u256(&mut buf, U256::from_u64(777));
        push_u256(&mut buf, U256::from_u64(888));

        // 3. Output claim commitments: 1
        push_usize(&mut buf, 1);
        push_g1(&mut buf, &sample_point(50));

        // 4. Layer proofs: 1 layer
        push_usize(&mut buf, 1);
        {
            // sum G1
            push_g1(&mut buf, &sample_point(60));
            // messages: 1
            push_usize(&mut buf, 1);
            push_g1(&mut buf, &sample_point(61));
            // PODP
            push_g1(&mut buf, &sample_point(70)); // commitD
            push_g1(&mut buf, &sample_point(71)); // commitDDotA
            push_usize(&mut buf, 1); // numZ = 1
            push_u256(&mut buf, U256::from_u64(99));
            push_u256(&mut buf, U256::from_u64(100)); // zDelta
            push_u256(&mut buf, U256::from_u64(101)); // zBeta
                                                      // commitments: 1
            push_usize(&mut buf, 1);
            push_g1(&mut buf, &sample_point(80));
            // pops: 0
            push_usize(&mut buf, 0);
            // hasAgg: 0
            push_usize(&mut buf, 0);
        }

        // 5. FS claims: 0
        push_usize(&mut buf, 0);

        // 6. Public value claims: 1
        push_usize(&mut buf, 1);
        push_u256(&mut buf, U256::from_u64(55)); // value
        push_u256(&mut buf, U256::from_u64(66)); // blinding
        push_g1(&mut buf, &sample_point(90)); // commitment
        push_usize(&mut buf, 1); // numPoint
        push_u256(&mut buf, U256::from_u64(0)); // point[0]

        // 7. DAG input proofs: 1
        push_usize(&mut buf, 1);
        {
            // commitment rows: 1
            push_usize(&mut buf, 1);
            push_g1(&mut buf, &sample_point(91));
            // evals: 0
            push_usize(&mut buf, 0);
        }

        buf
    }

    #[test]
    fn test_decode_proof_for_dag() {
        let data = build_minimal_dag_proof();
        let mut dec = ProofDecoder::new(&data);
        let (gkr, embedded, dag_inputs, pub_claims) = dec.decode_proof_for_dag();

        // Check embedded public inputs
        assert_eq!(embedded.len(), 2);
        assert_eq!(embedded[0], U256::from_u64(777));
        assert_eq!(embedded[1], U256::from_u64(888));

        // Check GKR proof structure
        assert_eq!(gkr.output_claim_commitments.len(), 1);
        assert_eq!(gkr.output_claim_commitments[0], sample_point(50));
        assert_eq!(gkr.layer_proofs.len(), 1);
        assert_eq!(gkr.layer_proofs[0].sumcheck_proof.messages.len(), 1);
        assert_eq!(gkr.layer_proofs[0].commitments.len(), 1);
        assert_eq!(gkr.layer_proofs[0].pops.len(), 0);

        // Check public value claims
        assert_eq!(pub_claims.len(), 1);
        assert_eq!(pub_claims[0].value, U256::from_u64(55));
        assert_eq!(pub_claims[0].blinding, U256::from_u64(66));
        assert_eq!(pub_claims[0].commitment, sample_point(90));

        // Check DAG input proofs
        assert_eq!(dag_inputs.len(), 1);
        assert_eq!(dag_inputs[0].commitment_rows.len(), 1);
        assert_eq!(dag_inputs[0].commitment_rows[0], sample_point(91));
        assert_eq!(dag_inputs[0].podps.len(), 0);
        assert_eq!(dag_inputs[0].com_evals.len(), 0);
    }

    #[test]
    fn test_decode_committed_layer_proof_with_agg() {
        let mut buf = Vec::new();

        // sum G1
        push_g1(&mut buf, &sample_point(1));
        // messages: 2
        push_usize(&mut buf, 2);
        push_g1(&mut buf, &sample_point(2));
        push_g1(&mut buf, &sample_point(3));
        // PODP
        push_g1(&mut buf, &sample_point(4)); // commitD
        push_g1(&mut buf, &sample_point(5)); // commitDDotA
        push_usize(&mut buf, 2); // numZ
        push_u256(&mut buf, U256::from_u64(10));
        push_u256(&mut buf, U256::from_u64(20));
        push_u256(&mut buf, U256::from_u64(30)); // zDelta
        push_u256(&mut buf, U256::from_u64(40)); // zBeta
                                                 // commitments: 3
        push_usize(&mut buf, 3);
        push_g1(&mut buf, &sample_point(6));
        push_g1(&mut buf, &sample_point(7));
        push_g1(&mut buf, &sample_point(8));
        // pops: 1
        push_usize(&mut buf, 1);
        push_g1(&mut buf, &sample_point(20)); // alpha
        push_g1(&mut buf, &sample_point(21)); // beta
        push_g1(&mut buf, &sample_point(22)); // delta
        for i in 1..=5u64 {
            push_u256(&mut buf, U256::from_u64(i));
        }
        // hasAgg: 1
        push_usize(&mut buf, 1);
        // numCoeffs: 1 (skip 64 bytes)
        push_usize(&mut buf, 1);
        buf.extend_from_slice(&[0u8; 64]);
        // numOpenings: 1 (skip 128 bytes)
        push_usize(&mut buf, 1);
        buf.extend_from_slice(&[0u8; 128]);
        // numEq: 1 (skip 96 bytes)
        push_usize(&mut buf, 1);
        buf.extend_from_slice(&[0u8; 96]);

        let mut dec = ProofDecoder::new(&buf);
        let lp = dec.decode_committed_layer_proof();

        assert_eq!(lp.sumcheck_proof.messages.len(), 2);
        assert_eq!(lp.commitments.len(), 3);
        assert_eq!(lp.pops.len(), 1);
        assert_eq!(lp.pops[0].z3, U256::from_u64(3));
        // Verify we consumed the entire buffer (including skipped agg data)
        assert_eq!(dec.offset(), buf.len());
    }

    #[test]
    fn test_decode_embedded_no_sections() {
        let mut buf = Vec::new();
        push_u256(&mut buf, U256::from_u64(0xBEEF)); // circuit hash
        push_usize(&mut buf, 0); // 0 sections

        let result = ProofDecoder::decode_embedded_public_inputs(&buf);
        assert_eq!(result.len(), 0);
    }
}
