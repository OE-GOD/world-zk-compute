// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "../../src/remainder/RemainderVerifier.sol";
import {UUPSProxy} from "../../src/Upgradeable.sol";

/// @title DeployRemainderVerifier
/// @notice Test helper for deploying RemainderVerifier behind a UUPS proxy
abstract contract DeployRemainderVerifierHelper {
    /// @notice Deploy a RemainderVerifier behind a UUPS proxy
    /// @param admin The admin address for the verifier
    /// @return verifier The RemainderVerifier instance (proxy address cast)
    function _deployRemainderVerifier(address admin) internal returns (RemainderVerifier) {
        RemainderVerifier impl = new RemainderVerifier();
        UUPSProxy proxy = new UUPSProxy(address(impl), abi.encodeCall(RemainderVerifier.initialize, (admin)));
        return RemainderVerifier(address(proxy));
    }
}
