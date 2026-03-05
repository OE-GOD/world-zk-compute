/// DAG GKR compute layer verification.
/// Ported from GKRDAGVerifier.sol

use alloc::vec;
use alloc::vec::Vec;

use crate::ec::{ec_add, ec_mul, G1Point, PedersenGens, PODPProof, ProofOfProduct};
use crate::field::{Fr, U256};
use crate::poseidon::PoseidonSponge;
use crate::sumcheck::{self, CommittedSumcheckProof};

/// Point template encoding: entries >= 20000 are fixed values
const FIXED_REF_BASE: u64 = 20000;

// ============================================================
// Types
// ============================================================

/// DAG circuit topology
pub struct DAGCircuitDescription {
    pub num_compute_layers: usize,
    pub num_input_layers: usize,
    pub layer_types: Vec<u8>,           // 0=subtract/identity, 1=multiply
    pub num_sumcheck_rounds: Vec<usize>,
    pub atom_offsets: Vec<usize>,       // Length num_compute_layers+1
    pub atom_target_layers: Vec<usize>, // Flat
    pub atom_commit_idxs: Vec<usize>,   // Flat
    pub pt_offsets: Vec<usize>,         // Length total_atoms+1
    pub pt_data: Vec<u64>,              // Flat: point template entries
    pub input_is_committed: Vec<bool>,
    pub oracle_product_offsets: Vec<usize>, // Length num_compute_layers+1
    pub oracle_result_idxs: Vec<usize>,     // Flat
    pub oracle_expr_coeffs: Vec<U256>,      // Flat (Fr values)
}

/// Per-layer committed proof data
pub struct CommittedLayerProof {
    pub sumcheck_proof: CommittedSumcheckProof,
    pub commitments: Vec<G1Point>,
    pub pops: Vec<ProofOfProduct>,
}

/// Complete GKR proof
pub struct GKRProof {
    pub output_claim_commitments: Vec<G1Point>,
    pub layer_proofs: Vec<CommittedLayerProof>,
}

/// DAG input layer proof
pub struct DAGInputLayerProof {
    pub commitment_rows: Vec<G1Point>,
    pub podps: Vec<PODPProof>,
    pub com_evals: Vec<G1Point>,
}

/// Public value claim (Pedersen opening)
pub struct PublicValueClaim {
    pub value: U256,
    pub blinding: U256,
    pub commitment: G1Point,
}

/// Verification context (bundles all state)
pub struct VerifyContext {
    pub all_bindings: Vec<Vec<U256>>,
    pub output_challenges: Vec<U256>,
    pub output_eval: G1Point,
}

// ============================================================
// Compute Layer Verification
// ============================================================

/// Verify all compute layers of a DAG circuit.
/// Returns VerifyContext with all bindings for input layer verification.
pub fn verify_compute_layers(
    proof: &GKRProof,
    desc: &DAGCircuitDescription,
    gens: &PedersenGens,
    sponge: &mut PoseidonSponge,
) -> VerifyContext {
    let mut ctx = VerifyContext {
        all_bindings: Vec::with_capacity(desc.num_compute_layers),
        output_challenges: Vec::new(),
        output_eval: G1Point::INFINITY,
    };

    // Output layer: squeeze numVars challenges
    let num_vars = proof.layer_proofs[0].sumcheck_proof.messages.len();
    ctx.output_challenges = Vec::with_capacity(num_vars);
    for _ in 0..num_vars {
        ctx.output_challenges.push(Fr::from_fq(&sponge.squeeze()).0);
    }
    ctx.output_eval = proof.output_claim_commitments[0];
    sponge.absorb_u256(&ctx.output_eval.x);
    sponge.absorb_u256(&ctx.output_eval.y);

    // Process each compute layer
    for i in 0..desc.num_compute_layers {
        let bindings = process_one_layer(i, proof, desc, gens, &ctx, sponge);
        ctx.all_bindings.push(bindings);
    }

    ctx
}

