/// Input layer verification (Hyrax PODP + public input MLE).
/// Ported from GKRDAGVerifier.sol input layer functions.
use crate::ec::{ec_add, ec_mul, G1Point, PODPProof, PedersenGens};
use crate::field::{Fr, U256};
use crate::gkr::{
    count_claims_for, resolve_point, DAGCircuitDescription, DAGInputLayerProof, GKRProof,
    PublicValueClaim, VerifyContext,
};
use crate::poseidon::PoseidonSponge;
use crate::sumcheck;

// ============================================================
// Input Layer Verification
// ============================================================

/// Verify all input layers (committed + public).
pub fn verify_input_layers(
    proof: &GKRProof,
    desc: &DAGCircuitDescription,
    gens: &PedersenGens,
    ctx: &VerifyContext,
    sponge: &mut PoseidonSponge,
    public_inputs: &[U256],
    dag_input_proofs: &[DAGInputLayerProof],
    public_value_claims: &[PublicValueClaim],
) {
    let mut dag_input_idx: usize = 0;
    let mut pub_claim_idx: usize = 0;

    for input_idx in 0..desc.num_input_layers {
        let target_layer = desc.num_compute_layers + input_idx;

        // Collect claim points for this input layer
        let num_claims = count_claims_for(target_layer, desc);
        let mut claim_points: Vec<Vec<U256>> = Vec::with_capacity(num_claims);

        // Scan atoms from all compute layers
        for j in 0..desc.num_compute_layers {
            let atom_start = desc.atom_offsets[j];
            let atom_end = desc.atom_offsets[j + 1];
            for a in atom_start..atom_end {
                if desc.atom_target_layers[a] == target_layer {
                    claim_points.push(resolve_point(a, &ctx.all_bindings[j], desc));
                }
            }
        }

        if desc.input_is_committed[input_idx] {
            verify_committed_input_batch_eval(
                &dag_input_proofs[dag_input_idx],
                &claim_points,
                sponge,
                gens,
            );
            dag_input_idx += 1;
        } else {
            pub_claim_idx = verify_public_input_claims(
                proof,
                desc,
                gens,
                ctx,
                &claim_points,
                target_layer,
                pub_claim_idx,
                public_inputs,
                public_value_claims,
            );
        }
    }
}

// ============================================================
// Public Input Verification
// ============================================================

/// Verify public input claims: Pedersen opening + MLE evaluation + commitment match.
fn verify_public_input_claims(
    proof: &GKRProof,
    desc: &DAGCircuitDescription,
    gens: &PedersenGens,
    _ctx: &VerifyContext,
    claim_points: &[Vec<U256>],
    target_layer: usize,
    pub_claim_start_idx: usize,
    public_inputs: &[U256],
    public_value_claims: &[PublicValueClaim],
) -> usize {
    let mut next_pub_claim_idx = pub_claim_start_idx;
    let mut c_idx: usize = 0;

    for j in 0..desc.num_compute_layers {
        let atom_start = desc.atom_offsets[j];
        let atom_end = desc.atom_offsets[j + 1];
        for a in atom_start..atom_end {
            if desc.atom_target_layers[a] != target_layer {
                continue;
            }

            verify_one_public_claim(
                &claim_points[c_idx],
                &proof.layer_proofs[j].commitments[desc.atom_commit_idxs[a]],
                next_pub_claim_idx,
                public_inputs,
                public_value_claims,
                gens,
            );
            next_pub_claim_idx += 1;
            c_idx += 1;
        }
    }

    next_pub_claim_idx
}

/// Verify a single public value claim.
fn verify_one_public_claim(
    claim_point: &[U256],
    atom_commitment: &G1Point,
    claim_idx: usize,
    public_inputs: &[U256],
    claims: &[PublicValueClaim],
    gens: &PedersenGens,
) {
    assert!(claim_idx < claims.len(), "not enough public value claims");
    let claim = &claims[claim_idx];

    // 1. Commitment consistency
    assert!(
        claim.commitment == *atom_commitment,
        "public claim commitment mismatch"
    );

    // 2. Pedersen opening: g*value + h*blinding == commitment
    let g_val = ec_mul(&gens.scalar_gen, &claim.value);
    let h_blind = ec_mul(&gens.blinding_gen, &claim.blinding);
    let expected = ec_add(&g_val, &h_blind);
    assert!(expected == claim.commitment, "Pedersen opening invalid");

    // 3. MLE evaluation: MLE(pubData, point) == value
    let mle_val = evaluate_mle_from_data(public_inputs, claim_point);
    assert!(mle_val == claim.value, "public input MLE mismatch");
}

