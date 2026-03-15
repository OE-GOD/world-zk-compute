// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/remainder/RemainderVerifier.sol";
import "../src/remainder/GKRDAGVerifier.sol";
import {DAGGroth16Verifier} from "../src/remainder/DAGRemainderGroth16Verifier.sol";

/// @title DeployRemainder
/// @notice Deploy RemainderVerifier + DAGGroth16Verifier and register the XGBoost DAG circuit.
/// @dev Usage:
///   # Deploy only:
///   forge script script/DeployRemainder.s.sol:DeployRemainder \
///     --rpc-url $RPC_URL --broadcast -vvv
///
///   # Deploy + verify proof:
///   VERIFY=true forge script script/DeployRemainder.s.sol:DeployRemainder \
///     --rpc-url $RPC_URL --broadcast -vvv --gas-limit 500000000
///
///   Requires:
///     - PRIVATE_KEY env var (deployer, also becomes admin)
///     - Fixture at test/fixtures/dag_groth16_e2e_fixture.json
///     - Optional: VERIFY=true to run on-chain proof verification after deploy
contract DeployRemainder is Script {
    struct Fixture {
        bytes innerProof;
        bytes gensHex;
        bytes32 circuitHash;
        bytes publicInputsAbi;
    }

    function run() external {
        uint256 deployerKey = vm.envUint("PRIVATE_KEY");
        address deployer = vm.addr(deployerKey);
        bool shouldVerify = _envBoolOr("VERIFY", false);

        Fixture memory f = _loadFixture();
        GKRDAGVerifier.DAGCircuitDescription memory desc = _parseDesc();

        console.log("=== Remainder DAG Deployment ===");
        console.log("Deployer:", deployer);
        console.log("Compute layers:", desc.numComputeLayers);
        console.log("Verify after deploy:", shouldVerify);

        vm.startBroadcast(deployerKey);

        // 1. Deploy contracts
        RemainderVerifier verifier = new RemainderVerifier(deployer);
        DAGGroth16Verifier groth16 = new DAGGroth16Verifier();

        // 2. Register DAG circuit
        bytes32 gensHash = keccak256(f.gensHex);
        verifier.registerDAGCircuit(f.circuitHash, abi.encode(desc), "XGBoost-4tree-2depth", gensHash);

        // 3. Set Groth16 verifier (3416 public inputs for this circuit)
        verifier.setDAGCircuitGroth16Verifier(f.circuitHash, address(groth16), 3416);

        // 4. Optional: verify a proof on-chain
        if (shouldVerify) {
            _runVerification(verifier, f);
        }

        vm.stopBroadcast();

        console.log("");
        console.log("=== Deployment Summary ===");
        console.log("RemainderVerifier:", address(verifier));
        console.log("DAGGroth16Verifier:", address(groth16));
        console.log("Circuit hash:", vm.toString(f.circuitHash));
        console.log("Admin:", deployer);
    }

    function _runVerification(RemainderVerifier verifier, Fixture memory f) private view {
        string memory json = vm.readFile("test/fixtures/dag_groth16_e2e_fixture.json");

        uint256[8] memory groth16Proof;
        for (uint256 i = 0; i < 8; i++) {
            groth16Proof[i] = vm.parseJsonUint(json, string.concat(".groth16_proof[", vm.toString(i), "]"));
        }
        uint256[] memory groth16Outputs = vm.parseJsonUintArray(json, ".groth16_outputs");

        uint256 gasBefore = gasleft();
        verifier.verifyDAGWithGroth16(
            f.innerProof, f.circuitHash, f.publicInputsAbi, f.gensHex, groth16Proof, groth16Outputs
        );
        console.log("Verification gas:", gasBefore - gasleft());
        console.log("Verification: PASSED");
    }

    function _loadFixture() private view returns (Fixture memory f) {
        string memory json = vm.readFile("test/fixtures/dag_groth16_e2e_fixture.json");
        f.innerProof = vm.parseJsonBytes(json, ".inner_proof_hex");
        f.gensHex = vm.parseJsonBytes(json, ".gens_hex");
        f.circuitHash = vm.parseJsonBytes32(json, ".circuit_hash_raw");
        f.publicInputsAbi = vm.parseJsonBytes(json, ".public_values_abi");
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

    function _envBoolOr(string memory key, bool defaultVal) private view returns (bool) {
        try vm.envBool(key) returns (bool val) {
            return val;
        } catch {
            return defaultVal;
        }
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
