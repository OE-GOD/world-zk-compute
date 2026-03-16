// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/tee/TEEMLVerifier.sol";
import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";
import {Pausable} from "@openzeppelin/contracts/utils/Pausable.sol";

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

/// @dev Malicious contract that attempts reentrancy on finalize()
contract ReentrantAttacker {
    TEEMLVerifier public target;
    bytes32 public targetResultId;
    bool public attacked;

    constructor(TEEMLVerifier _target) {
        target = _target;
    }

    function setTargetResultId(bytes32 _resultId) external {
        targetResultId = _resultId;
    }

    receive() external payable {
        if (!attacked) {
            attacked = true;
            // Attempt to re-enter finalize
            target.finalize(targetResultId);
        }
    }
}

contract TEEMLVerifierTest is Test {
    // Re-declare events for expectEmit (solc 0.8.20 compat)
    event ResultSubmitted(
        bytes32 indexed resultId, bytes32 indexed modelHash, bytes32 inputHash, address indexed submitter
    );
    event ResultChallenged(bytes32 indexed resultId, address challenger);
    event DisputeResolved(bytes32 indexed resultId, bool proverWon);
    event ResultFinalized(bytes32 indexed resultId);
    event ResultExpired(bytes32 indexed resultId);
    event ChallengeBondUpdated(uint256 oldAmount, uint256 newAmount);
    event ProverStakeUpdated(uint256 oldAmount, uint256 newAmount);
    event RemainderVerifierUpdated(address oldVerifier, address newVerifier);
    event ConfigUpdated(string param, uint256 oldValue, uint256 newValue);

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
        bytes32 structHash = keccak256(
            abi.encode(verifier.RESULT_TYPEHASH(), _modelHash, _inputHash, resultHash)
        );
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", _domainSeparator(), structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, digest);
        attestation = abi.encodePacked(r, s, v);
    }

    function _submitDefault() internal returns (bytes32 resultId) {
        bytes memory attestation = _signAttestation(modelHash, inputHash, resultData);
        resultId = verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, inputHash, resultData, attestation);
    }

    // ─── Happy Path: Submit ──────────────────────────────────────────────────

    function test_submitResult_validAttestation() public {
        bytes memory attestation = _signAttestation(modelHash, inputHash, resultData);

        vm.expectEmit(true, true, true, true);
        emit ResultSubmitted(
            keccak256(abi.encodePacked(address(this), modelHash, inputHash, block.number)),
            modelHash,
            inputHash,
            address(this)
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
        bytes32 structHash = keccak256(
            abi.encode(verifier.RESULT_TYPEHASH(), modelHash, inputHash, resultHash)
        );
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", _domainSeparator(), structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(wrongKey, digest);
        bytes memory attestation = abi.encodePacked(r, s, v);

        vm.expectRevert("TEEMLVerifier: unregistered enclave");
        verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, inputHash, resultData, attestation);
    }

    function test_submitResult_unregisteredEnclave() public {
        uint256 unknownKey = 0xDEAD;
        bytes32 resultHash = keccak256(resultData);
        bytes32 structHash = keccak256(
            abi.encode(verifier.RESULT_TYPEHASH(), modelHash, inputHash, resultHash)
        );
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", _domainSeparator(), structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(unknownKey, digest);
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
        emit ResultExpired(resultId);
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

        // Mock: ZK proof verifies -> prover was honest
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

        // Mock: ZK proof fails -> prover was dishonest
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
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, nonAdmin));
        verifier.registerEnclave(address(0x1234), imageHash);
    }

    function test_revokeEnclave_onlyAdmin() public {
        address nonAdmin = address(0xBEEF);
        vm.prank(nonAdmin);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, nonAdmin));
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
        bytes32 structHash = keccak256(
            abi.encode(verifier.RESULT_TYPEHASH(), modelHash, inputHash, resultHash)
        );
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", _domainSeparator(), structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, digest);

        address recovered = ecrecover(digest, v, r, s);
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
        vm.expectEmit(false, false, false, true);
        emit ChallengeBondUpdated(0.1 ether, 0.5 ether);
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
        vm.expectEmit(false, false, false, true);
        emit ProverStakeUpdated(0.1 ether, 0.5 ether);
        verifier.setProverStake(0.5 ether);
        assertEq(verifier.proverStake(), 0.5 ether);

        // Now submit requires 0.5 ether
        bytes memory attestation = _signAttestation(modelHash, inputHash, resultData);
        vm.expectRevert("TEEMLVerifier: insufficient stake");
        verifier.submitResult{value: 0.1 ether}(modelHash, inputHash, resultData, attestation);
    }

    // ─── NEW: Bug Fix — _settleDispute sets finalized ────────────────────────

    function test_settleDispute_setsFinalized() public {
        bytes32 resultId = _submitDefault();

        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);

        // Resolve dispute (prover wins)
        mockVerifier.setResult(true);
        verifier.resolveDispute(resultId, hex"", bytes32(0), hex"", hex"");

        // After dispute resolution, finalized must be true
        ITEEMLVerifier.MLResult memory r = verifier.getResult(resultId);
        assertTrue(r.finalized, "finalized should be true after dispute resolution");
        assertTrue(verifier.disputeResolved(resultId));
    }

    // ─── NEW: Pausable ──────────────────────────────────────────────────────

    function test_pause_blocksSubmit() public {
        verifier.pause();

        bytes memory attestation = _signAttestation(modelHash, inputHash, resultData);
        vm.expectRevert(Pausable.EnforcedPause.selector);
        verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, inputHash, resultData, attestation);
    }

    function test_pause_blocksChallenge() public {
        bytes32 resultId = _submitDefault();

        verifier.pause();

        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        vm.expectRevert(Pausable.EnforcedPause.selector);
        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);
    }

    function test_pause_doesNotBlockFinalize() public {
        bytes32 resultId = _submitDefault();
        vm.warp(block.timestamp + 1 hours + 1);

        verifier.pause();

        // finalize should still work while paused
        verifier.finalize(resultId);

        ITEEMLVerifier.MLResult memory r = verifier.getResult(resultId);
        assertTrue(r.finalized);
    }

    // ─── NEW: ReentrancyGuard ───────────────────────────────────────────────

    function test_reentrancy_finalize() public {
        // Deploy attacker contract
        ReentrantAttacker attacker = new ReentrantAttacker(verifier);

        // Submit a result as the attacker so the ETH payout goes to attacker's receive()
        bytes memory attestation = _signAttestation(modelHash, inputHash, resultData);

        vm.deal(address(attacker), 1 ether);
        vm.prank(address(attacker));
        bytes32 resultId =
            verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, inputHash, resultData, attestation);

        // Submit a second result that the attacker will try to finalize on re-entry
        vm.roll(block.number + 1);
        vm.deal(address(attacker), 1 ether);
        vm.prank(address(attacker));
        bytes32 resultId2 =
            verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, inputHash, resultData, attestation);

        attacker.setTargetResultId(resultId2);

        vm.warp(block.timestamp + 1 hours + 1);

        // When finalize pays out to attacker, attacker.receive() tries to re-enter finalize.
        // The re-entrant call reverts with ReentrancyGuardReentrantCall, which causes the
        // ETH transfer to fail, which causes the outer finalize to revert with "stake return failed".
        // This proves reentrancy is blocked -- the attacker cannot drain funds.
        vm.expectRevert("TEEMLVerifier: stake return failed");
        verifier.finalize(resultId);

        // Verify the result was NOT finalized (attack was fully prevented)
        ITEEMLVerifier.MLResult memory r = verifier.getResult(resultId);
        assertFalse(r.finalized, "result should not be finalized after failed reentrancy attack");
    }

    // ─── NEW: Ownable2Step ──────────────────────────────────────────────────

    function test_ownershipTransfer_twoStep() public {
        address newOwner = address(0xAE01);

        // Step 1: current owner initiates transfer
        verifier.transferOwnership(newOwner);
        // Owner hasn't changed yet
        assertEq(verifier.owner(), admin);
        assertEq(verifier.pendingOwner(), newOwner);

        // Step 2: new owner accepts
        vm.prank(newOwner);
        verifier.acceptOwnership();
        assertEq(verifier.owner(), newOwner);
        assertEq(verifier.pendingOwner(), address(0));

        // Old admin can no longer call admin functions
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, admin));
        verifier.registerEnclave(address(0x9999), imageHash);

        // New owner can call admin functions
        vm.prank(newOwner);
        verifier.registerEnclave(address(0x9999), imageHash);
    }

    // ─── NEW: Input Validation ──────────────────────────────────────────────

    function test_setRemainderVerifier_rejectsZero() public {
        vm.expectRevert("TEEMLVerifier: zero address");
        verifier.setRemainderVerifier(address(0));
    }

    function test_setBondAmount_rejectsZero() public {
        vm.expectRevert("TEEMLVerifier: zero amount");
        verifier.setChallengeBondAmount(0);

        vm.expectRevert("TEEMLVerifier: zero amount");
        verifier.setProverStake(0);
    }

    function test_setBondAmount_rejectsExcessive() public {
        vm.expectRevert("TEEMLVerifier: amount too high");
        verifier.setChallengeBondAmount(101 ether);

        vm.expectRevert("TEEMLVerifier: amount too high");
        verifier.setProverStake(101 ether);
    }

    function test_setRemainderVerifier_emitsEvent() public {
        address newVerifier = address(0xABCD);
        vm.expectEmit(false, false, false, true);
        emit RemainderVerifierUpdated(address(mockVerifier), newVerifier);
        verifier.setRemainderVerifier(newVerifier);
    }

    // ─── NEW: Event Indexing Improvements ────────────────────────────────────

    function test_resultSubmitted_indexedTopics() public {
        // Submit from two different addresses and verify indexed submitter filtering
        bytes memory attestation = _signAttestation(modelHash, inputHash, resultData);

        // Submit as address(this) — check all 3 indexed topics match
        vm.expectEmit(true, true, true, true);
        bytes32 expectedId = keccak256(abi.encodePacked(address(this), modelHash, inputHash, block.number));
        emit ResultSubmitted(expectedId, modelHash, inputHash, address(this));

        bytes32 resultId =
            verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, inputHash, resultData, attestation);
        assertEq(resultId, expectedId);
    }

    function test_resultSubmitted_differentSubmitter() public {
        // Submit from a different address and verify submitter is correct
        address submitter = address(0xFACE);
        vm.deal(submitter, 1 ether);
        bytes memory attestation = _signAttestation(modelHash, inputHash, resultData);

        vm.prank(submitter);
        vm.expectEmit(true, true, true, true);
        bytes32 expectedId = keccak256(abi.encodePacked(submitter, modelHash, inputHash, block.number));
        emit ResultSubmitted(expectedId, modelHash, inputHash, submitter);

        bytes32 resultId =
            verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, inputHash, resultData, attestation);
        assertEq(resultId, expectedId);
    }

    function test_resultExpired_emittedOnFinalize() public {
        bytes32 resultId = _submitDefault();
        vm.warp(block.timestamp + 1 hours + 1);

        // ResultExpired should be emitted when an unchallenged result finalizes
        vm.expectEmit(true, false, false, false);
        emit ResultExpired(resultId);

        verifier.finalize(resultId);
    }

    function test_resultExpired_notEmittedOnDisputeResolution() public {
        bytes32 resultId = _submitDefault();

        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);

        // Resolve dispute — this should NOT emit ResultExpired (only DisputeResolved)
        mockVerifier.setResult(true);

        // Record logs to verify ResultExpired is NOT emitted
        vm.recordLogs();
        verifier.resolveDispute(resultId, hex"", bytes32(0), hex"", hex"");

        Vm.Log[] memory logs = vm.getRecordedLogs();
        bytes32 resultExpiredTopic = keccak256("ResultExpired(bytes32)");
        for (uint256 i = 0; i < logs.length; i++) {
            assertTrue(
                logs[i].topics[0] != resultExpiredTopic, "ResultExpired should not be emitted on dispute resolution"
            );
        }
    }

    function test_configUpdated_challengeBond() public {
        vm.expectEmit(false, false, false, true);
        emit ConfigUpdated("challengeBondAmount", 0.1 ether, 0.5 ether);

        verifier.setChallengeBondAmount(0.5 ether);
    }

    function test_configUpdated_proverStake() public {
        vm.expectEmit(false, false, false, true);
        emit ConfigUpdated("proverStake", 0.1 ether, 0.2 ether);

        verifier.setProverStake(0.2 ether);
    }

    function test_configUpdated_emittedAlongsideSpecificEvent() public {
        // Both ChallengeBondUpdated and ConfigUpdated should be emitted
        vm.recordLogs();
        verifier.setChallengeBondAmount(0.3 ether);

        Vm.Log[] memory logs = vm.getRecordedLogs();

        // Should have exactly 2 events
        assertEq(logs.length, 2, "Expected 2 events (ChallengeBondUpdated + ConfigUpdated)");

        bytes32 challengeBondTopic = keccak256("ChallengeBondUpdated(uint256,uint256)");
        bytes32 configUpdatedTopic = keccak256("ConfigUpdated(string,uint256,uint256)");

        assertEq(logs[0].topics[0], challengeBondTopic, "First event should be ChallengeBondUpdated");
        assertEq(logs[1].topics[0], configUpdatedTopic, "Second event should be ConfigUpdated");
    }

    function test_configUpdated_proverStakeAlongsideSpecificEvent() public {
        // Both ProverStakeUpdated and ConfigUpdated should be emitted
        vm.recordLogs();
        verifier.setProverStake(0.3 ether);

        Vm.Log[] memory logs = vm.getRecordedLogs();

        assertEq(logs.length, 2, "Expected 2 events (ProverStakeUpdated + ConfigUpdated)");

        bytes32 proverStakeTopic = keccak256("ProverStakeUpdated(uint256,uint256)");
        bytes32 configUpdatedTopic = keccak256("ConfigUpdated(string,uint256,uint256)");

        assertEq(logs[0].topics[0], proverStakeTopic, "First event should be ProverStakeUpdated");
        assertEq(logs[1].topics[0], configUpdatedTopic, "Second event should be ConfigUpdated");
    }

    function test_finalize_emitsBothExpiredAndFinalized() public {
        bytes32 resultId = _submitDefault();
        vm.warp(block.timestamp + 1 hours + 1);

        vm.recordLogs();
        verifier.finalize(resultId);

        Vm.Log[] memory logs = vm.getRecordedLogs();

        // Should have exactly 2 events: ResultExpired and ResultFinalized
        assertEq(logs.length, 2, "Expected 2 events (ResultExpired + ResultFinalized)");

        bytes32 resultExpiredTopic = keccak256("ResultExpired(bytes32)");
        bytes32 resultFinalizedTopic = keccak256("ResultFinalized(bytes32)");

        assertEq(logs[0].topics[0], resultExpiredTopic, "First event should be ResultExpired");
        assertEq(logs[1].topics[0], resultFinalizedTopic, "Second event should be ResultFinalized");

        // Both events should have the same resultId as indexed topic
        assertEq(logs[0].topics[1], resultId, "ResultExpired should have correct resultId");
        assertEq(logs[1].topics[1], resultId, "ResultFinalized should have correct resultId");
    }

    // ─── Dispute Extension Tests (T72) ──────────────────────────────────────

    event DisputeExtended(bytes32 indexed resultId, uint256 newDeadline);

    function test_extendDisputeWindow_success() public {
        bytes memory att = _signAttestation(modelHash, inputHash, resultData);
        bytes32 resultId = verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, inputHash, resultData, att);

        // Challenge
        address challenger = address(0xBEEF);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);

        // Get original deadline
        ITEEMLVerifier.MLResult memory r = verifier.getResult(resultId);
        uint256 originalDeadline = r.disputeDeadline;

        // Extend (submitter = address(this) = admin)
        vm.expectEmit(true, false, false, true);
        emit DisputeExtended(resultId, originalDeadline + 30 minutes);
        verifier.extendDisputeWindow(resultId);

        // Verify deadline extended
        r = verifier.getResult(resultId);
        assertEq(r.disputeDeadline, originalDeadline + 30 minutes);
        assertEq(verifier.disputeExtensions(resultId), 1);
    }

    function test_extendDisputeWindow_notSubmitter() public {
        bytes memory att = _signAttestation(modelHash, inputHash, resultData);
        bytes32 resultId = verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, inputHash, resultData, att);

        address challenger = address(0xBEEF);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);

        // Non-submitter tries to extend
        vm.prank(address(0xDEAD));
        vm.expectRevert("TEEMLVerifier: not submitter");
        verifier.extendDisputeWindow(resultId);
    }

    function test_extendDisputeWindow_notChallenged() public {
        bytes memory att = _signAttestation(modelHash, inputHash, resultData);
        bytes32 resultId = verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, inputHash, resultData, att);

        vm.expectRevert("TEEMLVerifier: not challenged");
        verifier.extendDisputeWindow(resultId);
    }

    function test_extendDisputeWindow_alreadyExtended() public {
        bytes memory att = _signAttestation(modelHash, inputHash, resultData);
        bytes32 resultId = verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, inputHash, resultData, att);

        address challenger = address(0xBEEF);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);

        // First extension succeeds
        verifier.extendDisputeWindow(resultId);

        // Second extension fails (MAX_EXTENSIONS = 1)
        vm.expectRevert("TEEMLVerifier: max extensions reached");
        verifier.extendDisputeWindow(resultId);
    }

    function test_extendDisputeWindow_alreadyResolved() public {
        bytes memory att = _signAttestation(modelHash, inputHash, resultData);
        bytes32 resultId = verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, inputHash, resultData, att);

        address challenger = address(0xBEEF);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);

        // Resolve by timeout
        ITEEMLVerifier.MLResult memory r = verifier.getResult(resultId);
        vm.warp(r.disputeDeadline + 1);
        verifier.resolveDisputeByTimeout(resultId);

        // Can't extend after resolution
        vm.expectRevert("TEEMLVerifier: already resolved");
        verifier.extendDisputeWindow(resultId);
    }

    function test_resolveDisputeByTimeout_respectsExtension() public {
        bytes memory att = _signAttestation(modelHash, inputHash, resultData);
        bytes32 resultId = verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, inputHash, resultData, att);

        address challenger = address(0xBEEF);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);

        // Extend the deadline
        verifier.extendDisputeWindow(resultId);

        ITEEMLVerifier.MLResult memory r = verifier.getResult(resultId);
        uint256 extendedDeadline = r.disputeDeadline;

        // Try timeout at original deadline (should fail since extended)
        vm.warp(extendedDeadline - 1);
        vm.expectRevert("TEEMLVerifier: deadline not reached");
        verifier.resolveDisputeByTimeout(resultId);

        // Timeout at extended deadline works
        vm.warp(extendedDeadline + 1);
        verifier.resolveDisputeByTimeout(resultId);

        assertTrue(verifier.disputeResolved(resultId));
    }
}
