// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/tee/TEEMLVerifier.sol";

/// @dev Mock RemainderVerifier that returns a configurable result for verifyDAGProof
contract MockRemainderVerifier {
    bool public nextResult;

    function setResult(bool _result) external {
        nextResult = _result;
    }

    function verifyDAGProof(bytes calldata, bytes32, bytes calldata, bytes calldata) external view returns (bool) {
        return nextResult;
    }
}

contract TEEMLVerifierTest is Test {
    // Re-declare events for expectEmit (solc 0.8.20 compat)
    event ResultSubmitted(bytes32 indexed resultId, bytes32 modelHash, bytes32 inputHash);
    event ResultChallenged(bytes32 indexed resultId, address challenger);
    event DisputeResolved(bytes32 indexed resultId, bool proverWon);
    event ResultFinalized(bytes32 indexed resultId);

    TEEMLVerifier verifier;
    MockRemainderVerifier mockVerifier;

    address admin = address(this);
    uint256 enclavePrivateKey = 0xA11CE;
    address enclaveAddr;
    bytes32 imageHash = keccak256("test-enclave-image-v1");

    bytes32 modelHash = keccak256("xgboost-model-weights");
    bytes32 inputHash = keccak256("test-input-data");
    bytes resultData = hex"deadbeef";

    // Default amounts matching contract defaults
    uint256 constant DEFAULT_PROVER_STAKE = 0.1 ether;
    uint256 constant DEFAULT_CHALLENGE_BOND = 0.1 ether;

    // Allow test contract to receive ETH (admin / submitter payouts)
    receive() external payable {}

    function setUp() public {
        enclaveAddr = vm.addr(enclavePrivateKey);
        mockVerifier = new MockRemainderVerifier();
        verifier = new TEEMLVerifier(admin, address(mockVerifier));

        // Register the test enclave
        verifier.registerEnclave(enclaveAddr, imageHash);
    }

    // ─── Helpers ─────────────────────────────────────────────────────────────

    function _signAttestation(bytes32 _modelHash, bytes32 _inputHash, bytes memory _result)
        internal
        view
        returns (bytes memory attestation)
    {
        bytes32 resultHash = keccak256(_result);
        bytes32 message = keccak256(abi.encodePacked(_modelHash, _inputHash, resultHash));
        bytes32 ethSignedHash = keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", message));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, ethSignedHash);
        attestation = abi.encodePacked(r, s, v);
    }

    function _submitDefault() internal returns (bytes32 resultId) {
        bytes memory attestation = _signAttestation(modelHash, inputHash, resultData);
        resultId = verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, inputHash, resultData, attestation);
    }

    // ─── Happy Path: Submit ──────────────────────────────────────────────────

    function test_submitResult_validAttestation() public {
        bytes memory attestation = _signAttestation(modelHash, inputHash, resultData);

        vm.expectEmit(true, false, false, true);
        emit ResultSubmitted(
            keccak256(abi.encodePacked(address(this), modelHash, inputHash, block.number)), modelHash, inputHash
        );

        bytes32 resultId =
            verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, inputHash, resultData, attestation);

        ITEEMLVerifier.MLResult memory r = verifier.getResult(resultId);
        assertEq(r.enclave, enclaveAddr);
        assertEq(r.submitter, address(this));
        assertEq(r.modelHash, modelHash);
        assertEq(r.inputHash, inputHash);
        assertEq(r.resultHash, keccak256(resultData));
        assertEq(r.result, resultData);
        assertEq(r.proverStakeAmount, DEFAULT_PROVER_STAKE);
        assertFalse(r.finalized);
        assertFalse(r.challenged);
        assertEq(r.challengeDeadline, block.timestamp + 1 hours);
    }

    function test_submitResult_invalidSignature() public {
        uint256 wrongKey = 0xBAD;
        bytes32 resultHash = keccak256(resultData);
        bytes32 message = keccak256(abi.encodePacked(modelHash, inputHash, resultHash));
        bytes32 ethSignedHash = keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", message));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(wrongKey, ethSignedHash);
        bytes memory attestation = abi.encodePacked(r, s, v);

        vm.expectRevert("TEEMLVerifier: unregistered enclave");
        verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, inputHash, resultData, attestation);
    }

    function test_submitResult_unregisteredEnclave() public {
        uint256 unknownKey = 0xDEAD;
        bytes32 resultHash = keccak256(resultData);
        bytes32 message = keccak256(abi.encodePacked(modelHash, inputHash, resultHash));
        bytes32 ethSignedHash = keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", message));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(unknownKey, ethSignedHash);
        bytes memory attestation = abi.encodePacked(r, s, v);

        vm.expectRevert("TEEMLVerifier: unregistered enclave");
        verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, inputHash, resultData, attestation);
    }

    function test_submitResult_revokedEnclave() public {
        verifier.revokeEnclave(enclaveAddr);

        bytes memory attestation = _signAttestation(modelHash, inputHash, resultData);
        vm.expectRevert("TEEMLVerifier: enclave revoked");
        verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, inputHash, resultData, attestation);
    }

    // ─── Happy Path: Finalize ────────────────────────────────────────────────

    function test_finalize_afterWindow() public {
        uint256 balBefore = address(this).balance;
        bytes32 resultId = _submitDefault();

        vm.warp(block.timestamp + 1 hours + 1);

        vm.expectEmit(true, false, false, false);
        emit ResultFinalized(resultId);

        verifier.finalize(resultId);

        ITEEMLVerifier.MLResult memory r = verifier.getResult(resultId);
        assertTrue(r.finalized);
        assertTrue(verifier.isResultValid(resultId));
        // Prover stake returned on finalize
        assertEq(address(this).balance, balBefore);
    }

    function test_finalize_beforeWindow() public {
        bytes32 resultId = _submitDefault();

        vm.expectRevert("TEEMLVerifier: window not passed");
        verifier.finalize(resultId);
    }

    function test_getResult_afterFinalize() public {
        bytes32 resultId = _submitDefault();
        vm.warp(block.timestamp + 1 hours + 1);
        verifier.finalize(resultId);

        ITEEMLVerifier.MLResult memory r = verifier.getResult(resultId);
        assertTrue(r.finalized);
        assertEq(r.enclave, enclaveAddr);
        assertEq(r.result, resultData);
    }

    function test_isResultValid_notFinalized() public {
        bytes32 resultId = _submitDefault();
        assertFalse(verifier.isResultValid(resultId));
    }

    // ─── Challenge ───────────────────────────────────────────────────────────

    function test_challenge_withinWindow() public {
        bytes32 resultId = _submitDefault();

        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);

        vm.expectEmit(true, false, false, true);
        emit ResultChallenged(resultId, challenger);

        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);

        ITEEMLVerifier.MLResult memory r = verifier.getResult(resultId);
        assertTrue(r.challenged);
        assertEq(r.challenger, challenger);
        assertEq(r.challengeBond, DEFAULT_CHALLENGE_BOND);
        assertEq(r.disputeDeadline, block.timestamp + 24 hours);
    }

    function test_challenge_afterWindow() public {
        bytes32 resultId = _submitDefault();
        vm.warp(block.timestamp + 1 hours + 1);

        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        vm.expectRevert("TEEMLVerifier: window closed");
        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);
    }

    function test_challenge_insufficientBond() public {
        bytes32 resultId = _submitDefault();

        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        vm.expectRevert("TEEMLVerifier: insufficient bond");
        verifier.challenge{value: 0.01 ether}(resultId);
    }

    function test_challenge_alreadyChallenged() public {
        bytes32 resultId = _submitDefault();

        address challenger1 = address(0xC0FFEE);
        vm.deal(challenger1, 1 ether);
        vm.prank(challenger1);
        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);

        address challenger2 = address(0xBEEF);
        vm.deal(challenger2, 1 ether);
        vm.prank(challenger2);
        vm.expectRevert("TEEMLVerifier: already challenged");
        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);
    }

    function test_challenge_alreadyFinalized() public {
        bytes32 resultId = _submitDefault();
        vm.warp(block.timestamp + 1 hours + 1);
        verifier.finalize(resultId);

        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        vm.expectRevert("TEEMLVerifier: already finalized");
        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);
    }

    // ─── Dispute Resolution ──────────────────────────────────────────────────

    function test_resolveDispute_proverWins() public {
        bytes32 resultId = _submitDefault();

        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);

        // Mock: ZK proof verifies → prover was honest
        mockVerifier.setResult(true);
        uint256 submitterBalBefore = address(this).balance;

        vm.expectEmit(true, false, false, true);
        emit DisputeResolved(resultId, true);

        verifier.resolveDispute(resultId, hex"", bytes32(0), hex"", hex"");

        assertTrue(verifier.disputeResolved(resultId));
        assertTrue(verifier.disputeProverWon(resultId));
        assertTrue(verifier.isResultValid(resultId));
        // Submitter gets proverStake + challengeBond
        assertEq(address(this).balance, submitterBalBefore + DEFAULT_PROVER_STAKE + DEFAULT_CHALLENGE_BOND);
    }

    function test_resolveDispute_challengerWins() public {
        bytes32 resultId = _submitDefault();

        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);

        // Mock: ZK proof fails → prover was dishonest
        mockVerifier.setResult(false);
        uint256 challengerBalBefore = challenger.balance;

        vm.expectEmit(true, false, false, true);
        emit DisputeResolved(resultId, false);

        verifier.resolveDispute(resultId, hex"", bytes32(0), hex"", hex"");

        assertTrue(verifier.disputeResolved(resultId));
        assertFalse(verifier.disputeProverWon(resultId));
        assertFalse(verifier.isResultValid(resultId));
        // Challenger gets proverStake + challengeBond
        assertEq(challenger.balance, challengerBalBefore + DEFAULT_PROVER_STAKE + DEFAULT_CHALLENGE_BOND);
    }

    function test_resolveDispute_notChallenged() public {
        bytes32 resultId = _submitDefault();

        vm.expectRevert("TEEMLVerifier: not challenged");
        verifier.resolveDispute(resultId, hex"", bytes32(0), hex"", hex"");
    }

    function test_resolveDispute_alreadyResolved() public {
        bytes32 resultId = _submitDefault();

        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);

        mockVerifier.setResult(true);
        verifier.resolveDispute(resultId, hex"", bytes32(0), hex"", hex"");

        vm.expectRevert("TEEMLVerifier: already resolved");
        verifier.resolveDispute(resultId, hex"", bytes32(0), hex"", hex"");
    }

    // ─── Admin ───────────────────────────────────────────────────────────────

    function test_registerEnclave_onlyAdmin() public {
        address nonAdmin = address(0xBEEF);
        vm.prank(nonAdmin);
        vm.expectRevert("TEEMLVerifier: not admin");
        verifier.registerEnclave(address(0x1234), imageHash);
    }

    function test_revokeEnclave_onlyAdmin() public {
        address nonAdmin = address(0xBEEF);
        vm.prank(nonAdmin);
        vm.expectRevert("TEEMLVerifier: not admin");
        verifier.revokeEnclave(enclaveAddr);
    }

    function test_revokeEnclave_preventsSubmission() public {
        verifier.revokeEnclave(enclaveAddr);

        bytes memory attestation = _signAttestation(modelHash, inputHash, resultData);
        vm.expectRevert("TEEMLVerifier: enclave revoked");
        verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, inputHash, resultData, attestation);
    }

    function test_registerEnclave_alreadyRegistered() public {
        vm.expectRevert("TEEMLVerifier: already registered");
        verifier.registerEnclave(enclaveAddr, imageHash);
    }

    // ─── Attestation ─────────────────────────────────────────────────────────

    function test_attestation_ecrecover() public view {
        bytes32 resultHash = keccak256(resultData);
        bytes32 message = keccak256(abi.encodePacked(modelHash, inputHash, resultHash));
        bytes32 ethSignedHash = keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", message));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, ethSignedHash);

        address recovered = ecrecover(ethSignedHash, v, r, s);
        assertEq(recovered, enclaveAddr);
    }

    function test_attestation_wrongMessage() public {
        bytes memory attestation = _signAttestation(modelHash, inputHash, resultData);

        bytes32 wrongInputHash = keccak256("different-input");
        vm.expectRevert(); // ECDSA will recover a different (unregistered) address
        verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, wrongInputHash, resultData, attestation);
    }

    // ─── Finalize edge cases ─────────────────────────────────────────────────

    function test_finalize_challengedResult() public {
        bytes32 resultId = _submitDefault();

        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);

        vm.warp(block.timestamp + 1 hours + 1);
        vm.expectRevert("TEEMLVerifier: result is challenged");
        verifier.finalize(resultId);
    }

    function test_finalize_doubleFinalize() public {
        bytes32 resultId = _submitDefault();
        vm.warp(block.timestamp + 1 hours + 1);
        verifier.finalize(resultId);

        vm.expectRevert("TEEMLVerifier: already finalized");
        verifier.finalize(resultId);
    }

    // ─── Bug Fix: Excess bond tracked (Bug 1) ───────────────────────────────

    function test_challenge_fullBondReturned() public {
        bytes32 resultId = _submitDefault();

        // Challenger sends 0.5 ETH (well above the 0.1 ETH minimum)
        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        verifier.challenge{value: 0.5 ether}(resultId);

        // Verify full amount was stored
        ITEEMLVerifier.MLResult memory r = verifier.getResult(resultId);
        assertEq(r.challengeBond, 0.5 ether);

        // Challenger wins — should get back full 0.5 ETH + prover stake
        mockVerifier.setResult(false);
        uint256 challengerBalBefore = challenger.balance;
        verifier.resolveDispute(resultId, hex"", bytes32(0), hex"", hex"");

        assertEq(challenger.balance, challengerBalBefore + 0.5 ether + DEFAULT_PROVER_STAKE);
    }

    // ─── Bug Fix: Challenge griefing (Bug 2) ─────────────────────────────────

    function test_challenge_bondTooLow() public {
        bytes32 resultId = _submitDefault();

        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        // 0.09 ether < 0.1 ether default challengeBondAmount
        vm.expectRevert("TEEMLVerifier: insufficient bond");
        verifier.challenge{value: 0.09 ether}(resultId);
    }

    function test_submitResult_requiresStake() public {
        bytes memory attestation = _signAttestation(modelHash, inputHash, resultData);

        // No value sent
        vm.expectRevert("TEEMLVerifier: insufficient stake");
        verifier.submitResult(modelHash, inputHash, resultData, attestation);

        // Insufficient value
        vm.expectRevert("TEEMLVerifier: insufficient stake");
        verifier.submitResult{value: 0.05 ether}(modelHash, inputHash, resultData, attestation);
    }

    // ─── Bug Fix: Dispute timeout (Bug 3) ────────────────────────────────────

    function test_resolveDisputeByTimeout_challengerWins() public {
        bytes32 resultId = _submitDefault();

        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);

        uint256 challengerBalBefore = challenger.balance;

        // Advance past dispute deadline (24 hours)
        vm.warp(block.timestamp + 24 hours);

        vm.expectEmit(true, false, false, true);
        emit DisputeResolved(resultId, false);

        verifier.resolveDisputeByTimeout(resultId);

        assertTrue(verifier.disputeResolved(resultId));
        assertFalse(verifier.disputeProverWon(resultId));
        // Challenger gets both stakes
        assertEq(challenger.balance, challengerBalBefore + DEFAULT_CHALLENGE_BOND + DEFAULT_PROVER_STAKE);
    }

    function test_resolveDisputeByTimeout_tooEarly() public {
        bytes32 resultId = _submitDefault();

        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);

        // Only advance 12 hours (deadline is 24 hours)
        vm.warp(block.timestamp + 12 hours);

        vm.expectRevert("TEEMLVerifier: deadline not reached");
        verifier.resolveDisputeByTimeout(resultId);
    }

    function test_resolveDispute_withinDeadline() public {
        bytes32 resultId = _submitDefault();

        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);

        // Normal ZK resolution works within the deadline
        mockVerifier.setResult(true);
        vm.warp(block.timestamp + 12 hours);
        verifier.resolveDispute(resultId, hex"", bytes32(0), hex"", hex"");

        assertTrue(verifier.disputeResolved(resultId));
        assertTrue(verifier.disputeProverWon(resultId));
    }

    // ─── Admin: configurable bonds ───────────────────────────────────────────

    function test_admin_setChallengeBond() public {
        verifier.setChallengeBondAmount(0.5 ether);
        assertEq(verifier.challengeBondAmount(), 0.5 ether);

        // Now challenge requires 0.5 ether
        bytes32 resultId = _submitDefault();
        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        vm.expectRevert("TEEMLVerifier: insufficient bond");
        verifier.challenge{value: 0.1 ether}(resultId);
    }

    function test_admin_setProverStake() public {
        verifier.setProverStake(0.5 ether);
        assertEq(verifier.proverStake(), 0.5 ether);

        // Now submit requires 0.5 ether
        bytes memory attestation = _signAttestation(modelHash, inputHash, resultData);
        vm.expectRevert("TEEMLVerifier: insufficient stake");
        verifier.submitResult{value: 0.1 ether}(modelHash, inputHash, resultData, attestation);
    }
}
