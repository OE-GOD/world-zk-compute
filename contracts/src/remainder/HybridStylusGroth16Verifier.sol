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

    /// @notice A specific chunk's Groth16 verification failed
    error ChunkVerificationFailed(uint256 chunkIndex);

    /// @notice Invalid chunk count (zero or mismatch with proof array)
    error InvalidChunkCount();

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
    // CHUNKED GROTH16 VERIFICATION
    // ========================================================================

    /// @dev Build public inputs for a single chunk in multi-chunk verification.
    ///      Layout: [transcriptDigest, circuitHashLo, circuitHashHi, chunkIndex, totalChunks, opsDigest]
    function buildChunkedGroth16Inputs(
        bytes32 transcriptDigest,
        bytes32 circuitHash,
        uint256 chunkIndex,
        uint256 totalChunks,
        uint256 opsDigest,
        uint256 expectedCount
    ) internal pure returns (uint256[] memory inputs) {
        // Base inputs = 6 (digest + circuitHash lo/hi + chunkIndex + totalChunks + opsDigest)
        uint256 totalInputs = 6;
        if (expectedCount > 0 && expectedCount != totalInputs) {
            totalInputs = expectedCount;
        }

        inputs = new uint256[](totalInputs);

        inputs[0] = uint256(transcriptDigest) % FR_MODULUS;
        (uint256 lo, uint256 hi) = _hashToFqPair(circuitHash);
        inputs[1] = lo;
        inputs[2] = hi;
        inputs[3] = chunkIndex;
        inputs[4] = totalChunks;
        if (totalInputs > 5) {
            inputs[5] = opsDigest;
        }
    }

    /// @dev Call the Groth16 verifier with a memory proof array (for chunked verification
    ///      where proofs come from a dynamic array). Same as callGroth16Verifier but takes
    ///      uint256[8] memory instead of calldata.
    function callGroth16VerifierMem(
        address verifier,
        bytes4 selector,
        uint256[8] memory proofMem,
        uint256[] memory inputs
    ) internal view {
        uint256 n = inputs.length;
        bytes memory data = new bytes(4 + (8 + n) * 32);
        assembly {
            let ptr := add(data, 32)
            mstore(ptr, shl(224, shr(224, selector)))
            ptr := add(ptr, 4)
            for { let i := 0 } lt(i, 8) { i := add(i, 1) } {
                mstore(ptr, mload(add(proofMem, mul(i, 32))))
                ptr := add(ptr, 32)
            }
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

    /// @dev Verify all chunks of a chunked Groth16 proof.
    ///      Iterates over chunk proofs and verifies each against per-chunk public inputs.
    /// @param ecVerifier      Groth16 verifier contract address
    /// @param ecInputCount    Expected number of public inputs per chunk
    /// @param transcriptDigest Digest from Stylus step 1
    /// @param circuitHash     Circuit hash for input building
    /// @param chunkProofs     Array of 8-element Groth16 proofs (one per chunk)
    /// @param totalChunks     Total number of chunks
    /// @param opsDigest       Operations digest binding all chunks
    function verifyAllChunks(
        address ecVerifier,
        uint256 ecInputCount,
        bytes32 transcriptDigest,
        bytes32 circuitHash,
        uint256[][] memory chunkProofs,
        uint256 totalChunks,
        uint256 opsDigest
    ) internal view {
        if (totalChunks == 0) revert InvalidChunkCount();
        if (chunkProofs.length != totalChunks) revert InvalidChunkCount();

        bytes4 sel = computeGroth16Selector(ecInputCount);

        for (uint256 i = 0; i < totalChunks; i++) {
            // Each chunk proof must be exactly 8 uint256s
            if (chunkProofs[i].length != 8) revert ChunkVerificationFailed(i);

            uint256[] memory inputs =
                buildChunkedGroth16Inputs(transcriptDigest, circuitHash, i, totalChunks, opsDigest, ecInputCount);

            // Copy flat proof into fixed-size array
            uint256[8] memory proofMem;
            for (uint256 j = 0; j < 8; j++) {
                proofMem[j] = chunkProofs[i][j];
            }

            callGroth16VerifierMem(ecVerifier, sel, proofMem, inputs);
        }
    }

    // ========================================================================
    // FULL HYBRID CHUNKED VERIFICATION
    // ========================================================================

    /// @notice Verify a DAG proof using Stylus WASM + multiple Groth16 chunk proofs
    /// @dev Each chunk proves a subset of EC operations. All chunks share transcriptDigest binding.
    ///      Step 1: Call the Stylus verifier in hybrid mode (transcript replay + Fr arithmetic).
    ///      Step 2: Verify each chunk's Groth16 proof against per-chunk public inputs.
    /// @param stylusVerifier Address of Stylus WASM verifier
    /// @param ecGroth16Verifier Address of chunked EC Groth16 verifier (same contract for all chunks)
    /// @param ecGroth16InputCount Number of public inputs per chunk
    /// @param proof GKR proof data
    /// @param circuitHash Circuit hash
    /// @param publicInputs Public inputs for Stylus call
    /// @param gensData Generator data for Stylus call
    /// @param circuitDescData Circuit description for Stylus call
    /// @param ecGroth16Proofs Array of Groth16 proofs, one per chunk (each 8 uint256)
    /// @return verified Whether all verifications passed
    function verifyHybridChunked(
        address stylusVerifier,
        address ecGroth16Verifier,
        uint256 ecGroth16InputCount,
        bytes calldata proof,
        bytes32 circuitHash,
        bytes calldata publicInputs,
        bytes calldata gensData,
        bytes memory circuitDescData,
        uint256[][] memory ecGroth16Proofs
    ) internal view returns (bool verified) {
        // Step 1: Call Stylus hybrid (same as verifyHybrid)
        (bool stylusSuccess, bytes32 transcriptDigest, bytes memory frOutputs) =
            callStylusHybrid(stylusVerifier, proof, publicInputs, gensData, circuitDescData);
        if (!stylusSuccess) return false;

        // Step 2: Compute opsDigest from frOutputs for chunk binding
        uint256 opsDigest = uint256(keccak256(frOutputs)) % FR_MODULUS;

        // Step 3: Verify each chunk's Groth16 proof
        uint256 totalChunks = ecGroth16Proofs.length;
        verifyAllChunks(
            ecGroth16Verifier,
            ecGroth16InputCount,
            transcriptDigest,
            circuitHash,
            ecGroth16Proofs,
            totalChunks,
            opsDigest
        );

        return true;
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
