// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/tee/TEEMLVerifier.sol";
import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";

/// @dev Mock that supports both verifyDAGProof and verifyDAGProofStylus selectors
contract MockStylusRemainderVerifier {
    bool public nextResult;
    bool public stylusCalled;
    bool public solidityCalled;

    function setResult(bool _result) external {
        nextResult = _result;
    }

    function resetFlags() external {
        stylusCalled = false;
        solidityCalled = false;
    }

    function verifyDAGProof(bytes calldata, bytes32, bytes calldata, bytes calldata) external returns (bool) {
        solidityCalled = true;
        return nextResult;
    }

    function verifyDAGProofStylus(bytes calldata, bytes32, bytes calldata, bytes calldata) external returns (bool) {
        stylusCalled = true;
        return nextResult;
    }
}

contract TEEMLVerifierStylusTest is Test {
    event StylusVerifierToggled(bool enabled);
    event DisputeResolved(bytes32 indexed resultId, bool proverWon);

    TEEMLVerifier verifier;
    MockStylusRemainderVerifier mockVerifier;

    address admin = address(this);
    uint256 enclavePrivateKey = 0xA11CE;
    address enclaveAddr;
    bytes32 imageHash = keccak256("test-enclave-image-v1");

    bytes32 modelHash = keccak256("stylus-test-model");
    bytes32 inputHash = keccak256("stylus-test-input");
    bytes resultData = hex"cafebabe";

    uint256 constant DEFAULT_PROVER_STAKE = 0.1 ether;
    uint256 constant DEFAULT_CHALLENGE_BOND = 0.1 ether;

    address challenger = address(0xBEEF);

    receive() external payable {}

    function setUp() public {
        enclaveAddr = vm.addr(enclavePrivateKey);
        mockVerifier = new MockStylusRemainderVerifier();
        verifier = new TEEMLVerifier(admin, address(mockVerifier));
        verifier.registerEnclave(enclaveAddr, imageHash);
        vm.deal(challenger, 10 ether);
    }

    // ─── Helpers ─────────────────────────────────────────────────────────────

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

    function _submitAndChallenge(bytes32 _modelHash, bytes32 _inputHash) internal returns (bytes32 resultId) {
        bytes memory attestation = _signAttestation(_modelHash, _inputHash, resultData);
        resultId = verifier.submitResult{value: DEFAULT_PROVER_STAKE}(_modelHash, _inputHash, resultData, attestation);
        vm.prank(challenger);
        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);
    }

    // ─── T472: Stylus Routing Tests ──────────────────────────────────────────

    function test_resolveDispute_usesStylusWhenEnabled() public {
        bytes32 resultId = _submitAndChallenge(modelHash, inputHash);

        // Enable Stylus routing
        verifier.setUseStylusVerifier(true);
        mockVerifier.setResult(true);
        mockVerifier.resetFlags();

        // Resolve dispute
        verifier.resolveDispute(resultId, "", bytes32(0), "", "");

        // Verify Stylus selector was called, not Solidity
        assertTrue(mockVerifier.stylusCalled(), "Stylus path should have been called");
        assertFalse(mockVerifier.solidityCalled(), "Solidity path should NOT have been called");

        // Verify dispute outcome
        assertTrue(verifier.disputeResolved(resultId));
        assertTrue(verifier.disputeProverWon(resultId));
    }

    function test_resolveDispute_usesSolidityWhenDisabled() public {
        bytes32 mh2 = keccak256("solidity-test-model");
        bytes32 ih2 = keccak256("solidity-test-input");
        bytes32 resultId = _submitAndChallenge(mh2, ih2);

        // Stylus disabled by default
        assertFalse(verifier.useStylusVerifier());
        mockVerifier.setResult(true);
        mockVerifier.resetFlags();

        // Resolve dispute
        verifier.resolveDispute(resultId, "", bytes32(0), "", "");

        // Verify Solidity selector was called, not Stylus
        assertTrue(mockVerifier.solidityCalled(), "Solidity path should have been called");
        assertFalse(mockVerifier.stylusCalled(), "Stylus path should NOT have been called");
    }

    function test_setUseStylusVerifier_onlyOwner() public {
        // Non-owner should revert
        vm.prank(challenger);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, challenger));
        verifier.setUseStylusVerifier(true);

        // Owner should succeed
        verifier.setUseStylusVerifier(true);
        assertTrue(verifier.useStylusVerifier());
    }

    function test_setUseStylusVerifier_emitsEvent() public {
        vm.expectEmit(false, false, false, true);
        emit StylusVerifierToggled(true);
        verifier.setUseStylusVerifier(true);

        vm.expectEmit(false, false, false, true);
        emit StylusVerifierToggled(false);
        verifier.setUseStylusVerifier(false);
    }

    function test_stylus_disputeSettlesCorrectly_proverWins() public {
        bytes32 mh = keccak256("prover-wins-model");
        bytes32 ih = keccak256("prover-wins-input");
        bytes32 resultId = _submitAndChallenge(mh, ih);

        verifier.setUseStylusVerifier(true);
        mockVerifier.setResult(true);

        uint256 submitterBalBefore = address(this).balance;

        vm.expectEmit(true, false, false, true);
        emit DisputeResolved(resultId, true);
        verifier.resolveDispute(resultId, "", bytes32(0), "", "");

        // Prover wins — submitter gets stake + bond
        uint256 submitterBalAfter = address(this).balance;
        assertEq(submitterBalAfter - submitterBalBefore, DEFAULT_PROVER_STAKE + DEFAULT_CHALLENGE_BOND);
    }

    function test_stylus_disputeSettlesCorrectly_challengerWins() public {
        bytes32 mh = keccak256("challenger-wins-model");
        bytes32 ih = keccak256("challenger-wins-input");
        bytes32 resultId = _submitAndChallenge(mh, ih);

        verifier.setUseStylusVerifier(true);
        mockVerifier.setResult(false);

        uint256 challengerBalBefore = challenger.balance;

        verifier.resolveDispute(resultId, "", bytes32(0), "", "");

        // Challenger wins — gets stake + bond
        uint256 challengerBalAfter = challenger.balance;
        assertEq(challengerBalAfter - challengerBalBefore, DEFAULT_PROVER_STAKE + DEFAULT_CHALLENGE_BOND);
    }

    function test_stylus_toggleDoesNotAffectPendingDisputes() public {
        bytes32 mh = keccak256("toggle-mid-dispute-model");
        bytes32 ih = keccak256("toggle-mid-dispute-input");
        bytes32 resultId = _submitAndChallenge(mh, ih);

        // Submit with Solidity mode, then switch to Stylus before resolving
        assertFalse(verifier.useStylusVerifier());
        verifier.setUseStylusVerifier(true);

        mockVerifier.setResult(true);
        mockVerifier.resetFlags();

        // Should now use Stylus path
        verifier.resolveDispute(resultId, "", bytes32(0), "", "");
        assertTrue(mockVerifier.stylusCalled());
        assertTrue(verifier.disputeResolved(resultId));
    }
}