/// Process one compute layer: RLC aggregation + committed sumcheck + PoP
fn process_one_layer(
    layer_idx: usize,
    proof: &GKRProof,
    desc: &DAGCircuitDescription,
    gens: &PedersenGens,
    ctx: &VerifyContext,
    sponge: &mut PoseidonSponge,
) -> Vec<U256> {
    // Count claims for this layer
    let mut num_claims = count_claims_for(layer_idx, desc);
    if layer_idx == 0 {
        num_claims += 1; // Output claim
    }

    // Squeeze RLC coefficients
    let mut rlc_coeffs = Vec::with_capacity(num_claims);
    for _ in 0..num_claims {
        rlc_coeffs.push(Fr::from_fq(&sponge.squeeze()).0);
    }

    // Absorb messages and derive bindings
    let lp = &proof.layer_proofs[layer_idx];
    let bindings = absorb_messages_and_derive_bindings(&lp.sumcheck_proof.messages, sponge);

    // Absorb post-sumcheck commitments
    absorb_commitments(&lp.commitments, sponge);

    // Compute RLC eval and beta
    let (_rlc_eval, rlc_beta) = compute_rlc_eval_and_beta(
        layer_idx, proof, desc, ctx, &bindings, &rlc_coeffs,
    );

    // Compute oracle eval
    let oracle_eval = compute_oracle_eval(&lp.commitments, layer_idx, &rlc_beta, desc);

    // Verify committed sumcheck
    let degree = if desc.layer_types[layer_idx] == 1 { 3 } else { 2 };
    assert!(
        sumcheck::verify(
            &lp.sumcheck_proof,
            &oracle_eval,
            degree,
            &bindings,
            gens,
            sponge,
        ),
        "sumcheck failed at layer {}",
        layer_idx
    );

    // Verify ProofOfProduct
    assert!(
        verify_products(lp, gens, sponge),
        "PoP failed at layer {}",
        layer_idx
    );

    bindings
}

/// Absorb sumcheck messages and derive bindings.
fn absorb_messages_and_derive_bindings(
    messages: &[G1Point],
    sponge: &mut PoseidonSponge,
) -> Vec<U256> {
    let n = messages.len();
    let mut bindings = vec![U256::ZERO; n];

    if n > 0 {
        sponge.absorb_u256(&messages[0].x);
        sponge.absorb_u256(&messages[0].y);
    }

    for i in 1..n {
        bindings[i - 1] = Fr::from_fq(&sponge.squeeze()).0;
        sponge.absorb_u256(&messages[i].x);
        sponge.absorb_u256(&messages[i].y);
    }

    if n > 0 {
        bindings[n - 1] = Fr::from_fq(&sponge.squeeze()).0;
    }

    bindings
}

/// Absorb commitments into transcript.
fn absorb_commitments(commitments: &[G1Point], sponge: &mut PoseidonSponge) {
    for c in commitments {
        sponge.absorb_u256(&c.x);
        sponge.absorb_u256(&c.y);
    }
}

/// Compute rlcEval (EC point) and rlcBeta (scalar) from all incoming claims.
fn compute_rlc_eval_and_beta(
    layer_idx: usize,
    proof: &GKRProof,
    desc: &DAGCircuitDescription,
    ctx: &VerifyContext,
    bindings: &[U256],
    rlc_coeffs: &[U256],
) -> (G1Point, U256) {
    let mut rlc_eval = G1Point::INFINITY;
    let mut rlc_beta = Fr::ZERO;
    let mut coeff_idx: usize = 0;

    // Output claim contribution (targets layer 0)
    if layer_idx == 0 {
        rlc_eval = ec_mul(&ctx.output_eval, &rlc_coeffs[0]);
        let beta = compute_beta(bindings, &ctx.output_challenges);
        rlc_beta = Fr::new(beta).mul(&Fr::new(rlc_coeffs[0]));
        coeff_idx = 1;
    }

    // Claims from earlier layers' atoms
    for j in 0..layer_idx {
        let atom_start = desc.atom_offsets[j];
        let atom_end = desc.atom_offsets[j + 1];
        for a in atom_start..atom_end {
            if desc.atom_target_layers[a] != layer_idx {
                continue;
            }

            let commit_idx = desc.atom_commit_idxs[a];
            let atom_eval = proof.layer_proofs[j].commitments[commit_idx];
            let atom_point = resolve_point(a, &ctx.all_bindings[j], desc);
            let beta = compute_beta(bindings, &atom_point);

            rlc_eval = ec_add(
                &rlc_eval,
                &ec_mul(&atom_eval, &rlc_coeffs[coeff_idx]),
            );
            rlc_beta = rlc_beta.add(&Fr::new(beta).mul(&Fr::new(rlc_coeffs[coeff_idx])));
            coeff_idx += 1;
        }
    }

    (rlc_eval, rlc_beta.0)
}

