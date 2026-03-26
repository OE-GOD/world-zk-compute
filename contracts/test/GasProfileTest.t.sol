// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/tee/TEEMLVerifier.sol";
import "../src/ExecutionEngine.sol";
import "../src/ProgramRegistry.sol";
import "../src/mocks/MockRiscZeroVerifier.sol";
import {IProofVerifier} from "../src/IProofVerifier.sol";
import {DeployTEEMLVerifierHelper} from "./helpers/DeployTEEMLVerifier.sol";

/// @dev Mock RemainderVerifier that returns a configurable result for verifyDAGProof
contract GasMockRemainderVerifier {
    bool public nextResult;

    function setResult(bool _result) external {
        nextResult = _result;
    }

    function verifyDAGProof(bytes calldata, bytes32, bytes calldata, bytes calldata) external view returns (bool) {
        return nextResult;
    }
}

/// @dev Mock IProofVerifier that always accepts proofs (for ExecutionEngine submitProof tests)
contract MockProofVerifierForGas is IProofVerifier {
    function verify(bytes calldata, bytes32, bytes calldata) external pure override {
        // Accept all proofs
    }

    function proofSystem() external pure override returns (string memory) {
        return "mock";
    }
}

/// @dev Minimal callback for gas measurement of submitProof with callback
contract MockCallbackForGas is IExecutionCallback {
    function onExecutionComplete(uint256, bytes32, bytes calldata) external {}
}

