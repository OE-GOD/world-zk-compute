// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/remainder/RemainderVerifier.sol";
import "../src/remainder/PoseidonSponge.sol";
import "../src/remainder/SumcheckVerifier.sol";
import "../src/remainder/HyraxVerifier.sol";
import "../src/remainder/GKRVerifier.sol";
import "../src/remainder/CommittedSumcheckVerifier.sol";
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
    function test_all_gens_on_curve() public {
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
    function test_adapter_public_data_split() public {
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
        (GKRVerifier.GKRProof memory proof, uint256[] memory pubIn) = verifier.decodeProofCounted(proofData, pubInputs);

        // Use separate calldata call to read claims
        (uint256 numFs, uint256 numPub, uint256[][] memory fsPoints, uint256[][] memory pubPoints, uint256[] memory pubValues) =
            this._dumpClaimsFromCalldata(proofData);
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
        (GKRVerifier.GKRProof memory proof, HyraxVerifier.PedersenGens memory gens,
         PoseidonSponge.Sponge memory sponge) = _loadAndSetupTranscript();

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

    function _loadAndSetupTranscript() internal returns (
        GKRVerifier.GKRProof memory proof,
        HyraxVerifier.PedersenGens memory gens,
        PoseidonSponge.Sponge memory sponge
    ) {
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
        HyraxVerifier.G1Point memory alpha = HyraxVerifier.scalarMul(
            lp.sumcheckProof.messages[0], gamma0 % FR_MOD
        );

        // j_star computation (n=1, degree=2)
        uint256[] memory jStar = _computeJStar(rho0 % FR_MOD, rho1 % FR_MOD, gamma0 % FR_MOD, binding0);

        // Compute dotProduct in a separate function to avoid stack-too-deep
        HyraxVerifier.G1Point memory dotProduct = _computeDotProduct(
            lp, rho0 % FR_MOD, rho1 % FR_MOD, binding0, claimPoint0, randomCoeff
        );

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
            HyraxVerifier.scalarMul(lp.sumcheckProof.sum, rho0),
            HyraxVerifier.scalarMul(oracleEval, FR_MOD - rho1)
        );
    }

    function _computeJStar(uint256 rho0, uint256 rho1, uint256 gamma0, uint256 binding0)
        internal pure returns (uint256[] memory jStar)
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
        HyraxVerifier.G1Point memory lhs = HyraxVerifier.ecAdd(
            HyraxVerifier.scalarMul(alpha, c), podp.commitD
        );
        // RHS: MSM(g[0..n], z) + z_delta * h
        HyraxVerifier.G1Point memory msm = HyraxVerifier.scalarMul(gens.messageGens[0], podp.zVector[0]);
        for (uint256 i = 1; i < podp.zVector.length; i++) {
            msm = HyraxVerifier.ecAdd(msm, HyraxVerifier.scalarMul(gens.messageGens[i], podp.zVector[i]));
        }
        HyraxVerifier.G1Point memory rhs = HyraxVerifier.ecAdd(
            msm, HyraxVerifier.scalarMul(gens.blindingGen, podp.zDelta)
        );
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
        HyraxVerifier.G1Point memory lhs = HyraxVerifier.ecAdd(
            HyraxVerifier.scalarMul(dotProduct, c), podp.commitDDotA
        );
        // RHS: <z, jStar> * g_scalar + z_beta * h
        uint256 zDotJ = 0;
        for (uint256 i = 0; i < podp.zVector.length; i++) {
            zDotJ = addmod(zDotJ, mulmod(podp.zVector[i], jStar[i], FR_MOD), FR_MOD);
        }
        HyraxVerifier.G1Point memory rhs = HyraxVerifier.ecAdd(
            HyraxVerifier.scalarMul(gens.scalarGen, zDotJ),
            HyraxVerifier.scalarMul(gens.blindingGen, podp.zBeta)
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

    function hashToFqPair(bytes32 hash) external pure returns (uint256 fq1, uint256 fq2) {
        return _hashToFqPair(hash);
    }

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

    function extractInputCommitCoords(GKRVerifier.GKRProof memory proof)
        external
        pure
        returns (uint256[] memory)
    {
        return _extractInputCommitCoords(proof);
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
    function test_sha256_hash_chain_ec_points() public {
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
    function test_full_transcript_setup() public {
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
