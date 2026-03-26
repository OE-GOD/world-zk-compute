// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/remainder/RemainderVerifier.sol";
import {UUPSProxy} from "../src/Upgradeable.sol";
import "../src/remainder/GKRDAGVerifier.sol";

/// @title VerifyOnTestnet
/// @notice Verify a DAG proof on a testnet (or local Anvil) using the fixture file.
/// @dev Two modes:
///
///   Mode 1 — Verify against ALREADY-DEPLOYED contract:
///     DEPLOYER_KEY=0x... REMAINDER_VERIFIER=0x... \
///       forge script script/VerifyOnTestnet.s.sol:VerifyOnTestnet \
///         --rpc-url $RPC_URL --broadcast -vvv
///
///   Mode 2 — Deploy fresh + verify (default when REMAINDER_VERIFIER is not set):
///     DEPLOYER_KEY=0x... \
///       forge script script/VerifyOnTestnet.s.sol:VerifyOnTestnet \
///         --rpc-url $RPC_URL --broadcast -vvv --code-size-limit 200000
///
///   Optional env vars:
///     FIXTURE_PATH — path to fixture JSON (default: test/fixtures/phase1a_dag_fixture.json)
///     VERIFY_MODE  — "direct" | "batch" (default: direct)
contract VerifyOnTestnet is Script {
    uint256 constant LAYERS_PER_BATCH = 8;

    struct Fixture {
        bytes proofHex;
        bytes gensHex;
        bytes32 circuitHash;
        bytes publicInputsHex;
    }

    function run() external {
        uint256 deployerKey = vm.envUint("DEPLOYER_KEY");
        string memory verifyMode = vm.envOr("VERIFY_MODE", string("direct"));
        string memory fixturePath = vm.envOr("FIXTURE_PATH", string("test/fixtures/phase1a_dag_fixture.json"));

        console.log("=== Testnet E2E Verification ===");
        console.log("Mode:    ", verifyMode);
        console.log("Fixture: ", fixturePath);
        console.log("");

        Fixture memory f = _loadFixture(fixturePath);
        GKRDAGVerifier.DAGCircuitDescription memory desc = _parseDesc(fixturePath);

        console.log("Fixture loaded:");
        console.log("  Circuit hash:   ", vm.toString(f.circuitHash));
        console.log("  Compute layers: ", desc.numComputeLayers);
        console.log("  Input layers:   ", desc.numInputLayers);
        console.log("  Proof size:     ", f.proofHex.length, "bytes");
        console.log("  Gens size:      ", f.gensHex.length, "bytes");
        console.log("");

        vm.startBroadcast(deployerKey);

        RemainderVerifier verifier = _resolveVerifier(deployerKey, f, desc);

        if (_strEq(verifyMode, "batch")) {
            _runBatchVerify(verifier, f, desc.numComputeLayers);
        } else {
            _runDirectVerify(verifier, f);
        }

        vm.stopBroadcast();

        console.log("");
        console.log("=== Testnet E2E PASSED ===");
    }

    // ── Resolve or deploy verifier ────────────────────────────────────────────

    function _resolveVerifier(uint256 deployerKey, Fixture memory f, GKRDAGVerifier.DAGCircuitDescription memory desc)
        private
        returns (RemainderVerifier verifier)
    {
        address existingAddr = vm.envOr("REMAINDER_VERIFIER", address(0));

        if (existingAddr != address(0)) {
            verifier = RemainderVerifier(existingAddr);
            console.log("Using existing verifier:", existingAddr);

            // Check if circuit is registered
            bool isActive = verifier.isDAGCircuitActive(f.circuitHash);
            if (!isActive) {
                console.log("Circuit not registered. Registering...");
                bytes32 gensHash = keccak256(f.gensHex);
                verifier.registerDAGCircuit(f.circuitHash, abi.encode(desc), "XGBoost-Phase1a", gensHash);
                console.log("Circuit registered");
            } else {
                console.log("Circuit already registered and active");
            }
        } else {
            console.log("Deploying fresh RemainderVerifier...");
            verifier = _deployFresh(deployerKey, f, desc);
        }
    }

    function _deployFresh(uint256 deployerKey, Fixture memory f, GKRDAGVerifier.DAGCircuitDescription memory desc)
        private
        returns (RemainderVerifier verifier)
    {
        RemainderVerifier verifierImpl = new RemainderVerifier();
        UUPSProxy verifierProxy =
            new UUPSProxy(address(verifierImpl), abi.encodeCall(RemainderVerifier.initialize, (vm.addr(deployerKey))));
        verifier = RemainderVerifier(address(verifierProxy));
        console.log("  Implementation:", address(verifierImpl));
        console.log("  Proxy:         ", address(verifier));

        bytes32 gensHash = keccak256(f.gensHex);
        verifier.registerDAGCircuit(f.circuitHash, abi.encode(desc), "XGBoost-Phase1a", gensHash);
        console.log("  Circuit registered");
    }

    // ── Direct verification ──────────────────────────────────────────────────

    function _runDirectVerify(RemainderVerifier verifier, Fixture memory f) private {
        console.log("Running direct DAG verification...");
        uint256 gasBefore = gasleft();
        bool valid = verifier.verifyDAGProof(f.proofHex, f.circuitHash, f.publicInputsHex, f.gensHex);
        uint256 gasUsed = gasBefore - gasleft();

        console.log("  Result:   ", valid ? "VALID" : "INVALID");
        console.log("  Gas used: ", gasUsed);

        require(valid, "Direct verification failed");
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

        // Step 3: Finalize
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
