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
///     PRIVATE_KEY  — deployer private key (optional; defaults to Anvil account 0)
contract DeployMocks is Script {
    /// @dev Anvil's default account 0 private key — used when PRIVATE_KEY is not set.
    uint256 constant ANVIL_DEFAULT_KEY = 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80;

    function run() external {
        uint256 deployerKey = vm.envOr("PRIVATE_KEY", ANVIL_DEFAULT_KEY);
        address deployer = vm.addr(deployerKey);

        console.log("=== MOCK CONTRACTS DEPLOYMENT ===");
        console.log("Deployer:", deployer);
        if (deployerKey == ANVIL_DEFAULT_KEY) {
            console.log("(using Anvil default key - set PRIVATE_KEY to override)");
        }
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
