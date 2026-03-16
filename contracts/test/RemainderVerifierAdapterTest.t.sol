// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/RemainderVerifierAdapter.sol";
import "../src/remainder/RemainderVerifier.sol";

// ============================================================================
// MOCK CONTRACTS
// ============================================================================

/// @dev Mock RemainderVerifier that records calls and can be configured to pass or fail.
///      Inherits from the real RemainderVerifier so that the adapter's type-cast works,
///      but overrides verifyOrRevert to avoid needing real GKR/Hyrax infrastructure.
contract MockRemainderVerifier is RemainderVerifier {
    bool public shouldRevert;
    string public revertReason;

    // Track last call parameters for assertions
    bytes public lastProof;
    bytes32 public lastCircuitHash;
    bytes public lastPublicInputs;
    bytes public lastGensData;
    uint256 public callCount;

    constructor() RemainderVerifier(address(this)) {}

    /// @dev Override verifyOrRevert to skip real proof verification
    function verifyOrRevert(
        bytes calldata proof,
        bytes32 circuitHash,
        bytes calldata publicInputs,
        bytes calldata gensData
    ) external view override {
        // We cannot write to storage in a view function, so we use a workaround:
        // The test will use vm.expectCall to verify the correct arguments are passed.
        if (shouldRevert) {
            // Use assembly to revert with the stored reason
            revert("MockRemainderVerifier: verification failed");
        }
        // Silence unused variable warnings
        proof;
        circuitHash;
        publicInputs;
        gensData;
    }

    /// @dev Configure whether verifyOrRevert should revert
    function setShouldRevert(bool _shouldRevert) external {
        shouldRevert = _shouldRevert;
    }
}

/// @dev A RemainderVerifier mock that always reverts
contract AlwaysRevertRemainderVerifier is RemainderVerifier {
    constructor() RemainderVerifier(address(this)) {}

    function verifyOrRevert(bytes calldata, bytes32, bytes calldata, bytes calldata) external pure override {
        revert("AlwaysRevertRemainderVerifier: rejected");
    }
}

// ============================================================================
// ADAPTER TESTS
// ============================================================================

