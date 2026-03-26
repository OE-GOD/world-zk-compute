// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/remainder/HybridStylusGroth16Verifier.sol";
import "../src/remainder/RemainderVerifier.sol";
import "../src/remainder/GKRDAGVerifier.sol";
import "../src/tee/TEEMLVerifier.sol";
import {UUPSUpgradeable, UUPSProxy} from "../src/Upgradeable.sol";
import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";
import {DeployRemainderVerifierHelper} from "./helpers/DeployRemainderVerifier.sol";

/// @dev Mock Stylus verifier that returns configurable results for hybrid mode
contract MockStylusHybridVerifier {
    bool public nextSuccess;
    bytes32 public nextDigest;
    bytes public nextFrOutputs;
    bool public shouldRevert;
    string public revertReason;

    function setResult(bool _success, bytes32 _digest, bytes memory _frOutputs) external {
        nextSuccess = _success;
        nextDigest = _digest;
        nextFrOutputs = _frOutputs;
    }

    function setShouldRevert(bool _revert, string memory _reason) external {
        shouldRevert = _revert;
        revertReason = _reason;
    }

    /// @dev Matches the selector keccak256("verifyDagProofHybrid(bytes,bytes,bytes,bytes)")[:4]
    fallback(bytes calldata) external returns (bytes memory) {
        if (shouldRevert) {
            if (bytes(revertReason).length > 0) {
                revert(revertReason);
            }
            revert();
        }
        return abi.encode(nextSuccess, nextDigest, nextFrOutputs);
    }
}

/// @dev Mock chunked Groth16 verifier that tracks calls and can fail on specific chunks
contract MockChunkedGroth16Verifier {
    uint256 public callCount;
    mapping(uint256 => bool) public failOnChunk;

    function setFailOnChunk(uint256 chunkIdx, bool shouldFail) external {
        failOnChunk[chunkIdx] = shouldFail;
    }

    fallback(bytes calldata data) external returns (bytes memory) {
        // Extract chunk index from public inputs (4th uint256 after proof + selector)
        // The chunk index is at a known offset in the calldata
        uint256 currentCall = callCount;
        callCount++;
        if (failOnChunk[currentCall]) revert("chunk failed");
        return "";
    }
}

/// @dev Mock Groth16 verifier that returns success (view-safe for staticcall)
contract MockECGroth16Verifier {
    bool public shouldRevert;

    function setShouldRevert(bool _revert) external {
        shouldRevert = _revert;
    }

    /// @dev Accepts any call with the right selector pattern.
    ///      Only reads storage so staticcall is safe at EVM level.
    fallback(bytes calldata) external returns (bytes memory) {
        if (shouldRevert) revert("groth16 failed");
        return "";
    }
}

/// @dev Mock RemainderVerifier for TEEMLVerifier hybrid routing tests
contract MockHybridRemainderVerifier {
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

    function verifyDAGProofStylusGroth16(bytes calldata, bytes32, bytes calldata, bytes calldata, uint256[8] calldata)
        external
        returns (bool)
    {
        lastCalled = "hybrid";
        return nextResult;
    }

    function verifyDAGProofStylusGroth16Chunked(
        bytes calldata,
        bytes32,
        bytes calldata,
        bytes calldata,
        uint256[][] calldata,
        uint256,
        uint256
    ) external returns (bool) {
        lastCalled = "hybrid-chunked";
        return nextResult;
    }
}

