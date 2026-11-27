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
    #[cfg(test)]
    fn offset(&self) -> usize {
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
        verify!(end <= self.data.len(), "read_u256: out of bounds");
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

    /// Read a U256 that must fit in 64 bits. The upper limbs must be zero:
    /// dropping them would let 2^64 + e be read as e, while Solidity reads the
    /// full value, so the two verifiers would see different numbers.
    pub(crate) fn read_u64(&mut self) -> u64 {
        let v = self.read_u256();
        verify!(
            v.0[1] == 0 && v.0[2] == 0 && v.0[3] == 0,
            "value does not fit in 64 bits"
        );
        v.0[0]
    }

    /// Read a U256 and interpret it as a `usize` length/count field.
    /// `usize` is 32 bits on wasm32, so the value is converted with a check
    /// instead of a cast that would read 2^32 + k as k.
    pub(crate) fn read_usize(&mut self) -> usize {
        let n = usize::try_from(self.read_u64());
        verify!(n.is_ok(), "value too large for usize");
        n.unwrap_or(0)
    }

    /// Move past `count` items of `item_size` bytes each. The length is computed
    /// with checked arithmetic and must stay inside the data: overflow checks
    /// are off in the wasm32 release build, so an unchecked `count * item_size`
    /// could wrap and move the offset backwards onto bytes already read.
    fn skip(&mut self, count: usize, item_size: usize) {
        let end = count
            .checked_mul(item_size)
            .and_then(|len| self.offset.checked_add(len));
        verify!(
            matches!(end, Some(e) if e <= self.data.len()),
            "skip: out of bounds"
        );
        self.offset = end.unwrap_or(0);
    }

    /// Require that every byte has been read, so that appending bytes to a
    /// blob does not give a second encoding of the same data.
    pub(crate) fn finish(&self) {
        verify!(
            self.offset == self.data.len(),
            "trailing bytes after encoded data"
        );
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
    ///   hasAgg(u256, 0 or 1) | if 1: numCoeffs(u256) skip coeffs*64, numOpenings(u256) skip openings*128, numEq(u256) skip eq*96
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
        // hasAgg is a flag: any other value was read as "no aggregation".
        verify!(has_agg <= 1, "hasAgg must be 0 or 1");
        if has_agg == 1 {
            // Skip coefficients: numCoeffs * 64 bytes (each is a G1 point)
            let num_coeffs = self.read_usize();
            self.skip(num_coeffs, 64);

            // Skip openings: numOpenings * (64 + 64) bytes
            let num_openings = self.read_usize();
            self.skip(num_openings, 128);

            // Skip equality proofs: numEq * (64 + 32) bytes
            let num_eq = self.read_usize();
            self.skip(num_eq, 96);
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
            self.skip(num_point, 32);

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
        self.skip(1, 32 + 32 + 64); // value | blinding | commitment (G1)
        let num_point = self.read_usize();
        self.skip(num_point, 32);
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
        self.skip(1, 32);

        // 2. Skip public inputs section (decoded separately via extract_embedded_public_inputs)
        let num_pub_input_sections = self.read_usize();
        for _ in 0..num_pub_input_sections {
            let cnt = self.read_usize();
            self.skip(cnt, 32);
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
        self.finish();

        (gkr_proof, embedded, dag_input_proofs, public_value_claims)
    }

    /// Extract the embedded public input values from the proof binary.
    ///
    /// These are stored right after the circuit hash as:
    ///   numSections(u256), per section: count(u256) + count*32 bytes of Fr values.
    ///
    /// Uses a two-pass approach: first count total elements, then extract.
    fn extract_embedded_public_inputs(data: &[u8]) -> Vec<U256> {
        let mut dec = ProofDecoder::new(data);

        // Skip circuit hash (32 bytes)
        dec.skip(1, 32);

        // Read number of sections
        let num_sections = dec.read_usize();

        // First pass: count total elements
        let saved_offset = dec.offset;
        let mut total_elements: usize = 0;
        for _ in 0..num_sections {
            let cnt = dec.read_usize();
            dec.skip(cnt, 32);
            // Cannot overflow: each count was just checked against the data length.
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
        dec.finish();

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

        let result = ProofDecoder::extract_embedded_public_inputs(&buf);
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

    /// A layer proof up to (not including) the hasAgg word: no messages,
    /// an empty PODP, no commitments, no PoPs.
    fn layer_proof_prefix() -> Vec<u8> {
        let mut buf = Vec::new();
        push_g1(&mut buf, &sample_point(1)); // sum
        push_usize(&mut buf, 0); // messages
        push_g1(&mut buf, &sample_point(2)); // commitD
        push_g1(&mut buf, &sample_point(3)); // commitDDotA
        push_usize(&mut buf, 0); // numZ
        push_u256(&mut buf, U256::from_u64(4)); // zDelta
        push_u256(&mut buf, U256::from_u64(5)); // zBeta
        push_usize(&mut buf, 0); // commitments
        push_usize(&mut buf, 0); // pops
        buf
    }

    #[test]
    #[should_panic(expected = "hasAgg must be 0 or 1")]
    fn test_has_agg_other_than_zero_or_one_is_rejected() {
        let mut buf = layer_proof_prefix();
        push_usize(&mut buf, 2);
        let mut dec = ProofDecoder::new(&buf);
        dec.decode_committed_layer_proof();
    }

    #[test]
    #[should_panic(expected = "skip: out of bounds")]
    fn test_skip_count_that_overflows_is_rejected() {
        // numCoeffs * 64 overflows usize; it must not wrap to a small skip.
        let mut buf = layer_proof_prefix();
        push_usize(&mut buf, 1); // hasAgg
        push_u256(&mut buf, U256::from_u64(u64::MAX >> 5)); // numCoeffs
        push_usize(&mut buf, 0);
        push_usize(&mut buf, 0);
        let mut dec = ProofDecoder::new(&buf);
        dec.decode_committed_layer_proof();
    }

    #[test]
    #[should_panic(expected = "skip: out of bounds")]
    fn test_skip_past_end_of_data_is_rejected() {
        // The last section claims one equality proof (96 bytes) that is not there.
        let mut buf = layer_proof_prefix();
        push_usize(&mut buf, 1); // hasAgg
        push_usize(&mut buf, 0); // numCoeffs
        push_usize(&mut buf, 0); // numOpenings
        push_usize(&mut buf, 1); // numEq
        let mut dec = ProofDecoder::new(&buf);
        dec.decode_committed_layer_proof();
    }

    #[test]
    #[should_panic(expected = "skip: out of bounds")]
    fn test_public_input_count_past_end_is_rejected() {
        let mut buf = Vec::new();
        push_u256(&mut buf, U256::from_u64(0xABCD)); // circuit hash
        push_usize(&mut buf, 1); // 1 section
        push_u256(&mut buf, U256::from_u64(1 << 59)); // count * 32 wraps a 64-bit usize to 0
        let _ = ProofDecoder::extract_embedded_public_inputs(&buf);
    }

    #[test]
    #[should_panic(expected = "value does not fit in 64 bits")]
    fn test_count_above_64_bits_is_rejected() {
        // Counts were always checked this way; the array readers that used
        // to cut entries down are tested next to them in lib.rs.
        let mut buf = Vec::new();
        push_u256(&mut buf, U256([7, 1, 0, 0])); // 2^64 + 7
        let mut dec = ProofDecoder::new(&buf);
        dec.read_usize();
    }

    #[test]
    #[should_panic(expected = "trailing bytes after encoded data")]
    fn test_proof_with_trailing_bytes_is_rejected() {
        let mut data = build_minimal_dag_proof();
        data.extend_from_slice(&[0u8; 32]);
        let mut dec = ProofDecoder::new(&data);
        let _ = dec.decode_proof_for_dag();
    }

    #[test]
    #[should_panic(expected = "trailing bytes after encoded data")]
    fn test_gens_with_trailing_bytes_are_rejected() {
        let mut buf = Vec::new();
        push_usize(&mut buf, 1);
        push_g1(&mut buf, &sample_point(10));
        push_g1(&mut buf, &sample_point(20));
        push_g1(&mut buf, &sample_point(30));
        buf.push(0);
        let _ = ProofDecoder::decode_pedersen_gens(&buf);
    }

    #[test]
    fn test_decode_embedded_no_sections() {
        let mut buf = Vec::new();
        push_u256(&mut buf, U256::from_u64(0xBEEF)); // circuit hash
        push_usize(&mut buf, 0); // 0 sections

        let result = ProofDecoder::extract_embedded_public_inputs(&buf);
        assert_eq!(result.len(), 0);
    }
}
