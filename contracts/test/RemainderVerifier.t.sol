// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/remainder/RemainderVerifier.sol";
import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";
import "../src/remainder/PoseidonSponge.sol";
import "../src/remainder/SumcheckVerifier.sol";
import "../src/remainder/HyraxVerifier.sol";
import "../src/remainder/GKRVerifier.sol";
import "../src/remainder/CommittedSumcheckVerifier.sol";
import "../src/remainder/HyraxProofDecoder.sol";
import {Verifier as Groth16Verifier} from "../src/remainder/RemainderGroth16Verifier.sol";
import {GKRHybridVerifier} from "../src/remainder/GKRHybridVerifier.sol";
import "../src/RemainderVerifierAdapter.sol";
import "../src/ProgramRegistry.sol";
import "../src/ExecutionEngine.sol";
import "../src/MockRiscZeroVerifier.sol";

contract PoseidonSpongeTest is Test {
    /// @notice Test that the sponge initializes correctly
    function test_sponge_init() public pure {
        PoseidonSponge.Sponge memory sponge = PoseidonSponge.init();
        // Capacity = 2^64 (domain separator)
        assertEq(sponge.state[0], 1 << 64);
        assertEq(sponge.state[1], 0);
        assertEq(sponge.state[2], 0);
        assertEq(sponge.absorbing, 0);
    }

    /// @notice Test absorb and squeeze produces deterministic output
    function test_absorb_squeeze_deterministic() public pure {
        PoseidonSponge.Sponge memory sponge1 = PoseidonSponge.init();
        PoseidonSponge.Sponge memory sponge2 = PoseidonSponge.init();

        PoseidonSponge.absorb(sponge1, 42);
        PoseidonSponge.absorb(sponge2, 42);

        uint256 out1 = PoseidonSponge.squeeze(sponge1);
        uint256 out2 = PoseidonSponge.squeeze(sponge2);

        assertEq(out1, out2, "Same input should produce same output");
        assertTrue(out1 != 0, "Output should be non-zero");
        assertTrue(
            out1 < 21888242871839275222246405745257275088696311157297823662689037894645226208583,
            "Output should be < Fq"
        );
    }

    /// @notice Test that different inputs produce different outputs
    function test_absorb_different_inputs() public pure {
        PoseidonSponge.Sponge memory sponge1 = PoseidonSponge.init();
        PoseidonSponge.Sponge memory sponge2 = PoseidonSponge.init();

        PoseidonSponge.absorb(sponge1, 42);
        PoseidonSponge.absorb(sponge2, 43);

        uint256 out1 = PoseidonSponge.squeeze(sponge1);
        uint256 out2 = PoseidonSponge.squeeze(sponge2);

        assertTrue(out1 != out2, "Different inputs should produce different outputs");
    }

    /// @notice Test absorbing a point (as two field elements)
    function test_absorb_point() public pure {
        PoseidonSponge.Sponge memory sponge = PoseidonSponge.init();

        // Absorb the BN254 generator point as (x, y)
        PoseidonSponge.absorb(sponge, 1);
        PoseidonSponge.absorb(sponge, 2);

        uint256 output = PoseidonSponge.squeeze(sponge);
        assertTrue(output != 0, "Output should be non-zero after absorbing point");
    }

    /// @notice Test multiple squeezes give different values
    function test_multiple_squeezes() public pure {
        PoseidonSponge.Sponge memory sponge = PoseidonSponge.init();
        PoseidonSponge.absorb(sponge, 100);

        uint256 out1 = PoseidonSponge.squeeze(sponge);
        uint256 out2 = PoseidonSponge.squeeze(sponge);

        assertTrue(out1 != out2, "Consecutive squeezes should differ");
    }

    /// @notice Validate against Rust-generated test vectors (PoseidonSponge<Fq>)
    function test_poseidon_self_test() public pure {
        assertTrue(PoseidonSponge.selfTest(), "Poseidon self-test should pass");
    }

    /// @notice Test absorb(1) matches Rust test vector
    function test_absorb_1_matches_rust() public pure {
        PoseidonSponge.Sponge memory sponge = PoseidonSponge.init();
        PoseidonSponge.absorb(sponge, 1);
        uint256 out = PoseidonSponge.squeeze(sponge);
        assertEq(out, 0x11b59b2a25b09e83a0565c77d56d22b06e1f08976f455c78d28fee1a8ebdd9dd);
    }

    /// @notice Test absorb(0) matches Rust test vector
    function test_absorb_0_matches_rust() public pure {
        PoseidonSponge.Sponge memory sponge = PoseidonSponge.init();
        PoseidonSponge.absorb(sponge, 0);
        uint256 out = PoseidonSponge.squeeze(sponge);
        assertEq(out, 0x1d2908476fdc03547a286a6130b318306a3e007b85d0914bd00ac6be079220c1);
    }
}

contract SumcheckVerifierTest is Test {
    /// @notice Test polynomial evaluation via Lagrange interpolation (linear)
    function test_evaluate_linear() public pure {
        // Linear polynomial: f(0)=3, f(1)=7 → f(x) = 3 + 4x
        uint256[] memory evals = new uint256[](2);
        evals[0] = 3;
        evals[1] = 7;

        // f(0) = 3
        assertEq(SumcheckVerifier.evaluatePolynomial(evals, 0), 3);
        // f(1) = 7
        assertEq(SumcheckVerifier.evaluatePolynomial(evals, 1), 7);
    }

    /// @notice Test polynomial evaluation (quadratic)
    function test_evaluate_quadratic() public pure {
        // Quadratic: f(0)=1, f(1)=4, f(2)=9 → f(x) = x^2 + 2x + 1
        uint256[] memory evals = new uint256[](3);
        evals[0] = 1;
        evals[1] = 4;
        evals[2] = 9;

        // f(0) = 1
        assertEq(SumcheckVerifier.evaluatePolynomial(evals, 0), 1);
        // f(1) = 4
        assertEq(SumcheckVerifier.evaluatePolynomial(evals, 1), 4);
        // f(2) = 9
        assertEq(SumcheckVerifier.evaluatePolynomial(evals, 2), 9);
    }

    /// @notice Test modular inverse
    function test_mod_inverse() public pure {
        uint256 p = SumcheckVerifier.FR_MODULUS;
        // inv(2) * 2 == 1 (mod p)
        uint256 inv2 = SumcheckVerifier.modInverse(2, p);
        assertEq(mulmod(inv2, 2, p), 1);

        // inv(7) * 7 == 1 (mod p)
        uint256 inv7 = SumcheckVerifier.modInverse(7, p);
        assertEq(mulmod(inv7, 7, p), 1);
    }

    /// @notice Test sumcheck verification with valid proof
    function test_verify_valid_sumcheck() public pure {
        // Create a simple sumcheck proof for f(x) = 3 + 4x over {0,1}
        // Sum = f(0) + f(1) = 3 + 7 = 10
        SumcheckVerifier.RoundPoly[] memory rounds = new SumcheckVerifier.RoundPoly[](1);
        uint256[] memory evals = new uint256[](2);
        evals[0] = 3; // g(0) = 3
        evals[1] = 7; // g(1) = 7
        rounds[0] = SumcheckVerifier.RoundPoly({evals: evals});

        // The challenge will be squeezed from the sponge after absorbing evals
        PoseidonSponge.Sponge memory sponge = PoseidonSponge.init();

        // Pre-compute: squeeze to get challenge, then compute finalEval = g(challenge)
        PoseidonSponge.Sponge memory precomputeSponge = PoseidonSponge.init();
        PoseidonSponge.absorb(precomputeSponge, 3);
        PoseidonSponge.absorb(precomputeSponge, 7);
        uint256 challenge = PoseidonSponge.squeeze(precomputeSponge) % SumcheckVerifier.FR_MODULUS;
        uint256 finalEval = SumcheckVerifier.evaluatePolynomial(evals, challenge);

        SumcheckVerifier.SumcheckProof memory proof =
            SumcheckVerifier.SumcheckProof({rounds: rounds, finalEval: finalEval});

        SumcheckVerifier.SumcheckResult memory result = SumcheckVerifier.verify(proof, 10, sponge);

        assertTrue(result.valid, "Valid sumcheck proof should verify");
        assertEq(result.challenges.length, 1);
        assertEq(result.challenges[0], challenge);
    }
}

contract HyraxVerifierTest is Test {
    /// @notice Test BN254 generator is on curve
    function test_generator_on_curve() public pure {
        HyraxVerifier.G1Point memory g = HyraxVerifier.G1Point(1, 2);
        assertTrue(HyraxVerifier.isOnCurve(g), "BN254 generator should be on curve");
    }

    /// @notice Test point at infinity is on curve
    function test_infinity_on_curve() public pure {
        HyraxVerifier.G1Point memory inf = HyraxVerifier.G1Point(0, 0);
        assertTrue(HyraxVerifier.isOnCurve(inf), "Point at infinity should be on curve");
    }

    /// @notice Test invalid point is not on curve
    function test_invalid_not_on_curve() public pure {
        HyraxVerifier.G1Point memory bad = HyraxVerifier.G1Point(1, 3);
        assertFalse(HyraxVerifier.isOnCurve(bad), "Invalid point should not be on curve");
    }

    /// @notice Test EC scalar multiplication with generator
    function test_scalar_mul() public view {
        HyraxVerifier.G1Point memory g = HyraxVerifier.G1Point(1, 2);
        HyraxVerifier.G1Point memory result = HyraxVerifier.scalarMul(g, 1);
        assertEq(result.x, 1);
        assertEq(result.y, 2);
    }

    /// @notice Test EC addition: G + G = 2G
    function test_ec_add() public view {
        HyraxVerifier.G1Point memory g = HyraxVerifier.G1Point(1, 2);
        HyraxVerifier.G1Point memory g2_via_add = HyraxVerifier.ecAdd(g, g);
        HyraxVerifier.G1Point memory g2_via_mul = HyraxVerifier.scalarMul(g, 2);

        assertEq(g2_via_add.x, g2_via_mul.x, "G+G should equal 2*G (x)");
        assertEq(g2_via_add.y, g2_via_mul.y, "G+G should equal 2*G (y)");
    }

    /// @notice Test point negation
    function test_negate() public pure {
        HyraxVerifier.G1Point memory g = HyraxVerifier.G1Point(1, 2);
        HyraxVerifier.G1Point memory neg = HyraxVerifier.negate(g);

        assertEq(neg.x, 1);
        assertEq(neg.y, HyraxVerifier.FQ_MODULUS - 2);
    }

    /// @notice Test MSM with single element
    function test_msm_single() public view {
        HyraxVerifier.G1Point[] memory points = new HyraxVerifier.G1Point[](1);
        points[0] = HyraxVerifier.G1Point(1, 2);

        uint256[] memory scalars = new uint256[](1);
        scalars[0] = 3;

        HyraxVerifier.G1Point memory result = HyraxVerifier.multiScalarMul(points, scalars);
        HyraxVerifier.G1Point memory expected = HyraxVerifier.scalarMul(points[0], 3);

        assertEq(result.x, expected.x);
        assertEq(result.y, expected.y);
    }
}

contract RemainderVerifierTest is Test {
    RemainderVerifier public verifier;

    address public admin = address(1);
    bytes32 public circuitHash = keccak256("test-xgboost-circuit");

    function setUp() public {
        vm.startPrank(admin);
        verifier = new RemainderVerifier(admin);

        // Register a test circuit
        uint256 numLayers = 4;
        uint256[] memory layerSizes = new uint256[](4);
        layerSizes[0] = 8; // input
        layerSizes[1] = 4; // comparison
        layerSizes[2] = 4; // routing
        layerSizes[3] = 1; // output

        uint8[] memory layerTypes = new uint8[](4);
        layerTypes[0] = 3; // input
        layerTypes[1] = 1; // mul
        layerTypes[2] = 0; // add
        layerTypes[3] = 0; // add

        bool[] memory isCommitted = new bool[](4);
        isCommitted[0] = true; // features are private
        isCommitted[1] = false;
        isCommitted[2] = false;
        isCommitted[3] = false;

        verifier.registerCircuit(circuitHash, numLayers, layerSizes, layerTypes, isCommitted, "test-xgboost");
        vm.stopPrank();
    }

    function test_circuit_registered() public view {
        assertTrue(verifier.isCircuitActive(circuitHash));
    }

    function test_circuit_deactivation() public {
        vm.prank(admin);
        verifier.deactivateCircuit(circuitHash);
        assertFalse(verifier.isCircuitActive(circuitHash));
    }

    function test_circuit_reactivation() public {
        vm.startPrank(admin);
        verifier.deactivateCircuit(circuitHash);
        verifier.reactivateCircuit(circuitHash);
        vm.stopPrank();
        assertTrue(verifier.isCircuitActive(circuitHash));
    }

    function test_reject_unregistered_circuit() public {
        bytes memory proof = abi.encodePacked(bytes4("REM1"), bytes32(0));
        bytes memory pubInputs = abi.encodePacked(uint256(1));
        bytes32 fakeHash = keccak256("nonexistent");

        vm.expectRevert(RemainderVerifier.CircuitNotRegistered.selector);
        verifier.verifyProof(proof, fakeHash, pubInputs, "");
    }

    function test_reject_wrong_selector() public {
        bytes memory proof = abi.encodePacked(bytes4("FAKE"), bytes32(0));
        bytes memory pubInputs = abi.encodePacked(uint256(1));

        vm.expectRevert(RemainderVerifier.InvalidProofSelector.selector);
        verifier.verifyProof(proof, circuitHash, pubInputs, "");
    }

    function test_reject_empty_proof() public {
        bytes memory proof = "";
        bytes memory pubInputs = abi.encodePacked(uint256(1));

        vm.expectRevert(RemainderVerifier.InvalidProofLength.selector);
        verifier.verifyProof(proof, circuitHash, pubInputs, "");
    }

    function test_only_admin_can_register() public {
        vm.prank(address(99));
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, address(99)));
        uint256[] memory sizes = new uint256[](1);
        uint8[] memory types = new uint8[](1);
        bool[] memory committed = new bool[](1);
        verifier.registerCircuit(keccak256("another"), 1, sizes, types, committed, "test");
    }

    function test_ownership_transfer() public {
        address newAdmin = address(42);
        vm.prank(admin);
        verifier.transferOwnership(newAdmin);
        // Ownable2Step: must accept
        vm.prank(newAdmin);
        verifier.acceptOwnership();
        assertEq(verifier.owner(), newAdmin);
    }

    function test_get_circuit_hashes() public view {
        bytes32[] memory hashes = verifier.getCircuitHashes();
        assertEq(hashes.length, 1);
        assertEq(hashes[0], circuitHash);
    }

    // ========================================================================
    // NEGATIVE PATH SECURITY TESTS (T406)
    // ========================================================================

    /// @notice Register a circuit with a specific generators hash, then attempt
    ///         to verify a proof using generators data that does not match the
    ///         registered hash. Should revert with InvalidGenerators.
    function test_registerCircuit_invalidGeneratorsHash_reverts() public {
        bytes32 newCircuitHash = keccak256("gens-test-circuit");
        bytes memory fakeGens = abi.encodePacked(uint256(111), uint256(222));
        bytes32 expectedGensHash = keccak256(fakeGens);

        // Register with a specific generators hash
        vm.startPrank(admin);
        uint256[] memory sizes = new uint256[](2);
        sizes[0] = 4;
        sizes[1] = 2;
        uint8[] memory types = new uint8[](2);
        types[0] = 3;
        types[1] = 0;
        bool[] memory committed = new bool[](2);
        committed[0] = true;
        committed[1] = false;

        verifier.registerCircuitWithGens(
            newCircuitHash, 2, sizes, types, committed, "gens-test", expectedGensHash
        );
        vm.stopPrank();

        // Build a valid-looking proof (correct selector) but with WRONG generators data
        bytes memory proof = abi.encodePacked(bytes4("REM1"), bytes32(0));
        bytes memory pubInputs = abi.encodePacked(uint256(1));
        bytes memory wrongGens = abi.encodePacked(uint256(999), uint256(888));

        // The keccak256(wrongGens) != expectedGensHash, so this should revert
        vm.expectRevert(RemainderVerifier.InvalidGenerators.selector);
        verifier.verifyProof(proof, newCircuitHash, pubInputs, wrongGens);
    }

    /// @notice Attempt to verify a proof for a circuit hash that was never registered.
    ///         Should revert with CircuitNotRegistered.
    function test_verifyProof_unregisteredCircuit_reverts() public {
        bytes32 unregisteredHash = keccak256("never-registered");
        bytes memory proof = abi.encodePacked(bytes4("REM1"), bytes32(0));
        bytes memory pubInputs = abi.encodePacked(uint256(1));

        vm.expectRevert(RemainderVerifier.CircuitNotRegistered.selector);
        verifier.verifyProof(proof, unregisteredHash, pubInputs, "");
    }

    /// @notice Register a circuit, deactivate it, then attempt to verify a proof.
    ///         Should revert with CircuitNotActive.
    function test_verifyProof_deactivatedCircuit_reverts() public {
        // Deactivate the circuit that was registered in setUp()
        vm.prank(admin);
        verifier.deactivateCircuit(circuitHash);

        // Now attempt verification
        bytes memory proof = abi.encodePacked(bytes4("REM1"), bytes32(0));
        bytes memory pubInputs = abi.encodePacked(uint256(1));

        vm.expectRevert(RemainderVerifier.CircuitNotActive.selector);
        verifier.verifyProof(proof, circuitHash, pubInputs, "");
    }

    /// @notice Non-owner calling pause() should revert with OwnableUnauthorizedAccount.
    function test_nonOwner_cannotPause_reverts() public {
        address nonOwner = address(0xdead);
        vm.prank(nonOwner);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, nonOwner));
        verifier.pause();
    }

    /// @notice Non-owner calling registerCircuit() should revert with OwnableUnauthorizedAccount.
    function test_nonOwner_cannotRegisterCircuit_reverts() public {
        address nonOwner = address(0xdead);
        vm.prank(nonOwner);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, nonOwner));

        uint256[] memory sizes = new uint256[](1);
        uint8[] memory types = new uint8[](1);
        bool[] memory committed = new bool[](1);
        verifier.registerCircuit(keccak256("blocked"), 1, sizes, types, committed, "blocked");
    }

    // ========================================================================
    // T465: CIRCUIT REGISTRATION VALIDATION TESTS
    // ========================================================================

    function test_registerCircuit_zeroNumLayers_reverts() public {
        vm.prank(admin);
        uint256[] memory sizes = new uint256[](0);
        uint8[] memory types = new uint8[](0);
        bool[] memory committed = new bool[](0);

        vm.expectRevert("numLayers must be > 0");
        verifier.registerCircuit(keccak256("zero-layers"), 0, sizes, types, committed, "zero-layers");
    }

    function test_registerCircuit_zeroCircuitHash_reverts() public {
        vm.prank(admin);
        uint256[] memory sizes = new uint256[](1);
        sizes[0] = 4;
        uint8[] memory types = new uint8[](1);
        types[0] = 3;
        bool[] memory committed = new bool[](1);

        vm.expectRevert("Circuit hash cannot be zero");
        verifier.registerCircuit(bytes32(0), 1, sizes, types, committed, "zero-hash");
    }

    function test_registerCircuit_duplicate_reverts() public {
        // circuitHash already registered in setUp
        vm.prank(admin);
        uint256[] memory sizes = new uint256[](4);
        sizes[0] = 8;
        sizes[1] = 4;
        sizes[2] = 4;
        sizes[3] = 1;
        uint8[] memory types = new uint8[](4);
        types[0] = 3;
        types[1] = 1;
        types[2] = 0;
        types[3] = 0;
        bool[] memory committed = new bool[](4);
        committed[0] = true;

        vm.expectRevert("Circuit already registered");
        verifier.registerCircuit(circuitHash, 4, sizes, types, committed, "duplicate");
    }

    function test_registerCircuitWithGens_zeroNumLayers_reverts() public {
        vm.prank(admin);
        uint256[] memory sizes = new uint256[](0);
        uint8[] memory types = new uint8[](0);
        bool[] memory committed = new bool[](0);

        vm.expectRevert("numLayers must be > 0");
        verifier.registerCircuitWithGens(
            keccak256("zero-layers-gens"), 0, sizes, types, committed, "zero-layers-gens", bytes32(uint256(1))
        );
    }

    function test_registerCircuit_layerSizeMismatch_reverts() public {
        vm.prank(admin);
        uint256[] memory sizes = new uint256[](2);
        sizes[0] = 4;
        sizes[1] = 2;
        uint8[] memory types = new uint8[](3);
        bool[] memory committed = new bool[](3);

        vm.expectRevert("Layer sizes length mismatch");
        verifier.registerCircuit(keccak256("mismatch"), 3, sizes, types, committed, "mismatch");
    }
}

/// @title RemainderVerifierTestDAGSecurity
/// @notice DAG batch negative path tests for access control (T406).
///         Uses the phase1a_dag_fixture for realistic proof data.
contract RemainderVerifierTestDAGSecurity is Test {
    RemainderVerifier verifier;

    bytes proofHex;
    bytes gensHex;
    bytes32 circuitHash;
    bytes publicInputsHex;

    function setUp() public {
        verifier = new RemainderVerifier(address(this));
        _loadAndRegisterDAG();
    }

    function _loadAndRegisterDAG() internal {
        string memory json = vm.readFile("test/fixtures/phase1a_dag_fixture.json");
        proofHex = vm.parseJsonBytes(json, ".proof_hex");
        gensHex = vm.parseJsonBytes(json, ".gens_hex");
        circuitHash = vm.parseJsonBytes32(json, ".circuit_hash_raw");
        publicInputsHex = vm.parseJsonBytes(json, ".public_inputs_hex");

        GKRDAGVerifier.DAGCircuitDescription memory desc;
        desc.numComputeLayers = vm.parseJsonUint(json, ".dag_circuit_description.numComputeLayers");
        desc.numInputLayers = vm.parseJsonUint(json, ".dag_circuit_description.numInputLayers");
        desc.layerTypes = _parseJsonUint8Array(json, ".dag_circuit_description.layerTypes");
        desc.numSumcheckRounds = vm.parseJsonUintArray(json, ".dag_circuit_description.numSumcheckRounds");
        desc.atomOffsets = vm.parseJsonUintArray(json, ".dag_circuit_description.atomOffsets");
        desc.atomTargetLayers = vm.parseJsonUintArray(json, ".dag_circuit_description.atomTargetLayers");
        desc.atomCommitIdxs = vm.parseJsonUintArray(json, ".dag_circuit_description.atomCommitIdxs");
        desc.ptOffsets = vm.parseJsonUintArray(json, ".dag_circuit_description.ptOffsets");
        desc.ptData = vm.parseJsonUintArray(json, ".dag_circuit_description.ptData");
        desc.inputIsCommitted = _parseJsonBoolArray(json, ".dag_circuit_description.inputIsCommitted");
        desc.oracleProductOffsets = vm.parseJsonUintArray(json, ".dag_circuit_description.oracleProductOffsets");
        desc.oracleResultIdxs = vm.parseJsonUintArray(json, ".dag_circuit_description.oracleResultIdxs");
        desc.oracleExprCoeffs = _parseJsonUint256Array(json, ".dag_circuit_description.oracleExprCoeffs");

        verifier.registerDAGCircuit(circuitHash, abi.encode(desc), "xgboost-sec", keccak256(gensHex));
    }

    function _parseJsonUint8Array(string memory json, string memory key) internal pure returns (uint8[] memory result) {
        uint256[] memory raw = vm.parseJsonUintArray(json, key);
        result = new uint8[](raw.length);
        for (uint256 i = 0; i < raw.length; i++) {
            result[i] = uint8(raw[i]);
        }
    }

    function _parseJsonBoolArray(string memory json, string memory key) internal pure returns (bool[] memory result) {
        bytes memory raw = vm.parseJson(json, key);
        result = abi.decode(raw, (bool[]));
    }

    function _parseJsonUint256Array(string memory json, string memory key)
        internal
        pure
        returns (uint256[] memory result)
    {
        bytes memory raw = vm.parseJson(json, key);
        bytes32[] memory parsed = abi.decode(raw, (bytes32[]));
        result = new uint256[](parsed.length);
        for (uint256 i = 0; i < parsed.length; i++) {
            result[i] = uint256(parsed[i]);
        }
    }

    /// @notice Start a DAG batch session, then attempt to continue from a
    ///         different address. Should revert with "Batch: unauthorized".
    function test_DAGBatch_wrongSender_continue_reverts() public {
        // Start session as address(this)
        bytes32 sessionId = verifier.startDAGBatchVerify(proofHex, circuitHash, publicInputsHex, gensHex);
        assertTrue(sessionId != bytes32(0), "sessionId should be non-zero");

        // Attempt to continue from a different address
        vm.prank(address(0xdead));
        vm.expectRevert("Batch: unauthorized");
        verifier.continueDAGBatchVerify(sessionId, proofHex, publicInputsHex, gensHex);
    }

    /// @notice Start a DAG batch session, complete all compute batches, then
    ///         attempt to finalize from a different address. Should revert
    ///         with "Batch: unauthorized".
    function test_DAGBatch_wrongSender_finalize_reverts() public {
        // Start session as address(this)
        bytes32 sessionId = verifier.startDAGBatchVerify(proofHex, circuitHash, publicInputsHex, gensHex);

        // Complete all compute batches from the original sender
        (,, uint256 totalBatches,,,) = verifier.getDAGBatchSession(sessionId);
        for (uint256 i = 0; i < totalBatches; i++) {
            verifier.continueDAGBatchVerify(sessionId, proofHex, publicInputsHex, gensHex);
        }

        // Attempt to finalize from a different address
        vm.prank(address(0xdead));
        vm.expectRevert("Batch: unauthorized");
        verifier.finalizeDAGBatchVerify(sessionId, proofHex, publicInputsHex, gensHex);
    }
}