/// @dev Wrapper to expose library internal functions for testing.
///      verifyHybridTwoStep is split into stylus + groth16 steps to avoid stack-too-deep.
contract HybridVerifierHarness {
    // Intermediate state from step 1 (stored to avoid stack depth issues)
    bytes32 private _lastDigest;
    bytes private _lastFrOutputs;
    bool private _lastStylusOk;

    function buildGroth16Inputs(bytes32 digest, bytes32 circuitHash, bytes memory frOutputs, uint256 expectedCount)
        external
        pure
        returns (uint256[] memory)
    {
        return HybridStylusGroth16Verifier.buildGroth16Inputs(digest, circuitHash, frOutputs, expectedCount);
    }

    function computeGroth16Selector(uint256 n) external pure returns (bytes4) {
        return HybridStylusGroth16Verifier.computeGroth16Selector(n);
    }

    /// @dev Two-step hybrid verification. Uses storage to pass state between steps.
    function verifyHybridTwoStep(
        address stylusVerifier,
        address ecGroth16Verifier,
        uint256 ecGroth16InputCount,
        bytes calldata proof,
        bytes32 circuitHash,
        bytes calldata publicInputs,
        bytes calldata gensData,
        bytes memory circuitDescData,
        uint256[8] calldata ecGroth16Proof
    ) external returns (bool) {
        // Step 1: Stylus call
        _runStylusStep(stylusVerifier, proof, publicInputs, gensData, circuitDescData);
        if (!_lastStylusOk) return false;

        // Step 2: Groth16 call
        _runGroth16Step(ecGroth16Verifier, ecGroth16InputCount, circuitHash, ecGroth16Proof);
        return true;
    }

    function _runStylusStep(
        address stylusVerifier,
        bytes calldata proof,
        bytes calldata publicInputs,
        bytes calldata gensData,
        bytes memory circuitDescData
    ) private {
        (bool ok, bytes32 digest, bytes memory frOutputs) = HybridStylusGroth16Verifier.callStylusHybrid(
            stylusVerifier, proof, publicInputs, gensData, circuitDescData
        );
        _lastStylusOk = ok;
        _lastDigest = digest;
        _lastFrOutputs = frOutputs;
    }

    function _runGroth16Step(
        address ecGroth16Verifier,
        uint256 ecGroth16InputCount,
        bytes32 circuitHash,
        uint256[8] calldata ecGroth16Proof
    ) private view {
        uint256[] memory inputs = HybridStylusGroth16Verifier.buildGroth16Inputs(
            _lastDigest, circuitHash, _lastFrOutputs, ecGroth16InputCount
        );
        bytes4 sel = HybridStylusGroth16Verifier.computeGroth16Selector(ecGroth16InputCount);
        HybridStylusGroth16Verifier.callGroth16Verifier(ecGroth16Verifier, sel, ecGroth16Proof, inputs);
    }

    function callStylusHybrid(
        address stylusVerifier,
        bytes calldata proof,
        bytes calldata publicInputs,
        bytes calldata gensData,
        bytes memory circuitDescData
    ) external view returns (bool, bytes32, bytes memory) {
        return HybridStylusGroth16Verifier.callStylusHybrid(
            stylusVerifier, proof, publicInputs, gensData, circuitDescData
        );
    }

    function buildChunkedGroth16Inputs(
        bytes32 transcriptDigest,
        bytes32 circuitHash,
        uint256 chunkIndex,
        uint256 totalChunks,
        uint256 opsDigest,
        uint256 expectedCount
    ) external pure returns (uint256[] memory) {
        return HybridStylusGroth16Verifier.buildChunkedGroth16Inputs(
            transcriptDigest, circuitHash, chunkIndex, totalChunks, opsDigest, expectedCount
        );
    }

    function verifyAllChunks(
        address ecVerifier,
        uint256 ecInputCount,
        bytes32 transcriptDigest,
        bytes32 circuitHash,
        uint256[][] memory chunkProofs,
        uint256 totalChunks,
        uint256 opsDigest
    ) external view {
        HybridStylusGroth16Verifier.verifyAllChunks(
            ecVerifier, ecInputCount, transcriptDigest, circuitHash, chunkProofs, totalChunks, opsDigest
        );
    }

    /// @dev Full chunked hybrid verification using storage-based step splitting
    function verifyHybridChunkedTwoStep(
        address stylusVerifier,
        address ecGroth16Verifier,
        uint256 ecGroth16InputCount,
        bytes calldata proof,
        bytes32 circuitHash,
        bytes calldata publicInputs,
        bytes calldata gensData,
        bytes memory circuitDescData,
        uint256[][] memory ecGroth16Proofs
    ) external returns (bool) {
        // Step 1: Stylus call
        _runStylusStep(stylusVerifier, proof, publicInputs, gensData, circuitDescData);
        if (!_lastStylusOk) return false;

        // Step 2: Chunked Groth16
        uint256 opsDigest = uint256(keccak256(_lastFrOutputs)) % FR_MODULUS;
        HybridStylusGroth16Verifier.verifyAllChunks(
            ecGroth16Verifier,
            ecGroth16InputCount,
            _lastDigest,
            circuitHash,
            ecGroth16Proofs,
            ecGroth16Proofs.length,
            opsDigest
        );
        return true;
    }

    uint256 constant FR_MODULUS = 21888242871839275222246405745257275088548364400416034343698204186575808495617;
}

// =============================================================================
// TEST: HybridStylusGroth16Verifier library
// =============================================================================

