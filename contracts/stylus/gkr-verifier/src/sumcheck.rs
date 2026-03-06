/// Committed sumcheck verification.
/// Ported from CommittedSumcheckVerifier.sol
use alloc::vec;
use alloc::vec::Vec;

use crate::ec::{
    ec_add, ec_mul, msm_with_truncated_gens, multi_scalar_mul, G1Point, PODPProof, PedersenGens,
};
use crate::field::{Fr, U256};
use crate::poseidon::PoseidonSponge;

/// Committed sumcheck proof
pub struct CommittedSumcheckProof {
    pub sum: G1Point,
    pub messages: Vec<G1Point>,
    pub podp: PODPProof,
}

/// Verify a committed sumcheck proof.
/// Returns true if valid.
pub fn verify(
    proof: &CommittedSumcheckProof,
    oracle_eval: &G1Point,
    degree: usize,
    bindings: &[U256],
    gens: &PedersenGens,
    sponge: &mut PoseidonSponge,
) -> bool {
    let n = proof.messages.len();
    verify!(bindings.len() == n, "bindings length mismatch");

    // Step 1: Squeeze rho challenges (n+1)
    let mut rhos = Vec::with_capacity(n + 1);
    for _ in 0..=n {
        rhos.push(Fr::from_fq(&sponge.squeeze()).0);
    }

    // Step 2: Squeeze gamma challenges (n)
    let mut gammas = Vec::with_capacity(n);
    for _ in 0..n {
        gammas.push(Fr::from_fq(&sponge.squeeze()).0);
    }

    // Step 3: Compute alpha = MSM(messages, gammas)
    let alpha = multi_scalar_mul(&proof.messages, &gammas);

    // Step 4: Compute j_star vector
    let j_star = compute_j_star(&rhos, &gammas, bindings, degree, n);

    // Step 5: Compute dot_product = sum * rho0 + oracle_eval * (-rhoN)
    let dot_product = compute_dot_product(&proof.sum, oracle_eval, &rhos[0], &rhos[n]);

    // Step 6: Verify PODP with transcript
    verify_podp_with_transcript(&proof.podp, &alpha, &dot_product, &j_star, gens, sponge)
}

/// Compute the j_star vector for PODP verification.
/// j_star[i*(degree+1) + d] = gamma_inv[i] * (rhos[i] * coeff[d] - rhos[i+1] * bindings[i]^d)
/// where coeff[d] = 2 if d==0, else 1
pub fn compute_j_star(
    rhos: &[U256],
    gammas: &[U256],
    bindings: &[U256],
    degree: usize,
    n: usize,
) -> Vec<U256> {
    let degree_plus_one = degree + 1;
    let mut j_star = vec![U256::ZERO; degree_plus_one * n];

    for i in 0..n {
        let gamma_inv = Fr::new(gammas[i]).inv();
        let mut binding_power = Fr::ONE; // bindings[i]^d, starting at d=0

        for d in 0..degree_plus_one {
            let coeff: u64 = if d == 0 { 2 } else { 1 };

            // rhos[i] * coeff
            let term1 = Fr::new(rhos[i]).mul(&Fr::from_u64(coeff));
            // rhos[i+1] * bindings[i]^d
            let term2 = Fr::new(rhos[i + 1]).mul(&binding_power);
            // diff = term1 - term2
            let diff = term1.sub(&term2);

            j_star[i * degree_plus_one + d] = gamma_inv.mul(&diff).0;

            // Update binding power
            binding_power = binding_power.mul(&Fr::new(bindings[i]));
        }
    }

    j_star
}

/// Compute dot_product = sum * rho0 + oracle_eval * (-rhoN)
fn compute_dot_product(sum: &G1Point, oracle_eval: &G1Point, rho0: &U256, rho_n: &U256) -> G1Point {
    // sum * rho0
    let term1 = ec_mul(sum, rho0);
    // oracle_eval * (-rhoN) = oracle_eval * (FR_MOD - rhoN)
    let neg_rho_n = Fr::new(*rho_n).neg().0;
    let term2 = ec_mul(oracle_eval, &neg_rho_n);
    ec_add(&term1, &term2)
}

/// Verify PODP with full transcript operations.
pub fn verify_podp_with_transcript(
    podp: &PODPProof,
    com_x: &G1Point,
    com_y: &G1Point,
    a_vector: &[U256],
    gens: &PedersenGens,
    sponge: &mut PoseidonSponge,
) -> bool {
    // Absorb commit_d and commit_d_dot_a
    sponge.absorb_u256(&podp.commit_d.x);
    sponge.absorb_u256(&podp.commit_d.y);
    sponge.absorb_u256(&podp.commit_d_dot_a.x);
    sponge.absorb_u256(&podp.commit_d_dot_a.y);

    // Squeeze challenge
    let challenge = Fr::from_fq(&sponge.squeeze()).0;

    // Absorb z_vector
    for z in &podp.z_vector {
        sponge.absorb_u256(z);
    }

    // Absorb z_delta, z_beta
    sponge.absorb_u256(&podp.z_delta);
    sponge.absorb_u256(&podp.z_beta);

    // Verify the two PODP equations
    verify_podp(podp, &challenge, com_x, com_y, a_vector, gens)
}