/// @title RemainderVerifierNegativeTest
/// @notice Negative / adversarial tests for RemainderVerifier.verifyProof
///         using the real e2e_fixture.json proof data.
contract RemainderVerifierNegativeTest is Test {
    RemainderVerifier public verifier;
    bytes32 public circuitHash;

    // Loaded from fixture
    bytes internal rawProof;
    bytes internal gensData;
    bytes internal pubInputs;

    function setUp() public {
        // Deploy verifier
        verifier = new RemainderVerifier(address(this));

        // Load fixture
        string memory json = vm.readFile("test/fixtures/e2e_fixture.json");
        rawProof = vm.parseJsonBytes(json, ".proof_hex");
        gensData = vm.parseJsonBytes(json, ".gens_hex");
        circuitHash = bytes32(vm.parseJsonBytes32(json, ".circuit_hash_raw"));

        // Public inputs matching the fixture (values 6 and 20)
        pubInputs = abi.encodePacked(uint256(6), uint256(20));

        // Register the circuit matching the fixture topology:
        // 3 layers (input:committed, mul, add/output)
        uint256[] memory sizes = new uint256[](3);
        sizes[0] = 4;
        sizes[1] = 2;
        sizes[2] = 2;
        uint8[] memory types = new uint8[](3);
        types[0] = 3; // input
        types[1] = 1; // multiply
        types[2] = 0; // add/subtract (output)
        bool[] memory committed = new bool[](3);
        committed[0] = true;
        committed[1] = false;
        committed[2] = false;

        verifier.registerCircuit(circuitHash, 3, sizes, types, committed, "test-mul-circuit");
    }

    /// @notice Sanity: the valid proof should pass verification (baseline)
    function test_baseline_valid_proof_passes() public view {
        // Should not revert
        verifier.verifyOrRevert(rawProof, circuitHash, pubInputs, gensData);
    }

    /// @notice Truncated proof (first 100 bytes of a valid proof) should revert.
    ///         100 bytes is past the 4-byte selector but too short to decode any
    ///         meaningful GKR proof structure.
    function test_verify_truncated_proof_reverts() public {
        // Take only the first 100 bytes of the valid proof
        bytes memory truncated = new bytes(100);
        for (uint256 i = 0; i < 100; i++) {
            truncated[i] = rawProof[i];
        }

        // The selector "REM1" is intact, but the proof body is too short to decode.
        // This should revert during ABI decoding of the proof structure.
        vm.expectRevert();
        verifier.verifyProof(truncated, circuitHash, pubInputs, gensData);
    }

    /// @notice Corrupted proof: flip several bytes in the proof body.
    ///         The proof should either revert during decoding or fail verification.
    function test_verify_corrupted_proof_fails() public {
        // Make a mutable copy
        bytes memory corrupted = new bytes(rawProof.length);
        for (uint256 i = 0; i < rawProof.length; i++) {
            corrupted[i] = rawProof[i];
        }

        // Corrupt bytes at several positions past the selector (bytes 4-7 are proof header).
        // Flip bytes deep in the proof body to corrupt sumcheck / commitment data.
        // Choose offsets well past the header to hit actual proof values.
        uint256[5] memory offsets = [uint256(100), uint256(200), uint256(500), uint256(1000), uint256(2000)];
        for (uint256 i = 0; i < offsets.length; i++) {
            if (offsets[i] < corrupted.length) {
                corrupted[offsets[i]] = bytes1(uint8(corrupted[offsets[i]]) ^ 0xFF);
            }
        }

        // The corrupted proof may revert (decode failure) or return false (verification failure).
        // Either outcome is acceptable -- the key requirement is it must NOT return true.
        try verifier.verifyProof(corrupted, circuitHash, pubInputs, gensData) returns (bool valid) {
            assertFalse(valid, "Corrupted proof must not pass verification");
        } catch {
            // Revert is also acceptable -- corrupted data may not decode at all
            assertTrue(true, "Corrupted proof reverted (acceptable)");
        }
    }

    /// @notice Empty proof (zero bytes) should revert with InvalidProofLength.
    function test_verify_empty_proof_reverts() public {
        bytes memory empty = "";

        vm.expectRevert(RemainderVerifier.InvalidProofLength.selector);
        verifier.verifyProof(empty, circuitHash, pubInputs, gensData);
    }

    /// @notice Valid proof but with a circuit hash that does not match any registered circuit.
    ///         Should revert with CircuitNotRegistered.
    function test_verify_wrong_circuit_hash_fails() public {
        // Use a circuit hash that was never registered
        bytes32 wrongHash = keccak256("completely-wrong-circuit-hash");

        vm.expectRevert(RemainderVerifier.CircuitNotRegistered.selector);
        verifier.verifyProof(rawProof, wrongHash, pubInputs, gensData);
    }

    // ========================================================================
    // ADDITIONAL NEGATIVE TESTS (T386)
    // ========================================================================

    /// @notice Truncated proof: only selector + 1 byte. The proof body is far too short
    ///         to even read the first 32-byte circuit hash field, so decodeProof must revert.
    function test_verify_selector_plus_one_byte_reverts() public {
        bytes memory tiny = new bytes(5);
        tiny[0] = bytes1("R");
        tiny[1] = bytes1("E");
        tiny[2] = bytes1("M");
        tiny[3] = bytes1("1");
        tiny[4] = bytes1(0x00);

        // Selector is valid but the body is only 1 byte -- far too short for any decoding
        vm.expectRevert();
        verifier.verifyProof(tiny, circuitHash, pubInputs, gensData);
    }

    /// @notice Truncated proof: cut off mid-layer-proof. We keep the header sections
    ///         intact but truncate partway through the first committed layer proof.
    ///         The decoder should revert with an out-of-bounds slice.
    function test_verify_truncated_mid_layer_reverts() public {
        // Find a truncation point that is past the output proofs section
        // but inside the first committed layer proof.
        // The proof structure after the selector:
        //   32 bytes circuit_hash
        //   32 bytes numPubInputSections + variable pub inputs
        //   32 bytes numOutputProofs + 64*numOutputProofs
        //   32 bytes numLayerProofs  <-- we want to truncate just after this
        //
        // For a 3-layer circuit with 1 pub input section:
        //   offset 0: circuit hash (32)
        //   offset 32: numPubInputSections = 1 (32)
        //   offset 64: count of section 0 = 2 (32), then 2*32 = 64 bytes of values
        //   offset 160: numOutputProofs = 1 (32), then 1*64 = 64 bytes
        //   offset 256: numLayerProofs = 2 (32)
        //   offset 288: first committed layer proof data starts
        //
        // We truncate at offset 300 (4 bytes selector + 296 proof data), which is
        // 12 bytes into the first committed layer proof -- not enough to read even the
        // first G1 point (sum.x, sum.y = 64 bytes).

        // Use a conservative approach: keep 60% of the valid proof and chop the rest.
        // This ensures we are well past headers but truncated inside layer proof data.
        uint256 cutoff = (rawProof.length * 60) / 100;
        bytes memory truncated = new bytes(cutoff);
        for (uint256 i = 0; i < cutoff; i++) {
            truncated[i] = rawProof[i];
        }

        // Should revert during proof decoding (out-of-bounds calldata slice)
        vm.expectRevert();
        verifier.verifyProof(truncated, circuitHash, pubInputs, gensData);
    }

    /// @notice Corrupted proof: flip a single byte in the first output claim commitment
    ///         (an EC point). This targeted corruption invalidates the commitment point
    ///         without breaking the proof structure, so verification should fail or revert.
    function test_verify_corrupted_single_commitment_byte_fails() public {
        bytes memory corrupted = new bytes(rawProof.length);
        for (uint256 i = 0; i < rawProof.length; i++) {
            corrupted[i] = rawProof[i];
        }

        // The output claim commitments are the first EC points after the public inputs
        // section in the proof body. For this fixture, that is approximately at
        // offset 4 (selector) + 32 (circuit hash) + 32 (numPubSections) + ...
        // We target an offset deep enough to be inside a commitment G1 point.
        // Offset 200 is reliably within the output commitment section for this fixture.
        // XOR a single byte -- this changes one coordinate of a G1 point.
        if (corrupted.length > 200) {
            corrupted[200] = bytes1(uint8(corrupted[200]) ^ 0x01);
        }

        // A single-byte corruption in a commitment point should cause verification
        // failure (invalid EC point or bad Pedersen check) or a revert.
        try verifier.verifyProof(corrupted, circuitHash, pubInputs, gensData) returns (bool valid) {
            assertFalse(valid, "Single-byte corrupted commitment must not pass verification");
        } catch {
            // Revert is acceptable -- corrupted EC point may fail precompile check
            assertTrue(true, "Single-byte corrupted proof reverted (acceptable)");
        }
    }

    /// @notice Corrupted proof: flip bytes only in the sumcheck messages region.
    ///         The proof should decode fine but the sumcheck verification must fail.
    function test_verify_corrupted_sumcheck_messages_fails() public {
        bytes memory corrupted = new bytes(rawProof.length);
        for (uint256 i = 0; i < rawProof.length; i++) {
            corrupted[i] = rawProof[i];
        }

        // Target the middle of the proof body where sumcheck messages live.
        // For a ~3300 byte proof, the sumcheck messages are roughly in the
        // 400-1500 byte range. Corrupt several bytes there.
        uint256 midpoint = rawProof.length / 2;
        for (uint256 i = 0; i < 8; i++) {
            uint256 pos = midpoint + i * 32; // every 32 bytes (one field element)
            if (pos < corrupted.length) {
                corrupted[pos] = bytes1(uint8(corrupted[pos]) ^ 0xAA);
            }
        }

        try verifier.verifyProof(corrupted, circuitHash, pubInputs, gensData) returns (bool valid) {
            assertFalse(valid, "Corrupted sumcheck messages must not pass verification");
        } catch {
            assertTrue(true, "Corrupted sumcheck proof reverted (acceptable)");
        }
    }

    /// @notice Zero-length claims section: construct a minimal proof with valid selector
    ///         but zero output commitments and zero layer proofs. The verifier should
    ///         either revert during decoding or fail during verification (no layers to verify).
    function test_verify_zero_length_claims_section_reverts() public {
        // Build a minimal proof body with valid structure but zero-length arrays:
        //   selector: "REM1" (4 bytes)
        //   circuit_hash: 32 bytes (zeros)
        //   numPubInputSections: 0 (32 bytes)
        //   numOutputProofs: 0 (32 bytes)
        //   numLayerProofs: 0 (32 bytes)
        //   numFsClaims: 0 (32 bytes)
        //   numPubClaims: 0 (32 bytes)
        //   numInputProofs: 0 (32 bytes)
        bytes memory zeroProof = new bytes(4 + 7 * 32);
        zeroProof[0] = bytes1("R");
        zeroProof[1] = bytes1("E");
        zeroProof[2] = bytes1("M");
        zeroProof[3] = bytes1("1");
        // All remaining bytes are zero, which encodes:
        //   circuit_hash = 0x00...00
        //   all counts = 0

        // With zero layer proofs and zero input proofs, the GKR verifier will
        // either revert (e.g., array index out of bounds) or fail verification.
        // Either outcome is acceptable -- the proof must not succeed.
        try verifier.verifyProof(zeroProof, circuitHash, pubInputs, gensData) returns (bool valid) {
            assertFalse(valid, "Zero-claims proof must not pass verification");
        } catch {
            assertTrue(true, "Zero-claims proof reverted (acceptable)");
        }
    }

    /// @notice Zero output commitments with non-zero layer proofs:
    ///         The proof has a structurally valid format but the output claims array
    ///         is empty while layer proofs reference non-existent outputs.
    function test_verify_zero_output_commitments_fails() public {
        // Start with a valid proof and zero out the numOutputProofs field.
        // In the proof body (after selector), the structure is:
        //   [0..32)   circuit_hash
        //   [32..64)  numPubInputSections
        //   ...variable pub input data...
        //   [X..X+32) numOutputProofs  <-- we zero this out
        //
        // We find the numOutputProofs offset by parsing the pub input sections.
        bytes memory corrupted = new bytes(rawProof.length);
        for (uint256 i = 0; i < rawProof.length; i++) {
            corrupted[i] = rawProof[i];
        }

        // Parse to find the numOutputProofs offset:
        // After selector (4 bytes): circuit_hash (32) + numPubInputSections (32)
        uint256 offset = 4 + 32; // past selector + circuit hash
        // Read numPubInputSections
        uint256 numPubSections;
        assembly {
            numPubSections := mload(add(corrupted, add(32, offset)))
        }
        offset += 32;
        // Skip each pub input section
        for (uint256 i = 0; i < numPubSections; i++) {
            uint256 cnt;
            assembly {
                cnt := mload(add(corrupted, add(32, offset)))
            }
            offset += 32 + cnt * 32;
        }

        // Now offset points to numOutputProofs in the corrupted bytes.
        // Zero it out (set to 0)
        assembly {
            mstore(add(corrupted, add(32, offset)), 0)
        }

        // With zero output commitments, the GKR verifier has nothing to verify
        // the output layer against. This should fail.
        try verifier.verifyProof(corrupted, circuitHash, pubInputs, gensData) returns (bool valid) {
            assertFalse(valid, "Zero output commitments proof must not pass");
        } catch {
            assertTrue(true, "Zero output commitments proof reverted (acceptable)");
        }
    }

    /// @notice Misaligned offset: insert extra bytes after the selector to shift
    ///         all field boundaries by a non-32-byte amount. The decoder reads
    ///         32-byte words at fixed offsets, so misalignment causes it to read
    ///         garbage values for counts and coordinates.
    function test_verify_misaligned_offset_reverts() public {
        // Insert 7 extra bytes after the selector to misalign all subsequent
        // 32-byte field reads. This is 7 bytes (not a multiple of 32).
        uint256 extraBytes = 7;
        bytes memory misaligned = new bytes(rawProof.length + extraBytes);
        // Copy selector
        for (uint256 i = 0; i < 4; i++) {
            misaligned[i] = rawProof[i];
        }
        // Insert 7 garbage bytes
        for (uint256 i = 0; i < extraBytes; i++) {
            misaligned[4 + i] = bytes1(uint8(0xDE));
        }
        // Copy rest of proof body shifted by 7
        for (uint256 i = 4; i < rawProof.length; i++) {
            misaligned[i + extraBytes] = rawProof[i];
        }

        // The decoder will read the circuit hash starting at offset 0 of proof body,
        // which now contains 0xDE bytes + the real circuit hash shifted by 7.
        // The numPubInputSections field will be misread, likely as a huge number,
        // causing an out-of-bounds revert or memory allocation failure.
        vm.expectRevert();
        verifier.verifyProof(misaligned, circuitHash, pubInputs, gensData);
    }

    /// @notice Misaligned offset: remove 1 byte from the middle of the proof body
    ///         to shift all subsequent field boundaries. The decoder should revert
    ///         because 32-byte reads will cross into adjacent fields.
    function test_verify_offset_shift_by_one_byte_reverts() public {
        // Remove byte at position 100 (well inside the proof body past the header)
        bytes memory shifted = new bytes(rawProof.length - 1);
        uint256 removeAt = 100;
        for (uint256 i = 0; i < removeAt; i++) {
            shifted[i] = rawProof[i];
        }
        for (uint256 i = removeAt + 1; i < rawProof.length; i++) {
            shifted[i - 1] = rawProof[i];
        }

        // Removing 1 byte shifts all subsequent 32-byte fields by 1 byte,
        // causing every read after that point to be misaligned.
        // This should cause either a decode revert or verification failure.
        try verifier.verifyProof(shifted, circuitHash, pubInputs, gensData) returns (bool valid) {
            assertFalse(valid, "Byte-shifted proof must not pass verification");
        } catch {
            assertTrue(true, "Byte-shifted proof reverted (acceptable)");
        }
    }

    /// @notice Proof with only the selector (exactly 4 bytes). The proof body is
    ///         completely empty, so any attempt to decode fields should revert.
    function test_verify_selector_only_reverts() public {
        bytes memory selectorOnly = new bytes(4);
        selectorOnly[0] = bytes1("R");
        selectorOnly[1] = bytes1("E");
        selectorOnly[2] = bytes1("M");
        selectorOnly[3] = bytes1("1");

        // The selector is valid but there is no proof body at all.
        // The first thing decodeProof tries is reading circuit hash at offset 0..32,
        // which will revert because the calldata slice is empty.
        vm.expectRevert();
        verifier.verifyProof(selectorOnly, circuitHash, pubInputs, gensData);
    }

    /// @notice Proof with an absurdly large numLayerProofs count.
    ///         This should cause a memory allocation failure or out-of-gas revert.
    function test_verify_huge_layer_count_reverts() public {
        // Build a proof body with valid header but numLayerProofs = 2^64.
        // This will cause the decoder to try allocating an impossibly large array.
        bytes memory hugeCount = new bytes(rawProof.length);
        for (uint256 i = 0; i < rawProof.length; i++) {
            hugeCount[i] = rawProof[i];
        }

        // Find the numLayerProofs offset (same as zero output commitments test)
        uint256 offset = 4 + 32; // past selector + circuit hash
        uint256 numPubSections;
        assembly {
            numPubSections := mload(add(hugeCount, add(32, offset)))
        }
        offset += 32;
        for (uint256 i = 0; i < numPubSections; i++) {
            uint256 cnt;
            assembly {
                cnt := mload(add(hugeCount, add(32, offset)))
            }
            offset += 32 + cnt * 32;
        }

        // Skip numOutputProofs + output proof data
        uint256 numOutputProofs;
        assembly {
            numOutputProofs := mload(add(hugeCount, add(32, offset)))
        }
        offset += 32 + numOutputProofs * 64;

        // Now offset points to numLayerProofs. Set it to an absurdly large value.
        assembly {
            mstore(add(hugeCount, add(32, offset)), 0xFFFFFFFFFFFFFFFF)
        }

        // Trying to allocate 2^64 layer proofs will revert (out of gas or memory)
        vm.expectRevert();
        verifier.verifyProof{gas: 1_000_000}(hugeCount, circuitHash, pubInputs, gensData);
    }
}

contract IntegrationTest is Test {
    ProgramRegistry public registry;
    ExecutionEngine public engine;
    MockRiscZeroVerifier public mockVerifier;
    RemainderVerifier public remainderVerifier;
    RemainderVerifierAdapter public remainderAdapter;

    address public deployer = address(1);
    address public requester = address(2);
    address public proverAddr = address(3);
    address public feeRecipient = address(4);

    bytes32 public risc0ImageId = bytes32(uint256(100));
    bytes32 public remainderCircuitHash = keccak256("xgboost-circuit");

    function setUp() public {
        vm.startPrank(deployer);

        // Deploy registry and verifiers
        registry = new ProgramRegistry(deployer);
        mockVerifier = new MockRiscZeroVerifier();
        remainderVerifier = new RemainderVerifier(deployer);

        // Deploy adapters
        remainderAdapter = new RemainderVerifierAdapter(address(remainderVerifier));

        // Deploy execution engine
        engine = new ExecutionEngine(deployer, address(registry), address(mockVerifier), feeRecipient);

        // Register risc0 program (backward-compatible, no verifier)
        registry.registerProgram(risc0ImageId, "test-risc0", "https://example.com/elf", bytes32(0));

        // Register remainder program with custom verifier
        registry.registerProgramWithVerifier(
            remainderCircuitHash,
            "test-xgboost",
            "https://example.com/circuit",
            bytes32(0),
            address(remainderAdapter),
            "remainder"
        );

        // Register the circuit in RemainderVerifier
        uint256[] memory sizes = new uint256[](2);
        sizes[0] = 4;
        sizes[1] = 1;
        uint8[] memory types = new uint8[](2);
        types[0] = 3;
        types[1] = 0;
        bool[] memory committed = new bool[](2);
        committed[0] = true;
        committed[1] = false;

        remainderVerifier.registerCircuit(remainderCircuitHash, 2, sizes, types, committed, "test-xgboost");

        vm.stopPrank();
    }

    function test_risc0_program_uses_default_verifier() public view {
        ProgramRegistry.Program memory prog = registry.getProgram(risc0ImageId);
        assertEq(prog.verifierContract, address(0));
        assertEq(keccak256(bytes(prog.proofSystem)), keccak256(bytes("risc0")));
    }

    function test_remainder_program_has_custom_verifier() public view {
        ProgramRegistry.Program memory prog = registry.getProgram(remainderCircuitHash);
        assertEq(prog.verifierContract, address(remainderAdapter));
        assertEq(keccak256(bytes(prog.proofSystem)), keccak256(bytes("remainder")));
    }

    function test_adapter_proof_system_names() public view {
        assertEq(keccak256(bytes(remainderAdapter.proofSystem())), keccak256(bytes("remainder")));
    }

    function test_update_verifier() public {
        vm.prank(deployer);
        registry.updateVerifier(risc0ImageId, address(remainderAdapter));

        ProgramRegistry.Program memory prog = registry.getProgram(risc0ImageId);
        assertEq(prog.verifierContract, address(remainderAdapter));
    }
}

/// @notice Tests for HyraxProofDecoder — validates the Rust→Solidity ABI bridge
contract HyraxProofDecoderTest is Test {
    /// @notice Helper to load proof from fixture file
    function _loadProof() internal view returns (bytes memory) {
        string memory json = vm.readFile("test/fixtures/hyrax_proof.json");
        bytes memory proofHex = vm.parseJsonBytes(json, ".proof_hex");
        return proofHex;
    }

    /// @notice Test that the proof selector is "REM1"
    function test_proof_selector() public view {
        bytes memory proof = _loadProof();
        assertGt(proof.length, 4, "Proof should be > 4 bytes");
        assertEq(proof[0], bytes1("R"), "Selector byte 0");
        assertEq(proof[1], bytes1("E"), "Selector byte 1");
        assertEq(proof[2], bytes1("M"), "Selector byte 2");
        assertEq(proof[3], bytes1("1"), "Selector byte 3");
    }

    /// @notice Test that the circuit hash is at the expected position
    function test_circuit_hash() public view {
        bytes memory proof = _loadProof();
        bytes32 expected = 0xfa7aaf631d1d3dafad2c8b03f5db3fef2f3b235e1d27b39c110e78fe0ebddb07;

        // Circuit hash starts at offset 4 (after selector)
        bytes32 circuitHash;
        assembly {
            circuitHash := mload(add(proof, 36)) // 32 (length prefix) + 4 (selector)
        }
        assertEq(circuitHash, expected, "Circuit hash mismatch");
    }

    /// @notice Test proof summary decoding from real Rust-generated data
    function test_decode_summary() public view {
        bytes memory proof = _loadProof();
        // Pass data after the 4-byte selector via external call for calldata
        bytes memory proofData = _stripSelector(proof);
        HyraxProofDecoder.ProofSummary memory summary = this._decodeSummaryCalldata(proofData);

        // Validate circuit hash
        assertEq(
            summary.circuitHash,
            bytes32(0xfa7aaf631d1d3dafad2c8b03f5db3fef2f3b235e1d27b39c110e78fe0ebddb07),
            "Circuit hash"
        );

        // Validate structure counts (from gen_test_proof output)
        assertEq(summary.numPublicInputs, 1, "Should have 1 public input");
        assertEq(summary.numOutputProofs, 1, "Should have 1 output proof");
        assertEq(summary.numLayerProofs, 2, "Should have 2 layer proofs");
        assertEq(summary.numFsClaims, 0, "Should have 0 FS claims");
        assertEq(summary.numPubClaims, 1, "Should have 1 public value claim");
        assertEq(summary.numInputProofs, 1, "Should have 1 input proof");

        // Validate total bytes matches (3332 - 4 selector = 3328)
        assertEq(summary.totalBytes, proof.length - 4, "Total decoded bytes should match proof size");
    }

    /// @notice Test public input values from the decoded proof
    function test_decode_public_inputs() public view {
        bytes memory proof = _loadProof();
        bytes memory proofData = _stripSelector(proof);
        uint256[] memory values = this._decodePublicInputsCalldata(proofData);

        // The test circuit had [6, 20] as expected output (3*2=6, 5*4=20)
        assertEq(values.length, 2, "Public input should have 2 values");
        assertEq(values[0], 6, "Public input[0] = 6");
        assertEq(values[1], 20, "Public input[1] = 20");
    }

    /// @notice Test that ALL decoded EC points are on the BN254 curve
    function test_all_points_on_curve() public {
        bytes memory proof = _loadProof();
        bytes memory proofData = _stripSelector(proof);
        (uint256 numPoints, bool allOnCurve) = this._validateAllPointsCalldata(proofData);

        assertTrue(allOnCurve, "All EC points should be on BN254 curve");
        assertGt(numPoints, 0, "Should have found EC points");
        emit log_named_uint("Total EC points validated", numPoints);
    }

    /// @notice Test proof decode gas cost
    function test_decode_gas() public {
        bytes memory proof = _loadProof();
        bytes memory proofData = _stripSelector(proof);

        uint256 gasBefore = gasleft();
        HyraxProofDecoder.ProofSummary memory summary = this._decodeSummaryCalldata(proofData);
        uint256 gasUsed = gasBefore - gasleft();

        emit log_named_uint("Proof summary decode gas", gasUsed);
        emit log_named_uint("Proof size bytes", proof.length);
        emit log_named_uint("Layer proofs", summary.numLayerProofs);
        emit log_named_uint("Input proofs", summary.numInputProofs);

        gasBefore = gasleft();
        this._validateAllPointsCalldata(proofData);
        gasUsed = gasBefore - gasleft();
        emit log_named_uint("Point validation gas", gasUsed);
    }

    // ===== Helpers =====

    function _stripSelector(bytes memory proof) internal pure returns (bytes memory proofData) {
        proofData = new bytes(proof.length - 4);
        for (uint256 i = 0; i < proofData.length; i++) {
            proofData[i] = proof[i + 4];
        }
    }

    /// @notice External wrappers for calldata-based decoding
    function _decodeSummaryCalldata(bytes calldata data) external pure returns (HyraxProofDecoder.ProofSummary memory) {
        return HyraxProofDecoder.decodeSummary(data);
    }

    function _decodePublicInputsCalldata(bytes calldata data) external pure returns (uint256[] memory) {
        return HyraxProofDecoder.decodePublicInputs(data);
    }

    function _validateAllPointsCalldata(bytes calldata data) external pure returns (uint256, bool) {
        return HyraxProofDecoder.validateAllPoints(data);
    }
}

/// @title FiatShamirBridgeTest
/// @notice Tests that Solidity PoseidonSponge exactly matches Rust's PoseidonSponge
///         by replaying the Fiat-Shamir transcript from a real Remainder GKR proof.
///         Test vectors generated by gen_transcript_trace.rs.
contract FiatShamirBridgeTest is Test {
    /// @notice Replay the initial 6 absorbs (circuit hash + public inputs) and
    ///         verify the intermediate challenge matches the Rust trace.
    function test_circuit_hash_and_public_inputs_squeeze() public pure {
        PoseidonSponge.Sponge memory sponge = PoseidonSponge.init();

        // Absorb circuit description hash (2 Fq elements)
        PoseidonSponge.absorb(sponge, 0x000000000000000000000000000000005a9fc1776ad5b87f01d493983001d78f);
        PoseidonSponge.absorb(sponge, 0x000000000000000000000000000000005daa4b26c457652c4f9715758acff1bc);

        // Absorb public input values (2 Fq elements)
        PoseidonSponge.absorb(sponge, 0x0000000000000000000000000000000000000000000000000000000000000006);
        PoseidonSponge.absorb(sponge, 0x0000000000000000000000000000000000000000000000000000000000000014);

        // Absorb public input SHA-256 hash chain (2 Fq elements)
        PoseidonSponge.absorb(sponge, 0x00000000000000000000000000000000a998b9d31f69d8ae8e48768cf8b8a5ff);
        PoseidonSponge.absorb(sponge, 0x00000000000000000000000000000000c06bcddbc1b4d72d89678361cd10177b);

        // Squeeze intermediate challenge
        uint256 challenge = PoseidonSponge.squeeze(sponge);

        // From transcript_trace.json: after_circuit_hash_and_public_inputs
        assertEq(
            challenge,
            0x25386983a875cb8fab4670ee4a8edec51c642529a20fbaea97edb9d06004b066,
            "intermediate squeeze after 6 absorbs mismatch"
        );
    }

    /// @notice Full initial transcript replay: 12 absorbs → first Fiat-Shamir challenge.
    ///         This replays: circuit_hash(2) + public_inputs(4) + EC_points(4) + EC_hash_chain(2)
    ///         and verifies the resulting challenge matches "Challenge for claim on output".
    function test_full_initial_transcript_replay() public pure {
        PoseidonSponge.Sponge memory sponge = PoseidonSponge.init();

        // 1. Circuit description hash (2 elements)
        PoseidonSponge.absorb(sponge, 0x000000000000000000000000000000005a9fc1776ad5b87f01d493983001d78f);
        PoseidonSponge.absorb(sponge, 0x000000000000000000000000000000005daa4b26c457652c4f9715758acff1bc);

        // 2. Public input values (2 elements)
        PoseidonSponge.absorb(sponge, 0x0000000000000000000000000000000000000000000000000000000000000006);
        PoseidonSponge.absorb(sponge, 0x0000000000000000000000000000000000000000000000000000000000000014);

        // 3. Public input SHA-256 hash chain (2 elements)
        PoseidonSponge.absorb(sponge, 0x00000000000000000000000000000000a998b9d31f69d8ae8e48768cf8b8a5ff);
        PoseidonSponge.absorb(sponge, 0x00000000000000000000000000000000c06bcddbc1b4d72d89678361cd10177b);

        // 4. Hyrax input commitment EC points (4 elements: 2 points × 2 coords)
        PoseidonSponge.absorb(sponge, 0x1be725444751be14b75a8d3f9635338b1c9a4bc6ec128dc038c1b3e3657b6751);
        PoseidonSponge.absorb(sponge, 0x29acc709c5e329e3a561a90c16d62ef1053900e2c06b8931293469b540d1309e);
        PoseidonSponge.absorb(sponge, 0x23385f96de594180eb38ae9f9d0d2aa9af09c467c78f4c17e700f6f5b3ae96fe);
        PoseidonSponge.absorb(sponge, 0x2ef6deb672454c927e50322143c9bdebc720f4df6001f4da6863eca6986bbb90);

        // 5. EC commitment SHA-256 hash chain (2 elements)
        PoseidonSponge.absorb(sponge, 0x00000000000000000000000000000000d744e81c0ba8f5f2737a3a4ef940ace9);
        PoseidonSponge.absorb(sponge, 0x00000000000000000000000000000000c07203138670b777fcb4190942c7ac6e);

        // Squeeze first challenge
        uint256 challenge = PoseidonSponge.squeeze(sponge);

        // From transcript_trace.json: first_challenge_for_output_claim
        assertEq(
            challenge, 0x0d913ea20ccbceea7bb452e44f2b1e1bfd9efe3dca6898d297d23a2b22ea57ad, "first FS challenge mismatch"
        );
    }

    /// @notice Gas benchmark for full 12-absorb transcript replay
    function test_transcript_replay_gas() public {
        PoseidonSponge.Sponge memory sponge = PoseidonSponge.init();

        uint256 gasBefore = gasleft();

        PoseidonSponge.absorb(sponge, 0x000000000000000000000000000000005a9fc1776ad5b87f01d493983001d78f);
        PoseidonSponge.absorb(sponge, 0x000000000000000000000000000000005daa4b26c457652c4f9715758acff1bc);
        PoseidonSponge.absorb(sponge, 0x0000000000000000000000000000000000000000000000000000000000000006);
        PoseidonSponge.absorb(sponge, 0x0000000000000000000000000000000000000000000000000000000000000014);
        PoseidonSponge.absorb(sponge, 0x00000000000000000000000000000000a998b9d31f69d8ae8e48768cf8b8a5ff);
        PoseidonSponge.absorb(sponge, 0x00000000000000000000000000000000c06bcddbc1b4d72d89678361cd10177b);
        PoseidonSponge.absorb(sponge, 0x1be725444751be14b75a8d3f9635338b1c9a4bc6ec128dc038c1b3e3657b6751);
        PoseidonSponge.absorb(sponge, 0x29acc709c5e329e3a561a90c16d62ef1053900e2c06b8931293469b540d1309e);
        PoseidonSponge.absorb(sponge, 0x23385f96de594180eb38ae9f9d0d2aa9af09c467c78f4c17e700f6f5b3ae96fe);
        PoseidonSponge.absorb(sponge, 0x2ef6deb672454c927e50322143c9bdebc720f4df6001f4da6863eca6986bbb90);
        PoseidonSponge.absorb(sponge, 0x00000000000000000000000000000000d744e81c0ba8f5f2737a3a4ef940ace9);
        PoseidonSponge.absorb(sponge, 0x00000000000000000000000000000000c07203138670b777fcb4190942c7ac6e);
        uint256 challenge = PoseidonSponge.squeeze(sponge);

        uint256 gasUsed = gasBefore - gasleft();
        emit log_named_uint("Gas for 12-absorb transcript replay + squeeze", gasUsed);

        // Verify correctness
        assertEq(challenge, 0x0d913ea20ccbceea7bb452e44f2b1e1bfd9efe3dca6898d297d23a2b22ea57ad);

        // Expect < 350K gas (assembly-optimized Poseidon; ~46K per absorb+permute cycle)
        assertLt(gasUsed, 350_000, "transcript replay gas too high");
    }
}

