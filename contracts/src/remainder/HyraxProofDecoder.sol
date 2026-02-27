// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import {HyraxVerifier} from "./HyraxVerifier.sol";

/// @title HyraxProofDecoder
/// @notice Decodes ABI-encoded HyraxProof bytes from the Rust abi_encode module.
/// @dev Uses a flat offset-based approach to avoid stack-too-deep issues.
///      The proof format is a sequence of big-endian uint256 values.
library HyraxProofDecoder {
    // ============================
    // TYPES (minimal for testing)
    // ============================

    /// @notice Summary of decoded proof structure (lightweight)
    struct ProofSummary {
        bytes32 circuitHash;
        uint256 numPublicInputs;
        uint256 numOutputProofs;
        uint256 numLayerProofs;
        uint256 numFsClaims;
        uint256 numPubClaims;
        uint256 numInputProofs;
        uint256 totalBytes;
    }

    /// @notice A decoded G1 point with its source offset for debugging
    struct DecodedPoint {
        uint256 x;
        uint256 y;
    }

    // ============================
    // SUMMARY DECODE
    // ============================

    /// @notice Decode proof summary (counts only) — cheap, no deep structs
    function decodeSummary(bytes calldata data) internal pure returns (ProofSummary memory s) {
        uint256 offset = 0;

        // Circuit hash
        s.circuitHash = bytes32(data[offset:offset + 32]);
        offset += 32;

        // Public inputs
        s.numPublicInputs = _readU256(data, offset);
        offset += 32;
        for (uint256 i = 0; i < s.numPublicInputs; i++) {
            uint256 numValues = _readU256(data, offset);
            offset += 32;
            offset += numValues * 32; // Skip Fr values
        }

        // Output proofs
        s.numOutputProofs = _readU256(data, offset);
        offset += 32;
        offset += s.numOutputProofs * 64; // Skip G1 points

        // Layer proofs
        s.numLayerProofs = _readU256(data, offset);
        offset += 32;
        for (uint256 i = 0; i < s.numLayerProofs; i++) {
            offset = _skipLayerProof(data, offset);
        }

        // Fiat-Shamir claims
        s.numFsClaims = _readU256(data, offset);
        offset += 32;
        for (uint256 i = 0; i < s.numFsClaims; i++) {
            offset = _skipClaim(data, offset);
        }

        // Claims on public values
        s.numPubClaims = _readU256(data, offset);
        offset += 32;
        for (uint256 i = 0; i < s.numPubClaims; i++) {
            offset = _skipClaim(data, offset);
        }

        // Input proofs
        s.numInputProofs = _readU256(data, offset);
        offset += 32;
        for (uint256 i = 0; i < s.numInputProofs; i++) {
            offset = _skipInputProof(data, offset);
        }

        s.totalBytes = offset;
    }

    /// @notice Read public input values
    function decodePublicInputs(bytes calldata data) internal pure returns (uint256[] memory values) {
        uint256 offset = 32; // Skip circuit hash
        uint256 numPubInputs = _readU256(data, offset);
        offset += 32;

        // For simplicity, return first public input's values
        if (numPubInputs == 0) return new uint256[](0);

        uint256 numValues = _readU256(data, offset);
        offset += 32;
        values = new uint256[](numValues);
        for (uint256 i = 0; i < numValues; i++) {
            values[i] = _readU256(data, offset);
            offset += 32;
        }
    }

    /// @notice Decode input commitment points (from hyrax_input_proofs section)
    /// @dev Returns affine (x,y) coordinate pairs for all commitment rows across all input proofs.
    ///      These are the EC points absorbed into the Fiat-Shamir transcript.
    function decodeInputCommitmentPoints(bytes calldata data)
        internal
        pure
        returns (DecodedPoint[] memory points)
    {
        // Navigate to input proofs section
        uint256 offset = _offsetToInputProofs(data);

        // First pass: count total commitment points
        uint256 startOffset = offset;
        uint256 numInputProofs = _readU256(data, offset);
        offset += 32;
        uint256 totalPoints = 0;
        for (uint256 i = 0; i < numInputProofs; i++) {
            uint256 nc = _readU256(data, offset);
            offset += 32;
            totalPoints += nc;
            offset += nc * 64;
            offset = _skipInputProofEvals(data, offset);
        }

        // Second pass: extract the points
        points = new DecodedPoint[](totalPoints);
        offset = startOffset + 32;
        uint256 idx = 0;
        for (uint256 i = 0; i < numInputProofs; i++) {
            uint256 nc = _readU256(data, offset);
            offset += 32;
            for (uint256 j = 0; j < nc; j++) {
                points[idx].x = _readU256(data, offset);
                points[idx].y = _readU256(data, offset + 32);
                offset += 64;
                idx++;
            }
            offset = _skipInputProofEvals(data, offset);
        }
    }

    /// @dev Navigate past all sections to reach the input proofs section
    function _offsetToInputProofs(bytes calldata data) private pure returns (uint256 offset) {
        offset = 32; // skip circuit hash

        // Skip public inputs
        uint256 n = _readU256(data, offset);
        offset += 32;
        for (uint256 i = 0; i < n; i++) {
            uint256 cnt = _readU256(data, offset);
            offset += 32 + cnt * 32;
        }

        // Skip output proofs
        n = _readU256(data, offset);
        offset += 32 + n * 64;

        // Skip layer proofs
        n = _readU256(data, offset);
        offset += 32;
        for (uint256 i = 0; i < n; i++) {
            offset = _skipLayerProof(data, offset);
        }

        // Skip FS claims
        n = _readU256(data, offset);
        offset += 32;
        for (uint256 i = 0; i < n; i++) {
            offset = _skipClaim(data, offset);
        }

        // Skip public value claims
        n = _readU256(data, offset);
        offset += 32;
        for (uint256 i = 0; i < n; i++) {
            offset = _skipClaim(data, offset);
        }
    }

    /// @dev Skip evaluation proofs within an input proof (PODP + commitmentToEvaluation)
    function _skipInputProofEvals(bytes calldata data, uint256 offset) private pure returns (uint256) {
        uint256 numEvals = _readU256(data, offset);
        offset += 32;
        for (uint256 i = 0; i < numEvals; i++) {
            offset = _skipPODP(data, offset);
            offset += 64; // commitmentToEvaluation
        }
        return offset;
    }

    /// @notice Extract all G1 points from the proof and check they're on-curve
    /// @return numPoints Total number of G1 points found
    /// @return allOnCurve Whether all points are on the BN254 curve
    function validateAllPoints(bytes calldata data) internal pure returns (uint256 numPoints, bool allOnCurve) {
        allOnCurve = true;
        numPoints = 0;
        uint256 offset = 0;

        // Skip circuit hash
        offset += 32;

        // Public inputs (no points here, just Fr values)
        uint256 numPubInputs = _readU256(data, offset);
        offset += 32;
        for (uint256 i = 0; i < numPubInputs; i++) {
            uint256 n = _readU256(data, offset);
            offset += 32;
            offset += n * 32;
        }

        // Output proofs (each is a G1 point)
        uint256 numOutputProofs = _readU256(data, offset);
        offset += 32;
        for (uint256 i = 0; i < numOutputProofs; i++) {
            if (!_checkPointOnCurve(data, offset)) allOnCurve = false;
            offset += 64;
            numPoints++;
        }

        // Layer proofs
        uint256 numLayerProofs = _readU256(data, offset);
        offset += 32;
        for (uint256 i = 0; i < numLayerProofs; i++) {
            (uint256 pts, bool ok, uint256 newOff) = _validateLayerProofPoints(data, offset);
            numPoints += pts;
            if (!ok) allOnCurve = false;
            offset = newOff;
        }

        // Fiat-Shamir claims
        uint256 numFsClaims = _readU256(data, offset);
        offset += 32;
        for (uint256 i = 0; i < numFsClaims; i++) {
            (uint256 pts, bool ok, uint256 newOff) = _validateClaimPoints(data, offset);
            numPoints += pts;
            if (!ok) allOnCurve = false;
            offset = newOff;
        }

        // Claims on public values
        uint256 numPubClaims = _readU256(data, offset);
        offset += 32;
        for (uint256 i = 0; i < numPubClaims; i++) {
            (uint256 pts, bool ok, uint256 newOff) = _validateClaimPoints(data, offset);
            numPoints += pts;
            if (!ok) allOnCurve = false;
            offset = newOff;
        }

        // Input proofs
        uint256 numInputProofs = _readU256(data, offset);
        offset += 32;
        for (uint256 i = 0; i < numInputProofs; i++) {
            (uint256 pts, bool ok, uint256 newOff) = _validateInputProofPoints(data, offset);
            numPoints += pts;
            if (!ok) allOnCurve = false;
            offset = newOff;
        }
    }

    // ============================
    // SKIP HELPERS (advance offset past each section)
    // ============================

    function _skipLayerProof(bytes calldata data, uint256 offset) internal pure returns (uint256) {
        // Sumcheck: sum (G1) + messages + podp
        offset += 64; // sum point
        uint256 numMsg = _readU256(data, offset);
        offset += 32;
        offset += numMsg * 64; // message points

        // PODP
        offset = _skipPODP(data, offset);

        // Commitments
        uint256 numCommits = _readU256(data, offset);
        offset += 32;
        offset += numCommits * 64;

        // ProofOfProduct
        uint256 numPops = _readU256(data, offset);
        offset += 32;
        offset += numPops * (64 * 3 + 32 * 5); // 3 points + 5 scalars

        // Claim agg
        uint256 hasAgg = _readU256(data, offset);
        offset += 32;
        if (hasAgg == 1) {
            uint256 numCoeffs = _readU256(data, offset);
            offset += 32;
            offset += numCoeffs * 64;

            uint256 numOpenings = _readU256(data, offset);
            offset += 32;
            offset += numOpenings * (32 * 2 + 64); // 2 scalars + 1 point

            uint256 numEq = _readU256(data, offset);
            offset += 32;
            offset += numEq * (64 + 32); // 1 point + 1 scalar
        }

        return offset;
    }

    function _skipClaim(bytes calldata data, uint256 offset) internal pure returns (uint256) {
        offset += 32; // value
        offset += 32; // blinding
        offset += 64; // commitment point
        uint256 numPoint = _readU256(data, offset);
        offset += 32;
        offset += numPoint * 32; // point Fr values
        return offset;
    }

    function _skipPODP(bytes calldata data, uint256 offset) internal pure returns (uint256) {
        offset += 64; // commitD
        offset += 64; // commitDDotA
        uint256 numZ = _readU256(data, offset);
        offset += 32;
        offset += numZ * 32; // z_vector
        offset += 32; // z_delta
        offset += 32; // z_beta
        return offset;
    }

    function _skipInputProof(bytes calldata data, uint256 offset) internal pure returns (uint256) {
        uint256 numCommits = _readU256(data, offset);
        offset += 32;
        offset += numCommits * 64;

        uint256 numEvals = _readU256(data, offset);
        offset += 32;
        for (uint256 i = 0; i < numEvals; i++) {
            offset = _skipPODP(data, offset);
            offset += 64; // commitmentToEvaluation
        }
        return offset;
    }

    // ============================
    // POINT VALIDATION HELPERS
    // ============================

    function _validateLayerProofPoints(bytes calldata data, uint256 offset)
        internal
        pure
        returns (uint256 numPoints, bool allOnCurve, uint256 newOffset)
    {
        allOnCurve = true;

        // Sumcheck sum
        if (!_checkPointOnCurve(data, offset)) allOnCurve = false;
        offset += 64;
        numPoints++;

        // Messages
        uint256 numMsg = _readU256(data, offset);
        offset += 32;
        for (uint256 i = 0; i < numMsg; i++) {
            if (!_checkPointOnCurve(data, offset)) allOnCurve = false;
            offset += 64;
            numPoints++;
        }

        // PODP points
        if (!_checkPointOnCurve(data, offset)) allOnCurve = false;
        offset += 64;
        numPoints++; // commitD
        if (!_checkPointOnCurve(data, offset)) allOnCurve = false;
        offset += 64;
        numPoints++; // commitDDotA
        uint256 numZ = _readU256(data, offset);
        offset += 32;
        offset += numZ * 32 + 32 + 32; // z_vector + z_delta + z_beta

        // Commitments
        uint256 numCommits = _readU256(data, offset);
        offset += 32;
        for (uint256 i = 0; i < numCommits; i++) {
            if (!_checkPointOnCurve(data, offset)) allOnCurve = false;
            offset += 64;
            numPoints++;
        }

        // ProofOfProduct
        uint256 numPops = _readU256(data, offset);
        offset += 32;
        for (uint256 i = 0; i < numPops; i++) {
            // 3 points: alpha, beta, delta
            for (uint256 j = 0; j < 3; j++) {
                if (!_checkPointOnCurve(data, offset)) allOnCurve = false;
                offset += 64;
                numPoints++;
            }
            offset += 32 * 5; // 5 scalars
        }

        // Claim agg
        uint256 hasAgg = _readU256(data, offset);
        offset += 32;
        if (hasAgg == 1) {
            uint256 numCoeffs = _readU256(data, offset);
            offset += 32;
            for (uint256 i = 0; i < numCoeffs; i++) {
                if (!_checkPointOnCurve(data, offset)) allOnCurve = false;
                offset += 64;
                numPoints++;
            }

            uint256 numOpenings = _readU256(data, offset);
            offset += 32;
            for (uint256 i = 0; i < numOpenings; i++) {
                offset += 64; // z1, z2
                if (!_checkPointOnCurve(data, offset)) allOnCurve = false;
                offset += 64;
                numPoints++;
            }

            uint256 numEq = _readU256(data, offset);
            offset += 32;
            for (uint256 i = 0; i < numEq; i++) {
                if (!_checkPointOnCurve(data, offset)) allOnCurve = false;
                offset += 64;
                numPoints++;
                offset += 32; // z scalar
            }
        }

        newOffset = offset;
    }

    function _validateClaimPoints(bytes calldata data, uint256 offset)
        internal
        pure
        returns (uint256 numPoints, bool allOnCurve, uint256 newOffset)
    {
        allOnCurve = true;
        offset += 32 + 32; // value + blinding
        if (!_checkPointOnCurve(data, offset)) allOnCurve = false;
        offset += 64;
        numPoints = 1;

        uint256 numPoint = _readU256(data, offset);
        offset += 32;
        offset += numPoint * 32;

        newOffset = offset;
    }

    function _validateInputProofPoints(bytes calldata data, uint256 offset)
        internal
        pure
        returns (uint256 numPoints, bool allOnCurve, uint256 newOffset)
    {
        allOnCurve = true;

        uint256 numCommits = _readU256(data, offset);
        offset += 32;
        for (uint256 i = 0; i < numCommits; i++) {
            if (!_checkPointOnCurve(data, offset)) allOnCurve = false;
            offset += 64;
            numPoints++;
        }

        uint256 numEvals = _readU256(data, offset);
        offset += 32;
        for (uint256 i = 0; i < numEvals; i++) {
            // PODP: 2 points + z_vector + z_delta + z_beta
            if (!_checkPointOnCurve(data, offset)) allOnCurve = false;
            offset += 64;
            numPoints++; // commitD
            if (!_checkPointOnCurve(data, offset)) allOnCurve = false;
            offset += 64;
            numPoints++; // commitDDotA
            uint256 numZ = _readU256(data, offset);
            offset += 32;
            offset += numZ * 32 + 32 + 32;

            // commitmentToEvaluation
            if (!_checkPointOnCurve(data, offset)) allOnCurve = false;
            offset += 64;
            numPoints++;
        }

        newOffset = offset;
    }

    // ============================
    // PRIMITIVES
    // ============================

    function _readU256(bytes calldata data, uint256 offset) internal pure returns (uint256) {
        return uint256(bytes32(data[offset:offset + 32]));
    }

    function _checkPointOnCurve(bytes calldata data, uint256 offset) internal pure returns (bool) {
        uint256 x = uint256(bytes32(data[offset:offset + 32]));
        uint256 y = uint256(bytes32(data[offset + 32:offset + 64]));
        return HyraxVerifier.isOnCurve(HyraxVerifier.G1Point(x, y));
    }
}
