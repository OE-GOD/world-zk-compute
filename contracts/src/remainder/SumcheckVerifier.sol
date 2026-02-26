// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import {PoseidonSponge} from "./PoseidonSponge.sol";

/// @title SumcheckVerifier
/// @notice Verifies sumcheck protocol rounds for GKR layer proofs
/// @dev Each sumcheck round provides a univariate polynomial g_i(X) of degree d.
///      The verifier checks:
///        g_i(0) + g_i(1) == claimed_sum
///      Then derives challenge r_i via Fiat-Shamir and computes:
///        next_claimed_sum = g_i(r_i)
///
///      For GKR, the polynomial degree is typically 2 (quadratic gate).
library SumcheckVerifier {
    // ========================================================================
    // CONSTANTS
    // ========================================================================

    /// @notice BN254 scalar field modulus (Fr)
    uint256 internal constant FR_MODULUS =
        21888242871839275222246405745257275088548364400416034343698204186575808495617;

    // ========================================================================
    // TYPES
    // ========================================================================

    /// @notice A single sumcheck round polynomial (degree d)
    /// @dev Represented by d+1 evaluations at points 0, 1, ..., d
    struct RoundPoly {
        uint256[] evals; // evals[j] = g_i(j) for j = 0..d
    }

    /// @notice Complete sumcheck proof for one GKR layer
    struct SumcheckProof {
        RoundPoly[] rounds;     // One polynomial per variable
        uint256 finalEval;      // Final evaluation claim
    }

    /// @notice Result of sumcheck verification
    struct SumcheckResult {
        bool valid;
        uint256[] challenges;   // Fiat-Shamir challenges (one per round)
        uint256 finalClaim;     // The remaining claim after all rounds
    }

    // ========================================================================
    // VERIFICATION
    // ========================================================================

    /// @notice Verify a sumcheck proof
    /// @param proof The sumcheck proof to verify
    /// @param claimedSum The initial claimed sum (from GKR)
    /// @param sponge The Fiat-Shamir transcript sponge
    /// @return result The verification result with challenges
    function verify(
        SumcheckProof memory proof,
        uint256 claimedSum,
        PoseidonSponge.Sponge memory sponge
    ) internal pure returns (SumcheckResult memory result) {
        uint256 numRounds = proof.rounds.length;
        result.challenges = new uint256[](numRounds);
        result.valid = true;

        uint256 currentClaim = claimedSum;

        for (uint256 i = 0; i < numRounds; i++) {
            RoundPoly memory round = proof.rounds[i];

            // Check: g_i(0) + g_i(1) == currentClaim
            uint256 sum = addmod(round.evals[0], round.evals[1], FR_MODULUS);
            if (sum != currentClaim) {
                result.valid = false;
                return result;
            }

            // Absorb round polynomial into transcript for Fiat-Shamir
            for (uint256 j = 0; j < round.evals.length; j++) {
                PoseidonSponge.absorb(sponge, round.evals[j]);
            }

            // Squeeze challenge r_i
            uint256 challenge = PoseidonSponge.squeeze(sponge) % FR_MODULUS;
            result.challenges[i] = challenge;

            // Evaluate g_i(r_i) to get next claimed sum
            currentClaim = evaluatePolynomial(round.evals, challenge);
        }

        // The final claim should match proof.finalEval
        if (currentClaim != proof.finalEval) {
            result.valid = false;
            return result;
        }

        result.finalClaim = currentClaim;
    }

    /// @notice Evaluate a polynomial given by evaluations at 0, 1, ..., d
    ///         at an arbitrary point `x` using Lagrange interpolation
    /// @param evals Polynomial evaluations at points 0, 1, ..., d
    /// @param x The point to evaluate at
    /// @return The polynomial value at x
    function evaluatePolynomial(
        uint256[] memory evals,
        uint256 x
    ) internal pure returns (uint256) {
        uint256 d = evals.length;

        if (d == 1) return evals[0];

        // Lagrange interpolation over evaluation points {0, 1, ..., d-1}
        //
        // P(x) = sum_{i=0}^{d-1} evals[i] * prod_{j!=i} (x - j) / (i - j)
        //
        // For small d (typically 3 for degree-2 polynomials in GKR),
        // we compute this directly.

        if (d == 2) {
            // Linear: P(x) = evals[0] * (1 - x) + evals[1] * x
            // P(x) = evals[0] + x * (evals[1] - evals[0])
            uint256 diff;
            if (evals[1] >= evals[0]) {
                diff = evals[1] - evals[0];
                return addmod(evals[0], mulmod(x, diff, FR_MODULUS), FR_MODULUS);
            } else {
                diff = evals[0] - evals[1];
                uint256 sub = mulmod(x, diff, FR_MODULUS);
                return addmod(evals[0], FR_MODULUS - sub, FR_MODULUS);
            }
        }

        if (d == 3) {
            // Quadratic Lagrange interpolation at {0, 1, 2}
            // L_0(x) = (x-1)(x-2) / (0-1)(0-2) = (x-1)(x-2) / 2
            // L_1(x) = (x-0)(x-2) / (1-0)(1-2) = x(x-2) / (-1) = -x(x-2)
            // L_2(x) = (x-0)(x-1) / (2-0)(2-1) = x(x-1) / 2

            uint256 inv2 = modInverse(2, FR_MODULUS);

            // L_0(x) = (x-1)(x-2) * inv2
            uint256 xm1 = addmod(x, FR_MODULUS - 1, FR_MODULUS);
            uint256 xm2 = addmod(x, FR_MODULUS - 2, FR_MODULUS);
            uint256 l0 = mulmod(mulmod(xm1, xm2, FR_MODULUS), inv2, FR_MODULUS);

            // L_1(x) = -x(x-2) = (FR_MODULUS - x)(x-2) is wrong
            // L_1(x) = x * (x-2) / (1-0)(1-2) = x*(x-2)/(-1) = -(x*(x-2))
            uint256 l1_num = mulmod(x, xm2, FR_MODULUS);
            uint256 l1 = FR_MODULUS - l1_num; // Negate

            // L_2(x) = x(x-1) * inv2
            uint256 l2 = mulmod(mulmod(x, xm1, FR_MODULUS), inv2, FR_MODULUS);

            // P(x) = evals[0]*L_0 + evals[1]*L_1 + evals[2]*L_2
            uint256 quadResult = mulmod(evals[0], l0, FR_MODULUS);
            quadResult = addmod(quadResult, mulmod(evals[1], l1, FR_MODULUS), FR_MODULUS);
            quadResult = addmod(quadResult, mulmod(evals[2], l2, FR_MODULUS), FR_MODULUS);

            return quadResult;
        }

        // General case: Lagrange interpolation for degree d-1
        uint256 result = 0;
        for (uint256 i = 0; i < d; i++) {
            uint256 basis = evals[i];
            for (uint256 j = 0; j < d; j++) {
                if (j == i) continue;

                // Numerator: (x - j)
                uint256 num = addmod(x, FR_MODULUS - j, FR_MODULUS);
                // Denominator: (i - j)
                uint256 denom;
                if (i > j) {
                    denom = i - j;
                } else {
                    denom = FR_MODULUS - (j - i);
                }
                uint256 denomInv = modInverse(denom, FR_MODULUS);

                basis = mulmod(basis, mulmod(num, denomInv, FR_MODULUS), FR_MODULUS);
            }
            result = addmod(result, basis, FR_MODULUS);
        }

        return result;
    }

    /// @notice Compute modular inverse using Fermat's little theorem
    /// @dev For prime p: a^(-1) = a^(p-2) mod p
    function modInverse(uint256 a, uint256 p) internal pure returns (uint256) {
        return modExp(a, p - 2, p);
    }

    /// @notice Modular exponentiation using binary method
    function modExp(uint256 base, uint256 exp, uint256 mod_) internal pure returns (uint256) {
        uint256 result = 1;
        base = base % mod_;
        while (exp > 0) {
            if (exp & 1 == 1) {
                result = mulmod(result, base, mod_);
            }
            exp >>= 1;
            base = mulmod(base, base, mod_);
        }
        return result;
    }
}