contract HybridStylusGroth16VerifierTest is Test {
    HybridVerifierHarness harness;
    MockStylusHybridVerifier mockStylus;
    MockECGroth16Verifier mockGroth16;

    uint256 constant FR_MODULUS = 21888242871839275222246405745257275088548364400416034343698204186575808495617;

    function setUp() public {
        harness = new HybridVerifierHarness();
        mockStylus = new MockStylusHybridVerifier();
        mockGroth16 = new MockECGroth16Verifier();
    }

    // ---- buildGroth16Inputs tests ----

    function test_buildGroth16Inputs_basicLayout() public view {
        bytes32 digest = bytes32(uint256(42));
        bytes32 circuitHash = bytes32(uint256(123));
        // 2 Fr outputs (64 bytes)
        bytes memory frOutputs = abi.encodePacked(uint256(100), uint256(200));

        uint256[] memory inputs = harness.buildGroth16Inputs(digest, circuitHash, frOutputs, 0);

        // Should have 3 + 2 = 5 inputs
        assertEq(inputs.length, 5);
        // First input is digest reduced mod Fr
        assertEq(inputs[0], uint256(digest) % FR_MODULUS);
        // Inputs 3,4 are the Fr outputs
        assertEq(inputs[3], 100);
        assertEq(inputs[4], 200);
    }

    function test_buildGroth16Inputs_digestReducedModFr() public view {
        // Use a value larger than FR_MODULUS
        bytes32 digest = bytes32(type(uint256).max);
        bytes memory frOutputs = "";

        uint256[] memory inputs = harness.buildGroth16Inputs(digest, bytes32(0), frOutputs, 0);

        assertEq(inputs.length, 3);
        assertEq(inputs[0], type(uint256).max % FR_MODULUS);
        assertTrue(inputs[0] < FR_MODULUS);
    }

    function test_buildGroth16Inputs_circuitHashSplit() public view {
        // Known circuit hash
        bytes32 circuitHash = hex"0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";
        bytes memory frOutputs = "";

        uint256[] memory inputs = harness.buildGroth16Inputs(bytes32(0), circuitHash, frOutputs, 0);

        // The hash is split into two 128-bit LE halves
        // First 16 bytes: 0x0102...0f10 → LE
        // Second 16 bytes: 0x1112...1f20 → LE
        assertTrue(inputs[1] > 0, "circuitHashLo should be non-zero");
        assertTrue(inputs[2] > 0, "circuitHashHi should be non-zero");
        // Both should fit in 128 bits
        assertTrue(inputs[1] < (1 << 128), "lo should fit in 128 bits");
        assertTrue(inputs[2] < (1 << 128), "hi should fit in 128 bits");
    }

    function test_buildGroth16Inputs_expectedCountPads() public view {
        bytes32 digest = bytes32(uint256(1));
        bytes memory frOutputs = abi.encodePacked(uint256(10));
        // Natural count = 3 + 1 = 4, but expect 6
        uint256[] memory inputs = harness.buildGroth16Inputs(digest, bytes32(0), frOutputs, 6);

        assertEq(inputs.length, 6);
        // First 4 are populated, last 2 are zero-padded
        assertEq(inputs[3], 10);
        assertEq(inputs[4], 0);
        assertEq(inputs[5], 0);
    }

    function test_buildGroth16Inputs_expectedCountTruncates() public view {
        bytes memory frOutputs = abi.encodePacked(uint256(10), uint256(20), uint256(30));
        // Natural count = 3 + 3 = 6, but expect 4 (truncate)
        uint256[] memory inputs = harness.buildGroth16Inputs(bytes32(uint256(1)), bytes32(0), frOutputs, 4);

        assertEq(inputs.length, 4);
        // Only 1 Fr output fits (4 - 3 = 1)
        assertEq(inputs[3], 10);
    }

    function test_buildGroth16Inputs_noFrOutputs() public view {
        uint256[] memory inputs = harness.buildGroth16Inputs(bytes32(uint256(1)), bytes32(uint256(2)), "", 0);

        assertEq(inputs.length, 3);
    }

    // ---- computeGroth16Selector tests ----

    function test_computeGroth16Selector_known() public view {
        // verifyProof(uint256[8],uint256[3])
        bytes4 expected = bytes4(keccak256("verifyProof(uint256[8],uint256[3])"));
        assertEq(harness.computeGroth16Selector(3), expected);
    }

    function test_computeGroth16Selector_various() public view {
        assertEq(harness.computeGroth16Selector(0), bytes4(keccak256("verifyProof(uint256[8],uint256[0])")));
        assertEq(harness.computeGroth16Selector(100), bytes4(keccak256("verifyProof(uint256[8],uint256[100])")));
        assertEq(harness.computeGroth16Selector(3416), bytes4(keccak256("verifyProof(uint256[8],uint256[3416])")));
    }

    // ---- callStylusHybrid tests ----

    function test_callStylusHybrid_success() public {
        bytes32 expectedDigest = bytes32(uint256(0xCAFE));
        bytes memory expectedFrOutputs = abi.encodePacked(uint256(1), uint256(2));
        mockStylus.setResult(true, expectedDigest, expectedFrOutputs);

        (bool ok, bytes32 digest, bytes memory frOutputs) =
            harness.callStylusHybrid(address(mockStylus), hex"", hex"", hex"", "");

        assertTrue(ok);
        assertEq(digest, expectedDigest);
        assertEq(frOutputs.length, expectedFrOutputs.length);
    }

    function test_callStylusHybrid_stylusReturnsFalse() public {
        mockStylus.setResult(false, bytes32(0), "");

        (bool ok,,) = harness.callStylusHybrid(address(mockStylus), hex"", hex"", hex"", "");

        assertFalse(ok);
    }

    function test_callStylusHybrid_revertsPropagated() public {
        mockStylus.setShouldRevert(true, "test revert reason");

        vm.expectRevert();
        harness.callStylusHybrid(address(mockStylus), hex"", hex"", hex"", "");
    }

    function test_callStylusHybrid_revertsNoReason() public {
        mockStylus.setShouldRevert(true, "");

        vm.expectRevert(HybridStylusGroth16Verifier.StylusVerificationFailed.selector);
        harness.callStylusHybrid(address(mockStylus), hex"", hex"", hex"", "");
    }

    // ---- verifyHybrid end-to-end tests ----

    function test_verifyHybrid_fullSuccess() public {
        bytes32 digest = bytes32(uint256(0xBEEF));
        bytes memory frOutputs = abi.encodePacked(uint256(42));
        mockStylus.setResult(true, digest, frOutputs);

        uint256[8] memory ecProof;
        bool result = harness.verifyHybridTwoStep(
            address(mockStylus),
            address(mockGroth16),
            4, // expectedCount: 3 (digest + circuitHash lo/hi) + 1 frOutput = 4
            hex"1234",
            bytes32(uint256(99)),
            hex"5678",
            hex"abcd",
            "",
            ecProof
        );

        assertTrue(result);
    }

    function test_verifyHybrid_stylusFails() public {
        mockStylus.setResult(false, bytes32(0), "");

        uint256[8] memory ecProof;
        bool result = harness.verifyHybridTwoStep(
            address(mockStylus), address(mockGroth16), 3, hex"1234", bytes32(0), hex"", hex"", "", ecProof
        );

        assertFalse(result);
    }

    function test_verifyHybrid_groth16Fails() public {
        mockStylus.setResult(true, bytes32(uint256(1)), "");
        mockGroth16.setShouldRevert(true);

        uint256[8] memory ecProof;
        vm.expectRevert();
        harness.verifyHybridTwoStep(
            address(mockStylus), address(mockGroth16), 3, hex"1234", bytes32(0), hex"", hex"", "", ecProof
        );
    }

    // ---- buildChunkedGroth16Inputs tests ----

    function test_buildChunkedGroth16Inputs_basicLayout() public view {
        bytes32 digest = bytes32(uint256(42));
        bytes32 cHash = bytes32(uint256(123));

        uint256[] memory inputs = harness.buildChunkedGroth16Inputs(digest, cHash, 2, 5, 999, 0);

        assertEq(inputs.length, 6);
        assertEq(inputs[0], uint256(digest) % FR_MODULUS);
        // inputs[1] and [2] are circuitHash lo/hi
        assertEq(inputs[3], 2); // chunkIndex
        assertEq(inputs[4], 5); // totalChunks
        assertEq(inputs[5], 999); // opsDigest
    }

    function test_buildChunkedGroth16Inputs_expectedCountPads() public view {
        uint256[] memory inputs = harness.buildChunkedGroth16Inputs(bytes32(uint256(1)), bytes32(0), 0, 3, 42, 10);

        assertEq(inputs.length, 10);
        assertEq(inputs[3], 0); // chunkIndex
        assertEq(inputs[4], 3); // totalChunks
        assertEq(inputs[5], 42); // opsDigest
        // Remaining slots are zero-padded
        assertEq(inputs[6], 0);
        assertEq(inputs[9], 0);
    }

    function test_buildChunkedGroth16Inputs_digestReducedModFr() public view {
        bytes32 digest = bytes32(type(uint256).max);
        uint256[] memory inputs = harness.buildChunkedGroth16Inputs(digest, bytes32(0), 0, 1, 0, 0);

        assertEq(inputs[0], type(uint256).max % FR_MODULUS);
        assertTrue(inputs[0] < FR_MODULUS);
    }

    // ---- verifyAllChunks tests ----

    function test_verifyAllChunks_singleChunk() public view {
        uint256[][] memory chunks = new uint256[][](1);
        chunks[0] = new uint256[](8);

        harness.verifyAllChunks(address(mockGroth16), 6, bytes32(uint256(1)), bytes32(uint256(2)), chunks, 1, 42);
    }

    function test_verifyAllChunks_multipleChunks() public view {
        uint256[][] memory chunks = new uint256[][](3);
        for (uint256 i = 0; i < 3; i++) {
            chunks[i] = new uint256[](8);
        }

        harness.verifyAllChunks(address(mockGroth16), 6, bytes32(uint256(1)), bytes32(uint256(2)), chunks, 3, 42);
    }

    function test_verifyAllChunks_revertsZeroChunks() public {
        uint256[][] memory chunks = new uint256[][](0);

        vm.expectRevert(HybridStylusGroth16Verifier.InvalidChunkCount.selector);
        harness.verifyAllChunks(address(mockGroth16), 6, bytes32(0), bytes32(0), chunks, 0, 0);
    }

    function test_verifyAllChunks_revertsMismatchedCount() public {
        uint256[][] memory chunks = new uint256[][](2);
        chunks[0] = new uint256[](8);
        chunks[1] = new uint256[](8);

        vm.expectRevert(HybridStylusGroth16Verifier.InvalidChunkCount.selector);
        harness.verifyAllChunks(address(mockGroth16), 6, bytes32(0), bytes32(0), chunks, 3, 0);
    }

    function test_verifyAllChunks_revertsWrongProofLength() public {
        uint256[][] memory chunks = new uint256[][](1);
        chunks[0] = new uint256[](7); // Should be 8

        vm.expectRevert(abi.encodeWithSelector(HybridStylusGroth16Verifier.ChunkVerificationFailed.selector, 0));
        harness.verifyAllChunks(address(mockGroth16), 6, bytes32(0), bytes32(0), chunks, 1, 0);
    }

    function test_verifyAllChunks_revertsOnGroth16Failure() public {
        mockGroth16.setShouldRevert(true);

        uint256[][] memory chunks = new uint256[][](1);
        chunks[0] = new uint256[](8);

        vm.expectRevert();
        harness.verifyAllChunks(address(mockGroth16), 6, bytes32(0), bytes32(0), chunks, 1, 0);
    }

    // ---- verifyHybridChunked end-to-end tests ----

    function test_verifyHybridChunked_fullSuccess() public {
        bytes32 digest = bytes32(uint256(0xBEEF));
        bytes memory frOutputs = abi.encodePacked(uint256(42));
        mockStylus.setResult(true, digest, frOutputs);

        uint256[][] memory chunks = new uint256[][](2);
        for (uint256 i = 0; i < 2; i++) {
            chunks[i] = new uint256[](8);
        }

        bool result = harness.verifyHybridChunkedTwoStep(
            address(mockStylus),
            address(mockGroth16),
            6,
            hex"1234",
            bytes32(uint256(99)),
            hex"5678",
            hex"abcd",
            "",
            chunks
        );

        assertTrue(result);
    }

    function test_verifyHybridChunked_stylusFails() public {
        mockStylus.setResult(false, bytes32(0), "");

        uint256[][] memory chunks = new uint256[][](1);
        chunks[0] = new uint256[](8);

        bool result = harness.verifyHybridChunkedTwoStep(
            address(mockStylus), address(mockGroth16), 6, hex"1234", bytes32(0), hex"", hex"", "", chunks
        );

        assertFalse(result);
    }
}

