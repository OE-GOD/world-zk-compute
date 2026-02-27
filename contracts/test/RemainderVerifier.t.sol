// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/remainder/RemainderVerifier.sol";
import "../src/remainder/PoseidonSponge.sol";
import "../src/remainder/SumcheckVerifier.sol";
import "../src/remainder/HyraxVerifier.sol";
import "../src/remainder/GKRVerifier.sol";
import "../src/remainder/HyraxProofDecoder.sol";
import "../src/IProofVerifier.sol";
import "../src/RemainderVerifierAdapter.sol";
import "../src/RiscZeroVerifierAdapter.sol";
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
        vm.expectRevert(RemainderVerifier.NotAdmin.selector);
        uint256[] memory sizes = new uint256[](1);
        uint8[] memory types = new uint8[](1);
        bool[] memory committed = new bool[](1);
        verifier.registerCircuit(keccak256("another"), 1, sizes, types, committed, "test");
    }

    function test_admin_transfer() public {
        address newAdmin = address(42);
        vm.prank(admin);
        verifier.transferAdmin(newAdmin);
        assertEq(verifier.admin(), newAdmin);
    }

    function test_get_circuit_hashes() public view {
        bytes32[] memory hashes = verifier.getCircuitHashes();
        assertEq(hashes.length, 1);
        assertEq(hashes[0], circuitHash);
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
        registry = new ProgramRegistry();
        mockVerifier = new MockRiscZeroVerifier();
        remainderVerifier = new RemainderVerifier(deployer);

        // Deploy adapters
        remainderAdapter = new RemainderVerifierAdapter(address(remainderVerifier));

        // Deploy execution engine
        engine = new ExecutionEngine(address(registry), address(mockVerifier), feeRecipient);

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
    function test_e2e_commitment_points_match_fixture() public {
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
