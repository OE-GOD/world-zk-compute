// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import {PoseidonSponge} from "./PoseidonSponge.sol";
import {HyraxVerifier} from "./HyraxVerifier.sol";

/// @title CommittedSumcheckVerifier
/// @notice Verifies committed sumcheck proofs from Remainder's GKR protocol.
/// @dev In committed sumcheck, each round polynomial is committed as an EC point
///      (rather than revealed as scalar evaluations). Correctness is checked via
///      a Proof of Dot Product (PODP) against a computed j_star vector.
///
///      Verification flow (from proof_of_sumcheck.rs):
///      1. Squeeze rho (n+1) and gamma (n) RLC challenges
///      2. Compute alpha = MSM(messages, gammas)
///      3. Compute j_star vector from rhos, gammas, bindings, and degree
///      4. Compute dot_product = sum * rhos[0] + oracle_eval * (-rhos[n])
///      5. Verify PODP(alpha, dot_product, j_star)
library CommittedSumcheckVerifier {
    // ========================================================================
    // CONSTANTS
    // ========================================================================

    /// @notice BN254 scalar field modulus (Fr)
    uint256 internal constant FR_MODULUS =
        21888242871839275222246405745257275088548364400416034343698204186575808495617;

    // ========================================================================
    // TYPES
    // ========================================================================

    /// @notice Committed sumcheck proof (replaces SumcheckVerifier.SumcheckProof)
    struct CommittedSumcheckProof {
        HyraxVerifier.G1Point sum; // Commitment to the claimed sum
        HyraxVerifier.G1Point[] messages; // One EC point per round
        HyraxVerifier.PODPProof podp; // Dot product argument for final check
    }

    /// @notice Result of committed sumcheck verification
    struct CommittedSumcheckResult {
        bool valid;
        uint256[] bindings; // Fiat-Shamir challenges (one per round)
    }

    // ========================================================================
    // VERIFICATION
    // ========================================================================

    /// @notice Verify a committed sumcheck proof
    /// @param proof The committed sumcheck proof
    /// @param oracleEval EC point: evaluation of the PostSumcheckLayer
    /// @param degree Max polynomial degree per round (typically 2)
    /// @param bindings Sumcheck round challenges (already derived by caller during message absorption)
    /// @param gens Pedersen generators
    /// @param sponge Fiat-Shamir transcript (state after message absorption)
    /// @return valid Whether the proof verifies
    function verify(
        CommittedSumcheckProof memory proof,
        HyraxVerifier.G1Point memory oracleEval,
        uint256 degree,
        uint256[] memory bindings,
        HyraxVerifier.PedersenGens memory gens,
        PoseidonSponge.Sponge memory sponge
    ) internal view returns (bool valid) {
        uint256 n = proof.messages.length;
        require(n > 0, "CommittedSumcheck: no messages");
        require(bindings.length == n, "CommittedSumcheck: bindings length mismatch");

        // Step 1: Squeeze rho challenges (n+1 values for batching rows)
        uint256[] memory rhos = new uint256[](n + 1);
        for (uint256 i = 0; i <= n; i++) {
            rhos[i] = PoseidonSponge.squeeze(sponge) % FR_MODULUS;
        }

        // Step 2: Squeeze gamma challenges (n values for batching columns)
        uint256[] memory gammas = new uint256[](n);
        for (uint256 i = 0; i < n; i++) {
            gammas[i] = PoseidonSponge.squeeze(sponge) % FR_MODULUS;
        }

        // Step 3: Compute alpha = MSM(messages, gammas)
        HyraxVerifier.G1Point memory alpha = HyraxVerifier.multiScalarMul(proof.messages, gammas);

        // Step 4: Compute j_star vector
        uint256[] memory jStar = computeJStar(rhos, gammas, bindings, degree, n);

        // Step 5: Compute dot_product = sum * rhos[0] + oracle_eval * (-rhos[n])
        HyraxVerifier.G1Point memory dotProduct = computeDotProduct(proof.sum, oracleEval, rhos[0], rhos[n]);

        // Step 6: Verify PODP (alpha = com_x, dot_product = com_y, j_star = a_vector)
        // PODP transcript: absorb commitD, commitDDotA, squeeze challenge,
        // then absorb z_vector, z_delta, z_beta
        return verifyPODPWithTranscript(proof.podp, alpha, dotProduct, jStar, gens, sponge);
    }

    // ========================================================================
    // J_STAR COMPUTATION
    // ========================================================================

    /// @notice Compute the j_star vector for PODP verification
    /// @dev From proof_of_sumcheck.rs calculate_j_star():
    ///      j_star[i*(degree+1) + d] = gamma_inv[i] * (rhos[i] * coeff[d] - rhos[i+1] * bindings[i]^d)
    ///      where coeff[d] = 2 if d==0, else 1
    function computeJStar(
        uint256[] memory rhos,
        uint256[] memory gammas,
        uint256[] memory bindings,
        uint256 degree,
        uint256 n
    ) internal pure returns (uint256[] memory jStar) {
        uint256 degreePlusOne = degree + 1;
        jStar = new uint256[](degreePlusOne * n);

        for (uint256 i = 0; i < n; i++) {
            uint256 gammaInv = modInverse(gammas[i], FR_MODULUS);
            uint256 bindingPower = 1; // bindings[i]^d, starting at d=0

            for (uint256 d = 0; d < degreePlusOne; d++) {
                // coeff[d] = 2 if d==0, else 1
                uint256 coeff = (d == 0) ? 2 : 1;

                // rhos[i] * coeff - rhos[i+1] * bindings[i]^d
                uint256 term1 = mulmod(rhos[i], coeff, FR_MODULUS);
                uint256 term2 = mulmod(rhos[i + 1], bindingPower, FR_MODULUS);
                uint256 diff = addmod(term1, FR_MODULUS - term2, FR_MODULUS);

                jStar[i * degreePlusOne + d] = mulmod(gammaInv, diff, FR_MODULUS);

                // Update binding power: bindings[i]^(d+1)
                bindingPower = mulmod(bindingPower, bindings[i], FR_MODULUS);
            }
        }
    }

    // ========================================================================
    // DOT PRODUCT COMPUTATION
    // ========================================================================

    /// @notice Compute dot_product = sum * rhos[0] + oracle_eval * (-rhos[n])
    /// @dev From proof_of_sumcheck.rs: dot_product = oracle_eval * rhos[n] * (-1) + sum * rhos[0]
    function computeDotProduct(
        HyraxVerifier.G1Point memory sum,
        HyraxVerifier.G1Point memory oracleEval,
        uint256 rho0,
        uint256 rhoN
    ) internal view returns (HyraxVerifier.G1Point memory) {
        // sum * rhos[0]
        HyraxVerifier.G1Point memory term1 = HyraxVerifier.scalarMul(sum, rho0);

        // oracle_eval * (-rhos[n]) = oracle_eval * (FR_MODULUS - rhos[n])
        uint256 negRhoN = FR_MODULUS - rhoN;
        HyraxVerifier.G1Point memory term2 = HyraxVerifier.scalarMul(oracleEval, negRhoN);

        return HyraxVerifier.ecAdd(term1, term2);
    }

    // ========================================================================
    // PODP WITH TRANSCRIPT
    // ========================================================================

    /// @notice Verify PODP with full transcript operations
    /// @dev Absorbs commitD, commitDDotA, squeezes challenge, then absorbs
    ///      z_vector, z_delta, z_beta for downstream transcript consistency.
    function verifyPODPWithTranscript(
        HyraxVerifier.PODPProof memory podp,
        HyraxVerifier.G1Point memory comX,
        HyraxVerifier.G1Point memory comY,
        uint256[] memory aVector,
        HyraxVerifier.PedersenGens memory gens,
        PoseidonSponge.Sponge memory sponge
    ) internal view returns (bool) {
        // Absorb commit_d and commit_d_dot_a
        PoseidonSponge.absorb(sponge, podp.commitD.x);
        PoseidonSponge.absorb(sponge, podp.commitD.y);
        PoseidonSponge.absorb(sponge, podp.commitDDotA.x);
        PoseidonSponge.absorb(sponge, podp.commitDDotA.y);

        // Squeeze challenge
        uint256 challenge = PoseidonSponge.squeeze(sponge) % FR_MODULUS;

        // Absorb z_vector elements
        for (uint256 i = 0; i < podp.zVector.length; i++) {
            PoseidonSponge.absorb(sponge, podp.zVector[i]);
        }

        // Absorb z_delta, z_beta
        PoseidonSponge.absorb(sponge, podp.zDelta);
        PoseidonSponge.absorb(sponge, podp.zBeta);

        // Verify the two PODP equations
        return HyraxVerifier.verifyPODP(podp, challenge, comX, comY, aVector, gens);
    }

    // ========================================================================
    // MATH HELPERS
    // ========================================================================

    /// @notice Modular inverse via Fermat's little theorem: a^(p-2) mod p
    function modInverse(uint256 a, uint256 p) internal pure returns (uint256) {
        require(a != 0, "CommittedSumcheck: inverse of zero");
        return modExp(a, p - 2, p);
    }

    /// @notice Modular exponentiation via square-and-multiply
    function modExp(uint256 base, uint256 exp, uint256 mod) internal pure returns (uint256 result) {
        result = 1;
        base = base % mod;
        while (exp > 0) {
            if (exp & 1 == 1) {
                result = mulmod(result, base, mod);
            }
            exp >>= 1;
            base = mulmod(base, base, mod);
        }
    }
}
