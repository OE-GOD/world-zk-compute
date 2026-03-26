// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/tee/TEEMLVerifier.sol";
import {UUPSUpgradeable} from "../src/Upgradeable.sol";
import {DeployTEEMLVerifierHelper} from "./helpers/DeployTEEMLVerifier.sol";

/// @dev Mock RemainderVerifier that returns a configurable result for verifyDAGProof
contract FuzzMockRemainderVerifier {
    bool public nextResult;

    function setResult(bool _result) external {
        nextResult = _result;
    }

    function verifyDAGProof(bytes calldata, bytes32, bytes calldata, bytes calldata) external view returns (bool) {
        return nextResult;
    }
}

contract TEEMLVerifierFuzzTest is Test, DeployTEEMLVerifierHelper {
    TEEMLVerifier verifier;
    FuzzMockRemainderVerifier mockVerifier;

    address admin = address(this);
    uint256 enclavePrivateKey = 0xA11CE;
    address enclaveAddr;
    bytes32 imageHash = keccak256("test-enclave-image-v1");

    bytes32 modelHash = keccak256("xgboost-model-weights");
    bytes32 inputHash = keccak256("test-input-data");
    bytes resultData = hex"deadbeef";

    uint256 constant DEFAULT_PROVER_STAKE = 0.1 ether;
    uint256 constant DEFAULT_CHALLENGE_BOND = 0.1 ether;

    // Allow test contract to receive ETH (submitter payouts)
    receive() external payable {}

    function setUp() public {
        enclaveAddr = vm.addr(enclavePrivateKey);
        mockVerifier = new FuzzMockRemainderVerifier();
        verifier = _deployTEEMLVerifier(admin, address(mockVerifier));

        // Register the test enclave
        verifier.registerEnclave(enclaveAddr, imageHash);

        // Fund the test contract generously for fuzz runs
        vm.deal(address(this), 1000 ether);
    }

    // ---- Helpers ----

    /// @dev Builds the EIP-712 domain separator matching the verifier contract
    function _domainSeparator() internal view returns (bytes32) {
        return keccak256(
            abi.encode(
                keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"),
                keccak256("TEEMLVerifier"),
                keccak256("1"),
                block.chainid,
                address(verifier)
            )
        );
    }

    function _signAttestation(bytes32 _modelHash, bytes32 _inputHash, bytes memory _result)
        internal
        view
        returns (bytes memory attestation)
    {
        bytes32 resultHash = keccak256(_result);
        bytes32 structHash = keccak256(abi.encode(verifier.RESULT_TYPEHASH(), _modelHash, _inputHash, resultHash));
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", _domainSeparator(), structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, digest);
        attestation = abi.encodePacked(r, s, v);
    }

    function _submitDefault() internal returns (bytes32 resultId) {
        bytes memory attestation = _signAttestation(modelHash, inputHash, resultData);
        resultId = verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, inputHash, resultData, attestation);
    }

    // ---- Fuzz Test 1: submitResult with variable stake ----

    /// @notice Fuzz submitResult with random stake values in [0, 10 ether].
    ///         Below proverStake (0.1 ether) should revert "insufficient stake".
    ///         At or above should succeed and store the full msg.value.
    function testFuzz_submitResult_anyStake(uint256 stake) public {
        stake = bound(stake, 0, 10 ether);

        bytes memory attestation = _signAttestation(modelHash, inputHash, resultData);

        if (stake < DEFAULT_PROVER_STAKE) {
            vm.expectRevert(ITEEMLVerifier.InsufficientStake.selector);
            verifier.submitResult{value: stake}(modelHash, inputHash, resultData, attestation);
        } else {
            bytes32 resultId = verifier.submitResult{value: stake}(modelHash, inputHash, resultData, attestation);

            ITEEMLVerifier.MLResult memory r = verifier.getResult(resultId);
            assertEq(r.submitter, address(this), "submitter should be this contract");
            assertEq(r.proverStakeAmount, stake, "full msg.value should be stored as proverStakeAmount");
            assertEq(r.enclave, enclaveAddr, "enclave should match registered key");
            assertFalse(r.finalized, "result should not be finalized immediately");
            assertFalse(r.challenged, "result should not be challenged immediately");
        }
    }

    // ---- Fuzz Test 2: challenge with variable bond ----

    /// @notice Fuzz challenge with random bond amounts in [0, 10 ether].
    ///         Below challengeBondAmount (0.1 ether) should revert "insufficient bond".
    ///         At or above should succeed.
    function testFuzz_challenge_anyBond(uint256 bond) public {
        bond = bound(bond, 0, 10 ether);

        bytes32 resultId = _submitDefault();

        address challenger = address(0xC0FFEE);
        vm.deal(challenger, bond);
        vm.prank(challenger);

        if (bond < DEFAULT_CHALLENGE_BOND) {
            vm.expectRevert(ITEEMLVerifier.InsufficientBond.selector);
            verifier.challenge{value: bond}(resultId);
        } else {
            verifier.challenge{value: bond}(resultId);

            ITEEMLVerifier.MLResult memory r = verifier.getResult(resultId);
            assertTrue(r.challenged, "result should be challenged");
            assertEq(r.challenger, challenger, "challenger address should be stored");
            assertEq(r.challengeBond, bond, "full bond amount should be stored");
            assertEq(r.disputeDeadline, block.timestamp + 24 hours, "dispute deadline should be set");
        }
    }

    // ---- Fuzz Test 3: finalize with random (non-existent) resultId ----

    /// @notice Fuzz finalize with random resultId values.
    ///         Any non-existent resultId should revert with "result not found"
    ///         because submittedAt == 0.
    function testFuzz_finalize_randomResultId(bytes32 resultId) public {
        // Warp past challenge window so "window not passed" is not the revert reason
        vm.warp(block.timestamp + 2 hours);

        vm.expectRevert(ITEEMLVerifier.ResultNotFound.selector);
        verifier.finalize(resultId);
    }

    // ---- Fuzz Test 4: resolveDispute with random proof data ----

    /// @notice Fuzz resolveDispute with random proof data.
    ///         Submit and challenge first, then resolve with arbitrary proof bytes.
    ///         MockVerifier returns false, so challenger should win the full pot.
    function testFuzz_resolveDispute_randomProof(bytes calldata proof) public {
        bytes32 resultId = _submitDefault();

        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);

        // Mock verifier returns false for any proof data
        mockVerifier.setResult(false);

        uint256 challengerBalBefore = challenger.balance;
        verifier.resolveDispute(resultId, proof, bytes32(0), hex"", hex"");

        // Dispute should resolve gracefully -- challenger wins
        assertTrue(verifier.disputeResolved(resultId), "dispute should be resolved");
        assertFalse(verifier.disputeProverWon(resultId), "prover should not win with invalid proof");
        assertEq(
            challenger.balance,
            challengerBalBefore + DEFAULT_PROVER_STAKE + DEFAULT_CHALLENGE_BOND,
            "challenger should receive full pot"
        );
    }

    // ---- Fuzz Test 5: setChallengeBondAmount bounds ----

    /// @notice Fuzz setChallengeBondAmount with random amounts.
    ///         If amount == 0: revert "zero amount".
    ///         If amount > 100 ether: revert "amount too high".
    ///         Otherwise: succeed and update state.
    function testFuzz_setChallengeBondAmount_bounds(uint256 amount) public {
        if (amount == 0) {
            vm.expectRevert(ITEEMLVerifier.ZeroAmount.selector);
            verifier.setChallengeBondAmount(amount);
        } else if (amount > 100 ether) {
            vm.expectRevert(ITEEMLVerifier.AmountTooHigh.selector);
            verifier.setChallengeBondAmount(amount);
        } else {
            verifier.setChallengeBondAmount(amount);
            assertEq(verifier.challengeBondAmount(), amount, "challengeBondAmount should be updated");
        }
    }

    /// @notice Non-admin calling setChallengeBondAmount should always revert
    ///         with NotAdmin regardless of the amount.
    function testFuzz_setChallengeBondAmount_nonOwner(uint256 amount, address caller) public {
        vm.assume(caller != admin);
        vm.assume(caller != address(0));

        vm.prank(caller);
        vm.expectRevert(UUPSUpgradeable.NotAdmin.selector);
        verifier.setChallengeBondAmount(amount);
    }

    // ---- Fuzz Test 6: setProverStake bounds ----

    /// @notice Fuzz setProverStake with random amounts.
    ///         If amount == 0: revert "zero amount".
    ///         If amount > 100 ether: revert "amount too high".
    ///         Otherwise: succeed and update state.
    function testFuzz_setProverStake_bounds(uint256 amount) public {
        if (amount == 0) {
            vm.expectRevert(ITEEMLVerifier.ZeroAmount.selector);
            verifier.setProverStake(amount);
        } else if (amount > 100 ether) {
            vm.expectRevert(ITEEMLVerifier.AmountTooHigh.selector);
            verifier.setProverStake(amount);
        } else {
            verifier.setProverStake(amount);
            assertEq(verifier.proverStake(), amount, "proverStake should be updated");
        }
    }

    /// @notice Non-admin calling setProverStake should always revert
    ///         with NotAdmin regardless of the amount.
    function testFuzz_setProverStake_nonOwner(uint256 amount, address caller) public {
        vm.assume(caller != admin);
        vm.assume(caller != address(0));

        vm.prank(caller);
        vm.expectRevert(UUPSUpgradeable.NotAdmin.selector);
        verifier.setProverStake(amount);
    }

    // ---- Fuzz Test 7: challenge after window closed ----

    /// @notice Submit a result, then warp past the challenge deadline by a fuzzed delay.
    ///         The challenge check uses strict less-than (block.timestamp < challengeDeadline),
    ///         so warping to exactly the deadline or beyond always reverts "window closed".
    function testFuzz_challenge_afterWindowClosed(uint256 delay) public {
        delay = bound(delay, 0, 365 days);

        bytes32 resultId = _submitDefault();

        // challengeDeadline = block.timestamp + challengeWindow
        uint256 challengeDeadline = block.timestamp + verifier.challengeWindow();
        vm.warp(challengeDeadline + delay);

        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);

        vm.expectRevert(ITEEMLVerifier.ChallengeWindowClosed.selector);
        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);
    }

    // ---- Fuzz Test 8: finalize before window passed ----

    /// @notice Submit a result, then warp to a random time strictly before the challenge deadline.
    ///         Finalize requires block.timestamp >= challengeDeadline, so it should always
    ///         revert with "window not passed".
    function testFuzz_finalize_beforeWindowPassed(uint256 elapsed) public {
        uint256 submitTime = block.timestamp;
        bytes32 resultId = _submitDefault();

        // challengeDeadline = submitTime + challengeWindow
        // We want to stay strictly before the deadline, so bound to [0, challengeWindow - 1].
        uint256 cw = verifier.challengeWindow();
        elapsed = bound(elapsed, 0, cw - 1);
        vm.warp(submitTime + elapsed);

        vm.expectRevert(ITEEMLVerifier.ChallengeWindowNotPassed.selector);
        verifier.finalize(resultId);
    }
}
