// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @title HybridStylusGroth16Verifier
/// @notice Orchestrates two-step hybrid verification: Stylus WASM (GKR transcript replay)
///         followed by Groth16 (EC arithmetic proof).
/// @dev    Step 1: Call the Stylus verifier in hybrid mode to replay the Fiat-Shamir
///                 transcript and produce a transcript digest plus Fr-encoded outputs.
///         Step 2: Call the Groth16 verifier with the transcript digest, circuit hash,
///                 and Fr outputs as public inputs.
///         Both steps must succeed for the proof to be considered valid.
library HybridStylusGroth16Verifier {
    // ========================================================================
    // ERRORS
    // ========================================================================

    /// @notice Stylus verifier call reverted or ran out of gas
    error StylusVerificationFailed();

    /// @notice Groth16 verifier call reverted or ran out of gas
    error Groth16VerificationFailed();

    // ========================================================================
    // CONSTANTS
    // ========================================================================

    /// @dev BN254 scalar field modulus (Fr)
    uint256 internal constant FR_MODULUS =
        21888242871839275222246405745257275088548364400416034343698204186575808495617;

    // ========================================================================
    // STYLUS CALL
    // ========================================================================

    /// @dev Call the Stylus verifier in hybrid mode via staticcall
    ///      Selector: keccak256("verifyDagProofHybrid(bytes,bytes,bytes,bytes)")[:4]
    ///      Returns: (bool success, bytes32 transcriptDigest, bytes frOutputs)
    function callStylusHybrid(
        address stylusVerifier,
        bytes calldata proof,
        bytes calldata publicInputs,
        bytes calldata gensData,
        bytes memory circuitDescData
    ) internal view returns (bool success, bytes32 transcriptDigest, bytes memory frOutputs) {
        bytes memory callData = abi.encodeWithSelector(
            bytes4(keccak256("verifyDagProofHybrid(bytes,bytes,bytes,bytes)")),
            proof,
            publicInputs,
            gensData,
            circuitDescData
        );

        (bool callSuccess, bytes memory result) = stylusVerifier.staticcall(callData);

        if (!callSuccess) {
            // Propagate revert reason if available
            if (result.length > 0) {
                assembly {
                    revert(add(result, 32), mload(result))
                }
            }
            revert StylusVerificationFailed();
        }

        // Decode: (bool, bytes32, bytes)
        (success, transcriptDigest, frOutputs) = abi.decode(result, (bool, bytes32, bytes));
    }

    // ========================================================================
    // GROTH16 CALL
    // ========================================================================

    /// @dev Build the Groth16 public inputs array from transcript digest, circuit hash,
    ///      and Fr outputs from the Stylus verifier.
    ///      Layout: [transcriptDigest, circuitHashLo, circuitHashHi, frOutput0, frOutput1, ...]
    ///      The circuit hash is split into two 128-bit halves (lo/hi) to fit in Fr.
    function buildGroth16Inputs(
        bytes32 transcriptDigest,
        bytes32 circuitHash,
        bytes memory frOutputs,
        uint256 expectedCount
    ) internal pure returns (uint256[] memory inputs) {
        // frOutputs is packed as 32-byte words
        uint256 numFrOutputs = frOutputs.length / 32;

        // Total inputs = 1 (digest) + 2 (circuitHash lo/hi) + numFrOutputs
        uint256 totalInputs = 3 + numFrOutputs;

        // If expected count differs from what we computed, pad or truncate to match
        // the Groth16 verifier's fixed signature
        if (expectedCount > 0 && expectedCount != totalInputs) {
            totalInputs = expectedCount;
        }

        inputs = new uint256[](totalInputs);

        // Transcript digest (reduced to Fr)
        inputs[0] = uint256(transcriptDigest) % FR_MODULUS;

        // Circuit hash split: lo = first 16 bytes (LE), hi = last 16 bytes (LE)
        (uint256 circuitHashLo, uint256 circuitHashHi) = _hashToFqPair(circuitHash);
        inputs[1] = circuitHashLo;
        inputs[2] = circuitHashHi;

        // Copy Fr outputs from Stylus
        uint256 copyCount = numFrOutputs;
        if (3 + copyCount > totalInputs) {
            copyCount = totalInputs > 3 ? totalInputs - 3 : 0;
        }
        for (uint256 i = 0; i < copyCount; i++) {
            uint256 val;
            assembly {
                val := mload(add(add(frOutputs, 32), mul(i, 32)))
            }
            inputs[3 + i] = val;
        }
    }

    /// @dev Call the Groth16 verifier with raw staticcall (inline encoding, no offset/length)
    ///      Matches the pattern from RemainderVerifier._callGroth16Verifier()
    function callGroth16Verifier(address verifier, bytes4 selector, uint256[8] calldata proof, uint256[] memory inputs)
        internal
        view
    {
        // Copy proof to memory for assembly access
        uint256[8] memory proofMem = proof;
        uint256 n = inputs.length;

        // Build calldata: selector(4) + 8 proof words + N input words (all inline, 32 bytes each)
        bytes memory data = new bytes(4 + (8 + n) * 32);
        assembly {
            let ptr := add(data, 32) // skip Solidity length prefix

            // Write selector (4 bytes, left-aligned in word)
            mstore(ptr, shl(224, shr(224, selector)))
            ptr := add(ptr, 4)

            // Copy proof (8 uint256s from memory fixed array -- no length prefix)
            for { let i := 0 } lt(i, 8) { i := add(i, 1) } {
                mstore(ptr, mload(add(proofMem, mul(i, 32))))
                ptr := add(ptr, 32)
            }

            // Copy inputs (N uint256s from memory dynamic array -- skip length prefix)
            let inputsPtr := add(inputs, 32)
            for { let i := 0 } lt(i, n) { i := add(i, 1) } {
                mstore(ptr, mload(add(inputsPtr, mul(i, 32))))
                ptr := add(ptr, 32)
            }
        }

        (bool success, bytes memory returnData) = verifier.staticcall(data);
        if (!success) {
            if (returnData.length > 0) {
                assembly {
                    revert(add(returnData, 32), mload(returnData))
                }
            }
            revert Groth16VerificationFailed();
        }
    }

    // ========================================================================
    // UTILITIES
    // ========================================================================

    /// @dev Split a bytes32 hash into two field elements (LE 128-bit halves)
    ///      Matches RemainderVerifier._hashToFqPair()
    function _hashToFqPair(bytes32 hash) private pure returns (uint256 fq1, uint256 fq2) {
        assembly {
            // First 16 bytes -> LE uint256
            fq1 := 0
            for { let i := 0 } lt(i, 16) { i := add(i, 1) } { fq1 := or(shl(mul(i, 8), byte(i, hash)), fq1) }
            // Second 16 bytes -> LE uint256
            fq2 := 0
            for { let i := 0 } lt(i, 16) { i := add(i, 1) } {
                fq2 := or(shl(mul(i, 8), byte(add(i, 16), hash)), fq2)
            }
        }
    }

    /// @dev Compute the selector for verifyProof(uint256[8],uint256[N])
    function computeGroth16Selector(uint256 n) internal pure returns (bytes4) {
        return bytes4(keccak256(abi.encodePacked("verifyProof(uint256[8],uint256[", _uintToString(n), "])")));
    }

    /// @dev Convert uint to string for selector computation
    function _uintToString(uint256 value) private pure returns (string memory) {
        if (value == 0) return "0";
        uint256 temp = value;
        uint256 digits;
        while (temp != 0) {
            digits++;
            temp /= 10;
        }
        bytes memory buffer = new bytes(digits);
        while (value != 0) {
            digits--;
            buffer[digits] = bytes1(uint8(48 + value % 10));
            value /= 10;
        }
        return string(buffer);
    }
}
