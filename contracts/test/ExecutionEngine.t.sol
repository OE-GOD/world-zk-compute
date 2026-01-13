// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/ExecutionEngine.sol";
import "../src/ProgramRegistry.sol";
import "../src/MockRiscZeroVerifier.sol";

contract ExecutionEngineTest is Test {
    ExecutionEngine public engine;
    ProgramRegistry public registry;
    MockRiscZeroVerifier public verifier;

    address public deployer = address(1);
    address public requester = address(2);
    address public prover = address(3);
    address public feeRecipient = address(4);

    bytes32 public imageId = bytes32(uint256(1));
    bytes32 public inputDigest = keccak256("test inputs");
    string public inputUrl = "ipfs://QmTest";

    function setUp() public {
        vm.startPrank(deployer);

        // Deploy contracts
        verifier = new MockRiscZeroVerifier();
        registry = new ProgramRegistry();
        engine = new ExecutionEngine(
            address(registry),
            address(verifier),
            feeRecipient
        );

        // Register a test program
        registry.registerProgram(
            imageId,
            "Test Program",
            "https://example.com/test.elf",
            bytes32(0)
        );

        vm.stopPrank();

        // Fund accounts
        vm.deal(requester, 10 ether);
        vm.deal(prover, 1 ether);
    }

    // ========================================================================
    // REQUEST TESTS
    // ========================================================================

    function testRequestExecution() public {
        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(
            imageId,
            inputDigest,
            inputUrl,
            address(0),
            3600
        );

        assertEq(requestId, 1);

        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);
        assertEq(req.requester, requester);
        assertEq(req.imageId, imageId);
        assertEq(req.inputDigest, inputDigest);
        assertEq(req.tip, 0.1 ether);
        assertEq(uint256(req.status), uint256(ExecutionEngine.RequestStatus.Pending));
    }

    function testRequestExecutionInsufficientTip() public {
        vm.prank(requester);
        vm.expectRevert(ExecutionEngine.InsufficientTip.selector);
        engine.requestExecution{value: 0.00001 ether}(
            imageId,
            inputDigest,
            inputUrl,
            address(0),
            3600
        );
    }

    function testRequestExecutionInactiveProgram() public {
        bytes32 fakeImageId = bytes32(uint256(999));

        vm.prank(requester);
        vm.expectRevert(ExecutionEngine.ProgramNotActive.selector);
        engine.requestExecution{value: 0.1 ether}(
            fakeImageId,
            inputDigest,
            inputUrl,
            address(0),
            3600
        );
    }

    function testCancelExecution() public {
        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(
            imageId,
            inputDigest,
            inputUrl,
            address(0),
            3600
        );

        uint256 balanceBefore = requester.balance;

        vm.prank(requester);
        engine.cancelExecution(requestId);

        uint256 balanceAfter = requester.balance;
        assertEq(balanceAfter - balanceBefore, 0.1 ether);

        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);
        assertEq(uint256(req.status), uint256(ExecutionEngine.RequestStatus.Cancelled));
    }

    // ========================================================================
    // CLAIM TESTS
    // ========================================================================

    function testClaimExecution() public {
        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(
            imageId,
            inputDigest,
            inputUrl,
            address(0),
            3600
        );

        vm.prank(prover);
        engine.claimExecution(requestId);

        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);
        assertEq(req.claimedBy, prover);
        assertEq(uint256(req.status), uint256(ExecutionEngine.RequestStatus.Claimed));
    }

    function testClaimExecutionExpiredRequest() public {
        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(
            imageId,
            inputDigest,
            inputUrl,
            address(0),
            3600
        );

        // Fast forward past expiration
        vm.warp(block.timestamp + 3601);

        vm.prank(prover);
        vm.expectRevert(ExecutionEngine.RequestExpired.selector);
        engine.claimExecution(requestId);
    }

    function testReclaimAfterDeadline() public {
        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(
            imageId,
            inputDigest,
            inputUrl,
            address(0),
            3600
        );

        // First prover claims
        vm.prank(prover);
        engine.claimExecution(requestId);

        // Fast forward past claim deadline (10 minutes)
        vm.warp(block.timestamp + 601);

        // Second prover can reclaim
        address prover2 = address(5);
        vm.prank(prover2);
        engine.claimExecution(requestId);

        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);
        assertEq(req.claimedBy, prover2);
    }

    // ========================================================================
    // PROOF SUBMISSION TESTS
    // ========================================================================

    function testSubmitProof() public {
        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(
            imageId,
            inputDigest,
            inputUrl,
            address(0),
            3600
        );

        vm.prank(prover);
        engine.claimExecution(requestId);

        bytes memory seal = hex"deadbeef";
        bytes memory journal = hex"cafebabe";

        uint256 proverBalanceBefore = prover.balance;
        uint256 feeRecipientBalanceBefore = feeRecipient.balance;

        vm.prank(prover);
        engine.submitProof(requestId, seal, journal);

        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);
        assertEq(uint256(req.status), uint256(ExecutionEngine.RequestStatus.Completed));

        // Check prover got paid (minus 2.5% fee)
        uint256 expectedPayout = 0.1 ether * 9750 / 10000; // 97.5%
        uint256 expectedFee = 0.1 ether * 250 / 10000; // 2.5%

        assertApproxEqAbs(prover.balance - proverBalanceBefore, expectedPayout, 0.001 ether);
        assertApproxEqAbs(feeRecipient.balance - feeRecipientBalanceBefore, expectedFee, 0.001 ether);
    }

    function testSubmitProofNotClaimant() public {
        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(
            imageId,
            inputDigest,
            inputUrl,
            address(0),
            3600
        );

        vm.prank(prover);
        engine.claimExecution(requestId);

        address attacker = address(6);
        vm.prank(attacker);
        vm.expectRevert(ExecutionEngine.NotClaimant.selector);
        engine.submitProof(requestId, hex"deadbeef", hex"cafebabe");
    }

    function testSubmitProofAfterDeadline() public {
        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(
            imageId,
            inputDigest,
            inputUrl,
            address(0),
            3600
        );

        vm.prank(prover);
        engine.claimExecution(requestId);

        // Fast forward past claim deadline
        vm.warp(block.timestamp + 601);

        vm.prank(prover);
        vm.expectRevert(ExecutionEngine.ClaimDeadlinePassed.selector);
        engine.submitProof(requestId, hex"deadbeef", hex"cafebabe");
    }

    // ========================================================================
    // TIP DECAY TESTS
    // ========================================================================

    function testTipDecay() public {
        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(
            imageId,
            inputDigest,
            inputUrl,
            address(0),
            3600
        );

        // At t=0, tip should be max
        uint256 tip0 = engine.getCurrentTip(requestId);
        assertEq(tip0, 0.1 ether);

        // At t=15 minutes, tip should be ~75%
        vm.warp(block.timestamp + 15 minutes);
        uint256 tip15 = engine.getCurrentTip(requestId);
        assertApproxEqAbs(tip15, 0.075 ether, 0.001 ether);

        // At t=30+ minutes, tip should be at floor (50%)
        vm.warp(block.timestamp + 30 minutes);
        uint256 tip30 = engine.getCurrentTip(requestId);
        assertEq(tip30, 0.05 ether);
    }

    // ========================================================================
    // PROVER STATS TESTS
    // ========================================================================

    function testProverStats() public {
        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(
            imageId,
            inputDigest,
            inputUrl,
            address(0),
            3600
        );

        vm.prank(prover);
        engine.claimExecution(requestId);

        vm.prank(prover);
        engine.submitProof(requestId, hex"deadbeef", hex"cafebabe");

        (uint256 completed, uint256 earnings) = engine.getProverStats(prover);
        assertEq(completed, 1);
        assertGt(earnings, 0);
    }

    // ========================================================================
    // VIEW FUNCTION TESTS
    // ========================================================================

    function testGetPendingRequests() public {
        // Create 3 requests
        vm.startPrank(requester);
        engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);
        engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);
        engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);
        vm.stopPrank();

        uint256[] memory pending = engine.getPendingRequests(0, 10);
        assertEq(pending.length, 3);
    }
}
