// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/ExecutionEngine.sol";
import "../src/ProgramRegistry.sol";
import "../src/ProverRegistry.sol";
import "../src/ProverReputation.sol";
import "../src/MockRiscZeroVerifier.sol";
import "../src/tee/TEEMLVerifier.sol";
import "../src/RiscZeroVerifierRouter.sol";
import "../src/RiscZeroVerifierAdapter.sol";

/// @dev Minimal ERC20 for ProverRegistry staking in tests
contract StakeToken {
    string public name = "Stake";
    string public symbol = "STK";
    uint8 public decimals = 18;
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        return true;
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        balanceOf[msg.sender] -= amount;
        balanceOf[to] += amount;
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        allowance[from][msg.sender] -= amount;
        balanceOf[from] -= amount;
        balanceOf[to] += amount;
        return true;
    }
}

/// @dev Mock callback that records calls
contract TestCallback is IExecutionCallback {
    uint256 public lastRequestId;
    bytes32 public lastImageId;
    bytes public lastJournal;
    uint256 public callCount;

    function onExecutionComplete(uint256 requestId, bytes32 imageId, bytes calldata journal) external {
        lastRequestId = requestId;
        lastImageId = imageId;
        lastJournal = journal;
        callCount++;
    }
}

/// @dev Callback that always reverts
contract RevertingCallback is IExecutionCallback {
    function onExecutionComplete(uint256, bytes32, bytes calldata) external pure {
        revert("callback-boom");
    }
}

/// @dev Callback that consumes excessive gas
contract GasGuzzlingCallback is IExecutionCallback {
    uint256 public waste;

    function onExecutionComplete(uint256, bytes32, bytes calldata) external {
        // Burn gas with storage writes
        for (uint256 i = 0; i < 1000; i++) {
            waste = i;
        }
    }
}

