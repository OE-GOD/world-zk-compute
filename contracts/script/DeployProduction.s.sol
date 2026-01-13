// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/ProgramRegistry.sol";
import "../src/ExecutionEngine.sol";
import "../src/RiscZeroVerifierRouter.sol";
import "../src/MockRiscZeroVerifier.sol";

/// @title DeployProduction
/// @notice Production deployment script with verifier router
contract DeployProduction is Script {
    function run() external {
        uint256 deployerPrivateKey = vm.envUint("PRIVATE_KEY");
        address feeRecipient = vm.envAddress("FEE_RECIPIENT");
        address admin = vm.envOr("ADMIN", vm.addr(deployerPrivateKey));

        // Optional: Use existing RISC Zero verifier
        address existingVerifier = vm.envOr("RISC_ZERO_VERIFIER", address(0));

        vm.startBroadcast(deployerPrivateKey);

        // 1. Deploy Verifier Router
        RiscZeroVerifierRouter router = new RiscZeroVerifierRouter(admin);
        console.log("RiscZeroVerifierRouter deployed at:", address(router));

        // 2. If no existing verifier, deploy mock for testing
        if (existingVerifier == address(0)) {
            MockRiscZeroVerifier mockVerifier = new MockRiscZeroVerifier();
            existingVerifier = address(mockVerifier);
            console.log("MockRiscZeroVerifier deployed at:", existingVerifier);
        }

        // 3. Set default verifier
        router.setDefaultVerifier(existingVerifier);
        console.log("Default verifier set to:", existingVerifier);

        // 4. Deploy Program Registry
        ProgramRegistry registry = new ProgramRegistry();
        console.log("ProgramRegistry deployed at:", address(registry));

        // 5. Deploy Execution Engine
        ExecutionEngine engine = new ExecutionEngine(
            address(registry),
            address(router),
            feeRecipient
        );
        console.log("ExecutionEngine deployed at:", address(engine));

        vm.stopBroadcast();

        // Print deployment summary
        console.log("\n========================================");
        console.log("       DEPLOYMENT SUMMARY");
        console.log("========================================");
        console.log("Network:         ", block.chainid);
        console.log("Verifier Router: ", address(router));
        console.log("Default Verifier:", existingVerifier);
        console.log("Registry:        ", address(registry));
        console.log("Engine:          ", address(engine));
        console.log("Fee Recipient:   ", feeRecipient);
        console.log("Admin:           ", admin);
        console.log("========================================\n");
    }
}

/// @title DeployWorldChain
/// @notice Deployment script specifically for World Chain
contract DeployWorldChain is Script {
    // World Chain Sepolia configuration
    uint256 constant WORLD_CHAIN_SEPOLIA = 4801;

    function run() external {
        uint256 deployerPrivateKey = vm.envUint("PRIVATE_KEY");
        address feeRecipient = vm.envAddress("FEE_RECIPIENT");

        require(block.chainid == WORLD_CHAIN_SEPOLIA, "Not World Chain Sepolia");

        vm.startBroadcast(deployerPrivateKey);

        // Deploy with mock verifier for testnet
        MockRiscZeroVerifier verifier = new MockRiscZeroVerifier();
        ProgramRegistry registry = new ProgramRegistry();
        ExecutionEngine engine = new ExecutionEngine(
            address(registry),
            address(verifier),
            feeRecipient
        );

        vm.stopBroadcast();

        console.log("=== World Chain Sepolia Deployment ===");
        console.log("Verifier:", address(verifier));
        console.log("Registry:", address(registry));
        console.log("Engine:  ", address(engine));
    }
}
