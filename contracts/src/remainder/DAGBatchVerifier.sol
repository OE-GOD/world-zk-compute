// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import {PoseidonSponge} from "./PoseidonSponge.sol";

/// @title DAGBatchVerifier
/// @notice Multi-transaction batch verification for DAG circuits that exceed block gas limits.
/// @dev Full DAG verification for the XGBoost 88-layer circuit costs ~254M gas in a single
///      transaction, far exceeding the Ethereum 30M block gas limit. This library splits
///      verification into multiple transactions (start + continue + finalize), each staying
///      under 30M gas. The full proof is re-supplied via calldata each batch (~16 gas/byte,
///      cheaper than persisting in storage at ~20K gas/slot). Only sponge state, bindings,
///      and output eval are persisted between batches.
///
///      Gas profile for 88-layer XGBoost circuit (15 total txs):
///        - Start:    ~17.5M gas (1 tx -- setup, transcript init, output challenge squeeze)
///        - Continue: ~13-28M gas per tx (11 txs -- 8 compute layers each)
///        - Finalize: ~9-22M gas per tx (3 txs -- 16 input eval groups each)
///
///      Smaller circuits with fewer layers/groups will use fewer transactions.
library DAGBatchVerifier {
    // ========================================================================
    // CONSTANTS
    // ========================================================================

    /// @notice Number of compute layers processed per batch transaction
    /// @dev Chosen based on gas profiling of the 88-layer XGBoost DAG circuit:
    ///
    ///      Gas profiling results (88 compute layers, sample_model):
    ///        - Total single-tx cost: ~252M gas (far exceeds 30M block gas limit)
    ///        - Per-layer cost varies: ~1.5M-3.5M gas depending on layer complexity
    ///        - Average per-layer: ~2.9M gas
    ///        - 8 layers/batch => ~13M-28M gas per batch (all under 30M limit)
    ///        - 88 / 8 = 11 compute batches
    ///
    ///      Trade-offs considered:
    ///        - LAYERS_PER_BATCH = 4:  22 batches, ~7-14M gas each. Safe margin but
    ///          more transactions = higher total overhead from cross-batch storage I/O
    ///          (~5K gas per sstore for bindings persistence per batch).
    ///        - LAYERS_PER_BATCH = 8:  11 batches, ~13-28M gas each. Sweet spot --
    ///          all batches fit within 30M limit with margin, minimizes total tx count.
    ///        - LAYERS_PER_BATCH = 16: 6 batches, ~26-56M gas each. Some batches
    ///          would exceed 30M limit for complex layers. Not viable.
    ///
    ///      The value 8 provides the best balance: fewest transactions while staying
    ///      safely under the 30M block gas limit for all observed layer combinations.
    ///
    ///      Adjusting for different circuits:
    ///        Smaller circuits with fewer or simpler layers (fewer sumcheck rounds,
    ///        fewer atoms per layer) can safely increase this value. For example,
    ///        a 20-layer circuit with 2-round sumchecks may tolerate 16 layers/batch.
    ///        To determine the right value for a new circuit:
    ///          1. Deploy and run single-tx verification on a high-gas-limit testnet
    ///             (e.g., Anvil with --gas-limit 500000000) to measure total gas.
    ///          2. Divide total gas by number of compute layers for avg per-layer cost.
    ///          3. Set LAYERS_PER_BATCH = floor(25M / avg_per_layer_gas) to leave ~5M
    ///             margin for storage I/O and proof decoding overhead per batch.
    ///        The 30M limit is Ethereum mainnet; L2s with higher limits allow larger batches.
    uint256 internal constant LAYERS_PER_BATCH = 8;

    /// @notice Number of committed input eval groups processed per finalize transaction
    /// @dev Chosen based on gas profiling of input layer verification:
    ///
    ///      Gas profiling results (34 eval groups from XGBoost sample_model):
    ///        - Per-group cost: ~0.5M-1.4M gas (varies by group size and claim count)
    ///        - Groups with 2 claims (groups 12, 25) cost more due to RLC aggregation
    ///        - 16 groups/finalize => ~9M-22M gas per finalize tx
    ///        - 34 / 16 = 3 finalize transactions (16 + 16 + 2)
    ///
    ///      Trade-offs considered:
    ///        - GROUPS_PER_FINALIZE_BATCH = 8:  5 finalize txs, ~4.5-11M gas each.
    ///          Very safe but adds 2 extra transactions with storage overhead.
    ///        - GROUPS_PER_FINALIZE_BATCH = 16: 3 finalize txs, ~9-22M gas each.
    ///          All under 30M limit. Good balance of tx count vs gas per tx.
    ///        - GROUPS_PER_FINALIZE_BATCH = 34: 1 finalize tx, ~22-48M gas.
    ///          Would exceed 30M for large circuits. Not viable.
    ///
    ///      Total verification pipeline: 1 start + 11 continue + 3 finalize = 15 txs.
    ///
    ///      Adjusting for different circuits:
    ///        The number of eval groups equals the number of committed input claims
    ///        (each claim creates its own singleton group; claims with matching R-halves
    ///        also join earlier groups). Circuits with fewer committed inputs have fewer
    ///        groups and may process them all in a single finalize call.
    ///        To tune for a new circuit:
    ///          1. Count committed input claims from the proof (numGroups == numClaims).
    ///          2. Run single finalize on a high-gas-limit testnet to measure total gas.
    ///          3. Set GROUPS_PER_FINALIZE_BATCH = floor(25M / avg_per_group_gas).
    ///        Circuits with only public (non-committed) inputs skip PODP finalization
    ///        entirely, since public input verification uses MLE evaluation instead.
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

    /// @notice Persist a DAGBatchSession to storage using raw assembly sstore
    /// @dev Uses 14 contiguous storage slots starting from keccak256("DAGBatchVerifier.sessions", sessionId).
    ///      Assembly is used to avoid Solidity's per-slot overhead and to pack the struct efficiently.
    /// @param sessionId The unique session identifier
    /// @param session The session state to persist
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

    /// @notice Load a DAGBatchSession from storage using raw assembly sload
    /// @dev Reads 14 contiguous storage slots. Returns a zero-initialized session if the
    ///      session does not exist (caller must check circuitHash or verifier fields).
    /// @param sessionId The unique session identifier
    /// @return session The loaded session state
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

    /// @notice Extract PoseidonSponge state from a session struct
    /// @dev The sponge state is stored as 4 separate uint256 fields in the session to allow
    ///      efficient assembly-based storage. This reconstructs the Sponge struct for use
    ///      with PoseidonSponge library functions.
    /// @param session The batch session containing persisted sponge state
    /// @return sponge The reconstructed PoseidonSponge.Sponge struct
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

    /// @notice Write PoseidonSponge state back into a session struct
    /// @dev Copies the 4 sponge values into the session's flat fields for storage persistence.
    /// @param sponge The current sponge state to save
    /// @param session The batch session to update (modified in-place in memory)
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

    /// @dev Storage array identifiers for cross-batch data. Each array is stored at a
    ///      separate keccak256-derived slot to avoid collisions between sessions.
    uint256 private constant ARR_BINDINGS_FLAT = 0; // Flat concatenation of all layer bindings
    uint256 private constant ARR_BINDINGS_OFFSETS = 1; // Index boundaries for each layer in bindingsFlat
    uint256 private constant ARR_OUTPUT_CHALLENGES = 2; // Output challenges squeezed from transcript in batch 0

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

    /// @notice Persist cross-batch data (bindings + output challenges) to storage
    /// @dev Each array is stored at its own keccak256-derived slot. Storage cost scales
    ///      linearly with the total number of binding values across all processed layers.
    ///      For the 88-layer XGBoost circuit, this stores ~300-400 uint256 values.
    /// @param sessionId The session these bindings belong to
    /// @param data The cross-batch data containing bindings and output challenges
    function storeCrossBatchData(bytes32 sessionId, CrossBatchData memory data) internal {
        _storeArray(sessionId, ARR_BINDINGS_FLAT, data.bindingsFlat);
        _storeArray(sessionId, ARR_BINDINGS_OFFSETS, data.bindingsOffsets);
        _storeArray(sessionId, ARR_OUTPUT_CHALLENGES, data.outputChallenges);
    }

    /// @notice Load cross-batch data (bindings + output challenges) from storage
    /// @dev Returns empty arrays if no data has been stored for this session.
    /// @param sessionId The session to load data for
    /// @return data The cross-batch data containing bindings and output challenges
    function loadCrossBatchData(bytes32 sessionId) internal view returns (CrossBatchData memory data) {
        data.bindingsFlat = _loadArray(sessionId, ARR_BINDINGS_FLAT);
        data.bindingsOffsets = _loadArray(sessionId, ARR_BINDINGS_OFFSETS);
        data.outputChallenges = _loadArray(sessionId, ARR_OUTPUT_CHALLENGES);
    }

    // ========================================================================
    // CLEANUP
    // ========================================================================

    /// @notice Clear all storage associated with a batch verification session
    /// @dev Zeroes out the 14-slot session struct and all three cross-batch arrays.
    ///      Should be called after successful verification or to abort a stale session.
    ///      Gas cost depends on the amount of stored data (binding values + offsets).
    ///      Provides gas refunds via SSTORE-to-zero (EIP-3529: up to 4800 gas per slot).
    /// @param sessionId The session to clean up
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

    /// @notice Calculate the number of compute batch transactions needed
    /// @dev Uses ceiling division: (N + LAYERS_PER_BATCH - 1) / LAYERS_PER_BATCH.
    ///      For 88 layers with LAYERS_PER_BATCH=8, this returns 11.
    /// @param numComputeLayers Total number of compute layers in the DAG circuit
    /// @return The number of continueDAGBatchVerify() calls required
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

    /// @notice Generate a deterministic session ID from the caller, circuit, and block number
    /// @dev The session ID binds a verification session to a specific (sender, circuit, block) tuple.
    ///      COLLISION NOTE: If the same sender starts two sessions for the same circuit in the same
    ///      block, the session IDs will collide and the second start will overwrite the first. This
    ///      is by design -- callers needing parallel verification of the same circuit should use
    ///      different sender addresses or spread calls across blocks.
    ///      REPLAY PROTECTION: Since block.number is included, sessions cannot be reused or replayed
    ///      across blocks. A cleaned-up session from block N will have a different ID if recreated
    ///      in block M (where M != N).
    /// @param sender The address initiating the verification (typically msg.sender)
    /// @param circuitHash The circuit being verified
    /// @param blockNum The block number at session creation time
    /// @return The unique session identifier
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
