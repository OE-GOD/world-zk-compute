// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/ProgramRegistry.sol";
import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";
import {Pausable} from "@openzeppelin/contracts/utils/Pausable.sol";

contract ProgramRegistryTest is Test {
    ProgramRegistry public registry;

    address public admin = address(0xAD);
    address public owner = address(1);
    address public other = address(2);

    bytes32 public imageId = bytes32(uint256(1));
    string public name = "Test Program";
    string public programUrl = "https://example.com/test.elf";

    function setUp() public {
        registry = new ProgramRegistry(admin);
    }

    // ========================================================================
    // REGISTRATION TESTS
    // ========================================================================

    function testRegisterProgram() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        ProgramRegistry.Program memory program = registry.getProgram(imageId);
        assertEq(program.imageId, imageId);
        assertEq(program.owner, owner);
        assertEq(program.name, name);
        assertEq(program.programUrl, programUrl);
        assertTrue(program.active);
        assertFalse(program.verified);
    }

    function testRegisterProgramInvalidImageId() public {
        vm.expectRevert(ProgramRegistry.InvalidImageId.selector);
        registry.registerProgram(bytes32(0), name, programUrl, bytes32(0));
    }

    function testRegisterProgramAlreadyRegistered() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.prank(other);
        vm.expectRevert(ProgramRegistry.ProgramAlreadyRegistered.selector);
        registry.registerProgram(imageId, "Another Program", programUrl, bytes32(0));
    }

    event ProgramRegistered(bytes32 indexed imageId, address indexed owner, string name, string programUrl);
    event ProgramDeactivated(bytes32 indexed imageId);
    event ProgramReactivated(bytes32 indexed imageId);
    event ProgramVerified(bytes32 indexed imageId);
    event ProgramUnverified(bytes32 indexed imageId);

    function testRegisterProgramEmitsEvent() public {
        vm.prank(owner);
        vm.expectEmit(true, true, false, true);
        emit ProgramRegistered(imageId, owner, name, programUrl);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));
    }

    function testRegisterProgramWithVerifier() public {
        address verifier = address(0x99);
        vm.prank(owner);
        registry.registerProgramWithVerifier(imageId, name, programUrl, bytes32(0), verifier, "remainder");

        ProgramRegistry.Program memory program = registry.getProgram(imageId);
        assertEq(program.verifierContract, verifier);
        assertEq(program.proofSystem, "remainder");
        assertFalse(program.verified);
    }

    // ========================================================================
    // UPDATE TESTS
    // ========================================================================

    function testUpdateProgramUrl() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        string memory newUrl = "https://newurl.com/test.elf";

        vm.prank(owner);
        registry.updateProgramUrl(imageId, newUrl);

        ProgramRegistry.Program memory program = registry.getProgram(imageId);
        assertEq(program.programUrl, newUrl);
    }

    function testUpdateProgramUrlNotOwner() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.prank(other);
        vm.expectRevert(ProgramRegistry.NotProgramOwner.selector);
        registry.updateProgramUrl(imageId, "https://malicious.com");
    }

    function testUpdateProgramUrlNotFound() public {
        vm.prank(owner);
        vm.expectRevert(ProgramRegistry.ProgramNotFound.selector);
        registry.updateProgramUrl(imageId, "https://example.com");
    }

    // ========================================================================
    // DEACTIVATION TESTS (now admin-only)
    // ========================================================================

    function testDeactivateProgram() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        assertTrue(registry.isProgramActive(imageId));

        vm.prank(admin);
        registry.deactivateProgram(imageId);

        assertFalse(registry.isProgramActive(imageId));
    }

    function testReactivateProgram() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.prank(admin);
        registry.deactivateProgram(imageId);

        assertFalse(registry.isProgramActive(imageId));

        vm.prank(admin);
        registry.reactivateProgram(imageId);

        assertTrue(registry.isProgramActive(imageId));
    }

    function testDeactivateProgramNotAdmin() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.prank(other);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, other));
        registry.deactivateProgram(imageId);
    }

    function testDeactivateProgramNotFound() public {
        vm.prank(admin);
        vm.expectRevert(ProgramRegistry.ProgramNotFound.selector);
        registry.deactivateProgram(imageId);
    }

    function testDeactivateAlreadyInactive() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.startPrank(admin);
        registry.deactivateProgram(imageId);

        vm.expectRevert(ProgramRegistry.ProgramAlreadyInactive.selector);
        registry.deactivateProgram(imageId);
        vm.stopPrank();
    }

    function testReactivateAlreadyActive() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.prank(admin);
        vm.expectRevert(ProgramRegistry.ProgramAlreadyActive.selector);
        registry.reactivateProgram(imageId);
    }

    function testReactivateProgramNotAdmin() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.prank(admin);
        registry.deactivateProgram(imageId);

        vm.prank(other);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, other));
        registry.reactivateProgram(imageId);
    }

    function testDeactivateEmitsEvent() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.prank(admin);
        vm.expectEmit(true, false, false, false);
        emit ProgramDeactivated(imageId);
        registry.deactivateProgram(imageId);
    }

    function testReactivateEmitsEvent() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.prank(admin);
        registry.deactivateProgram(imageId);

        vm.prank(admin);
        vm.expectEmit(true, false, false, false);
        emit ProgramReactivated(imageId);
        registry.reactivateProgram(imageId);
    }

    // ========================================================================
    // VERIFICATION TESTS
    // ========================================================================

    function testVerifyProgram() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        assertFalse(registry.isProgramVerified(imageId));

        vm.prank(admin);
        registry.verifyProgram(imageId);

        assertTrue(registry.isProgramVerified(imageId));

        ProgramRegistry.Program memory program = registry.getProgram(imageId);
        assertTrue(program.verified);
    }

    function testVerifyProgramEmitsEvent() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.prank(admin);
        vm.expectEmit(true, false, false, false);
        emit ProgramVerified(imageId);
        registry.verifyProgram(imageId);
    }

    function testVerifyProgramNotAdmin() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.prank(other);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, other));
        registry.verifyProgram(imageId);
    }

    function testVerifyProgramNotFound() public {
        vm.prank(admin);
        vm.expectRevert(ProgramRegistry.ProgramNotFound.selector);
        registry.verifyProgram(imageId);
    }

    function testVerifyProgramAlreadyVerified() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.startPrank(admin);
        registry.verifyProgram(imageId);

        vm.expectRevert(ProgramRegistry.ProgramAlreadyVerified.selector);
        registry.verifyProgram(imageId);
        vm.stopPrank();
    }

    function testUnverifyProgram() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.startPrank(admin);
        registry.verifyProgram(imageId);
        assertTrue(registry.isProgramVerified(imageId));

        registry.unverifyProgram(imageId);
        assertFalse(registry.isProgramVerified(imageId));
        vm.stopPrank();
    }

    function testUnverifyProgramEmitsEvent() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.startPrank(admin);
        registry.verifyProgram(imageId);

        vm.expectEmit(true, false, false, false);
        emit ProgramUnverified(imageId);
        registry.unverifyProgram(imageId);
        vm.stopPrank();
    }

    function testUnverifyProgramNotAdmin() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.prank(admin);
        registry.verifyProgram(imageId);

        vm.prank(other);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, other));
        registry.unverifyProgram(imageId);
    }

    function testUnverifyProgramNotFound() public {
        vm.prank(admin);
        vm.expectRevert(ProgramRegistry.ProgramNotFound.selector);
        registry.unverifyProgram(imageId);
    }

    function testUnverifyProgramNotVerified() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.prank(admin);
        vm.expectRevert(ProgramRegistry.ProgramNotVerified.selector);
        registry.unverifyProgram(imageId);
    }

    function testVerifyAndUnverifyRoundtrip() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.startPrank(admin);

        // verify
        registry.verifyProgram(imageId);
        assertTrue(registry.isProgramVerified(imageId));

        // unverify
        registry.unverifyProgram(imageId);
        assertFalse(registry.isProgramVerified(imageId));

        // re-verify
        registry.verifyProgram(imageId);
        assertTrue(registry.isProgramVerified(imageId));

        vm.stopPrank();
    }

    // ========================================================================
    // PAUSE TESTS
    // ========================================================================

    function testPause() public {
        vm.prank(admin);
        registry.pause();

        assertTrue(registry.paused());
    }

    function testUnpause() public {
        vm.startPrank(admin);
        registry.pause();
        registry.unpause();
        vm.stopPrank();

        assertFalse(registry.paused());
    }

    function testPauseNotAdmin() public {
        vm.prank(other);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, other));
        registry.pause();
    }

    function testUnpauseNotAdmin() public {
        vm.prank(admin);
        registry.pause();

        vm.prank(other);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, other));
        registry.unpause();
    }

    function testRegisterProgramWhenPaused() public {
        vm.prank(admin);
        registry.pause();

        vm.prank(owner);
        vm.expectRevert(Pausable.EnforcedPause.selector);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));
    }

    function testRegisterProgramWithVerifierWhenPaused() public {
        vm.prank(admin);
        registry.pause();

        vm.prank(owner);
        vm.expectRevert(Pausable.EnforcedPause.selector);
        registry.registerProgramWithVerifier(imageId, name, programUrl, bytes32(0), address(0x99), "remainder");
    }

    function testViewFunctionsWorkWhenPaused() public {
        // Register before pausing
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.prank(admin);
        registry.pause();

        // All view functions should still work
        assertTrue(registry.isProgramActive(imageId));
        assertFalse(registry.isProgramVerified(imageId));

        ProgramRegistry.Program memory program = registry.getProgram(imageId);
        assertEq(program.imageId, imageId);

        assertEq(registry.getProgramCount(), 1);

        bytes32[] memory ownerProgs = registry.getOwnerPrograms(owner);
        assertEq(ownerProgs.length, 1);

        bytes32[] memory allProgs = registry.getAllPrograms(0, 10);
        assertEq(allProgs.length, 1);
    }

    function testAdminFunctionsWorkWhenPaused() public {
        // Register before pausing
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.startPrank(admin);
        registry.pause();

        // Admin functions should still work even when paused
        registry.verifyProgram(imageId);
        assertTrue(registry.isProgramVerified(imageId));

        registry.deactivateProgram(imageId);
        assertFalse(registry.isProgramActive(imageId));

        vm.stopPrank();
    }

    function testRegisterAfterUnpause() public {
        vm.prank(admin);
        registry.pause();

        vm.prank(admin);
        registry.unpause();

        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        ProgramRegistry.Program memory program = registry.getProgram(imageId);
        assertEq(program.imageId, imageId);
    }

    // ========================================================================
    // OWNERSHIP TRANSFER TESTS (Ownable2Step)
    // ========================================================================

    function testOwner() public view {
        assertEq(registry.owner(), admin);
    }

    function testTransferOwnership() public {
        address newAdmin = address(0xBB);

        vm.prank(admin);
        registry.transferOwnership(newAdmin);

        // Pending, not yet accepted
        assertEq(registry.owner(), admin);
        assertEq(registry.pendingOwner(), newAdmin);
    }

    function testAcceptOwnership() public {
        address newAdmin = address(0xBB);

        vm.prank(admin);
        registry.transferOwnership(newAdmin);

        vm.prank(newAdmin);
        registry.acceptOwnership();

        assertEq(registry.owner(), newAdmin);
        assertEq(registry.pendingOwner(), address(0));
    }

    function testAcceptOwnershipWrongCaller() public {
        address newAdmin = address(0xBB);

        vm.prank(admin);
        registry.transferOwnership(newAdmin);

        vm.prank(other);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, other));
        registry.acceptOwnership();
    }

    function testTransferOwnershipNotAdmin() public {
        vm.prank(other);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, other));
        registry.transferOwnership(address(0xCC));
    }

    function testNewOwnerCanVerify() public {
        address newAdmin = address(0xBB);

        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        // Transfer ownership
        vm.prank(admin);
        registry.transferOwnership(newAdmin);

        vm.prank(newAdmin);
        registry.acceptOwnership();

        // New owner can verify
        vm.prank(newAdmin);
        registry.verifyProgram(imageId);
        assertTrue(registry.isProgramVerified(imageId));

        // Old admin cannot
        vm.prank(admin);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, admin));
        registry.unverifyProgram(imageId);
    }

    // ========================================================================
    // VIEW FUNCTION TESTS
    // ========================================================================

    function testGetProgramCount() public {
        assertEq(registry.getProgramCount(), 0);

        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        assertEq(registry.getProgramCount(), 1);

        vm.prank(owner);
        registry.registerProgram(bytes32(uint256(2)), "Second", programUrl, bytes32(0));

        assertEq(registry.getProgramCount(), 2);
    }

    function testGetOwnerPrograms() public {
        vm.startPrank(owner);
        registry.registerProgram(bytes32(uint256(1)), "First", programUrl, bytes32(0));
        registry.registerProgram(bytes32(uint256(2)), "Second", programUrl, bytes32(0));
        vm.stopPrank();

        bytes32[] memory progs = registry.getOwnerPrograms(owner);
        assertEq(progs.length, 2);
    }

    function testGetAllPrograms() public {
        vm.startPrank(owner);
        registry.registerProgram(bytes32(uint256(1)), "First", programUrl, bytes32(0));
        registry.registerProgram(bytes32(uint256(2)), "Second", programUrl, bytes32(0));
        registry.registerProgram(bytes32(uint256(3)), "Third", programUrl, bytes32(0));
        vm.stopPrank();

        bytes32[] memory progs = registry.getAllPrograms(0, 10);
        assertEq(progs.length, 3);

        // Test pagination
        bytes32[] memory page1 = registry.getAllPrograms(0, 2);
        assertEq(page1.length, 2);

        bytes32[] memory page2 = registry.getAllPrograms(2, 2);
        assertEq(page2.length, 1);
    }

    function testGetProgramNotFound() public {
        vm.expectRevert(ProgramRegistry.ProgramNotFound.selector);
        registry.getProgram(imageId);
    }

    function testIsProgramActiveUnregistered() public view {
        assertFalse(registry.isProgramActive(bytes32(uint256(999))));
    }

    function testIsProgramVerifiedUnregistered() public view {
        assertFalse(registry.isProgramVerified(bytes32(uint256(999))));
    }

    // ========================================================================
    // UPDATE VERIFIER TESTS
    // ========================================================================

    event VerifierUpdated(bytes32 indexed programId, address oldVerifier, address newVerifier);

    function testUpdateVerifier() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        // Deploy a dummy contract to use as verifier
        DummyVerifier dummyVerifier = new DummyVerifier();
        address newVerifier = address(dummyVerifier);

        vm.prank(owner);
        registry.updateVerifier(imageId, newVerifier);

        ProgramRegistry.Program memory program = registry.getProgram(imageId);
        assertEq(program.verifierContract, newVerifier);
    }

    function testUpdateVerifierEmitsEvent() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        DummyVerifier dummyVerifier = new DummyVerifier();
        address newVerifier = address(dummyVerifier);

        vm.prank(owner);
        vm.expectEmit(true, false, false, true);
        emit VerifierUpdated(imageId, address(0), newVerifier);
        registry.updateVerifier(imageId, newVerifier);
    }

    function testUpdateVerifierNotOwner() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        DummyVerifier dummyVerifier = new DummyVerifier();

        vm.prank(other);
        vm.expectRevert(ProgramRegistry.NotProgramOwner.selector);
        registry.updateVerifier(imageId, address(dummyVerifier));
    }

    function testUpdateVerifierNotFound() public {
        DummyVerifier dummyVerifier = new DummyVerifier();

        vm.prank(owner);
        vm.expectRevert(ProgramRegistry.ProgramNotFound.selector);
        registry.updateVerifier(imageId, address(dummyVerifier));
    }

    function testUpdateVerifierRejectsZeroAddress() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.prank(owner);
        vm.expectRevert(ProgramRegistry.InvalidVerifierContract.selector);
        registry.updateVerifier(imageId, address(0));
    }

    function testUpdateVerifierRejectsEOA() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        // address(0x42) is an EOA -- has no code
        vm.prank(owner);
        vm.expectRevert(ProgramRegistry.InvalidVerifierContract.selector);
        registry.updateVerifier(imageId, address(0x42));
    }

    function testUpdateVerifierAcceptsDeployedContract() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        // Deploy a real contract and verify it is accepted
        DummyVerifier dummyVerifier = new DummyVerifier();
        assertTrue(address(dummyVerifier).code.length > 0, "DummyVerifier should have code");

        vm.prank(owner);
        registry.updateVerifier(imageId, address(dummyVerifier));

        ProgramRegistry.Program memory program = registry.getProgram(imageId);
        assertEq(program.verifierContract, address(dummyVerifier));
    }

    // ========================================================================
    // COMBINED SCENARIO TESTS
    // ========================================================================

    function testVerifyThenDeactivate() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.startPrank(admin);
        registry.verifyProgram(imageId);
        registry.deactivateProgram(imageId);
        vm.stopPrank();

        ProgramRegistry.Program memory program = registry.getProgram(imageId);
        assertTrue(program.verified);
        assertFalse(program.active);
    }

    function testDeactivateThenVerify() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.startPrank(admin);
        registry.deactivateProgram(imageId);
        registry.verifyProgram(imageId);
        vm.stopPrank();

        ProgramRegistry.Program memory program = registry.getProgram(imageId);
        assertTrue(program.verified);
        assertFalse(program.active);
    }

    function testUpdateUrlBlockedWhenPaused() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.prank(admin);
        registry.pause();

        // updateProgramUrl is gated by whenNotPaused
        vm.expectRevert(abi.encodeWithSelector(Pausable.EnforcedPause.selector));
        vm.prank(owner);
        registry.updateProgramUrl(imageId, "https://newurl.com");
    }

    // ========================================================================
    // STRING LENGTH BOUNDS (T418)
    // ========================================================================

    function testRegisterNameAtBoundary() public {
        // 64 bytes exactly — should succeed
        string memory longName = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"; // 64 A's
        assertEq(bytes(longName).length, 64);
        vm.prank(owner);
        registry.registerProgram(imageId, longName, programUrl, bytes32(0));
        ProgramRegistry.Program memory p = registry.getProgram(imageId);
        assertEq(p.name, longName);
    }

    function testRegisterNameTooLong() public {
        // 65 bytes — should revert
        string memory tooLong = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"; // 65 A's
        assertEq(bytes(tooLong).length, 65);
        vm.prank(owner);
        vm.expectRevert(abi.encodeWithSelector(ProgramRegistry.StringTooLong.selector, "name", 64));
        registry.registerProgram(imageId, tooLong, programUrl, bytes32(0));
    }

    function testRegisterUrlTooLong() public {
        // Build a 513-byte URL
        bytes memory longBytes = new bytes(513);
        for (uint256 i = 0; i < 513; i++) {
            longBytes[i] = "A";
        }
        string memory tooLong = string(longBytes);
        vm.prank(owner);
        vm.expectRevert(abi.encodeWithSelector(ProgramRegistry.StringTooLong.selector, "url", 512));
        registry.registerProgram(imageId, name, tooLong, bytes32(0));
    }

    function testUpdateUrlTooLong() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        bytes memory longBytes = new bytes(513);
        for (uint256 i = 0; i < 513; i++) {
            longBytes[i] = "A";
        }
        string memory tooLong = string(longBytes);
        vm.prank(owner);
        vm.expectRevert(abi.encodeWithSelector(ProgramRegistry.StringTooLong.selector, "url", 512));
        registry.updateProgramUrl(imageId, tooLong);
    }

    function testUrlAtBoundary() public {
        bytes memory longBytes = new bytes(512);
        for (uint256 i = 0; i < 512; i++) {
            longBytes[i] = "A";
        }
        string memory maxUrl = string(longBytes);
        vm.prank(owner);
        registry.registerProgram(imageId, name, maxUrl, bytes32(0));
        ProgramRegistry.Program memory p = registry.getProgram(imageId);
        assertEq(bytes(p.programUrl).length, 512);
    }

    // ========================================================================
    // ADMIN FORCE UPDATE URL TESTS (T423)
    // ========================================================================

    event ProgramUrlForceUpdated(bytes32 indexed imageId, string newUrl, address admin);

    function testAdminForceUpdateUrl() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        string memory newUrl = "https://safe-mirror.com/test.elf";

        vm.prank(admin);
        registry.adminForceUpdateUrl(imageId, newUrl);

        ProgramRegistry.Program memory program = registry.getProgram(imageId);
        assertEq(program.programUrl, newUrl);
    }

    function testAdminForceUpdateUrlNotAdmin() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.prank(other);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, other));
        registry.adminForceUpdateUrl(imageId, "https://attacker.com");
    }

    function testAdminForceUpdateUrlEmitsEvent() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        string memory newUrl = "https://safe-mirror.com/test.elf";

        vm.prank(admin);
        vm.expectEmit(true, false, false, true);
        emit ProgramUrlForceUpdated(imageId, newUrl, admin);
        registry.adminForceUpdateUrl(imageId, newUrl);
    }

    function testAdminForceUpdateUrlNotFound() public {
        vm.prank(admin);
        vm.expectRevert(ProgramRegistry.ProgramNotFound.selector);
        registry.adminForceUpdateUrl(imageId, "https://example.com");
    }

    function testAdminForceUpdateUrlProgramOwnerCannotCall() public {
        // Even the program owner (who is not admin) cannot call adminForceUpdateUrl
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.prank(owner);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, owner));
        registry.adminForceUpdateUrl(imageId, "https://owner-attempt.com");
    }

    function testAdminForceUpdateUrlTooLong() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        bytes memory longBytes = new bytes(513);
        for (uint256 i = 0; i < 513; i++) {
            longBytes[i] = "A";
        }
        string memory tooLong = string(longBytes);
        vm.prank(admin);
        vm.expectRevert(abi.encodeWithSelector(ProgramRegistry.StringTooLong.selector, "url", 512));
        registry.adminForceUpdateUrl(imageId, tooLong);
    }

    function testAdminForceUpdateUrlAtBoundary() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        bytes memory longBytes = new bytes(512);
        for (uint256 i = 0; i < 512; i++) {
            longBytes[i] = "A";
        }
        string memory maxUrl = string(longBytes);
        vm.prank(admin);
        registry.adminForceUpdateUrl(imageId, maxUrl);
        ProgramRegistry.Program memory p = registry.getProgram(imageId);
        assertEq(bytes(p.programUrl).length, 512);
    }
}

/// @dev Minimal contract used as a valid verifier address in tests
contract DummyVerifier {
    function verify(bytes calldata, bytes32, bytes calldata) external pure {}

    function proofSystem() external pure returns (string memory) {
        return "dummy";
    }
}
