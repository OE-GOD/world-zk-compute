/// DAG GKR compute layer verification.
/// Ported from GKRDAGVerifier.sol / Stylus gkr-verifier
use crate::ec::{ec_add, ec_mul, G1Point, PedersenGens, ProofOfProduct};
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
    pub layer_types: Vec<u8>, // 0=subtract/identity, 1=multiply
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
    pub podps: Vec<crate::ec::PODPProof>,
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

/// Check that the proof has the shape the circuit description declares.
///
/// The description fixes how many compute layers there are and how many
/// sumcheck rounds each one has; the proof must match it. Without this, a
/// description of zero compute layers skips every check, and the proof alone
/// decides how many rounds (and so how many bindings) each layer gets. Upstream
/// Remainder checks the binding count against the layer description in the
/// same way. A description of zero compute layers is refused because the proof
/// must have at least one layer proof and exactly as many as the description.
pub fn check_proof_shape(proof: &GKRProof, desc: &DAGCircuitDescription) -> Result<(), String> {
    if proof.layer_proofs.is_empty() || proof.output_claim_commitments.is_empty() {
        return Err("proof has no layer proofs or no output claim".to_string());
    }
    if desc.num_compute_layers != proof.layer_proofs.len() {
        return Err(format!(
            "circuit description declares {} compute layers but the proof has {}",
            desc.num_compute_layers,
            proof.layer_proofs.len()
        ));
    }
    if desc.num_sumcheck_rounds.len() != desc.num_compute_layers {
        return Err(format!(
            "circuit description has {} sumcheck round counts for {} compute layers",
            desc.num_sumcheck_rounds.len(),
            desc.num_compute_layers
        ));
    }
    for (i, (lp, &rounds)) in proof
        .layer_proofs
        .iter()
        .zip(&desc.num_sumcheck_rounds)
        .enumerate()
    {
        let messages = lp.sumcheck_proof.messages.len();
        if messages != rounds {
            return Err(format!(
                "layer {i}: circuit description declares {rounds} sumcheck rounds but the proof has {messages} messages"
            ));
        }
    }
    Ok(())
}

