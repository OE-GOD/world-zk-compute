// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/remainder/RemainderVerifier.sol";
import "../src/remainder/PoseidonSponge.sol";
import "../src/remainder/SumcheckVerifier.sol";
import "../src/remainder/HyraxVerifier.sol";
import "../src/remainder/GKRVerifier.sol";
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
        assertEq(sponge.state[0], 0);
        assertEq(sponge.state[1], 0);
        assertEq(sponge.state[2], 0);
        assertEq(sponge.absorb_pos, 0);
        assertFalse(sponge.squeezed);
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
        assertTrue(out1 < PoseidonSponge.FQ_MODULUS, "Output should be < Fq");
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

    /// @notice Test absorbing a point
    function test_absorb_point() public pure {
        PoseidonSponge.Sponge memory sponge = PoseidonSponge.init();

        // Absorb the BN254 generator point
        PoseidonSponge.absorbPoint(sponge, 1, 2);

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

    /// @notice Test S-box (x^5)
    function test_sbox() public pure {
        // x = 2, x^5 = 32 (in field arithmetic)
        uint256 result = PoseidonSponge.sbox(2);
        assertEq(result, 32);

        // x = 0
        assertEq(PoseidonSponge.sbox(0), 0);

        // x = 1
        assertEq(PoseidonSponge.sbox(1), 1);
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
        verifier.verifyProof(proof, fakeHash, pubInputs);
    }

    function test_reject_wrong_selector() public {
        bytes memory proof = abi.encodePacked(bytes4("FAKE"), bytes32(0));
        bytes memory pubInputs = abi.encodePacked(uint256(1));

        vm.expectRevert(RemainderVerifier.InvalidProofSelector.selector);
        verifier.verifyProof(proof, circuitHash, pubInputs);
    }

    function test_reject_empty_proof() public {
        bytes memory proof = "";
        bytes memory pubInputs = abi.encodePacked(uint256(1));

        vm.expectRevert(RemainderVerifier.InvalidProofLength.selector);
        verifier.verifyProof(proof, circuitHash, pubInputs);
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