/// Resolve an atom's claim point from template and source bindings.
pub fn resolve_point(
    global_atom_idx: usize,
    source_bindings: &[U256],
    desc: &DAGCircuitDescription,
) -> Vec<U256> {
    let start = desc.pt_offsets[global_atom_idx];
    let end = desc.pt_offsets[global_atom_idx + 1];
    let len = end - start;
    let mut point = vec![U256::ZERO; len];

    for k in 0..len {
        let entry = desc.pt_data[start + k];
        if entry < 1000 {
            point[k] = source_bindings[entry as usize];
        } else if entry >= FIXED_REF_BASE {
            point[k] = U256::from_u64(entry - FIXED_REF_BASE);
        }
    }

    point
}

/// Compute beta(bindings, point) = prod_i(r_i*c_i + (1-r_i)*(1-c_i))
pub fn compute_beta(bindings: &[U256], point: &[U256]) -> U256 {
    let n = bindings.len().min(point.len());
    let mut beta = Fr::ONE;

    for i in 0..n {
        let r = Fr::new(bindings[i]);
        let c = Fr::new(point[i]);
        let rc = r.mul(&c);
        let one_minus_r = Fr::ONE.sub(&r);
        let one_minus_c = Fr::ONE.sub(&c);
        let term = rc.add(&one_minus_r.mul(&one_minus_c));
        beta = beta.mul(&term);
    }

    beta.0
}

/// Compute oracle eval using general expression.
/// oracleEval = rlcBeta * SUM(exprCoeffs[j] * commitments[resultIdxs[j]])
fn compute_oracle_eval(
    commitments: &[G1Point],
    layer_idx: usize,
    rlc_beta: &U256,
    desc: &DAGCircuitDescription,
) -> G1Point {
    let prod_start = desc.oracle_product_offsets[layer_idx];
    let prod_end = desc.oracle_product_offsets[layer_idx + 1];

    let mut result = G1Point::INFINITY;
    for j in prod_start..prod_end {
        let result_idx = desc.oracle_result_idxs[j];
        let coeff = Fr::new(*rlc_beta).mul(&Fr::new(desc.oracle_expr_coeffs[j]));
        let term = ec_mul(&commitments[result_idx], &coeff.0);
        result = ec_add(&result, &term);
    }
    result
}

/// Verify ProofOfProduct entries (sliding window over commitments).
fn verify_products(
    layer_proof: &CommittedLayerProof,
    gens: &PedersenGens,
    sponge: &mut PoseidonSponge,
) -> bool {
    if layer_proof.pops.is_empty() {
        return true;
    }

    let mut commit_idx: usize = 0;
    for pop in &layer_proof.pops {
        assert!(
            commit_idx + 2 < layer_proof.commitments.len(),
            "not enough commitments for PoP"
        );

        if !sumcheck::verify_proof_of_product(
            pop,
            &layer_proof.commitments[commit_idx],
            &layer_proof.commitments[commit_idx + 1],
            &layer_proof.commitments[commit_idx + 2],
            gens,
            sponge,
        ) {
            return false;
        }
        commit_idx += 1;
    }
    true
}

/// Count atoms targeting a specific layer.
pub fn count_claims_for(target_layer: usize, desc: &DAGCircuitDescription) -> usize {
    desc.atom_target_layers
        .iter()
        .filter(|&&t| t == target_layer)
        .count()
}
