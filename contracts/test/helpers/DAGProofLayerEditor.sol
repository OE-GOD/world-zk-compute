// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

/// @notice Builds DAG proofs with one layer proof more or fewer than an honest proof, for tests of
///         the rule that a proof carries exactly one layer proof per compute layer.
/// @dev Walks the "REM1" proof layout the same way RemainderVerifier._decodeProofCommon and
///      _decodeCommittedLayerProof read it. Only the layer section changes; every other byte is
///      copied as it is. Deploy it and call it, so the proof can be sliced as calldata.
contract DAGProofLayerEditor {
    struct LayerSection {
        uint256 countOffset; // offset of the numLayerProofs word
        uint256 count;
        uint256 lastStart; // first byte of the last layer proof
        uint256 lastEnd; // first byte after the last layer proof
    }

    function layerProofCount(bytes calldata proof) external pure returns (uint256) {
        return _layerSection(proof).count;
    }

    /// @notice The proof with its last layer proof appended a second time and the count raised by one
    function withLastLayerProofRepeated(bytes calldata proof) external pure returns (bytes memory) {
        LayerSection memory s = _layerSection(proof);
        return bytes.concat(
            proof[:s.countOffset],
            bytes32(s.count + 1),
            proof[s.countOffset + 32:s.lastEnd],
            proof[s.lastStart:s.lastEnd],
            proof[s.lastEnd:]
        );
    }

    /// @notice The proof without its last layer proof and with the count lowered by one
    function withoutLastLayerProof(bytes calldata proof) external pure returns (bytes memory) {
        LayerSection memory s = _layerSection(proof);
        return bytes.concat(
            proof[:s.countOffset], bytes32(s.count - 1), proof[s.countOffset + 32:s.lastStart], proof[s.lastEnd:]
        );
    }

    function _layerSection(bytes calldata proof) private pure returns (LayerSection memory s) {
        // "REM1" selector, then the circuit hash
        uint256 off = 4 + 32;
        // Embedded public input sections
        uint256 n = _word(proof, off);
        off += 32;
        for (uint256 i = 0; i < n; i++) {
            off += 32 + _word(proof, off) * 32;
        }
        // Output claim commitments (G1 points)
        off += 32 + _word(proof, off) * 64;

        s.countOffset = off;
        s.count = _word(proof, off);
        off += 32;
        for (uint256 i = 0; i < s.count; i++) {
            s.lastStart = off;
            off = _skipLayerProof(proof, off);
        }
        s.lastEnd = off;
    }

    function _skipLayerProof(bytes calldata proof, uint256 off) private pure returns (uint256) {
        off += 64; // sumcheck sum
        off += 32 + _word(proof, off) * 64; // sumcheck messages
        off += 128; // PODP commitD, commitDDotA
        off += 32 + _word(proof, off) * 32; // PODP zVector
        off += 64; // PODP zDelta, zBeta
        off += 32 + _word(proof, off) * 64; // post-sumcheck commitments
        off += 32 + _word(proof, off) * (3 * 64 + 5 * 32); // proofs of product
        uint256 hasAgg = _word(proof, off);
        off += 32;
        if (hasAgg == 1) {
            off += 32 + _word(proof, off) * 64; // coefficients
            off += 32 + _word(proof, off) * 128; // openings
            off += 32 + _word(proof, off) * 96; // equality proofs
        }
        return off;
    }

    function _word(bytes calldata proof, uint256 off) private pure returns (uint256) {
        return uint256(bytes32(proof[off:off + 32]));
    }
}