/// Verify PODP (ProofOfDotProduct) equations.
/// Check 1: c * com_x + commit_d == com_z
///   where com_z = MSM(g_1..g_n, z_vector) + z_delta * h
/// Check 2: c * com_y + commit_d_dot_a == com_z_dot_a
///   where com_z_dot_a = <z_vector, a_vector> * g_scalar + z_beta * h
pub fn verify_podp(
    podp: &PODPProof,
    challenge: &U256,
    com_x: &G1Point,
    com_y: &G1Point,
    a_vector: &[U256],
    gens: &PedersenGens,
) -> bool {
    verify!(podp.z_vector.len() == a_vector.len());
    verify!(podp.z_vector.len() <= gens.message_gens.len());

    // 1. Compute z_dot_a = <z_vector, a_vector> (mod Fr)
    let z_dot_a = crate::ec::inner_product(&podp.z_vector, a_vector);

    // 2. Compute com_z = MSM(g_1..g_n, z_vector) + z_delta * h
    let msm_result = msm_with_truncated_gens(&gens.message_gens, &podp.z_vector);
    let z_delta_h = ec_mul(&gens.blinding_gen, &podp.z_delta);
    let com_z = ec_add(&msm_result, &z_delta_h);

    // 3. Compute com_z_dot_a = z_dot_a * g_scalar + z_beta * h
    let z_dot_a_g = ec_mul(&gens.scalar_gen, &z_dot_a.0);
    let z_beta_h = ec_mul(&gens.blinding_gen, &podp.z_beta);
    let com_z_dot_a = ec_add(&z_dot_a_g, &z_beta_h);

    // 4. Check 1: c * com_x + commit_d == com_z
    let lhs1 = ec_add(&ec_mul(com_x, challenge), &podp.commit_d);
    if lhs1 != com_z {
        return false;
    }

    // 5. Check 2: c * com_y + commit_d_dot_a == com_z_dot_a
    let lhs2 = ec_add(&ec_mul(com_y, challenge), &podp.commit_d_dot_a);
    if lhs2 != com_z_dot_a {
        return false;
    }

    true
}

/// Verify ProofOfProduct: proves x*y=z on committed values.
/// Transcript: absorb alpha, beta, delta, squeeze c, absorb z1..z5.
/// 3 EC equations:
///   alpha + c * com_x == z1 * g + z2 * h
///   beta + c * com_y == z3 * g + z4 * h
///   delta + c * com_z == z3 * com_x + z5 * h
pub fn verify_proof_of_product(
    pop: &crate::ec::ProofOfProduct,
    com_x: &G1Point,
    com_y: &G1Point,
    com_z: &G1Point,
    gens: &PedersenGens,
    sponge: &mut PoseidonSponge,
) -> bool {
    // Absorb alpha, beta, delta
    sponge.absorb_u256(&pop.alpha.x);
    sponge.absorb_u256(&pop.alpha.y);
    sponge.absorb_u256(&pop.beta.x);
    sponge.absorb_u256(&pop.beta.y);
    sponge.absorb_u256(&pop.delta.x);
    sponge.absorb_u256(&pop.delta.y);

    // Squeeze challenge
    let c = Fr::from_fq(&sponge.squeeze()).0;

    // Absorb z1..z5
    sponge.absorb_u256(&pop.z1);
    sponge.absorb_u256(&pop.z2);
    sponge.absorb_u256(&pop.z3);
    sponge.absorb_u256(&pop.z4);
    sponge.absorb_u256(&pop.z5);

    // Check 1: alpha + c * com_x == z1 * g + z2 * h
    let lhs1 = ec_add(&pop.alpha, &ec_mul(com_x, &c));
    let rhs1 = ec_add(
        &ec_mul(&gens.scalar_gen, &pop.z1),
        &ec_mul(&gens.blinding_gen, &pop.z2),
    );
    if lhs1 != rhs1 {
        return false;
    }

    // Check 2: beta + c * com_y == z3 * g + z4 * h
    let lhs2 = ec_add(&pop.beta, &ec_mul(com_y, &c));
    let rhs2 = ec_add(
        &ec_mul(&gens.scalar_gen, &pop.z3),
        &ec_mul(&gens.blinding_gen, &pop.z4),
    );
    if lhs2 != rhs2 {
        return false;
    }

    // Check 3: delta + c * com_z == z3 * com_x + z5 * h
    let lhs3 = ec_add(&pop.delta, &ec_mul(com_z, &c));
    let rhs3 = ec_add(
        &ec_mul(com_x, &pop.z3),
        &ec_mul(&gens.blinding_gen, &pop.z5),
    );
    if lhs3 != rhs3 {
        return false;
    }

    true
}
