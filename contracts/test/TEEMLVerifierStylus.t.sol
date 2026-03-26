// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/tee/TEEMLVerifier.sol";
import {UUPSUpgradeable} from "../src/Upgradeable.sol";
import {DeployTEEMLVerifierHelper} from "./helpers/DeployTEEMLVerifier.sol";

/// @dev Mock RemainderVerifier that tracks which verification function was called
///      via lastCalled string. Used to verify Stylus vs Solidity routing logic.
contract MockStylusRemainderVerifier {
    bool public nextResult;
    string public lastCalled;

    function setResult(bool _result) external {
        nextResult = _result;
    }

    function verifyDAGProof(bytes calldata, bytes32, bytes calldata, bytes calldata) external returns (bool) {
        lastCalled = "solidity";
        return nextResult;
    }

    function verifyDAGProofStylus(bytes calldata, bytes32, bytes calldata, bytes calldata) external returns (bool) {
        lastCalled = "stylus";
        return nextResult;
    }
}

/// @title TEEMLVerifierStylusTest
/// @notice Tests for Stylus routing in TEEMLVerifier.resolveDispute()
contract TEEMLVerifierStylusTest is Test, DeployTEEMLVerifierHelper {
    // Re-declare events for expectEmit (solc 0.8.20 compat)
    event StylusVerifierToggled(bool enabled);
    event DisputeResolved(bytes32 indexed resultId, bool proverWon);

    TEEMLVerifier verifier;
    MockStylusRemainderVerifier mockVerifier;

    address admin = address(this);
    uint256 enclavePrivateKey = 0xA11CE;
    address enclaveAddr;
    bytes32 imageHash = keccak256("test-enclave-image-v1");

    bytes32 modelHash = keccak256("xgboost-model-weights");
    bytes32 inputHash = keccak256("test-input-data");
    bytes resultData = hex"deadbeef";

    uint256 constant DEFAULT_PROVER_STAKE = 0.1 ether;
    uint256 constant DEFAULT_CHALLENGE_BOND = 0.1 ether;

    // Allow test contract to receive ETH (admin / submitter payouts)
    receive() external payable {}

    function setUp() public {
        enclaveAddr = vm.addr(enclavePrivateKey);
        mockVerifier = new MockStylusRemainderVerifier();
        verifier = _deployTEEMLVerifier(admin, address(mockVerifier));

        // Register the test enclave
        verifier.registerEnclave(enclaveAddr, imageHash);
    }

    // ---- Helpers ----

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

    /// @dev Signs an attestation using the test enclave private key
    function _signAttestation(bytes32 _modelHash, bytes32 _inputHash, bytes memory _result)
        internal
        view
        returns (bytes memory)
    {
        bytes32 resultHash = keccak256(_result);
        bytes32 structHash = keccak256(abi.encode(verifier.RESULT_TYPEHASH(), _modelHash, _inputHash, resultHash));
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", _domainSeparator(), structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, digest);
        return abi.encodePacked(r, s, v);
    }

    /// @dev Submits a result and challenges it, returning the resultId
    function _submitAndChallenge() internal returns (bytes32 resultId) {
        bytes memory att = _signAttestation(modelHash, inputHash, resultData);
        resultId = verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, inputHash, resultData, att);

        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);
    }

    // ---- Tests ----

    /// @notice When useStylusVerifier is enabled, resolveDispute routes to verifyDAGProofStylus
    function test_resolveDispute_usesStylusWhenEnabled() public {
        // Enable Stylus routing
        verifier.setUseStylusVerifier(true);
        assertTrue(verifier.useStylusVerifier());

        // Submit and challenge
        bytes32 resultId = _submitAndChallenge();

        // Mock verifier returns true (prover wins)
        mockVerifier.setResult(true);

        uint256 submitterBalBefore = address(this).balance;

        vm.expectEmit(true, false, false, true);
        emit DisputeResolved(resultId, true);

        verifier.resolveDispute(resultId, hex"", bytes32(0), hex"", hex"");

        // Verify Stylus path was taken
        assertEq(
            keccak256(bytes(mockVerifier.lastCalled())),
            keccak256(bytes("stylus")),
            "Should have called verifyDAGProofStylus"
        );

        // Verify prover won
        assertTrue(verifier.disputeResolved(resultId));
        assertTrue(verifier.disputeProverWon(resultId));
        assertTrue(verifier.isResultValid(resultId));

        // Submitter gets proverStake + challengeBond
        assertEq(address(this).balance, submitterBalBefore + DEFAULT_PROVER_STAKE + DEFAULT_CHALLENGE_BOND);
    }

    /// @notice When useStylusVerifier is disabled (default), resolveDispute routes to verifyDAGProof
    function test_resolveDispute_usesSolidityWhenDisabled() public {
        // Default: useStylusVerifier is false
        assertFalse(verifier.useStylusVerifier());

        // Submit and challenge
        bytes32 resultId = _submitAndChallenge();

        // Mock verifier returns true
        mockVerifier.setResult(true);

        verifier.resolveDispute(resultId, hex"", bytes32(0), hex"", hex"");

        // Verify Solidity path was taken
        assertEq(
            keccak256(bytes(mockVerifier.lastCalled())),
            keccak256(bytes("solidity")),
            "Should have called verifyDAGProof"
        );

        // Verify dispute resolved
        assertTrue(verifier.disputeResolved(resultId));
        assertTrue(verifier.disputeProverWon(resultId));
    }

    /// @notice Only the owner can call setUseStylusVerifier
    function test_setUseStylusVerifier_onlyOwner() public {
        address nonOwner = address(0xBEEF);

        vm.prank(nonOwner);
        vm.expectRevert(UUPSUpgradeable.NotAdmin.selector);
        verifier.setUseStylusVerifier(true);
    }

    /// @notice setUseStylusVerifier emits StylusVerifierToggled event
    function test_setUseStylusVerifier_emitsEvent() public {
        vm.expectEmit(false, false, false, true);
        emit StylusVerifierToggled(true);

        verifier.setUseStylusVerifier(true);
    }
}
