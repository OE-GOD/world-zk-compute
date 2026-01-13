// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/ProgramRegistry.sol";

contract ProgramRegistryTest is Test {
    ProgramRegistry public registry;

    address public owner = address(1);
    address public other = address(2);

    bytes32 public imageId = bytes32(uint256(1));
    string public name = "Test Program";
    string public programUrl = "https://example.com/test.elf";

    function setUp() public {
        registry = new ProgramRegistry();
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

    // ========================================================================
    // DEACTIVATION TESTS
    // ========================================================================

    function testDeactivateProgram() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        assertTrue(registry.isProgramActive(imageId));

        vm.prank(owner);
        registry.deactivateProgram(imageId);

        assertFalse(registry.isProgramActive(imageId));
    }

    function testReactivateProgram() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.prank(owner);
        registry.deactivateProgram(imageId);

        assertFalse(registry.isProgramActive(imageId));

        vm.prank(owner);
        registry.reactivateProgram(imageId);

        assertTrue(registry.isProgramActive(imageId));
    }

    function testDeactivateProgramNotOwner() public {
        vm.prank(owner);
        registry.registerProgram(imageId, name, programUrl, bytes32(0));

        vm.prank(other);
        vm.expectRevert(ProgramRegistry.NotProgramOwner.selector);
        registry.deactivateProgram(imageId);
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

        bytes32[] memory programs = registry.getOwnerPrograms(owner);
        assertEq(programs.length, 2);
    }

    function testGetAllPrograms() public {
        vm.startPrank(owner);
        registry.registerProgram(bytes32(uint256(1)), "First", programUrl, bytes32(0));
        registry.registerProgram(bytes32(uint256(2)), "Second", programUrl, bytes32(0));
        registry.registerProgram(bytes32(uint256(3)), "Third", programUrl, bytes32(0));
        vm.stopPrank();

        bytes32[] memory programs = registry.getAllPrograms(0, 10);
        assertEq(programs.length, 3);

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
}