/// @title GasProfileTest
/// @notice Dedicated gas measurement tests for TEEMLVerifier and ExecutionEngine.
///         Each test performs minimal setup followed by one measured operation so that
///         Foundry's gas report and snapshot capture clean measurements.
///
///         Run with:  forge test --match-contract GasProfileTest -vv --gas-report
///         Snapshot:  forge snapshot --match-contract GasProfileTest
contract GasProfileTest is Test, DeployTEEMLVerifierHelper {
    // ========================================================================
    // TEEMLVerifier state
    // ========================================================================

    TEEMLVerifier teeVerifier;
    GasMockRemainderVerifier mockRemainderVerifier;

    address admin = address(this);
    uint256 enclavePrivateKey = 0xA11CE;
    address enclaveAddr;
    bytes32 imageHash = keccak256("test-enclave-image-v1");

    bytes32 modelHash = keccak256("xgboost-model-weights");
    bytes32 inputHash = keccak256("test-input-data");
    bytes resultData = hex"deadbeef";

    uint256 constant PROVER_STAKE = 0.1 ether;
    uint256 constant CHALLENGE_BOND = 0.1 ether;

    // ========================================================================
    // ExecutionEngine state
    // ========================================================================

    ExecutionEngine engine;
    ProgramRegistry registry;
    MockRiscZeroVerifier mockRiscVerifier;
    MockProofVerifierForGas mockProofVerifier;

    address eeDeployer = address(0xDE01);
    address requester = address(0xAA01);
    address prover = address(0xBB01);
    address feeRecipient = address(0xCC01);

    bytes32 programImageId = bytes32(uint256(42));
    bytes32 programInputDigest = keccak256("gas-test-inputs");
    string programInputUrl = "ipfs://QmGasTest";

    // Allow test contract to receive ETH (for TEEMLVerifier stake returns)
    receive() external payable {}

    // ========================================================================
    // Setup
    // ========================================================================

    function setUp() public {
        // --- TEEMLVerifier setup ---
        enclaveAddr = vm.addr(enclavePrivateKey);
        mockRemainderVerifier = new GasMockRemainderVerifier();
        teeVerifier = _deployTEEMLVerifier(admin, address(mockRemainderVerifier));

        // Pre-register enclave (most tests need it)
        teeVerifier.registerEnclave(enclaveAddr, imageHash);

        // --- ExecutionEngine setup ---
        vm.startPrank(eeDeployer);
        mockRiscVerifier = new MockRiscZeroVerifier();
        registry = new ProgramRegistry(eeDeployer);
        mockProofVerifier = new MockProofVerifierForGas();
        engine = new ExecutionEngine(eeDeployer, address(registry), address(mockRiscVerifier), feeRecipient);

        // Register a test program with the mock proof verifier so submitProof uses
        // the lightweight mock path instead of the risc0 verifier
        registry.registerProgramWithVerifier(
            programImageId,
            "Gas Test Program",
            "https://example.com/gas.elf",
            bytes32(0),
            address(mockProofVerifier),
            "mock"
        );
        vm.stopPrank();

        // Fund accounts generously
        vm.deal(requester, 100 ether);
        vm.deal(prover, 100 ether);
        vm.deal(admin, 100 ether);
        vm.deal(address(this), 100 ether);
    }

    // ========================================================================
    // TEEMLVerifier helpers
    // ========================================================================

    /// @dev Produce a valid EIP-712 attestation signed by the test enclave key
    function _signAttestation(bytes32 _modelHash, bytes32 _inputHash, bytes memory _result)
        internal
        view
        returns (bytes memory attestation)
    {
        bytes32 resultHash = keccak256(_result);
        bytes32 structHash = keccak256(abi.encode(teeVerifier.RESULT_TYPEHASH(), _modelHash, _inputHash, resultHash));
        bytes32 domainSeparator = keccak256(
            abi.encode(
                keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"),
                keccak256("TEEMLVerifier"),
                keccak256("1"),
                block.chainid,
                address(teeVerifier)
            )
        );
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", domainSeparator, structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, digest);
        attestation = abi.encodePacked(r, s, v);
    }

    /// @dev Submit a result using default parameters (enclave already registered in setUp)
    function _submitDefaultResult() internal returns (bytes32 resultId) {
        bytes memory attestation = _signAttestation(modelHash, inputHash, resultData);
        resultId = teeVerifier.submitResult{value: PROVER_STAKE}(modelHash, inputHash, resultData, attestation);
    }

    /// @dev Submit a result and challenge it, returning the resultId
    function _submitAndChallenge() internal returns (bytes32 resultId) {
        resultId = _submitDefaultResult();
        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        teeVerifier.challenge{value: CHALLENGE_BOND}(resultId);
    }

    // ========================================================================
    // ExecutionEngine helpers
    // ========================================================================

    /// @dev Create a pending execution request
    function _createRequest() internal returns (uint256 requestId) {
        vm.prank(requester);
        requestId = engine.requestExecution{value: 0.1 ether}(
            programImageId, programInputDigest, programInputUrl, address(0), 3600
        );
    }

    /// @dev Create and claim an execution request
    function _createAndClaimRequest() internal returns (uint256 requestId) {
        requestId = _createRequest();
        vm.prank(prover);
        engine.claimExecution(requestId);
    }

    // ========================================================================
    // TEEMLVerifier Gas Tests
    // ========================================================================

    /// @notice Gas: TEEMLVerifier.registerEnclave
    ///         Measures cold storage write for new enclave registration.
    function test_gas_registerEnclave() public {
        address newEnclaveKey = address(0x9999);
        bytes32 newImageHash = keccak256("new-enclave-image");

        uint256 gasBefore = gasleft();
        teeVerifier.registerEnclave(newEnclaveKey, newImageHash);
        uint256 gasUsed = gasBefore - gasleft();

        // Verify
        ITEEMLVerifier.EnclaveInfo memory info = teeVerifier.enclaves(newEnclaveKey);
        assertTrue(info.registered);
        assertTrue(info.active);

        emit log_named_uint("gas_registerEnclave", gasUsed);
    }

    /// @notice Gas: TEEMLVerifier.submitResult
    ///         Measures ECDSA recovery + cold storage write + event emission.
    ///         This is the TEE happy-path hot function.
    function test_gas_submitResult() public {
        bytes memory attestation = _signAttestation(modelHash, inputHash, resultData);

        uint256 gasBefore = gasleft();
        bytes32 resultId = teeVerifier.submitResult{value: PROVER_STAKE}(modelHash, inputHash, resultData, attestation);
        uint256 gasUsed = gasBefore - gasleft();

        // Verify
        ITEEMLVerifier.MLResult memory r = teeVerifier.getResult(resultId);
        assertEq(r.enclave, enclaveAddr);
        assertFalse(r.finalized);

        emit log_named_uint("gas_submitResult", gasUsed);
    }

    /// @notice Gas: TEEMLVerifier.challenge
    ///         Measures challenge with bond deposit.
    function test_gas_challenge() public {
        bytes32 resultId = _submitDefaultResult();

        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);

        uint256 gasBefore = gasleft();
        teeVerifier.challenge{value: CHALLENGE_BOND}(resultId);
        uint256 gasUsed = gasBefore - gasleft();

        // Verify
        ITEEMLVerifier.MLResult memory r = teeVerifier.getResult(resultId);
        assertTrue(r.challenged);
        assertEq(r.challenger, challenger);

        emit log_named_uint("gas_challenge", gasUsed);
    }

    /// @notice Gas: TEEMLVerifier.finalize
    ///         Measures finalization of unchallenged result (includes ETH stake return).
    function test_gas_finalize() public {
        bytes32 resultId = _submitDefaultResult();

        // Warp past the challenge window (1 hour)
        vm.warp(block.timestamp + 1 hours + 1);

        uint256 gasBefore = gasleft();
        teeVerifier.finalize(resultId);
        uint256 gasUsed = gasBefore - gasleft();

        // Verify
        ITEEMLVerifier.MLResult memory r = teeVerifier.getResult(resultId);
        assertTrue(r.finalized);
        assertTrue(teeVerifier.isResultValid(resultId));

        emit log_named_uint("gas_finalize", gasUsed);
    }

    /// @notice Gas: TEEMLVerifier.resolveDisputeByTimeout
    ///         Measures timeout resolution (challenger wins, ETH payout of combined pot).
    function test_gas_resolveDisputeByTimeout() public {
        bytes32 resultId = _submitAndChallenge();

        // Warp past dispute deadline (24 hours)
        vm.warp(block.timestamp + 24 hours);

        uint256 gasBefore = gasleft();
        teeVerifier.resolveDisputeByTimeout(resultId);
        uint256 gasUsed = gasBefore - gasleft();

        // Verify
        assertTrue(teeVerifier.disputeResolved(resultId));
        assertFalse(teeVerifier.disputeProverWon(resultId));

        emit log_named_uint("gas_resolveDisputeByTimeout", gasUsed);
    }

    // ========================================================================
    // ExecutionEngine Gas Tests
    // ========================================================================

    /// @notice Gas: ExecutionEngine.requestExecution
    ///         Measures request creation with tip (8 storage slots + event).
    function test_gas_requestExecution() public {
        vm.prank(requester);

        uint256 gasBefore = gasleft();
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(
            programImageId, programInputDigest, programInputUrl, address(0), 3600
        );
        uint256 gasUsed = gasBefore - gasleft();

        // Verify
        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);
        assertEq(req.requester, requester);
        assertEq(uint256(req.status), uint256(ExecutionEngine.RequestStatus.Pending));

        emit log_named_uint("gas_requestExecution", gasUsed);
    }

    /// @notice Gas: ExecutionEngine.claimExecution
    ///         Measures claim of a pending request by a prover.
    function test_gas_claimExecution() public {
        uint256 requestId = _createRequest();

        vm.prank(prover);

        uint256 gasBefore = gasleft();
        engine.claimExecution(requestId);
        uint256 gasUsed = gasBefore - gasleft();

        // Verify
        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);
        assertEq(req.claimedBy, prover);
        assertEq(uint256(req.status), uint256(ExecutionEngine.RequestStatus.Claimed));

        emit log_named_uint("gas_claimExecution", gasUsed);
    }

    /// @notice Gas: ExecutionEngine.submitProof
    ///         Measures proof submission with mock verifier (verification + payout + fee split + stats).
    function test_gas_submitProof() public {
        uint256 requestId = _createAndClaimRequest();

        bytes memory seal = hex"deadbeef";
        bytes memory journal = hex"cafebabe";

        vm.prank(prover);

        uint256 gasBefore = gasleft();
        engine.submitProof(requestId, seal, journal);
        uint256 gasUsed = gasBefore - gasleft();

        // Verify
        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);
        assertEq(uint256(req.status), uint256(ExecutionEngine.RequestStatus.Completed));

        emit log_named_uint("gas_submitProof", gasUsed);
    }
}
