//! ABI encoder for HyraxProof → Solidity-compatible bytes.
//!
//! Since some Hyrax proof fields are private, we use serde serialization
//! (JSON) to extract all values, then re-encode as flat big-endian uint256
//! values for the Solidity decoder.
//!
//! EC points arrive as 32-byte compressed hex from halo2curves serde.
//! We decompress them (solve y² = x³ + 3 on BN254) to get affine (x, y)
//! for Solidity's ecAdd/ecMul precompiles.

use anyhow::{anyhow, Result};
use ff::{Field, PrimeField};
use hyrax::gkr::HyraxProof;
use serde_json::Value;
use shared_types::curves::PrimeOrderCurve;
use shared_types::pedersen::PedersenCommitter;
use shared_types::{Bn256Point, Fq};

/// ABI-encode a HyraxProof for on-chain verification.
///
/// Returns proof bytes starting with the "REM1" selector.
pub fn encode_hyrax_proof(
    proof: &HyraxProof<Bn256Point>,
    circuit_hash: &[u8; 32],
) -> Result<Vec<u8>> {
    // Serialize proof to JSON to access private fields
    let json = serde_json::to_value(proof)?;

    let mut buf = Vec::new();

    // Selector: "REM1"
    buf.extend_from_slice(b"REM1");

    // Circuit hash (32 bytes, as-is)
    buf.extend_from_slice(circuit_hash);

    // --- Public inputs ---
    let public_inputs = json["public_inputs"]
        .as_array()
        .ok_or_else(|| anyhow!("missing public_inputs"))?;
    encode_u256(&mut buf, public_inputs.len() as u64);
    for input in public_inputs {
        // Each entry is [layer_id, MLE_struct_or_null]
        let mle = &input[1];
        if mle.is_null() {
            encode_u256(&mut buf, 0u64);
        } else {
            // MLE structure: { f: { evals: { naive_buf: ["hex", ...], num_elements: N, ... }, num_vars: V, zero: "hex" } }
            let naive_buf = mle["f"]["evals"]["naive_buf"]
                .as_array()
                .ok_or_else(|| anyhow!("missing MLE.f.evals.naive_buf"))?;
            let num_elements = mle["f"]["evals"]["num_elements"]
                .as_u64()
                .ok_or_else(|| anyhow!("missing num_elements"))?;
            encode_u256(&mut buf, num_elements);
            for elem in naive_buf.iter().take(num_elements as usize) {
                encode_fr_from_json(&mut buf, elem)?;
            }
        }
    }

    // --- Circuit proof ---
    let circuit_proof = &json["circuit_proof"];

    // Output layer proofs
    let output_proofs = circuit_proof["output_layer_proofs"]
        .as_array()
        .ok_or_else(|| anyhow!("missing output_layer_proofs"))?;
    encode_u256(&mut buf, output_proofs.len() as u64);
    for output in output_proofs {
        // [layer_id, mle_indices, HyraxOutputLayerProof]
        let claim_commitment = &output[2]["claim_commitment"];
        encode_point_from_json(&mut buf, claim_commitment)?;
    }

    // Layer proofs
    let layer_proofs = circuit_proof["layer_proofs"]
        .as_array()
        .ok_or_else(|| anyhow!("missing layer_proofs"))?;
    encode_u256(&mut buf, layer_proofs.len() as u64);
    for layer in layer_proofs {
        // [layer_id, HyraxLayerProof]
        let lp = &layer[1];
        encode_layer_proof(&mut buf, lp)?;
    }

    // Fiat-Shamir claims
    let fs_claims = circuit_proof["fiat_shamir_claims"]
        .as_array()
        .ok_or_else(|| anyhow!("missing fiat_shamir_claims"))?;
    encode_u256(&mut buf, fs_claims.len() as u64);
    for claim in fs_claims {
        encode_hyrax_claim(&mut buf, claim)?;
    }

    // --- Claims on public values ---
    let pub_claims = json["claims_on_public_values"]
        .as_array()
        .ok_or_else(|| anyhow!("missing claims_on_public_values"))?;
    encode_u256(&mut buf, pub_claims.len() as u64);
    for claim in pub_claims {
        encode_hyrax_claim(&mut buf, claim)?;
    }

    // --- Hyrax input layer proofs ---
    let input_proofs = json["hyrax_input_proofs"]
        .as_array()
        .ok_or_else(|| anyhow!("missing hyrax_input_proofs"))?;
    encode_u256(&mut buf, input_proofs.len() as u64);
    for ip in input_proofs {
        // Input commitment rows
        let commits = ip["input_commitment"]
            .as_array()
            .ok_or_else(|| anyhow!("missing input_commitment"))?;
        encode_u256(&mut buf, commits.len() as u64);
        for c in commits {
            encode_point_from_json(&mut buf, c)?;
        }

        // Evaluation proofs
        let eval_proofs = ip["evaluation_proofs"]
            .as_array()
            .ok_or_else(|| anyhow!("missing evaluation_proofs"))?;
        encode_u256(&mut buf, eval_proofs.len() as u64);
        for ep in eval_proofs {
            // ProofOfDotProduct (private fields, accessed via JSON)
            encode_podp_from_json(&mut buf, &ep["podp_evaluation_proof"])?;
            encode_point_from_json(&mut buf, &ep["commitment_to_evaluation"])?;
        }
    }

    Ok(buf)
}

