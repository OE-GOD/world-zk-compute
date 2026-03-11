// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/ProgramRegistry.sol";
import "../src/ExecutionEngine.sol";

/// @title DeployTestnetScript
/// @notice TESTNET/MAINNET DEPLOYMENT — uses the real RISC Zero verifier router
/// @dev Does NOT deploy a verifier; uses the already-deployed RiscZeroVerifierRouter
contract DeployTestnetScript is Script {
    // Known RISC Zero Verifier Router addresses
    address constant SEPOLIA_VERIFIER = 0x925d8331ddc0a1F0d96E68CF073DFE1d92b69187;
    address constant MAINNET_VERIFIER = 0x8EaB2D97Dfce405A1692a21b3ff3A172d593D319;

    function run() external {
        uint256 deployerPrivateKey = vm.envUint("PRIVATE_KEY");
        address feeRecipient = vm.envAddress("FEE_RECIPIENT");
        address verifierAddress = vm.envAddress("VERIFIER_ADDRESS");

        require(verifierAddress != address(0), "VERIFIER_ADDRESS must be set");

        console.log("=== TESTNET/MAINNET DEPLOYMENT (Real Verifier) ===");
        console.log("Using verifier at:", verifierAddress);

        vm.startBroadcast(deployerPrivateKey);

        // 1. Deploy Program Registry (deployer is admin)
        address deployer = vm.addr(deployerPrivateKey);
        ProgramRegistry registry = new ProgramRegistry(deployer);
        console.log("ProgramRegistry deployed at:", address(registry));

        // 2. Deploy Execution Engine pointing to the real verifier (deployer is admin)
        ExecutionEngine engine = new ExecutionEngine(deployer, address(registry), verifierAddress, feeRecipient);
        console.log("ExecutionEngine deployed at:", address(engine));

        vm.stopBroadcast();

        // Print summary
        console.log("\n=== Testnet Deployment Summary ===");
        console.log("Verifier (REAL):", verifierAddress);
        console.log("Registry:       ", address(registry));
        console.log("Engine:         ", address(engine));
        console.log("Fee Recipient:  ", feeRecipient);
    }
}
