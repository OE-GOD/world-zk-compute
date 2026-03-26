// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/remainder/RemainderVerifier.sol";
import "../src/remainder/GKRDAGVerifier.sol";

/// @title VerifyOnChain
/// @notice Verify a DAG proof against an already-deployed RemainderVerifier.
/// @dev This script requires the circuit to be already registered on the verifier.
///      It loads a fixture, calls verifyDAGProof, and reports the result.
///
///   Required env vars:
///     REMAINDER_VERIFIER  -- address of the deployed RemainderVerifier (proxy)
///     DEPLOYER_KEY        -- private key for sending the verification tx
///
///   Optional env vars:
///     FIXTURE_PATH   -- path to fixture JSON (default: test/fixtures/phase1a_dag_fixture.json)
///     VERIFY_MODE    -- "direct" (default) or "batch" (multi-tx batch verification)
///     REGISTER_IF_NEEDED -- set to "true" to register the circuit if not yet registered
///
///   Usage (direct mode):
///     REMAINDER_VERIFIER=0x... DEPLOYER_KEY=0x... \
///       forge script script/VerifyOnChain.s.sol:VerifyOnChain \
///         --rpc-url $RPC_URL --broadcast -vvv
///
///   Usage (batch mode):
///     REMAINDER_VERIFIER=0x... DEPLOYER_KEY=0x... VERIFY_MODE=batch \
///       forge script script/VerifyOnChain.s.sol:VerifyOnChain \
///         --rpc-url $RPC_URL --broadcast -vvv
contract VerifyOnChain is Script {
    uint256 constant LAYERS_PER_BATCH = 8;

    struct Fixture {
        bytes proofHex;
        bytes gensHex;
        bytes32 circuitHash;
        bytes publicInputsHex;
    }

    function run() external {
        address verifierAddr = vm.envAddress("REMAINDER_VERIFIER");
        uint256 deployerKey = vm.envUint("DEPLOYER_KEY");
        string memory fixturePath = vm.envOr("FIXTURE_PATH", string("test/fixtures/phase1a_dag_fixture.json"));
        string memory verifyMode = vm.envOr("VERIFY_MODE", string("direct"));
        bool registerIfNeeded = vm.envOr("REGISTER_IF_NEEDED", false);

        console.log("=== On-Chain DAG Proof Verification ===");
        console.log("Verifier:  ", verifierAddr);
        console.log("Mode:      ", verifyMode);
        console.log("Fixture:   ", fixturePath);
        console.log("");

        // Validate contract exists
        uint256 codeSize;
        assembly {
            codeSize := extcodesize(verifierAddr)
        }
        require(codeSize > 0, "No contract deployed at REMAINDER_VERIFIER address");
        console.log("Contract code size:", codeSize, "bytes");

        // Load fixture data
        Fixture memory f = _loadFixture(fixturePath);

        console.log("Fixture loaded:");
        console.log("  Circuit hash:", vm.toString(f.circuitHash));
        console.log("  Proof size:  ", f.proofHex.length, "bytes");
        console.log("  Gens size:   ", f.gensHex.length, "bytes");
        console.log("");

        RemainderVerifier verifier = RemainderVerifier(verifierAddr);

        // Check circuit registration
        bool isActive = verifier.isDAGCircuitActive(f.circuitHash);

        if (!isActive && registerIfNeeded) {
            console.log("Circuit not registered. Registering (REGISTER_IF_NEEDED=true)...");
            GKRDAGVerifier.DAGCircuitDescription memory desc = _parseDesc(fixturePath);
            console.log("  Compute layers:", desc.numComputeLayers);
            console.log("  Input layers:  ", desc.numInputLayers);

            vm.startBroadcast(deployerKey);
            bytes32 gensHash = keccak256(f.gensHex);
            verifier.registerDAGCircuit(f.circuitHash, abi.encode(desc), "XGBoost-Phase1a", gensHash);
            vm.stopBroadcast();

            console.log("  Circuit registered");
            console.log("");
        } else if (!isActive) {
            revert("Circuit not registered. Set REGISTER_IF_NEEDED=true or register manually.");
        } else {
            console.log("Circuit is registered and active");
        }

        // Run verification
        vm.startBroadcast(deployerKey);

        if (_strEq(verifyMode, "batch")) {
            GKRDAGVerifier.DAGCircuitDescription memory desc = _parseDesc(fixturePath);
            _runBatchVerify(verifier, f, desc.numComputeLayers);
        } else {
            _runDirectVerify(verifier, f);
        }

        vm.stopBroadcast();

        console.log("");
        console.log("=== VERIFICATION PASSED ===");
    }

    // ── Direct verification ──────────────────────────────────────────────────

    function _runDirectVerify(RemainderVerifier verifier, Fixture memory f) private {
        console.log("Running direct DAG verification...");

        uint256 gasBefore = gasleft();
        bool valid = verifier.verifyDAGProof(f.proofHex, f.circuitHash, f.publicInputsHex, f.gensHex);
        uint256 gasUsed = gasBefore - gasleft();

        console.log("  Result:   ", valid ? "VALID" : "INVALID");
        console.log("  Gas used: ", gasUsed);

        require(valid, "Direct verification FAILED");
    }

    // ── Batch verification ───────────────────────────────────────────────────

    function _runBatchVerify(RemainderVerifier verifier, Fixture memory f, uint256 numComputeLayers) private {
        console.log("Running batch DAG verification...");
        uint256 totalGas = 0;

        // Step 1: Start
        uint256 gasBefore = gasleft();
        bytes32 sessionId = verifier.startDAGBatchVerify(f.proofHex, f.circuitHash, f.publicInputsHex, f.gensHex);
        uint256 startGas = gasBefore - gasleft();
        totalGas += startGas;
        console.log("  Start gas:  ", startGas);
        console.log("  Session:    ", vm.toString(sessionId));

        // Step 2: Continue batches
        uint256 numBatches = (numComputeLayers + LAYERS_PER_BATCH - 1) / LAYERS_PER_BATCH;
        for (uint256 b = 0; b < numBatches; b++) {
            gasBefore = gasleft();
            verifier.continueDAGBatchVerify(sessionId, f.proofHex, f.publicInputsHex, f.gensHex);
            uint256 batchGas = gasBefore - gasleft();
            totalGas += batchGas;
            console.log("  Continue batch", b, "gas:", batchGas);
        }

        // Step 3: Finalize (may require multiple calls)
        uint256 finalizeCount = 0;
        bool done = false;
        while (!done) {
            gasBefore = gasleft();
            done = verifier.finalizeDAGBatchVerify(sessionId, f.proofHex, f.publicInputsHex, f.gensHex);
            uint256 finGas = gasBefore - gasleft();
            totalGas += finGas;
            finalizeCount++;
            console.log("  Finalize", finalizeCount, "gas:", finGas);
        }

        // Step 4: Cleanup
        verifier.cleanupDAGBatchSession(sessionId);

        uint256 totalTxs = 1 + numBatches + finalizeCount;
        console.log("  Total txs:  ", totalTxs);
        console.log("  Total gas:  ", totalGas);
    }

    // ── Fixture loading ──────────────────────────────────────────────────────

    function _loadFixture(string memory fixturePath) private view returns (Fixture memory f) {
        string memory json = vm.readFile(fixturePath);
        f.proofHex = vm.parseJsonBytes(json, ".proof_hex");
        f.gensHex = vm.parseJsonBytes(json, ".gens_hex");
        f.circuitHash = vm.parseJsonBytes32(json, ".circuit_hash_raw");
        f.publicInputsHex = vm.parseJsonBytes(json, ".public_inputs_hex");
    }

    function _parseDesc(string memory fixturePath)
        private
        view
        returns (GKRDAGVerifier.DAGCircuitDescription memory desc)
    {
        string memory json = vm.readFile(fixturePath);
        desc.numComputeLayers = vm.parseJsonUint(json, ".dag_circuit_description.numComputeLayers");
        desc.numInputLayers = vm.parseJsonUint(json, ".dag_circuit_description.numInputLayers");
        desc.layerTypes = _parseUint8Array(json, ".dag_circuit_description.layerTypes");
        desc.numSumcheckRounds = vm.parseJsonUintArray(json, ".dag_circuit_description.numSumcheckRounds");
        desc.atomOffsets = vm.parseJsonUintArray(json, ".dag_circuit_description.atomOffsets");
        desc.atomTargetLayers = vm.parseJsonUintArray(json, ".dag_circuit_description.atomTargetLayers");
        desc.atomCommitIdxs = vm.parseJsonUintArray(json, ".dag_circuit_description.atomCommitIdxs");
        desc.ptOffsets = vm.parseJsonUintArray(json, ".dag_circuit_description.ptOffsets");
        desc.ptData = vm.parseJsonUintArray(json, ".dag_circuit_description.ptData");
        desc.inputIsCommitted = _parseBoolArray(json, ".dag_circuit_description.inputIsCommitted");
        desc.oracleProductOffsets = vm.parseJsonUintArray(json, ".dag_circuit_description.oracleProductOffsets");
        desc.oracleResultIdxs = vm.parseJsonUintArray(json, ".dag_circuit_description.oracleResultIdxs");
        desc.oracleExprCoeffs = _parseUint256Array(json, ".dag_circuit_description.oracleExprCoeffs");
    }

    // ── Parsing helpers ──────────────────────────────────────────────────────

    function _parseUint8Array(string memory json, string memory key) private pure returns (uint8[] memory result) {
        uint256[] memory raw = vm.parseJsonUintArray(json, key);
        result = new uint8[](raw.length);
        for (uint256 i = 0; i < raw.length; i++) {
            result[i] = uint8(raw[i]);
        }
    }

    function _parseBoolArray(string memory json, string memory key) private pure returns (bool[] memory result) {
        bytes memory raw = vm.parseJson(json, key);
        result = abi.decode(raw, (bool[]));
    }

    function _parseUint256Array(string memory json, string memory key) private pure returns (uint256[] memory result) {
        bytes memory raw = vm.parseJson(json, key);
        bytes32[] memory parsed = abi.decode(raw, (bytes32[]));
        result = new uint256[](parsed.length);
        for (uint256 i = 0; i < parsed.length; i++) {
            result[i] = uint256(parsed[i]);
        }
    }

    function _strEq(string memory a, string memory b) private pure returns (bool) {
        return keccak256(abi.encodePacked(a)) == keccak256(abi.encodePacked(b));
    }
}