// ============================================================
// Committed Input Verification
// ============================================================

/// Verify committed input layer with batch evaluation proofs.
fn verify_committed_input_batch_eval(
    dag_proof: &DAGInputLayerProof,
    claim_points: &[Vec<U256>],
    sponge: &mut PoseidonSponge,
    gens: &PedersenGens,
) {
    let _num_claims = claim_points.len();
    let num_rows = dag_proof.commitment_rows.len();
    let n = claim_points[0].len();
    let l_half_len = log2(if num_rows > 0 { num_rows } else { 1 });
    let log_n_cols = n - l_half_len;

    // Step 1: Sort claims lexicographically
    let sorted_indices = sort_claim_indices(claim_points);

    // Step 2: Group by R-half (matching Rust's behavior: each claim = 1 group)
    let groups = group_claims_by_r_half(claim_points, &sorted_indices, log_n_cols);

    // Step 3: Verify each group
    assert!(
        groups.len() == dag_proof.podps.len(),
        "eval proof count mismatch"
    );

    for g in 0..groups.len() {
        verify_one_eval_group(
            dag_proof,
            claim_points,
            l_half_len,
            log_n_cols,
            gens,
            g,
            &groups[g],
            sponge,
        );
    }
}

/// Verify a single eval proof group.
fn verify_one_eval_group(
    dag_proof: &DAGInputLayerProof,
    claim_points: &[Vec<U256>],
    l_half_len: usize,
    log_n_cols: usize,
    gens: &PedersenGens,
    group_idx: usize,
    group_claim_indices: &[usize],
    sponge: &mut PoseidonSponge,
) {
    let group_size = group_claim_indices.len();

    // Squeeze RLC coefficients
    let mut rlc_coeffs = Vec::with_capacity(group_size);
    for _ in 0..group_size {
        rlc_coeffs.push(Fr::from_fq(&sponge.squeeze()).0);
    }

    // Extract group's claim points
    let group_points: Vec<&Vec<U256>> = group_claim_indices
        .iter()
        .map(|&idx| &claim_points[idx])
        .collect();

    // Compute L tensor (RLC of L-halves)
    let l_coeffs = compute_rlc_tensor(&group_points, l_half_len, &rlc_coeffs);

    // Compute R tensor (shared R-half from first claim in group)
    let mut r_vars = Vec::with_capacity(log_n_cols);
    for j in 0..log_n_cols {
        r_vars.push(group_points[0][l_half_len + j]);
    }
    let r_coeffs = initialize_tensor(&r_vars);

    // Absorb comEval
    sponge.absorb_u256(&dag_proof.com_evals[group_idx].x);
    sponge.absorb_u256(&dag_proof.com_evals[group_idx].y);

    // Absorb PODP data and squeeze challenge
    let podp_challenge = absorb_podp(&dag_proof.podps[group_idx], sponge);

    // Compute comX = MSM(commitment_rows, l_coeffs)
    let com_x = crate::ec::multi_scalar_mul(&dag_proof.commitment_rows, &l_coeffs);
    let com_y = dag_proof.com_evals[group_idx];

    // Verify PODP
    assert!(
        sumcheck::verify_podp(
            &dag_proof.podps[group_idx],
            &podp_challenge,
            &com_x,
            &com_y,
            &r_coeffs,
            gens,
        ),
        "committed input eval failed"
    );
}

/// Absorb PODP data and return challenge.
fn absorb_podp(podp: &PODPProof, sponge: &mut PoseidonSponge) -> U256 {
    sponge.absorb_u256(&podp.commit_d.x);
    sponge.absorb_u256(&podp.commit_d.y);
    sponge.absorb_u256(&podp.commit_d_dot_a.x);
    sponge.absorb_u256(&podp.commit_d_dot_a.y);
    let challenge = Fr::from_fq(&sponge.squeeze()).0;

    for z in &podp.z_vector {
        sponge.absorb_u256(z);
    }
    sponge.absorb_u256(&podp.z_delta);
    sponge.absorb_u256(&podp.z_beta);

    challenge
}