/// ABI-encode Pedersen generators for on-chain PODP verification.
///
/// Format: numGens (uint256) | G1[numGens] (message gens) | G1 (scalarGen) | G1 (blindingGen)
/// Each G1 point is 64 bytes (x, y) in big-endian.
pub fn encode_pedersen_gens(
    committer: &PedersenCommitter<Bn256Point>,
) -> Result<Vec<u8>> {
    let mut buf = Vec::new();

    let num_gens = committer.generators.len();
    encode_u256(&mut buf, num_gens as u64);

    // Message generators
    for gen in &committer.generators {
        let (x, y) = gen
            .affine_coordinates()
            .ok_or_else(|| anyhow!("generator point at infinity"))?;
        encode_fq_be(&mut buf, &x);
        encode_fq_be(&mut buf, &y);
    }

    // Scalar generator (last message gen)
    let scalar_gen = committer.scalar_commit_generator();
    let (sx, sy) = scalar_gen
        .affine_coordinates()
        .ok_or_else(|| anyhow!("scalar gen point at infinity"))?;
    encode_fq_be(&mut buf, &sx);
    encode_fq_be(&mut buf, &sy);

    // Blinding generator
    let (bx, by) = committer
        .blinding_generator
        .affine_coordinates()
        .ok_or_else(|| anyhow!("blinding gen point at infinity"))?;
    encode_fq_be(&mut buf, &bx);
    encode_fq_be(&mut buf, &by);

    Ok(buf)
}

/// Encode an Fq field element as big-endian 32 bytes.
fn encode_fq_be(buf: &mut Vec<u8>, val: &Fq) {
    let repr = val.to_repr();
    let mut be = [0u8; 32];
    be.copy_from_slice(repr.as_ref());
    be.reverse();
    buf.extend_from_slice(&be);
}

