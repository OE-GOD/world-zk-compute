// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/tee/TEEMLVerifier.sol";

contract DeployTEEMLVerifier is Script {
    function run() external {
        uint256 deployerKey = vm.envUint("DEPLOYER_KEY");
        address deployer = vm.addr(deployerKey);

        // Use existing RemainderVerifier deployment, or address(0) if not yet deployed
        address remainderVerifier = vm.envOr("REMAINDER_VERIFIER", address(0));

        // Optional: transfer ownership to a separate admin address after deploy
        address adminAddress = vm.envOr("ADMIN_ADDRESS", address(0));

        vm.startBroadcast(deployerKey);

        TEEMLVerifier tee = new TEEMLVerifier(deployer, remainderVerifier);
        console.log("TEEMLVerifier deployed at:", address(tee));

        // Post-deploy verification
        require(tee.owner() == deployer, "owner mismatch after deploy");
        console.log("Owner verified:", tee.owner());

        // Initiate 2-step ownership transfer if ADMIN_ADDRESS is set
        if (adminAddress != address(0)) {
            tee.transferOwnership(adminAddress);
            console.log("Ownership transfer initiated to:", adminAddress);
            console.log("Pending owner:", tee.pendingOwner());
            console.log("NOTE: Admin must call acceptOwnership() to complete transfer");
        }

        vm.stopBroadcast();
    }
}