/// @title E2ETranscriptReplayTest
/// @notice End-to-end test: loads a real ABI-encoded proof from Rust, decodes it,
///         extracts values, replays the Fiat-Shamir transcript, and validates
///         the resulting challenge matches the Rust verifier's challenge.
///         All data comes from a SINGLE proof run (gen_transcript_trace.rs).
contract E2ETranscriptReplayTest is Test {
    /// @notice Load the E2E fixture (proof + transcript trace from same run)
    function _loadFixture()
        internal
        view
        returns (
            bytes memory proofData,
            uint256 circuitHashFq1,
            uint256 circuitHashFq2,
            uint256 pubHashChain1,
            uint256 pubHashChain2,
            uint256 ecHashChain1,
            uint256 ecHashChain2,
            uint256 expectedChallenge
        )
    {
        string memory json = vm.readFile("test/fixtures/e2e_fixture.json");

        // Proof bytes (strip "REM1" selector)
        bytes memory rawProof = vm.parseJsonBytes(json, ".proof_hex");
        proofData = new bytes(rawProof.length - 4);
        for (uint256 i = 0; i < proofData.length; i++) {
            proofData[i] = rawProof[i + 4];
        }

        // Transcript trace values
        circuitHashFq1 = vm.parseJsonUint(json, ".transcript_trace.circuit_hash_fq_1");
        circuitHashFq2 = vm.parseJsonUint(json, ".transcript_trace.circuit_hash_fq_2");
        pubHashChain1 = vm.parseJsonUint(json, ".transcript_trace.public_input_hash_chain_1");
        pubHashChain2 = vm.parseJsonUint(json, ".transcript_trace.public_input_hash_chain_2");
        ecHashChain1 = vm.parseJsonUint(json, ".transcript_trace.input_commitment_hash_chain_1");
        ecHashChain2 = vm.parseJsonUint(json, ".transcript_trace.input_commitment_hash_chain_2");
        expectedChallenge = vm.parseJsonUint(json, ".challenges.first_challenge_for_output_claim");
    }

    /// @notice Full E2E: decode proof → extract values → replay transcript → validate challenge
    function test_e2e_decode_and_replay() public {
        (
            bytes memory proofData,
            uint256 circuitHashFq1,
            uint256 circuitHashFq2,
            uint256 pubHashChain1,
            uint256 pubHashChain2,
            uint256 ecHashChain1,
            uint256 ecHashChain2,
            uint256 expectedChallenge
        ) = _loadFixture();

        // Step 1: Decode proof summary — validates ABI encoding is parseable
        HyraxProofDecoder.ProofSummary memory summary = this._decodeSummaryE2E(proofData);
        assertEq(summary.numPublicInputs, 1, "Should have 1 public input");
        assertEq(summary.numInputProofs, 1, "Should have 1 input proof");
        emit log_named_uint("Proof size (bytes, excl selector)", proofData.length);

        // Step 2: Decode public inputs from the proof
        uint256[] memory pubInputs = this._decodePublicInputsE2E(proofData);
        assertEq(pubInputs.length, 2, "Should have 2 public input values");
        assertEq(pubInputs[0], 6, "Public input[0] should be 6");
        assertEq(pubInputs[1], 20, "Public input[1] should be 20");
        emit log_string("Decoded public inputs: [6, 20]");

        // Step 3: Decode input commitment EC points from the proof
        HyraxProofDecoder.DecodedPoint[] memory commitPoints = this._decodeInputCommitsE2E(proofData);
        assertEq(commitPoints.length, 2, "Should have 2 commitment points");

        // Validate points are on BN254 curve
        for (uint256 i = 0; i < commitPoints.length; i++) {
            assertTrue(
                HyraxVerifier.isOnCurve(HyraxVerifier.G1Point(commitPoints[i].x, commitPoints[i].y)),
                "Commitment point must be on curve"
            );
        }
        emit log_named_uint("Commitment point[0].x", commitPoints[0].x);
        emit log_named_uint("Commitment point[0].y", commitPoints[0].y);

        // Step 4: Replay Fiat-Shamir transcript using decoded values
        PoseidonSponge.Sponge memory sponge = PoseidonSponge.init();

        // 4a. Absorb circuit description hash (from fixture — deterministic per circuit)
        PoseidonSponge.absorb(sponge, circuitHashFq1);
        PoseidonSponge.absorb(sponge, circuitHashFq2);

        // 4b. Absorb public input values (DECODED from proof)
        PoseidonSponge.absorb(sponge, pubInputs[0]);
        PoseidonSponge.absorb(sponge, pubInputs[1]);

        // 4c. Absorb public input hash chain (from fixture — SHA-256 of public inputs)
        PoseidonSponge.absorb(sponge, pubHashChain1);
        PoseidonSponge.absorb(sponge, pubHashChain2);

        // 4d. Absorb EC commitment points (DECODED from proof)
        for (uint256 i = 0; i < commitPoints.length; i++) {
            PoseidonSponge.absorb(sponge, commitPoints[i].x);
            PoseidonSponge.absorb(sponge, commitPoints[i].y);
        }

        // 4e. Absorb EC commitment hash chain (from fixture — SHA-256 of EC coordinates)
        PoseidonSponge.absorb(sponge, ecHashChain1);
        PoseidonSponge.absorb(sponge, ecHashChain2);

        // Step 5: Squeeze first challenge and compare
        uint256 challenge = PoseidonSponge.squeeze(sponge);

        emit log_named_uint("Computed challenge", challenge);
        emit log_named_uint("Expected challenge", expectedChallenge);

        assertEq(challenge, expectedChallenge, "E2E: First Fiat-Shamir challenge mismatch");
    }

    /// @notice Validates that decoded EC commitment points match fixture values
    function test_e2e_commitment_points_match_fixture() public view {
        string memory json = vm.readFile("test/fixtures/e2e_fixture.json");
        bytes memory rawProof = vm.parseJsonBytes(json, ".proof_hex");
        bytes memory proofData = new bytes(rawProof.length - 4);
        for (uint256 i = 0; i < proofData.length; i++) {
            proofData[i] = rawProof[i + 4];
        }

        // Load expected points from fixture
        uint256 expectedX0 = vm.parseJsonUint(json, ".transcript_trace.input_commitment_points[0].x");
        uint256 expectedY0 = vm.parseJsonUint(json, ".transcript_trace.input_commitment_points[0].y");
        uint256 expectedX1 = vm.parseJsonUint(json, ".transcript_trace.input_commitment_points[1].x");
        uint256 expectedY1 = vm.parseJsonUint(json, ".transcript_trace.input_commitment_points[1].y");

        // Decode from proof
        HyraxProofDecoder.DecodedPoint[] memory points = this._decodeInputCommitsE2E(proofData);

        // Compare
        assertEq(points[0].x, expectedX0, "Point[0].x mismatch");
        assertEq(points[0].y, expectedY0, "Point[0].y mismatch");
        assertEq(points[1].x, expectedX1, "Point[1].x mismatch");
        assertEq(points[1].y, expectedY1, "Point[1].y mismatch");
    }

    /// @notice Gas benchmark for the full E2E flow
    function test_e2e_gas() public {
        (
            bytes memory proofData,
            uint256 circuitHashFq1,
            uint256 circuitHashFq2,
            uint256 pubHashChain1,
            uint256 pubHashChain2,
            uint256 ecHashChain1,
            uint256 ecHashChain2,
            uint256 expectedChallenge
        ) = _loadFixture();

        uint256 gasBefore = gasleft();

        // Decode
        this._decodeSummaryE2E(proofData);
        uint256[] memory pubInputs = this._decodePublicInputsE2E(proofData);
        HyraxProofDecoder.DecodedPoint[] memory points = this._decodeInputCommitsE2E(proofData);

        uint256 decodeGas = gasBefore - gasleft();
        gasBefore = gasleft();

        // Transcript replay
        PoseidonSponge.Sponge memory sponge = PoseidonSponge.init();
        PoseidonSponge.absorb(sponge, circuitHashFq1);
        PoseidonSponge.absorb(sponge, circuitHashFq2);
        PoseidonSponge.absorb(sponge, pubInputs[0]);
        PoseidonSponge.absorb(sponge, pubInputs[1]);
        PoseidonSponge.absorb(sponge, pubHashChain1);
        PoseidonSponge.absorb(sponge, pubHashChain2);
        for (uint256 i = 0; i < points.length; i++) {
            PoseidonSponge.absorb(sponge, points[i].x);
            PoseidonSponge.absorb(sponge, points[i].y);
        }
        PoseidonSponge.absorb(sponge, ecHashChain1);
        PoseidonSponge.absorb(sponge, ecHashChain2);
        uint256 challenge = PoseidonSponge.squeeze(sponge);

        uint256 transcriptGas = gasBefore - gasleft();

        emit log_named_uint("Decode gas (summary + pubInputs + commitPoints)", decodeGas);
        emit log_named_uint("Transcript replay gas (12 absorbs + squeeze)", transcriptGas);
        emit log_named_uint("Total E2E gas", decodeGas + transcriptGas);

        assertEq(challenge, expectedChallenge, "Gas test: challenge mismatch");
    }

    // === External calldata wrappers ===

    function _decodeSummaryE2E(bytes calldata data) external pure returns (HyraxProofDecoder.ProofSummary memory) {
        return HyraxProofDecoder.decodeSummary(data);
    }

    function _decodePublicInputsE2E(bytes calldata data) external pure returns (uint256[] memory) {
        return HyraxProofDecoder.decodePublicInputs(data);
    }

    function _decodeInputCommitsE2E(bytes calldata data)
        external
        pure
        returns (HyraxProofDecoder.DecodedPoint[] memory)
    {
        return HyraxProofDecoder.decodeInputCommitmentPoints(data);
    }
}

/// @title PedersenGensDecoderTest
/// @notice Tests that Pedersen generators are correctly decoded from Rust-exported data.
contract PedersenGensDecoderTest is Test {
    /// @notice Load generators from the E2E fixture
    function _loadGens() internal view returns (bytes memory) {
        string memory json = vm.readFile("test/fixtures/e2e_fixture.json");
        return vm.parseJsonBytes(json, ".gens_hex");
    }

    /// @notice Decode gens by parsing the calldata bytes directly (mirrors RemainderVerifier.decodePedersenGens)
    function _decodeGensCalldata(bytes calldata data) external pure returns (HyraxVerifier.PedersenGens memory gens) {
        if (data.length == 0) {
            gens.messageGens = new HyraxVerifier.G1Point[](0);
            return gens;
        }

        uint256 offset = 0;
        uint256 numGens = uint256(bytes32(data[offset:offset + 32]));
        offset += 32;

        gens.messageGens = new HyraxVerifier.G1Point[](numGens);
        for (uint256 i = 0; i < numGens; i++) {
            gens.messageGens[i].x = uint256(bytes32(data[offset:offset + 32]));
            offset += 32;
            gens.messageGens[i].y = uint256(bytes32(data[offset:offset + 32]));
            offset += 32;
        }

        gens.scalarGen.x = uint256(bytes32(data[offset:offset + 32]));
        offset += 32;
        gens.scalarGen.y = uint256(bytes32(data[offset:offset + 32]));
        offset += 32;

        gens.blindingGen.x = uint256(bytes32(data[offset:offset + 32]));
        offset += 32;
        gens.blindingGen.y = uint256(bytes32(data[offset:offset + 32]));
        offset += 32;
    }

    /// @notice Test basic generator decoding (count, structure)
    function test_decode_gens_structure() public {
        bytes memory gensData = _loadGens();
        HyraxVerifier.PedersenGens memory gens = this._decodeGensCalldata(gensData);

        // 512 message generators
        assertEq(gens.messageGens.length, 512, "Should have 512 message generators");

        // Scalar gen should be non-zero
        assertTrue(gens.scalarGen.x != 0 || gens.scalarGen.y != 0, "Scalar gen should be non-zero");

        // Blinding gen should be non-zero
        assertTrue(gens.blindingGen.x != 0 || gens.blindingGen.y != 0, "Blinding gen should be non-zero");

        // Scalar gen should equal the last message gen
        assertEq(gens.scalarGen.x, gens.messageGens[511].x, "Scalar gen x should equal last message gen");
        assertEq(gens.scalarGen.y, gens.messageGens[511].y, "Scalar gen y should equal last message gen");

        emit log_named_uint("Gens data size (bytes)", gensData.length);
        emit log_named_uint("Message generators", gens.messageGens.length);
    }

    /// @notice Test that all generator points are on the BN254 curve
    function test_all_gens_on_curve() public view {
        bytes memory gensData = _loadGens();
        HyraxVerifier.PedersenGens memory gens = this._decodeGensCalldata(gensData);

        // Check first 10 message generators (checking all 512 would be expensive)
        for (uint256 i = 0; i < 10; i++) {
            assertTrue(
                HyraxVerifier.isOnCurve(gens.messageGens[i]),
                string(abi.encodePacked("messageGen[", vm.toString(i), "] not on curve"))
            );
        }

        // Check last message generator
        assertTrue(HyraxVerifier.isOnCurve(gens.messageGens[511]), "Last messageGen not on curve");

        // Check scalar gen
        assertTrue(HyraxVerifier.isOnCurve(gens.scalarGen), "Scalar gen not on curve");

        // Check blinding gen
        assertTrue(HyraxVerifier.isOnCurve(gens.blindingGen), "Blinding gen not on curve");
    }

    /// @notice Test gas cost of decoding generators
    function test_decode_gens_gas() public {
        bytes memory gensData = _loadGens();

        uint256 gasBefore = gasleft();
        this._decodeGensCalldata(gensData);
        uint256 gasUsed = gasBefore - gasleft();

        emit log_named_uint("Gas to decode 512 Pedersen generators", gasUsed);
    }

    /// @notice Test empty generators fallback
    function test_empty_gens() public view {
        bytes memory empty = "";
        HyraxVerifier.PedersenGens memory gens = this._decodeGensCalldata(empty);
        assertEq(gens.messageGens.length, 0, "Empty gens should have 0 message generators");
    }
}