// =============================================================================
// TEST: TEEMLVerifier hybrid routing
// =============================================================================

contract TEEMLVerifierHybridTest is Test {
    event StylusGroth16Toggled(bool enabled);
    event DisputeResolved(bytes32 indexed resultId, bool proverWon);

    TEEMLVerifier verifier;
    MockHybridRemainderVerifier mockVerifier;

    address admin = address(this);
    uint256 enclavePrivateKey = 0xA11CE;
    address enclaveAddr;
    bytes32 imageHash = keccak256("test-enclave-image-v1");

    bytes32 modelHash = keccak256("xgboost-model-weights");
    bytes32 inputHash = keccak256("test-input-data");
    bytes resultData = hex"deadbeef";

    uint256 constant DEFAULT_PROVER_STAKE = 0.1 ether;
    uint256 constant DEFAULT_CHALLENGE_BOND = 0.1 ether;

    receive() external payable {}

    function setUp() public {
        enclaveAddr = vm.addr(enclavePrivateKey);
        mockVerifier = new MockHybridRemainderVerifier();
        verifier = new TEEMLVerifier(admin, address(mockVerifier));
        verifier.registerEnclave(enclaveAddr, imageHash);
    }

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
        bytes32 resultHash_ = keccak256(_result);
        bytes32 structHash = keccak256(abi.encode(verifier.RESULT_TYPEHASH(), _modelHash, _inputHash, resultHash_));
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", _domainSeparator(), structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, digest);
        return abi.encodePacked(r, s, v);
    }

    function _submitAndChallenge() internal returns (bytes32 resultId) {
        bytes memory att = _signAttestation(modelHash, inputHash, resultData);
        resultId = verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, inputHash, resultData, att);

        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        verifier.challenge{value: DEFAULT_CHALLENGE_BOND}(resultId);
    }

    // ---- setUseStylusGroth16 tests ----

    function test_setUseStylusGroth16_toggle() public {
        assertFalse(verifier.useStylusGroth16());

        verifier.setUseStylusGroth16(true);
        assertTrue(verifier.useStylusGroth16());

        verifier.setUseStylusGroth16(false);
        assertFalse(verifier.useStylusGroth16());
    }

    function test_setUseStylusGroth16_onlyOwner() public {
        address nonOwner = address(0xBEEF);
        vm.prank(nonOwner);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, nonOwner));
        verifier.setUseStylusGroth16(true);
    }

    function test_setUseStylusGroth16_emitsEvent() public {
        vm.expectEmit(false, false, false, true);
        emit StylusGroth16Toggled(true);
        verifier.setUseStylusGroth16(true);
    }

    // ---- resolveDisputeHybrid tests ----

    function test_resolveDisputeHybrid_routesToHybrid() public {
        verifier.setUseStylusGroth16(true);
        bytes32 resultId = _submitAndChallenge();
        mockVerifier.setResult(true);

        uint256 balBefore = address(this).balance;

        vm.expectEmit(true, false, false, true);
        emit DisputeResolved(resultId, true);

        uint256[8] memory ecProof;
        verifier.resolveDisputeHybrid(resultId, hex"", bytes32(0), hex"", hex"", ecProof);

        assertEq(
            keccak256(bytes(mockVerifier.lastCalled())),
            keccak256(bytes("hybrid")),
            "Should route to verifyDAGProofStylusGroth16"
        );
        assertTrue(verifier.disputeResolved(resultId));
        assertTrue(verifier.disputeProverWon(resultId));
        assertTrue(verifier.isResultValid(resultId));
        assertEq(address(this).balance, balBefore + DEFAULT_PROVER_STAKE + DEFAULT_CHALLENGE_BOND);
    }

    function test_resolveDisputeHybrid_revertsWhenDisabled() public {
        assertFalse(verifier.useStylusGroth16());
        bytes32 resultId = _submitAndChallenge();

        uint256[8] memory ecProof;
        vm.expectRevert(abi.encodeWithSelector(ITEEMLVerifier.HybridNotEnabled.selector));
        verifier.resolveDisputeHybrid(resultId, hex"", bytes32(0), hex"", hex"", ecProof);
    }

    function test_resolveDisputeHybrid_challengerWins() public {
        verifier.setUseStylusGroth16(true);
        bytes32 resultId = _submitAndChallenge();
        mockVerifier.setResult(false);

        address challenger = address(0xC0FFEE);
        uint256 challengerBalBefore = challenger.balance;

        uint256[8] memory ecProof;
        verifier.resolveDisputeHybrid(resultId, hex"", bytes32(0), hex"", hex"", ecProof);

        assertTrue(verifier.disputeResolved(resultId));
        assertFalse(verifier.disputeProverWon(resultId));
        assertFalse(verifier.isResultValid(resultId));
        // Challenger gets their bond + prover stake
        assertEq(challenger.balance, challengerBalBefore + DEFAULT_CHALLENGE_BOND + DEFAULT_PROVER_STAKE);
    }

    function test_resolveDisputeHybrid_revertsNotChallenged() public {
        verifier.setUseStylusGroth16(true);

        bytes memory att = _signAttestation(modelHash, inputHash, resultData);
        bytes32 resultId = verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, inputHash, resultData, att);

        uint256[8] memory ecProof;
        vm.expectRevert(abi.encodeWithSelector(ITEEMLVerifier.NotChallenged.selector));
        verifier.resolveDisputeHybrid(resultId, hex"", bytes32(0), hex"", hex"", ecProof);
    }

    function test_resolveDisputeHybrid_revertsAlreadyResolved() public {
        verifier.setUseStylusGroth16(true);
        bytes32 resultId = _submitAndChallenge();
        mockVerifier.setResult(true);

        uint256[8] memory ecProof;
        verifier.resolveDisputeHybrid(resultId, hex"", bytes32(0), hex"", hex"", ecProof);

        vm.expectRevert(abi.encodeWithSelector(ITEEMLVerifier.AlreadyResolved.selector));
        verifier.resolveDisputeHybrid(resultId, hex"", bytes32(0), hex"", hex"", ecProof);
    }

    // ---- resolveDisputeHybridChunked tests ----

    function test_resolveDisputeHybridChunked_routesToChunked() public {
        verifier.setUseStylusGroth16(true);
        bytes32 resultId = _submitAndChallenge();
        mockVerifier.setResult(true);

        uint256 balBefore = address(this).balance;

        vm.expectEmit(true, false, false, true);
        emit DisputeResolved(resultId, true);

        uint256[][] memory chunks = new uint256[][](2);
        for (uint256 i = 0; i < 2; i++) {
            chunks[i] = new uint256[](8);
        }

        verifier.resolveDisputeHybridChunked(resultId, hex"", bytes32(0), hex"", hex"", chunks, 2, 42);

        assertEq(
            keccak256(bytes(mockVerifier.lastCalled())),
            keccak256(bytes("hybrid-chunked")),
            "Should route to verifyDAGProofStylusGroth16Chunked"
        );
        assertTrue(verifier.disputeResolved(resultId));
        assertTrue(verifier.disputeProverWon(resultId));
        assertTrue(verifier.isResultValid(resultId));
        assertEq(address(this).balance, balBefore + DEFAULT_PROVER_STAKE + DEFAULT_CHALLENGE_BOND);
    }

    function test_resolveDisputeHybridChunked_revertsWhenDisabled() public {
        assertFalse(verifier.useStylusGroth16());
        bytes32 resultId = _submitAndChallenge();

        uint256[][] memory chunks = new uint256[][](1);
        chunks[0] = new uint256[](8);

        vm.expectRevert(abi.encodeWithSelector(ITEEMLVerifier.HybridNotEnabled.selector));
        verifier.resolveDisputeHybridChunked(resultId, hex"", bytes32(0), hex"", hex"", chunks, 1, 0);
    }

    function test_resolveDisputeHybridChunked_challengerWins() public {
        verifier.setUseStylusGroth16(true);
        bytes32 resultId = _submitAndChallenge();
        mockVerifier.setResult(false);

        address challenger = address(0xC0FFEE);
        uint256 challengerBalBefore = challenger.balance;

        uint256[][] memory chunks = new uint256[][](1);
        chunks[0] = new uint256[](8);

        verifier.resolveDisputeHybridChunked(resultId, hex"", bytes32(0), hex"", hex"", chunks, 1, 0);

        assertTrue(verifier.disputeResolved(resultId));
        assertFalse(verifier.disputeProverWon(resultId));
        assertFalse(verifier.isResultValid(resultId));
        assertEq(challenger.balance, challengerBalBefore + DEFAULT_CHALLENGE_BOND + DEFAULT_PROVER_STAKE);
    }

    function test_resolveDisputeHybridChunked_revertsNotChallenged() public {
        verifier.setUseStylusGroth16(true);

        bytes memory att = _signAttestation(modelHash, inputHash, resultData);
        bytes32 resultId = verifier.submitResult{value: DEFAULT_PROVER_STAKE}(modelHash, inputHash, resultData, att);

        uint256[][] memory chunks = new uint256[][](1);
        chunks[0] = new uint256[](8);

        vm.expectRevert(abi.encodeWithSelector(ITEEMLVerifier.NotChallenged.selector));
        verifier.resolveDisputeHybridChunked(resultId, hex"", bytes32(0), hex"", hex"", chunks, 1, 0);
    }

    function test_resolveDisputeHybridChunked_revertsAlreadyResolved() public {
        verifier.setUseStylusGroth16(true);
        bytes32 resultId = _submitAndChallenge();
        mockVerifier.setResult(true);

        uint256[][] memory chunks = new uint256[][](1);
        chunks[0] = new uint256[](8);

        verifier.resolveDisputeHybridChunked(resultId, hex"", bytes32(0), hex"", hex"", chunks, 1, 0);

        vm.expectRevert(abi.encodeWithSelector(ITEEMLVerifier.AlreadyResolved.selector));
        verifier.resolveDisputeHybridChunked(resultId, hex"", bytes32(0), hex"", hex"", chunks, 1, 0);
    }
}