// ============================================================
// Hybrid Input Layer Verification (transcript replay, Fr-only)
// ============================================================

/// Fr-domain outputs from hybrid input layer verification.
#[derive(Debug, Clone)]
pub struct InputFrOutputs {
    /// Flattened L-tensor values (all groups concatenated).
    pub l_tensor_flat: Vec<U256>,
    /// Per-group z_dot_r = inner_product(z_vector, r_tensor).
    pub z_dot_rs: Vec<U256>,
    /// Per-public-claim MLE evaluation result.
    pub mle_evals: Vec<U256>,
}

/// Hybrid input layer verification: transcript replay + Fr outputs, no EC ops.
pub fn verify_input_layers_hybrid(
    proof: &GKRProof,
    desc: &DAGCircuitDescription,
    _gens: &PedersenGens,
    ctx: &VerifyContext,
    sponge: &mut PoseidonSponge,
    public_inputs: &[U256],
    dag_input_proofs: &[DAGInputLayerProof],
    _public_value_claims: &[PublicValueClaim],
) -> InputFrOutputs {
    let mut outputs = InputFrOutputs {
        l_tensor_flat: Vec::new(),
        z_dot_rs: Vec::new(),
        mle_evals: Vec::new(),
    };

    let mut dag_input_idx: usize = 0;
    let mut pub_claim_idx: usize = 0;

    for input_idx in 0..desc.num_input_layers {
        let target_layer = desc.num_compute_layers + input_idx;

        // Collect claim points for this input layer
        let num_claims = count_claims_for(target_layer, desc);
        let mut claim_points: Vec<Vec<U256>> = Vec::with_capacity(num_claims);

        for j in 0..desc.num_compute_layers {
            let atom_start = desc.atom_offsets[j];
            let atom_end = desc.atom_offsets[j + 1];
            for a in atom_start..atom_end {
                if desc.atom_target_layers[a] == target_layer {
                    claim_points.push(resolve_point(a, &ctx.all_bindings[j], desc));
                }
            }
        }

        if desc.input_is_committed[input_idx] {
            verify_committed_input_batch_eval_hybrid(
                &dag_input_proofs[dag_input_idx],
                &claim_points,
                sponge,
                &mut outputs,
            );
            dag_input_idx += 1;
        } else {
            pub_claim_idx = verify_public_input_claims_hybrid(
                proof,
                desc,
                ctx,
                &claim_points,
                target_layer,
                pub_claim_idx,
                public_inputs,
                &mut outputs,
            );
        }
    }

    outputs
}

/// Hybrid public input claim verification: computes MLE evals without EC checks.
fn verify_public_input_claims_hybrid(
    _proof: &GKRProof,
    desc: &DAGCircuitDescription,
    _ctx: &VerifyContext,
    claim_points: &[Vec<U256>],
    target_layer: usize,
    pub_claim_start_idx: usize,
    public_inputs: &[U256],
    outputs: &mut InputFrOutputs,
) -> usize {
    let mut next_pub_claim_idx = pub_claim_start_idx;
    let mut c_idx: usize = 0;

    for j in 0..desc.num_compute_layers {
        let atom_start = desc.atom_offsets[j];
        let atom_end = desc.atom_offsets[j + 1];
        for a in atom_start..atom_end {
            if desc.atom_target_layers[a] != target_layer {
                continue;
            }

            // Compute MLE eval (Fr only, no Pedersen opening check)
            let mle_val = evaluate_mle_from_data(public_inputs, &claim_points[c_idx]);
            outputs.mle_evals.push(mle_val);

            next_pub_claim_idx += 1;
            c_idx += 1;
        }
    }

    next_pub_claim_idx
}