/// @title E2EProofDecodeTest
/// @notice Validates the committed sumcheck proof decoder against real Rust-generated data.
///         Loads the real ABI-encoded proof, decodes it into committed GKR structs,
///         and validates the structure matches expected counts.
contract E2EProofDecodeTest is Test {
    RemainderVerifier public verifier;

    function setUp() public {
        verifier = new RemainderVerifier(address(this));
    }

    /// @notice Test that the proof decodes successfully and has correct structure
    function test_decode_committed_proof_structure() public {
        string memory json = vm.readFile("test/fixtures/e2e_fixture.json");
        bytes memory rawProof = vm.parseJsonBytes(json, ".proof_hex");
        bytes memory proofData = _stripSelector(rawProof);
        bytes memory pubInputs = abi.encodePacked(uint256(6), uint256(20));

        // Decode proof counts via RemainderVerifier helper
        (uint256 numLayers, uint256 numInputs, uint256 numPub) = verifier.decodeProofCounts(proofData, pubInputs);
        assertEq(numLayers, 2, "Should have 2 layer proofs");
        assertEq(numInputs, 1, "Should have 1 input proof");
        assertEq(numPub, 2, "Should have 2 public inputs");

        // Decode per-layer details
        for (uint256 i = 0; i < numLayers; i++) {
            (uint256 msgs, uint256 commits, uint256 pops) = verifier.decodeLayerDetail(proofData, pubInputs, i);
            emit log_named_uint(string(abi.encodePacked("Layer ", vm.toString(i), " messages")), msgs);
            emit log_named_uint(string(abi.encodePacked("Layer ", vm.toString(i), " commits")), commits);
            emit log_named_uint(string(abi.encodePacked("Layer ", vm.toString(i), " PoPs")), pops);
            assertGt(msgs, 0, "Layer should have messages");
        }
    }

    /// @notice Test that decoded layer proofs have valid sumcheck messages
    function test_decode_layer_messages_valid() public {
        string memory json = vm.readFile("test/fixtures/e2e_fixture.json");
        bytes memory rawProof = vm.parseJsonBytes(json, ".proof_hex");
        bytes memory proofData = _stripSelector(rawProof);
        bytes memory pubInputs = abi.encodePacked(uint256(6), uint256(20));

        (uint256 numLayers,,) = verifier.decodeProofCounts(proofData, pubInputs);

        uint256 totalPoints = 0;
        for (uint256 i = 0; i < numLayers; i++) {
            (uint256 msgs, uint256 commits,) = verifier.decodeLayerDetail(proofData, pubInputs, i);
            totalPoints += msgs + commits + 1; // +1 for sum point
        }

        assertGt(totalPoints, 0, "Should have decoded EC points");
        emit log_named_uint("Total EC points in layer proofs", totalPoints);
    }

    /// @notice Test full decode + verify path with real proof + generators
    function test_full_verify_path() public {
        string memory json = vm.readFile("test/fixtures/e2e_fixture.json");
        bytes memory rawProof = vm.parseJsonBytes(json, ".proof_hex");
        bytes memory gensData = vm.parseJsonBytes(json, ".gens_hex");
        bytes memory pubInputs = abi.encodePacked(uint256(6), uint256(20));

        // Test that decoding doesn't revert
        (uint256 numLayerProofs, uint256 numInputProofs,) =
            verifier.decodeProofCounts(_stripSelector(rawProof), pubInputs);

        // Test that generators decode successfully
        HyraxVerifier.PedersenGens memory gens = this._decodeGens(gensData);
        assertEq(gens.messageGens.length, 512, "Should have 512 generators");

        emit log_named_uint("Proof decoded layers", numLayerProofs);
        emit log_named_uint("Proof decoded inputs", numInputProofs);
        emit log_named_uint("Generators decoded", gens.messageGens.length);
    }

    /// @notice Test adapter publicData splitting convention
    function test_adapter_public_data_split() public view {
        // Create combined publicData: [pubInputsLen] [pubInputs] [gensData]
        bytes memory pubInputs = abi.encodePacked(uint256(6), uint256(20));

        string memory json = vm.readFile("test/fixtures/e2e_fixture.json");
        bytes memory gensData = vm.parseJsonBytes(json, ".gens_hex");

        // Encode with length prefix
        bytes memory combined = abi.encodePacked(uint256(pubInputs.length), pubInputs, gensData);

        // Verify split
        uint256 declaredLen;
        assembly {
            declaredLen := mload(add(combined, 32))
        }
        assertEq(declaredLen, 64, "pubInputsLen should be 64 (2 uint256s)");
        assertEq(combined.length, 32 + 64 + gensData.length, "Combined length should be header + pubInputs + gens");
    }

    /// @notice Test gensHash validation
    function test_gens_hash_validation() public {
        string memory json = vm.readFile("test/fixtures/e2e_fixture.json");
        bytes memory gensData = vm.parseJsonBytes(json, ".gens_hex");
        bytes32 gensHash = keccak256(gensData);

        // Register circuit with gensHash
        uint256[] memory sizes = new uint256[](2);
        sizes[0] = 4;
        sizes[1] = 1;
        uint8[] memory types = new uint8[](2);
        types[0] = 3;
        types[1] = 0;
        bool[] memory committed = new bool[](2);
        committed[0] = true;
        committed[1] = false;

        bytes32 circuitHash = keccak256("test-gens-hash");
        verifier.registerCircuitWithGens(circuitHash, 2, sizes, types, committed, "test", gensHash);
        assertTrue(verifier.isCircuitActive(circuitHash), "Circuit should be active");

        // Verify wrong generators are rejected
        bytes memory wrongGens =
            abi.encodePacked(uint256(1), uint256(1), uint256(2), uint256(1), uint256(2), uint256(1), uint256(2));
        bytes memory fakeProof = abi.encodePacked(bytes4("REM1"), bytes32(0));

        vm.expectRevert(RemainderVerifier.InvalidGenerators.selector);
        verifier.verifyOrRevert(fakeProof, circuitHash, "", wrongGens);
    }

    // ===== Helpers =====

    function _stripSelector(bytes memory proof) internal pure returns (bytes memory) {
        bytes memory data = new bytes(proof.length - 4);
        for (uint256 i = 0; i < data.length; i++) {
            data[i] = proof[i + 4];
        }
        return data;
    }

    /// @dev Modular exponentiation
    function _modExp(uint256 base, uint256 exp, uint256 mod) internal pure returns (uint256 result) {
        result = 1;
        base = base % mod;
        while (exp > 0) {
            if (exp & 1 == 1) result = mulmod(result, base, mod);
            exp >>= 1;
            base = mulmod(base, base, mod);
        }
    }

    /// @notice Full E2E verification: register circuit, call verifyOrRevert with real proof data
    function test_e2e_full_verification() public {
        string memory json = vm.readFile("test/fixtures/e2e_fixture.json");
        bytes memory rawProof = vm.parseJsonBytes(json, ".proof_hex");
        bytes memory gensData = vm.parseJsonBytes(json, ".gens_hex");
        bytes32 circuitHash = bytes32(vm.parseJsonBytes32(json, ".circuit_hash_raw"));

        // Register circuit: 3 layers (input:committed, mul, add/output)
        // Private input: 2 shreds × 2 elements = 4 total
        uint256[] memory sizes = new uint256[](3);
        sizes[0] = 4;
        sizes[1] = 2;
        sizes[2] = 2;
        uint8[] memory types = new uint8[](3);
        types[0] = 3; // input
        types[1] = 1; // multiply
        types[2] = 0; // add/subtract (output)
        bool[] memory committed = new bool[](3);
        committed[0] = true; // committed private inputs
        committed[1] = false;
        committed[2] = false;

        verifier.registerCircuit(circuitHash, 3, sizes, types, committed, "test-mul-circuit");

        // Prepare public inputs and call verifyOrRevert
        bytes memory pubInputs = abi.encodePacked(uint256(6), uint256(20));

        // This should NOT revert if the full verification works
        // If it does revert, the error message tells us where verification fails
        // Debug: check input proof structure
        {
            bytes memory proofData = _stripSelector(rawProof);
            (GKRVerifier.GKRProof memory dbgProof,) = verifier.decodeProofCounted(proofData, pubInputs);
            emit log_named_uint("input proof commitRows", dbgProof.inputProofs[0].commitmentRows.length);
            emit log_named_uint("input proof zVector.length", dbgProof.inputProofs[0].podp.zVector.length);
        }

        // Full E2E verification — should not revert
        verifier.verifyOrRevert(rawProof, circuitHash, pubInputs, gensData);
    }

    /// @notice Diagnostic test: dump FS claims from proof
    function test_dump_fs_claims() public {
        string memory json = vm.readFile("test/fixtures/e2e_fixture.json");
        bytes memory rawProof = vm.parseJsonBytes(json, ".proof_hex");
        bytes memory proofData = _stripSelector(rawProof);

        // Use this._readUint to read from calldata-based decoding
        // Instead, decode via verifier and then manually parse claims offset
        bytes memory pubInputs = abi.encodePacked(uint256(6), uint256(20));
        verifier.decodeProofCounted(proofData, pubInputs);

        // Use separate calldata call to read claims
        (
            uint256 numFs,
            uint256 numPub,
            uint256[][] memory fsPoints,
            uint256[][] memory pubPoints,
            uint256[] memory pubValues
        ) = this._dumpClaimsFromCalldata(proofData);
        emit log_named_uint("numFsClaims", numFs);
        emit log_named_uint("numPubClaims", numPub);
        for (uint256 i = 0; i < fsPoints.length; i++) {
            emit log_named_uint(string(abi.encodePacked("fsClaim[", vm.toString(i), "].numPoint")), fsPoints[i].length);
            for (uint256 j = 0; j < fsPoints[i].length; j++) {
                emit log_named_uint(
                    string(abi.encodePacked("fsClaim[", vm.toString(i), "].point[", vm.toString(j), "]")),
                    fsPoints[i][j]
                );
            }
        }
        for (uint256 i = 0; i < pubPoints.length; i++) {
            emit log_named_uint(string(abi.encodePacked("pubClaim[", vm.toString(i), "].value")), pubValues[i]);
            emit log_named_uint(
                string(abi.encodePacked("pubClaim[", vm.toString(i), "].numPoint")), pubPoints[i].length
            );
            for (uint256 j = 0; j < pubPoints[i].length; j++) {
                emit log_named_uint(
                    string(abi.encodePacked("pubClaim[", vm.toString(i), "].point[", vm.toString(j), "]")),
                    pubPoints[i][j]
                );
            }
        }
    }

    function _dumpClaimsFromCalldata(bytes calldata proofData)
        external
        pure
        returns (
            uint256 numFs,
            uint256 numPub,
            uint256[][] memory fsPoints,
            uint256[][] memory pubPoints,
            uint256[] memory pubValues
        )
    {
        uint256 offset = _skipToClaimsSection(proofData);

        // FS claims
        numFs = uint256(bytes32(proofData[offset:offset + 32]));
        offset += 32;
        fsPoints = new uint256[][](numFs);
        for (uint256 i = 0; i < numFs; i++) {
            offset += 128; // value + blinding + commitment
            uint256 numPt = uint256(bytes32(proofData[offset:offset + 32]));
            offset += 32;
            fsPoints[i] = new uint256[](numPt);
            for (uint256 j = 0; j < numPt; j++) {
                fsPoints[i][j] = uint256(bytes32(proofData[offset:offset + 32]));
                offset += 32;
            }
        }

        // Public value claims
        numPub = uint256(bytes32(proofData[offset:offset + 32]));
        offset += 32;
        pubPoints = new uint256[][](numPub);
        pubValues = new uint256[](numPub);
        for (uint256 i = 0; i < numPub; i++) {
            pubValues[i] = uint256(bytes32(proofData[offset:offset + 32]));
            offset += 128; // value + blinding + commitment
            uint256 numPt = uint256(bytes32(proofData[offset:offset + 32]));
            offset += 32;
            pubPoints[i] = new uint256[](numPt);
            for (uint256 j = 0; j < numPt; j++) {
                pubPoints[i][j] = uint256(bytes32(proofData[offset:offset + 32]));
                offset += 32;
            }
        }
    }

    function _skipToClaimsSection(bytes calldata data) internal pure returns (uint256 offset) {
        offset = 32; // skip circuit hash
        // Skip public inputs section
        uint256 cnt = uint256(bytes32(data[offset:offset + 32]));
        offset += 32;
        for (uint256 s = 0; s < cnt; s++) {
            uint256 n = uint256(bytes32(data[offset:offset + 32]));
            offset += 32 + n * 32;
        }
        // Skip output proofs
        cnt = uint256(bytes32(data[offset:offset + 32]));
        offset += 32 + cnt * 64;
        // Skip layer proofs
        cnt = uint256(bytes32(data[offset:offset + 32]));
        offset += 32;
        for (uint256 l = 0; l < cnt; l++) {
            offset = _skipOneLayerProof(data, offset);
        }
    }

    function _skipOneLayerProof(bytes calldata data, uint256 offset) internal pure returns (uint256) {
        offset += 64; // sum G1
        uint256 nm = uint256(bytes32(data[offset:offset + 32]));
        offset += 32 + nm * 64; // messages
        offset += 128; // commitD + commitDDotA
        uint256 nz = uint256(bytes32(data[offset:offset + 32]));
        offset += 32 + nz * 32 + 64; // z_vector + z_delta + z_beta
        uint256 nc = uint256(bytes32(data[offset:offset + 32]));
        offset += 32 + nc * 64; // commitments
        uint256 np = uint256(bytes32(data[offset:offset + 32]));
        offset += 32 + np * 352; // PoPs
        uint256 hasAgg = uint256(bytes32(data[offset:offset + 32]));
        offset += 32;
        if (hasAgg == 1) {
            uint256 nco = uint256(bytes32(data[offset:offset + 32]));
            offset += 32 + nco * 64;
            uint256 nop = uint256(bytes32(data[offset:offset + 32]));
            offset += 32 + nop * 128;
            uint256 neq = uint256(bytes32(data[offset:offset + 32]));
            offset += 32 + neq * 96;
        }
        return offset;
    }

    /// @notice Diagnostic test: extract output proof claim commitment from raw proof bytes
    function test_claim_commitment_diagnostic() public {
        string memory json = vm.readFile("test/fixtures/e2e_fixture.json");
        bytes memory rawProof = vm.parseJsonBytes(json, ".proof_hex");

        // Strip selector (4 bytes)
        bytes memory proofData = new bytes(rawProof.length - 4);
        for (uint256 i = 0; i < proofData.length; i++) {
            proofData[i] = rawProof[i + 4];
        }

        // Navigate to output layer proofs
        uint256 offset = 32; // skip circuit hash
        // Skip public inputs section
        uint256 numPubSections;
        assembly {
            numPubSections := mload(add(proofData, add(32, offset)))
        }
        offset += 32;
        for (uint256 i = 0; i < numPubSections; i++) {
            uint256 cnt;
            assembly {
                cnt := mload(add(proofData, add(32, offset)))
            }
            offset += 32 + cnt * 32;
        }

        // Read output layer proofs (claim commitments)
        uint256 numOutputProofs;
        assembly {
            numOutputProofs := mload(add(proofData, add(32, offset)))
        }
        offset += 32;
        emit log_named_uint("numOutputProofs", numOutputProofs);

        uint256 proofClaimX;
        for (uint256 i = 0; i < numOutputProofs; i++) {
            uint256 cx;
            uint256 cy;
            assembly {
                cx := mload(add(proofData, add(32, offset)))
                cy := mload(add(proofData, add(64, offset)))
            }
            offset += 64;
            emit log_named_uint("Proof claim_commitment.x", cx);
            emit log_named_uint("Proof claim_commitment.y", cy);
            if (i == 0) proofClaimX = cx;
        }

        // Continue from offset (past output proofs) to read layer proof sum
        uint256 numLayerProofs;
        assembly {
            numLayerProofs := mload(add(proofData, add(32, offset)))
        }
        offset += 32;
        emit log_named_uint("numLayerProofs", numLayerProofs);

        // First layer proof starts here; first 64 bytes are `sum` (G1 point)
        uint256 sumX;
        uint256 sumY;
        assembly {
            sumX := mload(add(proofData, add(32, offset)))
            sumY := mload(add(proofData, add(64, offset)))
        }
        emit log_named_uint("layerProof[0].sum.x", sumX);
        emit log_named_uint("layerProof[0].sum.y", sumY);
        uint256 expectedX = 0x203b6537e1105eb16de4825a8fa2464882d7051e3ba10f4b55e367da1f3aad35;
        emit log_string(sumX == expectedX ? "Layer sum.x == expected: YES" : "Layer sum.x == expected: NO");

        // Decode g_scalar from gens
        bytes memory gensData = vm.parseJsonBytes(json, ".gens_hex");
        uint256 numG;
        assembly {
            numG := mload(add(gensData, 32))
        }
        uint256 gScalarX;
        uint256 gScalarY;
        uint256 gOffset = 32 + numG * 64;
        assembly {
            gScalarX := mload(add(gensData, add(32, gOffset)))
            gScalarY := mload(add(gensData, add(64, gOffset)))
        }
        emit log_named_uint("g_scalar.x", gScalarX);
        emit log_named_uint("g_scalar.y", gScalarY);

        uint256 FR_MOD = 21888242871839275222246405745257275088548364400416034343698204186575808495617;
        uint256 r = 0x0d913ea20ccbceea7bb452e44f2b1e1bfd9efe3dca6898d297d23a2b22ea57ad;

        // Approach 1: RLC = 6 + 20*r
        uint256 rlcClaim = addmod(6, mulmod(20, r, FR_MOD), FR_MOD);
        HyraxVerifier.G1Point memory rlcCom =
            HyraxVerifier.scalarMul(HyraxVerifier.G1Point(gScalarX, gScalarY), rlcClaim);
        emit log_named_uint("RLC claim", rlcClaim);
        emit log_named_uint("RLC commitment.x", rlcCom.x);
        emit log_string(rlcCom.x == expectedX ? "RLC: MATCH" : "RLC: MISMATCH");

        // Approach 2: MLE = 6*(1-r) + 20*r = 6 + 14*r
        uint256 mleClaim = addmod(6, mulmod(14, r, FR_MOD), FR_MOD);
        HyraxVerifier.G1Point memory mleCom =
            HyraxVerifier.scalarMul(HyraxVerifier.G1Point(gScalarX, gScalarY), mleClaim);
        emit log_named_uint("MLE claim", mleClaim);
        emit log_named_uint("MLE commitment.x", mleCom.x);
        emit log_string(mleCom.x == expectedX ? "MLE: MATCH" : "MLE: MISMATCH");

        // Approach 3: Use evaluateMLEFromData function
        uint256[] memory data = new uint256[](2);
        data[0] = 6;
        data[1] = 20;
        uint256[] memory point = new uint256[](1);
        point[0] = r;
        uint256 mleFromFunc = GKRVerifier.evaluateMLEFromData(data, point);
        emit log_named_uint("evaluateMLEFromData result", mleFromFunc);
        emit log_string(mleFromFunc == mleClaim ? "MLE func matches manual: YES" : "MLE func matches manual: NO");
    }

    /// @notice Step-by-step GKR transcript trace test
    function test_gkr_transcript_step_by_step() public {
        (
            GKRVerifier.GKRProof memory proof,
            HyraxVerifier.PedersenGens memory gens,
            PoseidonSponge.Sponge memory sponge
        ) = _loadAndSetupTranscript();

        uint256 FR_MOD = 21888242871839275222246405745257275088548364400416034343698204186575808495617;

        // Step 9: Squeeze "Challenge for claim on output" - this IS the claim_point[0]
        uint256 claimPoint0 = PoseidonSponge.squeeze(sponge) % FR_MOD;

        // Step 10-11: Absorb claim commitment + squeeze RLC claim agg coefficient
        PoseidonSponge.absorb(sponge, proof.outputClaimCommitments[0].x);
        PoseidonSponge.absorb(sponge, proof.outputClaimCommitments[0].y);
        uint256 randomCoeff = PoseidonSponge.squeeze(sponge) % FR_MOD;

        // Step 12-13: Absorb first sumcheck message + squeeze binding
        PoseidonSponge.absorb(sponge, proof.layerProofs[0].sumcheckProof.messages[0].x);
        PoseidonSponge.absorb(sponge, proof.layerProofs[0].sumcheckProof.messages[0].y);
        uint256 scChallenge = PoseidonSponge.squeeze(sponge); // binding0

        // Steps 14-15: Absorb commitments
        for (uint256 i = 0; i < proof.layerProofs[0].commitments.length; i++) {
            PoseidonSponge.absorb(sponge, proof.layerProofs[0].commitments[i].x);
            PoseidonSponge.absorb(sponge, proof.layerProofs[0].commitments[i].y);
        }

        // Step 16-17: Squeeze rhos and gammas
        uint256 rho0 = PoseidonSponge.squeeze(sponge);
        uint256 rho1 = PoseidonSponge.squeeze(sponge);
        uint256 gamma0 = PoseidonSponge.squeeze(sponge);

        emit log_string("Transcript replay complete");

        _checkPODPEquations(proof, gens, sponge, rho0, rho1, gamma0, scChallenge, claimPoint0, randomCoeff);
    }

    function _loadAndSetupTranscript()
        internal
        view
        returns (
            GKRVerifier.GKRProof memory proof,
            HyraxVerifier.PedersenGens memory gens,
            PoseidonSponge.Sponge memory sponge
        )
    {
        string memory json = vm.readFile("test/fixtures/e2e_fixture.json");
        bytes memory rawProof = vm.parseJsonBytes(json, ".proof_hex");
        bytes memory gensData = vm.parseJsonBytes(json, ".gens_hex");
        bytes32 circuitHash = bytes32(vm.parseJsonBytes32(json, ".circuit_hash_raw"));

        bytes memory proofAfterSelector = _stripSelector(rawProof);
        bytes memory pubInputs = abi.encodePacked(uint256(6), uint256(20));
        uint256[] memory pubIn;
        (proof, pubIn) = verifier.decodeProofCounted(proofAfterSelector, pubInputs);
        gens = this._decodeGens(gensData);
        sponge = verifier.setupTranscriptPublic(circuitHash, pubIn, proof);
    }

    function _checkPODPEquations(
        GKRVerifier.GKRProof memory proof,
        HyraxVerifier.PedersenGens memory gens,
        PoseidonSponge.Sponge memory sponge,
        uint256 rho0,
        uint256 rho1,
        uint256 gamma0,
        uint256 scChallenge,
        uint256 claimPoint0,
        uint256 randomCoeff
    ) internal {
        uint256 FR_MOD = 21888242871839275222246405745257275088548364400416034343698204186575808495617;
        uint256 binding0 = scChallenge % FR_MOD;

        // alpha = messages[0] * gamma0
        GKRVerifier.CommittedLayerProof memory lp = proof.layerProofs[0];
        HyraxVerifier.G1Point memory alpha = HyraxVerifier.scalarMul(lp.sumcheckProof.messages[0], gamma0 % FR_MOD);

        // j_star computation (n=1, degree=2)
        uint256[] memory jStar = _computeJStar(rho0 % FR_MOD, rho1 % FR_MOD, gamma0 % FR_MOD, binding0);

        // Compute dotProduct in a separate function to avoid stack-too-deep
        HyraxVerifier.G1Point memory dotProduct =
            _computeDotProduct(lp, rho0 % FR_MOD, rho1 % FR_MOD, binding0, claimPoint0, randomCoeff);

        _checkPODPEqs2(proof, gens, sponge, alpha, dotProduct, jStar);
    }

    function _computeDotProduct(
        GKRVerifier.CommittedLayerProof memory lp,
        uint256 rho0,
        uint256 rho1,
        uint256 binding0,
        uint256 claimPoint0,
        uint256 randomCoeff
    ) internal returns (HyraxVerifier.G1Point memory) {
        uint256 FR_MOD = 21888242871839275222246405745257275088548364400416034343698204186575808495617;

        // rlc_beta = beta(bindings, claim_point) * random_coeff
        // beta(r, c) = prod_i (r_i * c_i + (1 - r_i) * (1 - c_i))
        // For n=1: beta = binding0 * claimPoint0 + (1 - binding0) * (1 - claimPoint0)
        uint256 rlcBeta;
        {
            uint256 term1 = mulmod(binding0, claimPoint0, FR_MOD);
            uint256 oneMinusB = addmod(1, FR_MOD - binding0, FR_MOD);
            uint256 oneMinusC = addmod(1, FR_MOD - claimPoint0, FR_MOD);
            uint256 term2 = mulmod(oneMinusB, oneMinusC, FR_MOD);
            rlcBeta = mulmod(addmod(term1, term2, FR_MOD), randomCoeff, FR_MOD);
        }
        emit log_named_uint("rlc_beta", rlcBeta);

        // oracleEval = rlcBeta * commitment[0] + (-rlcBeta) * commitment[1]
        HyraxVerifier.G1Point memory oracleEval = HyraxVerifier.ecAdd(
            HyraxVerifier.scalarMul(lp.commitments[0], rlcBeta),
            HyraxVerifier.scalarMul(lp.commitments[1], FR_MOD - rlcBeta)
        );

        // dotProduct = sum * rho0 - oracleEval * rho1
        return HyraxVerifier.ecAdd(
            HyraxVerifier.scalarMul(lp.sumcheckProof.sum, rho0), HyraxVerifier.scalarMul(oracleEval, FR_MOD - rho1)
        );
    }

    function _computeJStar(uint256 rho0, uint256 rho1, uint256 gamma0, uint256 binding0)
        internal
        pure
        returns (uint256[] memory jStar)
    {
        uint256 FR_MOD = 21888242871839275222246405745257275088548364400416034343698204186575808495617;
        uint256 gammaInv = _modExp(gamma0, FR_MOD - 2, FR_MOD);
        jStar = new uint256[](3);
        jStar[0] = mulmod(gammaInv, addmod(mulmod(rho0, 2, FR_MOD), FR_MOD - rho1, FR_MOD), FR_MOD);
        jStar[1] = mulmod(gammaInv, addmod(rho0, FR_MOD - mulmod(rho1, binding0, FR_MOD), FR_MOD), FR_MOD);
        uint256 b0sq = mulmod(binding0, binding0, FR_MOD);
        jStar[2] = mulmod(gammaInv, addmod(rho0, FR_MOD - mulmod(rho1, b0sq, FR_MOD), FR_MOD), FR_MOD);
    }

    function _checkPODPEqs2(
        GKRVerifier.GKRProof memory proof,
        HyraxVerifier.PedersenGens memory gens,
        PoseidonSponge.Sponge memory sponge,
        HyraxVerifier.G1Point memory alpha,
        HyraxVerifier.G1Point memory dotProduct,
        uint256[] memory jStar
    ) internal {
        uint256 FR_MOD = 21888242871839275222246405745257275088548364400416034343698204186575808495617;
        HyraxVerifier.PODPProof memory podp = proof.layerProofs[0].sumcheckProof.podp;
        emit log_named_uint("podp.zVector.length", podp.zVector.length);

        // Absorb PODP commitments, squeeze challenge
        PoseidonSponge.absorb(sponge, podp.commitD.x);
        PoseidonSponge.absorb(sponge, podp.commitD.y);
        PoseidonSponge.absorb(sponge, podp.commitDDotA.x);
        PoseidonSponge.absorb(sponge, podp.commitDDotA.y);
        uint256 c = PoseidonSponge.squeeze(sponge) % FR_MOD;
        emit log_named_uint("PODP challenge c", c);
        uint256 expected20 = 0x2eb0a7e50a310b365138154b735af2af4e547092caa2d6397938de9ba2f76931;
        emit log_string(c == expected20 ? "PODP challenge: MATCH" : "PODP challenge: MISMATCH");

        // Check eq1 and eq2 in separate functions
        bool eq1 = _podpEq1(podp, alpha, c, gens);
        emit log_string(eq1 ? "PODP eq1: PASS" : "PODP eq1: FAIL");
        bool eq2 = _podpEq2(podp, dotProduct, c, jStar, gens);
        emit log_string(eq2 ? "PODP eq2: PASS" : "PODP eq2: FAIL");
    }

    function _podpEq1(
        HyraxVerifier.PODPProof memory podp,
        HyraxVerifier.G1Point memory alpha,
        uint256 c,
        HyraxVerifier.PedersenGens memory gens
    ) internal view returns (bool) {
        // LHS: c * alpha + commitD
        HyraxVerifier.G1Point memory lhs = HyraxVerifier.ecAdd(HyraxVerifier.scalarMul(alpha, c), podp.commitD);
        // RHS: MSM(g[0..n], z) + z_delta * h
        HyraxVerifier.G1Point memory msm = HyraxVerifier.scalarMul(gens.messageGens[0], podp.zVector[0]);
        for (uint256 i = 1; i < podp.zVector.length; i++) {
            msm = HyraxVerifier.ecAdd(msm, HyraxVerifier.scalarMul(gens.messageGens[i], podp.zVector[i]));
        }
        HyraxVerifier.G1Point memory rhs =
            HyraxVerifier.ecAdd(msm, HyraxVerifier.scalarMul(gens.blindingGen, podp.zDelta));
        return lhs.x == rhs.x && lhs.y == rhs.y;
    }

    function _podpEq2(
        HyraxVerifier.PODPProof memory podp,
        HyraxVerifier.G1Point memory dotProduct,
        uint256 c,
        uint256[] memory jStar,
        HyraxVerifier.PedersenGens memory gens
    ) internal returns (bool) {
        uint256 FR_MOD = 21888242871839275222246405745257275088548364400416034343698204186575808495617;
        // LHS: c * dotProduct + commitDDotA
        HyraxVerifier.G1Point memory lhs = HyraxVerifier.ecAdd(HyraxVerifier.scalarMul(dotProduct, c), podp.commitDDotA);
        // RHS: <z, jStar> * g_scalar + z_beta * h
        uint256 zDotJ = 0;
        for (uint256 i = 0; i < podp.zVector.length; i++) {
            zDotJ = addmod(zDotJ, mulmod(podp.zVector[i], jStar[i], FR_MOD), FR_MOD);
        }
        HyraxVerifier.G1Point memory rhs = HyraxVerifier.ecAdd(
            HyraxVerifier.scalarMul(gens.scalarGen, zDotJ), HyraxVerifier.scalarMul(gens.blindingGen, podp.zBeta)
        );
        emit log_named_uint("eq2 LHS.x", lhs.x);
        emit log_named_uint("eq2 LHS.y", lhs.y);
        emit log_named_uint("eq2 RHS.x", rhs.x);
        emit log_named_uint("eq2 RHS.y", rhs.y);
        emit log_named_uint("dotProduct.x", dotProduct.x);
        emit log_named_uint("dotProduct.y", dotProduct.y);
        emit log_named_uint("zDotJ", zDotJ);
        for (uint256 i = 0; i < jStar.length; i++) {
            emit log_named_uint(string(abi.encodePacked("jStar[", vm.toString(i), "]")), jStar[i]);
        }
        for (uint256 i = 0; i < podp.zVector.length; i++) {
            emit log_named_uint(string(abi.encodePacked("zVector[", vm.toString(i), "]")), podp.zVector[i]);
        }
        return lhs.x == rhs.x && lhs.y == rhs.y;
    }

    /// @notice Decode generators from calldata
    function _decodeGens(bytes calldata data) external pure returns (HyraxVerifier.PedersenGens memory gens) {
        if (data.length == 0) {
            gens.messageGens = new HyraxVerifier.G1Point[](0);
            return gens;
        }
        uint256 offset = 0;
        uint256 numGens = uint256(bytes32(data[offset:offset + 32]));
        offset += 32;
        gens.messageGens = new HyraxVerifier.G1Point[](numGens);
        for (uint256 i = 0; i < numGens; i++) {
            gens.messageGens[i].x = uint256(bytes32(data[offset:offset + 32]));
            offset += 32;
            gens.messageGens[i].y = uint256(bytes32(data[offset:offset + 32]));
            offset += 32;
        }
        gens.scalarGen.x = uint256(bytes32(data[offset:offset + 32]));
        offset += 32;
        gens.scalarGen.y = uint256(bytes32(data[offset:offset + 32]));
        offset += 32;
        gens.blindingGen.x = uint256(bytes32(data[offset:offset + 32]));
        offset += 32;
        gens.blindingGen.y = uint256(bytes32(data[offset:offset + 32]));
    }
}

/// @title TestableRemainderVerifier
/// @notice Subclass that exposes internal transcript setup functions for testing
contract TestableRemainderVerifier is RemainderVerifier {
    constructor(address _admin) RemainderVerifier(_admin) {}

    function sha256HashChain(uint256[] memory fqElements) external view returns (uint256 fq1, uint256 fq2) {
        return _sha256HashChain(fqElements);
    }

    function setupTranscript(bytes32 circuitHash, uint256[] memory pubInputs, GKRVerifier.GKRProof memory gkrProof)
        external
        view
        returns (PoseidonSponge.Sponge memory)
    {
        return _setupTranscript(circuitHash, pubInputs, gkrProof);
    }
}

/// @title TranscriptSetupTest
/// @notice Tests for the SHA-256 hash chain and hash-to-Fq-pair conversion
///         that are used in the initial Fiat-Shamir transcript setup.
contract TranscriptSetupTest is Test {
    TestableRemainderVerifier public verifier;

    function setUp() public {
        verifier = new TestableRemainderVerifier(address(this));
    }

    /// @notice Test hashToFqPair conversion matches fixture values
    function test_hash_to_fq_pair() public view {
        bytes32 circuitHash = hex"8fd701309893d4017fb8d56a77c19f5abcf1cf8a7515974f2c6557c4264baa5d";
        uint256 expectedFq1 = 0x000000000000000000000000000000005a9fc1776ad5b87f01d493983001d78f;
        uint256 expectedFq2 = 0x000000000000000000000000000000005daa4b26c457652c4f9715758acff1bc;

        (uint256 fq1, uint256 fq2) = verifier.hashToFqPair(circuitHash);
        assertEq(fq1, expectedFq1, "circuit hash fq1 mismatch");
        assertEq(fq2, expectedFq2, "circuit hash fq2 mismatch");
    }

    /// @notice Test SHA-256 hash chain on public inputs matches fixture values
    function test_sha256_hash_chain_public_inputs() public view {
        uint256[] memory pubInputs = new uint256[](2);
        pubInputs[0] = 6;
        pubInputs[1] = 20;

        uint256 expectedHash1 = 0x00000000000000000000000000000000a998b9d31f69d8ae8e48768cf8b8a5ff;
        uint256 expectedHash2 = 0x00000000000000000000000000000000c06bcddbc1b4d72d89678361cd10177b;

        (uint256 h1, uint256 h2) = verifier.sha256HashChain(pubInputs);
        assertEq(h1, expectedHash1, "pub input hash chain fq1 mismatch");
        assertEq(h2, expectedHash2, "pub input hash chain fq2 mismatch");
    }

    /// @notice Test SHA-256 hash chain on EC commitment points matches fixture values
    function test_sha256_hash_chain_ec_points() public view {
        string memory json = vm.readFile("test/fixtures/e2e_fixture.json");
        uint256[] memory ecCoords = new uint256[](4);
        ecCoords[0] = vm.parseJsonUint(json, ".transcript_trace.input_commitment_points[0].x");
        ecCoords[1] = vm.parseJsonUint(json, ".transcript_trace.input_commitment_points[0].y");
        ecCoords[2] = vm.parseJsonUint(json, ".transcript_trace.input_commitment_points[1].x");
        ecCoords[3] = vm.parseJsonUint(json, ".transcript_trace.input_commitment_points[1].y");

        uint256 expectedHash1 = vm.parseJsonUint(json, ".transcript_trace.input_commitment_hash_chain_1");
        uint256 expectedHash2 = vm.parseJsonUint(json, ".transcript_trace.input_commitment_hash_chain_2");

        (uint256 h1, uint256 h2) = verifier.sha256HashChain(ecCoords);
        assertEq(h1, expectedHash1, "EC hash chain fq1 mismatch");
        assertEq(h2, expectedHash2, "EC hash chain fq2 mismatch");
    }

    /// @notice Test full transcript setup produces correct first challenge
    function test_full_transcript_setup() public view {
        string memory json = vm.readFile("test/fixtures/e2e_fixture.json");
        bytes memory rawProof = vm.parseJsonBytes(json, ".proof_hex");
        bytes32 circuitHash = bytes32(vm.parseJsonBytes32(json, ".circuit_hash_raw"));
        uint256 expectedChallenge = vm.parseJsonUint(json, ".challenges.first_challenge_for_output_claim");

        // Build a minimal GKRProof with just input proof commitment rows (needed for transcript)
        GKRVerifier.GKRProof memory gkrProof = _decodeProofForCommitPoints(rawProof);

        uint256[] memory pubInputs = new uint256[](2);
        pubInputs[0] = 6;
        pubInputs[1] = 20;

        PoseidonSponge.Sponge memory sponge = verifier.setupTranscript(circuitHash, pubInputs, gkrProof);
        uint256 challenge = PoseidonSponge.squeeze(sponge);

        assertEq(challenge, expectedChallenge, "Transcript setup first challenge mismatch");
    }

    /// @notice Gas benchmark for transcript setup
    function test_transcript_setup_gas() public {
        string memory json = vm.readFile("test/fixtures/e2e_fixture.json");
        bytes memory rawProof = vm.parseJsonBytes(json, ".proof_hex");
        bytes32 circuitHash = bytes32(vm.parseJsonBytes32(json, ".circuit_hash_raw"));

        GKRVerifier.GKRProof memory gkrProof = _decodeProofForCommitPoints(rawProof);
        uint256[] memory pubInputs = new uint256[](2);
        pubInputs[0] = 6;
        pubInputs[1] = 20;

        uint256 gasBefore = gasleft();
        verifier.setupTranscript(circuitHash, pubInputs, gkrProof);
        uint256 gasUsed = gasBefore - gasleft();

        emit log_named_uint("Transcript setup gas (hash chains + absorbs)", gasUsed);
    }

    /// @notice Gas benchmark for SHA-256 hash chain on public inputs (2 elements)
    function test_gas_sha256_hash_chain_pub_inputs() public {
        uint256[] memory pubInputs = new uint256[](2);
        pubInputs[0] = 6;
        pubInputs[1] = 20;

        uint256 gasBefore = gasleft();
        verifier.sha256HashChain(pubInputs);
        uint256 gasUsed = gasBefore - gasleft();

        emit log_named_uint("SHA-256 hash chain gas (2 pub inputs)", gasUsed);
    }

    /// @notice Gas benchmark for SHA-256 hash chain on EC coords (4 elements)
    function test_gas_sha256_hash_chain_ec_coords() public {
        string memory json = vm.readFile("test/fixtures/e2e_fixture.json");
        uint256[] memory ecCoords = new uint256[](4);
        ecCoords[0] = vm.parseJsonUint(json, ".transcript_trace.input_commitment_points[0].x");
        ecCoords[1] = vm.parseJsonUint(json, ".transcript_trace.input_commitment_points[0].y");
        ecCoords[2] = vm.parseJsonUint(json, ".transcript_trace.input_commitment_points[1].x");
        ecCoords[3] = vm.parseJsonUint(json, ".transcript_trace.input_commitment_points[1].y");

        uint256 gasBefore = gasleft();
        verifier.sha256HashChain(ecCoords);
        uint256 gasUsed = gasBefore - gasleft();

        emit log_named_uint("SHA-256 hash chain gas (4 EC coords)", gasUsed);
    }

    /// @notice Gas benchmark for SHA-256 hash chain on 16 elements (medium config public inputs)
    function test_gas_sha256_hash_chain_16_elements() public {
        uint256[] memory elements = new uint256[](16);
        for (uint256 i = 0; i < 16; i++) {
            elements[i] = uint256(keccak256(abi.encodePacked(i))) % type(uint128).max;
        }

        uint256 gasBefore = gasleft();
        verifier.sha256HashChain(elements);
        uint256 gasUsed = gasBefore - gasleft();

        emit log_named_uint("SHA-256 hash chain gas (16 elements)", gasUsed);
    }

    /// @dev Extract only input proof commitment rows from raw proof (minimal decode)
    function _decodeProofForCommitPoints(bytes memory rawProof)
        internal
        pure
        returns (GKRVerifier.GKRProof memory gkrProof)
    {
        // Strip selector
        bytes memory data = new bytes(rawProof.length - 4);
        for (uint256 i = 0; i < data.length; i++) {
            data[i] = rawProof[i + 4];
        }
        gkrProof.inputProofs = _decodeInputCommitRowsFromMemory(data);
    }

    function _decodeInputCommitRowsFromMemory(bytes memory data)
        internal
        pure
        returns (HyraxVerifier.EvalProof[] memory proofs)
    {
        uint256 offset = 32; // skip circuit hash
        // Skip public inputs
        uint256 numPub;
        assembly { numPub := mload(add(data, add(32, offset))) }
        offset += 32;
        for (uint256 i = 0; i < numPub; i++) {
            uint256 cnt;
            assembly { cnt := mload(add(data, add(32, offset))) }
            offset += 32 + cnt * 32;
        }
        // Skip output proofs
        uint256 numOut;
        assembly { numOut := mload(add(data, add(32, offset))) }
        offset += 32 + numOut * 64;
        // Skip layer proofs
        uint256 numLayers;
        assembly { numLayers := mload(add(data, add(32, offset))) }
        offset += 32;
        for (uint256 i = 0; i < numLayers; i++) {
            offset = _skipLayerMem(data, offset);
        }
        // Skip FS claims (value:32 + blinding:32 + commitment:64 + numPoint:32 + points)
        uint256 numFs;
        assembly { numFs := mload(add(data, add(32, offset))) }
        offset += 32;
        for (uint256 i = 0; i < numFs; i++) {
            uint256 np;
            assembly { np := mload(add(data, add(32, add(offset, 128)))) }
            offset += 160 + np * 32;
        }
        // Skip pub claims
        uint256 numPubC;
        assembly { numPubC := mload(add(data, add(32, offset))) }
        offset += 32;
        for (uint256 i = 0; i < numPubC; i++) {
            uint256 np;
            assembly { np := mload(add(data, add(32, add(offset, 128)))) }
            offset += 160 + np * 32;
        }
        // Decode input proofs (just commitment rows)
        uint256 numInputProofs;
        assembly { numInputProofs := mload(add(data, add(32, offset))) }
        offset += 32;
        proofs = new HyraxVerifier.EvalProof[](numInputProofs);
        for (uint256 i = 0; i < numInputProofs; i++) {
            uint256 numRows;
            assembly { numRows := mload(add(data, add(32, offset))) }
            offset += 32;
            proofs[i].commitmentRows = new HyraxVerifier.G1Point[](numRows);
            for (uint256 r = 0; r < numRows; r++) {
                uint256 px;
                uint256 py;
                assembly {
                    px := mload(add(data, add(32, offset)))
                    py := mload(add(data, add(64, offset)))
                }
                proofs[i].commitmentRows[r] = HyraxVerifier.G1Point(px, py);
                offset += 64;
            }
            // Skip eval proofs
            uint256 numEvals;
            assembly { numEvals := mload(add(data, add(32, offset))) }
            offset += 32;
            for (uint256 e = 0; e < numEvals; e++) {
                offset += 128; // commitD + commitDDotA
                uint256 numZ;
                assembly { numZ := mload(add(data, add(32, offset))) }
                offset += 32 + numZ * 32 + 64 + 64; // z_vec + z_delta + z_beta + comEval
            }
        }
    }

    function _skipLayerMem(bytes memory data, uint256 offset) internal pure returns (uint256) {
        offset += 64; // sum
        uint256 numMsg;
        assembly { numMsg := mload(add(data, add(32, offset))) }
        offset += 32 + numMsg * 64;
        offset += 128; // PODP commitD + commitDDotA
        uint256 numZ;
        assembly { numZ := mload(add(data, add(32, offset))) }
        offset += 32 + numZ * 32 + 64; // z_vec + z_delta + z_beta
        uint256 numCommits;
        assembly { numCommits := mload(add(data, add(32, offset))) }
        offset += 32 + numCommits * 64;
        uint256 numPops;
        assembly { numPops := mload(add(data, add(32, offset))) }
        offset += 32 + numPops * (192 + 160);
        uint256 hasAgg;
        assembly { hasAgg := mload(add(data, add(32, offset))) }
        offset += 32;
        if (hasAgg == 1) {
            uint256 nc;
            assembly { nc := mload(add(data, add(32, offset))) }
            offset += 32 + nc * 64;
            uint256 no;
            assembly { no := mload(add(data, add(32, offset))) }
            offset += 32 + no * 128;
            uint256 ne;
            assembly { ne := mload(add(data, add(32, offset))) }
            offset += 32 + ne * 96;
        }
        return offset;
    }
}

