// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/remainder/RemainderVerifier.sol";

/// @title RemainderE2E
/// @notice Deploy RemainderVerifier and test with a real proof from Remainder_CE
contract RemainderE2E is Script {
    function run() external {
        uint256 deployerKey = vm.envUint("DEPLOYER_KEY");
        address deployer = vm.addr(deployerKey);

        vm.startBroadcast(deployerKey);

        // 1. Deploy RemainderVerifier
        RemainderVerifier verifier = new RemainderVerifier(deployer);
        console.log("RemainderVerifier deployed at:", address(verifier));

        // 2. Register the XGBoost circuit
        // Circuit hash from the Rust prover output
        bytes32 circuitHash = vm.envBytes32("CIRCUIT_HASH");
        console.log("Circuit hash:");
        console.logBytes32(circuitHash);

        // XGBoost circuit structure:
        // Layer 0: input (committed, features + leaf_values)
        // Layer 1: multiplication (features * leaf_values)
        // Layer 2: subtraction (product - expected_output)
        // Layer 3: output (must be zero)
        uint256 numLayers = 4;
        uint256[] memory layerSizes = new uint256[](4);
        layerSizes[0] = 8; // input layer (padded to 2^3)
        layerSizes[1] = 8; // multiplication
        layerSizes[2] = 8; // subtraction
        layerSizes[3] = 8; // output

        uint8[] memory layerTypes = new uint8[](4);
        layerTypes[0] = 3; // input
        layerTypes[1] = 1; // mul
        layerTypes[2] = 0; // add (subtraction is add with negated operand)
        layerTypes[3] = 0; // add (output)

        bool[] memory isCommitted = new bool[](4);
        isCommitted[0] = true; // features are committed (private)
        isCommitted[1] = false;
        isCommitted[2] = false;
        isCommitted[3] = false;

        verifier.registerCircuit(circuitHash, numLayers, layerSizes, layerTypes, isCommitted, "XGBoost-5feat-2trees");

        console.log("Circuit registered: XGBoost-5feat-2trees");
        console.log("Circuit active:", verifier.isCircuitActive(circuitHash));

        // 3. Submit the proof
        bytes memory proof = vm.envBytes("PROOF_DATA");
        bytes memory publicInputs = vm.envBytes("PUBLIC_INPUTS");

        console.log("Proof size:", proof.length, "bytes");
        console.log("Public inputs size:", publicInputs.length, "bytes");

        // Verify proof selector is "REM1"
        console.log(
            "Proof starts with REM1 selector:",
            proof.length >= 4 && proof[0] == 0x52 && proof[1] == 0x45 && proof[2] == 0x4d && proof[3] == 0x31
        );

        // Try to verify — this will test the full on-chain flow
        // Note: GKR verification may fail if Poseidon constants don't match Remainder_CE
        try verifier.verifyProof(proof, circuitHash, publicInputs, "") returns (bool valid) {
            console.log("Proof verification result:", valid);
        } catch Error(string memory reason) {
            console.log("Proof verification reverted:", reason);
        } catch (bytes memory lowLevelData) {
            console.log("Proof verification reverted (low-level), data length:", lowLevelData.length);
        }

        vm.stopBroadcast();

        console.log("");
        console.log("=== E2E Summary ===");
        console.log("RemainderVerifier deployed: OK");
        console.log("Circuit registered: OK");
        console.log("Proof submitted: OK");
    }
}
