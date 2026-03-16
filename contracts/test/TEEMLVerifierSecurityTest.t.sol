// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/tee/TEEMLVerifier.sol";

/// @dev Malicious contract that attempts reentrancy on finalize
contract ReentrantFinalizer {
    TEEMLVerifier public target;
    bytes32 public targetResultId;
    uint256 public attackCount;

    constructor(address payable _target) {
        target = TEEMLVerifier(_target);
    }

    function setTarget(bytes32 _resultId) external {
        targetResultId = _resultId;
    }

    function attack() external {
        target.finalize(targetResultId);
    }

    receive() external payable {
        attackCount++;
        if (attackCount < 3) {
            // Attempt re-entry on finalize
            try target.finalize(targetResultId) {} catch {}
        }
    }
}

/// @dev Malicious contract that attempts reentrancy on dispute resolution
contract ReentrantChallenger {
    TEEMLVerifier public target;
    bytes32 public targetResultId;
    uint256 public attackCount;

    constructor(address payable _target) {
        target = TEEMLVerifier(_target);
    }

    function setTarget(bytes32 _resultId) external {
        targetResultId = _resultId;
    }

    function challengeAndWait(bytes32 resultId) external payable {
        target.challenge{value: msg.value}(resultId);
    }

    receive() external payable {
        attackCount++;
        if (attackCount < 3) {
            // Attempt re-entry on resolveDisputeByTimeout
            try target.resolveDisputeByTimeout(targetResultId) {} catch {}
        }
    }
}

/// @dev Contract that rejects ETH transfers
contract ETHRejecter {
    // No receive or fallback — will cause ETH transfers to revert

    }