// =============================================================================
// TEST: RemainderVerifier hybrid admin + routing
// =============================================================================

contract RemainderVerifierHybridTest is Test, DeployRemainderVerifierHelper {
    event DAGECGroth16VerifierUpdated(bytes32 indexed circuitHash, address indexed verifier, uint256 inputCount);

    RemainderVerifier verifier;
    address admin = address(this);

    bytes32 circuitHash = bytes32(uint256(0x1234));

    function setUp() public {
        verifier = _deployRemainderVerifier(admin);

        // Register a minimal DAG circuit so we can test admin functions.
        // atomOffsets[numComputeLayers] = totalAtoms = 0
        // ptOffsets length = totalAtoms + 1 = 1
        // oracleProductOffsets[numComputeLayers] = totalProducts = 0
        GKRDAGVerifier.DAGCircuitDescription memory desc;
        desc.numComputeLayers = 4;
        desc.numInputLayers = 2;
        desc.layerTypes = new uint8[](4);
        desc.numSumcheckRounds = new uint256[](4);
        desc.atomOffsets = new uint256[](5); // all zero → totalAtoms=0
        desc.atomTargetLayers = new uint256[](0);
        desc.atomCommitIdxs = new uint256[](0);
        desc.ptOffsets = new uint256[](1); // totalAtoms + 1 = 1
        desc.ptData = new uint256[](0);
        desc.inputIsCommitted = new bool[](2);
        desc.oracleProductOffsets = new uint256[](5); // all zero → totalProducts=0
        desc.oracleResultIdxs = new uint256[](0);
        desc.oracleExprCoeffs = new uint256[](0);

        verifier.registerDAGCircuit(circuitHash, abi.encode(desc), "test-circuit", bytes32(0));
    }

    // ---- setDAGECGroth16Verifier tests ----

    function test_setDAGECGroth16Verifier_success() public {
        address ecVerifier = address(0xBEEF);

        vm.expectEmit(true, true, false, true);
        emit DAGECGroth16VerifierUpdated(circuitHash, ecVerifier, 100);

        verifier.setDAGECGroth16Verifier(circuitHash, ecVerifier, 100);

        assertEq(verifier.dagECGroth16Verifiers(circuitHash), ecVerifier);
        assertEq(verifier.dagECGroth16InputCounts(circuitHash), 100);
    }

    function test_setDAGECGroth16Verifier_onlyAdmin() public {
        address nonAdmin = address(0xDEAD);
        vm.prank(nonAdmin);
        vm.expectRevert(UUPSUpgradeable.NotAdmin.selector);
        verifier.setDAGECGroth16Verifier(circuitHash, address(0xBEEF), 100);
    }

    function test_setDAGECGroth16Verifier_revertsUnregisteredCircuit() public {
        bytes32 unknownHash = bytes32(uint256(0x9999));
        vm.expectRevert(RemainderVerifier.DAGCircuitNotRegistered.selector);
        verifier.setDAGECGroth16Verifier(unknownHash, address(0xBEEF), 100);
    }

    function test_setDAGECGroth16Verifier_revertsZeroAddress() public {
        vm.expectRevert(RemainderVerifier.ZeroAddress.selector);
        verifier.setDAGECGroth16Verifier(circuitHash, address(0), 100);
    }

    // ---- verifyDAGProofStylusGroth16 validation tests ----

    function test_verifyDAGProofStylusGroth16_revertsUnregisteredCircuit() public {
        bytes32 unknownHash = bytes32(uint256(0x9999));
        uint256[8] memory ecProof;
        vm.expectRevert(RemainderVerifier.CircuitNotRegistered.selector);
        verifier.verifyDAGProofStylusGroth16(hex"52454d31", unknownHash, hex"", hex"", ecProof);
    }

    function test_verifyDAGProofStylusGroth16_revertsInvalidSelector() public {
        // Set up Stylus verifier so we get past that check
        verifier.setDAGStylusVerifier(circuitHash, address(0xBEEF));
        verifier.setDAGECGroth16Verifier(circuitHash, address(0xBEEF), 3);

        uint256[8] memory ecProof;
        // "XXXX" instead of "REM1"
        vm.expectRevert(RemainderVerifier.InvalidProofSelector.selector);
        verifier.verifyDAGProofStylusGroth16(hex"58585858", circuitHash, hex"", hex"", ecProof);
    }

    function test_verifyDAGProofStylusGroth16_revertsInvalidProofLength() public {
        uint256[8] memory ecProof;
        // Proof too short (< 4 bytes)
        vm.expectRevert(RemainderVerifier.InvalidProofLength.selector);
        verifier.verifyDAGProofStylusGroth16(hex"52454d", circuitHash, hex"", hex"", ecProof);
    }

    // ---- verifyDAGProofStylusGroth16Chunked validation tests ----

    function test_verifyDAGProofStylusGroth16Chunked_revertsUnregisteredCircuit() public {
        bytes32 unknownHash = bytes32(uint256(0x9999));
        uint256[][] memory chunks = new uint256[][](1);
        chunks[0] = new uint256[](8);

        vm.expectRevert(RemainderVerifier.CircuitNotRegistered.selector);
        verifier.verifyDAGProofStylusGroth16Chunked(hex"52454d31", unknownHash, hex"", hex"", chunks, 1, 0);
    }

    function test_verifyDAGProofStylusGroth16Chunked_revertsInvalidSelector() public {
        verifier.setDAGStylusVerifier(circuitHash, address(0xBEEF));
        verifier.setDAGECGroth16Verifier(circuitHash, address(0xBEEF), 6);

        uint256[][] memory chunks = new uint256[][](1);
        chunks[0] = new uint256[](8);

        // "XXXX" instead of "REM1"
        vm.expectRevert(RemainderVerifier.InvalidProofSelector.selector);
        verifier.verifyDAGProofStylusGroth16Chunked(hex"58585858", circuitHash, hex"", hex"", chunks, 1, 0);
    }

    function test_verifyDAGProofStylusGroth16Chunked_revertsInvalidProofLength() public {
        uint256[][] memory chunks = new uint256[][](1);
        chunks[0] = new uint256[](8);

        // Proof too short (< 4 bytes)
        vm.expectRevert(RemainderVerifier.InvalidProofLength.selector);
        verifier.verifyDAGProofStylusGroth16Chunked(hex"52454d", circuitHash, hex"", hex"", chunks, 1, 0);
    }
}

