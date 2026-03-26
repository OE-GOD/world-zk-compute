// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "../../src/tee/TEEMLVerifier.sol";
import {UUPSProxy} from "../../src/Upgradeable.sol";

/// @title DeployTEEMLVerifierHelper
/// @notice Test helper for deploying TEEMLVerifier behind a UUPS proxy
abstract contract DeployTEEMLVerifierHelper {
    /// @notice Deploy a TEEMLVerifier behind a UUPS proxy
    /// @param admin The admin address for the verifier
    /// @param remainderVerifier The RemainderVerifier address for dispute resolution
    /// @return verifier The TEEMLVerifier instance (proxy address cast)
    function _deployTEEMLVerifier(address admin, address remainderVerifier) internal returns (TEEMLVerifier) {
        TEEMLVerifier impl = new TEEMLVerifier();
        UUPSProxy proxy =
            new UUPSProxy(address(impl), abi.encodeCall(TEEMLVerifier.initialize, (admin, remainderVerifier)));
        return TEEMLVerifier(payable(address(proxy)));
    }
}