fn encode_layer_proof(buf: &mut Vec<u8>, lp: &Value) -> Result<()> {
    // ProofOfSumcheck
    let pos = &lp["proof_of_sumcheck"];
    encode_point_from_json(buf, &pos["sum"])?;

    let messages = pos["messages"]
        .as_array()
        .ok_or_else(|| anyhow!("missing sumcheck messages"))?;
    encode_u256(buf, messages.len() as u64);
    for msg in messages {
        encode_point_from_json(buf, msg)?;
    }

    // PODP inside sumcheck
    encode_podp_from_json(buf, &pos["podp"])?;

    // Commitments
    let commits = lp["commitments"]
        .as_array()
        .ok_or_else(|| anyhow!("missing commitments"))?;
    encode_u256(buf, commits.len() as u64);
    for c in commits {
        encode_point_from_json(buf, c)?;
    }

    // ProofOfProduct list
    let pops = lp["proofs_of_product"]
        .as_array()
        .ok_or_else(|| anyhow!("missing proofs_of_product"))?;
    encode_u256(buf, pops.len() as u64);
    for pop in pops {
        encode_point_from_json(buf, &pop["alpha"])?;
        encode_point_from_json(buf, &pop["beta"])?;
        encode_point_from_json(buf, &pop["delta"])?;
        encode_fr_from_json(buf, &pop["z1"])?;
        encode_fr_from_json(buf, &pop["z2"])?;
        encode_fr_from_json(buf, &pop["z3"])?;
        encode_fr_from_json(buf, &pop["z4"])?;
        encode_fr_from_json(buf, &pop["z5"])?;
    }

    // Claim aggregation (optional)
    let agg = &lp["maybe_proof_of_claim_agg"];
    if agg.is_null() {
        encode_u256(buf, 0u64);
    } else {
        encode_u256(buf, 1u64);
        let coeffs = agg["interpolant_coeffs"]
            .as_array()
            .ok_or_else(|| anyhow!("missing interpolant_coeffs"))?;
        encode_u256(buf, coeffs.len() as u64);
        for c in coeffs {
            encode_point_from_json(buf, c)?;
        }

        let openings = agg["proofs_of_opening"]
            .as_array()
            .ok_or_else(|| anyhow!("missing proofs_of_opening"))?;
        encode_u256(buf, openings.len() as u64);
        for po in openings {
            encode_fr_from_json(buf, &po["z1"])?;
            encode_fr_from_json(buf, &po["z2"])?;
            encode_point_from_json(buf, &po["alpha"])?;
        }

        let equalities = agg["proofs_of_equality"]
            .as_array()
            .ok_or_else(|| anyhow!("missing proofs_of_equality"))?;
        encode_u256(buf, equalities.len() as u64);
        for pe in equalities {
            encode_point_from_json(buf, &pe["alpha"])?;
            encode_fr_from_json(buf, &pe["z"])?;
        }
    }

    Ok(())
}

fn encode_hyrax_claim(buf: &mut Vec<u8>, claim: &Value) -> Result<()> {
    let eval = &claim["evaluation"];
    encode_fr_from_json(buf, &eval["value"])?;
    encode_fr_from_json(buf, &eval["blinding"])?;
    encode_point_from_json(buf, &eval["commitment"])?;

    let point = claim["point"]
        .as_array()
        .ok_or_else(|| anyhow!("missing claim point"))?;
    encode_u256(buf, point.len() as u64);
    for p in point {
        encode_fr_from_json(buf, p)?;
    }

    Ok(())
}

fn encode_podp_from_json(buf: &mut Vec<u8>, podp: &Value) -> Result<()> {
    encode_point_from_json(buf, &podp["commit_d"])?;
    encode_point_from_json(buf, &podp["commit_d_dot_a"])?;

    let z_vec = podp["z_vector"]
        .as_array()
        .ok_or_else(|| anyhow!("missing z_vector"))?;
    encode_u256(buf, z_vec.len() as u64);
    for z in z_vec {
        encode_fr_from_json(buf, z)?;
    }

    encode_fr_from_json(buf, &podp["z_delta"])?;
    encode_fr_from_json(buf, &podp["z_beta"])?;

    Ok(())
}

