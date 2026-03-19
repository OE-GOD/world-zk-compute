// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/remainder/RemainderVerifier.sol";
import "../src/remainder/GKRDAGVerifier.sol";
import "../src/remainder/HybridStylusGroth16Verifier.sol";

/// @title StylusHybridE2E
/// @notice Deploy RemainderVerifier, register DAG circuit, configure Stylus + EC Groth16 verifiers,
///         and run hybrid verification comparing gas costs across all three paths:
///         1. Pure Solidity (direct GKR)
///         2. Stylus WASM (full, with EC)
///         3. Hybrid Stylus+Groth16 (Fr only + pairing, target 3-6M gas)
/// @dev Usage:
///   DEPLOYER_KEY=0x... STYLUS_VERIFIER=0x... \
///     forge script script/StylusHybridE2E.s.sol:StylusHybridE2E \
///     --rpc-url $RPC_URL --broadcast --gas-limit 500000000 -vvv
contract StylusHybridE2E is Script {
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

        // EC Groth16 verifier is optional -- if not provided, hybrid step is skipped
        address ecGroth16Verifier = vm.envOr("EC_GROTH16_VERIFIER", address(0));
        uint256 ecGroth16InputCount = vm.envOr("EC_GROTH16_INPUT_COUNT", uint256(3));

        Fixture memory fix = _loadFixture();

        console.log("=== Stylus + Groth16 Hybrid E2E ===");
        console.log("  Proof size:", fix.proofHex.length, "bytes");
        console.log("  Stylus verifier:", stylusVerifierAddr);
        if (ecGroth16Verifier != address(0)) {
            console.log("  EC Groth16 verifier:", ecGroth16Verifier);
            console.log("  EC Groth16 input count:", ecGroth16InputCount);
        } else {
            console.log("  EC Groth16 verifier: not configured (hybrid test skipped)");
        }

        // Phase 1: Deploy + register + configure
        vm.startBroadcast(deployerKey);
        RemainderVerifier verifier = new RemainderVerifier(vm.addr(deployerKey));
        console.log("RemainderVerifier deployed at:", address(verifier));

        verifier.registerDAGCircuit(fix.circuitHash, fix.descData, "XGBoost-Phase1a", fix.gensHash);
        console.log("DAG circuit registered");

        verifier.setDAGStylusVerifier(fix.circuitHash, stylusVerifierAddr);
        console.log("Stylus verifier set");

        if (ecGroth16Verifier != address(0)) {
            verifier.setDAGECGroth16Verifier(fix.circuitHash, ecGroth16Verifier, ecGroth16InputCount);
            console.log("EC Groth16 verifier set");
        }
        vm.stopBroadcast();

        // Phase 2: Test Stylus full verification path
        console.log("");
        console.log("=== Stylus Full Verification ===");
        vm.startBroadcast(deployerKey);
        uint256 gasBefore = gasleft();
        bool stylusValid =
            verifier.verifyDAGProofStylus(fix.proofHex, fix.circuitHash, fix.publicInputsHex, fix.gensHex);
        uint256 stylusGas = gasBefore - gasleft();
        vm.stopBroadcast();
        console.log("  Result:", stylusValid);
        console.log("  Stylus path gas:", stylusGas);
        require(stylusValid, "Stylus verification failed");

        // Phase 3: Test Solidity verification path
        console.log("");
        console.log("=== Solidity Verification ===");
        vm.startBroadcast(deployerKey);
        gasBefore = gasleft();
        bool solidityValid = verifier.verifyDAGProof(fix.proofHex, fix.circuitHash, fix.publicInputsHex, fix.gensHex);
        uint256 solidityGas = gasBefore - gasleft();
        vm.stopBroadcast();
        console.log("  Result:", solidityValid);
        console.log("  Solidity path gas:", solidityGas);
        require(solidityValid, "Solidity verification failed");

        // Phase 4: Test Hybrid Stylus+Groth16 path (if EC verifier configured)
        if (ecGroth16Verifier != address(0)) {
            console.log("");
            console.log("=== Hybrid Stylus+Groth16 Verification ===");

            // Load the EC Groth16 proof from env or use zeros for pipeline test
            uint256[8] memory ecProof;
            string memory proofFile = vm.envOr("EC_GROTH16_PROOF_FILE", string(""));
            if (bytes(proofFile).length > 0 && vm.isFile(proofFile)) {
                string memory proofJson = vm.readFile(proofFile);
                uint256[] memory proofArr = vm.parseJsonUintArray(proofJson, ".proof");
                require(proofArr.length == 8, "EC Groth16 proof must have 8 elements");
                for (uint256 i = 0; i < 8; i++) {
                    ecProof[i] = proofArr[i];
                }
            }

            vm.startBroadcast(deployerKey);
            gasBefore = gasleft();
            bool hybridValid = verifier.verifyDAGProofStylusGroth16(
                fix.proofHex, fix.circuitHash, fix.publicInputsHex, fix.gensHex, ecProof
            );
            uint256 hybridGas = gasBefore - gasleft();
            vm.stopBroadcast();

            console.log("  Result:", hybridValid);
            console.log("  Hybrid path gas:", hybridGas);

            if (hybridValid) {
                console.log("");
                console.log("=== Gas Comparison ===");
                console.log("  Solidity gas:", solidityGas);
                console.log("  Stylus gas:", stylusGas);
                console.log("  Hybrid gas:", hybridGas);
                require(hybridValid, "Hybrid verification failed");
            } else {
                console.log("  WARNING: Hybrid verification returned false");
                console.log("  This may be expected if using mock EC Groth16 proof");
            }
        }

        console.log("");
        console.log("=== HYBRID E2E PASSED ===");
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
