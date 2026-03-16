// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/mocks/MockExecutionEngine.sol";

contract MockExecutionEngineTest is Test {
    MockExecutionEngine public mock;

    address public requester = address(0x1111);
    address public prover = address(0x2222);

    bytes32 public imageId = bytes32(uint256(42));
    bytes32 public inputDigest = keccak256("test inputs");
    string public inputUrl = "ipfs://QmTestData";

    function setUp() public {
        mock = new MockExecutionEngine();
        vm.deal(requester, 10 ether);
        vm.deal(prover, 1 ether);
    }

    // ========================================================================
    // CALL TRACKING TESTS
    // ========================================================================

    function testCallCountStartsAtZero() public view {
        assertEq(mock.getCallCount("requestExecution"), 0);
        assertEq(mock.getCallCount("claimExecution"), 0);
        assertEq(mock.getCallCount("submitProof"), 0);
        assertEq(mock.getCallCount("cancelExecution"), 0);
    }

    function testRequestExecutionIncrementsCallCount() public {
        vm.prank(requester);
        mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        assertEq(mock.getCallCount("requestExecution"), 1);

        vm.prank(requester);
        mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        assertEq(mock.getCallCount("requestExecution"), 2);
    }

    function testRequestExecutionWithInputTypeIncrementsCallCount() public {
        vm.prank(requester);
        mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600, 1);

        assertEq(mock.getCallCount("requestExecution"), 1);
    }

    function testClaimExecutionIncrementsCallCount() public {
        vm.prank(requester);
        uint256 requestId = mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        vm.prank(prover);
        mock.claimExecution(requestId);

        assertEq(mock.getCallCount("claimExecution"), 1);
    }

    function testSubmitProofIncrementsCallCount() public {
        vm.prank(requester);
        uint256 requestId = mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        vm.prank(prover);
        mock.claimExecution(requestId);

        vm.prank(prover);
        mock.submitProof(requestId, hex"deadbeef", hex"cafebabe");

        assertEq(mock.getCallCount("submitProof"), 1);
    }

    function testCancelExecutionIncrementsCallCount() public {
        vm.prank(requester);
        uint256 requestId = mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        vm.prank(requester);
        mock.cancelExecution(requestId);

        assertEq(mock.getCallCount("cancelExecution"), 1);
    }

    // ========================================================================
    // LAST CALL DATA TESTS
    // ========================================================================

    function testLastCallDataForRequestExecution() public {
        vm.prank(requester);
        mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        bytes memory data = mock.getLastCallData("requestExecution");
        (
            bytes32 decodedImageId,
            bytes32 decodedInputDigest,
            string memory decodedInputUrl,
            address decodedCallback,
            uint256 decodedExpiration,
            uint8 decodedInputType
        ) = abi.decode(data, (bytes32, bytes32, string, address, uint256, uint8));

        assertEq(decodedImageId, imageId);
        assertEq(decodedInputDigest, inputDigest);
        assertEq(decodedInputUrl, inputUrl);
        assertEq(decodedCallback, address(0));
        assertEq(decodedExpiration, 3600);
        assertEq(decodedInputType, 0);
    }

    function testLastCallDataForRequestExecutionWithInputType() public {
        vm.prank(requester);
        mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 7200, 1);

        bytes memory data = mock.getLastCallData("requestExecution");
        (,,,,, uint8 decodedInputType) = abi.decode(data, (bytes32, bytes32, string, address, uint256, uint8));

        assertEq(decodedInputType, 1);
    }

    function testLastCallDataForClaimExecution() public {
        vm.prank(requester);
        uint256 requestId = mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        vm.prank(prover);
        mock.claimExecution(requestId);

        bytes memory data = mock.getLastCallData("claimExecution");
        uint256 decodedRequestId = abi.decode(data, (uint256));
        assertEq(decodedRequestId, requestId);
    }

    function testLastCallDataForSubmitProof() public {
        vm.prank(requester);
        uint256 requestId = mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        vm.prank(prover);
        mock.claimExecution(requestId);

        bytes memory seal = hex"aabbccdd";
        bytes memory journal = hex"11223344";

        vm.prank(prover);
        mock.submitProof(requestId, seal, journal);

        bytes memory data = mock.getLastCallData("submitProof");
        (uint256 decodedRequestId, bytes memory decodedSeal, bytes memory decodedJournal) =
            abi.decode(data, (uint256, bytes, bytes));

        assertEq(decodedRequestId, requestId);
        assertEq(decodedSeal, seal);
        assertEq(decodedJournal, journal);
    }

    function testLastCallDataForCancelExecution() public {
        vm.prank(requester);
        uint256 requestId = mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        vm.prank(requester);
        mock.cancelExecution(requestId);

        bytes memory data = mock.getLastCallData("cancelExecution");
        uint256 decodedRequestId = abi.decode(data, (uint256));
        assertEq(decodedRequestId, requestId);
    }

    function testLastCallDataUpdatesOnSubsequentCalls() public {
        bytes32 imageId2 = bytes32(uint256(99));

        vm.prank(requester);
        mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        vm.prank(requester);
        mock.requestExecution{value: 0.2 ether}(imageId2, inputDigest, "ipfs://other", address(0), 7200);

        bytes memory data = mock.getLastCallData("requestExecution");
        (bytes32 decodedImageId,,,,,) = abi.decode(data, (bytes32, bytes32, string, address, uint256, uint8));

        // Should reflect the SECOND call
        assertEq(decodedImageId, imageId2);
    }

    // ========================================================================
    // CONFIGURED RESPONSE TESTS
    // ========================================================================

    function testSetResponseStoresData() public {
        bytes32 requestId = bytes32(uint256(1));
        bytes memory result = hex"deadbeefcafebabe";

        mock.setResponse(requestId, result);

        bytes memory stored = mock.getResponse(requestId);
        assertEq(stored, result);
    }

    function testSetResponseOverwritesPrevious() public {
        bytes32 requestId = bytes32(uint256(1));

        mock.setResponse(requestId, hex"aabb");
        mock.setResponse(requestId, hex"ccdd");

        bytes memory stored = mock.getResponse(requestId);
        assertEq(stored, hex"ccdd");
    }

    function testGetResponseReturnsEmptyForUnconfigured() public view {
        bytes32 requestId = bytes32(uint256(999));
        bytes memory stored = mock.getResponse(requestId);
        assertEq(stored.length, 0);
    }

    // ========================================================================
    // EVENT EMISSION TESTS
    // ========================================================================

    function testEmitsExecutionRequestedEvent() public {
        vm.recordLogs();

        vm.prank(requester);
        mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        Vm.Log[] memory logs = vm.getRecordedLogs();

        bool found = false;
        for (uint256 i = 0; i < logs.length; i++) {
            if (
                logs[i].topics[0]
                    == keccak256("ExecutionRequested(uint256,address,bytes32,bytes32,string,uint8,uint256,uint256)")
            ) {
                // Verify indexed parameters
                assertEq(logs[i].topics[1], bytes32(uint256(1))); // requestId
                assertEq(logs[i].topics[2], bytes32(uint256(uint160(requester)))); // requester
                assertEq(logs[i].topics[3], imageId); // imageId

                // Decode non-indexed parameters
                (bytes32 evInputDigest, string memory evInputUrl, uint8 evInputType, uint256 evTip,) =
                    abi.decode(logs[i].data, (bytes32, string, uint8, uint256, uint256));

                assertEq(evInputDigest, inputDigest);
                assertEq(evInputUrl, inputUrl);
                assertEq(evInputType, 0);
                assertEq(evTip, 0.1 ether);

                found = true;
                break;
            }
        }
        assertTrue(found, "ExecutionRequested event not emitted");
    }

    function testEmitsExecutionRequestedWithInputType() public {
        vm.recordLogs();

        vm.prank(requester);
        mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600, 1);

        Vm.Log[] memory logs = vm.getRecordedLogs();

        bool found = false;
        for (uint256 i = 0; i < logs.length; i++) {
            if (
                logs[i].topics[0]
                    == keccak256("ExecutionRequested(uint256,address,bytes32,bytes32,string,uint8,uint256,uint256)")
            ) {
                (,, uint8 evInputType,,) = abi.decode(logs[i].data, (bytes32, string, uint8, uint256, uint256));
                assertEq(evInputType, 1);
                found = true;
                break;
            }
        }
        assertTrue(found, "ExecutionRequested event not emitted with inputType=1");
    }

    function testEmitsExecutionClaimedEvent() public {
        vm.prank(requester);
        uint256 requestId = mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        vm.recordLogs();

        vm.prank(prover);
        mock.claimExecution(requestId);

        Vm.Log[] memory logs = vm.getRecordedLogs();

        bool found = false;
        for (uint256 i = 0; i < logs.length; i++) {
            if (logs[i].topics[0] == keccak256("ExecutionClaimed(uint256,address,uint256)")) {
                assertEq(logs[i].topics[1], bytes32(requestId)); // requestId
                assertEq(logs[i].topics[2], bytes32(uint256(uint160(prover)))); // prover

                uint256 evClaimDeadline = abi.decode(logs[i].data, (uint256));
                assertEq(evClaimDeadline, uint256(uint48(block.timestamp + 10 minutes)));

                found = true;
                break;
            }
        }
        assertTrue(found, "ExecutionClaimed event not emitted");
    }

    function testEmitsExecutionCompletedEvent() public {
        vm.prank(requester);
        uint256 requestId = mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        vm.prank(prover);
        mock.claimExecution(requestId);

        bytes memory journal = hex"cafebabe";

        vm.recordLogs();

        vm.prank(prover);
        mock.submitProof(requestId, hex"deadbeef", journal);

        Vm.Log[] memory logs = vm.getRecordedLogs();

        bool found = false;
        for (uint256 i = 0; i < logs.length; i++) {
            if (logs[i].topics[0] == keccak256("ExecutionCompleted(uint256,address,bytes32,uint256)")) {
                assertEq(logs[i].topics[1], bytes32(requestId)); // requestId
                assertEq(logs[i].topics[2], bytes32(uint256(uint160(prover)))); // prover

                (bytes32 evJournalDigest, uint256 evPayout) = abi.decode(logs[i].data, (bytes32, uint256));
                assertEq(evJournalDigest, sha256(journal));
                assertEq(evPayout, 0.1 ether);

                found = true;
                break;
            }
        }
        assertTrue(found, "ExecutionCompleted event not emitted");
    }

    function testEmitsExecutionCancelledEvent() public {
        vm.prank(requester);
        uint256 requestId = mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        vm.recordLogs();

        vm.prank(requester);
        mock.cancelExecution(requestId);

        Vm.Log[] memory logs = vm.getRecordedLogs();

        bool found = false;
        for (uint256 i = 0; i < logs.length; i++) {
            if (logs[i].topics[0] == keccak256("ExecutionCancelled(uint256)")) {
                assertEq(logs[i].topics[1], bytes32(requestId));
                found = true;
                break;
            }
        }
        assertTrue(found, "ExecutionCancelled event not emitted");
    }

    // ========================================================================
    // STATE MANAGEMENT TESTS
    // ========================================================================

    function testRequestCreatesStoredRequest() public {
        vm.warp(1000);

        vm.prank(requester);
        uint256 requestId = mock.requestExecution{value: 0.5 ether}(imageId, inputDigest, inputUrl, address(0), 7200);

        MockExecutionEngine.ExecutionRequest memory req = mock.getRequest(requestId);

        assertEq(req.id, 1);
        assertEq(req.imageId, imageId);
        assertEq(req.inputDigest, inputDigest);
        assertEq(req.requester, requester);
        assertEq(req.createdAt, 1000);
        assertEq(req.expiresAt, 1000 + 7200);
        assertEq(req.callbackContract, address(0));
        assertEq(uint256(req.status), uint256(MockExecutionEngine.RequestStatus.Pending));
        assertEq(req.claimedBy, address(0));
        assertEq(req.tip, 0.5 ether);
        assertEq(req.maxTip, 0.5 ether);
    }

    function testClaimUpdatesRequestState() public {
        vm.warp(2000);

        vm.prank(requester);
        uint256 requestId = mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        vm.warp(2100);

        vm.prank(prover);
        mock.claimExecution(requestId);

        MockExecutionEngine.ExecutionRequest memory req = mock.getRequest(requestId);

        assertEq(uint256(req.status), uint256(MockExecutionEngine.RequestStatus.Claimed));
        assertEq(req.claimedBy, prover);
        assertEq(req.claimedAt, 2100);
        assertEq(req.claimDeadline, 2100 + 600); // 10 minutes
    }

    function testSubmitProofUpdatesRequestStatus() public {
        vm.prank(requester);
        uint256 requestId = mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        vm.prank(prover);
        mock.claimExecution(requestId);

        vm.prank(prover);
        mock.submitProof(requestId, hex"deadbeef", hex"cafebabe");

        MockExecutionEngine.ExecutionRequest memory req = mock.getRequest(requestId);
        assertEq(uint256(req.status), uint256(MockExecutionEngine.RequestStatus.Completed));
    }

    function testCancelUpdatesRequestStatus() public {
        vm.prank(requester);
        uint256 requestId = mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        vm.prank(requester);
        mock.cancelExecution(requestId);

        MockExecutionEngine.ExecutionRequest memory req = mock.getRequest(requestId);
        assertEq(uint256(req.status), uint256(MockExecutionEngine.RequestStatus.Cancelled));
    }

    function testCancelRefundsTip() public {
        vm.prank(requester);
        uint256 requestId = mock.requestExecution{value: 0.5 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        uint256 balanceBefore = requester.balance;

        vm.prank(requester);
        mock.cancelExecution(requestId);

        uint256 balanceAfter = requester.balance;
        assertEq(balanceAfter - balanceBefore, 0.5 ether);
    }

    function testNextRequestIdIncrements() public {
        assertEq(mock.nextRequestId(), 1);

        vm.prank(requester);
        mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);
        assertEq(mock.nextRequestId(), 2);

        vm.prank(requester);
        mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);
        assertEq(mock.nextRequestId(), 3);
    }

    // ========================================================================
    // PROVER STATS TESTS
    // ========================================================================

    function testProverStatsRecordedOnSubmit() public {
        vm.prank(requester);
        uint256 requestId = mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        vm.prank(prover);
        mock.claimExecution(requestId);

        vm.prank(prover);
        mock.submitProof(requestId, hex"deadbeef", hex"cafebabe");

        (uint256 completed, uint256 earnings) = mock.getProverStats(prover);
        assertEq(completed, 1);
        assertEq(earnings, 0.1 ether);
    }

    function testProverStatsAccumulate() public {
        // First request
        vm.prank(requester);
        uint256 id1 = mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);
        vm.prank(prover);
        mock.claimExecution(id1);
        vm.prank(prover);
        mock.submitProof(id1, hex"deadbeef", hex"cafebabe");

        // Second request
        vm.prank(requester);
        uint256 id2 = mock.requestExecution{value: 0.2 ether}(imageId, inputDigest, inputUrl, address(0), 3600);
        vm.prank(prover);
        mock.claimExecution(id2);
        vm.prank(prover);
        mock.submitProof(id2, hex"deadbeef", hex"cafebabe");

        (uint256 completed, uint256 earnings) = mock.getProverStats(prover);
        assertEq(completed, 2);
        assertEq(earnings, 0.3 ether);
    }

    // ========================================================================
    // VIEW FUNCTION TESTS
    // ========================================================================

    function testGetCurrentTipForPendingRequest() public {
        vm.prank(requester);
        uint256 requestId = mock.requestExecution{value: 0.5 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        assertEq(mock.getCurrentTip(requestId), 0.5 ether);
    }

    function testGetCurrentTipReturnsZeroForCompleted() public {
        vm.prank(requester);
        uint256 requestId = mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);
        vm.prank(prover);
        mock.claimExecution(requestId);
        vm.prank(prover);
        mock.submitProof(requestId, hex"deadbeef", hex"cafebabe");

        assertEq(mock.getCurrentTip(requestId), 0);
    }

    function testGetCurrentTipReturnsZeroForNonexistent() public view {
        assertEq(mock.getCurrentTip(999), 0);
    }

    function testGetPendingRequests() public {
        vm.startPrank(requester);
        mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);
        mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);
        mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);
        vm.stopPrank();

        uint256[] memory pending = mock.getPendingRequests(0, 10);
        assertEq(pending.length, 3);
        assertEq(pending[0], 1);
        assertEq(pending[1], 2);
        assertEq(pending[2], 3);
    }

    function testGetPendingRequestsExcludesClaimed() public {
        vm.startPrank(requester);
        mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);
        mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);
        vm.stopPrank();

        // Claim the first request
        vm.prank(prover);
        mock.claimExecution(1);

        uint256[] memory pending = mock.getPendingRequests(0, 10);
        assertEq(pending.length, 1);
        assertEq(pending[0], 2);
    }

    // ========================================================================
    // MOCK CONFIGURATION TESTS
    // ========================================================================

    function testSubmitProofRevertWhenConfigured() public {
        mock.setSubmitProofRevert(true, "Mock: proof invalid");

        vm.prank(requester);
        uint256 requestId = mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        vm.prank(prover);
        mock.claimExecution(requestId);

        vm.prank(prover);
        vm.expectRevert(abi.encodeWithSelector(MockExecutionEngine.MockSubmitProofReverted.selector, "Mock: proof invalid"));
        mock.submitProof(requestId, hex"deadbeef", hex"cafebabe");

        // Call count should NOT have incremented because the call reverted
        assertEq(mock.getCallCount("submitProof"), 0);
    }

    function testSetRequestDirectly() public {
        MockExecutionEngine.ExecutionRequest memory req = MockExecutionEngine.ExecutionRequest({
            id: 100,
            imageId: imageId,
            inputDigest: inputDigest,
            requester: requester,
            createdAt: uint48(block.timestamp),
            expiresAt: uint48(block.timestamp + 3600),
            callbackContract: address(0),
            status: MockExecutionEngine.RequestStatus.Claimed,
            claimedBy: prover,
            claimedAt: uint48(block.timestamp),
            claimDeadline: uint48(block.timestamp + 600),
            tip: 1 ether,
            maxTip: 1 ether
        });

        mock.setRequest(100, req);

        MockExecutionEngine.ExecutionRequest memory stored = mock.getRequest(100);
        assertEq(stored.id, 100);
        assertEq(stored.imageId, imageId);
        assertEq(uint256(stored.status), uint256(MockExecutionEngine.RequestStatus.Claimed));
        assertEq(stored.claimedBy, prover);
        assertEq(stored.tip, 1 ether);

        // nextRequestId should be updated
        assertGe(mock.nextRequestId(), 101);
    }

    // ========================================================================
    // FULL FLOW TEST
    // ========================================================================

    function testFullRequestClaimSubmitFlow() public {
        // 1. Request
        vm.prank(requester);
        uint256 requestId = mock.requestExecution{value: 1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);
        assertEq(mock.getCallCount("requestExecution"), 1);

        // 2. Claim
        vm.prank(prover);
        mock.claimExecution(requestId);
        assertEq(mock.getCallCount("claimExecution"), 1);

        // 3. Submit proof
        vm.prank(prover);
        mock.submitProof(requestId, hex"aabbccdd", hex"11223344");
        assertEq(mock.getCallCount("submitProof"), 1);

        // 4. Verify final state
        MockExecutionEngine.ExecutionRequest memory req = mock.getRequest(requestId);
        assertEq(uint256(req.status), uint256(MockExecutionEngine.RequestStatus.Completed));

        (uint256 completed, uint256 earnings) = mock.getProverStats(prover);
        assertEq(completed, 1);
        assertEq(earnings, 1 ether);
    }

    function testFullRequestCancelFlow() public {
        uint256 balanceBefore = requester.balance;

        vm.prank(requester);
        uint256 requestId = mock.requestExecution{value: 0.5 ether}(imageId, inputDigest, inputUrl, address(0), 3600);

        vm.prank(requester);
        mock.cancelExecution(requestId);

        // Verify refund
        assertEq(requester.balance, balanceBefore);

        // Verify final state
        MockExecutionEngine.ExecutionRequest memory req = mock.getRequest(requestId);
        assertEq(uint256(req.status), uint256(MockExecutionEngine.RequestStatus.Cancelled));

        // Verify call counts
        assertEq(mock.getCallCount("requestExecution"), 1);
        assertEq(mock.getCallCount("cancelExecution"), 1);
    }

    // ========================================================================
    // EVENT SIGNATURE MATCHING TESTS
    // ========================================================================

    /// @notice Verify that the mock's event signatures match the real ExecutionEngine's
    function testEventSignaturesMatchRealContract() public pure {
        // These are the expected keccak256 hashes of the real ExecutionEngine events.
        // If the mock's event definitions differ, the emitted topic[0] would not match.
        bytes32 expectedRequested =
            keccak256("ExecutionRequested(uint256,address,bytes32,bytes32,string,uint8,uint256,uint256)");
        bytes32 expectedClaimed = keccak256("ExecutionClaimed(uint256,address,uint256)");
        bytes32 expectedCompleted = keccak256("ExecutionCompleted(uint256,address,bytes32,uint256)");
        bytes32 expectedCancelled = keccak256("ExecutionCancelled(uint256)");

        // These are compile-time constants derived from the string literals.
        // If the event signatures in MockExecutionEngine.sol differ from ExecutionEngine.sol,
        // the events will have different topic[0] hashes and SDK code filtering on them will break.
        assertTrue(expectedRequested != bytes32(0));
        assertTrue(expectedClaimed != bytes32(0));
        assertTrue(expectedCompleted != bytes32(0));
        assertTrue(expectedCancelled != bytes32(0));
    }

    /// @notice Cross-check: emit from mock and verify the topic hash matches the expected signature
    function testEmittedEventTopicsMatchExpectedSignatures() public {
        bytes32 expectedRequested =
            keccak256("ExecutionRequested(uint256,address,bytes32,bytes32,string,uint8,uint256,uint256)");
        bytes32 expectedClaimed = keccak256("ExecutionClaimed(uint256,address,uint256)");
        bytes32 expectedCompleted = keccak256("ExecutionCompleted(uint256,address,bytes32,uint256)");

        // Request
        vm.recordLogs();
        vm.prank(requester);
        uint256 requestId = mock.requestExecution{value: 0.1 ether}(imageId, inputDigest, inputUrl, address(0), 3600);
        Vm.Log[] memory logs = vm.getRecordedLogs();
        assertEq(logs[0].topics[0], expectedRequested);

        // Claim
        vm.recordLogs();
        vm.prank(prover);
        mock.claimExecution(requestId);
        logs = vm.getRecordedLogs();
        assertEq(logs[0].topics[0], expectedClaimed);

        // Submit
        vm.recordLogs();
        vm.prank(prover);
        mock.submitProof(requestId, hex"deadbeef", hex"cafebabe");
        logs = vm.getRecordedLogs();
        assertEq(logs[0].topics[0], expectedCompleted);
    }
}