/// Hybrid committed input batch eval: transcript replay + Fr outputs.
fn verify_committed_input_batch_eval_hybrid(
    dag_proof: &DAGInputLayerProof,
    claim_points: &[Vec<U256>],
    sponge: &mut PoseidonSponge,
    outputs: &mut InputFrOutputs,
) {
    let num_rows = dag_proof.commitment_rows.len();
    let n = claim_points[0].len();
    let l_half_len = log2(if num_rows > 0 { num_rows } else { 1 });
    let log_n_cols = n - l_half_len;

    // Step 1: Sort claims lexicographically
    let sorted_indices = sort_claim_indices(claim_points);

    // Step 2: Group by R-half
    let groups = group_claims_by_r_half(claim_points, &sorted_indices, log_n_cols);

    // Step 3: Verify each group (hybrid)
    for g in 0..groups.len() {
        verify_one_eval_group_hybrid(
            dag_proof,
            claim_points,
            l_half_len,
            log_n_cols,
            g,
            &groups[g],
            sponge,
            outputs,
        );
    }
}

/// Hybrid single eval group: transcript replay + Fr outputs.
fn verify_one_eval_group_hybrid(
    dag_proof: &DAGInputLayerProof,
    claim_points: &[Vec<U256>],
    l_half_len: usize,
    log_n_cols: usize,
    group_idx: usize,
    group_claim_indices: &[usize],
    sponge: &mut PoseidonSponge,
    outputs: &mut InputFrOutputs,
) {
    let group_size = group_claim_indices.len();

    // Squeeze RLC coefficients (same as full)
    let mut rlc_coeffs = Vec::with_capacity(group_size);
    for _ in 0..group_size {
        rlc_coeffs.push(Fr::from_fq(&sponge.squeeze()).0);
    }

    // Extract group's claim points
    let group_points: Vec<&Vec<U256>> = group_claim_indices
        .iter()
        .map(|&idx| &claim_points[idx])
        .collect();

    // Compute L tensor (Fr only)
    let l_coeffs = compute_rlc_tensor(&group_points, l_half_len, &rlc_coeffs);
    outputs.l_tensor_flat.extend_from_slice(&l_coeffs);

    // Compute R tensor (Fr only)
    let mut r_vars = Vec::with_capacity(log_n_cols);
    for j in 0..log_n_cols {
        r_vars.push(group_points[0][l_half_len + j]);
    }
    let r_coeffs = initialize_tensor(&r_vars);

    // Absorb comEval (same as full)
    sponge.absorb_u256(&dag_proof.com_evals[group_idx].x);
    sponge.absorb_u256(&dag_proof.com_evals[group_idx].y);

    // Absorb PODP data and squeeze challenge (same as full)
    let _podp_challenge = absorb_podp(&dag_proof.podps[group_idx], sponge);

    // Compute z_dot_r = inner_product(z_vector, r_coeffs)
    let z_dot_r = crate::ec::inner_product(&dag_proof.podps[group_idx].z_vector, &r_coeffs);
    outputs.z_dot_rs.push(z_dot_r.0);

    // Skip EC equation checks (MSM, PODP verification)
}

// ============================================================
// Tensor Products
// ============================================================

/// Tensor product matching Rust's initialize_tensor (reverse order).
pub fn initialize_tensor(coords: &[U256]) -> Vec<U256> {
    let size = 1usize << coords.len();
    let mut table = vec![U256::ZERO; size];
    table[0] = Fr::ONE.0;

    if coords.is_empty() {
        return table;
    }

    let mut len: usize = 1;
    for idx in (0..coords.len()).rev() {
        let r = Fr::new(coords[idx]);
        let one_minus_r = Fr::ONE.sub(&r);
        for i in 0..len {
            let val = Fr::new(table[i]);
            table[i + len] = val.mul(&r).0;
            table[i] = val.mul(&one_minus_r).0;
        }
        len *= 2;
    }

    table
}

/// Compute RLC of tensor products for L-halves.
fn compute_rlc_tensor(
    claim_points: &[&Vec<U256>],
    l_half_len: usize,
    rlc_coeffs: &[U256],
) -> Vec<U256> {
    let tensor_len = 1usize << l_half_len;
    let mut result = vec![U256::ZERO; tensor_len];

    for c in 0..claim_points.len() {
        let l_vars: Vec<U256> = claim_points[c][..l_half_len].to_vec();
        let tensor = initialize_tensor(&l_vars);
        for j in 0..tensor_len {
            result[j] = Fr::new(result[j])
                .add(&Fr::new(tensor[j]).mul(&Fr::new(rlc_coeffs[c])))
                .0;
        }
    }

    result
}

