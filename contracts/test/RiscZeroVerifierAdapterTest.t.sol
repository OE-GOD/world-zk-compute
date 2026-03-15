// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/RiscZeroVerifierAdapter.sol";
import "../src/RiscZeroVerifierRouter.sol";
import {IRiscZeroVerifier, Receipt as RiscZeroReceipt, VerificationFailed} from "risc0-ethereum/IRiscZeroVerifier.sol";

// ============================================================================
// MOCK CONTRACTS
// ============================================================================

/// @dev Configurable mock verifier: can be set to accept or reject proofs.
///      All functions are view-compatible (no state mutation) so they work
///      when called via STATICCALL from the view-only router.
contract ConfigurableMockVerifier is IRiscZeroVerifier {
    bool public shouldPass;

    constructor(bool _shouldPass) {
        shouldPass = _shouldPass;
    }

    function setShouldPass(bool _shouldPass) external {
        shouldPass = _shouldPass;
    }

    function verify(bytes calldata, bytes32, bytes32) external view override {
        if (!shouldPass) {
            revert("MockVerifier: verification failed");
        }
    }

    function verifyIntegrity(RiscZeroReceipt calldata) external view override {
        if (!shouldPass) {
            revert("MockVerifier: verification failed");
        }
    }
}

/// @dev A verifier that always reverts -- used to test router failure path.
contract AlwaysFailVerifier is IRiscZeroVerifier {
    function verify(bytes calldata, bytes32, bytes32) external pure override {
        revert("AlwaysFailVerifier: rejected");
    }

    function verifyIntegrity(RiscZeroReceipt calldata) external pure override {
        revert("AlwaysFailVerifier: rejected");
    }
}

/// @dev A pass-through verifier that always succeeds (view-compatible).
///      Used for routing tests where we use vm.expectCall to verify dispatch.
contract PassthroughVerifier is IRiscZeroVerifier {
    function verify(bytes calldata, bytes32, bytes32) external pure override {
        // always succeeds
    }

    function verifyIntegrity(RiscZeroReceipt calldata) external pure override {
        // always succeeds
    }
}

// ============================================================================
// ADAPTER TESTS
// ============================================================================