/// Decompress a BN254 G1 point from 32-byte compressed form (halo2curves format).
///
/// Compressed format (from halo2curves `new_curve_impl!` macro):
///   - Bytes 0..32: x-coordinate (little-endian), with flags in last byte:
///     - Byte 31, bit 7: infinity flag (1 = point at infinity)
///     - Byte 31, bit 6: y sign (LSB of y's LE representation)
///     - Byte 31, bits 0-5: upper bits of x
///
/// We recover y by solving y² = x³ + 3 (mod p) and selecting the root
/// whose LE LSB matches the stored sign bit.
fn decompress_point(compressed: &[u8]) -> Result<([u8; 32], [u8; 32])> {
    if compressed.is_empty() {
        return Ok(([0u8; 32], [0u8; 32]));
    }
    if compressed.len() != 32 {
        return Err(anyhow!(
            "expected 32-byte compressed point, got {} bytes",
            compressed.len()
        ));
    }

    let mut tmp = [0u8; 32];
    tmp.copy_from_slice(compressed);

    // Extract flags from last byte
    let is_infinity = (tmp[31] >> 7) & 1;
    let y_sign = (tmp[31] >> 6) & 1;
    // Clear flag bits to get pure x coordinate
    tmp[31] &= 0b0011_1111;

    // Point at infinity
    if is_infinity == 1 {
        return Ok(([0u8; 32], [0u8; 32]));
    }

    // Parse x as Fq field element (LE representation)
    let x: Fq = Option::from(Fq::from_repr(tmp))
        .ok_or_else(|| anyhow!("invalid x coordinate in compressed point"))?;

    // Compute y² = x³ + 3 (BN254 curve equation: y² = x³ + b, b=3)
    let x2 = x.square();
    let x3 = x2 * x;
    let y2 = x3 + Fq::from(3u64);

    // Compute y = sqrt(y²)
    let y_opt: Option<Fq> = y2.sqrt().into();
    let mut y = y_opt.ok_or_else(|| anyhow!("no sqrt found — x not on BN254 curve"))?;

    // Select correct root: match the sign bit (LSB of y's LE representation)
    let y_repr = y.to_repr();
    let computed_sign = y_repr.as_ref()[0] & 1;
    if computed_sign != y_sign {
        y = -y;
    }

    // Convert x, y to big-endian for Solidity uint256
    let x_repr = x.to_repr();
    let y_repr = y.to_repr();
    let mut x_be = [0u8; 32];
    let mut y_be = [0u8; 32];
    x_be.copy_from_slice(x_repr.as_ref());
    y_be.copy_from_slice(y_repr.as_ref());
    x_be.reverse();
    y_be.reverse();

    Ok((x_be, y_be))
}

/// Encode an EC point from its JSON serialized form (32-byte compressed hex string).
fn encode_point_from_json(buf: &mut Vec<u8>, val: &Value) -> Result<()> {
    let s = val
        .as_str()
        .ok_or_else(|| anyhow!("expected hex string for EC point, got: {:?}", val))?;
    let bytes = hex::decode(s.trim_start_matches("0x"))
        .map_err(|e| anyhow!("invalid point hex: {}", e))?;

    let (x_be, y_be) = decompress_point(&bytes)?;
    buf.extend_from_slice(&x_be);
    buf.extend_from_slice(&y_be);
    Ok(())
}

/// Encode a field element from its JSON serialized form.
///
/// Fr/Fq elements in halo2curves serialize as hex strings of their
/// little-endian representation. We convert to big-endian uint256.
fn encode_fr_from_json(buf: &mut Vec<u8>, val: &Value) -> Result<()> {
    let s = val
        .as_str()
        .ok_or_else(|| anyhow!("expected string for field element, got: {:?}", val))?;
    let bytes = hex::decode(s.trim_start_matches("0x"))
        .map_err(|e| anyhow!("invalid field element hex: {}", e))?;

    let mut be = [0u8; 32];
    // Bytes are little-endian from to_repr(), reverse to big-endian
    for (i, b) in bytes.iter().enumerate().take(32) {
        be[31 - i] = *b;
    }
    buf.extend_from_slice(&be);
    Ok(())
}

/// Encode a usize/u64 as a big-endian uint256.
fn encode_u256(buf: &mut Vec<u8>, val: u64) {
    let mut bytes = [0u8; 32];
    bytes[24..32].copy_from_slice(&val.to_be_bytes());
    buf.extend_from_slice(&bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_u256() {
        let mut buf = Vec::new();
        encode_u256(&mut buf, 42);
        assert_eq!(buf.len(), 32);
        assert_eq!(buf[31], 42);
        assert_eq!(buf[0], 0);
    }

    #[test]
    fn test_encode_fr_from_json() {
        // Fr element representing 6 in LE hex:
        // 0600000000000000000000000000000000000000000000000000000000000000
        let val = serde_json::json!("0600000000000000000000000000000000000000000000000000000000000000");
        let mut buf = Vec::new();
        encode_fr_from_json(&mut buf, &val).unwrap();
        assert_eq!(buf.len(), 32);
        assert_eq!(buf[31], 6); // BE: 6 is at the end
        assert_eq!(buf[0], 0);
    }

    #[test]
    fn test_decompress_identity() {
        // Point at infinity: bit 7 of last byte is set
        let mut infinity = [0u8; 32];
        infinity[31] = 0b1000_0000;
        let (x, y) = decompress_point(&infinity).unwrap();
        assert_eq!(x, [0u8; 32]);
        assert_eq!(y, [0u8; 32]);
    }
}
