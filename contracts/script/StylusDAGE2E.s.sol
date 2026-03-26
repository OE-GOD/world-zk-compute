// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/remainder/RemainderVerifier.sol";
import {UUPSProxy} from "../src/Upgradeable.sol";
import "../src/remainder/GKRDAGVerifier.sol";

/// @title StylusDAGE2E
/// @notice Deploy RemainderVerifier, register DAG circuit, and verify via both
///         Stylus (delegated) and pure-Solidity paths. Compares gas usage.
/// @dev Usage:
///   DEPLOYER_KEY=0x... STYLUS_VERIFIER=0x... \
///     forge script script/StylusDAGE2E.s.sol:StylusDAGE2E \
///     --rpc-url $RPC_URL --broadcast --gas-limit 500000000 -vvv
contract StylusDAGE2E is Script {
    struct Fixture {
        bytes proofHex;
        bytes gensHex;
        bytes32 circuitHash;
        bytes publicInputsHex;
        bytes descData;
        bytes32 gensHash;
    }

    function run() external {
        uint256 deployerKey = vm.envUint("DEPLOYER_KEY");
        address stylusVerifierAddr = vm.envAddress("STYLUS_VERIFIER");

        Fixture memory fix = _loadFixture();

        console.log("Fixture loaded:");
        console.log("  Proof size:", fix.proofHex.length, "bytes");
        console.log("  Stylus verifier:", stylusVerifierAddr);

        // Deploy + register + configure
        vm.startBroadcast(deployerKey);
        RemainderVerifier verifierImpl = new RemainderVerifier();
        UUPSProxy verifierProxy =
            new UUPSProxy(address(verifierImpl), abi.encodeCall(RemainderVerifier.initialize, (vm.addr(deployerKey))));
        RemainderVerifier verifier = RemainderVerifier(address(verifierProxy));
        console.log("RemainderVerifier deployed at:", address(verifier));

        verifier.registerDAGCircuit(fix.circuitHash, fix.descData, "XGBoost-Phase1a", fix.gensHash);
        console.log("DAG circuit registered");

        verifier.setDAGStylusVerifier(fix.circuitHash, stylusVerifierAddr);
        console.log("Stylus verifier set");
        vm.stopBroadcast();

        // Verify via Stylus path
        console.log("");
        console.log("=== Stylus Verification ===");
        vm.startBroadcast(deployerKey);
        bool stylusValid =
            verifier.verifyDAGProofStylus(fix.proofHex, fix.circuitHash, fix.publicInputsHex, fix.gensHex);
        vm.stopBroadcast();
        console.log("  Result:", stylusValid);
        require(stylusValid, "Stylus verification failed");

        // Verify via pure-Solidity path
        console.log("");
        console.log("=== Solidity Verification ===");
        vm.startBroadcast(deployerKey);
        bool solidityValid = verifier.verifyDAGProof(fix.proofHex, fix.circuitHash, fix.publicInputsHex, fix.gensHex);
        vm.stopBroadcast();
        console.log("  Result:", solidityValid);
        require(solidityValid, "Solidity verification failed");

        console.log("");
        console.log("=== STYLUS DAG E2E PASSED ===");
    }

    function _loadFixture() private view returns (Fixture memory fix) {
        string memory json = vm.readFile("test/fixtures/phase1a_dag_fixture.json");
        fix.proofHex = vm.parseJsonBytes(json, ".proof_hex");
        fix.gensHex = vm.parseJsonBytes(json, ".gens_hex");
        fix.circuitHash = vm.parseJsonBytes32(json, ".circuit_hash_raw");
        fix.publicInputsHex = vm.parseJsonBytes(json, ".public_inputs_hex");
        fix.gensHash = keccak256(fix.gensHex);

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
        fix.descData = abi.encode(desc);
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
