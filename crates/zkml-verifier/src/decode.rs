/// Proof decoding from ABI-encoded bytes.
/// Ported from RemainderVerifier.sol decode functions.
///
/// The proof is a flat byte array where every field occupies 32 bytes (big-endian U256).
/// Variable-length sections are length-prefixed: a U256 count followed by that many items.
use crate::ec::{G1Point, PODPProof, PedersenGens, ProofOfProduct};
use crate::error::{Result, VerifyError};
use crate::field::U256;
use crate::gkr::{CommittedLayerProof, DAGInputLayerProof, GKRProof, PublicValueClaim};
use crate::sumcheck::CommittedSumcheckProof;

/// Maximum number of elements allowed in a length-prefixed array.
/// Prevents OOM from malicious inputs with absurdly large counts.
/// A proof for an 88-layer XGBoost circuit is ~475KB; 100_000 elements
/// would require ~3.2MB of 32-byte fields, which is a generous bound.
const MAX_ARRAY_LEN: usize = 100_000;

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
        assert!(end <= self.data.len(), "read_u256: out of bounds");
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
        assert!(
            v.0[1] == 0 && v.0[2] == 0 && v.0[3] == 0,
            "read_usize: value too large for usize"
        );
        v.0[0] as usize
    }

    // --------------------------------------------------------
    // Safe (Result-returning) primitive readers
    // --------------------------------------------------------

    /// Read 32 big-endian bytes, returning Err instead of panicking on OOB.
    pub fn try_read_u256(&mut self) -> Result<U256> {
        let start = self.offset;
        let end = start + 32;
        if end > self.data.len() {
            return Err(VerifyError::DecodeError(format!(
                "read_u256: out of bounds at offset {} (data len {})",
                start,
                self.data.len()
            )));
        }
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&self.data[start..end]);
        self.offset = end;
        Ok(U256::from_be_bytes(&bytes))
    }

    /// Read a G1 point safely.
    fn try_read_g1(&mut self) -> Result<G1Point> {
        let x = self.try_read_u256()?;
        let y = self.try_read_u256()?;
        Ok(G1Point { x, y })
    }

    /// Read a usize safely with bounds checking.
    fn try_read_usize(&mut self) -> Result<usize> {
        let v = self.try_read_u256()?;
        if v.0[1] != 0 || v.0[2] != 0 || v.0[3] != 0 {
            return Err(VerifyError::DecodeError(
                "value too large for usize".to_string(),
            ));
        }
        let val = v.0[0] as usize;
        if val > MAX_ARRAY_LEN {
            return Err(VerifyError::DecodeError(format!(
                "array length {} exceeds maximum {}",
                val, MAX_ARRAY_LEN
            )));
        }
        Ok(val)
    }

    /// Safely advance offset, checking bounds.
    fn try_advance(&mut self, n: usize) -> Result<()> {
        let new_offset = self
            .offset
            .checked_add(n)
            .ok_or_else(|| VerifyError::DecodeError("offset overflow".to_string()))?;
        if new_offset > self.data.len() {
            return Err(VerifyError::DecodeError(format!(
                "advance: out of bounds (offset {} + {} > data len {})",
                self.offset,
                n,
                self.data.len()
            )));
        }
        self.offset = new_offset;
        Ok(())
    }

    // --------------------------------------------------------
    // Safe composite decoders
    // --------------------------------------------------------

    /// Decode a PODP proof safely.
    fn try_decode_podp(&mut self) -> Result<PODPProof> {
        let commit_d = self.try_read_g1()?;
        let commit_d_dot_a = self.try_read_g1()?;
        let num_z = self.try_read_usize()?;
        let mut z_vector = Vec::with_capacity(num_z);
        for _ in 0..num_z {
            z_vector.push(self.try_read_u256()?);
        }
        let z_delta = self.try_read_u256()?;
        let z_beta = self.try_read_u256()?;
        Ok(PODPProof {
            commit_d,
            commit_d_dot_a,
            z_vector,
            z_delta,
            z_beta,
        })
    }

    /// Decode a ProofOfProduct safely.
    fn try_decode_pop(&mut self) -> Result<ProofOfProduct> {
        let alpha = self.try_read_g1()?;
        let beta = self.try_read_g1()?;
        let delta = self.try_read_g1()?;
        let z1 = self.try_read_u256()?;
        let z2 = self.try_read_u256()?;
        let z3 = self.try_read_u256()?;
        let z4 = self.try_read_u256()?;
        let z5 = self.try_read_u256()?;
        Ok(ProofOfProduct {
            alpha,
            beta,
            delta,
            z1,
            z2,
            z3,
            z4,
            z5,
        })
    }

    /// Decode a committed layer proof safely.
    fn try_decode_committed_layer_proof(&mut self) -> Result<CommittedLayerProof> {
        let sum = self.try_read_g1()?;
        let num_messages = self.try_read_usize()?;
        let mut messages = Vec::with_capacity(num_messages);
        for _ in 0..num_messages {
            messages.push(self.try_read_g1()?);
        }
        let podp = self.try_decode_podp()?;
        let sumcheck_proof = CommittedSumcheckProof {
            sum,
            messages,
            podp,
        };

        let num_commits = self.try_read_usize()?;
        let mut commitments = Vec::with_capacity(num_commits);
        for _ in 0..num_commits {
            commitments.push(self.try_read_g1()?);
        }

        let num_pops = self.try_read_usize()?;
        let mut pops = Vec::with_capacity(num_pops);
        for _ in 0..num_pops {
            pops.push(self.try_decode_pop()?);
        }

        let has_agg = self.try_read_usize()?;
        if has_agg == 1 {
            let num_coeffs = self.try_read_usize()?;
            self.try_advance(
                num_coeffs.checked_mul(64).ok_or_else(|| {
                    VerifyError::DecodeError("overflow in coeffs size".to_string())
                })?,
            )?;
            let num_openings = self.try_read_usize()?;
            self.try_advance(num_openings.checked_mul(128).ok_or_else(|| {
                VerifyError::DecodeError("overflow in openings size".to_string())
            })?)?;
            let num_eq = self.try_read_usize()?;
            self.try_advance(num_eq.checked_mul(96).ok_or_else(|| {
                VerifyError::DecodeError("overflow in eq proofs size".to_string())
            })?)?;
        }

        Ok(CommittedLayerProof {
            sumcheck_proof,
            commitments,
            pops,
        })
    }

    /// Decode a DAG input layer proof safely.
    fn try_decode_dag_input_layer_proof(&mut self) -> Result<DAGInputLayerProof> {
        let num_rows = self.try_read_usize()?;
        let mut commitment_rows = Vec::with_capacity(num_rows);
        for _ in 0..num_rows {
            commitment_rows.push(self.try_read_g1()?);
        }
        let num_evals = self.try_read_usize()?;
        let mut podps = Vec::with_capacity(num_evals);
        let mut com_evals = Vec::with_capacity(num_evals);
        for _ in 0..num_evals {
            podps.push(self.try_decode_podp()?);
            com_evals.push(self.try_read_g1()?);
        }
        Ok(DAGInputLayerProof {
            commitment_rows,
            podps,
            com_evals,
        })
    }

    /// Decode public value claims safely.
    fn try_decode_public_value_claims(&mut self) -> Result<Vec<PublicValueClaim>> {
        let num_claims = self.try_read_usize()?;
        let mut claims = Vec::with_capacity(num_claims);
        for _ in 0..num_claims {
            let value = self.try_read_u256()?;
            let blinding = self.try_read_u256()?;
            let commitment = self.try_read_g1()?;
            let num_point = self.try_read_usize()?;
            self.try_advance(
                num_point.checked_mul(32).ok_or_else(|| {
                    VerifyError::DecodeError("overflow in point size".to_string())
                })?,
            )?;
            claims.push(PublicValueClaim {
                value,
                blinding,
                commitment,
            });
        }
        Ok(claims)
    }

    /// Skip a claim section safely.
    fn try_skip_claim(&mut self) -> Result<()> {
        self.try_advance(32)?; // value
        self.try_advance(32)?; // blinding
        self.try_advance(64)?; // commitment (G1)
        let num_point = self.try_read_usize()?;
        self.try_advance(num_point.checked_mul(32).ok_or_else(|| {
            VerifyError::DecodeError("overflow in skip_claim point size".to_string())
        })?)?;
        Ok(())
    }

    /// Decode the common proof prefix safely.
    fn try_decode_proof_common(&mut self, gkr_proof: &mut GKRProof) -> Result<usize> {
        self.try_advance(32)?; // circuit hash

        let num_pub_input_sections = self.try_read_usize()?;
        for _ in 0..num_pub_input_sections {
            let cnt = self.try_read_usize()?;
            self.try_advance(cnt.checked_mul(32).ok_or_else(|| {
                VerifyError::DecodeError("overflow in pub input section size".to_string())
            })?)?;
        }

        let num_output_proofs = self.try_read_usize()?;
        let mut output_claim_commitments = Vec::with_capacity(num_output_proofs);
        for _ in 0..num_output_proofs {
            output_claim_commitments.push(self.try_read_g1()?);
        }
        gkr_proof.output_claim_commitments = output_claim_commitments;

        let num_layer_proofs = self.try_read_usize()?;
        let mut layer_proofs = Vec::with_capacity(num_layer_proofs);
        for _ in 0..num_layer_proofs {
            layer_proofs.push(self.try_decode_committed_layer_proof()?);
        }
        gkr_proof.layer_proofs = layer_proofs;

        let num_fs_claims = self.try_read_usize()?;
        for _ in 0..num_fs_claims {
            self.try_skip_claim()?;
        }

        let pub_claims_offset = self.offset;

        let num_pub_claims = self.try_read_usize()?;
        for _ in 0..num_pub_claims {
            self.try_skip_claim()?;
        }

        Ok(pub_claims_offset)
    }

    /// Safely extract embedded public inputs.
    fn try_extract_embedded_public_inputs(data: &[u8]) -> Result<Vec<U256>> {
        let mut dec = ProofDecoder::new(data);
        dec.try_advance(32)?; // circuit hash

        let num_sections = dec.try_read_usize()?;

        // First pass: count total elements
        let saved_offset = dec.offset;
        let mut total_elements: usize = 0;
        for _ in 0..num_sections {
            let cnt = dec.try_read_usize()?;
            dec.try_advance(cnt.checked_mul(32).ok_or_else(|| {
                VerifyError::DecodeError("overflow in embedded pub inputs".to_string())
            })?)?;
            total_elements = total_elements.checked_add(cnt).ok_or_else(|| {
                VerifyError::DecodeError("overflow in total elements count".to_string())
            })?;
        }

        // Second pass: extract elements
        dec.offset = saved_offset;
        let mut result = Vec::with_capacity(total_elements);
        for _ in 0..num_sections {
            let cnt = dec.try_read_usize()?;
            for _ in 0..cnt {
                result.push(dec.try_read_u256()?);
            }
        }
        Ok(result)
    }

    /// Decode a full proof for DAG verification, returning Result instead of panicking.
    pub fn try_decode_proof_for_dag(
        &mut self,
    ) -> Result<(
        GKRProof,
        Vec<U256>,
        Vec<DAGInputLayerProof>,
        Vec<PublicValueClaim>,
    )> {
        let embedded = Self::try_extract_embedded_public_inputs(self.data)?;

        let mut gkr_proof = GKRProof {
            output_claim_commitments: Vec::new(),
            layer_proofs: Vec::new(),
        };
        let pub_claims_offset = self.try_decode_proof_common(&mut gkr_proof)?;

        let mut claims_decoder = ProofDecoder::new(self.data);
        claims_decoder.set_offset(pub_claims_offset);
        let public_value_claims = claims_decoder.try_decode_public_value_claims()?;

        let num_input_proofs = self.try_read_usize()?;
        let mut dag_input_proofs = Vec::with_capacity(num_input_proofs);
        for _ in 0..num_input_proofs {
            dag_input_proofs.push(self.try_decode_dag_input_layer_proof()?);
        }

        Ok((gkr_proof, embedded, dag_input_proofs, public_value_claims))
    }

    /// Decode Pedersen generators safely.
    pub fn try_decode_pedersen_gens(data: &[u8]) -> Result<PedersenGens> {
        if data.is_empty() {
            return Ok(PedersenGens {
                message_gens: Vec::new(),
                scalar_gen: G1Point::INFINITY,
                blinding_gen: G1Point::INFINITY,
            });
        }
        let mut dec = ProofDecoder::new(data);
        let num_gens = dec.try_read_usize()?;
        let mut message_gens = Vec::with_capacity(num_gens);
        for _ in 0..num_gens {
            message_gens.push(dec.try_read_g1()?);
        }
        let scalar_gen = dec.try_read_g1()?;
        let blinding_gen = dec.try_read_g1()?;
        Ok(PedersenGens {
            message_gens,
            scalar_gen,
            blinding_gen,
        })
    }

    // --------------------------------------------------------
    // Composite decoders (panicking, legacy)
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

        // 6. Save offset for public value claims
        let pub_claims_offset = self.offset;

        // 7. Skip public value claims section
        let num_pub_claims = self.read_usize();
        for _ in 0..num_pub_claims {
            self.skip_claim();
        }

        pub_claims_offset
    }

    /// Decode a full proof for DAG verification.
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
        push_usize(&mut buf, 3);
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
        push_usize(&mut buf, 2);
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
        push_u256(&mut buf, U256::from_u64(0xABCD));
        push_usize(&mut buf, 2);
        push_usize(&mut buf, 3);
        push_u256(&mut buf, U256::from_u64(100));
        push_u256(&mut buf, U256::from_u64(200));
        push_u256(&mut buf, U256::from_u64(300));
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
        push_usize(&mut buf, 2);
        push_u256(&mut buf, U256::from_u64(11));
        push_u256(&mut buf, U256::from_u64(22));
        push_g1(&mut buf, &sample_point(1));
        push_usize(&mut buf, 2);
        push_u256(&mut buf, U256::from_u64(0));
        push_u256(&mut buf, U256::from_u64(0));
        push_u256(&mut buf, U256::from_u64(33));
        push_u256(&mut buf, U256::from_u64(44));
        push_g1(&mut buf, &sample_point(2));
        push_usize(&mut buf, 0);

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
        push_usize(&mut buf, 2);
        push_g1(&mut buf, &sample_point(1));
        push_g1(&mut buf, &sample_point(2));
        push_usize(&mut buf, 1);
        push_g1(&mut buf, &sample_point(10));
        push_g1(&mut buf, &sample_point(11));
        push_usize(&mut buf, 2);
        push_u256(&mut buf, U256::from_u64(7));
        push_u256(&mut buf, U256::from_u64(8));
        push_u256(&mut buf, U256::from_u64(9));
        push_u256(&mut buf, U256::from_u64(10));
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

    fn build_minimal_dag_proof() -> Vec<u8> {
        let mut buf = Vec::new();
        push_u256(&mut buf, U256::from_u64(0xDEAD));
        push_usize(&mut buf, 1);
        push_usize(&mut buf, 2);
        push_u256(&mut buf, U256::from_u64(777));
        push_u256(&mut buf, U256::from_u64(888));
        push_usize(&mut buf, 1);
        push_g1(&mut buf, &sample_point(50));
        push_usize(&mut buf, 1);
        {
            push_g1(&mut buf, &sample_point(60));
            push_usize(&mut buf, 1);
            push_g1(&mut buf, &sample_point(61));
            push_g1(&mut buf, &sample_point(70));
            push_g1(&mut buf, &sample_point(71));
            push_usize(&mut buf, 1);
            push_u256(&mut buf, U256::from_u64(99));
            push_u256(&mut buf, U256::from_u64(100));
            push_u256(&mut buf, U256::from_u64(101));
            push_usize(&mut buf, 1);
            push_g1(&mut buf, &sample_point(80));
            push_usize(&mut buf, 0);
            push_usize(&mut buf, 0);
        }
        push_usize(&mut buf, 0);
        push_usize(&mut buf, 1);
        push_u256(&mut buf, U256::from_u64(55));
        push_u256(&mut buf, U256::from_u64(66));
        push_g1(&mut buf, &sample_point(90));
        push_usize(&mut buf, 1);
        push_u256(&mut buf, U256::from_u64(0));
        push_usize(&mut buf, 1);
        {
            push_usize(&mut buf, 1);
            push_g1(&mut buf, &sample_point(91));
            push_usize(&mut buf, 0);
        }
        buf
    }

    #[test]
    fn test_decode_proof_for_dag() {
        let data = build_minimal_dag_proof();
        let mut dec = ProofDecoder::new(&data);
        let (gkr, embedded, dag_inputs, pub_claims) = dec.decode_proof_for_dag();

        assert_eq!(embedded.len(), 2);
        assert_eq!(embedded[0], U256::from_u64(777));
        assert_eq!(embedded[1], U256::from_u64(888));
        assert_eq!(gkr.output_claim_commitments.len(), 1);
        assert_eq!(gkr.output_claim_commitments[0], sample_point(50));
        assert_eq!(gkr.layer_proofs.len(), 1);
        assert_eq!(gkr.layer_proofs[0].sumcheck_proof.messages.len(), 1);
        assert_eq!(gkr.layer_proofs[0].commitments.len(), 1);
        assert_eq!(gkr.layer_proofs[0].pops.len(), 0);
        assert_eq!(pub_claims.len(), 1);
        assert_eq!(pub_claims[0].value, U256::from_u64(55));
        assert_eq!(pub_claims[0].blinding, U256::from_u64(66));
        assert_eq!(pub_claims[0].commitment, sample_point(90));
        assert_eq!(dag_inputs.len(), 1);
        assert_eq!(dag_inputs[0].commitment_rows.len(), 1);
        assert_eq!(dag_inputs[0].commitment_rows[0], sample_point(91));
        assert_eq!(dag_inputs[0].podps.len(), 0);
        assert_eq!(dag_inputs[0].com_evals.len(), 0);
    }

    #[test]
    fn test_decode_committed_layer_proof_with_agg() {
        let mut buf = Vec::new();
        push_g1(&mut buf, &sample_point(1));
        push_usize(&mut buf, 2);
        push_g1(&mut buf, &sample_point(2));
        push_g1(&mut buf, &sample_point(3));
        push_g1(&mut buf, &sample_point(4));
        push_g1(&mut buf, &sample_point(5));
        push_usize(&mut buf, 2);
        push_u256(&mut buf, U256::from_u64(10));
        push_u256(&mut buf, U256::from_u64(20));
        push_u256(&mut buf, U256::from_u64(30));
        push_u256(&mut buf, U256::from_u64(40));
        push_usize(&mut buf, 3);
        push_g1(&mut buf, &sample_point(6));
        push_g1(&mut buf, &sample_point(7));
        push_g1(&mut buf, &sample_point(8));
        push_usize(&mut buf, 1);
        push_g1(&mut buf, &sample_point(20));
        push_g1(&mut buf, &sample_point(21));
        push_g1(&mut buf, &sample_point(22));
        for i in 1..=5u64 {
            push_u256(&mut buf, U256::from_u64(i));
        }
        push_usize(&mut buf, 1);
        push_usize(&mut buf, 1);
        buf.extend_from_slice(&[0u8; 64]);
        push_usize(&mut buf, 1);
        buf.extend_from_slice(&[0u8; 128]);
        push_usize(&mut buf, 1);
        buf.extend_from_slice(&[0u8; 96]);

        let mut dec = ProofDecoder::new(&buf);
        let lp = dec.decode_committed_layer_proof();

        assert_eq!(lp.sumcheck_proof.messages.len(), 2);
        assert_eq!(lp.commitments.len(), 3);
        assert_eq!(lp.pops.len(), 1);
        assert_eq!(lp.pops[0].z3, U256::from_u64(3));
        assert_eq!(dec.offset(), buf.len());
    }

    #[test]
    fn test_decode_embedded_no_sections() {
        let mut buf = Vec::new();
        push_u256(&mut buf, U256::from_u64(0xBEEF));
        push_usize(&mut buf, 0);

        let result = ProofDecoder::decode_embedded_public_inputs(&buf);
        assert_eq!(result.len(), 0);
    }
}