contract RiscZeroVerifierAdapterTest is Test {
    RiscZeroVerifierAdapter public adapter;
    ConfigurableMockVerifier public mockVerifier;

    bytes32 constant TEST_IMAGE_ID = bytes32(uint256(0x1234));
    bytes constant TEST_JOURNAL = hex"cafebabe";
    bytes constant TEST_SEAL = hex"73c457badeadbeefcafebabe";

    function setUp() public {
        mockVerifier = new ConfigurableMockVerifier(true);
        adapter = new RiscZeroVerifierAdapter(address(mockVerifier));
    }

    // --- Constructor ---

    function test_constructor_setsRouterAddress() public view {
        assertEq(address(adapter.riscZeroVerifier()), address(mockVerifier));
    }

    function test_constructor_withZeroAddress() public {
        // The adapter does not block zero address in constructor -- it just stores it.
        RiscZeroVerifierAdapter zeroAdapter = new RiscZeroVerifierAdapter(address(0));
        assertEq(address(zeroAdapter.riscZeroVerifier()), address(0));
    }

    // --- proofSystem ---

    function test_proofSystem_returnsRisc0() public view {
        assertEq(adapter.proofSystem(), "risc0");
    }

    // --- verify: happy path ---

    function test_verify_callsUnderlyingVerifier() public view {
        // Should not revert when the mock is configured to pass
        adapter.verify(TEST_SEAL, TEST_IMAGE_ID, TEST_JOURNAL);
    }

    function test_verify_computesSha256OfJournal() public {
        // The adapter computes sha256(publicData) as the journalDigest.
        // Verify the correct journalDigest is passed to the underlying verifier
        // by using vm.expectCall.
        bytes memory journal = hex"deadbeef";
        bytes32 expectedDigest = sha256(journal);

        vm.expectCall(
            address(mockVerifier), abi.encodeCall(IRiscZeroVerifier.verify, (TEST_SEAL, TEST_IMAGE_ID, expectedDigest))
        );

        adapter.verify(TEST_SEAL, TEST_IMAGE_ID, journal);
    }

    function test_verify_passesImageIdThrough() public {
        bytes32 specificImageId = bytes32(uint256(0xDEAD));

        vm.expectCall(
            address(mockVerifier),
            abi.encodeCall(IRiscZeroVerifier.verify, (TEST_SEAL, specificImageId, sha256(TEST_JOURNAL)))
        );

        adapter.verify(TEST_SEAL, specificImageId, TEST_JOURNAL);
    }

    function test_verify_passesProofDataAsSeal() public {
        bytes memory seal = hex"aabbccdd11223344";

        vm.expectCall(
            address(mockVerifier), abi.encodeCall(IRiscZeroVerifier.verify, (seal, TEST_IMAGE_ID, sha256(TEST_JOURNAL)))
        );

        adapter.verify(seal, TEST_IMAGE_ID, TEST_JOURNAL);
    }

    // --- verify: failure path ---

    function test_verify_revertsWhenUnderlyingRejects() public {
        mockVerifier.setShouldPass(false);

        vm.expectRevert("MockVerifier: verification failed");
        adapter.verify(TEST_SEAL, TEST_IMAGE_ID, TEST_JOURNAL);
    }

    function test_verify_succeedsWithEmptyJournal() public view {
        // Empty journal is valid input -- sha256("") is a well-defined hash.
        adapter.verify(TEST_SEAL, TEST_IMAGE_ID, hex"");
    }

    // --- verify: edge cases ---

    function test_verify_emptyProofData() public view {
        // The adapter does not validate seal length -- that is the router/verifier's job.
        adapter.verify(hex"", TEST_IMAGE_ID, TEST_JOURNAL);
    }

    function test_verify_zeroImageId() public view {
        // The adapter does not validate imageId -- passes through to underlying verifier.
        adapter.verify(TEST_SEAL, bytes32(0), TEST_JOURNAL);
    }

    function test_verify_largeJournal() public view {
        // Test with a larger journal to ensure sha256 works on bigger data
        bytes memory largeJournal = new bytes(1024);
        for (uint256 i = 0; i < 1024; i++) {
            // forge-lint: disable-next-line(unsafe-typecast)
            largeJournal[i] = bytes1(uint8(i % 256));
        }
        adapter.verify(TEST_SEAL, TEST_IMAGE_ID, largeJournal);
    }

    function test_verify_sha256DigestIsCorrect() public {
        // Verify the exact sha256 digest computed for a known input
        bytes memory journal = hex"01020304";
        bytes32 expectedDigest = sha256(journal);

        // Manually compute what we expect
        assertEq(expectedDigest, sha256(hex"01020304"), "sha256 digest should be deterministic");

        vm.expectCall(
            address(mockVerifier), abi.encodeCall(IRiscZeroVerifier.verify, (TEST_SEAL, TEST_IMAGE_ID, expectedDigest))
        );

        adapter.verify(TEST_SEAL, TEST_IMAGE_ID, journal);
    }
}

// ============================================================================
// ROUTER TESTS
// ============================================================================