// =============================================================================
// TEST: Chunked Hybrid Gas Profile
// =============================================================================

contract ChunkedHybridGasProfileTest is Test, DeployRemainderVerifierHelper {
    RemainderVerifier verifier;
    MockStylusHybridVerifier mockStylus;
    MockECGroth16Verifier mockGroth16;

    bytes32 circuitHash = bytes32(uint256(0xDAD));
    bytes constant DUMMY_PROOF = hex"52454d31aabbccdd";
    bytes32 constant DUMMY_DIGEST = bytes32(uint256(0x42));
    bytes constant DUMMY_FR_OUTPUTS = hex"";

    function setUp() public {
        verifier = _deployRemainderVerifier(address(this));
        mockStylus = new MockStylusHybridVerifier();
        mockGroth16 = new MockECGroth16Verifier();

        // Register minimal DAG circuit
        GKRDAGVerifier.DAGCircuitDescription memory desc;
        desc.numComputeLayers = 4;
        desc.numInputLayers = 2;
        desc.layerTypes = new uint8[](4);
        desc.numSumcheckRounds = new uint256[](4);
        desc.atomOffsets = new uint256[](5);
        desc.atomTargetLayers = new uint256[](0);
        desc.atomCommitIdxs = new uint256[](0);
        desc.ptOffsets = new uint256[](1);
        desc.ptData = new uint256[](0);
        desc.inputIsCommitted = new bool[](2);
        desc.oracleProductOffsets = new uint256[](5);
        desc.oracleResultIdxs = new uint256[](0);
        desc.oracleExprCoeffs = new uint256[](0);

        bytes memory descData = abi.encode(desc);
        verifier.registerDAGCircuit(circuitHash, descData, "gas-profile-circuit", bytes32(0));

        // Wire Stylus + EC Groth16 verifiers
        verifier.setDAGStylusVerifier(circuitHash, address(mockStylus));
        verifier.setDAGECGroth16Verifier(circuitHash, address(mockGroth16), 5);

        // Configure mock Stylus to return success
        mockStylus.setResult(true, DUMMY_DIGEST, DUMMY_FR_OUTPUTS);
    }

    function _makeChunks(uint256 numChunks) internal pure returns (uint256[][] memory) {
        uint256[][] memory chunks = new uint256[][](numChunks);
        for (uint256 i = 0; i < numChunks; i++) {
            chunks[i] = new uint256[](8);
        }
        return chunks;
    }

    function test_chunked_hybrid_gas_profile() public {
        emit log("=== CHUNKED HYBRID GAS PROFILE ===");

        // 1 chunk
        {
            uint256 gasBefore = gasleft();
            verifier.verifyDAGProofStylusGroth16Chunked(
                DUMMY_PROOF, circuitHash, hex"", hex"", _makeChunks(1), 1, 0
            );
            uint256 gasUsed = gasBefore - gasleft();
            emit log_named_uint("1 chunk total", gasUsed);
        }

        // 4 chunks
        {
            uint256 gasBefore = gasleft();
            verifier.verifyDAGProofStylusGroth16Chunked(
                DUMMY_PROOF, circuitHash, hex"", hex"", _makeChunks(4), 4, 0
            );
            uint256 gasUsed = gasBefore - gasleft();
            emit log_named_uint("4 chunks total", gasUsed);
        }

        // 8 chunks (expected for full XGBoost)
        {
            uint256 gasBefore = gasleft();
            verifier.verifyDAGProofStylusGroth16Chunked(
                DUMMY_PROOF, circuitHash, hex"", hex"", _makeChunks(8), 8, 0
            );
            uint256 gasUsed = gasBefore - gasleft();
            emit log_named_uint("8 chunks total", gasUsed);
        }

        emit log("vs Solidity DAG direct: ~254M gas");
        emit log("Target: 3-6M gas for single-tx chunked hybrid");
    }
}

