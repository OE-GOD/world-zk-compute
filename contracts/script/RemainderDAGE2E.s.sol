// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/remainder/RemainderVerifier.sol";
import {UUPSProxy} from "../src/Upgradeable.sol";
import "../src/remainder/GKRDAGVerifier.sol";

/// @title RemainderDAGE2E
/// @notice Deploy RemainderVerifier and verify a DAG proof from a fixture file.
/// @dev Usage: forge script script/RemainderDAGE2E.s.sol:RemainderDAGE2E --rpc-url $RPC -vvv --broadcast
///      Requires: DEPLOYER_KEY env var, fixture at test/fixtures/phase1a_dag_fixture.json
contract RemainderDAGE2E is Script {
    function run() external {
        uint256 deployerKey = vm.envUint("DEPLOYER_KEY");

        // Load fixture
        string memory json = vm.readFile("test/fixtures/phase1a_dag_fixture.json");
        bytes memory proofHex = vm.parseJsonBytes(json, ".proof_hex");
        bytes memory gensHex = vm.parseJsonBytes(json, ".gens_hex");
        bytes32 circuitHash = vm.parseJsonBytes32(json, ".circuit_hash_raw");
        bytes memory publicInputsHex = vm.parseJsonBytes(json, ".public_inputs_hex");

        GKRDAGVerifier.DAGCircuitDescription memory desc;
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

        console.log("Fixture loaded:");
        console.log("  Compute layers:", desc.numComputeLayers);
        console.log("  Input layers:", desc.numInputLayers);
        console.log("  Proof size:", proofHex.length, "bytes");
        console.log("  Gens size:", gensHex.length, "bytes");

        vm.startBroadcast(deployerKey);

        // 1. Deploy
        RemainderVerifier verifierImpl = new RemainderVerifier();
        UUPSProxy verifierProxy =
            new UUPSProxy(address(verifierImpl), abi.encodeCall(RemainderVerifier.initialize, (vm.addr(deployerKey))));
        RemainderVerifier verifier = RemainderVerifier(address(verifierProxy));
        console.log("RemainderVerifier deployed at:", address(verifier));

        // 2. Register DAG circuit
        bytes32 gensHash = keccak256(gensHex);
        bytes memory descData = abi.encode(desc);
        verifier.registerDAGCircuit(circuitHash, descData, "XGBoost-Phase1a", gensHash);
        console.log("DAG circuit registered");

        // 3. Verify proof
        bool valid = verifier.verifyDAGProof(proofHex, circuitHash, publicInputsHex, gensHex);
        console.log("Proof verification result:", valid);

        vm.stopBroadcast();

        require(valid, "Proof verification failed");
        console.log("");
        console.log("=== DAG E2E PASSED ===");
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
