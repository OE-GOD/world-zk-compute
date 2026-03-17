// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/ExecutionEngine.sol";
import "../src/ProgramRegistry.sol";
import "../src/ProverRegistry.sol";
import "../src/IProofVerifier.sol";
import "../src/remainder/RemainderVerifier.sol";
import "../src/tee/TEEMLVerifier.sol";
import "../src/mocks/MockRiscZeroVerifier.sol";
import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {GKRDAGVerifier} from "../src/remainder/GKRDAGVerifier.sol";

// ========================================================================
// MOCKS
// ========================================================================

/// @dev Mock IProofVerifier that checks proof has "REM1" prefix (validates routing, skips GKR)
contract MockDAGVerifierAdapter is IProofVerifier {
    function verify(bytes calldata proofData, bytes32, bytes calldata) external pure override {
        require(proofData.length >= 4, "proof too short");
        require(bytes4(proofData[:4]) == bytes4("REM1"), "missing REM1 prefix");
    }

    function proofSystem() external pure override returns (string memory) {
        return "remainder-mock";
    }
}

/// @dev Mock RemainderVerifier for TEE tests — returns configurable bool
contract MockRemainderForTEE {
    bool public nextResult;

    function setResult(bool _result) external {
        nextResult = _result;
    }

    function verifyDAGProof(bytes calldata, bytes32, bytes calldata, bytes calldata) external view returns (bool) {
        return nextResult;
    }
}

/// @dev Minimal ERC20 for ProverRegistry staking tests
contract MockToken is ERC20 {
    constructor() ERC20("MockStake", "MST") {
        _mint(msg.sender, 1_000_000e18);
    }

    function mint(address to, uint256 amount) external {
        _mint(to, amount);
    }
}

/// @dev IExecutionCallback that records calls for assertion
contract MockCallback is IExecutionCallback {
    uint256 public lastRequestId;
    bytes32 public lastImageId;
    bytes public lastJournal;
    uint256 public callCount;

    function onExecutionComplete(uint256 requestId, bytes32 imageId, bytes calldata journal) external override {
        lastRequestId = requestId;
        lastImageId = imageId;
        lastJournal = journal;
        callCount++;
    }
}

// ========================================================================
// SHARED FIXTURE BASE (DAG circuit loading)
// ========================================================================