contract RiscZeroVerifierRouterTest is Test {
    RiscZeroVerifierRouter public router;
    ConfigurableMockVerifier public verifierA;
    ConfigurableMockVerifier public verifierB;

    address public admin = address(this);
    address public nonAdmin = address(0xBEEF);

    // forge-lint: disable-next-line(unsafe-typecast)
    bytes4 constant SELECTOR_A = bytes4(hex"73c457ba"); // risc0 v3.0 selector
    // forge-lint: disable-next-line(unsafe-typecast)
    bytes4 constant SELECTOR_B = bytes4(hex"c101b42b"); // risc0 v1.2 selector

    bytes32 constant TEST_IMAGE_ID = bytes32(uint256(0xABCD));
    bytes32 constant TEST_JOURNAL_DIGEST = bytes32(uint256(0x1111));

    // Re-declare events for expectEmit
    event VerifierAdded(bytes4 indexed selector, address verifier, string name);
    event VerifierRemoved(bytes4 indexed selector);
    event DefaultVerifierSet(address verifier);
    event AdminTransferred(address indexed oldAdmin, address indexed newAdmin);

    function setUp() public {
        router = new RiscZeroVerifierRouter(admin);
        verifierA = new ConfigurableMockVerifier(true);
        verifierB = new ConfigurableMockVerifier(true);
    }

    // --- Constructor ---

    function test_constructor_setsAdmin() public view {
        assertEq(router.admin(), admin);
    }

    function test_constructor_noSelectorsInitially() public view {
        bytes4[] memory sels = router.getSelectors();
        assertEq(sels.length, 0);
    }

    function test_constructor_noDefaultVerifier() public view {
        assertEq(router.defaultVerifier(), address(0));
    }

    // --- addVerifier ---

    function test_addVerifier_registersNewVerifier() public {
        router.addVerifier(SELECTOR_A, address(verifierA), "Groth16 v3.0");

        (address v, string memory name, bool active) = router.verifiers(SELECTOR_A);
        assertEq(v, address(verifierA));
        assertEq(name, "Groth16 v3.0");
        assertTrue(active);
    }

    function test_addVerifier_appendsToSelectors() public {
        router.addVerifier(SELECTOR_A, address(verifierA), "Verifier A");
        router.addVerifier(SELECTOR_B, address(verifierB), "Verifier B");

        bytes4[] memory sels = router.getSelectors();
        assertEq(sels.length, 2);
        assertEq(sels[0], SELECTOR_A);
        assertEq(sels[1], SELECTOR_B);
    }

    function test_addVerifier_emitsEvent() public {
        vm.expectEmit(true, false, false, true);
        emit VerifierAdded(SELECTOR_A, address(verifierA), "Groth16 v3.0");

        router.addVerifier(SELECTOR_A, address(verifierA), "Groth16 v3.0");
    }

    function test_addVerifier_revertsForNonAdmin() public {
        vm.prank(nonAdmin);
        vm.expectRevert(RiscZeroVerifierRouter.NotAdmin.selector);
        router.addVerifier(SELECTOR_A, address(verifierA), "Groth16 v3.0");
    }

    function test_addVerifier_allowsZeroAddress() public {
        // The contract does not explicitly block zero-address verifiers.
        router.addVerifier(SELECTOR_A, address(0), "Null verifier");

        (address v,, bool active) = router.verifiers(SELECTOR_A);
        assertEq(v, address(0));
        assertTrue(active);
    }

    function test_addVerifier_overwritesDuplicate() public {
        router.addVerifier(SELECTOR_A, address(verifierA), "Original");
        router.addVerifier(SELECTOR_A, address(verifierB), "Replacement");

        (address v, string memory name,) = router.verifiers(SELECTOR_A);
        assertEq(v, address(verifierB));
        assertEq(name, "Replacement");

        // Note: selectors array will have duplicates (by design -- no dedup)
        bytes4[] memory sels = router.getSelectors();
        assertEq(sels.length, 2);
    }

    // --- removeVerifier ---

    function test_removeVerifier_deactivates() public {
        router.addVerifier(SELECTOR_A, address(verifierA), "Test");
        router.removeVerifier(SELECTOR_A);

        (address v,, bool active) = router.verifiers(SELECTOR_A);
        assertEq(v, address(verifierA)); // Address still stored
        assertFalse(active); // But inactive
    }

    function test_removeVerifier_emitsEvent() public {
        router.addVerifier(SELECTOR_A, address(verifierA), "Test");

        vm.expectEmit(true, false, false, false);
        emit VerifierRemoved(SELECTOR_A);

        router.removeVerifier(SELECTOR_A);
    }

    function test_removeVerifier_revertsForNonAdmin() public {
        vm.prank(nonAdmin);
        vm.expectRevert(RiscZeroVerifierRouter.NotAdmin.selector);
        router.removeVerifier(SELECTOR_A);
    }

    // --- setDefaultVerifier ---

    function test_setDefaultVerifier_setsAddress() public {
        router.setDefaultVerifier(address(verifierA));
        assertEq(router.defaultVerifier(), address(verifierA));
    }

    function test_setDefaultVerifier_emitsEvent() public {
        vm.expectEmit(false, false, false, true);
        emit DefaultVerifierSet(address(verifierA));

        router.setDefaultVerifier(address(verifierA));
    }

    function test_setDefaultVerifier_revertsForNonAdmin() public {
        vm.prank(nonAdmin);
        vm.expectRevert(RiscZeroVerifierRouter.NotAdmin.selector);
        router.setDefaultVerifier(address(verifierA));
    }

    function test_setDefaultVerifier_allowsZeroAddress() public {
        router.setDefaultVerifier(address(verifierA));
        router.setDefaultVerifier(address(0)); // Remove default
        assertEq(router.defaultVerifier(), address(0));
    }

    // --- transferAdmin ---

    function test_transferAdmin_changesAdmin() public {
        address newAdmin = address(0xABCD);

        vm.expectEmit(true, true, false, false);
        emit AdminTransferred(admin, newAdmin);

        router.transferAdmin(newAdmin);
        assertEq(router.admin(), newAdmin);
    }

    function test_transferAdmin_revertsForNonAdmin() public {
        vm.prank(nonAdmin);
        vm.expectRevert(RiscZeroVerifierRouter.NotAdmin.selector);
        router.transferAdmin(address(0x1234));
    }

    function test_transferAdmin_oldAdminLosesAccess() public {
        address newAdmin = address(0xABCD);
        router.transferAdmin(newAdmin);

        // Old admin can no longer add verifiers
        vm.expectRevert(RiscZeroVerifierRouter.NotAdmin.selector);
        router.addVerifier(SELECTOR_A, address(verifierA), "Test");

        // New admin can
        vm.prank(newAdmin);
        router.addVerifier(SELECTOR_A, address(verifierA), "Test");
    }

    // --- verify: routing ---

    function test_verify_routesToCorrectVerifier() public {
        PassthroughVerifier ptA = new PassthroughVerifier();
        PassthroughVerifier ptB = new PassthroughVerifier();

        router.addVerifier(SELECTOR_A, address(ptA), "Verifier A");
        router.addVerifier(SELECTOR_B, address(ptB), "Verifier B");

        // Build a seal starting with SELECTOR_A
        bytes memory sealA = abi.encodePacked(SELECTOR_A, hex"deadbeef");

        // Expect call to verifier A with the full seal, imageId, journalDigest
        vm.expectCall(
            address(ptA), abi.encodeCall(IRiscZeroVerifier.verify, (sealA, TEST_IMAGE_ID, TEST_JOURNAL_DIGEST))
        );

        router.verify(sealA, TEST_IMAGE_ID, TEST_JOURNAL_DIGEST);
    }

    function test_verify_routesToSecondVerifier() public {
        PassthroughVerifier ptA = new PassthroughVerifier();
        PassthroughVerifier ptB = new PassthroughVerifier();

        router.addVerifier(SELECTOR_A, address(ptA), "Verifier A");
        router.addVerifier(SELECTOR_B, address(ptB), "Verifier B");

        bytes memory sealB = abi.encodePacked(SELECTOR_B, hex"cafebabe");

        vm.expectCall(
            address(ptB), abi.encodeCall(IRiscZeroVerifier.verify, (sealB, TEST_IMAGE_ID, TEST_JOURNAL_DIGEST))
        );

        router.verify(sealB, TEST_IMAGE_ID, TEST_JOURNAL_DIGEST);
    }

    function test_verify_revertsForUnknownSelector() public {
        router.addVerifier(SELECTOR_A, address(verifierA), "Test");

        // forge-lint: disable-next-line(unsafe-typecast)
        bytes4 unknownSelector = bytes4(hex"ffffffff");
        bytes memory seal = abi.encodePacked(unknownSelector, hex"deadbeef");

        vm.expectRevert(RiscZeroVerifierRouter.NoVerifierFound.selector);
        router.verify(seal, TEST_IMAGE_ID, TEST_JOURNAL_DIGEST);
    }

    function test_verify_revertsForSealTooShort() public {
        vm.expectRevert(RiscZeroVerifierRouter.InvalidSeal.selector);
        router.verify(hex"aabb", TEST_IMAGE_ID, TEST_JOURNAL_DIGEST);
    }

    function test_verify_revertsForEmptySeal() public {
        vm.expectRevert(RiscZeroVerifierRouter.InvalidSeal.selector);
        router.verify(hex"", TEST_IMAGE_ID, TEST_JOURNAL_DIGEST);
    }

    function test_verify_revertsFor3ByteSeal() public {
        vm.expectRevert(RiscZeroVerifierRouter.InvalidSeal.selector);
        router.verify(hex"aabbcc", TEST_IMAGE_ID, TEST_JOURNAL_DIGEST);
    }

    function test_verify_succeedsWithExactly4ByteSeal() public {
        // 4 bytes is the minimum valid seal length (just the selector)
        router.addVerifier(SELECTOR_A, address(verifierA), "Test");
        // Seal is exactly the selector with no additional bytes
        router.verify(abi.encodePacked(SELECTOR_A), TEST_IMAGE_ID, TEST_JOURNAL_DIGEST);
    }

    function test_verify_revertsForInactiveVerifier() public {
        router.addVerifier(SELECTOR_A, address(verifierA), "Test");
        router.removeVerifier(SELECTOR_A);

        bytes memory seal = abi.encodePacked(SELECTOR_A, hex"deadbeef");

        vm.expectRevert(RiscZeroVerifierRouter.VerifierNotActive.selector);
        router.verify(seal, TEST_IMAGE_ID, TEST_JOURNAL_DIGEST);
    }

    function test_verify_fallsBackToDefault() public {
        PassthroughVerifier defaultV = new PassthroughVerifier();
        router.setDefaultVerifier(address(defaultV));

        // Use a selector with no registered verifier
        // forge-lint: disable-next-line(unsafe-typecast)
        bytes4 unknownSelector = bytes4(hex"aaaaaaaa");
        bytes memory seal = abi.encodePacked(unknownSelector, hex"deadbeef");

        vm.expectCall(
            address(defaultV), abi.encodeCall(IRiscZeroVerifier.verify, (seal, TEST_IMAGE_ID, TEST_JOURNAL_DIGEST))
        );

        router.verify(seal, TEST_IMAGE_ID, TEST_JOURNAL_DIGEST);
    }

    function test_verify_inactiveVerifierDoesNotFallBackToDefault() public {
        PassthroughVerifier defaultV = new PassthroughVerifier();
        router.setDefaultVerifier(address(defaultV));
        router.addVerifier(SELECTOR_A, address(verifierA), "Test");
        router.removeVerifier(SELECTOR_A);

        bytes memory seal = abi.encodePacked(SELECTOR_A, hex"deadbeef");

        // The logic: info.verifier != 0 so verifier = info.verifier (not default).
        // Then !info.active && verifier != defaultVerifier -> revert VerifierNotActive.
        // Inactive verifiers do NOT fall back to default.
        vm.expectRevert(RiscZeroVerifierRouter.VerifierNotActive.selector);
        router.verify(seal, TEST_IMAGE_ID, TEST_JOURNAL_DIGEST);
    }

    function test_verify_wrapsUnderlyingRevertAsVerificationFailed() public {
        AlwaysFailVerifier failVerifier = new AlwaysFailVerifier();
        router.addVerifier(SELECTOR_A, address(failVerifier), "Fail");

        bytes memory seal = abi.encodePacked(SELECTOR_A, hex"deadbeef");

        vm.expectRevert(VerificationFailed.selector);
        router.verify(seal, TEST_IMAGE_ID, TEST_JOURNAL_DIGEST);
    }

    // --- verifyIntegrity ---

    function test_verifyIntegrity_routesToCorrectVerifier() public {
        PassthroughVerifier pt = new PassthroughVerifier();
        router.addVerifier(SELECTOR_A, address(pt), "Verifier A");

        bytes memory seal = abi.encodePacked(SELECTOR_A, hex"deadbeef");

        // Use abi.encodeWithSelector since RiscZeroReceipt is a struct param
        vm.expectCall(
            address(pt),
            0,
            abi.encodeWithSelector(
                IRiscZeroVerifier.verifyIntegrity.selector,
                RiscZeroReceipt({seal: seal, claimDigest: bytes32(uint256(42))})
            )
        );

        router.verifyIntegrity(RiscZeroReceipt({seal: seal, claimDigest: bytes32(uint256(42))}));
    }

    function test_verifyIntegrity_revertsForSealTooShort() public {
        vm.expectRevert(RiscZeroVerifierRouter.InvalidSeal.selector);
        router.verifyIntegrity(RiscZeroReceipt({seal: hex"aabb", claimDigest: bytes32(uint256(42))}));
    }

    function test_verifyIntegrity_revertsForNoVerifier() public {
        // forge-lint: disable-next-line(unsafe-typecast)
        bytes memory seal = abi.encodePacked(bytes4(hex"ffffffff"), hex"deadbeef");

        vm.expectRevert(RiscZeroVerifierRouter.NoVerifierFound.selector);
        router.verifyIntegrity(RiscZeroReceipt({seal: seal, claimDigest: bytes32(uint256(42))}));
    }

    function test_verifyIntegrity_revertsForInactiveVerifier() public {
        router.addVerifier(SELECTOR_A, address(verifierA), "Test");
        router.removeVerifier(SELECTOR_A);

        bytes memory seal = abi.encodePacked(SELECTOR_A, hex"deadbeef");

        vm.expectRevert(RiscZeroVerifierRouter.VerifierNotActive.selector);
        router.verifyIntegrity(RiscZeroReceipt({seal: seal, claimDigest: bytes32(uint256(42))}));
    }

    // --- hasVerifier ---

    function test_hasVerifier_trueForActive() public {
        router.addVerifier(SELECTOR_A, address(verifierA), "Test");
        assertTrue(router.hasVerifier(SELECTOR_A));
    }

    function test_hasVerifier_falseForInactiveNoDefault() public {
        router.addVerifier(SELECTOR_A, address(verifierA), "Test");
        router.removeVerifier(SELECTOR_A);
        assertFalse(router.hasVerifier(SELECTOR_A));
    }

    function test_hasVerifier_trueWhenDefaultSet() public {
        // Any selector returns true when a default verifier is set
        router.setDefaultVerifier(address(verifierA));
        // forge-lint: disable-next-line(unsafe-typecast)
        assertTrue(router.hasVerifier(bytes4(hex"ffffffff")));
    }

    function test_hasVerifier_falseForUnknownNoDefault() public view {
        // forge-lint: disable-next-line(unsafe-typecast)
        assertFalse(router.hasVerifier(bytes4(hex"ffffffff")));
    }

    // --- getSelectors ---

    function test_getSelectors_empty() public view {
        bytes4[] memory sels = router.getSelectors();
        assertEq(sels.length, 0);
    }

    function test_getSelectors_returnsAll() public {
        router.addVerifier(SELECTOR_A, address(verifierA), "A");
        router.addVerifier(SELECTOR_B, address(verifierB), "B");

        bytes4[] memory sels = router.getSelectors();
        assertEq(sels.length, 2);
        assertEq(sels[0], SELECTOR_A);
        assertEq(sels[1], SELECTOR_B);
    }

    // --- selectors(uint256) public getter ---

    function test_selectors_publicGetter() public {
        router.addVerifier(SELECTOR_A, address(verifierA), "A");
        assertEq(router.selectors(0), SELECTOR_A);
    }
}

