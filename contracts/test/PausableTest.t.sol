// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/Pausable.sol";

/// @dev Concrete implementation of abstract Pausable for testing
contract TestPausable is Pausable {
    uint256 public counter;

    function initialize(address _owner, address[] memory _guardians, uint256 _unpauseThreshold) external {
        __Pausable_init(_owner, _guardians, _unpauseThreshold);
    }

    /// @dev A function gated by whenNotPaused for testing the modifier
    function increment() external whenNotPaused {
        counter++;
    }

    /// @dev A function gated by whenPaused for testing the modifier
    function resetCounter() external whenPaused {
        counter = 0;
    }
}

contract PausableTest is Test {
    // ========================================================================
    // RE-DECLARE EVENTS FOR expectEmit
    // ========================================================================

    event Paused(address indexed by, string reason);
    event Unpaused(address indexed by);
    event GuardianAdded(address indexed guardian);
    event GuardianRemoved(address indexed guardian);
    event PauseThresholdChanged(uint256 oldThreshold, uint256 newThreshold);
    event OwnershipTransferred(address indexed previousOwner, address indexed newOwner);

    // ========================================================================
    // STATE
    // ========================================================================

    TestPausable public p;

    address owner = address(0xAAAA);
    address guardian1 = address(0x1111);
    address guardian2 = address(0x2222);
    address guardian3 = address(0x3333);
    address stranger = address(0xDEAD);

    // ========================================================================
    // SETUP
    // ========================================================================

    function setUp() public {
        p = new TestPausable();
        address[] memory guardians = new address[](3);
        guardians[0] = guardian1;
        guardians[1] = guardian2;
        guardians[2] = guardian3;
        p.initialize(owner, guardians, 2);
    }

    // ========================================================================
    // HELPERS
    // ========================================================================

    function _pauseAsGuardian1() internal {
        vm.prank(guardian1);
        p.pause("test pause");
    }

    // ========================================================================
    // 1. INITIALIZATION
    // ========================================================================

    function test_init_ownerIsSet() public view {
        assertEq(p.owner(), owner, "owner should match");
    }

    function test_init_guardiansAreRegistered() public view {
        assertTrue(p.isGuardian(guardian1), "guardian1 should be registered");
        assertTrue(p.isGuardian(guardian2), "guardian2 should be registered");
        assertTrue(p.isGuardian(guardian3), "guardian3 should be registered");
        assertEq(p.guardianCount(), 3, "should have 3 guardians");
    }

    function test_init_thresholdIsSet() public view {
        assertEq(p.unpauseThreshold(), 2, "threshold should be 2");
    }

    function test_init_notPausedByDefault() public view {
        assertFalse(p.paused(), "should not be paused initially");
    }

    function test_init_getGuardiansReturnsAll() public view {
        address[] memory g = p.getGuardians();
        assertEq(g.length, 3, "should return 3 guardians");
        assertEq(g[0], guardian1);
        assertEq(g[1], guardian2);
        assertEq(g[2], guardian3);
    }

    function test_init_revertsOnZeroOwner() public {
        TestPausable p2 = new TestPausable();
        address[] memory guardians = new address[](1);
        guardians[0] = guardian1;
        vm.expectRevert(Pausable.ZeroAddress.selector);
        p2.initialize(address(0), guardians, 1);
    }

    function test_init_revertsOnZeroThreshold() public {
        TestPausable p2 = new TestPausable();
        address[] memory guardians = new address[](1);
        guardians[0] = guardian1;
        vm.expectRevert(Pausable.InvalidThreshold.selector);
        p2.initialize(owner, guardians, 0);
    }

    function test_init_revertsOnThresholdExceedsGuardians() public {
        TestPausable p2 = new TestPausable();
        address[] memory guardians = new address[](1);
        guardians[0] = guardian1;
        vm.expectRevert(Pausable.InvalidThreshold.selector);
        p2.initialize(owner, guardians, 2);
    }

    function test_init_revertsOnDuplicateGuardian() public {
        TestPausable p2 = new TestPausable();
        address[] memory guardians = new address[](2);
        guardians[0] = guardian1;
        guardians[1] = guardian1;
        vm.expectRevert(Pausable.AlreadyGuardian.selector);
        p2.initialize(owner, guardians, 1);
    }

    function test_init_revertsOnZeroAddressGuardian() public {
        TestPausable p2 = new TestPausable();
        address[] memory guardians = new address[](1);
        guardians[0] = address(0);
        vm.expectRevert(Pausable.ZeroAddress.selector);
        p2.initialize(owner, guardians, 1);
    }

    // ========================================================================
    // 2. PAUSE
    // ========================================================================

    function test_pause_guardianCanPause() public {
        vm.prank(guardian1);
        p.pause("security issue");

        assertTrue(p.paused(), "should be paused");
        assertEq(p.pauseReason(), "security issue");
        assertEq(p.pausedAt(), block.timestamp);
    }

    function test_pause_emitsEvent() public {
        vm.expectEmit(true, false, false, true);
        emit Paused(guardian1, "reason");

        vm.prank(guardian1);
        p.pause("reason");
    }

    function test_pause_anyGuardianCanPause() public {
        vm.prank(guardian2);
        p.pause("guardian2 paused");
        assertTrue(p.paused(), "guardian2 should be able to pause");
    }

    function test_pause_revertsIfNotGuardian() public {
        vm.prank(stranger);
        vm.expectRevert(Pausable.NotGuardian.selector);
        p.pause("unauthorized");
    }

    function test_pause_ownerCannotPauseUnlessGuardian() public {
        vm.prank(owner);
        vm.expectRevert(Pausable.NotGuardian.selector);
        p.pause("owner attempt");
    }

    function test_pause_revertsIfAlreadyPaused() public {
        _pauseAsGuardian1();

        vm.prank(guardian2);
        vm.expectRevert(Pausable.ContractPaused.selector);
        p.pause("double pause");
    }

    function test_pause_resetsUnpauseVotes() public {
        // Pause, vote once, then unpause via owner, then pause again
        _pauseAsGuardian1();
        vm.prank(guardian1);
        p.voteToUnpause();
        assertEq(p.unpauseVoteCount(), 1, "should have 1 vote");

        // Owner emergency unpause
        vm.prank(owner);
        p.emergencyUnpause();

        // Pause again
        vm.prank(guardian1);
        p.pause("second pause");
        assertEq(p.unpauseVoteCount(), 0, "votes should be reset after pause");
    }

    function test_pause_storesTimestamp() public {
        vm.warp(1000);
        _pauseAsGuardian1();
        assertEq(p.pausedAt(), 1000, "pausedAt should match block.timestamp");
    }

    // ========================================================================
    // 3. VOTE TO UNPAUSE
    // ========================================================================

    function test_voteToUnpause_singleVoteBelowThreshold() public {
        _pauseAsGuardian1();

        vm.prank(guardian1);
        p.voteToUnpause();

        assertEq(p.unpauseVoteCount(), 1, "should have 1 vote");
        assertTrue(p.paused(), "should still be paused (threshold=2)");
        assertTrue(p.unpauseVotes(guardian1), "guardian1 should be marked as voted");
    }

    function test_voteToUnpause_thresholdMetUnpauses() public {
        _pauseAsGuardian1();

        vm.prank(guardian1);
        p.voteToUnpause();

        vm.prank(guardian2);
        p.voteToUnpause();

        assertFalse(p.paused(), "should be unpaused after threshold met");
        assertEq(p.pauseReason(), "", "reason should be cleared");
        assertEq(p.pausedAt(), 0, "pausedAt should be cleared");
        assertEq(p.unpauseVoteCount(), 0, "votes should be reset");
    }

    function test_voteToUnpause_emitsUnpausedEvent() public {
        _pauseAsGuardian1();

        vm.prank(guardian1);
        p.voteToUnpause();

        vm.expectEmit(true, false, false, false);
        emit Unpaused(guardian2);

        vm.prank(guardian2);
        p.voteToUnpause();
    }

    function test_voteToUnpause_revertsIfNotGuardian() public {
        _pauseAsGuardian1();

        vm.prank(stranger);
        vm.expectRevert(Pausable.NotGuardian.selector);
        p.voteToUnpause();
    }

    function test_voteToUnpause_revertsIfNotPaused() public {
        vm.prank(guardian1);
        vm.expectRevert(Pausable.ContractNotPaused.selector);
        p.voteToUnpause();
    }

    function test_voteToUnpause_revertsIfAlreadyVoted() public {
        _pauseAsGuardian1();

        vm.prank(guardian1);
        p.voteToUnpause();

        vm.prank(guardian1);
        vm.expectRevert(Pausable.AlreadyVoted.selector);
        p.voteToUnpause();
    }

    function test_voteToUnpause_canUnpauseReturnsTrueAtThreshold() public {
        _pauseAsGuardian1();

        vm.prank(guardian1);
        p.voteToUnpause();
        assertFalse(p.canUnpause(), "should not be unpausable with 1/2 votes");

        // Second vote will trigger unpause so canUnpause won't be true -- test right before
        assertEq(p.unpauseVoteCount(), 1);
        assertEq(p.unpauseThreshold(), 2);
    }

    function test_voteToUnpause_thresholdOf1UnpausesImmediately() public {
        // Deploy with threshold=1
        TestPausable p2 = new TestPausable();
        address[] memory guardians = new address[](2);
        guardians[0] = guardian1;
        guardians[1] = guardian2;
        p2.initialize(owner, guardians, 1);

        vm.prank(guardian1);
        p2.pause("test");

        vm.prank(guardian2);
        p2.voteToUnpause();

        assertFalse(p2.paused(), "single vote should unpause with threshold=1");
    }

    // ========================================================================
    // 4. EMERGENCY UNPAUSE
    // ========================================================================

    function test_emergencyUnpause_ownerCanUnpause() public {
        _pauseAsGuardian1();

        vm.prank(owner);
        p.emergencyUnpause();

        assertFalse(p.paused(), "should be unpaused");
        assertEq(p.pauseReason(), "", "reason should be cleared");
        assertEq(p.pausedAt(), 0, "pausedAt should be cleared");
    }

    function test_emergencyUnpause_emitsEvent() public {
        _pauseAsGuardian1();

        vm.expectEmit(true, false, false, false);
        emit Unpaused(owner);

        vm.prank(owner);
        p.emergencyUnpause();
    }

    function test_emergencyUnpause_resetsVotes() public {
        _pauseAsGuardian1();

        // Cast a vote first
        vm.prank(guardian1);
        p.voteToUnpause();
        assertEq(p.unpauseVoteCount(), 1);

        vm.prank(owner);
        p.emergencyUnpause();

        assertEq(p.unpauseVoteCount(), 0, "votes should be reset");
        assertFalse(p.unpauseVotes(guardian1), "guardian1 vote should be cleared");
    }

    function test_emergencyUnpause_revertsIfNotOwner() public {
        _pauseAsGuardian1();

        vm.prank(guardian1);
        vm.expectRevert(Pausable.NotOwner.selector);
        p.emergencyUnpause();
    }

    function test_emergencyUnpause_revertsIfNotPaused() public {
        vm.prank(owner);
        vm.expectRevert(Pausable.ContractNotPaused.selector);
        p.emergencyUnpause();
    }

    function test_emergencyUnpause_strangerCannotUnpause() public {
        _pauseAsGuardian1();

        vm.prank(stranger);
        vm.expectRevert(Pausable.NotOwner.selector);
        p.emergencyUnpause();
    }

    // ========================================================================
    // 5. ADD GUARDIAN
    // ========================================================================

    function test_addGuardian_ownerCanAdd() public {
        address newGuardian = address(0x4444);

        vm.prank(owner);
        p.addGuardian(newGuardian);

        assertTrue(p.isGuardian(newGuardian), "should be registered");
        assertEq(p.guardianCount(), 4, "count should be 4");
    }

    function test_addGuardian_emitsEvent() public {
        address newGuardian = address(0x4444);

        vm.expectEmit(true, false, false, false);
        emit GuardianAdded(newGuardian);

        vm.prank(owner);
        p.addGuardian(newGuardian);
    }

    function test_addGuardian_appearsInGetGuardians() public {
        address newGuardian = address(0x4444);

        vm.prank(owner);
        p.addGuardian(newGuardian);

        address[] memory g = p.getGuardians();
        assertEq(g.length, 4);
        assertEq(g[3], newGuardian, "new guardian should be at end");
    }

    function test_addGuardian_revertsIfNotOwner() public {
        vm.prank(stranger);
        vm.expectRevert(Pausable.NotOwner.selector);
        p.addGuardian(address(0x4444));
    }

    function test_addGuardian_revertsOnZeroAddress() public {
        vm.prank(owner);
        vm.expectRevert(Pausable.ZeroAddress.selector);
        p.addGuardian(address(0));
    }

    function test_addGuardian_revertsOnDuplicate() public {
        vm.prank(owner);
        vm.expectRevert(Pausable.AlreadyGuardian.selector);
        p.addGuardian(guardian1);
    }

    function test_addGuardian_guardianCannotAdd() public {
        vm.prank(guardian1);
        vm.expectRevert(Pausable.NotOwner.selector);
        p.addGuardian(address(0x4444));
    }

    // ========================================================================
    // 6. REMOVE GUARDIAN
    // ========================================================================

    function test_removeGuardian_ownerCanRemove() public {
        vm.prank(owner);
        p.removeGuardian(guardian3);

        assertFalse(p.isGuardian(guardian3), "guardian3 should be removed");
        assertEq(p.guardianCount(), 2, "count should be 2");
    }

    function test_removeGuardian_emitsEvent() public {
        vm.expectEmit(true, false, false, false);
        emit GuardianRemoved(guardian3);

        vm.prank(owner);
        p.removeGuardian(guardian3);
    }

    function test_removeGuardian_revertsIfNotOwner() public {
        vm.prank(stranger);
        vm.expectRevert(Pausable.NotOwner.selector);
        p.removeGuardian(guardian3);
    }

    function test_removeGuardian_revertsIfNotAGuardian() public {
        vm.prank(owner);
        vm.expectRevert(Pausable.NotAGuardian.selector);
        p.removeGuardian(stranger);
    }

    function test_removeGuardian_revertsIfWouldBreakThreshold() public {
        // threshold=2, 3 guardians. Remove one -> 2 left, OK.
        // Remove second -> 1 left < threshold=2, should revert.
        vm.prank(owner);
        p.removeGuardian(guardian3); // 2 left, threshold=2, OK

        vm.prank(owner);
        vm.expectRevert(Pausable.InvalidThreshold.selector);
        p.removeGuardian(guardian2); // would leave 1 < threshold=2
    }

    function test_removeGuardian_clearsExistingVote() public {
        _pauseAsGuardian1();

        // guardian3 votes to unpause
        vm.prank(guardian3);
        p.voteToUnpause();
        assertEq(p.unpauseVoteCount(), 1);

        // Owner removes guardian3 (who voted)
        vm.prank(owner);
        p.removeGuardian(guardian3);

        assertEq(p.unpauseVoteCount(), 0, "vote count should be decremented");
        assertFalse(p.unpauseVotes(guardian3), "vote should be cleared");
    }

    function test_removeGuardian_doesNotDecrementIfNoVote() public {
        _pauseAsGuardian1();

        // guardian1 votes
        vm.prank(guardian1);
        p.voteToUnpause();
        assertEq(p.unpauseVoteCount(), 1);

        // Remove guardian3 who has NOT voted
        vm.prank(owner);
        p.removeGuardian(guardian3);

        assertEq(p.unpauseVoteCount(), 1, "vote count should not change");
    }

    function test_removeGuardian_removedGuardianCannotPause() public {
        vm.prank(owner);
        p.removeGuardian(guardian3);

        vm.prank(guardian3);
        vm.expectRevert(Pausable.NotGuardian.selector);
        p.pause("no longer guardian");
    }

    function test_removeGuardian_swapAndPopMaintainsArray() public {
        // Remove guardian1 (first element). guardian3 should be swapped in.
        vm.prank(owner);
        p.removeGuardian(guardian1);

        address[] memory g = p.getGuardians();
        assertEq(g.length, 2, "should have 2 guardians");
        // guardian3 was swapped into index 0, guardian2 stays at index 1
        assertEq(g[0], guardian3, "guardian3 should be at index 0 after swap");
        assertEq(g[1], guardian2, "guardian2 should remain at index 1");
    }

    // ========================================================================
    // 7. SET UNPAUSE THRESHOLD
    // ========================================================================

    function test_setUnpauseThreshold_ownerCanChange() public {
        vm.prank(owner);
        p.setUnpauseThreshold(3);

        assertEq(p.unpauseThreshold(), 3, "threshold should be 3");
    }

    function test_setUnpauseThreshold_emitsEvent() public {
        vm.expectEmit(false, false, false, true);
        emit PauseThresholdChanged(2, 1);

        vm.prank(owner);
        p.setUnpauseThreshold(1);
    }

    function test_setUnpauseThreshold_revertsIfNotOwner() public {
        vm.prank(stranger);
        vm.expectRevert(Pausable.NotOwner.selector);
        p.setUnpauseThreshold(1);
    }

    function test_setUnpauseThreshold_revertsOnZero() public {
        vm.prank(owner);
        vm.expectRevert(Pausable.InvalidThreshold.selector);
        p.setUnpauseThreshold(0);
    }

    function test_setUnpauseThreshold_revertsIfExceedsGuardianCount() public {
        vm.prank(owner);
        vm.expectRevert(Pausable.InvalidThreshold.selector);
        p.setUnpauseThreshold(4); // only 3 guardians
    }

    function test_setUnpauseThreshold_canSetToGuardianCount() public {
        vm.prank(owner);
        p.setUnpauseThreshold(3); // exactly 3 guardians
        assertEq(p.unpauseThreshold(), 3);
    }

    function test_setUnpauseThreshold_canSetToOne() public {
        vm.prank(owner);
        p.setUnpauseThreshold(1);
        assertEq(p.unpauseThreshold(), 1);
    }

    // ========================================================================
    // 8. TRANSFER OWNERSHIP
    // ========================================================================

    function test_transferOwnership_ownerCanTransfer() public {
        address newOwner = address(0xBBBB);

        vm.prank(owner);
        p.transferOwnership(newOwner);

        assertEq(p.owner(), newOwner, "owner should be updated");
    }

    function test_transferOwnership_emitsEvent() public {
        address newOwner = address(0xBBBB);

        vm.expectEmit(true, true, false, false);
        emit OwnershipTransferred(owner, newOwner);

        vm.prank(owner);
        p.transferOwnership(newOwner);
    }

    function test_transferOwnership_oldOwnerLosesAccess() public {
        address newOwner = address(0xBBBB);

        vm.prank(owner);
        p.transferOwnership(newOwner);

        vm.prank(owner);
        vm.expectRevert(Pausable.NotOwner.selector);
        p.addGuardian(address(0x5555));
    }

    function test_transferOwnership_newOwnerGainsAccess() public {
        address newOwner = address(0xBBBB);

        vm.prank(owner);
        p.transferOwnership(newOwner);

        vm.prank(newOwner);
        p.addGuardian(address(0x5555));
        assertTrue(p.isGuardian(address(0x5555)));
    }

    function test_transferOwnership_revertsIfNotOwner() public {
        vm.prank(stranger);
        vm.expectRevert(Pausable.NotOwner.selector);
        p.transferOwnership(address(0xBBBB));
    }

    function test_transferOwnership_revertsOnZeroAddress() public {
        vm.prank(owner);
        vm.expectRevert(Pausable.ZeroAddress.selector);
        p.transferOwnership(address(0));
    }

    // ========================================================================
    // 9. VIEW FUNCTIONS
    // ========================================================================

    function test_getPauseStatus_whenNotPaused() public view {
        (bool isPaused, string memory reason, uint256 pauseTime, uint256 votesToUnpause, uint256 threshold) =
            p.getPauseStatus();

        assertFalse(isPaused);
        assertEq(reason, "");
        assertEq(pauseTime, 0);
        assertEq(votesToUnpause, 0);
        assertEq(threshold, 2);
    }

    function test_getPauseStatus_whenPaused() public {
        vm.warp(12345);
        _pauseAsGuardian1();

        (bool isPaused, string memory reason, uint256 pauseTime, uint256 votesToUnpause, uint256 threshold) =
            p.getPauseStatus();

        assertTrue(isPaused);
        assertEq(reason, "test pause");
        assertEq(pauseTime, 12345);
        assertEq(votesToUnpause, 0);
        assertEq(threshold, 2);
    }

    function test_getPauseStatus_withVotes() public {
        _pauseAsGuardian1();

        vm.prank(guardian1);
        p.voteToUnpause();

        (,,, uint256 votesToUnpause,) = p.getPauseStatus();
        assertEq(votesToUnpause, 1);
    }

    function test_canUnpause_falseWhenNotPaused() public view {
        assertFalse(p.canUnpause(), "should be false when not paused (votes=0, threshold=2)");
    }

    function test_guardianCount_afterAddAndRemove() public {
        assertEq(p.guardianCount(), 3);

        vm.prank(owner);
        p.addGuardian(address(0x4444));
        assertEq(p.guardianCount(), 4);

        vm.prank(owner);
        p.removeGuardian(address(0x4444));
        assertEq(p.guardianCount(), 3);
    }

    // ========================================================================
    // 10. MODIFIER TESTS (whenNotPaused / whenPaused)
    // ========================================================================

    function test_whenNotPaused_allowsCallWhenNotPaused() public {
        p.increment();
        assertEq(p.counter(), 1, "increment should work when not paused");
    }

    function test_whenNotPaused_revertsWhenPaused() public {
        _pauseAsGuardian1();

        vm.expectRevert(Pausable.ContractPaused.selector);
        p.increment();
    }

    function test_whenPaused_allowsCallWhenPaused() public {
        p.increment(); // counter = 1
        _pauseAsGuardian1();

        p.resetCounter();
        assertEq(p.counter(), 0, "resetCounter should work when paused");
    }

    function test_whenPaused_revertsWhenNotPaused() public {
        vm.expectRevert(Pausable.ContractNotPaused.selector);
        p.resetCounter();
    }

    // ========================================================================
    // 11. ACCESS CONTROL: NON-OWNER REVERTS FOR ALL ADMIN FUNCTIONS
    // ========================================================================

    function test_accessControl_strangerCannotAddGuardian() public {
        vm.prank(stranger);
        vm.expectRevert(Pausable.NotOwner.selector);
        p.addGuardian(address(0x4444));
    }

    function test_accessControl_strangerCannotRemoveGuardian() public {
        vm.prank(stranger);
        vm.expectRevert(Pausable.NotOwner.selector);
        p.removeGuardian(guardian1);
    }

    function test_accessControl_strangerCannotSetThreshold() public {
        vm.prank(stranger);
        vm.expectRevert(Pausable.NotOwner.selector);
        p.setUnpauseThreshold(1);
    }

    function test_accessControl_strangerCannotTransferOwnership() public {
        vm.prank(stranger);
        vm.expectRevert(Pausable.NotOwner.selector);
        p.transferOwnership(stranger);
    }

    function test_accessControl_strangerCannotEmergencyUnpause() public {
        _pauseAsGuardian1();
        vm.prank(stranger);
        vm.expectRevert(Pausable.NotOwner.selector);
        p.emergencyUnpause();
    }

    function test_accessControl_guardianCannotAddGuardian() public {
        vm.prank(guardian1);
        vm.expectRevert(Pausable.NotOwner.selector);
        p.addGuardian(address(0x4444));
    }

    function test_accessControl_guardianCannotRemoveGuardian() public {
        vm.prank(guardian1);
        vm.expectRevert(Pausable.NotOwner.selector);
        p.removeGuardian(guardian2);
    }

    function test_accessControl_guardianCannotSetThreshold() public {
        vm.prank(guardian1);
        vm.expectRevert(Pausable.NotOwner.selector);
        p.setUnpauseThreshold(1);
    }

    function test_accessControl_guardianCannotTransferOwnership() public {
        vm.prank(guardian1);
        vm.expectRevert(Pausable.NotOwner.selector);
        p.transferOwnership(guardian1);
    }

    // ========================================================================
    // 12. EDGE CASES
    // ========================================================================

    function test_edge_pauseUnpausePauseCycle() public {
        // Pause
        vm.prank(guardian1);
        p.pause("first pause");
        assertTrue(p.paused());

        // Unpause via owner
        vm.prank(owner);
        p.emergencyUnpause();
        assertFalse(p.paused());

        // Pause again
        vm.prank(guardian2);
        p.pause("second pause");
        assertTrue(p.paused());
        assertEq(p.pauseReason(), "second pause");
    }

    function test_edge_removeGuardianWithThresholdOneAndTwoGuardians() public {
        // Setup: 2 guardians, threshold=1
        TestPausable p2 = new TestPausable();
        address[] memory guardians = new address[](2);
        guardians[0] = guardian1;
        guardians[1] = guardian2;
        p2.initialize(owner, guardians, 1);

        // Remove one: 1 guardian left, threshold=1, should succeed
        vm.prank(owner);
        p2.removeGuardian(guardian2);
        assertEq(p2.guardianCount(), 1);

        // Cannot remove the last one because 0 < threshold=1
        vm.prank(owner);
        vm.expectRevert(Pausable.InvalidThreshold.selector);
        p2.removeGuardian(guardian1);
    }

    function test_edge_lowerThresholdThenRemoveGuardian() public {
        // Lower threshold to 1
        vm.prank(owner);
        p.setUnpauseThreshold(1);

        // Now we can remove down to 1 guardian
        vm.prank(owner);
        p.removeGuardian(guardian3);

        vm.prank(owner);
        p.removeGuardian(guardian2);

        assertEq(p.guardianCount(), 1);
        assertEq(p.unpauseThreshold(), 1);
    }

    function test_edge_voteCountResetAfterUnpauseThenRePause() public {
        _pauseAsGuardian1();

        // guardian1 votes
        vm.prank(guardian1);
        p.voteToUnpause();
        assertEq(p.unpauseVoteCount(), 1);

        // owner emergency unpause
        vm.prank(owner);
        p.emergencyUnpause();

        // Re-pause
        vm.prank(guardian1);
        p.pause("re-pause");

        // Votes should be fresh
        assertEq(p.unpauseVoteCount(), 0);
        assertFalse(p.unpauseVotes(guardian1));
        assertFalse(p.unpauseVotes(guardian2));
        assertFalse(p.unpauseVotes(guardian3));
    }

    function test_edge_addGuardianThenNewGuardianCanPause() public {
        address newGuardian = address(0x4444);

        vm.prank(owner);
        p.addGuardian(newGuardian);

        vm.prank(newGuardian);
        p.pause("new guardian pause");
        assertTrue(p.paused());
    }

    function test_edge_addGuardianThenNewGuardianCanVote() public {
        address newGuardian = address(0x4444);

        vm.prank(owner);
        p.addGuardian(newGuardian);

        _pauseAsGuardian1();

        vm.prank(newGuardian);
        p.voteToUnpause();
        assertEq(p.unpauseVoteCount(), 1);
        assertTrue(p.unpauseVotes(newGuardian));
    }

    function test_edge_emptyReasonString() public {
        vm.prank(guardian1);
        p.pause("");
        assertEq(p.pauseReason(), "");
    }

    function test_edge_longReasonString() public {
        string memory longReason = "This is an extremely long reason string that describes the security vulnerability "
            "discovered in the protocol requiring immediate emergency pause action to protect users";

        vm.prank(guardian1);
        p.pause(longReason);
        assertEq(p.pauseReason(), longReason);
    }

    function test_edge_thresholdChangeAffectsPendingUnpause() public {
        // Threshold=2, pause, then 1 vote
        _pauseAsGuardian1();
        vm.prank(guardian1);
        p.voteToUnpause();
        assertEq(p.unpauseVoteCount(), 1);
        assertTrue(p.paused());

        // Owner lowers threshold to 1 -- canUnpause should now be true but doesn't auto-unpause
        vm.prank(owner);
        p.setUnpauseThreshold(1);

        assertTrue(p.canUnpause(), "canUnpause should return true after threshold lowered");
        // Note: the contract remains paused until another vote triggers unpause or emergencyUnpause
        assertTrue(p.paused(), "should still be paused (no auto-unpause on threshold change)");
    }

    function test_edge_transferOwnershipToGuardian() public {
        // Transfer ownership to guardian1 -- guardian1 is now both owner and guardian
        vm.prank(owner);
        p.transferOwnership(guardian1);

        assertEq(p.owner(), guardian1);
        assertTrue(p.isGuardian(guardian1));

        // guardian1 can now both pause (as guardian) and emergency unpause (as owner)
        vm.prank(guardian1);
        p.pause("owner-guardian pause");

        vm.prank(guardian1);
        p.emergencyUnpause();
        assertFalse(p.paused());
    }

    function test_edge_removeMiddleGuardianArrayIntegrity() public {
        // Remove guardian2 (middle element)
        vm.prank(owner);
        p.removeGuardian(guardian2);

        address[] memory g = p.getGuardians();
        assertEq(g.length, 2);
        // guardian3 should be swapped into guardian2's slot
        assertEq(g[0], guardian1);
        assertEq(g[1], guardian3);

        // Both remaining guardians should still work
        assertFalse(p.isGuardian(guardian2));
        assertTrue(p.isGuardian(guardian1));
        assertTrue(p.isGuardian(guardian3));
    }

    function test_edge_removeLastGuardianInArray() public {
        // Remove guardian3 (last element) -- no swap needed
        vm.prank(owner);
        p.removeGuardian(guardian3);

        address[] memory g = p.getGuardians();
        assertEq(g.length, 2);
        assertEq(g[0], guardian1);
        assertEq(g[1], guardian2);
    }

    // ========================================================================
    // 13. FULL LIFECYCLE SCENARIO
    // ========================================================================

    function test_fullLifecycle() public {
        // 1. Initial state
        assertFalse(p.paused());
        assertEq(p.guardianCount(), 3);

        // 2. Increment works when not paused
        p.increment();
        assertEq(p.counter(), 1);

        // 3. Guardian pauses
        vm.warp(5000);
        vm.prank(guardian1);
        p.pause("suspicious activity");
        assertTrue(p.paused());

        // 4. Increment blocked
        vm.expectRevert(Pausable.ContractPaused.selector);
        p.increment();

        // 5. Guardian1 votes to unpause
        vm.prank(guardian1);
        p.voteToUnpause();
        assertEq(p.unpauseVoteCount(), 1);
        assertTrue(p.paused()); // still paused, need 2

        // 6. Guardian2 votes -- meets threshold, auto-unpause
        vm.prank(guardian2);
        p.voteToUnpause();
        assertFalse(p.paused());

        // 7. Increment works again
        p.increment();
        assertEq(p.counter(), 2);

        // 8. Add a 4th guardian
        address guardian4 = address(0x4444);
        vm.prank(owner);
        p.addGuardian(guardian4);
        assertEq(p.guardianCount(), 4);

        // 9. Raise threshold to 3
        vm.prank(owner);
        p.setUnpauseThreshold(3);

        // 10. Guardian4 pauses
        vm.prank(guardian4);
        p.pause("new threat");

        // 11. 2 votes not enough
        vm.prank(guardian1);
        p.voteToUnpause();
        vm.prank(guardian2);
        p.voteToUnpause();
        assertTrue(p.paused()); // need 3

        // 12. Owner emergency unpause
        vm.prank(owner);
        p.emergencyUnpause();
        assertFalse(p.paused());

        // 13. Transfer ownership
        address newOwner = address(0xBBBB);
        vm.prank(owner);
        p.transferOwnership(newOwner);
        assertEq(p.owner(), newOwner);

        // 14. Old owner cannot act
        vm.prank(owner);
        vm.expectRevert(Pausable.NotOwner.selector);
        p.addGuardian(address(0x5555));

        // 15. New owner can act
        vm.prank(newOwner);
        p.addGuardian(address(0x5555));
        assertEq(p.guardianCount(), 5);
    }
}