// ============================================================
// MLE Evaluation
// ============================================================

/// Evaluate MLE of data at point: MLE(x) = sum_w data[w] * eq(w, x)
/// Uses MSB-first convention: point[0] is the MSB of the data index.
pub fn evaluate_mle_from_data(data: &[U256], point: &[U256]) -> U256 {
    let n = point.len();
    assert!(data.len() <= (1 << n), "data too large");

    let mut result = Fr::ZERO;
    for w in 0..data.len() {
        if Fr::new(data[w]).0.is_zero() {
            continue;
        }
        let mut eq = Fr::ONE;
        for i in 0..n {
            // MSB-first: point[0] controls the highest bit of w
            let wi = ((w >> (n - 1 - i)) & 1) as u64;
            let xi = Fr::new(point[i]);
            if wi == 1 {
                eq = eq.mul(&xi);
            } else {
                eq = eq.mul(&Fr::ONE.sub(&xi));
            }
        }
        result = result.add(&Fr::new(data[w]).mul(&eq));
    }

    result.0
}

// ============================================================
// Sorting and Grouping
// ============================================================

/// Sort claim indices lexicographically by point (insertion sort).
fn sort_claim_indices(claim_points: &[Vec<U256>]) -> Vec<usize> {
    let n = claim_points.len();
    let mut indices: Vec<usize> = (0..n).collect();

    for i in 1..n {
        let key = indices[i];
        let mut j = i;
        while j > 0 && compare_points(&claim_points[indices[j - 1]], &claim_points[key]) > 0 {
            indices[j] = indices[j - 1];
            j -= 1;
        }
        indices[j] = key;
    }

    indices
}

/// Lexicographic comparison of two claim points.
fn compare_points(a: &[U256], b: &[U256]) -> i32 {
    let len = a.len().min(b.len());
    for i in 0..len {
        if a[i] == b[i] {
            continue;
        }
        if !a[i].gte(&b[i]) {
            return -1;
        }
        return 1;
    }
    if a.len() < b.len() {
        return -1;
    }
    if a.len() > b.len() {
        return 1;
    }
    0
}

/// Group sorted claims by R-half, matching Rust's behavior.
/// Each claim always creates a new group.
fn group_claims_by_r_half(
    claim_points: &[Vec<U256>],
    sorted_indices: &[usize],
    log_n_cols: usize,
) -> Vec<Vec<usize>> {
    let num_claims = sorted_indices.len();
    let mut groups: Vec<Vec<usize>> = Vec::with_capacity(num_claims);
    let mut temp_groups: Vec<Vec<usize>> = Vec::with_capacity(num_claims);
    let mut temp_group_sizes: Vec<usize> = Vec::with_capacity(num_claims);

    for i in 0..num_claims {
        let claim_idx = sorted_indices[i];

        // Try to find existing group with matching R-half
        for g in 0..i {
            if temp_group_sizes[g] > 0 {
                let first_in_group = temp_groups[g][0];
                if r_half_equals(
                    &claim_points[first_in_group],
                    &claim_points[claim_idx],
                    log_n_cols,
                ) {
                    temp_groups[g].push(claim_idx);
                    temp_group_sizes[g] += 1;
                    break;
                }
            }
        }

        // Always create a new singleton group
        temp_groups.push(vec![claim_idx]);
        temp_group_sizes.push(1);
    }

    // Convert to final format
    for i in 0..num_claims {
        groups.push(temp_groups[i].clone());
    }

    groups
}

/// Check if two points share the same R-half.
fn r_half_equals(a: &[U256], b: &[U256], log_n_cols: usize) -> bool {
    let n = a.len();
    let start_r = n - log_n_cols;
    for i in start_r..n {
        if a[i] != b[i] {
            return false;
        }
    }
    true
}

// ============================================================
// Helpers
// ============================================================

/// floor(log2(x)) for x > 0
fn log2(x: usize) -> usize {
    assert!(x > 0, "log2(0)");
    let mut result = 0;
    let mut v = x;
    while v > 1 {
        v >>= 1;
        result += 1;
    }
    result
}
