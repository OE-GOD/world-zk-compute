// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import {PoseidonSponge} from "./PoseidonSponge.sol";

/// @title DAGBatchVerifier
/// @notice Multi-transaction batch verification for DAG circuits that exceed block gas limits.
/// @dev Splits the ~252M gas single-tx DAG verification into ~12 batches of ~20M gas each.
///      The full proof is re-supplied via calldata each batch (cheaper than storing in storage).
///      Only sponge state, bindings, and output eval are persisted between batches.
library DAGBatchVerifier {
    // ========================================================================
    // CONSTANTS
    // ========================================================================

    /// @notice Number of compute layers processed per batch
    uint256 internal constant LAYERS_PER_BATCH = 8;

    /// @notice Number of committed input eval groups processed per finalize tx
    uint256 internal constant GROUPS_PER_FINALIZE_BATCH = 16;

    // ========================================================================
    // TYPES
    // ========================================================================

    /// @notice Session state persisted across batch transactions
    struct DAGBatchSession {
        bytes32 circuitHash;
        address verifier; // msg.sender for auth
        uint256 nextBatchIdx; // Next expected batch index
        uint256 totalBatches; // Total number of compute batches (not counting finalize)
        uint256 numComputeLayers; // From circuit description
        bool finalized;
        // Sponge state (4 values)
        uint256 spongeState0;
        uint256 spongeState1;
        uint256 spongeState2;
        uint256 spongeAbsorbing;
        // Output layer state (needed for layer 0 RLC computation)
        uint256 outputEvalX;
        uint256 outputEvalY;
        // Finalize progress (for multi-tx finalize)
        uint256 finalizeInputIdx; // Current input layer index during finalize
        uint256 finalizeGroupsDone; // Eval groups verified so far in current committed input
    }

    /// @notice Cross-batch data persisted in storage between transactions
    struct CrossBatchData {
        // Flat array of all bindings from processed layers, concatenated.
        // bindingsFlat[bindingsOffsets[i]..bindingsOffsets[i+1]] = bindings for layer i
        uint256[] bindingsFlat;
        uint256[] bindingsOffsets; // length = processedLayers + 1
        // Output challenges (squeezed once in batch 0, needed for layer 0 RLC)
        uint256[] outputChallenges;
    }

    // ========================================================================
    // STORAGE SLOT COMPUTATION
    // ========================================================================

    function _sessionSlot(bytes32 sessionId) private pure returns (bytes32) {
        return keccak256(abi.encodePacked("DAGBatchVerifier.sessions", sessionId));
    }

    // ========================================================================
    // SESSION STORAGE
    // ========================================================================

    function storeSession(bytes32 sessionId, DAGBatchSession memory session) internal {
        bytes32 base = _sessionSlot(sessionId);
        assembly {
            sstore(base, mload(session)) // circuitHash
            sstore(add(base, 1), mload(add(session, 0x20))) // verifier
            sstore(add(base, 2), mload(add(session, 0x40))) // nextBatchIdx
            sstore(add(base, 3), mload(add(session, 0x60))) // totalBatches
            sstore(add(base, 4), mload(add(session, 0x80))) // numComputeLayers
            sstore(add(base, 5), mload(add(session, 0xa0))) // finalized
            sstore(add(base, 6), mload(add(session, 0xc0))) // spongeState0
            sstore(add(base, 7), mload(add(session, 0xe0))) // spongeState1
            sstore(add(base, 8), mload(add(session, 0x100))) // spongeState2
            sstore(add(base, 9), mload(add(session, 0x120))) // spongeAbsorbing
            sstore(add(base, 10), mload(add(session, 0x140))) // outputEvalX
            sstore(add(base, 11), mload(add(session, 0x160))) // outputEvalY
            sstore(add(base, 12), mload(add(session, 0x180))) // finalizeInputIdx
            sstore(add(base, 13), mload(add(session, 0x1a0))) // finalizeGroupsDone
        }
    }

    function loadSession(bytes32 sessionId) internal view returns (DAGBatchSession memory session) {
        bytes32 base = _sessionSlot(sessionId);
        assembly {
            mstore(session, sload(base)) // circuitHash
            mstore(add(session, 0x20), sload(add(base, 1))) // verifier
            mstore(add(session, 0x40), sload(add(base, 2))) // nextBatchIdx
            mstore(add(session, 0x60), sload(add(base, 3))) // totalBatches
            mstore(add(session, 0x80), sload(add(base, 4))) // numComputeLayers
            mstore(add(session, 0xa0), sload(add(base, 5))) // finalized
            mstore(add(session, 0xc0), sload(add(base, 6))) // spongeState0
            mstore(add(session, 0xe0), sload(add(base, 7))) // spongeState1
            mstore(add(session, 0x100), sload(add(base, 8))) // spongeState2
            mstore(add(session, 0x120), sload(add(base, 9))) // spongeAbsorbing
            mstore(add(session, 0x140), sload(add(base, 10))) // outputEvalX
            mstore(add(session, 0x160), sload(add(base, 11))) // outputEvalY
            mstore(add(session, 0x180), sload(add(base, 12))) // finalizeInputIdx
            mstore(add(session, 0x1a0), sload(add(base, 13))) // finalizeGroupsDone
        }
    }

    // ========================================================================
    // SPONGE STATE HELPERS
    // ========================================================================

    function spongeFromSession(DAGBatchSession memory session)
        internal
        pure
        returns (PoseidonSponge.Sponge memory sponge)
    {
        sponge.state[0] = session.spongeState0;
        sponge.state[1] = session.spongeState1;
        sponge.state[2] = session.spongeState2;
        sponge.absorbing = session.spongeAbsorbing;
    }

    function spongeToSession(PoseidonSponge.Sponge memory sponge, DAGBatchSession memory session) internal pure {
        session.spongeState0 = sponge.state[0];
        session.spongeState1 = sponge.state[1];
        session.spongeState2 = sponge.state[2];
        session.spongeAbsorbing = sponge.absorbing;
    }

    // ========================================================================
    // CROSS-BATCH DATA STORAGE
    // ========================================================================

    function _crossBatchSlot(bytes32 sessionId, uint256 arrayId) private pure returns (bytes32) {
        return keccak256(abi.encodePacked("DAGBatchVerifier.crossBatch", sessionId, arrayId));
    }

    uint256 private constant ARR_BINDINGS_FLAT = 0;
    uint256 private constant ARR_BINDINGS_OFFSETS = 1;
    uint256 private constant ARR_OUTPUT_CHALLENGES = 2;

    function _storeArray(bytes32 sessionId, uint256 arrayId, uint256[] memory arr) private {
        bytes32 base = _crossBatchSlot(sessionId, arrayId);
        uint256 len = arr.length;
        assembly {
            sstore(base, len)
        }
        for (uint256 i = 0; i < len; i++) {
            bytes32 slot = bytes32(uint256(keccak256(abi.encodePacked(base))) + i);
            assembly {
                sstore(slot, mload(add(add(arr, 0x20), mul(i, 0x20))))
            }
        }
    }

    function _loadArray(bytes32 sessionId, uint256 arrayId) private view returns (uint256[] memory arr) {
        bytes32 base = _crossBatchSlot(sessionId, arrayId);
        uint256 len;
        assembly {
            len := sload(base)
        }
        arr = new uint256[](len);
        for (uint256 i = 0; i < len; i++) {
            bytes32 slot = bytes32(uint256(keccak256(abi.encodePacked(base))) + i);
            assembly {
                mstore(add(add(arr, 0x20), mul(i, 0x20)), sload(slot))
            }
        }
    }

    function _clearArray(bytes32 sessionId, uint256 arrayId) private {
        bytes32 base = _crossBatchSlot(sessionId, arrayId);
        uint256 len;
        assembly {
            len := sload(base)
        }
        for (uint256 i = 0; i < len; i++) {
            bytes32 slot = bytes32(uint256(keccak256(abi.encodePacked(base))) + i);
            assembly {
                sstore(slot, 0)
            }
        }
        assembly {
            sstore(base, 0)
        }
    }

    function storeCrossBatchData(bytes32 sessionId, CrossBatchData memory data) internal {
        _storeArray(sessionId, ARR_BINDINGS_FLAT, data.bindingsFlat);
        _storeArray(sessionId, ARR_BINDINGS_OFFSETS, data.bindingsOffsets);
        _storeArray(sessionId, ARR_OUTPUT_CHALLENGES, data.outputChallenges);
    }

    function loadCrossBatchData(bytes32 sessionId) internal view returns (CrossBatchData memory data) {
        data.bindingsFlat = _loadArray(sessionId, ARR_BINDINGS_FLAT);
        data.bindingsOffsets = _loadArray(sessionId, ARR_BINDINGS_OFFSETS);
        data.outputChallenges = _loadArray(sessionId, ARR_OUTPUT_CHALLENGES);
    }

    // ========================================================================
    // CLEANUP
    // ========================================================================

    function cleanupSession(bytes32 sessionId) internal {
        // Clear session struct (14 slots)
        bytes32 base = _sessionSlot(sessionId);
        for (uint256 i = 0; i < 14; i++) {
            assembly {
                sstore(add(base, i), 0)
            }
        }
        _clearArray(sessionId, ARR_BINDINGS_FLAT);
        _clearArray(sessionId, ARR_BINDINGS_OFFSETS);
        _clearArray(sessionId, ARR_OUTPUT_CHALLENGES);
    }

    // ========================================================================
    // BATCH COMPUTATION HELPERS
    // ========================================================================

    function computeNumComputeBatches(uint256 numComputeLayers) internal pure returns (uint256) {
        return (numComputeLayers + LAYERS_PER_BATCH - 1) / LAYERS_PER_BATCH;
    }

    function batchLayerRange(uint256 batchIdx, uint256 numComputeLayers)
        internal
        pure
        returns (uint256 startLayer, uint256 endLayer)
    {
        startLayer = batchIdx * LAYERS_PER_BATCH;
        endLayer = startLayer + LAYERS_PER_BATCH;
        if (endLayer > numComputeLayers) {
            endLayer = numComputeLayers;
        }
    }

    function generateSessionId(address sender, bytes32 circuitHash, uint256 blockNum) internal pure returns (bytes32) {
        return keccak256(abi.encodePacked(sender, circuitHash, blockNum));
    }

    // ========================================================================
    // BINDINGS RECONSTRUCTION
    // ========================================================================

    /// @notice Reconstruct allBindings from cross-batch data
    function reconstructBindings(CrossBatchData memory data, uint256 numComputeLayers)
        internal
        pure
        returns (uint256[][] memory allBindings)
    {
        allBindings = new uint256[][](numComputeLayers);
        if (data.bindingsOffsets.length == 0) return allBindings;
        uint256 numProcessed = data.bindingsOffsets.length - 1;
        for (uint256 i = 0; i < numProcessed && i < numComputeLayers; i++) {
            uint256 start = data.bindingsOffsets[i];
            uint256 end = data.bindingsOffsets[i + 1];
            if (end > start) {
                uint256 len = end - start;
                allBindings[i] = new uint256[](len);
                for (uint256 j = 0; j < len; j++) {
                    allBindings[i][j] = data.bindingsFlat[start + j];
                }
            }
        }
    }

    /// @notice Update cross-batch data with bindings from layers [0, endLayer)
    function updateBindings(CrossBatchData memory data, uint256[][] memory allBindings, uint256 endLayer)
        internal
        pure
    {
        uint256 totalLen = 0;
        for (uint256 i = 0; i < endLayer; i++) {
            totalLen += allBindings[i].length;
        }

        data.bindingsFlat = new uint256[](totalLen);
        data.bindingsOffsets = new uint256[](endLayer + 1);

        uint256 offset = 0;
        for (uint256 i = 0; i < endLayer; i++) {
            data.bindingsOffsets[i] = offset;
            for (uint256 j = 0; j < allBindings[i].length; j++) {
                data.bindingsFlat[offset++] = allBindings[i][j];
            }
        }
        data.bindingsOffsets[endLayer] = offset;
    }
}