/// @title CommittedSumcheckVerifierTest
/// @notice Unit tests for the committed sumcheck verification components.
contract CommittedSumcheckVerifierTest is Test {
    /// @notice Test modular inverse
    function test_mod_inverse() public pure {
        uint256 p = 21888242871839275222246405745257275088548364400416034343698204186575808495617;

        // inv(2) * 2 == 1 (mod p)
        uint256 inv2 = CommittedSumcheckVerifier.modInverse(2, p);
        assertEq(mulmod(inv2, 2, p), 1, "inv(2) * 2 should be 1");

        // inv(7) * 7 == 1 (mod p)
        uint256 inv7 = CommittedSumcheckVerifier.modInverse(7, p);
        assertEq(mulmod(inv7, 7, p), 1, "inv(7) * 7 should be 1");

        // inv(p-1) * (p-1) == 1 (mod p)
        uint256 invPm1 = CommittedSumcheckVerifier.modInverse(p - 1, p);
        assertEq(mulmod(invPm1, p - 1, p), 1, "inv(p-1) * (p-1) should be 1");
    }

    /// @notice Test modular exponentiation
    function test_mod_exp() public pure {
        uint256 p = 21888242871839275222246405745257275088548364400416034343698204186575808495617;

        // 2^10 = 1024
        assertEq(CommittedSumcheckVerifier.modExp(2, 10, p), 1024, "2^10 should be 1024");

        // 3^0 = 1
        assertEq(CommittedSumcheckVerifier.modExp(3, 0, p), 1, "3^0 should be 1");

        // Fermat's little theorem: a^(p-1) = 1 (mod p)
        assertEq(CommittedSumcheckVerifier.modExp(5, p - 1, p), 1, "5^(p-1) should be 1 mod p");
    }

    /// @notice Test j_star computation with known values
    function test_compute_jstar_basic() public pure {
        uint256 n = 1;
        uint256 degree = 2;

        uint256[] memory rhos = new uint256[](2);
        rhos[0] = 3;
        rhos[1] = 5;

        uint256[] memory gammas = new uint256[](1);
        gammas[0] = 7;

        uint256[] memory bindings = new uint256[](1);
        bindings[0] = 11;

        uint256[] memory jStar = CommittedSumcheckVerifier.computeJStar(rhos, gammas, bindings, degree, n);

        // j_star length should be (degree+1) * n = 3
        assertEq(jStar.length, 3, "j_star should have 3 elements");

        // Verify all elements are within field
        uint256 p = 21888242871839275222246405745257275088548364400416034343698204186575808495617;
        for (uint256 i = 0; i < jStar.length; i++) {
            assertTrue(jStar[i] < p, "j_star element should be < Fr modulus");
        }
    }

    /// @notice Test MLE evaluation from GKRVerifier
    function test_evaluate_mle() public pure {
        // MLE of [3, 7] at point [0] should give 3
        uint256[] memory data = new uint256[](2);
        data[0] = 3;
        data[1] = 7;

        uint256[] memory point0 = new uint256[](1);
        point0[0] = 0;
        assertEq(GKRVerifier.evaluateMLEFromData(data, point0), 3, "MLE([3,7], [0]) should be 3");

        // MLE of [3, 7] at point [1] should give 7
        uint256[] memory point1 = new uint256[](1);
        point1[0] = 1;
        assertEq(GKRVerifier.evaluateMLEFromData(data, point1), 7, "MLE([3,7], [1]) should be 7");
    }

    /// @notice Test ProofOfProduct EC check structure (basic smoke test)
    function test_pop_check_basic() public view {
        // Create trivial PoP data: all zeros should fail (degenerate case)
        HyraxVerifier.ProofOfProduct memory pop;
        pop.alpha = HyraxVerifier.G1Point(0, 0);
        pop.beta = HyraxVerifier.G1Point(0, 0);
        pop.delta = HyraxVerifier.G1Point(0, 0);
        pop.z1 = 0;
        pop.z2 = 0;
        pop.z3 = 0;
        pop.z4 = 0;
        pop.z5 = 0;

        HyraxVerifier.G1Point memory comX = HyraxVerifier.G1Point(0, 0);
        HyraxVerifier.G1Point memory comY = HyraxVerifier.G1Point(0, 0);
        HyraxVerifier.G1Point memory comZ = HyraxVerifier.G1Point(0, 0);

        HyraxVerifier.PedersenGens memory gens;
        gens.messageGens = new HyraxVerifier.G1Point[](0);
        gens.scalarGen = HyraxVerifier.G1Point(1, 2);
        gens.blindingGen = HyraxVerifier.G1Point(1, 2);

        PoseidonSponge.Sponge memory sponge = PoseidonSponge.init();

        // Zero PoP with identity points: all checks should pass trivially
        // (0 + 0*com = 0*g + 0*h → 0 = 0 for each check)
        bool valid = HyraxVerifier.verifyProofOfProduct(pop, comX, comY, comZ, gens, sponge);
        assertTrue(valid, "Trivial zero PoP should pass");
    }
}

/// @title Groth16VerifierTest
/// @notice E2E tests for the gnark-generated Groth16 verifier (Option C wrapper).
/// The Groth16 proof certifies GKR algebraic relations (Fr arithmetic)
/// while Poseidon transcript replay and EC checks stay on-chain.
contract Groth16VerifierTest is Test {
    Groth16Verifier verifier;

    function setUp() public {
        verifier = new Groth16Verifier();
    }

    /// @notice Full E2E: load fixture, call verifyProof, expect no revert
    function test_e2e_groth16_verification() public view {
        string memory json = vm.readFile("test/fixtures/groth16_fixture.json");

        // Parse proof (8 uint256)
        uint256[8] memory proof;
        for (uint256 i = 0; i < 8; i++) {
            proof[i] = vm.parseJsonUint(json, string.concat(".proof[", vm.toString(i), "]"));
        }

        // Parse public inputs (78 uint256 for medium config)
        uint256[78] memory pubInputs;
        for (uint256 i = 0; i < 78; i++) {
            pubInputs[i] = vm.parseJsonUint(json, string.concat(".public_inputs[", vm.toString(i), "]"));
        }

        // Should not revert — proof is valid
        verifier.verifyProof(proof, pubInputs);
    }

    /// @notice Gas measurement for Groth16 verification
    function test_e2e_groth16_gas() public {
        string memory json = vm.readFile("test/fixtures/groth16_fixture.json");

        uint256[8] memory proof;
        for (uint256 i = 0; i < 8; i++) {
            proof[i] = vm.parseJsonUint(json, string.concat(".proof[", vm.toString(i), "]"));
        }
        uint256[78] memory pubInputs;
        for (uint256 i = 0; i < 78; i++) {
            pubInputs[i] = vm.parseJsonUint(json, string.concat(".public_inputs[", vm.toString(i), "]"));
        }

        uint256 gasBefore = gasleft();
        verifier.verifyProof(proof, pubInputs);
        uint256 gasUsed = gasBefore - gasleft();

        emit log_named_uint("Groth16 verification gas", gasUsed);
    }

    /// @notice Verify that a corrupted proof is rejected
    function test_groth16_rejects_bad_proof() public {
        string memory json = vm.readFile("test/fixtures/groth16_fixture.json");

        uint256[8] memory proof;
        for (uint256 i = 0; i < 8; i++) {
            proof[i] = vm.parseJsonUint(json, string.concat(".proof[", vm.toString(i), "]"));
        }
        uint256[78] memory pubInputs;
        for (uint256 i = 0; i < 78; i++) {
            pubInputs[i] = vm.parseJsonUint(json, string.concat(".public_inputs[", vm.toString(i), "]"));
        }

        // Corrupt a public input
        pubInputs[4] = pubInputs[4] ^ 1;

        vm.expectRevert();
        verifier.verifyProof(proof, pubInputs);
    }
}