contract RemainderVerifierAdapterTest is Test {
    RemainderVerifierAdapter public adapter;
    MockRemainderVerifier public mockVerifier;

    bytes32 constant TEST_CIRCUIT_HASH = bytes32(uint256(0xABCD));
    bytes constant TEST_PROOF = hex"52454d31deadbeefcafebabe"; // "REM1" prefix + data
    bytes constant TEST_PUBLIC_INPUTS = hex"0000000000000000000000000000000000000000000000000000000000000001";
    bytes constant TEST_GENS_DATA = hex"aabbccdd";

    function setUp() public {
        mockVerifier = new MockRemainderVerifier();
        adapter = new RemainderVerifierAdapter(address(mockVerifier));
    }

    // ========================================================================
    // Constructor tests
    // ========================================================================

    function test_constructor_setsVerifierAddress() public view {
        assertEq(address(adapter.remainderVerifier()), address(mockVerifier));
    }

    function test_constructor_withZeroAddress() public {
        // The adapter does not block zero address in constructor -- it just stores it.
        RemainderVerifierAdapter zeroAdapter = new RemainderVerifierAdapter(address(0));
        assertEq(address(zeroAdapter.remainderVerifier()), address(0));
    }

    // ========================================================================
    // proofSystem tests
    // ========================================================================

    function test_proofSystem_returnsRemainder() public view {
        assertEq(adapter.proofSystem(), "remainder");
    }

    // ========================================================================
    // verify: legacy/simple path (publicData.length < 32)
    // ========================================================================

    function test_verify_legacyPath_shortPublicData() public {
        // When publicData < 32 bytes, the adapter passes it directly as publicInputs
        // with empty gensData.
        bytes memory shortPublicData = hex"aabbccdd"; // 4 bytes < 32

        vm.expectCall(
            address(mockVerifier),
            abi.encodeCall(RemainderVerifier.verifyOrRevert, (TEST_PROOF, TEST_CIRCUIT_HASH, shortPublicData, ""))
        );

        adapter.verify(TEST_PROOF, TEST_CIRCUIT_HASH, shortPublicData);
    }

    function test_verify_legacyPath_emptyPublicData() public {
        vm.expectCall(
            address(mockVerifier),
            abi.encodeCall(RemainderVerifier.verifyOrRevert, (TEST_PROOF, TEST_CIRCUIT_HASH, hex"", ""))
        );

        adapter.verify(TEST_PROOF, TEST_CIRCUIT_HASH, hex"");
    }

    function test_verify_legacyPath_31Bytes() public {
        // Exactly 31 bytes -- still takes the legacy path
        bytes memory data31 = new bytes(31);
        for (uint256 i = 0; i < 31; i++) {
            // forge-lint: disable-next-line(unsafe-typecast)
            data31[i] = bytes1(uint8(i));
        }

        vm.expectCall(
            address(mockVerifier),
            abi.encodeCall(RemainderVerifier.verifyOrRevert, (TEST_PROOF, TEST_CIRCUIT_HASH, data31, ""))
        );

        adapter.verify(TEST_PROOF, TEST_CIRCUIT_HASH, data31);
    }

    // ========================================================================
    // verify: split path (publicData.length >= 32)
    // ========================================================================

    function test_verify_splitPath_pubInputsAndGens() public {
        // publicData = [pubInputsLen (32 bytes)] [pubInputs (pubInputsLen bytes)] [gensData (rest)]
        uint256 pubInputsLen = 32; // 32 bytes of public inputs
        bytes memory pubInputs = hex"0000000000000000000000000000000000000000000000000000000000000042";
        bytes memory gensData = hex"aabbccdd";

        // Encode: len prefix + pubInputs + gensData
        bytes memory publicData = abi.encodePacked(bytes32(pubInputsLen), pubInputs, gensData);

        vm.expectCall(
            address(mockVerifier),
            abi.encodeCall(RemainderVerifier.verifyOrRevert, (TEST_PROOF, TEST_CIRCUIT_HASH, pubInputs, gensData))
        );

        adapter.verify(TEST_PROOF, TEST_CIRCUIT_HASH, publicData);
    }

    function test_verify_splitPath_emptyGens() public {
        // pubInputsLen consumes all remaining data, leaving empty gensData
        uint256 pubInputsLen = 32;
        bytes memory pubInputs = hex"0000000000000000000000000000000000000000000000000000000000000001";

        bytes memory publicData = abi.encodePacked(bytes32(pubInputsLen), pubInputs);
        // gensData will be publicData[64:64] = empty

        vm.expectCall(
            address(mockVerifier),
            abi.encodeCall(RemainderVerifier.verifyOrRevert, (TEST_PROOF, TEST_CIRCUIT_HASH, pubInputs, hex""))
        );

        adapter.verify(TEST_PROOF, TEST_CIRCUIT_HASH, publicData);
    }

    function test_verify_splitPath_emptyPubInputs() public {
        // pubInputsLen = 0 means empty pubInputs, all remaining is gensData
        uint256 pubInputsLen = 0;
        bytes memory gensData = hex"deadbeef01020304";

        bytes memory publicData = abi.encodePacked(bytes32(pubInputsLen), gensData);

        vm.expectCall(
            address(mockVerifier),
            abi.encodeCall(RemainderVerifier.verifyOrRevert, (TEST_PROOF, TEST_CIRCUIT_HASH, hex"", gensData))
        );

        adapter.verify(TEST_PROOF, TEST_CIRCUIT_HASH, publicData);
    }

    function test_verify_splitPath_invalidLengthPrefixFallsBackToRaw() public {
        // When the length prefix exceeds available data, treat the entire publicData as raw pubInputs
        // pubInputsLen = 999 but publicData only has 40 bytes total (32 header + 8 data)
        bytes memory publicData = abi.encodePacked(bytes32(uint256(999)), hex"aabbccdd11223344");

        // Since pubInputsLen + 32 > publicData.length, falls back to raw path
        vm.expectCall(
            address(mockVerifier),
            abi.encodeCall(RemainderVerifier.verifyOrRevert, (TEST_PROOF, TEST_CIRCUIT_HASH, publicData, ""))
        );

        adapter.verify(TEST_PROOF, TEST_CIRCUIT_HASH, publicData);
    }

    function test_verify_splitPath_exactly32BytePublicData() public {
        // publicData is exactly 32 bytes -- the length prefix says 0 bytes of pubInputs
        bytes memory publicData = abi.encodePacked(bytes32(uint256(0)));
        // pubInputsLen=0, pubInputs=empty, gensData=empty

        vm.expectCall(
            address(mockVerifier),
            abi.encodeCall(RemainderVerifier.verifyOrRevert, (TEST_PROOF, TEST_CIRCUIT_HASH, hex"", hex""))
        );

        adapter.verify(TEST_PROOF, TEST_CIRCUIT_HASH, publicData);
    }

    // ========================================================================
    // verify: failure paths
    // ========================================================================

    function test_verify_revertsWhenUnderlyingReverts() public {
        mockVerifier.setShouldRevert(true);

        vm.expectRevert("MockRemainderVerifier: verification failed");
        adapter.verify(TEST_PROOF, TEST_CIRCUIT_HASH, hex"aabb");
    }

    function test_verify_revertsWhenAlwaysRevertVerifier() public {
        AlwaysRevertRemainderVerifier alwaysRevert = new AlwaysRevertRemainderVerifier();
        RemainderVerifierAdapter failAdapter = new RemainderVerifierAdapter(address(alwaysRevert));

        vm.expectRevert("AlwaysRevertRemainderVerifier: rejected");
        failAdapter.verify(TEST_PROOF, TEST_CIRCUIT_HASH, hex"aabb");
    }

    // ========================================================================
    // verify: edge cases
    // ========================================================================

    function test_verify_emptyProofData() public {
        // The adapter does not validate proof data -- it passes through to the verifier.
        vm.expectCall(
            address(mockVerifier),
            abi.encodeCall(RemainderVerifier.verifyOrRevert, (hex"", TEST_CIRCUIT_HASH, hex"", ""))
        );

        adapter.verify(hex"", TEST_CIRCUIT_HASH, hex"");
    }

    function test_verify_zeroCircuitHash() public {
        // The adapter does not validate circuitHash -- passes through to underlying verifier.
        vm.expectCall(
            address(mockVerifier), abi.encodeCall(RemainderVerifier.verifyOrRevert, (TEST_PROOF, bytes32(0), hex"", ""))
        );

        adapter.verify(TEST_PROOF, bytes32(0), hex"");
    }

    function test_verify_largePublicData() public {
        // Test with larger publicData to ensure the split logic works correctly
        uint256 pubInputsLen = 128;
        bytes memory pubInputs = new bytes(128);
        bytes memory gensData = new bytes(256);

        for (uint256 i = 0; i < 128; i++) {
            // forge-lint: disable-next-line(unsafe-typecast)
            pubInputs[i] = bytes1(uint8(i % 256));
        }
        for (uint256 i = 0; i < 256; i++) {
            // forge-lint: disable-next-line(unsafe-typecast)
            gensData[i] = bytes1(uint8((i + 128) % 256));
        }

        bytes memory publicData = abi.encodePacked(bytes32(pubInputsLen), pubInputs, gensData);

        vm.expectCall(
            address(mockVerifier),
            abi.encodeCall(RemainderVerifier.verifyOrRevert, (TEST_PROOF, TEST_CIRCUIT_HASH, pubInputs, gensData))
        );

        adapter.verify(TEST_PROOF, TEST_CIRCUIT_HASH, publicData);
    }

    function test_verify_passesProofDataThrough() public {
        // Ensure the exact proof bytes are forwarded
        bytes memory specificProof = hex"52454d31aaaabbbbccccdddd";

        vm.expectCall(
            address(mockVerifier),
            abi.encodeCall(RemainderVerifier.verifyOrRevert, (specificProof, TEST_CIRCUIT_HASH, hex"", ""))
        );

        adapter.verify(specificProof, TEST_CIRCUIT_HASH, hex"");
    }

    function test_verify_zeroAddressVerifierReverts() public {
        // Adapter with zero address verifier -- calling verify should revert (call to non-contract)
        RemainderVerifierAdapter zeroAdapter = new RemainderVerifierAdapter(address(0));

        vm.expectRevert();
        zeroAdapter.verify(TEST_PROOF, TEST_CIRCUIT_HASH, hex"aabb");
    }

    // ========================================================================
    // IProofVerifier interface compliance
    // ========================================================================

    function test_implementsIProofVerifier() public view {
        // Verify that the adapter satisfies the IProofVerifier interface
        // by calling both interface methods without reverting
        IProofVerifier verifier = IProofVerifier(address(adapter));

        // proofSystem() should return "remainder"
        string memory system = verifier.proofSystem();
        assertEq(system, "remainder");
    }

    function test_verify_throughInterface() public {
        // Call verify through the IProofVerifier interface
        IProofVerifier verifier = IProofVerifier(address(adapter));

        vm.expectCall(
            address(mockVerifier),
            abi.encodeCall(RemainderVerifier.verifyOrRevert, (TEST_PROOF, TEST_CIRCUIT_HASH, hex"", ""))
        );

        verifier.verify(TEST_PROOF, TEST_CIRCUIT_HASH, hex"");
    }
}
