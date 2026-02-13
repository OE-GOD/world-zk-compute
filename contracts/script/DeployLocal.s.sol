// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/ProgramRegistry.sol";
import "../src/ExecutionEngine.sol";
import "../src/MockRiscZeroVerifier.sol";

/// @title DeployLocalScript
/// @notice LOCAL DEPLOYMENT (Mock Verifier) — for Anvil testing only
contract DeployLocalScript is Script {
    function run() external {
        uint256 deployerPrivateKey = vm.envUint("PRIVATE_KEY");
        address feeRecipient = vm.envAddress("FEE_RECIPIENT");

        console.log("=== LOCAL DEPLOYMENT (Mock Verifier) ===");

        vm.startBroadcast(deployerPrivateKey);

        // 1. Deploy Mock Verifier (accepts all proofs — NOT for production)
        MockRiscZeroVerifier verifier = new MockRiscZeroVerifier();
        console.log("MockRiscZeroVerifier deployed at:", address(verifier));

        // 2. Deploy Program Registry
        ProgramRegistry registry = new ProgramRegistry();
        console.log("ProgramRegistry deployed at:", address(registry));

        // 3. Deploy Execution Engine
        ExecutionEngine engine = new ExecutionEngine(
            address(registry),
            address(verifier),
            feeRecipient
        );
        console.log("ExecutionEngine deployed at:", address(engine));

        vm.stopBroadcast();

        // Print summary
        console.log("\n=== Local Deployment Summary ===");
        console.log("Verifier (MOCK):", address(verifier));
        console.log("Registry:       ", address(registry));
        console.log("Engine:         ", address(engine));
        console.log("Fee Recipient:  ", feeRecipient);
    }
}