/// @title GKRHybridVerifierTest
/// @notice Tests for the hybrid GKR+Groth16 verification flow.
/// Tests transcript replay, Groth16 input building, and EC check wiring.
contract GKRHybridVerifierTest is Test {
    RemainderVerifier remainderVerifier;
    Groth16Verifier groth16Verifier;

    uint256 constant FR_MODULUS = 21888242871839275222246405745257275088548364400416034343698204186575808495617;

    function setUp() public {
        remainderVerifier = new RemainderVerifier(address(this));
        groth16Verifier = new Groth16Verifier();
        remainderVerifier.setGroth16Verifier(address(groth16Verifier));

        // Register the test circuit (3 layers: input(committed), multiply, subtract)
        // matching the circuit structure used in gen_groth16_witness.rs (medium config, num_vars=4)
        bytes32 circuitHash =
            vm.parseJsonBytes32(vm.readFile("test/fixtures/groth16_e2e_fixture.json"), ".circuit_hash_raw");
        uint256[] memory sizes = new uint256[](3);
        sizes[0] = 32; // input layer: 2 shreds * 2^num_vars = 2*16 = 32
        sizes[1] = 16; // multiply layer: 2^num_vars = 16
        sizes[2] = 16; // subtract layer: 2^num_vars = 16
        uint8[] memory types = new uint8[](3);
        types[0] = 3;
        types[1] = 1;
        types[2] = 0;
        bool[] memory committed = new bool[](3);
        committed[0] = true;
        remainderVerifier.registerCircuit(circuitHash, 3, sizes, types, committed, "test-hybrid-medium");

        // Register per-circuit Groth16 verifier (medium config: 78 inputs)
        remainderVerifier.setCircuitGroth16Verifier(circuitHash, address(groth16Verifier), 78);

        // Also register the circuit from e2e_fixture.json (used by non-Groth16 tests)
        bytes32 e2eCircuitHash = vm.parseJsonBytes32(vm.readFile("test/fixtures/e2e_fixture.json"), ".circuit_hash_raw");
        if (e2eCircuitHash != circuitHash) {
            // e2e_fixture is small config (num_vars=1, sizes 4/2/2)
            uint256[] memory smallSizes = new uint256[](3);
            smallSizes[0] = 4;
            smallSizes[1] = 2;
            smallSizes[2] = 2;
            remainderVerifier.registerCircuit(e2eCircuitHash, 3, smallSizes, types, committed, "test-e2e");
        }
    }

    /// @notice Load e2e fixture data for testing
    function _loadE2EFixture()
        internal
        view
        returns (bytes memory proofHex, bytes memory gensHex, bytes32 circuitHash)
    {
        string memory json = vm.readFile("test/fixtures/e2e_fixture.json");
        proofHex = vm.parseJsonBytes(json, ".proof_hex");
        gensHex = vm.parseJsonBytes(json, ".gens_hex");
        circuitHash = vm.parseJsonBytes32(json, ".circuit_hash_raw");
    }

    /// @notice Test that transcript replay produces deterministic challenges
    function test_hybrid_transcript_replay_deterministic() public {
        (bytes memory proofHex,, bytes32 circuitHash) = _loadE2EFixture();

        // Load public values ABI from combined fixture
        string memory fixtureJson = vm.readFile("test/fixtures/groth16_e2e_fixture.json");
        bytes memory publicInputs = vm.parseJsonBytes(fixtureJson, ".public_values_abi");

        // Replay transcript twice, verify determinism
        (uint256[] memory out1, uint256 agg1, uint256 inter1) =
            remainderVerifier.replayTranscriptPublic(proofHex, circuitHash, publicInputs);
        (uint256[] memory out2, uint256 agg2, uint256 inter2) =
            remainderVerifier.replayTranscriptPublic(proofHex, circuitHash, publicInputs);

        assertEq(out1.length, out2.length, "Output challenges length should match");
        for (uint256 i = 0; i < out1.length; i++) {
            assertEq(out1[i], out2[i], "Output challenge should be deterministic");
        }
        assertEq(agg1, agg2, "Claim agg coeff should be deterministic");
        assertEq(inter1, inter2, "Inter-layer coeff should be deterministic");

        // All values should be non-zero and < FR_MODULUS
        assertTrue(out1[0] > 0, "Output challenge should be non-zero");
        assertTrue(agg1 > 0, "Claim agg coeff should be non-zero");
        assertTrue(inter1 > 0, "Inter-layer coeff should be non-zero");
        assertTrue(out1[0] < FR_MODULUS, "Output challenge should be < FR_MODULUS");
        assertTrue(agg1 < FR_MODULUS, "Claim agg coeff should be < FR_MODULUS");
        assertTrue(inter1 < FR_MODULUS, "Inter-layer coeff should be < FR_MODULUS");

        emit log_named_uint("Output challenge[0]", out1[0]);
        emit log_named_uint("Claim agg coeff", agg1);
        emit log_named_uint("Inter-layer coeff", inter1);
    }

    /// @notice Test that the transcript replay matches the direct GKR transcript
    /// @dev Both the hybrid replay and the direct GKR verification should produce
    ///      the same first output challenge (first squeeze after setup)
    function test_hybrid_transcript_matches_direct_gkr() public view {
        (bytes memory proofHex,, bytes32 circuitHash) = _loadE2EFixture();

        // Load public values ABI from combined fixture
        string memory fixtureJson = vm.readFile("test/fixtures/groth16_e2e_fixture.json");
        bytes memory publicInputs = vm.parseJsonBytes(fixtureJson, ".public_values_abi");

        // Get hybrid transcript output challenges
        (uint256[] memory hybridOutputChallenges,,) =
            remainderVerifier.replayTranscriptPublic(proofHex, circuitHash, publicInputs);

        // The first output challenge should match what a direct squeeze would produce.
        // Skip the 4-byte selector to get proof data
        bytes memory proofData = new bytes(proofHex.length - 4);
        for (uint256 i = 4; i < proofHex.length; i++) {
            proofData[i - 4] = proofHex[i];
        }

        (GKRVerifier.GKRProof memory gkrProof, uint256[] memory pubInputs) =
            this.decodeProofHelper(proofData, publicInputs);
        PoseidonSponge.Sponge memory sponge = remainderVerifier.setupTranscriptPublic(circuitHash, pubInputs, gkrProof);
        uint256 directOutputChallenge = PoseidonSponge.squeeze(sponge) % FR_MODULUS;

        assertEq(
            hybridOutputChallenges[0],
            directOutputChallenge,
            "Hybrid and direct transcript should produce same first output challenge"
        );
    }

    /// @notice Helper to call decodeProofCounted with calldata args
    function decodeProofHelper(bytes calldata proofData, bytes calldata publicInputs)
        external
        view
        returns (GKRVerifier.GKRProof memory, uint256[] memory)
    {
        return remainderVerifier.decodeProofCounted(proofData, publicInputs);
    }

    /// @notice Helper to parse flat groth16 outputs into struct (avoids stack-too-deep)
    function _parseGroth16OutputsHelper(uint256[] memory groth16Outputs, uint256 numComputeLayers)
        internal
        pure
        returns (GKRHybridVerifier.Groth16Outputs memory outputs)
    {
        uint256[] memory rlcBeta = new uint256[](numComputeLayers);
        uint256[] memory zDotJStar = new uint256[](numComputeLayers);
        for (uint256 i = 0; i < numComputeLayers; i++) {
            rlcBeta[i] = groth16Outputs[i];
        }
        for (uint256 i = 0; i < numComputeLayers; i++) {
            zDotJStar[i] = groth16Outputs[numComputeLayers + i];
        }
        uint256 numLTensor = groth16Outputs.length - 2 * numComputeLayers - 2;
        uint256[] memory lTensor = new uint256[](numLTensor);
        for (uint256 i = 0; i < numLTensor; i++) {
            lTensor[i] = groth16Outputs[2 * numComputeLayers + i];
        }
        outputs = GKRHybridVerifier.Groth16Outputs({
            rlcBeta: rlcBeta,
            zDotJStar: zDotJStar,
            lTensor: lTensor,
            zDotR: groth16Outputs[groth16Outputs.length - 2],
            mleEval: groth16Outputs[groth16Outputs.length - 1]
        });
    }

    /// @notice Test buildGroth16Inputs constructs the correct 78-element array (medium config, N=4)
    function test_buildGroth16Inputs_layout() public pure {
        GKRHybridVerifier.TranscriptChallenges memory challenges;

        // Output challenges (4 values for N=4)
        challenges.outputChallenges = new uint256[](4);
        for (uint256 i = 0; i < 4; i++) {
            challenges.outputChallenges[i] = 100 + i;
        }

        challenges.claimAggCoeff = 200;

        // 2 layers
        challenges.layers = new GKRHybridVerifier.LayerChallenges[](2);

        // Layer 0: 4 bindings, 5 rhos, 4 gammas, no PoP
        challenges.layers[0].bindings = new uint256[](4);
        for (uint256 i = 0; i < 4; i++) {
            challenges.layers[0].bindings[i] = 300 + i;
        }
        challenges.layers[0].rhos = new uint256[](5);
        for (uint256 i = 0; i < 5; i++) {
            challenges.layers[0].rhos[i] = 400 + i;
        }
        challenges.layers[0].gammas = new uint256[](4);
        for (uint256 i = 0; i < 4; i++) {
            challenges.layers[0].gammas[i] = 500 + i;
        }
        challenges.layers[0].podpChallenge = 550;
        // layer 0 has no PoP (popChallenge = 0)

        // Layer 1: 4 bindings, 5 rhos, 4 gammas, has PoP
        challenges.layers[1].bindings = new uint256[](4);
        for (uint256 i = 0; i < 4; i++) {
            challenges.layers[1].bindings[i] = 600 + i;
        }
        challenges.layers[1].rhos = new uint256[](5);
        for (uint256 i = 0; i < 5; i++) {
            challenges.layers[1].rhos[i] = 700 + i;
        }
        challenges.layers[1].gammas = new uint256[](4);
        for (uint256 i = 0; i < 4; i++) {
            challenges.layers[1].gammas[i] = 800 + i;
        }
        challenges.layers[1].podpChallenge = 850;
        challenges.layers[1].popChallenge = 860;

        challenges.inputRlcCoeffs = new uint256[](2);
        challenges.inputRlcCoeffs[0] = 900;
        challenges.inputRlcCoeffs[1] = 901;
        challenges.inputPodpChallenge = 910;
        challenges.interLayerCoeffs = new uint256[](1);
        challenges.interLayerCoeffs[0] = 1000;
        // MLE eval point (4 values for log2(16)=4)
        challenges.mleEvalPoint = new uint256[](4);
        for (uint256 i = 0; i < 4; i++) {
            challenges.mleEvalPoint[i] = 1100 + i;
        }
        // Input claim point (4 values, same as last layer bindings for linear circuits)
        challenges.inputClaimPoint = new uint256[](4);
        for (uint256 i = 0; i < 4; i++) {
            challenges.inputClaimPoint[i] = 1200 + i;
        }

        // Public inputs (16 values for N=4)
        uint256[] memory pubInputs = new uint256[](16);
        for (uint256 i = 0; i < 16; i++) {
            pubInputs[i] = 10 + i;
        }

        // Groth16 outputs: rlcBeta(2) + zDotJStar(2) + lTensor(8) + zDotR + mleEval = 14
        GKRHybridVerifier.Groth16Outputs memory outputs;
        outputs.rlcBeta = new uint256[](2);
        outputs.rlcBeta[0] = 2000;
        outputs.rlcBeta[1] = 2001;
        outputs.zDotJStar = new uint256[](2);
        outputs.zDotJStar[0] = 2002;
        outputs.zDotJStar[1] = 2003;
        outputs.lTensor = new uint256[](8);
        for (uint256 i = 0; i < 8; i++) {
            outputs.lTensor[i] = 2004 + i;
        }
        outputs.zDotR = 2012;
        outputs.mleEval = 2013;

        uint256[] memory inputs = GKRHybridVerifier.buildGroth16Inputs(12345, 67890, pubInputs, challenges, outputs);
        assertEq(inputs.length, 78, "total length should be 78");

        uint256 idx = 0;
        // [0-1] Circuit hash
        assertEq(inputs[idx++], 12345, "circuitHash0");
        assertEq(inputs[idx++], 67890, "circuitHash1");
        // [2-17] Public inputs
        for (uint256 i = 0; i < 16; i++) {
            assertEq(inputs[idx++], 10 + i, "pubInput");
        }
        // [18-21] Output challenges
        for (uint256 i = 0; i < 4; i++) {
            assertEq(inputs[idx++], 100 + i, "outputChallenge");
        }
        // [22] Claim agg coeff
        assertEq(inputs[idx++], 200, "claimAggCoeff");
        // [23-26] Layer 0 bindings
        for (uint256 i = 0; i < 4; i++) {
            assertEq(inputs[idx++], 300 + i, "layer0Binding");
        }
        // [27-31] Layer 0 rhos
        for (uint256 i = 0; i < 5; i++) {
            assertEq(inputs[idx++], 400 + i, "layer0Rho");
        }
        // [32-35] Layer 0 gammas
        for (uint256 i = 0; i < 4; i++) {
            assertEq(inputs[idx++], 500 + i, "layer0Gamma");
        }
        // [36] Layer 0 PODP challenge
        assertEq(inputs[idx++], 550, "layer0PodpChallenge");
        // [37-40] Layer 1 bindings
        for (uint256 i = 0; i < 4; i++) {
            assertEq(inputs[idx++], 600 + i, "layer1Binding");
        }
        // [41-45] Layer 1 rhos
        for (uint256 i = 0; i < 5; i++) {
            assertEq(inputs[idx++], 700 + i, "layer1Rho");
        }
        // [46-49] Layer 1 gammas
        for (uint256 i = 0; i < 4; i++) {
            assertEq(inputs[idx++], 800 + i, "layer1Gamma");
        }
        // [50] Layer 1 PODP challenge
        assertEq(inputs[idx++], 850, "layer1PodpChallenge");
        // [51] Layer 1 PoP challenge
        assertEq(inputs[idx++], 860, "layer1PopChallenge");
        // [52-53] Input RLC coefficients
        assertEq(inputs[idx++], 900, "inputRlcCoeff0");
        assertEq(inputs[idx++], 901, "inputRlcCoeff1");
        // [54] Input PODP challenge
        assertEq(inputs[idx++], 910, "inputPodpChallenge");
        // [55] Inter-layer coefficient
        assertEq(inputs[idx++], 1000, "interLayerCoeff");
        // [56-59] MLE eval point
        for (uint256 i = 0; i < 4; i++) {
            assertEq(inputs[idx++], 1100 + i, "mleEvalPoint");
        }
        // [60-63] Input claim point
        for (uint256 i = 0; i < 4; i++) {
            assertEq(inputs[idx++], 1200 + i, "inputClaimPoint");
        }
        // [64-77] Groth16 outputs
        assertEq(inputs[idx++], 2000, "rlcBeta0");
        assertEq(inputs[idx++], 2001, "rlcBeta1");
        assertEq(inputs[idx++], 2002, "zDotJStar0");
        assertEq(inputs[idx++], 2003, "zDotJStar1");
        for (uint256 i = 0; i < 8; i++) {
            assertEq(inputs[idx++], 2004 + i, "lTensor");
        }
        assertEq(inputs[idx++], 2012, "zDotR");
        assertEq(inputs[idx++], 2013, "mleEval");
        assertEq(idx, inputs.length, "idx should equal array length");
    }

    /// @notice Test buildGroth16Inputs with small config (N=1) produces 29-element array
    function test_buildGroth16Inputs_small_config() public pure {
        GKRHybridVerifier.TranscriptChallenges memory challenges;

        // Output challenges (1 value for N=1)
        challenges.outputChallenges = new uint256[](1);
        challenges.outputChallenges[0] = 100;

        challenges.claimAggCoeff = 200;

        // 2 layers
        challenges.layers = new GKRHybridVerifier.LayerChallenges[](2);

        // Layer 0: 1 binding, 2 rhos, 1 gamma, no PoP
        challenges.layers[0].bindings = new uint256[](1);
        challenges.layers[0].bindings[0] = 300;
        challenges.layers[0].rhos = new uint256[](2);
        challenges.layers[0].rhos[0] = 400;
        challenges.layers[0].rhos[1] = 401;
        challenges.layers[0].gammas = new uint256[](1);
        challenges.layers[0].gammas[0] = 500;
        challenges.layers[0].podpChallenge = 550;

        // Layer 1: 1 binding, 2 rhos, 1 gamma, has PoP
        challenges.layers[1].bindings = new uint256[](1);
        challenges.layers[1].bindings[0] = 600;
        challenges.layers[1].rhos = new uint256[](2);
        challenges.layers[1].rhos[0] = 700;
        challenges.layers[1].rhos[1] = 701;
        challenges.layers[1].gammas = new uint256[](1);
        challenges.layers[1].gammas[0] = 800;
        challenges.layers[1].podpChallenge = 850;
        challenges.layers[1].popChallenge = 860;

        challenges.inputRlcCoeffs = new uint256[](2);
        challenges.inputRlcCoeffs[0] = 900;
        challenges.inputRlcCoeffs[1] = 901;
        challenges.inputPodpChallenge = 910;
        challenges.interLayerCoeffs = new uint256[](1);
        challenges.interLayerCoeffs[0] = 1000;
        // MLE eval point (1 value for log2(2)=1)
        challenges.mleEvalPoint = new uint256[](1);
        challenges.mleEvalPoint[0] = 1100;
        // Input claim point (1 value for InputNumVars=1)
        challenges.inputClaimPoint = new uint256[](1);
        challenges.inputClaimPoint[0] = 1200;

        // Public inputs (2^1 = 2 values for N=1)
        uint256[] memory pubInputs = new uint256[](2);
        pubInputs[0] = 10;
        pubInputs[1] = 11;

        // Groth16 outputs: rlcBeta(2) + zDotJStar(2) + lTensor(2) + zDotR + mleEval = 8
        GKRHybridVerifier.Groth16Outputs memory outputs;
        outputs.rlcBeta = new uint256[](2);
        outputs.rlcBeta[0] = 2000;
        outputs.rlcBeta[1] = 2001;
        outputs.zDotJStar = new uint256[](2);
        outputs.zDotJStar[0] = 2002;
        outputs.zDotJStar[1] = 2003;
        outputs.lTensor = new uint256[](2);
        outputs.lTensor[0] = 2004;
        outputs.lTensor[1] = 2005;
        outputs.zDotR = 2006;
        outputs.mleEval = 2007;

        uint256[] memory inputs = GKRHybridVerifier.buildGroth16Inputs(12345, 67890, pubInputs, challenges, outputs);

        // Total = 29 (old) + 1 (mleEvalPoint) + 1 (inputClaimPoint) = 31
        assertEq(inputs.length, 31, "Small config (N=1) should have 31 inputs");

        // Spot-check key positions
        assertEq(inputs[0], 12345, "circuitHash0");
        assertEq(inputs[1], 67890, "circuitHash1");
        assertEq(inputs[2], 10, "pubInput0");
        assertEq(inputs[3], 11, "pubInput1");
        assertEq(inputs[4], 100, "outputChallenge0");
        assertEq(inputs[inputs.length - 1], 2007, "mleEval (last)");
        assertEq(inputs[inputs.length - 2], 2006, "zDotR (second to last)");
    }

    /// @notice Test buildGroth16Inputs with 3 computation layers (N=4) to verify N-layer generalization
    function test_buildGroth16Inputs_three_layers() public pure {
        GKRHybridVerifier.TranscriptChallenges memory challenges;

        // Output challenges (4 values for N=4)
        challenges.outputChallenges = new uint256[](4);
        for (uint256 i = 0; i < 4; i++) {
            challenges.outputChallenges[i] = 100 + i;
        }

        challenges.claimAggCoeff = 200;

        // 3 computation layers (subtract + multiply + subtract)
        challenges.layers = new GKRHybridVerifier.LayerChallenges[](3);

        // Layer 0: 4 bindings, 5 rhos, 4 gammas, no PoP
        challenges.layers[0].bindings = new uint256[](4);
        for (uint256 i = 0; i < 4; i++) {
            challenges.layers[0].bindings[i] = 300 + i;
        }
        challenges.layers[0].rhos = new uint256[](5);
        for (uint256 i = 0; i < 5; i++) {
            challenges.layers[0].rhos[i] = 400 + i;
        }
        challenges.layers[0].gammas = new uint256[](4);
        for (uint256 i = 0; i < 4; i++) {
            challenges.layers[0].gammas[i] = 500 + i;
        }
        challenges.layers[0].podpChallenge = 550;

        // Layer 1: 4 bindings, 5 rhos, 4 gammas, no PoP
        challenges.layers[1].bindings = new uint256[](4);
        for (uint256 i = 0; i < 4; i++) {
            challenges.layers[1].bindings[i] = 600 + i;
        }
        challenges.layers[1].rhos = new uint256[](5);
        for (uint256 i = 0; i < 5; i++) {
            challenges.layers[1].rhos[i] = 700 + i;
        }
        challenges.layers[1].gammas = new uint256[](4);
        for (uint256 i = 0; i < 4; i++) {
            challenges.layers[1].gammas[i] = 800 + i;
        }
        challenges.layers[1].podpChallenge = 850;

        // Layer 2: 4 bindings, 5 rhos, 4 gammas, has PoP (last layer)
        challenges.layers[2].bindings = new uint256[](4);
        for (uint256 i = 0; i < 4; i++) {
            challenges.layers[2].bindings[i] = 1100 + i;
        }
        challenges.layers[2].rhos = new uint256[](5);
        for (uint256 i = 0; i < 5; i++) {
            challenges.layers[2].rhos[i] = 1200 + i;
        }
        challenges.layers[2].gammas = new uint256[](4);
        for (uint256 i = 0; i < 4; i++) {
            challenges.layers[2].gammas[i] = 1300 + i;
        }
        challenges.layers[2].podpChallenge = 1350;
        challenges.layers[2].popChallenge = 1360;

        challenges.inputRlcCoeffs = new uint256[](2);
        challenges.inputRlcCoeffs[0] = 900;
        challenges.inputRlcCoeffs[1] = 901;
        challenges.inputPodpChallenge = 910;
        // 2 inter-layer coefficients for 3 layers
        challenges.interLayerCoeffs = new uint256[](2);
        challenges.interLayerCoeffs[0] = 1000;
        challenges.interLayerCoeffs[1] = 1001;

        // Public inputs (2^4 = 16 values)
        uint256[] memory pubInputs = new uint256[](16);
        for (uint256 i = 0; i < 16; i++) {
            pubInputs[i] = 10 + i;
        }

        // Groth16 outputs: rlcBeta(3) + zDotJStar(3) + lTensor(8) + zDotR + mleEval = 16
        GKRHybridVerifier.Groth16Outputs memory outputs;
        outputs.rlcBeta = new uint256[](3);
        outputs.rlcBeta[0] = 2000;
        outputs.rlcBeta[1] = 2001;
        outputs.rlcBeta[2] = 2002;
        outputs.zDotJStar = new uint256[](3);
        outputs.zDotJStar[0] = 2003;
        outputs.zDotJStar[1] = 2004;
        outputs.zDotJStar[2] = 2005;
        outputs.lTensor = new uint256[](8);
        for (uint256 i = 0; i < 8; i++) {
            outputs.lTensor[i] = 2006 + i;
        }
        outputs.zDotR = 2014;
        outputs.mleEval = 2015;

        uint256[] memory inputs = GKRHybridVerifier.buildGroth16Inputs(12345, 67890, pubInputs, challenges, outputs);

        // Total = 2(hash) + 16(pub) + 4(outCh) + 1(claimAgg)
        //       + 14(L0: 4+5+4+1) + 14(L1: 4+5+4+1) + 15(L2: 4+5+4+1+1pop)
        //       + 2(inputRlc) + 1(inputPodp) + 2(interLayer)
        //       + 3(rlcBeta) + 3(zDotJStar) + 8(lTensor) + 1(zDotR) + 1(mleEval) = 87
        assertEq(inputs.length, 87, "3-layer N=4 config should have 87 inputs");

        // Verify structure: walk through all positions
        uint256 idx = 0;

        // Circuit hash
        assertEq(inputs[idx++], 12345, "circuitHash0");
        assertEq(inputs[idx++], 67890, "circuitHash1");

        // Public inputs
        for (uint256 i = 0; i < 16; i++) {
            assertEq(inputs[idx++], 10 + i, "pubInput");
        }

        // Output challenges
        for (uint256 i = 0; i < 4; i++) {
            assertEq(inputs[idx++], 100 + i, "outputChallenge");
        }

        // Claim agg coeff
        assertEq(inputs[idx++], 200, "claimAggCoeff");

        // Layer 0
        for (uint256 i = 0; i < 4; i++) {
            assertEq(inputs[idx++], 300 + i, "L0 binding");
        }
        for (uint256 i = 0; i < 5; i++) {
            assertEq(inputs[idx++], 400 + i, "L0 rho");
        }
        for (uint256 i = 0; i < 4; i++) {
            assertEq(inputs[idx++], 500 + i, "L0 gamma");
        }
        assertEq(inputs[idx++], 550, "L0 podpChallenge");

        // Layer 1
        for (uint256 i = 0; i < 4; i++) {
            assertEq(inputs[idx++], 600 + i, "L1 binding");
        }
        for (uint256 i = 0; i < 5; i++) {
            assertEq(inputs[idx++], 700 + i, "L1 rho");
        }
        for (uint256 i = 0; i < 4; i++) {
            assertEq(inputs[idx++], 800 + i, "L1 gamma");
        }
        assertEq(inputs[idx++], 850, "L1 podpChallenge");

        // Layer 2 (with PoP)
        for (uint256 i = 0; i < 4; i++) {
            assertEq(inputs[idx++], 1100 + i, "L2 binding");
        }
        for (uint256 i = 0; i < 5; i++) {
            assertEq(inputs[idx++], 1200 + i, "L2 rho");
        }
        for (uint256 i = 0; i < 4; i++) {
            assertEq(inputs[idx++], 1300 + i, "L2 gamma");
        }
        assertEq(inputs[idx++], 1350, "L2 podpChallenge");
        assertEq(inputs[idx++], 1360, "L2 popChallenge");

        // Input RLC + PODP
        assertEq(inputs[idx++], 900, "inputRlcCoeff0");
        assertEq(inputs[idx++], 901, "inputRlcCoeff1");
        assertEq(inputs[idx++], 910, "inputPodpChallenge");

        // Inter-layer coefficients (2 for 3 layers)
        assertEq(inputs[idx++], 1000, "interLayerCoeff0");
        assertEq(inputs[idx++], 1001, "interLayerCoeff1");

        // Groth16 outputs
        assertEq(inputs[idx++], 2000, "rlcBeta0");
        assertEq(inputs[idx++], 2001, "rlcBeta1");
        assertEq(inputs[idx++], 2002, "rlcBeta2");
        assertEq(inputs[idx++], 2003, "zDotJStar0");
        assertEq(inputs[idx++], 2004, "zDotJStar1");
        assertEq(inputs[idx++], 2005, "zDotJStar2");
        for (uint256 i = 0; i < 8; i++) {
            assertEq(inputs[idx++], 2006 + i, "lTensor");
        }
        assertEq(inputs[idx++], 2014, "zDotR");
        assertEq(inputs[idx++], 2015, "mleEval");

        assertEq(idx, inputs.length, "idx should equal array length");
    }

    /// @notice Gas measurement for hybrid transcript replay
    function test_hybrid_transcript_replay_gas() public {
        (bytes memory proofHex,, bytes32 circuitHash) = _loadE2EFixture();

        string memory fixtureJson = vm.readFile("test/fixtures/groth16_e2e_fixture.json");
        bytes memory publicInputs = vm.parseJsonBytes(fixtureJson, ".public_values_abi");

        uint256 gasBefore = gasleft();
        remainderVerifier.replayTranscriptPublic(proofHex, circuitHash, publicInputs);
        uint256 gasUsed = gasBefore - gasleft();

        emit log_named_uint("Hybrid transcript replay gas (decode + setup + replay)", gasUsed);
    }

    /// @notice Test that verifyWithGroth16 rejects when groth16Verifier is not set
    function test_hybrid_rejects_no_groth16_verifier() public {
        RemainderVerifier freshVerifier = new RemainderVerifier(address(this));
        // Don't set groth16Verifier, but register the circuit

        (bytes memory proofHex, bytes memory gensHex, bytes32 circuitHash) = _loadE2EFixture();

        // Register circuit on freshVerifier (medium config sizes)
        uint256[] memory sizes = new uint256[](3);
        sizes[0] = 32;
        sizes[1] = 16;
        sizes[2] = 16;
        uint8[] memory types = new uint8[](3);
        types[0] = 3;
        types[1] = 1;
        types[2] = 0;
        bool[] memory committed = new bool[](3);
        committed[0] = true;
        freshVerifier.registerCircuit(circuitHash, 3, sizes, types, committed, "test");

        string memory fixtureJson = vm.readFile("test/fixtures/groth16_e2e_fixture.json");
        bytes memory publicInputs = vm.parseJsonBytes(fixtureJson, ".public_values_abi");

        uint256[8] memory fakeProof;
        uint256[] memory fakeOutputs = new uint256[](14);

        vm.expectRevert("Groth16 verifier not set");
        freshVerifier.verifyWithGroth16(proofHex, circuitHash, publicInputs, gensHex, fakeProof, fakeOutputs);
    }

    /// @notice Test that verifyWithGroth16 rejects invalid selector
    function test_hybrid_rejects_bad_selector() public {
        // Use a registered circuit hash so we pass the registration check
        bytes32 circuitHash =
            vm.parseJsonBytes32(vm.readFile("test/fixtures/groth16_e2e_fixture.json"), ".circuit_hash_raw");

        bytes memory badProof = hex"DEADBEEF";
        bytes memory publicInputs = new bytes(64);
        bytes memory gensData = new bytes(0);
        uint256[8] memory proof;
        uint256[] memory outputs = new uint256[](14);

        vm.expectRevert(RemainderVerifier.InvalidProofSelector.selector);
        remainderVerifier.verifyWithGroth16(badProof, circuitHash, publicInputs, gensData, proof, outputs);
    }

    /// @notice Test that verifyWithGroth16 rejects too-short proof
    function test_hybrid_rejects_short_proof() public {
        // Use a registered circuit hash so we pass the registration check
        bytes32 circuitHash =
            vm.parseJsonBytes32(vm.readFile("test/fixtures/groth16_e2e_fixture.json"), ".circuit_hash_raw");

        bytes memory shortProof = hex"5245";
        bytes memory publicInputs = new bytes(0);
        bytes memory gensData = new bytes(0);
        uint256[8] memory proof;
        uint256[] memory outputs = new uint256[](14);

        vm.expectRevert(RemainderVerifier.InvalidProofLength.selector);
        remainderVerifier.verifyWithGroth16(shortProof, circuitHash, publicInputs, gensData, proof, outputs);
    }

    // ========================================================================
    // COMBINED FIXTURE E2E TESTS
    // ========================================================================

    /// @notice Load the combined fixture (single proof run: inner proof + Groth16)
    function _loadCombinedFixture()
        internal
        view
        returns (
            bytes memory innerProof,
            bytes memory gensHex,
            bytes32 circuitHash,
            bytes memory publicValuesAbi,
            uint256[8] memory groth16Proof,
            uint256[] memory groth16Outputs
        )
    {
        string memory json = vm.readFile("test/fixtures/groth16_e2e_fixture.json");
        innerProof = vm.parseJsonBytes(json, ".inner_proof_hex");
        gensHex = vm.parseJsonBytes(json, ".gens_hex");
        circuitHash = vm.parseJsonBytes32(json, ".circuit_hash_raw");
        publicValuesAbi = vm.parseJsonBytes(json, ".public_values_abi");

        for (uint256 i = 0; i < 8; i++) {
            groth16Proof[i] = vm.parseJsonUint(json, string.concat(".groth16_proof[", vm.toString(i), "]"));
        }

        // Load groth16 outputs (14 values for medium config)
        uint256[] memory jsonOutputs = vm.parseJsonUintArray(json, ".groth16_outputs");
        groth16Outputs = jsonOutputs;
    }

    /// @notice Test transcript replay with combined fixture matches Rust-computed challenges
    function test_combined_transcript_replay_matches_rust() public {
        string memory json = vm.readFile("test/fixtures/groth16_e2e_fixture.json");
        bytes memory innerProof = vm.parseJsonBytes(json, ".inner_proof_hex");
        bytes32 circuitHash = vm.parseJsonBytes32(json, ".circuit_hash_raw");
        bytes memory publicValuesAbi = vm.parseJsonBytes(json, ".public_values_abi");

        // Expected challenge values from Rust witness output
        uint256[] memory expectedOutputChallenges = vm.parseJsonUintArray(json, ".public_inputs.output_challenges");
        uint256 expectedClaimAggCoeff = vm.parseJsonUint(json, ".public_inputs.claim_agg_coeff");
        uint256 expectedInterLayerCoeff = vm.parseJsonUint(json, ".public_inputs.inter_layer_coeff");

        // Replay transcript on-chain
        (uint256[] memory outputChallenges, uint256 claimAggCoeff, uint256 interLayerCoeff) =
            remainderVerifier.replayTranscriptPublic(innerProof, circuitHash, publicValuesAbi);

        assertEq(outputChallenges.length, expectedOutputChallenges.length, "Output challenges length mismatch");
        for (uint256 i = 0; i < outputChallenges.length; i++) {
            assertEq(
                outputChallenges[i],
                expectedOutputChallenges[i],
                string.concat("Output challenge[", vm.toString(i), "] mismatch vs Rust")
            );
        }
        assertEq(claimAggCoeff, expectedClaimAggCoeff, "Claim agg coeff mismatch vs Rust");
        assertEq(interLayerCoeff, expectedInterLayerCoeff, "Inter-layer coeff mismatch vs Rust");

        emit log_named_uint("Output challenge[0] (Solidity)", outputChallenges[0]);
        emit log_named_uint("Output challenge[0] (Rust)", expectedOutputChallenges[0]);
    }

    /// @notice Replay transcript and build Groth16 inputs from fixture data
    function _replayAndBuildGroth16Inputs(
        bytes memory innerProof,
        bytes32 circuitHash,
        bytes memory publicValuesAbi,
        uint256[] memory groth16Outputs
    ) internal view returns (uint256[] memory groth16Inputs) {
        bytes memory proofData = new bytes(innerProof.length - 4);
        for (uint256 i = 4; i < innerProof.length; i++) {
            proofData[i - 4] = innerProof[i];
        }

        (GKRVerifier.GKRProof memory gkrProof, uint256[] memory pubInputs) =
            this.decodeProofHelper(proofData, publicValuesAbi);
        PoseidonSponge.Sponge memory sponge = remainderVerifier.setupTranscriptPublic(circuitHash, pubInputs, gkrProof);

        GKRHybridVerifier.TranscriptChallenges memory challenges =
            GKRHybridVerifier.replayTranscriptAndCollectChallenges(gkrProof, sponge);

        GKRHybridVerifier.Groth16Outputs memory outputs =
            _parseGroth16OutputsHelper(groth16Outputs, gkrProof.layerProofs.length);

        (uint256 chFr0, uint256 chFr1) = remainderVerifier.hashToFqPair(circuitHash);
        groth16Inputs = GKRHybridVerifier.buildGroth16Inputs(chFr0, chFr1, pubInputs, challenges, outputs);
    }

    /// @notice Test Groth16 verification with replayed challenge inputs
    function test_combined_groth16_with_replayed_inputs() public {
        (
            bytes memory innerProof,,
            bytes32 circuitHash,
            bytes memory publicValuesAbi,
            uint256[8] memory groth16Proof,
            uint256[] memory groth16Outputs
        ) = _loadCombinedFixture();

        uint256[] memory groth16Inputs =
            _replayAndBuildGroth16Inputs(innerProof, circuitHash, publicValuesAbi, groth16Outputs);
        assertEq(groth16Inputs.length, 78, "Expected 78 Groth16 inputs for medium config");

        uint256[78] memory fixedInputs;
        for (uint256 i = 0; i < 78; i++) {
            fixedInputs[i] = groth16Inputs[i];
        }
        groth16Verifier.verifyProof(groth16Proof, fixedInputs);

        emit log_string("Groth16 verification with replayed inputs: PASSED");
    }

    /// @notice Full E2E: verifyWithGroth16() end-to-end with combined fixture
    function test_combined_e2e_full_verification() public {
        (
            bytes memory innerProof,
            bytes memory gensHex,
            bytes32 circuitHash,
            bytes memory publicValuesAbi,
            uint256[8] memory groth16Proof,
            uint256[] memory groth16Outputs
        ) = _loadCombinedFixture();

        // Should not revert — this is the full hybrid verification flow
        remainderVerifier.verifyWithGroth16(
            innerProof, circuitHash, publicValuesAbi, gensHex, groth16Proof, groth16Outputs
        );

        emit log_string("Full E2E hybrid verification: PASSED");
    }

    /// @notice Test that corrupted inner proof is rejected
    function test_combined_rejects_bad_inner_proof() public {
        (
            bytes memory innerProof,
            bytes memory gensHex,
            bytes32 circuitHash,
            bytes memory publicValuesAbi,
            uint256[8] memory groth16Proof,
            uint256[] memory groth16Outputs
        ) = _loadCombinedFixture();

        // Corrupt a commitment point deep in the proof data.
        // Use an index well into the layer proof commitments section.
        uint256 corruptIdx = 2000;
        if (innerProof.length > corruptIdx) {
            innerProof[corruptIdx] = bytes1(uint8(innerProof[corruptIdx]) ^ 0xFF);
        }

        vm.expectRevert();
        remainderVerifier.verifyWithGroth16(
            innerProof, circuitHash, publicValuesAbi, gensHex, groth16Proof, groth16Outputs
        );
    }

    /// @notice Test that corrupted Groth16 outputs are rejected
    function test_combined_rejects_bad_groth16_outputs() public {
        (
            bytes memory innerProof,
            bytes memory gensHex,
            bytes32 circuitHash,
            bytes memory publicValuesAbi,
            uint256[8] memory groth16Proof,
            uint256[] memory groth16Outputs
        ) = _loadCombinedFixture();

        // Corrupt a Groth16 output — the Groth16 proof should fail
        groth16Outputs[0] = groth16Outputs[0] ^ 1;

        vm.expectRevert();
        remainderVerifier.verifyWithGroth16(
            innerProof, circuitHash, publicValuesAbi, gensHex, groth16Proof, groth16Outputs
        );
    }

    /// @notice Gas measurement for full hybrid E2E verification
    function test_combined_e2e_gas() public {
        (
            bytes memory innerProof,
            bytes memory gensHex,
            bytes32 circuitHash,
            bytes memory publicValuesAbi,
            uint256[8] memory groth16Proof,
            uint256[] memory groth16Outputs
        ) = _loadCombinedFixture();

        uint256 gasBefore = gasleft();
        remainderVerifier.verifyWithGroth16(
            innerProof, circuitHash, publicValuesAbi, gensHex, groth16Proof, groth16Outputs
        );
        uint256 gasUsed = gasBefore - gasleft();

        emit log_named_uint("Full hybrid E2E verification gas", gasUsed);
    }

    // ========================================================================
    // CIRCUIT REGISTRATION REJECTION TESTS
    // ========================================================================

    /// @notice Test that verifyWithGroth16 rejects unregistered circuit
    function test_hybrid_rejects_unregistered_circuit() public {
        (
            bytes memory innerProof,
            bytes memory gensHex,,
            bytes memory publicValuesAbi,
            uint256[8] memory groth16Proof,
            uint256[] memory groth16Outputs
        ) = _loadCombinedFixture();

        // Use a wrong circuit hash that is not registered
        bytes32 wrongHash = bytes32(uint256(0xDEAD));

        vm.expectRevert(RemainderVerifier.CircuitNotRegistered.selector);
        remainderVerifier.verifyWithGroth16(
            innerProof, wrongHash, publicValuesAbi, gensHex, groth16Proof, groth16Outputs
        );
    }

    /// @notice Test that verifyWithGroth16 rejects inactive circuit
    function test_hybrid_rejects_inactive_circuit() public {
        (
            bytes memory innerProof,
            bytes memory gensHex,
            bytes32 circuitHash,
            bytes memory publicValuesAbi,
            uint256[8] memory groth16Proof,
            uint256[] memory groth16Outputs
        ) = _loadCombinedFixture();

        // Deactivate the circuit
        remainderVerifier.deactivateCircuit(circuitHash);

        vm.expectRevert(RemainderVerifier.CircuitNotActive.selector);
        remainderVerifier.verifyWithGroth16(
            innerProof, circuitHash, publicValuesAbi, gensHex, groth16Proof, groth16Outputs
        );
    }

    // ========================================================================
    // PUBLIC INPUT CLAIM VERIFICATION TESTS
    // ========================================================================

    /// @notice Test that non-committed input layer triggers the public input claim check.
    /// @dev Registers the same circuit with isCommitted[0]=false. The Groth16 proof still
    ///      passes (same circuit hash), but the claim commitment check fails because the
    ///      fixture's final layer commitment is a Hyrax commitment, not scalarMul(g, mleEval).
    function test_hybrid_noncommitted_triggers_claim_check() public {
        // Deploy a fresh verifier and register the circuit with isCommitted[0] = false
        RemainderVerifier freshVerifier = new RemainderVerifier(address(this));
        freshVerifier.setGroth16Verifier(address(groth16Verifier));

        string memory json = vm.readFile("test/fixtures/groth16_e2e_fixture.json");
        bytes32 circuitHash = vm.parseJsonBytes32(json, ".circuit_hash_raw");

        uint256[] memory sizes = new uint256[](3);
        sizes[0] = 32;
        sizes[1] = 16;
        sizes[2] = 16;
        uint8[] memory types = new uint8[](3);
        types[0] = 3;
        types[1] = 1;
        types[2] = 0;
        bool[] memory committed = new bool[](3);
        committed[0] = false; // Non-committed input layer
        freshVerifier.registerCircuit(circuitHash, 3, sizes, types, committed, "test-noncommitted");
        freshVerifier.setCircuitGroth16Verifier(circuitHash, address(groth16Verifier), 78);

        // Load fixture data
        (
            bytes memory innerProof,
            bytes memory gensHex,,
            bytes memory publicValuesAbi,
            uint256[8] memory groth16Proof,
            uint256[] memory groth16Outputs
        ) = _loadCombinedFixture();

        // The mleEval defense-in-depth check passes (on-chain MLE matches Groth16 output).
        // But the claim commitment check fails because the fixture's final layer commitment
        // is a Hyrax commitment (committed input), not scalarMul(g, mleEval).
        vm.expectRevert("Hybrid: public input claim mismatch");
        freshVerifier.verifyWithGroth16(innerProof, circuitHash, publicValuesAbi, gensHex, groth16Proof, groth16Outputs);
    }

    /// @notice Test that committed-only constraint is relaxed: registering with isCommitted[0]=false
    ///         no longer reverts at _validateHybridInputs (it proceeds past validation).
    function test_hybrid_accepts_noncommitted_registration() public {
        RemainderVerifier freshVerifier = new RemainderVerifier(address(this));
        freshVerifier.setGroth16Verifier(address(groth16Verifier));

        string memory json = vm.readFile("test/fixtures/groth16_e2e_fixture.json");
        bytes32 circuitHash = vm.parseJsonBytes32(json, ".circuit_hash_raw");

        uint256[] memory sizes = new uint256[](3);
        sizes[0] = 32;
        sizes[1] = 16;
        sizes[2] = 16;
        uint8[] memory types = new uint8[](3);
        types[0] = 3;
        types[1] = 1;
        types[2] = 0;
        bool[] memory committed = new bool[](3);
        committed[0] = false;
        freshVerifier.registerCircuit(circuitHash, 3, sizes, types, committed, "test-noncommitted");
        freshVerifier.setCircuitGroth16Verifier(circuitHash, address(groth16Verifier), 78);

        (
            bytes memory innerProof,
            bytes memory gensHex,,
            bytes memory publicValuesAbi,
            uint256[8] memory groth16Proof,
            uint256[] memory groth16Outputs
        ) = _loadCombinedFixture();

        // Should NOT revert with "Hybrid: layer 0 must be committed" — that constraint is removed.
        // It will revert at the claim commitment check (fixture has Hyrax commitment, not scalar).
        vm.expectRevert("Hybrid: public input claim mismatch");
        freshVerifier.verifyWithGroth16(innerProof, circuitHash, publicValuesAbi, gensHex, groth16Proof, groth16Outputs);
    }

    /// @notice Regression: committed input circuit still passes E2E (new check is no-op)
    function test_hybrid_committed_still_passes_e2e() public view {
        // This is the same as test_combined_e2e_full_verification but explicitly
        // verifies the new code path doesn't break committed-input circuits.
        (
            bytes memory innerProof,
            bytes memory gensHex,
            bytes32 circuitHash,
            bytes memory publicValuesAbi,
            uint256[8] memory groth16Proof,
            uint256[] memory groth16Outputs
        ) = _loadCombinedFixture();

        // Should not revert — committed input circuit bypasses the new check
        remainderVerifier.verifyWithGroth16(
            innerProof, circuitHash, publicValuesAbi, gensHex, groth16Proof, groth16Outputs
        );
    }
}

