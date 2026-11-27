// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import {DeployLocalScript} from "../script/DeployLocal.s.sol";
import {StylusHybridE2E} from "../script/StylusHybridE2E.s.sol";

/// @notice DeployLocal wires an engine to a verifier that accepts every proof, so it must only run on Anvil
contract DeployLocalScriptTest is Test {
    function test_deployLocal_reverts_off_anvil() public {
        DeployLocalScript script = new DeployLocalScript();
        vm.chainId(4801);
        vm.expectRevert(bytes("DeployLocal is for Anvil only"));
        script.run();
    }

    function test_deployLocal_runs_on_anvil() public {
        assertEq(block.chainid, 31337, "forge tests run on the Anvil chain id");
        vm.setEnv("PRIVATE_KEY", vm.toString(uint256(0xA11CE)));
        vm.setEnv("FEE_RECIPIENT", vm.toString(address(0xFEE)));
        DeployLocalScript script = new DeployLocalScript();
        script.run();
    }
}

/// @dev Stylus stand-in: full verification passes, the hybrid call reports failure
contract HybridFailsStylusMock {
    function verifyDagProof(bytes calldata, bytes calldata, bytes calldata, bytes calldata)
        external
        pure
        returns (bool)
    {
        return true;
    }

    function verifyDagProofHybrid(bytes calldata, bytes calldata, bytes calldata, bytes calldata)
        external
        pure
        returns (bool, bytes32, bytes memory)
    {
        return (false, bytes32(0), "");
    }
}

/// @notice The hybrid E2E must not pass when the hybrid path returns false, and must skip, not
///         revert, when no EC verifier is configured
/// @dev Only one test here sets process-wide environment variables (forge runs tests in parallel and
///      they share them); the others pass their settings to runWith directly.
contract StylusHybridE2EScriptTest is Test {
    function test_stylusHybridE2E_fails_when_hybrid_path_returns_false() public {
        HybridFailsStylusMock mock = new HybridFailsStylusMock();
        StylusHybridE2E.Config memory cfg;
        cfg.deployerKey = 0xB0B;
        cfg.stylusVerifier = address(mock);
        // Any contract with code is accepted as the EC verifier; the hybrid call fails before reaching it
        cfg.ecGroth16Verifier = address(mock);
        cfg.ecGroth16InputCount = 3;
        cfg.ecGroth16ProofFile = "test/fixtures/ec_groth16_zero_proof.json";

        StylusHybridE2E script = new StylusHybridE2E();
        vm.expectRevert(bytes("Hybrid verification failed"));
        script.runWith(cfg);
    }

    /// @dev scripts/stylus-groth16-hybrid-e2e.sh sets EC_GROTH16_PROOF_FILE to a file under
    ///      <repo>/target and no EC_GROTH16_VERIFIER. foundry.toml lets scripts read only
    ///      test/fixtures, so looking at that file would revert; the run must skip the hybrid step.
    function test_stylusHybridE2E_skips_without_ec_verifier_when_proof_file_is_outside_readable_folder() public {
        HybridFailsStylusMock mock = new HybridFailsStylusMock();
        vm.setEnv("DEPLOYER_KEY", vm.toString(uint256(0xB0B)));
        vm.setEnv("STYLUS_VERIFIER", vm.toString(address(mock)));
        // forge has no cheatcode to unset a variable; the zero address is what run() reads as "not set"
        vm.setEnv("EC_GROTH16_VERIFIER", vm.toString(address(0)));
        vm.setEnv("EC_GROTH16_PROOF_FILE", "../target/ec_groth16_proof.json");

        StylusHybridE2E script = new StylusHybridE2E();
        script.run();
    }

    function test_stylusHybridE2E_reports_hybrid_step_not_run_without_ec_verifier() public {
        HybridFailsStylusMock mock = new HybridFailsStylusMock();
        StylusHybridE2E.Config memory cfg;
        cfg.deployerKey = 0xB0B;
        cfg.stylusVerifier = address(mock);
        cfg.ecGroth16InputCount = 3;
        cfg.ecGroth16ProofFile = "../target/ec_groth16_proof.json";

        StylusHybridE2E script = new StylusHybridE2E();
        assertFalse(script.runWith(cfg), "hybrid step skipped");
    }
}
