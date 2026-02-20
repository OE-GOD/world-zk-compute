// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/ExecutionEngine.sol";
import "../src/ProgramRegistry.sol";
import "../src/MockRiscZeroVerifier.sol";

contract MockCallback is IExecutionCallback {
    uint256 public lastRequestId;
    bytes32 public lastImageId;
    bytes public lastJournal;

    function onExecutionComplete(uint256 requestId, bytes32 imageId, bytes calldata journal) external {
        lastRequestId = requestId;
        lastImageId = imageId;
        lastJournal = journal;
    }
}

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
        engine = new ExecutionEngine(address(registry), address(verifier), feeRecipient);

        // Register a test program
        registry.registerProgram(imageId, "Test Program", "https://example.com/test.elf", bytes32(0));

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
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

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
        engine.requestExecution{value: 0.00001 ether}(imageId, inputDigest, inputUrl, address(0), 3600);
    }

    function testRequestExecutionInactiveProgram() public {
        bytes32 fakeImageId = bytes32(uint256(999));

        vm.prank(requester);
        vm.expectRevert(ExecutionEngine.ProgramNotActive.selector);
        engine.requestExecution{value: 0.1 ether}(fakeImageId, inputDigest, inputUrl, address(0), 3600);
    }

    function testCancelExecution() public {
        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

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
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        vm.prank(prover);
        engine.claimExecution(requestId);

        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);
        assertEq(req.claimedBy, prover);
        assertEq(uint256(req.status), uint256(ExecutionEngine.RequestStatus.Claimed));
    }

    function testClaimExecutionExpiredRequest() public {
        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        // Fast forward past expiration
        vm.warp(block.timestamp + 3601);

        vm.prank(prover);
        vm.expectRevert(ExecutionEngine.RequestExpired.selector);
        engine.claimExecution(requestId);
    }

    function testReclaimAfterDeadline() public {
        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

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
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

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
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        vm.prank(prover);
        engine.claimExecution(requestId);

        address attacker = address(6);
        vm.prank(attacker);
        vm.expectRevert(ExecutionEngine.NotClaimant.selector);
        engine.submitProof(requestId, hex"deadbeef", hex"cafebabe");
    }

    function testSubmitProofAfterDeadline() public {
        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

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
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

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
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

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

    // ========================================================================
    // PACKED STRUCT LAYOUT TESTS
    // ========================================================================

    function testPackedStructFieldsAfterRequest() public {
        vm.warp(1000);

        address callback = address(0xBEEF);

        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(
            imageId,
            inputDigest,
            inputUrl,
            callback,
            7200 // 2h expiration
        );

        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);

        // Slot 0
        assertEq(req.id, 1);
        // Slot 1
        assertEq(req.imageId, imageId);
        // Slot 2
        assertEq(req.inputDigest, inputDigest);
        // Slot 3 (packed: requester + createdAt + expiresAt)
        assertEq(req.requester, requester);
        assertEq(req.createdAt, 1000);
        assertEq(req.expiresAt, 1000 + 7200);
        // Slot 4 (packed: callbackContract + status)
        assertEq(req.callbackContract, callback);
        assertEq(uint256(req.status), uint256(ExecutionEngine.RequestStatus.Pending));
        // Slot 5 (packed: claimedBy + claimedAt + claimDeadline)
        assertEq(req.claimedBy, address(0));
        assertEq(req.claimedAt, 0);
        assertEq(req.claimDeadline, 0);
        // Slot 6
        assertEq(req.tip, 0.1 ether);
        // Slot 7
        assertEq(req.maxTip, 0.1 ether);
    }

    function testPackedStructFieldsAfterClaim() public {
        vm.warp(2000);

        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        vm.warp(2100);
        vm.prank(prover);
        engine.claimExecution(requestId);

        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);

        // Slot 3 unchanged from request
        assertEq(req.requester, requester);
        assertEq(req.createdAt, 2000);
        assertEq(req.expiresAt, 2000 + 3600);
        // Slot 4 status updated
        assertEq(req.callbackContract, address(0));
        assertEq(uint256(req.status), uint256(ExecutionEngine.RequestStatus.Claimed));
        // Slot 5 fully populated
        assertEq(req.claimedBy, prover);
        assertEq(req.claimedAt, 2100);
        assertEq(req.claimDeadline, 2100 + 600); // CLAIM_WINDOW = 10 min
    }

    function testPackedStructFieldsAfterSubmitProof() public {
        MockCallback callback = new MockCallback();

        vm.warp(3000);
        vm.prank(requester);
        uint256 requestId =
            engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(callback), 3600);

        vm.warp(3100);
        vm.prank(prover);
        engine.claimExecution(requestId);

        vm.warp(3200);
        vm.prank(prover);
        engine.submitProof(requestId, hex"deadbeef", hex"cafebabe");

        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);

        // Slot 3 unchanged
        assertEq(req.requester, requester);
        assertEq(req.createdAt, 3000);
        assertEq(req.expiresAt, 3000 + 3600);
        // Slot 4: status updated to Completed, callback intact
        assertEq(req.callbackContract, address(callback));
        assertEq(uint256(req.status), uint256(ExecutionEngine.RequestStatus.Completed));
        // Slot 5 unchanged from claim
        assertEq(req.claimedBy, prover);
        assertEq(req.claimedAt, 3100);
        assertEq(req.claimDeadline, 3100 + 600);
    }

    function testUint48MaxTimestamp() public {
        uint48 nearMax = type(uint48).max - 3600;
        vm.warp(uint256(nearMax));

        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);
        assertEq(req.createdAt, nearMax);
        assertEq(req.expiresAt, nearMax + 3600);

        vm.warp(uint256(nearMax) + 100);
        vm.prank(prover);
        engine.claimExecution(requestId);

        req = engine.getRequest(requestId);
        assertEq(req.claimedAt, nearMax + 100);
        assertEq(req.claimDeadline, nearMax + 100 + 600);
    }

    function testStatusEnumPacksWithCallbackAddress() public {
        MockCallback callback = new MockCallback();

        vm.prank(requester);
        uint256 requestId =
            engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(callback), 3600);

        // Pending: callback intact
        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);
        assertEq(req.callbackContract, address(callback));
        assertEq(uint256(req.status), uint256(ExecutionEngine.RequestStatus.Pending));

        // Claimed: callback still intact
        vm.prank(prover);
        engine.claimExecution(requestId);
        req = engine.getRequest(requestId);
        assertEq(req.callbackContract, address(callback));
        assertEq(uint256(req.status), uint256(ExecutionEngine.RequestStatus.Claimed));

        // Completed: callback still intact
        vm.prank(prover);
        engine.submitProof(requestId, hex"deadbeef", hex"cafebabe");
        req = engine.getRequest(requestId);
        assertEq(req.callbackContract, address(callback));
        assertEq(uint256(req.status), uint256(ExecutionEngine.RequestStatus.Completed));
    }

    function testInputUrlEmittedNotStored() public {
        string memory testInputUrl = "ipfs://QmTestInputData12345";

        vm.recordLogs();

        vm.prank(requester);
        uint256 requestId =
            engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, testInputUrl, address(0), 3600);

        Vm.Log[] memory logs = vm.getRecordedLogs();

        // Find the ExecutionRequested event
        // event ExecutionRequested(uint256 indexed requestId, address indexed requester, bytes32 indexed imageId, bytes32 inputDigest, string inputUrl, uint8 inputType, uint256 tip, uint256 expiresAt)
        bool found = false;
        for (uint256 i = 0; i < logs.length; i++) {
            // Topic 0 is the event signature
            if (
                logs[i].topics[0]
                    == keccak256("ExecutionRequested(uint256,address,bytes32,bytes32,string,uint8,uint256,uint256)")
            ) {
                // Decode non-indexed params: inputDigest, inputUrl, inputType, tip, expiresAt
                (bytes32 evInputDigest, string memory evInputUrl, uint8 evInputType, uint256 evTip,) =
                    abi.decode(logs[i].data, (bytes32, string, uint8, uint256, uint256));
                assertEq(evInputUrl, testInputUrl);
                assertEq(evInputDigest, inputDigest);
                assertEq(evInputType, 0); // Default public
                assertEq(evTip, 0.1 ether);
                found = true;
                break;
            }
        }
        assertTrue(found, "ExecutionRequested event not found");

        // Struct has no inputUrl field — getRequest compiles and returns without it
        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);
        assertEq(req.id, requestId);
    }

    function testPrivateInputRequestEmitsInputType1() public {
        vm.recordLogs();

        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(
            imageId, inputDigest, "https://auth.example.com/inputs", address(0), 3600, 1
        );

        Vm.Log[] memory logs = vm.getRecordedLogs();

        bool found = false;
        for (uint256 i = 0; i < logs.length; i++) {
            if (
                logs[i].topics[0]
                    == keccak256("ExecutionRequested(uint256,address,bytes32,bytes32,string,uint8,uint256,uint256)")
            ) {
                (,, uint8 evInputType,,) = abi.decode(logs[i].data, (bytes32, string, uint8, uint256, uint256));
                assertEq(evInputType, 1); // Private
                found = true;
                break;
            }
        }
        assertTrue(found, "ExecutionRequested event not found");

        // Verify request was created correctly
        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);
        assertEq(req.id, requestId);
        assertEq(req.tip, 0.1 ether);
    }

    function testBackwardCompatibleOverloadEmitsInputType0() public {
        vm.recordLogs();

        vm.prank(requester);
        engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        Vm.Log[] memory logs = vm.getRecordedLogs();

        bool found = false;
        for (uint256 i = 0; i < logs.length; i++) {
            if (
                logs[i].topics[0]
                    == keccak256("ExecutionRequested(uint256,address,bytes32,bytes32,string,uint8,uint256,uint256)")
            ) {
                (,, uint8 evInputType,,) = abi.decode(logs[i].data, (bytes32, string, uint8, uint256, uint256));
                assertEq(evInputType, 0); // Public (default)
                found = true;
                break;
            }
        }
        assertTrue(found, "ExecutionRequested event not found");
    }

    function testMultipleRequestsIsolation() public {
        address callback1 = address(0xAAAA);
        address callback2 = address(0xBBBB);

        vm.warp(5000);
        vm.prank(requester);
        uint256 id1 = engine.requestExecution{value: 0.2 ether}(imageId, inputDigest, inputUrl, callback1, 3600);

        vm.warp(6000);
        vm.prank(requester);
        uint256 id2 = engine.requestExecution{value: 0.3 ether}(imageId, inputDigest, inputUrl, callback2, 7200);

        // Verify isolation after creation
        ExecutionEngine.ExecutionRequest memory req1 = engine.getRequest(id1);
        ExecutionEngine.ExecutionRequest memory req2 = engine.getRequest(id2);

        assertEq(req1.createdAt, 5000);
        assertEq(req1.expiresAt, 5000 + 3600);
        assertEq(req1.tip, 0.2 ether);
        assertEq(req1.callbackContract, callback1);

        assertEq(req2.createdAt, 6000);
        assertEq(req2.expiresAt, 6000 + 7200);
        assertEq(req2.tip, 0.3 ether);
        assertEq(req2.callbackContract, callback2);

        // Claim only request 1
        vm.warp(6100);
        vm.prank(prover);
        engine.claimExecution(id1);

        // Request 1 is Claimed, request 2 still Pending
        req1 = engine.getRequest(id1);
        req2 = engine.getRequest(id2);

        assertEq(uint256(req1.status), uint256(ExecutionEngine.RequestStatus.Claimed));
        assertEq(req1.claimedBy, prover);
        assertEq(req1.claimedAt, 6100);

        assertEq(uint256(req2.status), uint256(ExecutionEngine.RequestStatus.Pending));
        assertEq(req2.claimedBy, address(0));
        assertEq(req2.claimedAt, 0);
        // Request 2 fields unchanged
        assertEq(req2.createdAt, 6000);
        assertEq(req2.tip, 0.3 ether);
        assertEq(req2.callbackContract, callback2);
    }
}
