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

        vm.startBroadcast(deployerKey);

        TEEMLVerifier tee = new TEEMLVerifier(deployer, remainderVerifier);
        console.log("TEEMLVerifier deployed at:", address(tee));

        vm.stopBroadcast();
    }
}