/// @title TEEMLVerifierSecurityTest
/// @notice Security-focused tests: reentrancy, timing attacks, edge cases, griefing
contract TEEMLVerifierSecurityTest is Test {
    TEEMLVerifier public verifier;

    address admin = address(this);
    address submitter = address(0x1111);
    address challenger = address(0x3333);

    uint256 enclavePrivateKey = 0xA11CE;
    address enclaveKey;

    bytes32 modelHash = keccak256("model");
    bytes32 inputHash = keccak256("input");

    function setUp() public {
        enclaveKey = vm.addr(enclavePrivateKey);
        verifier = new TEEMLVerifier(admin, address(0));
        verifier.registerEnclave(enclaveKey, bytes32(uint256(0xBEEF)));

        vm.deal(submitter, 100 ether);
        vm.deal(challenger, 100 ether);
    }

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

    /// @dev Builds the EIP-712 digest for a TEEMLResult
    function _buildDigest(bytes32 _modelHash, bytes32 _inputHash, bytes32 resultHash) internal view returns (bytes32) {
        bytes32 structHash = keccak256(abi.encode(verifier.RESULT_TYPEHASH(), _modelHash, _inputHash, resultHash));
        return keccak256(abi.encodePacked("\x19\x01", _domainSeparator(), structHash));
    }

    /// @dev Helper to create a valid attestation and submit a result
    function _submitResult(address _submitter, bytes memory result) internal returns (bytes32) {
        bytes32 resultHash = keccak256(result);
        bytes32 digest = _buildDigest(modelHash, inputHash, resultHash);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, digest);
        bytes memory attestation = abi.encodePacked(r, s, v);

        vm.prank(_submitter);
        return verifier.submitResult{value: 0.1 ether}(modelHash, inputHash, result, attestation);
    }

    // ========================================================================
    // 1. Reentrancy protection
    // ========================================================================

    function test_reentrancyOnFinalize() public {
        ReentrantFinalizer attacker = new ReentrantFinalizer(payable(address(verifier)));
        vm.deal(address(attacker), 10 ether);

        // Submit result from attacker contract (so stake returns to it)
        bytes memory result = "malicious-result";
        bytes32 resultHash = keccak256(result);
        bytes32 digest = _buildDigest(modelHash, inputHash, resultHash);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, digest);
        bytes memory attestation = abi.encodePacked(r, s, v);

        vm.prank(address(attacker));
        bytes32 resultId = verifier.submitResult{value: 0.1 ether}(modelHash, inputHash, result, attestation);

        attacker.setTarget(resultId);

        // Fast forward past challenge window
        vm.warp(block.timestamp + 1 hours + 1);

        // Reentrancy attempt via receive() should be blocked by ReentrancyGuard
        attacker.attack();

        // Only one finalization should have occurred
        assertTrue(verifier.isResultValid(resultId));
        assertEq(attacker.attackCount(), 1); // receive called once, re-entry failed
    }

    function test_reentrancyOnDisputeResolution() public {
        ReentrantChallenger attacker = new ReentrantChallenger(payable(address(verifier)));
        vm.deal(address(attacker), 10 ether);

        // Submit result from submitter
        bytes32 resultId = _submitResult(submitter, "result-1");

        // Challenge from attacker contract
        attacker.setTarget(resultId);
        attacker.challengeAndWait{value: 0.1 ether}(resultId);

        // Fast forward past dispute window — challenger wins
        vm.warp(block.timestamp + 24 hours + 1);

        // Resolve — payout goes to attacker's receive() which tries re-entry
        verifier.resolveDisputeByTimeout(resultId);

        // Dispute resolved once, re-entry blocked
        assertTrue(verifier.disputeResolved(resultId));
        assertEq(attacker.attackCount(), 1);
    }

    // ========================================================================
    // 2. Timing attacks / boundary conditions
    // ========================================================================

    function test_challengeExactlyAtDeadline() public {
        bytes32 resultId = _submitResult(submitter, "result");

        ITEEMLVerifier.MLResult memory res = verifier.getResult(resultId);

        // Warp to exactly 1 second before deadline
        vm.warp(res.challengeDeadline - 1);

        // Challenge should still succeed (within window)
        vm.prank(challenger);
        verifier.challenge{value: 0.1 ether}(resultId);

        assertTrue(verifier.getResult(resultId).challenged);
    }

    function test_challengeOneSecondAfterDeadline() public {
        bytes32 resultId = _submitResult(submitter, "result");

        ITEEMLVerifier.MLResult memory res = verifier.getResult(resultId);

        // Warp to exactly at deadline (closed)
        vm.warp(res.challengeDeadline);

        vm.prank(challenger);
        vm.expectRevert("TEEMLVerifier: window closed");
        verifier.challenge{value: 0.1 ether}(resultId);
    }

    function test_finalizeExactlyAtDeadline() public {
        bytes32 resultId = _submitResult(submitter, "result");

        ITEEMLVerifier.MLResult memory res = verifier.getResult(resultId);

        // Warp to exactly at challenge deadline
        vm.warp(res.challengeDeadline);

        // Finalize should succeed (>= deadline)
        verifier.finalize(resultId);
        assertTrue(verifier.isResultValid(resultId));
    }

    function test_finalizeOneSecondBeforeDeadline() public {
        bytes32 resultId = _submitResult(submitter, "result");

        ITEEMLVerifier.MLResult memory res = verifier.getResult(resultId);

        // Warp to 1 second before deadline
        vm.warp(res.challengeDeadline - 1);

        vm.expectRevert("TEEMLVerifier: window not passed");
        verifier.finalize(resultId);
    }

    function test_disputeTimeoutExactlyAtDeadline() public {
        bytes32 resultId = _submitResult(submitter, "result");

        vm.prank(challenger);
        verifier.challenge{value: 0.1 ether}(resultId);

        ITEEMLVerifier.MLResult memory res = verifier.getResult(resultId);

        // Warp to exactly at dispute deadline
        vm.warp(res.disputeDeadline);

        // Should succeed
        verifier.resolveDisputeByTimeout(resultId);
        assertTrue(verifier.disputeResolved(resultId));
    }

    function test_disputeTimeoutBeforeDeadline() public {
        bytes32 resultId = _submitResult(submitter, "result");

        vm.prank(challenger);
        verifier.challenge{value: 0.1 ether}(resultId);

        ITEEMLVerifier.MLResult memory res = verifier.getResult(resultId);

        // Warp to 1 second before dispute deadline
        vm.warp(res.disputeDeadline - 1);

        vm.expectRevert("TEEMLVerifier: deadline not reached");
        verifier.resolveDisputeByTimeout(resultId);
    }

    // ========================================================================
    // 3. ETH handling edge cases
    // ========================================================================

    function test_overpaymentStakeStoredCorrectly() public {
        // Overpay the stake (send 1 ETH when 0.1 required)
        bytes memory result = "result";
        bytes32 resultHash = keccak256(result);
        bytes32 digest = _buildDigest(modelHash, inputHash, resultHash);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, digest);
        bytes memory attestation = abi.encodePacked(r, s, v);

        vm.prank(submitter);
        bytes32 resultId = verifier.submitResult{value: 1 ether}(modelHash, inputHash, result, attestation);

        // Full 1 ETH stored as stake
        ITEEMLVerifier.MLResult memory res = verifier.getResult(resultId);
        assertEq(res.proverStakeAmount, 1 ether);

        // Finalize returns full amount
        vm.warp(block.timestamp + 1 hours + 1);
        uint256 balBefore = submitter.balance;
        verifier.finalize(resultId);
        assertEq(submitter.balance, balBefore + 1 ether);
    }

    function test_challengerOverpaymentBondStoredCorrectly() public {
        bytes32 resultId = _submitResult(submitter, "result");

        // Challenger overpays bond
        vm.prank(challenger);
        verifier.challenge{value: 5 ether}(resultId);

        ITEEMLVerifier.MLResult memory res = verifier.getResult(resultId);
        assertEq(res.challengeBond, 5 ether);
    }

    function test_payoutToContractThatRejectsETH() public {
        ETHRejecter rejecter = new ETHRejecter();
        vm.deal(address(rejecter), 10 ether);

        // Submit from ETH-rejecting contract
        bytes memory result = "result";
        bytes32 resultHash = keccak256(result);
        bytes32 digest = _buildDigest(modelHash, inputHash, resultHash);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, digest);
        bytes memory attestation = abi.encodePacked(r, s, v);

        vm.prank(address(rejecter));
        bytes32 resultId = verifier.submitResult{value: 0.1 ether}(modelHash, inputHash, result, attestation);

        // Finalize should revert because rejecter can't receive ETH
        vm.warp(block.timestamp + 1 hours + 1);
        vm.expectRevert("TEEMLVerifier: stake return failed");
        verifier.finalize(resultId);
    }

    // ========================================================================
    // 4. Double-action prevention
    // ========================================================================

    function test_cannotFinalizeAlreadyFinalized() public {
        bytes32 resultId = _submitResult(submitter, "result");

        vm.warp(block.timestamp + 1 hours + 1);
        verifier.finalize(resultId);

        vm.expectRevert("TEEMLVerifier: already finalized");
        verifier.finalize(resultId);
    }

    function test_cannotChallengeAlreadyChallenged() public {
        bytes32 resultId = _submitResult(submitter, "result");

        vm.prank(challenger);
        verifier.challenge{value: 0.1 ether}(resultId);

        vm.prank(address(0x4444));
        vm.deal(address(0x4444), 1 ether);
        vm.expectRevert("TEEMLVerifier: already challenged");
        verifier.challenge{value: 0.1 ether}(resultId);
    }

    function test_cannotResolveAlreadyResolved() public {
        bytes32 resultId = _submitResult(submitter, "result");

        vm.prank(challenger);
        verifier.challenge{value: 0.1 ether}(resultId);

        vm.warp(block.timestamp + 24 hours + 1);
        verifier.resolveDisputeByTimeout(resultId);

        vm.expectRevert("TEEMLVerifier: already resolved");
        verifier.resolveDisputeByTimeout(resultId);
    }

    function test_cannotChallengeAfterFinalized() public {
        bytes32 resultId = _submitResult(submitter, "result");

        vm.warp(block.timestamp + 1 hours + 1);
        verifier.finalize(resultId);

        vm.prank(challenger);
        vm.expectRevert("TEEMLVerifier: already finalized");
        verifier.challenge{value: 0.1 ether}(resultId);
    }

    // ========================================================================
    // 5. Access control
    // ========================================================================

    function test_onlyOwnerCanRegisterEnclave() public {
        vm.prank(submitter);
        vm.expectRevert();
        verifier.registerEnclave(address(0x5555), bytes32(uint256(1)));
    }

    function test_onlyOwnerCanRevokeEnclave() public {
        vm.prank(submitter);
        vm.expectRevert();
        verifier.revokeEnclave(enclaveKey);
    }

    function test_onlyOwnerCanPause() public {
        vm.prank(submitter);
        vm.expectRevert();
        verifier.pause();
    }

    function test_onlyOwnerCanSetConfig() public {
        vm.prank(submitter);
        vm.expectRevert();
        verifier.setChallengeBondAmount(1 ether);

        vm.prank(submitter);
        vm.expectRevert();
        verifier.setProverStake(1 ether);

        vm.prank(submitter);
        vm.expectRevert();
        verifier.setRemainderVerifier(address(1));
    }

    // ========================================================================
    // 6. Attestation / signature edge cases
    // ========================================================================

    function test_invalidAttestationRejected() public {
        bytes memory result = "result";
        // Random invalid attestation
        bytes memory fakeAttestation = abi.encodePacked(bytes32(uint256(1)), bytes32(uint256(2)), uint8(27));

        vm.prank(submitter);
        vm.expectRevert(); // ecrecover returns non-registered address
        verifier.submitResult{value: 0.1 ether}(modelHash, inputHash, result, fakeAttestation);
    }

    function test_revokedEnclaveAttestationRejected() public {
        verifier.revokeEnclave(enclaveKey);

        bytes memory result = "result";
        bytes32 resultHash = keccak256(result);
        bytes32 digest = _buildDigest(modelHash, inputHash, resultHash);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, digest);
        bytes memory attestation = abi.encodePacked(r, s, v);

        vm.prank(submitter);
        vm.expectRevert("TEEMLVerifier: enclave revoked");
        verifier.submitResult{value: 0.1 ether}(modelHash, inputHash, result, attestation);
    }

    function test_tamperedResultRejected() public {
        bytes memory result = "result";
        // Sign with correct data
        bytes32 resultHash = keccak256(result);
        bytes32 digest = _buildDigest(modelHash, inputHash, resultHash);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, digest);
        bytes memory attestation = abi.encodePacked(r, s, v);

        // Submit with tampered result
        bytes memory tamperedResult = "tampered-result";

        vm.prank(submitter);
        vm.expectRevert(); // Hash mismatch — recovered signer won't match
        verifier.submitResult{value: 0.1 ether}(modelHash, inputHash, tamperedResult, attestation);
    }

    // ========================================================================
    // 7. Config boundary validation
    // ========================================================================

    function test_challengeBondCannotBeZero() public {
        vm.expectRevert("TEEMLVerifier: zero amount");
        verifier.setChallengeBondAmount(0);
    }

    function test_challengeBondCannotExceed100ETH() public {
        vm.expectRevert("TEEMLVerifier: amount too high");
        verifier.setChallengeBondAmount(101 ether);
    }

    function test_proverStakeCannotBeZero() public {
        vm.expectRevert("TEEMLVerifier: zero amount");
        verifier.setProverStake(0);
    }

    function test_proverStakeCannotExceed100ETH() public {
        vm.expectRevert("TEEMLVerifier: amount too high");
        verifier.setProverStake(101 ether);
    }

    function test_remainderVerifierCannotBeZero() public {
        vm.expectRevert("TEEMLVerifier: zero address");
        verifier.setRemainderVerifier(address(0));
    }

    // ========================================================================
    // 8. Dispute extension edge cases
    // ========================================================================

    function test_onlySubmitterCanExtend() public {
        bytes32 resultId = _submitResult(submitter, "result");

        vm.prank(challenger);
        verifier.challenge{value: 0.1 ether}(resultId);

        // Non-submitter cannot extend
        vm.prank(challenger);
        vm.expectRevert("TEEMLVerifier: not submitter");
        verifier.extendDisputeWindow(resultId);
    }

    function test_cannotExtendUnchallengedResult() public {
        bytes32 resultId = _submitResult(submitter, "result");

        vm.prank(submitter);
        vm.expectRevert("TEEMLVerifier: not challenged");
        verifier.extendDisputeWindow(resultId);
    }

    function test_cannotExtendResolvedDispute() public {
        bytes32 resultId = _submitResult(submitter, "result");

        vm.prank(challenger);
        verifier.challenge{value: 0.1 ether}(resultId);

        vm.warp(block.timestamp + 24 hours + 1);
        verifier.resolveDisputeByTimeout(resultId);

        vm.prank(submitter);
        vm.expectRevert("TEEMLVerifier: already resolved");
        verifier.extendDisputeWindow(resultId);
    }

    // ========================================================================
    // 9. Griefing resistance
    // ========================================================================

    function test_insufficientBondRejected() public {
        bytes32 resultId = _submitResult(submitter, "result");

        vm.prank(challenger);
        vm.expectRevert("TEEMLVerifier: insufficient bond");
        verifier.challenge{value: 0.01 ether}(resultId);
    }

    function test_insufficientStakeRejected() public {
        bytes memory result = "result";
        bytes32 resultHash = keccak256(result);
        bytes32 digest = _buildDigest(modelHash, inputHash, resultHash);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, digest);
        bytes memory attestation = abi.encodePacked(r, s, v);

        vm.prank(submitter);
        vm.expectRevert("TEEMLVerifier: insufficient stake");
        verifier.submitResult{value: 0.01 ether}(modelHash, inputHash, result, attestation);
    }

    // ========================================================================
    // 10. Pause blocking
    // ========================================================================

    function test_pauseBlocksSubmitAndChallenge() public {
        verifier.pause();

        // Submit blocked
        vm.prank(submitter);
        vm.expectRevert();
        verifier.submitResult{value: 0.1 ether}(modelHash, inputHash, "result", "attestation");

        // Unpause
        verifier.unpause();

        // Submit works again
        bytes32 resultId = _submitResult(submitter, "result");

        // Pause again
        verifier.pause();

        // Challenge blocked
        vm.prank(challenger);
        vm.expectRevert();
        verifier.challenge{value: 0.1 ether}(resultId);
    }

    function test_finalizeWorksWhilePaused() public {
        bytes32 resultId = _submitResult(submitter, "result");
        vm.warp(block.timestamp + 1 hours + 1);

        // Pause
        verifier.pause();

        // Finalize should still work (no whenNotPaused on finalize)
        verifier.finalize(resultId);
        assertTrue(verifier.isResultValid(resultId));
    }

    function test_resolveDisputeWorksWhilePaused() public {
        bytes32 resultId = _submitResult(submitter, "result");

        vm.prank(challenger);
        verifier.challenge{value: 0.1 ether}(resultId);

        vm.warp(block.timestamp + 24 hours + 1);

        // Pause
        verifier.pause();

        // Resolve should still work (no whenNotPaused on resolve)
        verifier.resolveDisputeByTimeout(resultId);
        assertTrue(verifier.disputeResolved(resultId));
    }

    // ========================================================================
    // 13. EIP-712 cross-chain replay protection
    // ========================================================================

    function test_eip712_crossChainReplayBlocked() public {
        // Deploy a second verifier on a different "chain" (different chainId)
        // by using vm.chainId to simulate a fork
        uint256 originalChainId = block.chainid;

        // Deploy verifier on chain A (current chain)
        // verifier is already deployed in setUp at originalChainId

        // Create attestation for chain A
        bytes memory result = "cross-chain-test";
        bytes32 resultHash = keccak256(result);
        bytes32 structHash = keccak256(abi.encode(verifier.RESULT_TYPEHASH(), modelHash, inputHash, resultHash));

        // Domain separator for chain A
        bytes32 domainA = keccak256(
            abi.encode(
                keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"),
                keccak256("TEEMLVerifier"),
                keccak256("1"),
                originalChainId,
                address(verifier)
            )
        );
        bytes32 digestA = keccak256(abi.encodePacked("\x19\x01", domainA, structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, digestA);
        bytes memory attestationA = abi.encodePacked(r, s, v);

        // Attestation works on chain A
        vm.prank(submitter);
        verifier.submitResult{value: 0.1 ether}(modelHash, inputHash, result, attestationA);

        // Now switch to chain B (different chainId)
        vm.chainId(originalChainId + 1);

        // Deploy a second verifier on chain B
        TEEMLVerifier verifierB = new TEEMLVerifier(admin, address(0));
        verifierB.registerEnclave(enclaveKey, bytes32(uint256(0xBEEF)));

        // Try to replay chain A's attestation on chain B -- should fail
        // because the domain separator includes chainId
        vm.prank(submitter);
        vm.expectRevert(); // ecrecover recovers wrong address due to different domain
        verifierB.submitResult{value: 0.1 ether}(modelHash, inputHash, result, attestationA);

        // Restore original chainId
        vm.chainId(originalChainId);
    }

    function test_eip712_crossContractReplayBlocked() public {
        // Deploy two verifiers on the SAME chain but at different addresses
        TEEMLVerifier verifier2 = new TEEMLVerifier(admin, address(0));
        verifier2.registerEnclave(enclaveKey, bytes32(uint256(0xBEEF)));

        // Create attestation targeting verifier (first contract)
        bytes memory result = "cross-contract-test";
        bytes32 resultHash = keccak256(result);
        bytes32 structHash = keccak256(abi.encode(verifier.RESULT_TYPEHASH(), modelHash, inputHash, resultHash));
        bytes32 domain1 = keccak256(
            abi.encode(
                keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"),
                keccak256("TEEMLVerifier"),
                keccak256("1"),
                block.chainid,
                address(verifier)
            )
        );
        bytes32 digest1 = keccak256(abi.encodePacked("\x19\x01", domain1, structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, digest1);
        bytes memory attestation1 = abi.encodePacked(r, s, v);

        // Works on verifier
        vm.prank(submitter);
        verifier.submitResult{value: 0.1 ether}(modelHash, inputHash, result, attestation1);

        // Fails on verifier2 (different contract address in domain separator)
        vm.prank(submitter);
        vm.expectRevert(); // ecrecover recovers wrong address due to different domain
        verifier2.submitResult{value: 0.1 ether}(modelHash, inputHash, result, attestation1);
    }

    function test_eip712_domainSeparatorCachingAndForkProtection() public {
        uint256 originalChainId = block.chainid;

        // Domain separator should be cached
        bytes32 ds1 = verifier.DOMAIN_SEPARATOR();
        bytes32 ds2 = verifier.DOMAIN_SEPARATOR();
        assertEq(ds1, ds2, "domain separator should be deterministic");

        // After chain fork (chainId change), domain separator should change
        vm.chainId(originalChainId + 100);
        bytes32 ds3 = verifier.DOMAIN_SEPARATOR();
        assertTrue(ds3 != ds1, "domain separator must change after chain fork");

        // Verify it matches the expected recomputed value
        bytes32 expected = keccak256(
            abi.encode(
                keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"),
                keccak256("TEEMLVerifier"),
                keccak256("1"),
                originalChainId + 100,
                address(verifier)
            )
        );
        assertEq(ds3, expected, "recomputed domain separator should match expected");

        // Restore
        vm.chainId(originalChainId);
    }

    // ========================================================================
    // 11. Non-existent result operations
    // ========================================================================

    function test_cannotChallengeNonExistentResult() public {
        vm.prank(challenger);
        vm.expectRevert("TEEMLVerifier: result not found");
        verifier.challenge{value: 0.1 ether}(bytes32(uint256(999)));
    }

    function test_cannotFinalizeNonExistentResult() public {
        vm.expectRevert("TEEMLVerifier: result not found");
        verifier.finalize(bytes32(uint256(999)));
    }

    function test_nonExistentResultNotValid() public view {
        assertFalse(verifier.isResultValid(bytes32(uint256(999))));
    }

    // ========================================================================
    // 12. Dispute payout correctness
    // ========================================================================

    function test_proverWinsDisputePayoutCorrect() public {
        bytes32 resultId = _submitResult(submitter, "result");

        vm.prank(challenger);
        verifier.challenge{value: 0.5 ether}(resultId);

        // Set a mock remainder verifier that returns true
        MockDisputeVerifier mockDispute = new MockDisputeVerifier(true);
        verifier.setRemainderVerifier(address(mockDispute));

        uint256 submitterBalBefore = submitter.balance;

        // Resolve with valid proof
        verifier.resolveDispute(resultId, "", bytes32(0), "", "");

        // Submitter gets prover stake (0.1) + challenger bond (0.5) = 0.6 ETH
        assertEq(submitter.balance, submitterBalBefore + 0.6 ether);
        assertTrue(verifier.disputeProverWon(resultId));
    }

    function test_challengerWinsDisputePayoutCorrect() public {
        bytes32 resultId = _submitResult(submitter, "result");

        vm.prank(challenger);
        verifier.challenge{value: 0.5 ether}(resultId);

        uint256 challengerBalBefore = challenger.balance;

        // Timeout = challenger wins
        vm.warp(block.timestamp + 24 hours + 1);
        verifier.resolveDisputeByTimeout(resultId);

        // Challenger gets challenger bond (0.5) + prover stake (0.1) = 0.6 ETH
        assertEq(challenger.balance, challengerBalBefore + 0.6 ether);
        assertFalse(verifier.disputeProverWon(resultId));
    }
}

/// @dev Mock verifier that returns a configurable result
contract MockDisputeVerifier {
    bool public result;

    constructor(bool _result) {
        result = _result;
    }

    function verifyDAGProof(bytes calldata, bytes32, bytes calldata, bytes calldata) external view returns (bool) {
        return result;
    }
}