contract IntegrationFixtureBase is Test {
    RemainderVerifier remainderVerifier;

    bytes proofHex;
    bytes gensHex;
    bytes32 circuitHash;
    bytes publicInputsHex;

    function _deployAndRegisterDAGCircuit() internal {
        remainderVerifier = new RemainderVerifier(address(this));
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

        remainderVerifier.registerDAGCircuit(circuitHash, abi.encode(desc), "xgboost-integration", keccak256(gensHex));
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
}

// ========================================================================
// TEST 1: ExecutionEngine + ProgramRegistry + Mock Verifier Pipeline
// ========================================================================

contract IntegrationE2E_EnginePipelineTest is IntegrationFixtureBase {
    MockRiscZeroVerifier mockRiscVerifier;
    ProgramRegistry registry;
    ExecutionEngine engine;
    MockDAGVerifierAdapter mockAdapter;
    MockCallback callback;

    address admin = address(this);
    address feeRecipient = address(0xFEE);
    address requester = address(0xA1);
    address prover = address(0xB1);

    bytes32 programImageId = keccak256("integration-test-program");
    bytes32 inputDigest = keccak256("test-inputs");

    receive() external payable {}

    function setUp() public {
        // Deploy core contracts
        mockRiscVerifier = new MockRiscZeroVerifier();
        registry = new ProgramRegistry(admin);
        mockAdapter = new MockDAGVerifierAdapter();
        engine = new ExecutionEngine(admin, address(registry), address(mockRiscVerifier), feeRecipient);
        callback = new MockCallback();

        // Register program with mock adapter as custom verifier
        registry.registerProgramWithVerifier(
            programImageId,
            "IntegrationTest",
            "https://example.com/test",
            bytes32(0),
            address(mockAdapter),
            "remainder-mock"
        );

        // Deploy real DAG verifier for the direct DAG test
        _deployAndRegisterDAGCircuit();

        // Fund accounts
        vm.deal(requester, 10 ether);
        vm.deal(prover, 10 ether);
        vm.deal(feeRecipient, 0);
    }

    /// @notice Full lifecycle: request → claim → submitProof → completed
    function test_full_request_lifecycle() public {
        // 1. Requester creates request with callback
        vm.prank(requester);
        uint256 reqId = engine.requestExecution{value: 0.5 ether}(
            programImageId, inputDigest, "https://inputs.example.com/1", address(callback), 3600
        );

        // Verify request created
        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(reqId);
        assertEq(uint8(req.status), uint8(ExecutionEngine.RequestStatus.Pending));
        assertEq(req.requester, requester);
        assertEq(req.tip, 0.5 ether);

        // 2. Prover claims
        vm.prank(prover);
        engine.claimExecution(reqId);
        req = engine.getRequest(reqId);
        assertEq(uint8(req.status), uint8(ExecutionEngine.RequestStatus.Claimed));
        assertEq(req.claimedBy, prover);

        // 3. Prover submits proof (seal has REM1 prefix for mock adapter)
        bytes memory seal = abi.encodePacked(bytes4("REM1"), bytes32(uint256(42)));
        bytes memory journal = abi.encodePacked(bytes32(uint256(1)));

        uint256 proverBalBefore = prover.balance;
        vm.prank(prover);
        engine.submitProof(reqId, seal, journal);

        // 4. Verify completed
        req = engine.getRequest(reqId);
        assertEq(uint8(req.status), uint8(ExecutionEngine.RequestStatus.Completed));

        // Prover received payment
        assertTrue(prover.balance > proverBalBefore, "prover should have been paid");

        // Stats updated
        (uint256 completed, uint256 earnings) = engine.getProverStats(prover);
        assertEq(completed, 1);
        assertTrue(earnings > 0);
    }

    /// @notice Verify fee math: prover gets (100% - 2.5%), feeRecipient gets 2.5%
    function test_prover_payment_and_protocol_fee() public {
        uint256 tipAmount = 1 ether;

        vm.prank(requester);
        uint256 reqId = engine.requestExecution{value: tipAmount}(
            programImageId, inputDigest, "https://inputs.example.com/2", address(0), 3600
        );

        vm.prank(prover);
        engine.claimExecution(reqId);

        uint256 proverBalBefore = prover.balance;
        uint256 feeBalBefore = feeRecipient.balance;

        // Submit immediately (no decay since same block)
        bytes memory seal = abi.encodePacked(bytes4("REM1"), bytes32(uint256(1)));
        bytes memory journal = abi.encodePacked(bytes32(uint256(1)));
        vm.prank(prover);
        engine.submitProof(reqId, seal, journal);

        uint256 proverReceived = prover.balance - proverBalBefore;
        uint256 feeReceived = feeRecipient.balance - feeBalBefore;

        // Fee = payout * 250 / 10000 = 2.5%
        // Since submitted in same block, payout = maxTip = tipAmount
        uint256 expectedFee = (tipAmount * 250) / 10000;
        uint256 expectedProverPayout = tipAmount - expectedFee;

        assertEq(feeReceived, expectedFee, "fee recipient should get 2.5%");
        assertEq(proverReceived, expectedProverPayout, "prover should get 97.5%");
        assertEq(proverReceived + feeReceived, tipAmount, "total payout should equal tip");
    }

    /// @notice Callback is invoked with correct data on proof submission
    function test_callback_receives_journal() public {
        vm.prank(requester);
        uint256 reqId = engine.requestExecution{value: 0.5 ether}(
            programImageId, inputDigest, "https://inputs.example.com/3", address(callback), 3600
        );

        vm.prank(prover);
        engine.claimExecution(reqId);

        bytes memory seal = abi.encodePacked(bytes4("REM1"), bytes32(uint256(99)));
        bytes memory journal = hex"deadbeefcafe";

        vm.prank(prover);
        engine.submitProof(reqId, seal, journal);

        // Callback was invoked
        assertEq(callback.callCount(), 1, "callback should have been called");
        assertEq(callback.lastRequestId(), reqId, "callback got correct requestId");
        assertEq(callback.lastImageId(), programImageId, "callback got correct imageId");
        assertEq(callback.lastJournal(), journal, "callback got correct journal");
    }

    /// @notice Real DAG proof verification directly on RemainderVerifier (254M gas)
    function test_dag_proof_verifies_directly() public {
        bool valid = remainderVerifier.verifyDAGProof(proofHex, circuitHash, publicInputsHex, gensHex);
        assertTrue(valid, "real DAG proof should verify");
    }
}

// ========================================================================
// TEST 2: TEE Happy Path + ZK Dispute
// ========================================================================

contract IntegrationE2E_TEEDisputeTest is Test {
    TEEMLVerifier teeVerifier;
    MockRemainderForTEE mockVerifier;

    address admin = address(this);
    uint256 enclavePrivateKey = 0xA11CE;
    address enclaveAddr;
    bytes32 imageHash = keccak256("integration-enclave-v1");

    bytes32 modelHash = keccak256("xgboost-model");
    bytes32 inputHash = keccak256("test-input");
    bytes resultData = hex"cafebabe";

    uint256 constant PROVER_STAKE = 0.1 ether;
    uint256 constant CHALLENGE_BOND = 0.1 ether;

    receive() external payable {}

    function setUp() public {
        enclaveAddr = vm.addr(enclavePrivateKey);
        mockVerifier = new MockRemainderForTEE();
        teeVerifier = new TEEMLVerifier(admin, address(mockVerifier));
        teeVerifier.registerEnclave(enclaveAddr, imageHash);

        vm.deal(address(this), 10 ether);
    }

    function _domainSeparator() internal view returns (bytes32) {
        return keccak256(
            abi.encode(
                keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"),
                keccak256("TEEMLVerifier"),
                keccak256("1"),
                block.chainid,
                address(teeVerifier)
            )
        );
    }

    function _signAttestation(bytes32 _modelHash, bytes32 _inputHash, bytes memory _result)
        internal
        view
        returns (bytes memory attestation)
    {
        bytes32 resultHash = keccak256(_result);
        bytes32 structHash = keccak256(abi.encode(teeVerifier.RESULT_TYPEHASH(), _modelHash, _inputHash, resultHash));
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", _domainSeparator(), structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, digest);
        attestation = abi.encodePacked(r, s, v);
    }

    function _submitDefault() internal returns (bytes32 resultId) {
        bytes memory attestation = _signAttestation(modelHash, inputHash, resultData);
        resultId = teeVerifier.submitResult{value: PROVER_STAKE}(modelHash, inputHash, resultData, attestation);
    }

    /// @notice TEE happy path: submit → wait → finalize → isResultValid
    function test_tee_happy_path_finalize() public {
        uint256 balBefore = address(this).balance;
        bytes32 resultId = _submitDefault();

        // Before challenge window passes, result is not valid
        assertFalse(teeVerifier.isResultValid(resultId));

        // Advance past challenge window (1 hour)
        vm.warp(block.timestamp + 1 hours + 1);
        teeVerifier.finalize(resultId);

        // Result is now valid, stake returned
        assertTrue(teeVerifier.isResultValid(resultId));
        assertEq(address(this).balance, balBefore, "prover stake should be returned");
    }

    /// @notice Dispute path: submit → challenge → ZK valid → prover wins
    function test_tee_dispute_prover_wins() public {
        bytes32 resultId = _submitDefault();

        // Challenger challenges
        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        teeVerifier.challenge{value: CHALLENGE_BOND}(resultId);

        ITEEMLVerifier.MLResult memory r = teeVerifier.getResult(resultId);
        assertTrue(r.challenged);

        // Resolve: ZK proof is valid → prover wins
        mockVerifier.setResult(true);
        uint256 submitterBalBefore = address(this).balance;
        teeVerifier.resolveDispute(resultId, hex"", bytes32(0), hex"", hex"");

        // Prover wins: gets stake + challenge bond
        assertTrue(teeVerifier.isResultValid(resultId));
        assertEq(address(this).balance, submitterBalBefore + PROVER_STAKE + CHALLENGE_BOND);
    }

    /// @notice Dispute path: submit → challenge → ZK invalid → challenger wins
    function test_tee_dispute_challenger_wins() public {
        bytes32 resultId = _submitDefault();

        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        teeVerifier.challenge{value: CHALLENGE_BOND}(resultId);

        // Resolve: ZK proof is invalid → challenger wins
        mockVerifier.setResult(false);
        uint256 challengerBalBefore = challenger.balance;
        teeVerifier.resolveDispute(resultId, hex"", bytes32(0), hex"", hex"");

        // Challenger wins: gets both stakes
        assertFalse(teeVerifier.isResultValid(resultId));
        assertEq(challenger.balance, challengerBalBefore + PROVER_STAKE + CHALLENGE_BOND);
    }

    /// @notice Dispute timeout: submit → challenge → timeout → challenger wins by default
    function test_tee_dispute_timeout() public {
        bytes32 resultId = _submitDefault();

        address challenger = address(0xC0FFEE);
        vm.deal(challenger, 1 ether);
        vm.prank(challenger);
        teeVerifier.challenge{value: CHALLENGE_BOND}(resultId);

        uint256 challengerBalBefore = challenger.balance;

        // Advance past dispute deadline (24 hours)
        vm.warp(block.timestamp + 24 hours);
        teeVerifier.resolveDisputeByTimeout(resultId);

        // Challenger wins by timeout
        assertFalse(teeVerifier.isResultValid(resultId));
        assertEq(challenger.balance, challengerBalBefore + PROVER_STAKE + CHALLENGE_BOND);
    }
}

// ========================================================================
// TEST 3: ProverRegistry Lifecycle
// ========================================================================

contract IntegrationE2E_ProverRegistryTest is Test {
    ProverRegistry proverRegistry;
    MockToken token;

    address prover1 = address(0xBB01);
    uint256 constant MIN_STAKE = 100e18;
    uint256 constant SLASH_BPS = 500; // 5%

    function setUp() public {
        token = new MockToken();
        proverRegistry = new ProverRegistry(address(token), MIN_STAKE, SLASH_BPS);

        // Give prover1 tokens and approve
        token.mint(prover1, 1000e18);
        vm.prank(prover1);
        token.approve(address(proverRegistry), type(uint256).max);
    }

    /// @notice Full lifecycle: register → select → recordSuccess → reputation increases
    function test_prover_full_lifecycle() public {
        // 1. Register with 200 tokens
        vm.prank(prover1);
        proverRegistry.register(200e18, "https://prover1.example.com");

        ProverRegistry.Prover memory p = proverRegistry.getProver(prover1);
        assertEq(p.stake, 200e18);
        assertEq(p.reputation, 5000); // starts at 50%
        assertTrue(p.active);

        // 2. Prover selection via commit-reveal
        bytes32 secret = keccak256("my-secret");
        bytes32 commitment = keccak256(abi.encodePacked(secret));
        uint256 selReqId = proverRegistry.requestProverSelection(commitment);

        // Must be in a later block
        vm.roll(block.number + 1);
        address selected = proverRegistry.fulfillProverSelection(selReqId, secret);
        assertEq(selected, prover1, "only registered prover should be selected");

        // 3. Record success (need to be slasher/owner)
        proverRegistry.setSlasher(address(this), true);
        proverRegistry.recordSuccess(prover1, 0.1 ether);

        p = proverRegistry.getProver(prover1);
        assertEq(p.proofsSubmitted, 1);
        assertEq(p.reputation, 5050); // 5000 + 50 increment
    }

    /// @notice Slash reduces stake and auto-deactivates if below minimum
    function test_slash_and_deactivation() public {
        vm.prank(prover1);
        proverRegistry.register(MIN_STAKE, "https://prover1.example.com");

        // Authorize slasher
        proverRegistry.setSlasher(address(this), true);

        // Slash at 5% of 100e18 = 5e18
        proverRegistry.slash(prover1, "missed deadline");

        ProverRegistry.Prover memory p = proverRegistry.getProver(prover1);
        assertEq(p.stake, 95e18, "stake should be reduced by 5%");
        assertEq(p.reputation, 4750); // 5000 * 95 / 100
        assertFalse(p.active, "should be deactivated (stake < minStake)");
    }
}

// ========================================================================
// TEST 4: DAG Batch Verification (Multi-Tx)
// ========================================================================

contract IntegrationE2E_BatchTest is IntegrationFixtureBase {
    function setUp() public {
        _deployAndRegisterDAGCircuit();
    }

    /// @notice Full batch flow: start → continue ×N → finalize until done
    function test_batch_full_flow() public {
        // Start
        bytes32 sessionId = remainderVerifier.startDAGBatchVerify(proofHex, circuitHash, publicInputsHex, gensHex);
        assertTrue(sessionId != bytes32(0), "sessionId should be non-zero");

        (bytes32 storedHash, uint256 nextBatch, uint256 totalBatches, bool finalized,,) =
            remainderVerifier.getDAGBatchSession(sessionId);
        assertEq(storedHash, circuitHash);
        assertEq(nextBatch, 0, "start is setup-only, no compute");
        assertEq(totalBatches, 11, "88 layers / 8 per batch = 11");
        assertFalse(finalized);

        // Continue all compute batches
        for (uint256 i = 0; i < totalBatches; i++) {
            remainderVerifier.continueDAGBatchVerify(sessionId, proofHex, publicInputsHex, gensHex);
            (, nextBatch,,,,) = remainderVerifier.getDAGBatchSession(sessionId);
            assertEq(nextBatch, i + 1);
        }

        // Finalize (multi-call)
        uint256 finalizeCalls = 0;
        while (true) {
            finalized = remainderVerifier.finalizeDAGBatchVerify(sessionId, proofHex, publicInputsHex, gensHex);
            finalizeCalls++;
            if (finalized) break;
        }

        (,,, finalized,,) = remainderVerifier.getDAGBatchSession(sessionId);
        assertTrue(finalized, "should be fully finalized");
        assertGe(finalizeCalls, 1);

        // Cleanup
        remainderVerifier.cleanupDAGBatchSession(sessionId);
        (storedHash,,,,,) = remainderVerifier.getDAGBatchSession(sessionId);
        assertEq(storedHash, bytes32(0), "session cleared after cleanup");
    }

    /// @notice Every batch step (start, continue, finalize) must fit in a 30M gas block
    function test_all_batch_steps_under_30M_gas() public {
        uint256 gasBefore = gasleft();
        bytes32 sessionId = remainderVerifier.startDAGBatchVerify(proofHex, circuitHash, publicInputsHex, gensHex);
        uint256 startGas = gasBefore - gasleft();
        assertLt(startGas, 30_000_000, "start < 30M");

        (,, uint256 totalBatches,,,) = remainderVerifier.getDAGBatchSession(sessionId);

        for (uint256 i = 0; i < totalBatches; i++) {
            gasBefore = gasleft();
            remainderVerifier.continueDAGBatchVerify(sessionId, proofHex, publicInputsHex, gensHex);
            uint256 batchGas = gasBefore - gasleft();
            assertLt(batchGas, 30_000_000, "continue < 30M");
        }

        while (true) {
            gasBefore = gasleft();
            bool done = remainderVerifier.finalizeDAGBatchVerify(sessionId, proofHex, publicInputsHex, gensHex);
            uint256 finalizeGas = gasBefore - gasleft();
            assertLt(finalizeGas, 30_000_000, "finalize < 30M");
            if (done) break;
        }
    }
}
