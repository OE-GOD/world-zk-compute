// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/remainder/RemainderVerifier.sol";
import "../src/remainder/GKRDAGVerifier.sol";
import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";

/// @notice Mock Stylus verifier that returns true
contract MockStylusVerifier {
    function verifyDagProof(bytes calldata, bytes calldata, bytes calldata, bytes calldata)
        external
        pure
        returns (bool)
    {
        return true;
    }
}

/// @notice Mock Stylus verifier that returns false
contract MockStylusVerifierFalse {
    function verifyDagProof(bytes calldata, bytes calldata, bytes calldata, bytes calldata)
        external
        pure
        returns (bool)
    {
        return false;
    }
}

/// @notice Mock Stylus verifier that reverts
contract MockStylusVerifierReverts {
    function verifyDagProof(bytes calldata, bytes calldata, bytes calldata, bytes calldata) external pure {
        revert("mock stylus revert");
    }
}

contract StylusAdapterTest is Test {
    RemainderVerifier verifier;
    MockStylusVerifier mockStylus;
    MockStylusVerifierFalse mockStylusFalse;
    MockStylusVerifierReverts mockStylusReverts;

    bytes32 circuitHash;

    function setUp() public {
        verifier = new RemainderVerifier(address(this));
        mockStylus = new MockStylusVerifier();
        mockStylusFalse = new MockStylusVerifierFalse();
        mockStylusReverts = new MockStylusVerifierReverts();

        // Register a minimal DAG circuit
        circuitHash = bytes32(uint256(0x1234));
        GKRDAGVerifier.DAGCircuitDescription memory desc;
        desc.numComputeLayers = 2;
        desc.numInputLayers = 1;
        desc.layerTypes = new uint8[](2);
        desc.layerTypes[0] = 0;
        desc.layerTypes[1] = 0;
        desc.numSumcheckRounds = new uint256[](2);
        desc.numSumcheckRounds[0] = 3;
        desc.numSumcheckRounds[1] = 3;
        desc.atomOffsets = new uint256[](3);
        desc.atomOffsets[0] = 0;
        desc.atomOffsets[1] = 1;
        desc.atomOffsets[2] = 2;
        desc.atomTargetLayers = new uint256[](2);
        desc.atomTargetLayers[0] = 1;
        desc.atomTargetLayers[1] = 2;
        desc.ptOffsets = new uint256[](3);
        desc.ptData = new uint256[](0);
        desc.inputIsCommitted = new bool[](1);
        desc.inputIsCommitted[0] = true;
        desc.atomCommitIdxs = new uint256[](2);
        desc.oracleProductOffsets = new uint256[](3);
        desc.oracleResultIdxs = new uint256[](0);
        desc.oracleExprCoeffs = new uint256[](0);

        bytes memory descData = abi.encode(desc);
        verifier.registerDAGCircuit(circuitHash, descData, "test-circuit", bytes32(0));
    }

    /// @notice Test setting a Stylus verifier
    function test_set_stylus_verifier() public {
        verifier.setDAGStylusVerifier(circuitHash, address(mockStylus));
        assertEq(verifier.dagCircuitStylusVerifiers(circuitHash), address(mockStylus));
    }

    /// @notice Test setting Stylus verifier for unregistered circuit reverts
    function test_set_stylus_verifier_unregistered_reverts() public {
        bytes32 unknownHash = bytes32(uint256(0xdead));
        vm.expectRevert(RemainderVerifier.DAGCircuitNotRegistered.selector);
        verifier.setDAGStylusVerifier(unknownHash, address(mockStylus));
    }

    /// @notice Test only admin can set Stylus verifier
    function test_set_stylus_verifier_not_admin_reverts() public {
        vm.prank(address(0xBEEF));
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, address(0xBEEF)));
        verifier.setDAGStylusVerifier(circuitHash, address(mockStylus));
    }

    /// @notice Test successful delegation to Stylus verifier
    function test_verify_dag_proof_stylus_success() public {
        verifier.setDAGStylusVerifier(circuitHash, address(mockStylus));

        // forge-lint: disable-next-line(unsafe-typecast)
        bytes memory proof = abi.encodePacked(bytes4("REM1"), bytes32(uint256(1)));
        bytes memory publicInputs = abi.encodePacked(uint256(42));
        bytes memory gensData = new bytes(0);

        bool valid = verifier.verifyDAGProofStylus(proof, circuitHash, publicInputs, gensData);
        assertTrue(valid, "Should delegate and return true");
    }

    /// @notice Test delegation returns false when mock returns false
    function test_verify_dag_proof_stylus_returns_false() public {
        verifier.setDAGStylusVerifier(circuitHash, address(mockStylusFalse));

        // forge-lint: disable-next-line(unsafe-typecast)
        bytes memory proof = abi.encodePacked(bytes4("REM1"), bytes32(uint256(1)));
        bytes memory publicInputs = new bytes(0);
        bytes memory gensData = new bytes(0);

        bool valid = verifier.verifyDAGProofStylus(proof, circuitHash, publicInputs, gensData);
        assertFalse(valid, "Should delegate and return false");
    }

    /// @notice Test delegation reverts when Stylus contract reverts
    function test_verify_dag_proof_stylus_revert_propagated() public {
        verifier.setDAGStylusVerifier(circuitHash, address(mockStylusReverts));

        // forge-lint: disable-next-line(unsafe-typecast)
        bytes memory proof = abi.encodePacked(bytes4("REM1"), bytes32(uint256(1)));
        bytes memory publicInputs = new bytes(0);
        bytes memory gensData = new bytes(0);

        vm.expectRevert("mock stylus revert");
        verifier.verifyDAGProofStylus(proof, circuitHash, publicInputs, gensData);
    }

    /// @notice Test unregistered circuit reverts
    function test_verify_dag_proof_stylus_unregistered_reverts() public {
        bytes32 unknownHash = bytes32(uint256(0xdead));
        // forge-lint: disable-next-line(unsafe-typecast)
        bytes memory proof = abi.encodePacked(bytes4("REM1"), bytes32(uint256(1)));

        vm.expectRevert(RemainderVerifier.CircuitNotRegistered.selector);
        verifier.verifyDAGProofStylus(proof, unknownHash, new bytes(0), new bytes(0));
    }

    /// @notice Test no Stylus verifier configured reverts
    function test_verify_dag_proof_stylus_not_configured_reverts() public {
        // forge-lint: disable-next-line(unsafe-typecast)
        bytes memory proof = abi.encodePacked(bytes4("REM1"), bytes32(uint256(1)));

        vm.expectRevert(RemainderVerifier.StylusVerifierNotConfigured.selector);
        verifier.verifyDAGProofStylus(proof, circuitHash, new bytes(0), new bytes(0));
    }

    /// @notice Test invalid proof selector reverts
    function test_verify_dag_proof_stylus_invalid_selector_reverts() public {
        verifier.setDAGStylusVerifier(circuitHash, address(mockStylus));

        // forge-lint: disable-next-line(unsafe-typecast)
        bytes memory proof = abi.encodePacked(bytes4("FAKE"), bytes32(uint256(1)));

        vm.expectRevert(RemainderVerifier.InvalidProofSelector.selector);
        verifier.verifyDAGProofStylus(proof, circuitHash, new bytes(0), new bytes(0));
    }

    /// @notice Test proof too short reverts
    function test_verify_dag_proof_stylus_short_proof_reverts() public {
        verifier.setDAGStylusVerifier(circuitHash, address(mockStylus));

        bytes memory proof = hex"0102";

        vm.expectRevert(RemainderVerifier.InvalidProofLength.selector);
        verifier.verifyDAGProofStylus(proof, circuitHash, new bytes(0), new bytes(0));
    }

    /// @notice Test generators hash mismatch reverts
    function test_verify_dag_proof_stylus_gens_hash_mismatch_reverts() public {
        // Re-register circuit with a gens hash
        bytes32 circuitHash2 = bytes32(uint256(0x5678));
        GKRDAGVerifier.DAGCircuitDescription memory desc;
        desc.numComputeLayers = 2;
        desc.numInputLayers = 1;
        desc.layerTypes = new uint8[](2);
        desc.numSumcheckRounds = new uint256[](2);
        desc.numSumcheckRounds[0] = 3;
        desc.numSumcheckRounds[1] = 3;
        desc.atomOffsets = new uint256[](3);
        desc.atomTargetLayers = new uint256[](0);
        desc.ptOffsets = new uint256[](1);
        desc.ptData = new uint256[](0);
        desc.inputIsCommitted = new bool[](1);
        desc.atomCommitIdxs = new uint256[](0);
        desc.oracleProductOffsets = new uint256[](3);
        desc.oracleResultIdxs = new uint256[](0);
        desc.oracleExprCoeffs = new uint256[](0);

        bytes memory validGens = abi.encodePacked(uint256(99), uint256(100));
        bytes32 gensHash = keccak256(validGens);
        verifier.registerDAGCircuit(circuitHash2, abi.encode(desc), "test-gens", gensHash);
        verifier.setDAGStylusVerifier(circuitHash2, address(mockStylus));

        // forge-lint: disable-next-line(unsafe-typecast)
        bytes memory proof = abi.encodePacked(bytes4("REM1"), bytes32(uint256(1)));
        bytes memory wrongGens = abi.encodePacked(uint256(1), uint256(2));

        vm.expectRevert(RemainderVerifier.InvalidGenerators.selector);
        verifier.verifyDAGProofStylus(proof, circuitHash2, new bytes(0), wrongGens);
    }

    /// @notice Test replacing Stylus verifier address
    function test_replace_stylus_verifier() public {
        verifier.setDAGStylusVerifier(circuitHash, address(mockStylus));
        assertEq(verifier.dagCircuitStylusVerifiers(circuitHash), address(mockStylus));

        verifier.setDAGStylusVerifier(circuitHash, address(mockStylusFalse));
        assertEq(verifier.dagCircuitStylusVerifiers(circuitHash), address(mockStylusFalse));
    }
}