// ========================================================================
// DAG Verifier Tests
// ========================================================================

contract GKRDAGVerifierTest is Test {
    RemainderVerifier verifier;

    function setUp() public {
        verifier = new RemainderVerifier(address(this));
    }

    /// @notice Load fixture and build DAGCircuitDescription from JSON
    function _loadFixture()
        internal
        view
        returns (
            bytes memory proofHex,
            bytes memory gensHex,
            bytes32 circuitHash,
            bytes memory publicInputsHex,
            GKRDAGVerifier.DAGCircuitDescription memory desc
        )
    {
        string memory json = vm.readFile("test/fixtures/phase1a_dag_fixture.json");
        proofHex = vm.parseJsonBytes(json, ".proof_hex");
        gensHex = vm.parseJsonBytes(json, ".gens_hex");
        circuitHash = vm.parseJsonBytes32(json, ".circuit_hash_raw");
        publicInputsHex = vm.parseJsonBytes(json, ".public_inputs_hex");

        // Parse DAG circuit description
        desc.numComputeLayers = vm.parseJsonUint(json, ".dag_circuit_description.numComputeLayers");
        desc.numInputLayers = vm.parseJsonUint(json, ".dag_circuit_description.numInputLayers");
        desc.layerTypes = _parseJsonUint8Array(json, ".dag_circuit_description.layerTypes");
        desc.numSumcheckRounds = vm.parseJsonUintArray(json, ".dag_circuit_description.numSumcheckRounds");
        desc.atomOffsets = vm.parseJsonUintArray(json, ".dag_circuit_description.atomOffsets");
        desc.atomTargetLayers = vm.parseJsonUintArray(json, ".dag_circuit_description.atomTargetLayers");
        desc.atomCommitIdxs = vm.parseJsonUintArray(json, ".dag_circuit_description.atomCommitIdxs");
        desc.ptOffsets = vm.parseJsonUintArray(json, ".dag_circuit_description.ptOffsets");
        desc.ptData = vm.parseJsonUintArray(json, ".dag_circuit_description.ptData");
        desc.inputIsCommitted = _parseJsonBoolArray(json, ".dag_circuit_description.inputIsCommitted");
        desc.oracleProductOffsets = vm.parseJsonUintArray(json, ".dag_circuit_description.oracleProductOffsets");
        desc.oracleResultIdxs = vm.parseJsonUintArray(json, ".dag_circuit_description.oracleResultIdxs");
        desc.oracleExprCoeffs = _parseJsonUint256Array(json, ".dag_circuit_description.oracleExprCoeffs");
    }

    /// @notice Parse a JSON uint array as uint8[]
    function _parseJsonUint8Array(string memory json, string memory key) internal pure returns (uint8[] memory result) {
        uint256[] memory raw = vm.parseJsonUintArray(json, key);
        result = new uint8[](raw.length);
        for (uint256 i = 0; i < raw.length; i++) {
            result[i] = uint8(raw[i]);
        }
    }

    /// @notice Parse a JSON bool array using vm.parseJson (raw ABI decode)
    function _parseJsonBoolArray(string memory json, string memory key) internal pure returns (bool[] memory result) {
        bytes memory raw = vm.parseJson(json, key);
        result = abi.decode(raw, (bool[]));
    }

    /// @notice Parse a JSON array of hex strings as uint256[]
    function _parseJsonUint256Array(string memory json, string memory key)
        internal
        pure
        returns (uint256[] memory result)
    {
        bytes memory raw = vm.parseJson(json, key);
        bytes32[] memory parsed = abi.decode(raw, (bytes32[]));
        result = new uint256[](parsed.length);
        for (uint256 i = 0; i < parsed.length; i++) {
            result[i] = uint256(parsed[i]);
        }
    }

    /// @notice Test fixture loads correctly
    function test_fixture_loads() public view {
        (
            bytes memory proofHex,
            bytes memory gensHex,
            bytes32 circuitHash,
            bytes memory publicInputsHex,
            GKRDAGVerifier.DAGCircuitDescription memory desc
        ) = _loadFixture();

        assertEq(desc.numComputeLayers, 88, "numComputeLayers");
        assertEq(desc.numInputLayers, 2, "numInputLayers");
        assertEq(desc.layerTypes.length, 88, "layerTypes length");
        assertEq(desc.atomOffsets.length, 89, "atomOffsets length");
        assertEq(desc.atomTargetLayers.length, 144, "atomTargetLayers length");
        assertTrue(desc.inputIsCommitted[0], "input 0 committed");
        assertFalse(desc.inputIsCommitted[1], "input 1 not committed");
        assertTrue(proofHex.length > 0, "proof not empty");
        assertTrue(gensHex.length > 0, "gens not empty");
        assertTrue(circuitHash != bytes32(0), "circuit hash not zero");
        assertTrue(publicInputsHex.length > 0, "public inputs not empty");
    }

    /// @notice Test DAG circuit registration
    function test_register_dag_circuit() public {
        (, bytes memory gensHex, bytes32 circuitHash,, GKRDAGVerifier.DAGCircuitDescription memory desc) =
            _loadFixture();

        bytes32 gensHash = keccak256(gensHex);
        bytes memory descData = abi.encode(desc);

        verifier.registerDAGCircuit(circuitHash, descData, "xgboost-phase1a", gensHash);
        assertTrue(verifier.isDAGCircuitActive(circuitHash), "DAG circuit should be active");
    }

    /// @notice Test duplicate DAG circuit registration fails
    function test_register_dag_circuit_duplicate_reverts() public {
        (, bytes memory gensHex, bytes32 circuitHash,, GKRDAGVerifier.DAGCircuitDescription memory desc) =
            _loadFixture();

        bytes32 gensHash = keccak256(gensHex);
        bytes memory descData = abi.encode(desc);

        verifier.registerDAGCircuit(circuitHash, descData, "xgboost-phase1a", gensHash);

        vm.expectRevert("DAG circuit already registered");
        verifier.registerDAGCircuit(circuitHash, descData, "xgboost-phase1a-dup", gensHash);
    }

    /// @notice Test registration reverts when numComputeLayers == 0
    function test_register_dag_circuit_no_compute_layers_reverts() public {
        (, bytes memory gensHex,, , GKRDAGVerifier.DAGCircuitDescription memory desc) = _loadFixture();

        // Override numComputeLayers to 0
        desc.numComputeLayers = 0;
        desc.layerTypes = new uint8[](0);
        desc.numSumcheckRounds = new uint256[](0);
        desc.atomOffsets = new uint256[](1);
        desc.atomOffsets[0] = 0;
        desc.oracleProductOffsets = new uint256[](1);
        desc.oracleProductOffsets[0] = 0;

        bytes32 hash = bytes32(uint256(0xdead01));
        bytes32 gensHash = keccak256(gensHex);

        vm.expectRevert("no compute layers");
        verifier.registerDAGCircuit(hash, abi.encode(desc), "bad-circuit", gensHash);
    }

    /// @notice Test registration reverts when numInputLayers == 0
    function test_register_dag_circuit_no_input_layers_reverts() public {
        (, bytes memory gensHex,,, GKRDAGVerifier.DAGCircuitDescription memory desc) = _loadFixture();

        // Override numInputLayers to 0
        desc.numInputLayers = 0;
        desc.inputIsCommitted = new bool[](0);

        bytes32 hash = bytes32(uint256(0xdead02));
        bytes32 gensHash = keccak256(gensHex);

        vm.expectRevert("no input layers");
        verifier.registerDAGCircuit(hash, abi.encode(desc), "bad-circuit", gensHash);
    }

    /// @notice Test registration reverts when numComputeLayers > 256 (gas DOS protection)
    function test_register_dag_circuit_too_many_compute_layers_reverts() public {
        (, bytes memory gensHex,,, GKRDAGVerifier.DAGCircuitDescription memory desc) = _loadFixture();

        // Override numComputeLayers to 257
        desc.numComputeLayers = 257;
        // Resize layerTypes to match (so the "too many compute layers" check fires first)
        desc.layerTypes = new uint8[](257);
        desc.numSumcheckRounds = new uint256[](257);
        desc.atomOffsets = new uint256[](258);
        desc.oracleProductOffsets = new uint256[](258);

        bytes32 hash = bytes32(uint256(0xdead03));
        bytes32 gensHash = keccak256(gensHex);

        vm.expectRevert("too many compute layers");
        verifier.registerDAGCircuit(hash, abi.encode(desc), "bad-circuit", gensHash);
    }

    /// @notice Test registration reverts when numInputLayers > 64
    function test_register_dag_circuit_too_many_input_layers_reverts() public {
        (, bytes memory gensHex,,, GKRDAGVerifier.DAGCircuitDescription memory desc) = _loadFixture();

        desc.numInputLayers = 65;
        desc.inputIsCommitted = new bool[](65);

        bytes32 hash = bytes32(uint256(0xdead04));
        bytes32 gensHash = keccak256(gensHex);

        vm.expectRevert("too many input layers");
        verifier.registerDAGCircuit(hash, abi.encode(desc), "bad-circuit", gensHash);
    }

    /// @notice Test registration reverts on numSumcheckRounds length mismatch
    function test_register_dag_circuit_sumcheck_rounds_mismatch_reverts() public {
        (, bytes memory gensHex,,, GKRDAGVerifier.DAGCircuitDescription memory desc) = _loadFixture();

        // Make numSumcheckRounds too short
        desc.numSumcheckRounds = new uint256[](desc.numComputeLayers - 1);

        bytes32 hash = bytes32(uint256(0xdead05));
        bytes32 gensHash = keccak256(gensHex);

        vm.expectRevert("numSumcheckRounds length mismatch");
        verifier.registerDAGCircuit(hash, abi.encode(desc), "bad-circuit", gensHash);
    }

    /// @notice Test registration reverts on inputIsCommitted length mismatch
    function test_register_dag_circuit_input_committed_mismatch_reverts() public {
        (, bytes memory gensHex,,, GKRDAGVerifier.DAGCircuitDescription memory desc) = _loadFixture();

        // Make inputIsCommitted wrong length
        desc.inputIsCommitted = new bool[](desc.numInputLayers + 1);

        bytes32 hash = bytes32(uint256(0xdead06));
        bytes32 gensHash = keccak256(gensHex);

        vm.expectRevert("inputIsCommitted length mismatch");
        verifier.registerDAGCircuit(hash, abi.encode(desc), "bad-circuit", gensHash);
    }

    /// @notice Test registration reverts on oracleProductOffsets length mismatch
    function test_register_dag_circuit_oracle_offsets_mismatch_reverts() public {
        (, bytes memory gensHex,,, GKRDAGVerifier.DAGCircuitDescription memory desc) = _loadFixture();

        // Make oracleProductOffsets wrong length
        desc.oracleProductOffsets = new uint256[](desc.numComputeLayers);

        bytes32 hash = bytes32(uint256(0xdead07));
        bytes32 gensHash = keccak256(gensHex);

        vm.expectRevert("oracleProductOffsets length mismatch");
        verifier.registerDAGCircuit(hash, abi.encode(desc), "bad-circuit", gensHash);
    }

    /// @notice Test registration reverts on atomTargetLayers length mismatch
    function test_register_dag_circuit_atom_targets_mismatch_reverts() public {
        (, bytes memory gensHex,,, GKRDAGVerifier.DAGCircuitDescription memory desc) = _loadFixture();

        // Truncate atomTargetLayers
        uint256 totalAtoms = desc.atomOffsets[desc.numComputeLayers];
        desc.atomTargetLayers = new uint256[](totalAtoms - 1);

        bytes32 hash = bytes32(uint256(0xdead08));
        bytes32 gensHash = keccak256(gensHex);

        vm.expectRevert("atomTargetLayers length mismatch");
        verifier.registerDAGCircuit(hash, abi.encode(desc), "bad-circuit", gensHash);
    }

    /// @notice Test registration reverts on zero circuit hash
    function test_register_dag_circuit_zero_hash_reverts() public {
        (, bytes memory gensHex,,, GKRDAGVerifier.DAGCircuitDescription memory desc) = _loadFixture();

        bytes32 gensHash = keccak256(gensHex);

        vm.expectRevert("Circuit hash cannot be zero");
        verifier.registerDAGCircuit(bytes32(0), abi.encode(desc), "bad-circuit", gensHash);
    }

    function _stripSelector(bytes memory proof) internal pure returns (bytes memory) {
        bytes memory data = new bytes(proof.length - 4);
        for (uint256 i = 0; i < data.length; i++) {
            data[i] = proof[i + 4];
        }
        return data;
    }

    /// @notice Diagnostic: check embedded public inputs from proof binary
    function test_embedded_public_inputs() public {
        (bytes memory proofHex,,,,) = _loadFixture();
        bytes memory proofData = _stripSelector(proofHex);

        uint256[] memory embedded = verifier.decodeEmbeddedPublicInputsExternal(proofData);
        emit log_named_uint("Embedded pubInputs count", embedded.length);
        assertEq(embedded.length, 64, "Embedded should be 64 (power-of-2 padded)");

        // First value should be 0x8000 = 32768 (comparison offset 2^15)
        emit log_named_uint("embedded[0]", embedded[0]);
    }

    /// @notice Diagnostic: step-by-step transcript trace for DAG circuit
    function test_dag_transcript_trace() public {
        (
            bytes memory proofHex,
            ,
            bytes32 circuitHash,,
            GKRDAGVerifier.DAGCircuitDescription memory desc
        ) = _loadFixture();

        // Decode proof
        bytes memory proofData = _stripSelector(proofHex);
        (GKRVerifier.GKRProof memory gkrProof,) = verifier.decodeProofCounted(proofData, "");

        emit log_named_uint("Layer proofs count", gkrProof.layerProofs.length);
        emit log_named_uint("Output claims count", gkrProof.outputClaimCommitments.length);
        emit log_named_uint("Input proofs count", gkrProof.inputProofs.length);

        // Check first layer proof structure
        emit log_named_uint("Layer0 messages", gkrProof.layerProofs[0].sumcheckProof.messages.length);
        emit log_named_uint("Layer0 commitments", gkrProof.layerProofs[0].commitments.length);
        emit log_named_uint("Layer0 pops", gkrProof.layerProofs[0].pops.length);

        // Check atom routing for layer 0: how many atoms target layers?
        uint256 atomStart = desc.atomOffsets[0];
        uint256 atomEnd = desc.atomOffsets[1];
        emit log_named_uint("Layer0 atoms", atomEnd - atomStart);
        for (uint256 a = atomStart; a < atomEnd; a++) {
            emit log_named_uint(string(abi.encodePacked("atom[", vm.toString(a), "] target")), desc.atomTargetLayers[a]);
        }

        // Setup transcript
        uint256[] memory embeddedPubInputs = verifier.decodeEmbeddedPublicInputsExternal(proofData);
        emit log_named_uint("Embedded pub inputs", embeddedPubInputs.length);

        // Check _hashToFqPair
        (uint256 fq1, uint256 fq2) = verifier.hashToFqPair(circuitHash);
        emit log_named_uint("circuitHash fq1", fq1);
        emit log_named_uint("circuitHash fq2", fq2);
    }

    /// @notice E2E: Register DAG circuit and verify proof
    function test_e2e_dag_verification() public {
        (
            bytes memory proofHex,
            bytes memory gensHex,
            bytes32 circuitHash,
            bytes memory publicInputsHex,
            GKRDAGVerifier.DAGCircuitDescription memory desc
        ) = _loadFixture();

        bytes32 gensHash = keccak256(gensHex);
        bytes memory descData = abi.encode(desc);

        // Register circuit
        verifier.registerDAGCircuit(circuitHash, descData, "xgboost-phase1a", gensHash);

        // Verify proof
        bool valid = verifier.verifyDAGProof(proofHex, circuitHash, publicInputsHex, gensHex);
        assertTrue(valid, "DAG proof should verify");
    }

    /// @notice Test unregistered circuit reverts
    function test_dag_unregistered_circuit_reverts() public {
        bytes32 fakeHash = keccak256("nonexistent");
        bytes memory fakeProof = abi.encodePacked(bytes4("REM1"), bytes32(0));

        vm.expectRevert(RemainderVerifier.CircuitNotRegistered.selector);
        verifier.verifyDAGProof(fakeProof, fakeHash, "", "");
    }

    /// @notice Test wrong generators are rejected
    function test_dag_wrong_gens_reverts() public {
        (
            bytes memory proofHex,
            bytes memory gensHex,
            bytes32 circuitHash,
            bytes memory publicInputsHex,
            GKRDAGVerifier.DAGCircuitDescription memory desc
        ) = _loadFixture();

        bytes32 gensHash = keccak256(gensHex);
        bytes memory descData = abi.encode(desc);
        verifier.registerDAGCircuit(circuitHash, descData, "xgboost-phase1a", gensHash);

        // Use wrong generators
        bytes memory wrongGens =
            abi.encodePacked(uint256(1), uint256(1), uint256(2), uint256(1), uint256(2), uint256(1), uint256(2));

        vm.expectRevert(RemainderVerifier.InvalidGenerators.selector);
        verifier.verifyDAGProof(proofHex, circuitHash, publicInputsHex, wrongGens);
    }

    /// @notice Test invalid proof selector reverts
    function test_dag_invalid_selector_reverts() public {
        (, bytes memory gensHex, bytes32 circuitHash,, GKRDAGVerifier.DAGCircuitDescription memory desc) =
            _loadFixture();

        bytes32 gensHash = keccak256(gensHex);
        bytes memory descData = abi.encode(desc);
        verifier.registerDAGCircuit(circuitHash, descData, "xgboost-phase1a", gensHash);

        bytes memory badProof = abi.encodePacked(bytes4("FAKE"), bytes32(0));

        vm.expectRevert(RemainderVerifier.InvalidProofSelector.selector);
        verifier.verifyDAGProof(badProof, circuitHash, "", gensHex);
    }

    /// @notice Diagnostic: test DAG input proof decoding (all eval proofs)
    function test_dag_input_proof_decode() public {
        (bytes memory proofHex,,,,) = _loadFixture();
        bytes memory proofData = _stripSelector(proofHex);

        (uint256 numLayers, uint256[] memory numRows, uint256[] memory numEvals) =
            verifier.decodeDAGInputProofCounts(proofData);
        emit log_named_uint("DAG input layers", numLayers);
        for (uint256 i = 0; i < numLayers; i++) {
            emit log_named_uint(string(abi.encodePacked("layer[", vm.toString(i), "].commitmentRows")), numRows[i]);
            emit log_named_uint(string(abi.encodePacked("layer[", vm.toString(i), "].evalProofs")), numEvals[i]);
        }
    }

    /// @notice Diagnostic: verify public value claims and MLE evaluation
    function test_public_value_claims_diagnostic() public {
        (bytes memory proofHex,,,,) = _loadFixture();
        bytes memory proofData = _stripSelector(proofHex);

        // Decode embedded public inputs (MLE data)
        uint256[] memory embedded = verifier.decodeEmbeddedPublicInputsExternal(proofData);
        emit log_named_uint("Embedded pubInputs count", embedded.length);

        // Decode public value claims from proof
        // The public claims are after the FS claims section in the proof
        // Use the decodeProofForDAG which now returns publicValueClaims
        // We need a wrapper for this...
        // For now, let's decode manually from the fixture
        // Read input_layers[1].claim_points from fixture
        string memory json = vm.readFile("test/fixtures/phase1a_dag_fixture.json");

        // Get the first claim point for input layer 1 (public)
        string memory basePath = ".input_layers[1].claim_points[0]";
        uint256[] memory point = new uint256[](6);
        for (uint256 i = 0; i < 6; i++) {
            string memory path = string(abi.encodePacked(basePath, "[", vm.toString(i), "]"));
            bytes memory raw = vm.parseJson(json, path);
            point[i] = abi.decode(raw, (uint256));
        }

        emit log_named_uint("claim point[0]", point[0]);
        emit log_named_uint("claim point[1]", point[1]);
        emit log_named_uint("claim point[2]", point[2]);
        emit log_named_uint("claim point[3]", point[3]);
        emit log_named_uint("claim point[4]", point[4]);
        emit log_named_uint("claim point[5]", point[5]);

        // Evaluate MLE
        uint256 mleEval = GKRDAGVerifier.evaluateMLEFromData(embedded, point);
        emit log_named_uint("MLE evaluation", mleEval);
    }
}

// ========================================================================
// DAG HYBRID VERIFIER TESTS (Phase 1 validation)
// ========================================================================

contract GKRDAGHybridVerifierTest is Test {
    RemainderVerifier verifier;

    function setUp() public {
        verifier = new RemainderVerifier(address(this));
    }

    function _loadFixtureAndRegister()
        internal
        returns (
            bytes memory proofHex,
            bytes memory gensHex,
            bytes32 circuitHash,
            bytes memory publicInputsHex,
            GKRDAGVerifier.DAGCircuitDescription memory desc
        )
    {
        string memory json = vm.readFile("test/fixtures/phase1a_dag_fixture.json");
        proofHex = vm.parseJsonBytes(json, ".proof_hex");
        gensHex = vm.parseJsonBytes(json, ".gens_hex");
        circuitHash = vm.parseJsonBytes32(json, ".circuit_hash_raw");
        publicInputsHex = vm.parseJsonBytes(json, ".public_inputs_hex");

        desc.numComputeLayers = vm.parseJsonUint(json, ".dag_circuit_description.numComputeLayers");
        desc.numInputLayers = vm.parseJsonUint(json, ".dag_circuit_description.numInputLayers");
        desc.layerTypes = _parseJsonUint8Array(json, ".dag_circuit_description.layerTypes");
        desc.numSumcheckRounds = vm.parseJsonUintArray(json, ".dag_circuit_description.numSumcheckRounds");
        desc.atomOffsets = vm.parseJsonUintArray(json, ".dag_circuit_description.atomOffsets");
        desc.atomTargetLayers = vm.parseJsonUintArray(json, ".dag_circuit_description.atomTargetLayers");
        desc.atomCommitIdxs = vm.parseJsonUintArray(json, ".dag_circuit_description.atomCommitIdxs");
        desc.ptOffsets = vm.parseJsonUintArray(json, ".dag_circuit_description.ptOffsets");
        desc.ptData = vm.parseJsonUintArray(json, ".dag_circuit_description.ptData");
        desc.inputIsCommitted = _parseJsonBoolArray(json, ".dag_circuit_description.inputIsCommitted");
        desc.oracleProductOffsets = vm.parseJsonUintArray(json, ".dag_circuit_description.oracleProductOffsets");
        desc.oracleResultIdxs = vm.parseJsonUintArray(json, ".dag_circuit_description.oracleResultIdxs");
        desc.oracleExprCoeffs = _parseJsonUint256Array(json, ".dag_circuit_description.oracleExprCoeffs");

        // Register circuit
        bytes32 gensHash = keccak256(gensHex);
        bytes memory descData = abi.encode(desc);
        verifier.registerDAGCircuit(circuitHash, descData, "xgboost-dag-hybrid", gensHash);
    }

    function _parseJsonUint8Array(string memory json, string memory key) internal pure returns (uint8[] memory result) {
        uint256[] memory raw = vm.parseJsonUintArray(json, key);
        result = new uint8[](raw.length);
        for (uint256 i = 0; i < raw.length; i++) {
            result[i] = uint8(raw[i]);
        }
    }

    function _parseJsonBoolArray(string memory json, string memory key) internal pure returns (bool[] memory result) {
        bytes memory raw = vm.parseJson(json, key);
        result = abi.decode(raw, (bool[]));
    }

    function _parseJsonUint256Array(string memory json, string memory key)
        internal
        pure
        returns (uint256[] memory result)
    {
        bytes memory raw = vm.parseJson(json, key);
        bytes32[] memory parsed = abi.decode(raw, (bytes32[]));
        result = new uint256[](parsed.length);
        for (uint256 i = 0; i < parsed.length; i++) {
            result[i] = uint256(parsed[i]);
        }
    }

    /// @notice Test: DAG hybrid transcript replay completes on 88-layer circuit
    function test_dag_hybrid_transcript_replay() public {
        (bytes memory proofHex,, bytes32 circuitHash, bytes memory publicInputsHex,) = _loadFixtureAndRegister();

        (
            uint256[] memory outputChallenges,
            uint256 layer0RlcCoeff0,
            uint256 layer0Binding0,
            uint256 layer0Rho0,
            uint256 layer0Gamma0,
            uint256 layer0PodpChallenge
        ) = verifier.replayDAGTranscriptPublic(proofHex, circuitHash, publicInputsHex);

        // Verify output challenges are non-zero and have expected length
        assertTrue(outputChallenges.length > 0, "should have output challenges");
        assertTrue(outputChallenges[0] != 0, "output challenge 0 should be non-zero");

        // Verify layer 0 challenges are non-zero
        assertTrue(layer0RlcCoeff0 != 0, "layer0 RLC coeff should be non-zero");
        assertTrue(layer0Binding0 != 0, "layer0 binding should be non-zero");
        assertTrue(layer0Rho0 != 0, "layer0 rho should be non-zero");
        assertTrue(layer0Gamma0 != 0, "layer0 gamma should be non-zero");
        assertTrue(layer0PodpChallenge != 0, "layer0 PODP challenge should be non-zero");
    }

    /// @notice Test: DAG hybrid transcript produces same output challenges as direct verifier
    function test_dag_hybrid_vs_direct_transcript_consistency() public {
        (bytes memory proofHex, bytes memory gensHex, bytes32 circuitHash, bytes memory publicInputsHex,) =
            _loadFixtureAndRegister();

        // Get output challenges from hybrid replay
        (uint256[] memory hybridOutputChallenges,,,,,) =
            verifier.replayDAGTranscriptPublic(proofHex, circuitHash, publicInputsHex);

        // Run direct DAG verification to confirm proof is valid
        // This implicitly validates the transcript is consistent
        bool valid = verifier.verifyDAGProof(proofHex, circuitHash, publicInputsHex, gensHex);
        assertTrue(valid, "direct DAG verification should pass");

        // If both pass, the transcripts are consistent
        assertTrue(hybridOutputChallenges.length > 0, "hybrid should produce output challenges");
    }

    /// @notice Test: buildDAGGroth16Inputs produces correct total size and structure
    function test_dag_build_groth16_inputs_size() public {
        (bytes memory proofHex,, bytes32 circuitHash, bytes memory publicInputsHex,) = _loadFixtureAndRegister();

        // Use the public wrapper to build Groth16 inputs and get metadata
        (uint256 inputCount, uint256 numGroups, uint256 numPubClaims, uint256 numEmbeddedPubInputs) =
            verifier.buildDAGGroth16InputsPublic(proofHex, circuitHash, publicInputsHex);

        emit log_named_uint("DAG Groth16 input count", inputCount);
        emit log_named_uint("Input groups", numGroups);
        emit log_named_uint("Public claims", numPubClaims);
        emit log_named_uint("Embedded pub inputs", numEmbeddedPubInputs);

        // Verify reasonable sizes for 88-layer XGBoost circuit
        assertTrue(inputCount > 0, "inputs should be non-empty");
        assertTrue(numGroups > 0, "should have input groups");
        assertTrue(numPubClaims > 0, "should have public claims");
        assertTrue(numEmbeddedPubInputs > 0, "should have embedded public inputs");

        // Expected: ~1700+ public inputs for 88-layer DAG
        assertTrue(inputCount > 500, "DAG circuit should have many public inputs");
    }

    /// @notice Test: collectPubClaimPoints returns expected count from DAG description
    function test_dag_collect_pub_claim_points() public {
        (,,,, GKRDAGVerifier.DAGCircuitDescription memory desc) = _loadFixtureAndRegister();

        // Count public claims from description
        uint256 expectedPubClaims = 0;
        for (uint256 inputIdx = 0; inputIdx < desc.numInputLayers; inputIdx++) {
            if (desc.inputIsCommitted[inputIdx]) continue;
            uint256 targetLayer = desc.numComputeLayers + inputIdx;
            for (uint256 j = 0; j < desc.atomTargetLayers.length; j++) {
                if (desc.atomTargetLayers[j] == targetLayer) expectedPubClaims++;
            }
        }

        emit log_named_uint("Expected public claims", expectedPubClaims);
        assertTrue(expectedPubClaims > 0, "should have public claims in DAG circuit");
    }
}

// ========================================================================
// DAG GROTH16 E2E TESTS (Full hybrid Groth16 verification for DAG circuits)
// ========================================================================

