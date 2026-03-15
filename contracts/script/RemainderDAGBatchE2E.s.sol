// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/remainder/RemainderVerifier.sol";
import "../src/remainder/GKRDAGVerifier.sol";

/// @title RemainderDAGBatchE2E
/// @notice Deploy and run DAG batch verification (multi-tx) on Anvil.
/// @dev Usage:
///   forge script script/RemainderDAGBatchE2E.s.sol:RemainderDAGBatchE2E \
///     --rpc-url http://localhost:8545 -vvv --broadcast --gas-limit 500000000
///   Requires: DEPLOYER_KEY env var, fixture at test/fixtures/dag_groth16_e2e_fixture.json
contract RemainderDAGBatchE2E is Script {
    uint256 constant LAYERS_PER_BATCH = 8;

    struct Fixture {
        bytes innerProof;
        bytes gensHex;
        bytes32 circuitHash;
        bytes publicInputsAbi;
    }

    function run() external {
        uint256 deployerKey = vm.envUint("DEPLOYER_KEY");

        Fixture memory f = _loadFixture();
        GKRDAGVerifier.DAGCircuitDescription memory desc = _parseDesc();

        console.log("=== DAG Batch E2E ===");
        console.log("Compute layers:", desc.numComputeLayers);

        vm.startBroadcast(deployerKey);

        RemainderVerifier verifier = _deploy(deployerKey, f, desc);
        _runBatchVerify(verifier, f, desc.numComputeLayers);

        vm.stopBroadcast();

        console.log("=== DAG Batch E2E PASSED ===");
    }

    function _loadFixture() private view returns (Fixture memory f) {
        string memory json = vm.readFile("test/fixtures/dag_groth16_e2e_fixture.json");
        f.innerProof = vm.parseJsonBytes(json, ".inner_proof_hex");
        f.gensHex = vm.parseJsonBytes(json, ".gens_hex");
        f.circuitHash = vm.parseJsonBytes32(json, ".circuit_hash_raw");
        f.publicInputsAbi = vm.parseJsonBytes(json, ".public_values_abi");
    }

    function _deploy(uint256 deployerKey, Fixture memory f, GKRDAGVerifier.DAGCircuitDescription memory desc)
        private
        returns (RemainderVerifier verifier)
    {
        verifier = new RemainderVerifier(vm.addr(deployerKey));
        console.log("RemainderVerifier:", address(verifier));

        bytes32 gensHash = keccak256(f.gensHex);
        verifier.registerDAGCircuit(f.circuitHash, abi.encode(desc), "XGBoost-DAG-Batch", gensHash);
        console.log("Circuit registered");
    }

    function _runBatchVerify(RemainderVerifier verifier, Fixture memory f, uint256 numComputeLayers) private {
        uint256 gasBefore = gasleft();
        bytes32 sessionId = verifier.startDAGBatchVerify(f.innerProof, f.circuitHash, f.publicInputsAbi, f.gensHex);
        console.log("Start gas:", gasBefore - gasleft());

        uint256 numBatches = (numComputeLayers + LAYERS_PER_BATCH - 1) / LAYERS_PER_BATCH;

        for (uint256 b = 0; b < numBatches; b++) {
            gasBefore = gasleft();
            verifier.continueDAGBatchVerify(sessionId, f.innerProof, f.publicInputsAbi, f.gensHex);
            console.log("  Continue gas:", gasBefore - gasleft());
        }

        uint256 finalizeCount = 0;
        bool done = false;
        while (!done) {
            gasBefore = gasleft();
            done = verifier.finalizeDAGBatchVerify(sessionId, f.innerProof, f.publicInputsAbi, f.gensHex);
            console.log("  Finalize gas:", gasBefore - gasleft());
            finalizeCount++;
        }

        verifier.cleanupDAGBatchSession(sessionId);
        console.log("Total txs:", 1 + numBatches + finalizeCount);
    }

    function _parseDesc() private view returns (GKRDAGVerifier.DAGCircuitDescription memory desc) {
        string memory json = vm.readFile("test/fixtures/dag_groth16_e2e_fixture.json");
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
}