/// @title SystemIntegrationTest
/// @notice Tests the full system stack: ProgramRegistry + ProverRegistry + ExecutionEngine + TEEMLVerifier
/// @dev Verifies cross-contract wiring and end-to-end flows that unit tests don't cover
contract SystemIntegrationTest is Test {
    // Re-declare events for expectEmit
    event ExecutionRequested(
        uint256 indexed requestId,
        address indexed requester,
        bytes32 indexed imageId,
        bytes32 inputDigest,
        string inputUrl,
        uint8 inputType,
        uint256 tip,
        uint256 expiresAt
    );
    event ExecutionCompleted(uint256 indexed requestId, address indexed prover, bytes32 journalDigest, uint256 payout);
    event ResultSubmitted(bytes32 indexed resultId, bytes32 modelHash, bytes32 inputHash, address submitter);
    event ResultFinalized(bytes32 indexed resultId);
    event ResultChallenged(bytes32 indexed resultId, address challenger);
    event DisputeResolved(bytes32 indexed resultId, bool proofValid);

    // Contracts
    ProgramRegistry public programRegistry;
    ProverRegistry public proverRegistry;
    ProverReputation public reputation;
    ExecutionEngine public engine;
    TEEMLVerifier public teeVerifier;
    MockRiscZeroVerifier public mockVerifier;
    StakeToken public stakeToken;
    TestCallback public callback;

    // Actors
    address admin = address(this);
    address requester = address(0x1111);
    address prover = address(0x2222);
    address challenger = address(0x3333);
    uint256 enclavePrivateKey = 0xA11CE;
    address enclaveKey;

    // Test data
    bytes32 imageId = bytes32(uint256(0xDEAD));
    bytes32 inputDigest = keccak256("test-input");
    bytes32 modelHash = keccak256("test-model");
    bytes32 inputHash = keccak256("test-input-data");

    function setUp() public {
        enclaveKey = vm.addr(enclavePrivateKey);

        // Deploy all contracts
        mockVerifier = new MockRiscZeroVerifier();
        programRegistry = new ProgramRegistry(admin);
        stakeToken = new StakeToken();
        proverRegistry = new ProverRegistry(address(stakeToken), 100 ether, 500); // 100 STK min, 5% slash
        reputation = new ProverReputation();
        engine = new ExecutionEngine(admin, address(programRegistry), address(mockVerifier), address(0xFEE));
        teeVerifier = new TEEMLVerifier(admin, address(0)); // no remainder verifier for happy-path tests

        // Wire up
        engine.setReputation(address(reputation));

        // Fund actors
        vm.deal(requester, 10 ether);
        vm.deal(prover, 10 ether);
        vm.deal(challenger, 10 ether);

        // Setup stake tokens for prover
        stakeToken.mint(prover, 1000 ether);

        // Deploy callback
        callback = new TestCallback();
    }

    // ========================================================================
    // 1. Full ExecutionEngine lifecycle with ProgramRegistry
    // ========================================================================

    function test_fullExecutionLifecycle() public {
        // Step 1: Register program in ProgramRegistry
        vm.prank(prover);
        programRegistry.registerProgram(imageId, "test-program", "https://example.com/program.elf", bytes32(0));
        assertTrue(programRegistry.isProgramActive(imageId));

        // Step 2: Requester creates execution request
        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(
            imageId, inputDigest, "https://inputs.url", address(callback), 3600
        );
        assertEq(requestId, 1);

        // Step 3: Prover claims execution
        vm.prank(prover);
        engine.claimExecution(requestId);

        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);
        assertEq(req.claimedBy, prover);
        assertEq(uint8(req.status), uint8(ExecutionEngine.RequestStatus.Claimed));

        // Step 4: Prover submits proof
        bytes memory seal = hex"deadbeef";
        bytes memory journal = hex"cafebabe";
        vm.deal(address(engine), 1 ether); // Fund for payout

        vm.prank(prover);
        engine.submitProof(requestId, seal, journal);

        req = engine.getRequest(requestId);
        assertEq(uint8(req.status), uint8(ExecutionEngine.RequestStatus.Completed));

        // Step 5: Verify callback was called
        assertEq(callback.callCount(), 1);
        assertEq(callback.lastRequestId(), requestId);
        assertEq(callback.lastImageId(), imageId);
    }

    function test_executionWithProverReputation() public {
        // Register program
        vm.prank(prover);
        programRegistry.registerProgram(imageId, "test", "url", bytes32(0));

        // Request + claim + submit
        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, "url", address(0), 3600);

        vm.prank(prover);
        engine.claimExecution(requestId);

        vm.deal(address(engine), 1 ether);
        vm.prank(prover);
        engine.submitProof(requestId, hex"deadbeef", hex"cafebabe");

        // Verify prover stats updated
        (uint256 completed, uint256 earnings) = engine.getProverStats(prover);
        assertEq(completed, 1);
        assertGt(earnings, 0);
    }

    // ========================================================================
    // 2. Full TEEMLVerifier lifecycle
    // ========================================================================

    function test_teeSubmitAndFinalize() public {
        // Register enclave
        teeVerifier.registerEnclave(enclaveKey, bytes32(uint256(0xBEEF)));
        (bool registered,,,) = teeVerifier.enclaves(enclaveKey);
        assertTrue(registered);

        // Create attestation
        bytes memory result = "prediction: 0.95";
        bytes32 resultHash = keccak256(result);
        bytes32 message = keccak256(abi.encodePacked(modelHash, inputHash, resultHash));
        bytes32 ethSignedHash = keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", message));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, ethSignedHash);
        bytes memory attestation = abi.encodePacked(r, s, v);

        // Submit result
        vm.prank(prover);
        bytes32 resultId = teeVerifier.submitResult{value: 0.1 ether}(modelHash, inputHash, result, attestation);

        // Verify result stored
        ITEEMLVerifier.MLResult memory res = teeVerifier.getResult(resultId);
        assertEq(res.enclave, enclaveKey);
        assertEq(res.submitter, prover);
        assertFalse(res.finalized);

        // Fast forward past challenge window
        vm.warp(block.timestamp + 1 hours + 1);

        // Finalize
        teeVerifier.finalize(resultId);

        // Verify result is valid
        assertTrue(teeVerifier.isResultValid(resultId));
    }

    function test_teeSubmitChallengeAndTimeout() public {
        // Register enclave + submit result
        teeVerifier.registerEnclave(enclaveKey, bytes32(uint256(0xBEEF)));

        bytes memory result = "prediction: 0.95";
        bytes32 resultHash = keccak256(result);
        bytes32 message = keccak256(abi.encodePacked(modelHash, inputHash, resultHash));
        bytes32 ethSignedHash = keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", message));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, ethSignedHash);
        bytes memory attestation = abi.encodePacked(r, s, v);

        vm.prank(prover);
        bytes32 resultId = teeVerifier.submitResult{value: 0.1 ether}(modelHash, inputHash, result, attestation);

        // Challenger challenges
        vm.prank(challenger);
        teeVerifier.challenge{value: 0.1 ether}(resultId);

        ITEEMLVerifier.MLResult memory res = teeVerifier.getResult(resultId);
        assertTrue(res.challenged);
        assertEq(res.challenger, challenger);

        // Fast forward past dispute window — challenger wins by timeout
        vm.warp(block.timestamp + 24 hours + 1);

        uint256 challengerBalBefore = challenger.balance;
        teeVerifier.resolveDisputeByTimeout(resultId);

        // Challenger wins: gets prover stake + own bond back
        assertGt(challenger.balance, challengerBalBefore);
        assertFalse(teeVerifier.isResultValid(resultId));
    }

    // ========================================================================
    // 3. ProverRegistry + staking integration
    // ========================================================================

    function test_proverRegistryStakingCycle() public {
        // Register prover with stake
        vm.startPrank(prover);
        stakeToken.approve(address(proverRegistry), 200 ether);
        proverRegistry.register(200 ether, "https://prover.example.com");
        vm.stopPrank();

        assertTrue(proverRegistry.isActive(prover));
        assertEq(proverRegistry.totalStaked(), 200 ether);

        // Owner authorizes engine as slasher
        proverRegistry.setSlasher(address(engine), true);

        // Record success (simulating ExecutionEngine calling back)
        vm.prank(address(engine));
        proverRegistry.recordSuccess(prover, 1 ether);

        ProverRegistry.Prover memory p = proverRegistry.getProver(prover);
        assertEq(p.proofsSubmitted, 1);
        assertEq(p.totalEarnings, 1 ether);
        assertGt(p.reputation, 5000); // Increased from default 50%
    }

    function test_proverSlashingAndDeactivation() public {
        // Register prover at minimum stake
        vm.startPrank(prover);
        stakeToken.approve(address(proverRegistry), 100 ether);
        proverRegistry.register(100 ether, "");
        vm.stopPrank();

        // Authorize admin as slasher
        proverRegistry.setSlasher(admin, true);

        // Slash prover (5% of 100 = 5 ETH)
        proverRegistry.slash(prover, "Invalid proof submitted");

        ProverRegistry.Prover memory p = proverRegistry.getProver(prover);
        assertEq(p.stake, 95 ether);
        assertEq(p.proofsFailed, 1);

        // Prover is still active (95 >= 100? No, deactivated!)
        assertFalse(proverRegistry.isActive(prover));
    }

    // ========================================================================
    // 4. Cross-contract: ProgramRegistry deactivation blocks ExecutionEngine
    // ========================================================================

    function test_deactivatedProgramBlocksExecution() public {
        // Register and verify program
        vm.prank(prover);
        programRegistry.registerProgram(imageId, "test", "url", bytes32(0));

        // Admin deactivates program
        programRegistry.deactivateProgram(imageId);
        assertFalse(programRegistry.isProgramActive(imageId));

        // Requester tries to create execution — should revert
        vm.prank(requester);
        vm.expectRevert();
        engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, "url", address(0), 3600);
    }

    // ========================================================================
    // 5. Cross-contract: TEE + Execution Engine parallel flows
    // ========================================================================

    function test_parallelTEEAndExecutionFlows() public {
        // Setup: Register program for ExecutionEngine
        vm.prank(prover);
        programRegistry.registerProgram(imageId, "test", "url", bytes32(0));

        // Setup: Register enclave for TEEMLVerifier
        teeVerifier.registerEnclave(enclaveKey, bytes32(uint256(0xBEEF)));

        // Flow 1: ExecutionEngine request
        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, "url", address(0), 3600);

        // Flow 2: TEE submission (same model, different path)
        bytes memory result = "prediction: 0.95";
        bytes32 resultHash = keccak256(result);
        bytes32 message = keccak256(abi.encodePacked(modelHash, inputHash, resultHash));
        bytes32 ethSignedHash = keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", message));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, ethSignedHash);
        bytes memory attestation = abi.encodePacked(r, s, v);

        vm.prank(prover);
        bytes32 resultId = teeVerifier.submitResult{value: 0.1 ether}(modelHash, inputHash, result, attestation);

        // Complete ExecutionEngine flow
        vm.prank(prover);
        engine.claimExecution(requestId);
        vm.deal(address(engine), 1 ether);
        vm.prank(prover);
        engine.submitProof(requestId, hex"deadbeef", hex"cafebabe");

        // Complete TEE flow
        vm.warp(block.timestamp + 1 hours + 1);
        teeVerifier.finalize(resultId);

        // Both paths complete successfully
        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);
        assertEq(uint8(req.status), uint8(ExecutionEngine.RequestStatus.Completed));
        assertTrue(teeVerifier.isResultValid(resultId));
    }

    // ========================================================================
    // 6. Multi-request execution engine stress
    // ========================================================================

    function test_multipleRequestsConcurrentClaims() public {
        // Register program
        vm.prank(prover);
        programRegistry.registerProgram(imageId, "test", "url", bytes32(0));

        address prover2 = address(0x4444);
        vm.deal(prover2, 10 ether);

        // Create 3 requests
        uint256[] memory requestIds = new uint256[](3);
        for (uint256 i = 0; i < 3; i++) {
            vm.prank(requester);
            requestIds[i] = engine.requestExecution{value: 0.1 ether}(
                imageId, keccak256(abi.encodePacked("input-", i)), "url", address(0), 3600
            );
        }

        // Different provers claim different requests
        vm.prank(prover);
        engine.claimExecution(requestIds[0]);
        vm.prank(prover);
        engine.claimExecution(requestIds[1]);
        vm.prank(prover2);
        engine.claimExecution(requestIds[2]);

        // Verify claims
        assertEq(engine.getRequest(requestIds[0]).claimedBy, prover);
        assertEq(engine.getRequest(requestIds[1]).claimedBy, prover);
        assertEq(engine.getRequest(requestIds[2]).claimedBy, prover2);
    }

    // ========================================================================
    // 7. TEE enclave lifecycle
    // ========================================================================

    function test_enclaveRegistrationRevocationResubmit() public {
        // Register enclave
        teeVerifier.registerEnclave(enclaveKey, bytes32(uint256(0xBEEF)));

        // Submit a result with valid attestation
        bytes memory result = "result-1";
        bytes32 resultHash = keccak256(result);
        bytes32 message = keccak256(abi.encodePacked(modelHash, inputHash, resultHash));
        bytes32 ethSignedHash = keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", message));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, ethSignedHash);
        bytes memory attestation = abi.encodePacked(r, s, v);

        vm.prank(prover);
        teeVerifier.submitResult{value: 0.1 ether}(modelHash, inputHash, result, attestation);

        // Revoke enclave
        teeVerifier.revokeEnclave(enclaveKey);

        // Try to submit with revoked enclave — should fail
        bytes memory result2 = "result-2";
        bytes32 resultHash2 = keccak256(result2);
        bytes32 message2 = keccak256(abi.encodePacked(modelHash, inputHash, resultHash2));
        bytes32 ethSignedHash2 = keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", message2));
        (uint8 v2, bytes32 r2, bytes32 s2) = vm.sign(enclavePrivateKey, ethSignedHash2);
        bytes memory attestation2 = abi.encodePacked(r2, s2, v2);

        vm.prank(prover);
        vm.expectRevert("TEEMLVerifier: enclave revoked");
        teeVerifier.submitResult{value: 0.1 ether}(modelHash, inputHash, result2, attestation2);
    }

    // ========================================================================
    // 8. Pause propagation across contracts
    // ========================================================================

    function test_pauseBlocksOperationsAcrossContracts() public {
        // Register program
        vm.prank(prover);
        programRegistry.registerProgram(imageId, "test", "url", bytes32(0));

        // Register enclave
        teeVerifier.registerEnclave(enclaveKey, bytes32(uint256(0xBEEF)));

        // Pause ExecutionEngine
        engine.pause();

        // ExecutionEngine requests blocked
        vm.prank(requester);
        vm.expectRevert();
        engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, "url", address(0), 3600);

        // Pause TEEMLVerifier
        teeVerifier.pause();

        // TEE submissions blocked
        vm.prank(prover);
        vm.expectRevert();
        teeVerifier.submitResult{value: 0.1 ether}(modelHash, inputHash, "result", "attestation");

        // Pause ProgramRegistry
        programRegistry.pause();

        // Program registration blocked
        vm.prank(prover);
        vm.expectRevert();
        programRegistry.registerProgram(bytes32(uint256(2)), "test2", "url2", bytes32(0));

        // Unpause all
        engine.unpause();
        teeVerifier.unpause();
        programRegistry.unpause();

        // Operations resume
        vm.prank(requester);
        uint256 rid = engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, "url", address(0), 3600);
        assertEq(rid, 1);
    }

    // ========================================================================
    // 9. ProverReputation integration with ExecutionEngine
    // ========================================================================

    function test_reputationUpdatesOnExecution() public {
        // Register program
        vm.prank(prover);
        programRegistry.registerProgram(imageId, "test", "url", bytes32(0));

        // Register prover in reputation system and authorize engine
        vm.prank(prover);
        reputation.register();
        reputation.authorizeReporter(address(engine));
        ProverReputation.Reputation memory rep = reputation.getReputation(prover);
        assertEq(rep.totalJobs, 0);

        // Complete an execution
        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, "url", address(0), 3600);

        vm.prank(prover);
        engine.claimExecution(requestId);

        vm.deal(address(engine), 1 ether);
        vm.prank(prover);
        engine.submitProof(requestId, hex"deadbeef", hex"cafebabe");

        // Check reputation was updated
        rep = reputation.getReputation(prover);
        assertEq(rep.completedJobs, 1);
        assertGt(rep.score, 0);
    }

    // ========================================================================
    // 10. TEE dispute extension
    // ========================================================================

    function test_teeDisputeExtension() public {
        // Register enclave + submit
        teeVerifier.registerEnclave(enclaveKey, bytes32(uint256(0xBEEF)));

        bytes memory result = "prediction: 0.95";
        bytes32 resultHash = keccak256(result);
        bytes32 message = keccak256(abi.encodePacked(modelHash, inputHash, resultHash));
        bytes32 ethSignedHash = keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", message));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, ethSignedHash);
        bytes memory attestation = abi.encodePacked(r, s, v);

        vm.prank(prover);
        bytes32 resultId = teeVerifier.submitResult{value: 0.1 ether}(modelHash, inputHash, result, attestation);

        // Challenge
        vm.prank(challenger);
        teeVerifier.challenge{value: 0.1 ether}(resultId);

        ITEEMLVerifier.MLResult memory res = teeVerifier.getResult(resultId);
        uint256 originalDeadline = res.disputeDeadline;

        // Prover extends
        vm.prank(prover);
        teeVerifier.extendDisputeWindow(resultId);

        res = teeVerifier.getResult(resultId);
        assertEq(res.disputeDeadline, originalDeadline + 30 minutes);

        // Can't extend again (max 1)
        vm.prank(prover);
        vm.expectRevert("TEEMLVerifier: max extensions reached");
        teeVerifier.extendDisputeWindow(resultId);
    }

    // ========================================================================
    // 11. Config updates across contracts
    // ========================================================================

    function test_configUpdatesAffectBehavior() public {
        // Update TEE challenge bond
        teeVerifier.setChallengeBondAmount(0.5 ether);
        assertEq(teeVerifier.challengeBondAmount(), 0.5 ether);

        // Update TEE prover stake
        teeVerifier.setProverStake(0.2 ether);
        assertEq(teeVerifier.proverStake(), 0.2 ether);

        // Update ExecutionEngine protocol fee
        engine.setProtocolFee(500); // 5%
        assertEq(engine.protocolFeeBps(), 500);

        // Register enclave + submit with new stake requirement
        teeVerifier.registerEnclave(enclaveKey, bytes32(uint256(0xBEEF)));

        bytes memory result = "result";
        bytes32 resultHash = keccak256(result);
        bytes32 message = keccak256(abi.encodePacked(modelHash, inputHash, resultHash));
        bytes32 ethSignedHash = keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", message));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, ethSignedHash);
        bytes memory attestation = abi.encodePacked(r, s, v);

        // Insufficient stake with new requirement
        vm.prank(prover);
        vm.expectRevert("TEEMLVerifier: insufficient stake");
        teeVerifier.submitResult{value: 0.1 ether}(modelHash, inputHash, result, attestation);

        // Sufficient stake
        vm.prank(prover);
        bytes32 resultId = teeVerifier.submitResult{value: 0.2 ether}(modelHash, inputHash, result, attestation);
        assertTrue(resultId != bytes32(0));
    }

    // ========================================================================
    // 12. Ownership transfer across contracts
    // ========================================================================

    function test_ownershipTransferFlow() public {
        address newAdmin = address(0x9999);

        // Transfer ExecutionEngine ownership (Ownable2Step)
        engine.transferOwnership(newAdmin);
        assertEq(engine.pendingOwner(), newAdmin);

        vm.prank(newAdmin);
        engine.acceptOwnership();
        assertEq(engine.owner(), newAdmin);

        // Transfer TEEMLVerifier ownership (Ownable2Step)
        teeVerifier.transferOwnership(newAdmin);
        vm.prank(newAdmin);
        teeVerifier.acceptOwnership();
        assertEq(teeVerifier.owner(), newAdmin);

        // Transfer ProgramRegistry ownership (Ownable2Step)
        programRegistry.transferOwnership(newAdmin);
        vm.prank(newAdmin);
        programRegistry.acceptOwnership();
        assertEq(programRegistry.owner(), newAdmin);

        // New admin can operate
        vm.prank(newAdmin);
        teeVerifier.registerEnclave(enclaveKey, bytes32(uint256(0xBEEF)));
        (bool reg,,,) = teeVerifier.enclaves(enclaveKey);
        assertTrue(reg);
    }

    // ========================================================================
    // 13. Cancellation and refund flow
    // ========================================================================

    function test_executionCancellationRefund() public {
        // Register program
        vm.prank(prover);
        programRegistry.registerProgram(imageId, "test", "url", bytes32(0));

        // Create request
        uint256 balBefore = requester.balance;
        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, "url", address(0), 3600);

        // Cancel
        vm.prank(requester);
        engine.cancelExecution(requestId);

        // Refund received
        assertEq(requester.balance, balBefore);

        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);
        assertEq(uint8(req.status), uint8(ExecutionEngine.RequestStatus.Cancelled));
    }

    // ========================================================================
    // 14. Weighted prover selection in ProverRegistry
    // ========================================================================

    function test_proverSelectionWeightedByStakeAndReputation() public {
        address prover2 = address(0x5555);
        stakeToken.mint(prover2, 1000 ether);

        // Register two provers with different stakes
        vm.startPrank(prover);
        stakeToken.approve(address(proverRegistry), 200 ether);
        proverRegistry.register(200 ether, "");
        vm.stopPrank();

        vm.startPrank(prover2);
        stakeToken.approve(address(proverRegistry), 500 ether);
        proverRegistry.register(500 ether, "");
        vm.stopPrank();

        // Both should be selectable
        assertEq(proverRegistry.activeProverCount(), 2);

        // Selection should work with different seeds
        address selected1 = proverRegistry.selectProver(1);
        address selected2 = proverRegistry.selectProver(2);
        assertTrue(selected1 == prover || selected1 == prover2);
        assertTrue(selected2 == prover || selected2 == prover2);

        // Top provers should return both (same initial reputation)
        address[] memory top = proverRegistry.getTopProvers(2);
        assertEq(top.length, 2);
    }

    // ========================================================================
    // 15. Contract balance isolation
    // ========================================================================

    function test_contractBalancesIsolated() public {
        // Register enclave
        teeVerifier.registerEnclave(enclaveKey, bytes32(uint256(0xBEEF)));

        bytes memory result = "prediction: 0.95";
        bytes32 resultHash = keccak256(result);
        bytes32 message = keccak256(abi.encodePacked(modelHash, inputHash, resultHash));
        bytes32 ethSignedHash = keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", message));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, ethSignedHash);
        bytes memory attestation = abi.encodePacked(r, s, v);

        // Submit to TEE verifier
        vm.prank(prover);
        teeVerifier.submitResult{value: 0.1 ether}(modelHash, inputHash, result, attestation);

        // TEE verifier holds the stake
        assertEq(address(teeVerifier).balance, 0.1 ether);

        // ExecutionEngine has separate balance
        assertEq(address(engine).balance, 0);

        // Register program + request execution
        vm.prank(prover);
        programRegistry.registerProgram(imageId, "test", "url", bytes32(0));
        vm.prank(requester);
        engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, "url", address(0), 3600);

        // ExecutionEngine holds its own funds
        assertEq(address(engine).balance, 0.1 ether);
        // TEE verifier unaffected
        assertEq(address(teeVerifier).balance, 0.1 ether);
    }

    // ========================================================================
    // 16. Callback failure does NOT revert proof submission
    // ========================================================================

    function test_revertingCallbackDoesNotBlockProof() public {
        // Register program
        vm.prank(prover);
        programRegistry.registerProgram(imageId, "test", "url", bytes32(0));

        // Deploy reverting callback
        RevertingCallback badCallback = new RevertingCallback();

        // Create request with reverting callback
        vm.prank(requester);
        uint256 requestId =
            engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, "url", address(badCallback), 3600);

        // Claim and submit proof
        vm.prank(prover);
        engine.claimExecution(requestId);

        vm.deal(address(engine), 1 ether);
        vm.prank(prover);
        engine.submitProof(requestId, hex"deadbeef", hex"cafebabe");

        // Proof still completed despite callback revert
        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);
        assertEq(uint8(req.status), uint8(ExecutionEngine.RequestStatus.Completed));
    }

    function test_gasGuzzlingCallbackDoesNotBlockProof() public {
        // Register program
        vm.prank(prover);
        programRegistry.registerProgram(imageId, "test", "url", bytes32(0));

        // Deploy gas-consuming callback
        GasGuzzlingCallback gasCallback = new GasGuzzlingCallback();

        // Create request with gas-heavy callback
        vm.prank(requester);
        uint256 requestId =
            engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, "url", address(gasCallback), 3600);

        // Claim and submit proof
        vm.prank(prover);
        engine.claimExecution(requestId);

        vm.deal(address(engine), 1 ether);
        vm.prank(prover);
        engine.submitProof(requestId, hex"deadbeef", hex"cafebabe");

        // Proof still completed
        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);
        assertEq(uint8(req.status), uint8(ExecutionEngine.RequestStatus.Completed));
    }

    // ========================================================================
    // 17. Claim reclaim after deadline
    // ========================================================================

    function test_reclaimAfterDeadlineAndResubmit() public {
        // Register program
        vm.prank(prover);
        programRegistry.registerProgram(imageId, "test", "url", bytes32(0));

        // Create request
        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, "url", address(0), 3600);

        // Prover 1 claims
        vm.prank(prover);
        engine.claimExecution(requestId);

        // Prover 1 fails to submit — deadline passes
        vm.warp(block.timestamp + 11 minutes);

        // Prover 2 reclaims
        address prover2 = address(0x4444);
        vm.deal(prover2, 10 ether);
        vm.prank(prover2);
        engine.claimExecution(requestId);

        // Prover 2 successfully submits
        vm.deal(address(engine), 1 ether);
        vm.prank(prover2);
        engine.submitProof(requestId, hex"deadbeef", hex"cafebabe");

        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);
        assertEq(uint8(req.status), uint8(ExecutionEngine.RequestStatus.Completed));
        assertEq(req.claimedBy, prover2);
    }

    // ========================================================================
    // 18. Tip decay over time
    // ========================================================================

    function test_tipDecayAffectsPayoutAmounts() public {
        // Register program
        vm.prank(prover);
        programRegistry.registerProgram(imageId, "test", "url", bytes32(0));

        // Create request with 1 ETH tip
        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 1 ether}(imageId, inputDigest, "url", address(0), 3600);

        // Immediate tip should be ~1 ETH
        uint256 tipAtStart = engine.getCurrentTip(requestId);
        assertEq(tipAtStart, 1 ether);

        // After half the decay period (15 min), tip should be ~0.75 ETH
        vm.warp(block.timestamp + 15 minutes);
        uint256 tipMid = engine.getCurrentTip(requestId);
        assertLt(tipMid, tipAtStart);
        assertGt(tipMid, 0.5 ether);

        // After full decay period (30 min), tip should be 0.5 ETH
        vm.warp(block.timestamp + 15 minutes);
        uint256 tipAfterDecay = engine.getCurrentTip(requestId);
        assertEq(tipAfterDecay, 0.5 ether);
    }

    // ========================================================================
    // 19. Empty seal/journal rejection
    // ========================================================================

    function test_emptyProofInputsRejected() public {
        // Register program
        vm.prank(prover);
        programRegistry.registerProgram(imageId, "test", "url", bytes32(0));

        // Create and claim request
        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, "url", address(0), 3600);
        vm.prank(prover);
        engine.claimExecution(requestId);

        // Empty seal
        vm.prank(prover);
        vm.expectRevert(ExecutionEngine.EmptySeal.selector);
        engine.submitProof(requestId, "", hex"cafebabe");

        // Empty journal
        vm.prank(prover);
        vm.expectRevert(ExecutionEngine.EmptyJournal.selector);
        engine.submitProof(requestId, hex"deadbeef", "");
    }

    // ========================================================================
    // 20. Fee recipient receives correct fees
    // ========================================================================

    function test_protocolFeeDistribution() public {
        // Register program
        vm.prank(prover);
        programRegistry.registerProgram(imageId, "test", "url", bytes32(0));

        address feeAddr = address(0xFEE);
        uint256 feeBalBefore = feeAddr.balance;

        // Create request with 1 ETH tip
        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 1 ether}(imageId, inputDigest, "url", address(0), 3600);

        // Claim and submit immediately (no tip decay)
        vm.prank(prover);
        engine.claimExecution(requestId);

        vm.deal(address(engine), 2 ether); // ensure enough funds
        vm.prank(prover);
        engine.submitProof(requestId, hex"deadbeef", hex"cafebabe");

        // Protocol fee = 2.5% of 1 ETH = 0.025 ETH
        uint256 feeReceived = feeAddr.balance - feeBalBefore;
        assertEq(feeReceived, 0.025 ether);
    }
}