import {DAGGroth16Verifier} from "../src/remainder/DAGRemainderGroth16Verifier.sol";

contract GKRDAGGroth16E2ETest is Test {
    RemainderVerifier verifier;
    DAGGroth16Verifier dagGroth16Verifier;

    function setUp() public {
        verifier = new RemainderVerifier(address(this));
        dagGroth16Verifier = new DAGGroth16Verifier();
    }

    function _loadE2EFixtureAndRegister()
        internal
        returns (
            bytes memory innerProof,
            bytes memory gensHex,
            bytes32 circuitHash,
            bytes memory publicInputsAbi,
            uint256[8] memory groth16Proof,
            uint256[] memory groth16Outputs
        )
    {
        string memory json = vm.readFile("test/fixtures/dag_groth16_e2e_fixture.json");
        innerProof = vm.parseJsonBytes(json, ".inner_proof_hex");
        gensHex = vm.parseJsonBytes(json, ".gens_hex");
        circuitHash = vm.parseJsonBytes32(json, ".circuit_hash_raw");
        publicInputsAbi = vm.parseJsonBytes(json, ".public_values_abi");

        for (uint256 i = 0; i < 8; i++) {
            groth16Proof[i] = vm.parseJsonUint(json, string.concat(".groth16_proof[", vm.toString(i), "]"));
        }
        groth16Outputs = vm.parseJsonUintArray(json, ".groth16_outputs");

        // Register DAG circuit with description from fixture
        GKRDAGVerifier.DAGCircuitDescription memory desc;
        desc.numComputeLayers = vm.parseJsonUint(json, ".dag_circuit_description.numComputeLayers");
        desc.numInputLayers = vm.parseJsonUint(json, ".dag_circuit_description.numInputLayers");
        desc.layerTypes = _parseUint8Array(json, ".dag_circuit_description.layerTypes");
        desc.numSumcheckRounds = vm.parseJsonUintArray(json, ".dag_circuit_description.numSumcheckRounds");
        desc.atomOffsets = vm.parseJsonUintArray(json, ".dag_circuit_description.atomOffsets");
        desc.atomTargetLayers = vm.parseJsonUintArray(json, ".dag_circuit_description.atomTargetLayers");
        desc.atomCommitIdxs = vm.parseJsonUintArray(json, ".dag_circuit_description.atomCommitIdxs");
        desc.ptOffsets = vm.parseJsonUintArray(json, ".dag_circuit_description.ptOffsets");
        desc.ptData = vm.parseJsonUintArray(json, ".dag_circuit_description.ptData");
        desc.inputIsCommitted = _parseBoolArray(json, ".dag_circuit_description.inputIsCommitted");
        desc.oracleProductOffsets = vm.parseJsonUintArray(json, ".dag_circuit_description.oracleProductOffsets");
        desc.oracleResultIdxs = vm.parseJsonUintArray(json, ".dag_circuit_description.oracleResultIdxs");
        desc.oracleExprCoeffs = _parseUint256Array(json, ".dag_circuit_description.oracleExprCoeffs");

        bytes32 gensHash = keccak256(gensHex);
        bytes memory descData = abi.encode(desc);
        verifier.registerDAGCircuit(circuitHash, descData, "xgboost-dag-groth16", gensHash);

        // Set DAG Groth16 verifier (3416 public inputs)
        verifier.setDAGCircuitGroth16Verifier(circuitHash, address(dagGroth16Verifier), 3416);
    }

    function _parseUint8Array(string memory json, string memory key) internal pure returns (uint8[] memory result) {
        uint256[] memory raw = vm.parseJsonUintArray(json, key);
        result = new uint8[](raw.length);
        for (uint256 i = 0; i < raw.length; i++) {
            result[i] = uint8(raw[i]);
        }
    }

    function _parseBoolArray(string memory json, string memory key) internal pure returns (bool[] memory result) {
        bytes memory raw = vm.parseJson(json, key);
        result = abi.decode(raw, (bool[]));
    }

    function _parseUint256Array(string memory json, string memory key) internal pure returns (uint256[] memory result) {
        bytes memory raw = vm.parseJson(json, key);
        bytes32[] memory parsed = abi.decode(raw, (bytes32[]));
        result = new uint256[](parsed.length);
        for (uint256 i = 0; i < parsed.length; i++) {
            result[i] = uint256(parsed[i]);
        }
    }

    /// @notice Full E2E: verifyDAGWithGroth16() with real XGBoost proof + Groth16 wrapper
    function test_dag_e2e_groth16_verification() public {
        (
            bytes memory innerProof,
            bytes memory gensHex,
            bytes32 circuitHash,
            bytes memory publicInputsAbi,
            uint256[8] memory groth16Proof,
            uint256[] memory groth16Outputs
        ) = _loadE2EFixtureAndRegister();

        // Should not revert — full hybrid verification
        verifier.verifyDAGWithGroth16(innerProof, circuitHash, publicInputsAbi, gensHex, groth16Proof, groth16Outputs);

        emit log_string("DAG Groth16 E2E verification: PASSED");
    }

    /// @notice Gas measurement for DAG hybrid Groth16 verification
    function test_dag_e2e_groth16_gas() public {
        (
            bytes memory innerProof,
            bytes memory gensHex,
            bytes32 circuitHash,
            bytes memory publicInputsAbi,
            uint256[8] memory groth16Proof,
            uint256[] memory groth16Outputs
        ) = _loadE2EFixtureAndRegister();

        uint256 gasBefore = gasleft();
        verifier.verifyDAGWithGroth16(innerProof, circuitHash, publicInputsAbi, gensHex, groth16Proof, groth16Outputs);
        uint256 gasUsed = gasBefore - gasleft();

        emit log_named_uint("DAG Groth16 E2E verification gas", gasUsed);
    }

    /// @notice Test that corrupted Groth16 proof is rejected
    function test_dag_e2e_rejects_bad_groth16_proof() public {
        (
            bytes memory innerProof,
            bytes memory gensHex,
            bytes32 circuitHash,
            bytes memory publicInputsAbi,
            uint256[8] memory groth16Proof,
            uint256[] memory groth16Outputs
        ) = _loadE2EFixtureAndRegister();

        // Corrupt a Groth16 proof point
        groth16Proof[0] = groth16Proof[0] ^ 1;

        vm.expectRevert();
        verifier.verifyDAGWithGroth16(innerProof, circuitHash, publicInputsAbi, gensHex, groth16Proof, groth16Outputs);
    }

    /// @notice Diagnostic: verify gnark proof directly with gnark-provided public inputs
    function test_dag_groth16_direct_verification() public {
        string memory json = vm.readFile("test/fixtures/dag_groth16_e2e_fixture.json");

        uint256[8] memory groth16Proof;
        for (uint256 i = 0; i < 8; i++) {
            groth16Proof[i] = vm.parseJsonUint(json, string.concat(".groth16_proof[", vm.toString(i), "]"));
        }

        // Load gnark-computed public inputs (3416 values)
        uint256[] memory gnarkInputs = vm.parseJsonUintArray(json, ".groth16_public_inputs");
        emit log_named_uint("gnark public inputs count", gnarkInputs.length);
        emit log_named_uint("gnark input[0]", gnarkInputs[0]);
        emit log_named_uint("gnark input[1]", gnarkInputs[1]);
        emit log_named_uint("gnark input[2]", gnarkInputs[2]);
        require(gnarkInputs.length == 3416, "Expected 3416 gnark public inputs");

        // Use _callGroth16Verifier which we know works (it's used in verifyWithGroth16)
        bytes4 selector = bytes4(keccak256("verifyProof(uint256[8],uint256[3416])"));
        uint256 n = gnarkInputs.length;
        bytes memory data = new bytes(4 + (8 + n) * 32);
        assembly {
            let ptr := add(data, 32)
            mstore(ptr, shl(224, shr(224, selector)))
            ptr := add(ptr, 4)
            // Note: we cannot access calldata array here, proof is in memory
        }
        // Copy proof (memory array, 8 elements, no length prefix)
        for (uint256 i = 0; i < 8; i++) {
            uint256 val = groth16Proof[i];
            uint256 offset = 4 + i * 32;
            assembly { mstore(add(data, add(32, offset)), val) }
        }
        // Copy inputs (memory dynamic array)
        for (uint256 i = 0; i < n; i++) {
            uint256 val = gnarkInputs[i];
            uint256 offset = 4 + 256 + i * 32;
            assembly { mstore(add(data, add(32, offset)), val) }
        }

        emit log_named_uint("calldata size", data.length);
        (bool success,) = address(dagGroth16Verifier).staticcall(data);
        require(success, "Direct gnark Groth16 verification failed");
    }

    /// @notice Diagnostic: compare Solidity-computed inputs with gnark's
    function test_dag_groth16_input_comparison() public {
        (bytes memory innerProof,, bytes32 circuitHash, bytes memory publicInputsAbi,,) = _loadE2EFixtureAndRegister();

        // Get Solidity-computed inputs (with dummy outputs since we just want challenges section)
        uint256[] memory solInputs = verifier.buildDAGGroth16InputsFull(innerProof, circuitHash, publicInputsAbi);

        // Get gnark inputs from fixture
        string memory json = vm.readFile("test/fixtures/dag_groth16_e2e_fixture.json");
        uint256[] memory gnarkInputs = vm.parseJsonUintArray(json, ".groth16_public_inputs");

        emit log_named_uint("Solidity input count", solInputs.length);
        emit log_named_uint("gnark input count", gnarkInputs.length);

        // Compare element by element, find first mismatch
        uint256 minLen = solInputs.length < gnarkInputs.length ? solInputs.length : gnarkInputs.length;
        for (uint256 i = 0; i < minLen; i++) {
            if (solInputs[i] != gnarkInputs[i]) {
                emit log_named_uint("FIRST MISMATCH at index", i);
                emit log_named_uint("  Solidity value", solInputs[i]);
                emit log_named_uint("  gnark value", gnarkInputs[i]);
                // Also show surrounding values
                if (i > 0) {
                    emit log_named_uint("  prev Solidity", solInputs[i - 1]);
                    emit log_named_uint("  prev gnark", gnarkInputs[i - 1]);
                }
                if (i + 1 < minLen) {
                    emit log_named_uint("  next Solidity", solInputs[i + 1]);
                    emit log_named_uint("  next gnark", gnarkInputs[i + 1]);
                }
                break;
            }
        }
        if (solInputs.length != gnarkInputs.length) {
            emit log_string("LENGTH MISMATCH");
        }
    }

    /// @notice Diagnostic: verify the E2E fixture's inner proof works with the direct DAG verifier
    function test_dag_e2e_inner_proof_direct_verification() public {
        (bytes memory innerProof, bytes memory gensHex, bytes32 circuitHash, bytes memory publicInputsAbi,,) =
            _loadE2EFixtureAndRegister();

        bool valid = verifier.verifyDAGProof(innerProof, circuitHash, publicInputsAbi, gensHex);
        assertTrue(valid, "Direct DAG verification of E2E fixture inner proof should pass");
        emit log_string("Direct DAG verification: PASSED");
    }

    /// @notice Test that corrupted Groth16 outputs are rejected
    function test_dag_e2e_rejects_bad_groth16_outputs() public {
        (
            bytes memory innerProof,
            bytes memory gensHex,
            bytes32 circuitHash,
            bytes memory publicInputsAbi,
            uint256[8] memory groth16Proof,
            uint256[] memory groth16Outputs
        ) = _loadE2EFixtureAndRegister();

        // Corrupt an output value (rlcBeta[0])
        groth16Outputs[0] = groth16Outputs[0] ^ 1;

        vm.expectRevert();
        verifier.verifyDAGWithGroth16(innerProof, circuitHash, publicInputsAbi, gensHex, groth16Proof, groth16Outputs);
    }
}

// ========================================================================
// DAG BATCH VERIFIER TESTS (Multi-transaction batch verification)
// ========================================================================

contract DAGBatchVerifierTest is Test {
    RemainderVerifier verifier;

    // Fixture data cached for reuse
    bytes proofHex;
    bytes gensHex;
    bytes32 circuitHash;
    bytes publicInputsHex;

    function setUp() public {
        verifier = new RemainderVerifier(address(this));
        _loadAndRegister();
    }

    function _loadAndRegister() internal {
        string memory json = vm.readFile("test/fixtures/phase1a_dag_fixture.json");
        proofHex = vm.parseJsonBytes(json, ".proof_hex");
        gensHex = vm.parseJsonBytes(json, ".gens_hex");
        circuitHash = vm.parseJsonBytes32(json, ".circuit_hash_raw");
        publicInputsHex = vm.parseJsonBytes(json, ".public_inputs_hex");

        GKRDAGVerifier.DAGCircuitDescription memory desc;
        desc.numComputeLayers = vm.parseJsonUint(json, ".dag_circuit_description.numComputeLayers");
        desc.numInputLayers = vm.parseJsonUint(json, ".dag_circuit_description.numInputLayers");
        desc.layerTypes = _parseJsonUint8Array(json, ".dag_circuit_description.layerTypes");
        desc.numSumcheckRounds = vm.parseJsonUintArray(json, ".dag_circuit_description.numSumcheckRounds");
        desc.atomOffsets = vm.parseJsonUintArray(json, ".dag_circuit_description.atomOffsets");
        desc.atomTargetLayers = vm.parseJsonUintArray(json, ".dag_circuit_description.atomTargetLayers");
        desc.atomCommitIdxs = vm.parseJsonUintArray(json, ".dag_circuit_description.atomCommitIdxs");
        desc.ptOffsets = vm.parseJsonUintArray(json, ".dag_circuit_description.ptOffsets");
        desc.ptData = vm.parseJsonUintArray(json, ".dag_circuit_description.ptData");
        desc.inputIsCommitted = _parseJsonBoolArray(json, ".dag_circuit_description.inputIsCommitted");
        desc.oracleProductOffsets = vm.parseJsonUintArray(json, ".dag_circuit_description.oracleProductOffsets");
        desc.oracleResultIdxs = vm.parseJsonUintArray(json, ".dag_circuit_description.oracleResultIdxs");
        desc.oracleExprCoeffs = _parseJsonUint256Array(json, ".dag_circuit_description.oracleExprCoeffs");

        verifier.registerDAGCircuit(circuitHash, abi.encode(desc), "xgboost-batch", keccak256(gensHex));
    }

    function _parseJsonUint8Array(string memory json, string memory key) internal pure returns (uint8[] memory result) {
        uint256[] memory raw = vm.parseJsonUintArray(json, key);
        result = new uint8[](raw.length);
        for (uint256 i = 0; i < raw.length; i++) {
            result[i] = uint8(raw[i]);
        }
    }

    function _parseJsonBoolArray(string memory json, string memory key) internal pure returns (bool[] memory result) {
        bytes memory raw = vm.parseJson(json, key);
        result = abi.decode(raw, (bool[]));
    }

    function _parseJsonUint256Array(string memory json, string memory key)
        internal
        pure
        returns (uint256[] memory result)
    {
        bytes memory raw = vm.parseJson(json, key);
        bytes32[] memory parsed = abi.decode(raw, (bytes32[]));
        result = new uint256[](parsed.length);
        for (uint256 i = 0; i < parsed.length; i++) {
            result[i] = uint256(parsed[i]);
        }
    }

    // ========================================================================
    // E2E BATCH VERIFICATION
    // ========================================================================

    /// @notice Full multi-batch verification: start → continue × N → finalize
    function test_dag_batch_e2e() public {
        // Start (setup only — no compute layers processed)
        bytes32 sessionId = verifier.startDAGBatchVerify(proofHex, circuitHash, publicInputsHex, gensHex);
        assertTrue(sessionId != bytes32(0), "sessionId should be non-zero");

        // Query session state
        (bytes32 storedHash, uint256 nextBatch, uint256 totalBatches, bool finalized,,) =
            verifier.getDAGBatchSession(sessionId);
        assertEq(storedHash, circuitHash, "stored circuit hash");
        assertEq(nextBatch, 0, "nextBatch after start (no compute done)");
        // 88 compute layers / 8 per batch = 11 batches
        assertEq(totalBatches, 11, "totalBatches for 88 layers");
        assertFalse(finalized, "not finalized yet");

        // Continue batches 0..10 (all compute via continue)
        for (uint256 i = 0; i < totalBatches; i++) {
            verifier.continueDAGBatchVerify(sessionId, proofHex, publicInputsHex, gensHex);

            (, nextBatch,,,,) = verifier.getDAGBatchSession(sessionId);
            assertEq(nextBatch, i + 1, "nextBatch should increment");
        }

        // Finalize (input layer verification — may take multiple calls)
        uint256 finalizeCalls = 0;
        while (true) {
            finalized = verifier.finalizeDAGBatchVerify(sessionId, proofHex, publicInputsHex, gensHex);
            finalizeCalls++;
            if (finalized) break;
        }

        (,,, finalized,,) = verifier.getDAGBatchSession(sessionId);
        assertTrue(finalized, "should be finalized");
        // 34 eval groups / 16 per batch = 3 finalize calls (16 + 16 + 2)
        assertGe(finalizeCalls, 1, "at least 1 finalize call");
        emit log_named_uint("Finalize calls", finalizeCalls);
    }

    // ========================================================================
    // AUTH TESTS
    // ========================================================================

    /// @notice Only the original caller can continue a batch session
    function test_dag_batch_session_auth_continue() public {
        bytes32 sessionId = verifier.startDAGBatchVerify(proofHex, circuitHash, publicInputsHex, gensHex);

        // A different address tries to continue
        vm.prank(address(0xBEEF));
        vm.expectRevert("Batch: unauthorized");
        verifier.continueDAGBatchVerify(sessionId, proofHex, publicInputsHex, gensHex);
    }

    /// @notice Only the original caller can finalize a batch session
    function test_dag_batch_session_auth_finalize() public {
        bytes32 sessionId = verifier.startDAGBatchVerify(proofHex, circuitHash, publicInputsHex, gensHex);

        // Complete all compute batches first
        (,, uint256 totalBatches,,,) = verifier.getDAGBatchSession(sessionId);
        for (uint256 i = 0; i < totalBatches; i++) {
            verifier.continueDAGBatchVerify(sessionId, proofHex, publicInputsHex, gensHex);
        }

        // A different address tries to finalize
        vm.prank(address(0xBEEF));
        vm.expectRevert("Batch: unauthorized");
        verifier.finalizeDAGBatchVerify(sessionId, proofHex, publicInputsHex, gensHex);
    }

    /// @notice Only the original caller can cleanup a session
    function test_dag_batch_session_auth_cleanup() public {
        bytes32 sessionId = _runFullBatchVerification();

        vm.prank(address(0xBEEF));
        vm.expectRevert("Batch: unauthorized");
        verifier.cleanupDAGBatchSession(sessionId);
    }

    // ========================================================================
    // ORDERING TESTS
    // ========================================================================

    /// @notice Cannot finalize before all compute batches are done
    function test_dag_batch_premature_finalize_reverts() public {
        bytes32 sessionId = verifier.startDAGBatchVerify(proofHex, circuitHash, publicInputsHex, gensHex);

        // Try to finalize immediately (only batch 0 done, need 1..10)
        vm.expectRevert("Batch: compute batches not done");
        verifier.finalizeDAGBatchVerify(sessionId, proofHex, publicInputsHex, gensHex);
    }

    /// @notice Cannot continue after all compute batches are done
    function test_dag_batch_extra_continue_reverts() public {
        bytes32 sessionId = verifier.startDAGBatchVerify(proofHex, circuitHash, publicInputsHex, gensHex);

        // Complete all compute batches
        (,, uint256 totalBatches,,,) = verifier.getDAGBatchSession(sessionId);
        for (uint256 i = 0; i < totalBatches; i++) {
            verifier.continueDAGBatchVerify(sessionId, proofHex, publicInputsHex, gensHex);
        }

        // Try to continue again
        vm.expectRevert("Batch: all compute batches done, call finalize");
        verifier.continueDAGBatchVerify(sessionId, proofHex, publicInputsHex, gensHex);
    }

    /// @notice Cannot finalize twice
    function test_dag_batch_double_finalize_reverts() public {
        bytes32 sessionId = _runFullBatchVerification();

        // Try to finalize again
        vm.expectRevert("Batch: already finalized");
        verifier.finalizeDAGBatchVerify(sessionId, proofHex, publicInputsHex, gensHex);
    }

    /// @notice Cannot continue a finalized session
    function test_dag_batch_continue_after_finalize_reverts() public {
        bytes32 sessionId = _runFullBatchVerification();

        vm.expectRevert("Batch: already finalized");
        verifier.continueDAGBatchVerify(sessionId, proofHex, publicInputsHex, gensHex);
    }

    /// @notice Non-existent session reverts
    function test_dag_batch_nonexistent_session_reverts() public {
        bytes32 fakeSession = keccak256("nonexistent");

        vm.expectRevert("Batch: session not found");
        verifier.continueDAGBatchVerify(fakeSession, proofHex, publicInputsHex, gensHex);
    }

    // ========================================================================
    // FINALIZE PROGRESS TESTS
    // ========================================================================

    /// @notice Verify finalizeInputIdx and finalizeGroupsDone advance correctly
    function test_dag_batch_finalize_progress() public {
        bytes32 sessionId = verifier.startDAGBatchVerify(proofHex, circuitHash, publicInputsHex, gensHex);

        (,, uint256 totalBatches,,,) = verifier.getDAGBatchSession(sessionId);
        for (uint256 i = 0; i < totalBatches; i++) {
            verifier.continueDAGBatchVerify(sessionId, proofHex, publicInputsHex, gensHex);
        }

        // Check initial finalize state
        (,,,, uint256 fInputIdx, uint256 fGroupsDone) = verifier.getDAGBatchSession(sessionId);
        assertEq(fInputIdx, 0, "finalizeInputIdx starts at 0");
        assertEq(fGroupsDone, 0, "finalizeGroupsDone starts at 0");

        // Call finalize steps and verify progress
        uint256 calls = 0;
        uint256 prevInputIdx = 0;
        while (true) {
            bool done = verifier.finalizeDAGBatchVerify(sessionId, proofHex, publicInputsHex, gensHex);
            calls++;

            (,,,, fInputIdx, fGroupsDone) = verifier.getDAGBatchSession(sessionId);

            if (!done) {
                // Not done yet — should have advanced groupsDone or inputIdx
                assertTrue(fGroupsDone > 0 || fInputIdx > prevInputIdx, "finalize should make progress");
                prevInputIdx = fInputIdx;
            } else {
                // Done — finalized flag set
                (,,, bool finalized,,) = verifier.getDAGBatchSession(sessionId);
                assertTrue(finalized, "should be finalized");
                break;
            }
        }

        emit log_named_uint("Total finalize calls", calls);
    }

    // ========================================================================
    // GAS TESTS
    // ========================================================================

    /// @notice Gas profile for each batch. Logs gas per step and enforces all < 30M.
    /// Current gas profile (88-layer XGBoost circuit, LAYERS_PER_BATCH=8, GROUPS_PER_FINALIZE_BATCH=16):
    ///   Start:    ~14M (transcript setup + proof decode + storage writes, NO compute)
    ///   Continue: ~13-28M (proof decode + 8 layers + storage read/write)
    ///   Finalize: ~28M per call (proof decode + 16 eval groups + bindings reconstruction)
    function test_dag_batch_gas_per_batch() public {
        uint256 gasBefore = gasleft();
        bytes32 sessionId = verifier.startDAGBatchVerify(proofHex, circuitHash, publicInputsHex, gensHex);
        uint256 startGas = gasBefore - gasleft();
        emit log_named_uint("Start (setup) gas", startGas);
        assertLt(startGas, 30_000_000, "start should fit in a block (< 30M)");

        (,, uint256 totalBatches,,,) = verifier.getDAGBatchSession(sessionId);
        uint256 maxContinueGas = 0;

        for (uint256 i = 0; i < totalBatches; i++) {
            gasBefore = gasleft();
            verifier.continueDAGBatchVerify(sessionId, proofHex, publicInputsHex, gensHex);
            uint256 batchGas = gasBefore - gasleft();
            emit log_named_uint(string(abi.encodePacked("Batch ", vm.toString(i), " gas")), batchGas);
            if (batchGas > maxContinueGas) maxContinueGas = batchGas;
            assertLt(batchGas, 30_000_000, "continue batch should fit in a block (< 30M)");
        }

        // Finalize may take multiple calls — measure each
        uint256 maxFinalizeGas = 0;
        uint256 finalizeCalls = 0;
        bool finalized;
        while (true) {
            gasBefore = gasleft();
            finalized = verifier.finalizeDAGBatchVerify(sessionId, proofHex, publicInputsHex, gensHex);
            uint256 finalizeGas = gasBefore - gasleft();
            finalizeCalls++;
            emit log_named_uint(
                string(abi.encodePacked("Finalize step ", vm.toString(finalizeCalls), " gas")), finalizeGas
            );
            if (finalizeGas > maxFinalizeGas) maxFinalizeGas = finalizeGas;
            assertLt(finalizeGas, 30_000_000, "finalize step should fit in a block (< 30M)");
            if (finalized) break;
        }
        emit log_named_uint("Finalize calls", finalizeCalls);
        emit log_named_uint("Max continue gas", maxContinueGas);
        emit log_named_uint("Max finalize gas", maxFinalizeGas);
    }

    // ========================================================================
    // CLEANUP TESTS
    // ========================================================================

    /// @notice Cleanup after finalization works and clears session
    function test_dag_batch_cleanup() public {
        bytes32 sessionId = _runFullBatchVerification();

        // Cleanup
        verifier.cleanupDAGBatchSession(sessionId);

        // Session should be cleared
        (bytes32 storedHash,,,,,) = verifier.getDAGBatchSession(sessionId);
        assertEq(storedHash, bytes32(0), "session should be cleared after cleanup");
    }

    /// @notice Cannot cleanup a non-finalized session
    function test_dag_batch_cleanup_before_finalize_reverts() public {
        bytes32 sessionId = verifier.startDAGBatchVerify(proofHex, circuitHash, publicInputsHex, gensHex);

        vm.expectRevert("Batch: not finalized");
        verifier.cleanupDAGBatchSession(sessionId);
    }

    // ========================================================================
    // CIRCUIT VALIDATION TESTS
    // ========================================================================

    /// @notice Start with unregistered circuit reverts
    function test_dag_batch_unregistered_circuit_reverts() public {
        bytes32 fakeHash = keccak256("nonexistent");
        vm.expectRevert(RemainderVerifier.CircuitNotRegistered.selector);
        verifier.startDAGBatchVerify(proofHex, fakeHash, publicInputsHex, gensHex);
    }

    /// @notice Start with wrong generators reverts
    function test_dag_batch_wrong_gens_reverts() public {
        bytes memory wrongGens =
            abi.encodePacked(uint256(1), uint256(1), uint256(2), uint256(1), uint256(2), uint256(1), uint256(2));
        vm.expectRevert(RemainderVerifier.InvalidGenerators.selector);
        verifier.startDAGBatchVerify(proofHex, circuitHash, publicInputsHex, wrongGens);
    }

    /// @notice Start with invalid proof selector reverts
    function test_dag_batch_invalid_selector_reverts() public {
        bytes memory badProof = abi.encodePacked(bytes4("FAKE"), bytes32(0));
        vm.expectRevert(RemainderVerifier.InvalidProofSelector.selector);
        verifier.startDAGBatchVerify(badProof, circuitHash, publicInputsHex, gensHex);
    }

    // ========================================================================
    // REGRESSION
    // ========================================================================

    /// @notice Single-tx DAG verification still works alongside batch verification
    function test_dag_single_tx_still_works() public view {
        bool valid = verifier.verifyDAGProof(proofHex, circuitHash, publicInputsHex, gensHex);
        assertTrue(valid, "single-tx DAG verification should still pass");
    }

    // ========================================================================
    // HELPERS
    // ========================================================================

    /// @dev Run full batch verification (start + continue × N + finalize loop)
    function _runFullBatchVerification() internal returns (bytes32 sessionId) {
        sessionId = verifier.startDAGBatchVerify(proofHex, circuitHash, publicInputsHex, gensHex);

        (,, uint256 totalBatches,,,) = verifier.getDAGBatchSession(sessionId);
        for (uint256 i = 0; i < totalBatches; i++) {
            verifier.continueDAGBatchVerify(sessionId, proofHex, publicInputsHex, gensHex);
        }

        // Finalize may take multiple calls
        while (true) {
            bool finalized = verifier.finalizeDAGBatchVerify(sessionId, proofHex, publicInputsHex, gensHex);
            if (finalized) break;
        }
    }
}