// =============================================================================
// TEST: Chunked Hybrid Verification (T491)
// =============================================================================

contract ChunkedHybridVerificationTest is Test, DeployRemainderVerifierHelper {
    event DAGProofVerified(bytes32 indexed circuitHash, bool valid, string method);

    RemainderVerifier verifier;
    MockStylusHybridVerifier mockStylus;
    MockECGroth16Verifier mockGroth16;
    MockChunkedGroth16Verifier mockChunkedGroth16;

    bytes32 circuitHash = bytes32(uint256(0xC0DE));
    bytes constant DUMMY_PROOF = hex"52454d31aabbccdd";
    bytes32 constant DUMMY_DIGEST = bytes32(uint256(0x42));

    function setUp() public {
        verifier = _deployRemainderVerifier(address(this));
        mockStylus = new MockStylusHybridVerifier();
        mockGroth16 = new MockECGroth16Verifier();
        mockChunkedGroth16 = new MockChunkedGroth16Verifier();

        // Register minimal DAG circuit
        GKRDAGVerifier.DAGCircuitDescription memory desc;
        desc.numComputeLayers = 4;
        desc.numInputLayers = 2;
        desc.layerTypes = new uint8[](4);
        desc.numSumcheckRounds = new uint256[](4);
        desc.atomOffsets = new uint256[](5);
        desc.atomTargetLayers = new uint256[](0);
        desc.atomCommitIdxs = new uint256[](0);
        desc.ptOffsets = new uint256[](1);
        desc.ptData = new uint256[](0);
        desc.inputIsCommitted = new bool[](2);
        desc.oracleProductOffsets = new uint256[](5);
        desc.oracleResultIdxs = new uint256[](0);
        desc.oracleExprCoeffs = new uint256[](0);

        bytes memory descData = abi.encode(desc);
        verifier.registerDAGCircuit(circuitHash, descData, "chunked-test-circuit", bytes32(0));

        // Wire verifiers
        verifier.setDAGStylusVerifier(circuitHash, address(mockStylus));
        verifier.setDAGECGroth16Verifier(circuitHash, address(mockGroth16), 5);

        // Configure mock Stylus to succeed
        mockStylus.setResult(true, DUMMY_DIGEST, hex"");
    }

    function _makeChunks(uint256 n) internal pure returns (uint256[][] memory) {
        uint256[][] memory chunks = new uint256[][](n);
        for (uint256 i = 0; i < n; i++) {
            chunks[i] = new uint256[](8);
        }
        return chunks;
    }

    function test_chunked_all_pass_1_chunk() public {
        bool valid = verifier.verifyDAGProofStylusGroth16Chunked(
            DUMMY_PROOF, circuitHash, hex"", hex"", _makeChunks(1), 1, 0
        );
        assertTrue(valid, "1 chunk should pass");
    }

    function test_chunked_all_pass_3_chunks() public {
        bool valid = verifier.verifyDAGProofStylusGroth16Chunked(
            DUMMY_PROOF, circuitHash, hex"", hex"", _makeChunks(3), 3, 0
        );
        assertTrue(valid, "3 chunks should pass");
    }

    function test_chunked_all_pass_8_chunks() public {
        bool valid = verifier.verifyDAGProofStylusGroth16Chunked(
            DUMMY_PROOF, circuitHash, hex"", hex"", _makeChunks(8), 8, 0
        );
        assertTrue(valid, "8 chunks should pass");
    }

    function test_chunked_zero_chunks_reverts() public {
        uint256[][] memory chunks = new uint256[][](0);
        vm.expectRevert(HybridStylusGroth16Verifier.InvalidChunkCount.selector);
        verifier.verifyDAGProofStylusGroth16Chunked(
            DUMMY_PROOF, circuitHash, hex"", hex"", chunks, 0, 0
        );
    }

    function test_chunked_mismatched_count_reverts() public {
        // 2 proofs but totalChunks = 3
        vm.expectRevert(HybridStylusGroth16Verifier.InvalidChunkCount.selector);
        verifier.verifyDAGProofStylusGroth16Chunked(
            DUMMY_PROOF, circuitHash, hex"", hex"", _makeChunks(2), 3, 0
        );
    }

    function test_chunked_wrong_proof_size_reverts() public {
        // Chunk with 7 elements instead of 8
        uint256[][] memory chunks = new uint256[][](1);
        chunks[0] = new uint256[](7);
        vm.expectRevert(abi.encodeWithSelector(HybridStylusGroth16Verifier.ChunkVerificationFailed.selector, 0));
        verifier.verifyDAGProofStylusGroth16Chunked(
            DUMMY_PROOF, circuitHash, hex"", hex"", chunks, 1, 0
        );
    }

    function test_chunked_stylus_failure_returns_false() public {
        // Configure Stylus to fail
        mockStylus.setResult(false, bytes32(0), hex"");
        bool valid = verifier.verifyDAGProofStylusGroth16Chunked(
            DUMMY_PROOF, circuitHash, hex"", hex"", _makeChunks(1), 1, 0
        );
        assertFalse(valid, "Should return false when Stylus fails");
    }

    function test_chunked_groth16_failure_reverts() public {
        // Configure Groth16 to fail
        mockGroth16.setShouldRevert(true);
        vm.expectRevert();
        verifier.verifyDAGProofStylusGroth16Chunked(
            DUMMY_PROOF, circuitHash, hex"", hex"", _makeChunks(1), 1, 0
        );
    }

    function test_chunked_emits_verified_event() public {
        vm.expectEmit(true, false, false, true);
        emit DAGProofVerified(circuitHash, true, "stylus-groth16-chunked");
        verifier.verifyDAGProofStylusGroth16Chunked(
            DUMMY_PROOF, circuitHash, hex"", hex"", _makeChunks(2), 2, 0
        );
    }
}
