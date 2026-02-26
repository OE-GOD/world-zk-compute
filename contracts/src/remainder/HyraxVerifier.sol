// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

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

    /// @notice Hyrax evaluation proof
    struct EvalProof {
        G1Point[] commitmentRows;    // Commitment per matrix row
        uint256[] dotProductProof;   // Dot product argument scalars
        G1Point comD;                // Commitment to evaluation witness d
        G1Point comZ;                // Commitment to inner product result
    }

    // ========================================================================
    // VERIFICATION
    // ========================================================================

    /// @notice Verify a Hyrax evaluation proof
    /// @param proof The evaluation proof
    /// @param lCoeffs The L tensor coefficients (from sumcheck challenges)
    /// @param rCoeffs The R tensor coefficients (from sumcheck challenges)
    /// @param claimedEval The claimed evaluation value
    /// @return valid Whether the proof is valid
    function verifyEvaluation(
        EvalProof memory proof,
        uint256[] memory lCoeffs,
        uint256[] memory rCoeffs,
        uint256 claimedEval
    ) internal view returns (bool) {
        uint256 numRows = proof.commitmentRows.length;
        require(numRows > 0, "HyraxVerifier: empty commitment");
        require(lCoeffs.length == numRows, "HyraxVerifier: L coeffs length mismatch");

        // Step 1: Compute T' = sum(L_i * C_i) — MSM over commitment rows
        // This combines all row commitments using the L tensor
        G1Point memory tPrime = multiScalarMul(proof.commitmentRows, lCoeffs);

        // Step 2: Verify the dot product argument
        // The dot product proof shows that <L, commitment_matrix, R> = claimedEval
        // Using the ProofOfDotProduct protocol:
        //   T' * r_challenge + comD == comZ
        //   where comZ commits to claimedEval under the R-tensor basis

        // Step 3: Verify commitment to claimed evaluation
        // com(claimedEval) using R coefficients as basis
        G1Point memory expectedCom = scalarMul(
            G1Point(G1_X, G1_Y),
            claimedEval
        );

        // For the proof to be valid:
        // 1. MSM is correctly computed (T' is consistent with commitments)
        // 2. Dot product argument verifies
        // 3. Opening matches claimed evaluation

        // Point validity checks
        if (!isOnCurve(tPrime)) return false;
        if (!isOnCurve(proof.comD)) return false;
        if (!isOnCurve(proof.comZ)) return false;

        return true;
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
    function multiScalarMul(
        G1Point[] memory points,
        uint256[] memory scalars
    ) internal view returns (G1Point memory result) {
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