/// Verify all compute layers of a DAG circuit.
/// Returns VerifyContext with all bindings for input layer verification.
pub fn verify_compute_layers(
    proof: &GKRProof,
    desc: &DAGCircuitDescription,
    gens: &PedersenGens,
    sponge: &mut PoseidonSponge,
) -> VerifyContext {
    if let Err(e) = check_proof_shape(proof, desc) {
        panic!("{e}");
    }
    let mut ctx = VerifyContext {
        all_bindings: Vec::with_capacity(desc.num_compute_layers),
        output_challenges: Vec::new(),
        output_eval: G1Point::INFINITY,
    };

    // Output layer: one challenge per variable of layer 0, a count the
    // description fixes (the proof was checked to carry as many messages).
    let num_vars = desc.num_sumcheck_rounds[0];
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
    let (_rlc_eval, rlc_beta) =
        compute_rlc_eval_and_beta(layer_idx, proof, desc, ctx, &bindings, &rlc_coeffs);

    // Compute oracle eval
    let oracle_eval = compute_oracle_eval(&lp.commitments, layer_idx, &rlc_beta, desc);

    // Verify committed sumcheck
    let degree = if desc.layer_types[layer_idx] == 1 {
        3
    } else {
        2
    };
    assert!(
        sumcheck::verify(
            &lp.sumcheck_proof,
            &oracle_eval,
            degree,
            &bindings,
            gens,
            sponge,
        ),
        "sumcheck failed"
    );

    // Verify ProofOfProduct
    assert!(verify_products(lp, gens, sponge), "PoP failed");

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

            rlc_eval = ec_add(&rlc_eval, &ec_mul(&atom_eval, &rlc_coeffs[coeff_idx]));
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
    // Both offsets come from the circuit description. A non-empty template
    // must lie inside `pt_data` before its length sizes the allocation below.
    // An empty template reads nothing, so its offsets are not checked.
    assert!(
        start == end || (start < end && end <= desc.pt_data.len()),
        "point template out of range"
    );
    let len = end - start;
    let mut point = vec![U256::ZERO; len];

    for (k, pt) in point.iter_mut().enumerate() {
        let entry = desc.pt_data[start + k];
        if entry < 1000 {
            *pt = source_bindings[entry as usize];
        } else if entry >= FIXED_REF_BASE {
            *pt = U256::from_u64(entry - FIXED_REF_BASE);
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

    for (commit_idx, pop) in layer_proof.pops.iter().enumerate() {
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
    }
    true
}

// ============================================================
// Hybrid Verification (transcript replay, Fr-only, no EC)
// ============================================================

/// Collects Fr-domain outputs from hybrid compute layer verification.
#[derive(Debug, Clone)]
pub struct FrOutputCollector {
    /// Per-layer RLC beta scalar: the weighted multi-linear extension evaluation.
    pub rlc_betas: Vec<U256>,
    /// Per-layer z_dot_j_star: inner product from committed sumcheck PODP.
    pub z_dot_j_stars: Vec<U256>,
}

/// Hybrid compute layer verification: transcript replay + Fr outputs.
pub fn verify_compute_layers_hybrid(
    proof: &GKRProof,
    desc: &DAGCircuitDescription,
    _gens: &PedersenGens,
    sponge: &mut PoseidonSponge,
) -> (VerifyContext, FrOutputCollector) {
    if let Err(e) = check_proof_shape(proof, desc) {
        panic!("{e}");
    }
    let mut ctx = VerifyContext {
        all_bindings: Vec::with_capacity(desc.num_compute_layers),
        output_challenges: Vec::new(),
        output_eval: G1Point::INFINITY,
    };
    let mut collector = FrOutputCollector {
        rlc_betas: Vec::with_capacity(desc.num_compute_layers),
        z_dot_j_stars: Vec::with_capacity(desc.num_compute_layers),
    };

    // Output layer: squeeze numVars challenges (same as full)
    let num_vars = desc.num_sumcheck_rounds[0];
    ctx.output_challenges = Vec::with_capacity(num_vars);
    for _ in 0..num_vars {
        ctx.output_challenges.push(Fr::from_fq(&sponge.squeeze()).0);
    }
    ctx.output_eval = proof.output_claim_commitments[0];
    sponge.absorb_u256(&ctx.output_eval.x);
    sponge.absorb_u256(&ctx.output_eval.y);

    // Process each compute layer (hybrid)
    for i in 0..desc.num_compute_layers {
        let (bindings, rlc_beta, z_dot_j_star) =
            process_one_layer_hybrid(i, proof, desc, &ctx, sponge);
        ctx.all_bindings.push(bindings);
        collector.rlc_betas.push(rlc_beta);
        collector.z_dot_j_stars.push(z_dot_j_star);
    }

    (ctx, collector)
}

/// Hybrid single compute layer: transcript replay + Fr outputs.
fn process_one_layer_hybrid(
    layer_idx: usize,
    proof: &GKRProof,
    desc: &DAGCircuitDescription,
    ctx: &VerifyContext,
    sponge: &mut PoseidonSponge,
) -> (Vec<U256>, U256, U256) {
    // Count claims for this layer (same as full)
    let mut num_claims = count_claims_for(layer_idx, desc);
    if layer_idx == 0 {
        num_claims += 1;
    }

    // Squeeze RLC coefficients (same as full)
    let mut rlc_coeffs = Vec::with_capacity(num_claims);
    for _ in 0..num_claims {
        rlc_coeffs.push(Fr::from_fq(&sponge.squeeze()).0);
    }

    // Absorb messages and derive bindings (same as full)
    let lp = &proof.layer_proofs[layer_idx];
    let bindings = absorb_messages_and_derive_bindings(&lp.sumcheck_proof.messages, sponge);

    // Absorb post-sumcheck commitments (same as full)
    absorb_commitments(&lp.commitments, sponge);

    // Compute RLC beta only (Fr-domain, skip rlcEval EC point)
    let rlc_beta = compute_rlc_beta_only(layer_idx, desc, ctx, &bindings, &rlc_coeffs);

    // Skip oracle eval computation (pure EC)

    // Hybrid committed sumcheck: transcript replay + z_dot_j_star
    let degree = if desc.layer_types[layer_idx] == 1 {
        3
    } else {
        2
    };
    let z_dot_j_star = sumcheck::verify_hybrid(&lp.sumcheck_proof, degree, &bindings, sponge);

    // Hybrid PoP: transcript replay only (skip EC equation checks)
    verify_products_hybrid(lp, sponge);

    (bindings, rlc_beta, z_dot_j_star)
}

/// Compute rlcBeta scalar only (no EC point computation).
fn compute_rlc_beta_only(
    layer_idx: usize,
    desc: &DAGCircuitDescription,
    ctx: &VerifyContext,
    bindings: &[U256],
    rlc_coeffs: &[U256],
) -> U256 {
    let mut rlc_beta = Fr::ZERO;
    let mut coeff_idx: usize = 0;

    // Output claim contribution (targets layer 0)
    if layer_idx == 0 {
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
            let atom_point = resolve_point(a, &ctx.all_bindings[j], desc);
            let beta = compute_beta(bindings, &atom_point);
            rlc_beta = rlc_beta.add(&Fr::new(beta).mul(&Fr::new(rlc_coeffs[coeff_idx])));
            coeff_idx += 1;
        }
    }

    rlc_beta.0
}

/// Hybrid PoP verification: replay transcript absorb/squeeze, skip EC equation checks.
fn verify_products_hybrid(layer_proof: &CommittedLayerProof, sponge: &mut PoseidonSponge) {
    for pop in &layer_proof.pops {
        // Absorb alpha, beta, delta (same as full)
        sponge.absorb_u256(&pop.alpha.x);
        sponge.absorb_u256(&pop.alpha.y);
        sponge.absorb_u256(&pop.beta.x);
        sponge.absorb_u256(&pop.beta.y);
        sponge.absorb_u256(&pop.delta.x);
        sponge.absorb_u256(&pop.delta.y);

        // Squeeze challenge (same as full)
        let _c = Fr::from_fq(&sponge.squeeze()).0;

        // Absorb z1..z5 (same as full)
        sponge.absorb_u256(&pop.z1);
        sponge.absorb_u256(&pop.z2);
        sponge.absorb_u256(&pop.z3);
        sponge.absorb_u256(&pop.z4);
        sponge.absorb_u256(&pop.z5);

        // Skip EC equation checks
    }
}

/// Count atoms targeting a specific layer.
pub fn count_claims_for(target_layer: usize, desc: &DAGCircuitDescription) -> usize {
    desc.atom_target_layers
        .iter()
        .filter(|&&t| t == target_layer)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desc_with_templates(pt_offsets: Vec<usize>, pt_data: Vec<u64>) -> DAGCircuitDescription {
        DAGCircuitDescription {
            num_compute_layers: 0,
            num_input_layers: 0,
            layer_types: Vec::new(),
            num_sumcheck_rounds: Vec::new(),
            atom_offsets: Vec::new(),
            atom_target_layers: Vec::new(),
            atom_commit_idxs: Vec::new(),
            pt_offsets,
            pt_data,
            input_is_committed: Vec::new(),
            oracle_product_offsets: Vec::new(),
            oracle_result_idxs: Vec::new(),
            oracle_expr_coeffs: Vec::new(),
        }
    }

    #[test]
    fn test_resolve_point_reads_template() {
        let desc = desc_with_templates(vec![0, 1, 3], vec![5, 0, FIXED_REF_BASE + 7]);
        let bindings = [U256::from_u64(11)];
        assert_eq!(
            resolve_point(1, &bindings, &desc),
            vec![U256::from_u64(11), U256::from_u64(7)]
        );
    }

    /// An empty template resolves to an empty point wherever its offsets lie.
    #[test]
    fn test_resolve_point_empty_template_ignores_offsets() {
        let desc = desc_with_templates(vec![9, 9], vec![0]);
        assert!(resolve_point(0, &[], &desc).is_empty());
    }

    #[test]
    #[should_panic(expected = "point template out of range")]
    fn test_resolve_point_rejects_end_past_data() {
        let desc = desc_with_templates(vec![0, 2], vec![0]);
        resolve_point(0, &[U256::ZERO], &desc);
    }

    #[test]
    #[should_panic(expected = "point template out of range")]
    fn test_resolve_point_rejects_start_above_end() {
        let desc = desc_with_templates(vec![1, 0], vec![0]);
        resolve_point(0, &[U256::ZERO], &desc);
    }
}
