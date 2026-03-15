// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/mocks/MockExecutionEngine.sol";
import "../src/MockRiscZeroVerifier.sol";

/// @title DeployMocks
/// @notice Deploy all mock contracts for local testing and SDK development.
/// @dev Usage:
///   forge script script/DeployMocks.s.sol:DeployMocks \
///     --rpc-url http://localhost:8545 --broadcast -vvv
///
///   Env vars:
///     PRIVATE_KEY  — deployer private key (required)
contract DeployMocks is Script {
    function run() external {
        uint256 deployerKey = vm.envUint("PRIVATE_KEY");
        address deployer = vm.addr(deployerKey);

        console.log("=== MOCK CONTRACTS DEPLOYMENT ===");
        console.log("Deployer:", deployer);
        console.log("");

        vm.startBroadcast(deployerKey);

        // 1. Deploy MockExecutionEngine
        MockExecutionEngine mockEngine = new MockExecutionEngine();
        console.log("[1/2] MockExecutionEngine:", address(mockEngine));

        // 2. Deploy MockRiscZeroVerifier
        MockRiscZeroVerifier mockVerifier = new MockRiscZeroVerifier();
        console.log("[2/2] MockRiscZeroVerifier:", address(mockVerifier));

        vm.stopBroadcast();

        // Summary
        console.log("");
        console.log("=== MOCK DEPLOYMENT COMPLETE ===");
        console.log("");
        console.log("Deployed Addresses:");
        console.log("  MockExecutionEngine: ", address(mockEngine));
        console.log("  MockRiscZeroVerifier:", address(mockVerifier));
        console.log("");
        console.log("These contracts are for testing only. Do NOT use in production.");
    }
}