// ============================================================================
// INTEGRATION TESTS: Adapter -> Router -> Verifier chain
// ============================================================================

contract RiscZeroIntegrationTest is Test {
    RiscZeroVerifierAdapter public adapter;
    RiscZeroVerifierRouter public router;
    PassthroughVerifier public backendVerifier;

    address public admin = address(this);
    // forge-lint: disable-next-line(unsafe-typecast)
    bytes4 constant SELECTOR = bytes4(hex"73c457ba");
    bytes32 constant IMAGE_ID = bytes32(uint256(0xCAFE));

    function setUp() public {
        // Set up the full chain: Adapter -> Router -> BackendVerifier
        router = new RiscZeroVerifierRouter(admin);
        backendVerifier = new PassthroughVerifier();
        router.addVerifier(SELECTOR, address(backendVerifier), "Backend Groth16");

        adapter = new RiscZeroVerifierAdapter(address(router));
    }

    function test_fullChain_adapterToRouterToVerifier() public {
        // Build proof data with the selector prefix
        bytes memory proofData = abi.encodePacked(SELECTOR, hex"aabbccdd");
        bytes memory journal = hex"deadbeef01020304";
        bytes32 expectedDigest = sha256(journal);

        // Expect the backend verifier to receive the call with correct args
        vm.expectCall(
            address(backendVerifier), abi.encodeCall(IRiscZeroVerifier.verify, (proofData, IMAGE_ID, expectedDigest))
        );

        // Call through adapter
        adapter.verify(proofData, IMAGE_ID, journal);
    }

    function test_fullChain_failingBackendRevertsAdapter() public {
        // Replace backend with failing verifier
        AlwaysFailVerifier failV = new AlwaysFailVerifier();
        router.addVerifier(SELECTOR, address(failV), "Fail");

        bytes memory proofData = abi.encodePacked(SELECTOR, hex"aabbccdd");
        bytes memory journal = hex"deadbeef";

        // The router wraps the error as VerificationFailed
        vm.expectRevert(VerificationFailed.selector);
        adapter.verify(proofData, IMAGE_ID, journal);
    }

    function test_fullChain_unknownSelectorReverts() public {
        // forge-lint: disable-next-line(unsafe-typecast)
        bytes4 unknownSelector = bytes4(hex"ffffffff");
        bytes memory proofData = abi.encodePacked(unknownSelector, hex"aabbccdd");
        bytes memory journal = hex"deadbeef";

        vm.expectRevert(RiscZeroVerifierRouter.NoVerifierFound.selector);
        adapter.verify(proofData, IMAGE_ID, journal);
    }

    function test_fullChain_multipleVerifiers() public {
        // Register a second verifier with a different selector
        // forge-lint: disable-next-line(unsafe-typecast)
        bytes4 selectorV2 = bytes4(hex"c101b42b");
        PassthroughVerifier backendV2 = new PassthroughVerifier();
        router.addVerifier(selectorV2, address(backendV2), "Backend STARK");

        // Call with selector A -- expect call goes to backendVerifier
        bytes memory proofA = abi.encodePacked(SELECTOR, hex"1111");
        vm.expectCall(
            address(backendVerifier), abi.encodeCall(IRiscZeroVerifier.verify, (proofA, IMAGE_ID, sha256(hex"aaaa")))
        );
        adapter.verify(proofA, IMAGE_ID, hex"aaaa");

        // Call with selector B -- expect call goes to backendV2
        bytes memory proofB = abi.encodePacked(selectorV2, hex"2222");
        vm.expectCall(
            address(backendV2), abi.encodeCall(IRiscZeroVerifier.verify, (proofB, IMAGE_ID, sha256(hex"bbbb")))
        );
        adapter.verify(proofB, IMAGE_ID, hex"bbbb");
    }

    function test_fullChain_defaultVerifierFallback() public {
        PassthroughVerifier defaultV = new PassthroughVerifier();
        router.setDefaultVerifier(address(defaultV));

        // Use a selector with no registered verifier
        // forge-lint: disable-next-line(unsafe-typecast)
        bytes4 randomSelector = bytes4(hex"99887766");
        bytes memory proofData = abi.encodePacked(randomSelector, hex"aabbccdd");
        bytes memory journal = hex"cafe";

        vm.expectCall(
            address(defaultV), abi.encodeCall(IRiscZeroVerifier.verify, (proofData, IMAGE_ID, sha256(journal)))
        );

        adapter.verify(proofData, IMAGE_ID, journal);
    }

    function test_fullChain_verifyIntegrityThroughAdapter() public {
        // The adapter only exposes verify(), not verifyIntegrity().
        // But we can test the router's verifyIntegrity directly through the full chain.
        bytes memory seal = abi.encodePacked(SELECTOR, hex"aabbccdd");
        bytes32 claimDigest = bytes32(uint256(0x999));

        vm.expectCall(
            address(backendVerifier),
            0,
            abi.encodeWithSelector(
                IRiscZeroVerifier.verifyIntegrity.selector, RiscZeroReceipt({seal: seal, claimDigest: claimDigest})
            )
        );

        router.verifyIntegrity(RiscZeroReceipt({seal: seal, claimDigest: claimDigest}));
    }
}
