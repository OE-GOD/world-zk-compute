// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import {PoseidonSponge} from "./PoseidonSponge.sol";

/// @title HyraxVerifier
/// @notice Verifies Hyrax polynomial commitment scheme (PCS) evaluation proofs
/// @dev Hyrax uses a matrix structure for Pedersen commitments:
///      - Commitment rows: C_i = <v_i, G> (Pedersen commitment of vector chunk)
///      - Evaluation proof: dot product argument on committed vectors
///
///      Uses BN254 curve with ecAdd (0x06) and ecMul (0x07) precompiles.
///
///      The Hyrax PCS organizes a vector of length N into a sqrt(N) x sqrt(N) matrix.
///      Each row is committed independently, and evaluation proofs use a
///      dot product argument across the L tensor coefficients.
library HyraxVerifier {
    // ========================================================================
    // CONSTANTS
    // ========================================================================

    /// @notice BN254 scalar field modulus (Fr)
    uint256 internal constant FR_MODULUS =
        21888242871839275222246405745257275088548364400416034343698204186575808495617;

    /// @notice BN254 base field modulus (Fq) — for point validation
    uint256 internal constant FQ_MODULUS =
        21888242871839275222246405745257275088696311157297823662689037894645226208583;

    /// @notice BN254 G1 generator point
    uint256 internal constant G1_X = 1;
    uint256 internal constant G1_Y = 2;

    // ========================================================================
    // TYPES
    // ========================================================================

    /// @notice A G1 affine point on BN254
    struct G1Point {
        uint256 x;
        uint256 y;
    }

    /// @notice PODP (ProofOfDotProduct) — Sigma protocol proof
    struct PODPProof {
        G1Point commitD; // Commitment to random masking vector d
        G1Point commitDDotA; // Commitment to <d, a>
        uint256[] zVector; // Blinded vector z = c*x + d
        uint256 zDelta; // Blinding factor for com(z)
        uint256 zBeta; // Blinding factor for com(<z,a>)
    }

    /// @notice Pedersen generator set for PODP verification
    struct PedersenGens {
        G1Point[] messageGens; // g_1..g_m (vector Pedersen bases)
        G1Point scalarGen; // g_scalar (for scalar_commit, = last generator)
        G1Point blindingGen; // h (blinding generator)
    }

    /// @notice ProofOfProduct — Sigma protocol proof that x*y=z on committed values
    struct ProofOfProduct {
        G1Point alpha;
        G1Point beta;
        G1Point delta;
        uint256 z1;
        uint256 z2;
        uint256 z3;
        uint256 z4;
        uint256 z5;
    }

    /// @notice Hyrax evaluation proof
    struct EvalProof {
        G1Point[] commitmentRows; // Commitment per matrix row
        PODPProof podp; // PODP proof for dot product argument
        G1Point comEval; // Commitment to evaluation (commitmentToEvaluation)
    }

    // ========================================================================
    // VERIFICATION
    // ========================================================================

    /// @notice Verify a Hyrax evaluation proof with full PODP verification
    /// @param proof The evaluation proof
    /// @param lCoeffs The L tensor coefficients (from sumcheck challenges)
    /// @param rCoeffs The R tensor coefficients (from sumcheck challenges)
    /// @param claimedEval The claimed evaluation value (unused in PODP, kept for API)
    /// @param podpChallenge The Fiat-Shamir challenge for the PODP protocol
    /// @param gens Pedersen generators for commitment verification
    /// @return valid Whether the proof is valid
    function verifyEvaluation(
        EvalProof memory proof,
        uint256[] memory lCoeffs,
        uint256[] memory rCoeffs,
        uint256 claimedEval,
        uint256 podpChallenge,
        PedersenGens memory gens
    ) internal view returns (bool) {
        uint256 numRows = proof.commitmentRows.length;
        require(numRows > 0, "HyraxVerifier: empty commitment");
        require(lCoeffs.length == numRows, "HyraxVerifier: L coeffs length mismatch");
        require(proof.podp.zVector.length == rCoeffs.length, "HyraxVerifier: z_vector length != R coeffs length");

        // Step 1: Compute T' = sum(L_i * C_i) — MSM over commitment rows
        // This is com_x: the commitment to the L-tensor-compressed column vector
        G1Point memory comX = multiScalarMul(proof.commitmentRows, lCoeffs);

        // Step 2: com_y is the commitmentToEvaluation from the proof
        G1Point memory comY = proof.comEval;

        // Step 3: Verify the PODP (a_vector = R tensor coefficients)
        return verifyPODP(proof.podp, podpChallenge, comX, comY, rCoeffs, gens);
    }

    /// @notice Verify a PODP (ProofOfDotProduct) Sigma protocol proof
    /// @dev Checks two EC-point equations:
    ///      Check 1: c * com_x + commit_d       == com_z       (vector commitment consistency)
    ///      Check 2: c * com_y + commit_d_dot_a  == com_z_dot_a (dot product consistency)
    ///
    ///      Where:
    ///        com_z       = MSM(g_1..g_m, z_vector) + z_delta * h
    ///        com_z_dot_a = <z_vector, a_vector> * g_scalar + z_beta * h
    ///
    /// @param podp The PODP proof
    /// @param challenge The Fiat-Shamir challenge c
    /// @param comX Commitment to the private vector x
    /// @param comY Commitment to <x, a> (the dot product result)
    /// @param aVector The public vector a (R tensor coefficients)
    /// @param gens Pedersen generators
    /// @return valid Whether the PODP proof verifies
    function verifyPODP(
        PODPProof memory podp,
        uint256 challenge,
        G1Point memory comX,
        G1Point memory comY,
        uint256[] memory aVector,
        PedersenGens memory gens
    ) internal view returns (bool) {
        require(podp.zVector.length == aVector.length, "HyraxVerifier: PODP vector length mismatch");
        require(podp.zVector.length == gens.messageGens.length, "HyraxVerifier: PODP generators length mismatch");

        // 1. Compute z_dot_a = <z_vector, a_vector> (mod Fr)
        uint256 zDotA = innerProduct(podp.zVector, aVector);

        // 2. Compute com_z = MSM(g_1..g_m, z_vector) + z_delta * h
        G1Point memory comZ = multiScalarMul(gens.messageGens, podp.zVector);
        G1Point memory zDeltaH = scalarMul(gens.blindingGen, podp.zDelta);
        comZ = ecAdd(comZ, zDeltaH);

        // 3. Compute com_z_dot_a = z_dot_a * g_scalar + z_beta * h
        G1Point memory comZDotA = scalarMul(gens.scalarGen, zDotA);
        G1Point memory zBetaH = scalarMul(gens.blindingGen, podp.zBeta);
        comZDotA = ecAdd(comZDotA, zBetaH);

        // 4. Check 1: c * com_x + commit_d == com_z
        G1Point memory lhs1 = ecAdd(scalarMul(comX, challenge), podp.commitD);
        if (!isEqual(lhs1, comZ)) return false;

        // 5. Check 2: c * com_y + commit_d_dot_a == com_z_dot_a
        G1Point memory lhs2 = ecAdd(scalarMul(comY, challenge), podp.commitDDotA);
        if (!isEqual(lhs2, comZDotA)) return false;

        return true;
    }

    /// @notice Compute inner product <a, b> mod Fr
    function innerProduct(uint256[] memory a, uint256[] memory b) internal pure returns (uint256 result) {
        require(a.length == b.length, "HyraxVerifier: inner product length mismatch");
        result = 0;
        for (uint256 i = 0; i < a.length; i++) {
            result = addmod(result, mulmod(a[i], b[i], FR_MODULUS), FR_MODULUS);
        }
    }

    // ========================================================================
    // PROOF OF PRODUCT
    // ========================================================================

    /// @notice Verify a ProofOfProduct: proves x*y=z on committed values
    /// @dev Transcript operations (from proof_of_product.rs):
    ///      1. Absorb alpha, beta, delta (6 field elements)
    ///      2. Squeeze challenge c
    ///      3. Absorb z1..z5
    ///
    ///      EC checks (3 equations):
    ///        alpha + c * com_x == z1 * g + z2 * h
    ///        beta  + c * com_y == z3 * g + z4 * h
    ///        delta + c * com_z == z3 * com_x + z5 * h
    ///
    /// @param pop The ProofOfProduct
    /// @param comX Commitment to x
    /// @param comY Commitment to y
    /// @param comZ Commitment to z (= x*y)
    /// @param gens Pedersen generators
    /// @param sponge Fiat-Shamir transcript (modified in-place)
    /// @return valid Whether the proof verifies
    function verifyProofOfProduct(
        ProofOfProduct memory pop,
        G1Point memory comX,
        G1Point memory comY,
        G1Point memory comZ,
        PedersenGens memory gens,
        PoseidonSponge.Sponge memory sponge
    ) internal view returns (bool) {
        uint256 c = _popAbsorbAndSqueeze(pop, sponge);
        return _popCheckEquations(pop, comX, comY, comZ, gens, c);
    }

    /// @notice Absorb PoP elements into transcript and squeeze challenge
    function _popAbsorbAndSqueeze(ProofOfProduct memory pop, PoseidonSponge.Sponge memory sponge)
        private
        pure
        returns (uint256 c)
    {
        // Absorb alpha, beta, delta
        PoseidonSponge.absorb(sponge, pop.alpha.x);
        PoseidonSponge.absorb(sponge, pop.alpha.y);
        PoseidonSponge.absorb(sponge, pop.beta.x);
        PoseidonSponge.absorb(sponge, pop.beta.y);
        PoseidonSponge.absorb(sponge, pop.delta.x);
        PoseidonSponge.absorb(sponge, pop.delta.y);

        // Squeeze challenge
        c = PoseidonSponge.squeeze(sponge) % FR_MODULUS;

        // Absorb z1..z5
        PoseidonSponge.absorb(sponge, pop.z1);
        PoseidonSponge.absorb(sponge, pop.z2);
        PoseidonSponge.absorb(sponge, pop.z3);
        PoseidonSponge.absorb(sponge, pop.z4);
        PoseidonSponge.absorb(sponge, pop.z5);
    }

    /// @notice Check the three PoP EC equations
    function _popCheckEquations(
        ProofOfProduct memory pop,
        G1Point memory comX,
        G1Point memory comY,
        G1Point memory comZ,
        PedersenGens memory gens,
        uint256 c
    ) private view returns (bool) {
        // Check 1: alpha + c * com_x == z1 * g + z2 * h
        if (!_popCheck1(pop, comX, gens, c)) return false;

        // Check 2: beta + c * com_y == z3 * g + z4 * h
        if (!_popCheck2(pop, comY, gens, c)) return false;

        // Check 3: delta + c * com_z == z3 * com_x + z5 * h
        G1Point memory lhs3 = ecAdd(pop.delta, scalarMul(comZ, c));
        G1Point memory rhs3 = ecAdd(scalarMul(comX, pop.z3), scalarMul(gens.blindingGen, pop.z5));
        if (!isEqual(lhs3, rhs3)) return false;

        return true;
    }

    /// @notice PoP check 1: alpha + c * com_x == z1 * g + z2 * h
    function _popCheck1(ProofOfProduct memory pop, G1Point memory comX, PedersenGens memory gens, uint256 c)
        private
        view
        returns (bool)
    {
        G1Point memory lhs = ecAdd(pop.alpha, scalarMul(comX, c));
        G1Point memory rhs = ecAdd(scalarMul(gens.scalarGen, pop.z1), scalarMul(gens.blindingGen, pop.z2));
        return isEqual(lhs, rhs);
    }

    /// @notice PoP check 2: beta + c * com_y == z3 * g + z4 * h
    function _popCheck2(ProofOfProduct memory pop, G1Point memory comY, PedersenGens memory gens, uint256 c)
        private
        view
        returns (bool)
    {
        G1Point memory lhs = ecAdd(pop.beta, scalarMul(comY, c));
        G1Point memory rhs = ecAdd(scalarMul(gens.scalarGen, pop.z3), scalarMul(gens.blindingGen, pop.z4));
        return isEqual(lhs, rhs);
    }

    // ========================================================================
    // BN254 EC OPERATIONS (using precompiles)
    // ========================================================================

    /// @notice EC point addition using precompile at 0x06
    function ecAdd(G1Point memory p1, G1Point memory p2) internal view returns (G1Point memory r) {
        uint256[4] memory input;
        input[0] = p1.x;
        input[1] = p1.y;
        input[2] = p2.x;
        input[3] = p2.y;

        uint256[2] memory output;
        bool success;

        assembly {
            success := staticcall(gas(), 0x06, input, 0x80, output, 0x40)
        }

        require(success, "HyraxVerifier: ecAdd failed");
        r.x = output[0];
        r.y = output[1];
    }

    /// @notice EC scalar multiplication using precompile at 0x07
    function scalarMul(G1Point memory p, uint256 s) internal view returns (G1Point memory r) {
        uint256[3] memory input;
        input[0] = p.x;
        input[1] = p.y;
        input[2] = s;

        uint256[2] memory output;
        bool success;

        assembly {
            success := staticcall(gas(), 0x07, input, 0x60, output, 0x40)
        }

        require(success, "HyraxVerifier: ecMul failed");
        r.x = output[0];
        r.y = output[1];
    }

    /// @notice Multi-scalar multiplication: sum(s_i * P_i)
    /// @dev Naive implementation: iterates over each (scalar, point) pair.
    ///      For production, consider Pippenger's algorithm or batched precompile calls.
    function multiScalarMul(G1Point[] memory points, uint256[] memory scalars)
        internal
        view
        returns (G1Point memory result)
    {
        require(points.length == scalars.length, "HyraxVerifier: MSM length mismatch");
        require(points.length > 0, "HyraxVerifier: empty MSM");

        // Start with first term
        result = scalarMul(points[0], scalars[0]);

        // Add remaining terms
        for (uint256 i = 1; i < points.length; i++) {
            G1Point memory term = scalarMul(points[i], scalars[i]);
            result = ecAdd(result, term);
        }
    }

    /// @notice Check if a point is on the BN254 curve (y^2 = x^3 + 3)
    function isOnCurve(G1Point memory p) internal pure returns (bool) {
        if (p.x == 0 && p.y == 0) return true; // Point at infinity

        if (p.x >= FQ_MODULUS || p.y >= FQ_MODULUS) return false;

        uint256 lhs = mulmod(p.y, p.y, FQ_MODULUS);
        uint256 x3 = mulmod(mulmod(p.x, p.x, FQ_MODULUS), p.x, FQ_MODULUS);
        uint256 rhs = addmod(x3, 3, FQ_MODULUS);

        return lhs == rhs;
    }

    /// @notice Negate a G1 point (reflect over x-axis)
    function negate(G1Point memory p) internal pure returns (G1Point memory) {
        if (p.x == 0 && p.y == 0) return p;
        return G1Point(p.x, FQ_MODULUS - p.y);
    }

    /// @notice Check if two G1 points are equal
    function isEqual(G1Point memory p1, G1Point memory p2) internal pure returns (bool) {
        return p1.x == p2.x && p1.y == p2.y;
    }
}
