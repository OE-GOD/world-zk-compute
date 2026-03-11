// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/ExecutionEngine.sol";
import "../src/ProgramRegistry.sol";
import "../src/MockRiscZeroVerifier.sol";
import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";

contract ExecutionEngineFuzzTest is Test {
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
        verifier = new MockRiscZeroVerifier();
        registry = new ProgramRegistry(deployer);
        engine = new ExecutionEngine(deployer, address(registry), address(verifier), feeRecipient);
        registry.registerProgram(imageId, "Test Program", "https://example.com/test.elf", bytes32(0));
        vm.stopPrank();

        vm.deal(requester, 100 ether);
        vm.deal(prover, 10 ether);
    }

    // ========================================================================
    // 1. testFuzz_requestExecution_tip
    //    Bound to [0, 10 ether]. Below MIN_TIP reverts; at or above succeeds.
    // ========================================================================

    function testFuzz_requestExecution_tip(uint256 tip) public {
        tip = bound(tip, 0, 10 ether);

        if (tip < engine.MIN_TIP()) {
            vm.prank(requester);
            vm.expectRevert(ExecutionEngine.InsufficientTip.selector);
            engine.requestExecution{value: tip}(imageId, inputDigest, inputUrl, address(0), 3600, 0);
        } else {
            vm.prank(requester);
            uint256 requestId = engine.requestExecution{value: tip}(imageId, inputDigest, inputUrl, address(0), 3600, 0);

            ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);
            assertEq(req.tip, tip, "Stored tip must match sent value");
            assertEq(req.requester, requester, "Requester must be msg.sender");
            assertEq(uint256(req.status), uint256(ExecutionEngine.RequestStatus.Pending), "Status must be Pending");
        }
    }

    // ========================================================================
    // 2. testFuzz_requestExecution_zeroImageId
    //    Bound tip >= MIN_TIP. bytes32(0) imageId always reverts ZeroImageId.
    // ========================================================================

    function testFuzz_requestExecution_zeroImageId(uint256 tip) public {
        tip = bound(tip, engine.MIN_TIP(), 10 ether);

        vm.prank(requester);
        vm.expectRevert(ExecutionEngine.ZeroImageId.selector);
        engine.requestExecution{value: tip}(bytes32(0), inputDigest, inputUrl, address(0), 3600, 0);
    }

    // ========================================================================
    // 3. testFuzz_cancelExecution_refund
    //    Bound tip to [MIN_TIP, 1 ether]. Request, cancel, verify full refund.
    // ========================================================================

    function testFuzz_cancelExecution_refund(uint256 tip) public {
        tip = bound(tip, engine.MIN_TIP(), 1 ether);

        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: tip}(imageId, inputDigest, inputUrl, address(0), 3600, 0);

        uint256 balanceBefore = requester.balance;

        vm.prank(requester);
        engine.cancelExecution(requestId);

        uint256 balanceAfter = requester.balance;
        assertEq(balanceAfter - balanceBefore, tip, "Full tip must be refunded on cancel");

        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);
        assertEq(uint256(req.status), uint256(ExecutionEngine.RequestStatus.Cancelled), "Status must be Cancelled");
    }

    // ========================================================================
    // 4. testFuzz_claimExecution_nonExistent
    //    Claim random requestId -- should revert RequestNotFound.
    // ========================================================================

    function testFuzz_claimExecution_nonExistent(uint256 requestId) public {
        vm.prank(prover);
        vm.expectRevert(ExecutionEngine.RequestNotFound.selector);
        engine.claimExecution(requestId);
    }

    // ========================================================================
    // 5. testFuzz_protocolFee_bounds
    //    If feeBps > 1000, expect revert. Otherwise, success. Call as owner.
    // ========================================================================

    function testFuzz_protocolFee_bounds(uint256 feeBps) public {
        vm.prank(deployer);
        if (feeBps > 1000) {
            vm.expectRevert("Fee too high");
            engine.setProtocolFee(feeBps);
        } else {
            engine.setProtocolFee(feeBps);
            assertEq(engine.protocolFeeBps(), feeBps, "Protocol fee must be updated");
        }
    }

    // ========================================================================
    // 6. testFuzz_protocolFee_nonOwner
    //    Non-owner (not deployer) always reverts OwnableUnauthorizedAccount.
    // ========================================================================

    function testFuzz_protocolFee_nonOwner(address caller, uint256 feeBps) public {
        vm.assume(caller != deployer);

        vm.prank(caller);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, caller));
        engine.setProtocolFee(feeBps);
    }

    // ========================================================================
    // 7. testFuzz_setFeeRecipient
    //    If recipient == address(0), revert ZeroAddress. Otherwise success.
    // ========================================================================

    function testFuzz_setFeeRecipient(address recipient) public {
        vm.prank(deployer);
        if (recipient == address(0)) {
            vm.expectRevert(ExecutionEngine.ZeroAddress.selector);
            engine.setFeeRecipient(recipient);
        } else {
            engine.setFeeRecipient(recipient);
            assertEq(engine.feeRecipient(), recipient, "Fee recipient must be updated");
        }
    }

    // ========================================================================
    // 8. testFuzz_cancelExecution_notRequester
    //    Anyone other than requester cannot cancel. Bound tip >= MIN_TIP.
    // ========================================================================

    function testFuzz_cancelExecution_notRequester(address caller, uint256 tip) public {
        vm.assume(caller != requester);
        tip = bound(tip, engine.MIN_TIP(), 1 ether);

        vm.prank(requester);
        uint256 requestId = engine.requestExecution{value: tip}(imageId, inputDigest, inputUrl, address(0), 3600, 0);

        vm.prank(caller);
        vm.expectRevert(ExecutionEngine.NotRequester.selector);
        engine.cancelExecution(requestId);

        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);
        assertEq(uint256(req.status), uint256(ExecutionEngine.RequestStatus.Pending), "Status must still be Pending");
    }
}
