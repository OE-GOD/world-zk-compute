// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/tee/TEEMLVerifier.sol";
import {UUPSProxy} from "../src/Upgradeable.sol";

contract DeployTEEMLVerifier is Script {
    function run() external {
        uint256 deployerKey = vm.envUint("DEPLOYER_KEY");
        address deployer = vm.addr(deployerKey);

        // Use existing RemainderVerifier deployment, or address(0) if not yet deployed
        address remainderVerifier = vm.envOr("REMAINDER_VERIFIER", address(0));

        // Optional: transfer admin to a separate address after deploy
        address adminAddress = vm.envOr("ADMIN_ADDRESS", address(0));

        vm.startBroadcast(deployerKey);

        TEEMLVerifier teeImpl = new TEEMLVerifier();
        UUPSProxy teeProxy = new UUPSProxy(
            address(teeImpl), abi.encodeCall(TEEMLVerifier.initialize, (deployer, remainderVerifier))
        );
        TEEMLVerifier tee = TEEMLVerifier(payable(address(teeProxy)));
        console.log("TEEMLVerifier deployed at:", address(tee));

        // Post-deploy verification
        require(tee.admin() == deployer, "admin mismatch after deploy");
        console.log("Admin verified:", tee.admin());

        // Transfer admin if ADMIN_ADDRESS is set
        if (adminAddress != address(0)) {
            tee.changeAdmin(adminAddress);
            console.log("Admin changed to:", adminAddress);
        }

        vm.stopBroadcast();
    }
}
