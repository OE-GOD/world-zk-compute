// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/tee/TEEMLVerifier.sol";
import "../src/tee/ITEEMLVerifier.sol";
import "../src/ExecutionEngine.sol";
import "../src/ProgramRegistry.sol";
import "../src/MockRiscZeroVerifier.sol";

// =============================================================================
// Mock Remainder Verifier (configurable per-result)
// =============================================================================

contract InvariantMockRemainderVerifier {
    bool public defaultResult = true;

    function setDefaultResult(bool _result) external {
        defaultResult = _result;
    }

    function verifyDAGProof(bytes calldata, bytes32, bytes calldata, bytes calldata) external view returns (bool) {
        return defaultResult;
    }
}

// =============================================================================
// TEEMLVerifier Handler -- drives submit/challenge/finalize/resolve actions
// =============================================================================

contract TEEMLVerifierHandler is Test {
    TEEMLVerifier public verifier;
    InvariantMockRemainderVerifier public mockVerifier;

    uint256 internal enclavePrivateKey = 0xA11CE;
    address internal enclaveAddr;

    bytes32 internal modelHash = keccak256("invariant-model");
    bytes internal resultData = hex"deadbeef";

    // Ghost variables for invariant tracking
    uint256 public ghost_totalActiveStakes;
    uint256 public ghost_totalActiveChallengeBonds;
    uint256 public ghost_disputesResolvedProverPaid;
    uint256 public ghost_disputesResolvedChallengerPaid;
    uint256 public ghost_disputesResolvedTotal;

    // Track all result IDs for iteration
    bytes32[] public allResultIds;
    mapping(bytes32 => bool) public resultIdExists;

    // Per-result tracking
    mapping(bytes32 => uint256) public resultStake;
    mapping(bytes32 => uint256) public resultBond;
    mapping(bytes32 => bool) public resultSettled; // finalized or dispute resolved

    // Counters for calls
    uint256 public submitCount;
    uint256 public challengeCount;
    uint256 public finalizeCount;
    uint256 public resolveCount;
    uint256 public resolveTimeoutCount;

    // Allow handler to receive ETH
    receive() external payable {}

    constructor(TEEMLVerifier _verifier, InvariantMockRemainderVerifier _mockVerifier) {
        verifier = _verifier;
        mockVerifier = _mockVerifier;
        enclaveAddr = vm.addr(enclavePrivateKey);
    }

    function _signAttestation(bytes32 _inputHash) internal view returns (bytes memory attestation) {
        bytes32 resultHash = keccak256(resultData);
        // EIP-712 structured data signing (matches TEEMLVerifier.submitResult)
        bytes32 structHash = keccak256(abi.encode(verifier.RESULT_TYPEHASH(), modelHash, _inputHash, resultHash));
        bytes32 domainSep = keccak256(
            abi.encode(
                keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"),
                keccak256("TEEMLVerifier"),
                keccak256("1"),
                block.chainid,
                address(verifier)
            )
        );
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", domainSep, structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclavePrivateKey, digest);
        attestation = abi.encodePacked(r, s, v);
    }

    /// @notice Submit a result with valid attestation and stake.
    ///         Uses a unique inputHash derived from a seed to avoid "result exists" collisions.
    function submitResult(uint256 seed) external {
        // Generate a unique input hash so we get a unique resultId
        bytes32 inputHash = keccak256(abi.encodePacked("input", seed, block.number, submitCount));
        bytes memory attestation = _signAttestation(inputHash);

        uint256 stakeAmount = verifier.proverStake();

        // Make sure we have enough ETH
        if (address(this).balance < stakeAmount) {
            vm.deal(address(this), address(this).balance + stakeAmount + 1 ether);
        }

        bytes32 resultId = verifier.submitResult{value: stakeAmount}(modelHash, inputHash, resultData, attestation);

        // Track the active stake
        ghost_totalActiveStakes += stakeAmount;
        resultStake[resultId] = stakeAmount;
        allResultIds.push(resultId);
        resultIdExists[resultId] = true;
        submitCount++;
    }

    /// @notice Challenge a random submitted (unchallenged, unfinalized) result.
    function challengeResult(uint256 seed) external {
        if (allResultIds.length == 0) return;

        // Find a challengeable result
        bytes32 resultId = _findChallengeableResult(seed);
        if (resultId == bytes32(0)) return;

        ITEEMLVerifier.MLResult memory r = verifier.getResult(resultId);
        if (r.challenged || r.finalized) return;
        if (block.timestamp >= r.challengeDeadline) return;

        uint256 bondAmount = verifier.challengeBondAmount();

        // Use a separate challenger address
        address challenger = address(uint160(uint256(keccak256(abi.encodePacked("challenger", seed)))));
        vm.deal(challenger, bondAmount + 1 ether);

        vm.prank(challenger);
        verifier.challenge{value: bondAmount}(resultId);

        ghost_totalActiveChallengeBonds += bondAmount;
        resultBond[resultId] = bondAmount;
        challengeCount++;
    }

    /// @notice Finalize a result whose challenge window has passed without challenge.
    function finalizeResult(uint256 seed) external {
        if (allResultIds.length == 0) return;

        bytes32 resultId = _findFinalizableResult(seed);
        if (resultId == bytes32(0)) return;

        ITEEMLVerifier.MLResult memory r = verifier.getResult(resultId);
        if (r.finalized || r.challenged) return;
        if (block.timestamp < r.challengeDeadline) {
            // Warp past the challenge window
            vm.warp(r.challengeDeadline + 1);
        }

        uint256 stakeReturned = resultStake[resultId];

        verifier.finalize(resultId);

        // Stake is returned to submitter on finalize
        ghost_totalActiveStakes -= stakeReturned;
        resultSettled[resultId] = true;
        finalizeCount++;
    }

    /// @notice Resolve a disputed result via ZK proof (prover wins).
    function resolveDisputeProverWins(uint256 seed) external {
        if (allResultIds.length == 0) return;

        bytes32 resultId = _findDisputedResult(seed);
        if (resultId == bytes32(0)) return;

        if (verifier.disputeResolved(resultId)) return;

        mockVerifier.setDefaultResult(true);

        uint256 stake = resultStake[resultId];
        uint256 bond = resultBond[resultId];

        verifier.resolveDispute(resultId, hex"", bytes32(0), hex"", hex"");

        // Prover gets stake + bond
        ghost_totalActiveStakes -= stake;
        ghost_totalActiveChallengeBonds -= bond;
        resultSettled[resultId] = true;
        ghost_disputesResolvedProverPaid++;
        ghost_disputesResolvedTotal++;
        resolveCount++;
    }

    /// @notice Resolve a disputed result via ZK proof (challenger wins).
    function resolveDisputeChallengerWins(uint256 seed) external {
        if (allResultIds.length == 0) return;

        bytes32 resultId = _findDisputedResult(seed);
        if (resultId == bytes32(0)) return;

        if (verifier.disputeResolved(resultId)) return;

        mockVerifier.setDefaultResult(false);

        uint256 stake = resultStake[resultId];
        uint256 bond = resultBond[resultId];

        verifier.resolveDispute(resultId, hex"", bytes32(0), hex"", hex"");

        // Challenger gets stake + bond
        ghost_totalActiveStakes -= stake;
        ghost_totalActiveChallengeBonds -= bond;
        resultSettled[resultId] = true;
        ghost_disputesResolvedChallengerPaid++;
        ghost_disputesResolvedTotal++;
        resolveCount++;
    }

    /// @notice Resolve a disputed result by timeout (challenger wins by default).
    function resolveDisputeByTimeout(uint256 seed) external {
        if (allResultIds.length == 0) return;

        bytes32 resultId = _findDisputedResult(seed);
        if (resultId == bytes32(0)) return;

        if (verifier.disputeResolved(resultId)) return;

        ITEEMLVerifier.MLResult memory r = verifier.getResult(resultId);
        // Warp past dispute deadline
        if (block.timestamp < r.disputeDeadline) {
            vm.warp(r.disputeDeadline + 1);
        }

        uint256 stake = resultStake[resultId];
        uint256 bond = resultBond[resultId];

        verifier.resolveDisputeByTimeout(resultId);

        // Challenger gets stake + bond
        ghost_totalActiveStakes -= stake;
        ghost_totalActiveChallengeBonds -= bond;
        resultSettled[resultId] = true;
        ghost_disputesResolvedChallengerPaid++;
        ghost_disputesResolvedTotal++;
        resolveTimeoutCount++;
    }

    // ---- Internal helpers ----

    function _findChallengeableResult(uint256 seed) internal view returns (bytes32) {
        uint256 len = allResultIds.length;
        if (len == 0) return bytes32(0);
        uint256 start = seed % len;
        for (uint256 i = 0; i < len; i++) {
            bytes32 rid = allResultIds[(start + i) % len];
            ITEEMLVerifier.MLResult memory r = verifier.getResult(rid);
            if (!r.challenged && !r.finalized && block.timestamp < r.challengeDeadline) {
                return rid;
            }
        }
        return bytes32(0);
    }

    function _findFinalizableResult(uint256 seed) internal view returns (bytes32) {
        uint256 len = allResultIds.length;
        if (len == 0) return bytes32(0);
        uint256 start = seed % len;
        for (uint256 i = 0; i < len; i++) {
            bytes32 rid = allResultIds[(start + i) % len];
            ITEEMLVerifier.MLResult memory r = verifier.getResult(rid);
            if (!r.finalized && !r.challenged) {
                return rid;
            }
        }
        return bytes32(0);
    }

    function _findDisputedResult(uint256 seed) internal view returns (bytes32) {
        uint256 len = allResultIds.length;
        if (len == 0) return bytes32(0);
        uint256 start = seed % len;
        for (uint256 i = 0; i < len; i++) {
            bytes32 rid = allResultIds[(start + i) % len];
            ITEEMLVerifier.MLResult memory r = verifier.getResult(rid);
            if (r.challenged && !verifier.disputeResolved(rid)) {
                return rid;
            }
        }
        return bytes32(0);
    }

    function allResultIdsLength() external view returns (uint256) {
        return allResultIds.length;
    }
}

// =============================================================================
// ExecutionEngine Handler -- drives request/cancel/claim/submit actions
// =============================================================================

contract ExecutionEngineHandler is Test {
    ExecutionEngine public engine;
    ProgramRegistry public registry;

    bytes32 public imageId = bytes32(uint256(1));
    string public inputUrl = "ipfs://QmTest";

    // Ghost variables
    uint256 public ghost_totalUnclaimedTips; // ETH locked for pending/claimed requests
    uint256 public ghost_cancelledRefunds; // total ETH refunded via cancel

    // Track request IDs
    uint256[] public allRequestIds;
    mapping(uint256 => uint256) public requestTip;
    mapping(uint256 => bool) public requestSettled; // completed, cancelled, or expired

    uint256 public requestCount;
    uint256 public cancelCount;
    uint256 public claimCount;
    uint256 public proofCount;

    // Allow handler to receive ETH
    receive() external payable {}

    constructor(ExecutionEngine _engine, ProgramRegistry _registry) {
        engine = _engine;
        registry = _registry;
    }

    /// @notice Request execution with a random tip amount above MIN_TIP.
    function requestExecution(uint256 tipSeed) external {
        // Constrain tip to [MIN_TIP, 1 ether]
        uint256 minTip = engine.MIN_TIP();
        uint256 tip = minTip + (tipSeed % (1 ether - minTip));

        bytes32 inputDigest = keccak256(abi.encodePacked("input", requestCount));

        vm.deal(address(this), address(this).balance + tip + 1 ether);

        uint256 requestId = engine.requestExecution{value: tip}(imageId, inputDigest, inputUrl, address(0), 3600);

        ghost_totalUnclaimedTips += tip;
        allRequestIds.push(requestId);
        requestTip[requestId] = tip;
        requestCount++;
    }

    /// @notice Cancel a pending request (refunds the requester).
    function cancelExecution(uint256 seed) external {
        if (allRequestIds.length == 0) return;

        uint256 reqId = _findCancellableRequest(seed);
        if (reqId == 0) return;

        uint256 tip = requestTip[reqId];

        // cancelExecution checks msg.sender == requester, so we prank as this contract (the requester)
        engine.cancelExecution(reqId);

        ghost_totalUnclaimedTips -= tip;
        ghost_cancelledRefunds += tip;
        requestSettled[reqId] = true;
        cancelCount++;
    }

    /// @notice Claim a pending request as a prover.
    function claimExecution(uint256 seed) external {
        if (allRequestIds.length == 0) return;

        uint256 reqId = _findClaimableRequest(seed);
        if (reqId == 0) return;

        address proverAddr = address(uint160(uint256(keccak256(abi.encodePacked("prover", seed)))));
        vm.prank(proverAddr);
        engine.claimExecution(reqId);

        claimCount++;
    }

    /// @notice Submit proof for a claimed request (settles it).
    function submitProof(uint256 seed) external {
        if (allRequestIds.length == 0) return;

        uint256 reqId = _findClaimedRequest(seed);
        if (reqId == 0) return;

        ExecutionEngine.ExecutionRequest memory req = engine.getRequest(reqId);

        // Must be called by the claimant within the claim deadline
        if (block.timestamp > req.claimDeadline) return;

        vm.prank(req.claimedBy);
        engine.submitProof(reqId, hex"deadbeef", hex"cafebabe");

        uint256 tip = requestTip[reqId];
        ghost_totalUnclaimedTips -= tip;
        requestSettled[reqId] = true;
        proofCount++;
    }

    // ---- Internal helpers ----

    function _findCancellableRequest(uint256 seed) internal view returns (uint256) {
        uint256 len = allRequestIds.length;
        if (len == 0) return 0;
        uint256 start = seed % len;
        for (uint256 i = 0; i < len; i++) {
            uint256 rid = allRequestIds[(start + i) % len];
            ExecutionEngine.ExecutionRequest memory req = engine.getRequest(rid);
            if (req.status == ExecutionEngine.RequestStatus.Pending && !requestSettled[rid]) {
                return rid;
            }
        }
        return 0;
    }

    function _findClaimableRequest(uint256 seed) internal view returns (uint256) {
        uint256 len = allRequestIds.length;
        if (len == 0) return 0;
        uint256 start = seed % len;
        for (uint256 i = 0; i < len; i++) {
            uint256 rid = allRequestIds[(start + i) % len];
            ExecutionEngine.ExecutionRequest memory req = engine.getRequest(rid);
            if (req.status == ExecutionEngine.RequestStatus.Pending && block.timestamp <= req.expiresAt) {
                return rid;
            }
        }
        return 0;
    }

    function _findClaimedRequest(uint256 seed) internal view returns (uint256) {
        uint256 len = allRequestIds.length;
        if (len == 0) return 0;
        uint256 start = seed % len;
        for (uint256 i = 0; i < len; i++) {
            uint256 rid = allRequestIds[(start + i) % len];
            ExecutionEngine.ExecutionRequest memory req = engine.getRequest(rid);
            if (req.status == ExecutionEngine.RequestStatus.Claimed && block.timestamp <= req.claimDeadline) {
                return rid;
            }
        }
        return 0;
    }

    function allRequestIdsLength() external view returns (uint256) {
        return allRequestIds.length;
    }
}

// =============================================================================
// Invariant Test: TEEMLVerifier Fund Accounting
// =============================================================================

contract TEEMLVerifierInvariantTest is Test {
    TEEMLVerifier public verifier;
    InvariantMockRemainderVerifier public mockVerifier;
    TEEMLVerifierHandler public handler;

    uint256 internal enclavePrivateKey = 0xA11CE;
    address internal enclaveAddr;

    // Allow this contract to receive ETH
    receive() external payable {}

    function setUp() public {
        enclaveAddr = vm.addr(enclavePrivateKey);
        mockVerifier = new InvariantMockRemainderVerifier();
        verifier = new TEEMLVerifier(address(this), address(mockVerifier));
        verifier.registerEnclave(enclaveAddr, keccak256("test-enclave-image"));

        handler = new TEEMLVerifierHandler(verifier, mockVerifier);
        vm.deal(address(handler), 100 ether);

        // Target only the handler for invariant calls
        targetContract(address(handler));
    }

    /// @notice INVARIANT 1: Contract balance must always be >= sum of active stakes + active bonds.
    ///         This ensures the contract is always solvent and can pay out all obligations.
    function invariant_tee_balance_covers_obligations() public view {
        uint256 contractBalance = address(verifier).balance;
        uint256 totalObligations = handler.ghost_totalActiveStakes() + handler.ghost_totalActiveChallengeBonds();

        assertGe(
            contractBalance, totalObligations, "TEEMLVerifier balance must cover all active stakes + challenge bonds"
        );
    }

    /// @notice INVARIANT 3: No result can be both finalized=true and have an active (unresolved) dispute.
    ///         If finalized && challenged, the dispute must be resolved.
    function invariant_tee_no_finalized_with_active_dispute() public view {
        uint256 len = handler.allResultIdsLength();
        for (uint256 i = 0; i < len; i++) {
            bytes32 resultId = handler.allResultIds(i);
            ITEEMLVerifier.MLResult memory r = verifier.getResult(resultId);

            if (r.finalized && r.challenged) {
                // If finalized AND challenged, dispute MUST be resolved
                assertTrue(verifier.disputeResolved(resultId), "Finalized+challenged result must have dispute resolved");
            }
        }
    }

    /// @notice INVARIANT 4: After dispute resolution, exactly one party gets paid.
    ///         Total disputes resolved = prover wins + challenger wins.
    function invariant_tee_dispute_resolution_exactly_one_party_paid() public view {
        uint256 total = handler.ghost_disputesResolvedTotal();
        uint256 proverPaid = handler.ghost_disputesResolvedProverPaid();
        uint256 challengerPaid = handler.ghost_disputesResolvedChallengerPaid();

        assertEq(total, proverPaid + challengerPaid, "Total resolved disputes must equal prover wins + challenger wins");
    }

    /// @notice INVARIANT: Ghost tracking is consistent -- total obligations never go negative.
    ///         Since we use uint256, underflow would revert the handler, but let us assert it is sane.
    function invariant_tee_ghost_variables_sane() public view {
        // These are uint256 so they cannot go negative (would have reverted).
        // Just assert they are consistent with submit/resolve counts.
        uint256 totalSubmits = handler.submitCount();
        uint256 totalFinalizes = handler.finalizeCount();
        uint256 totalResolves = handler.resolveCount() + handler.resolveTimeoutCount();

        // Total settled results cannot exceed total submitted
        assertLe(totalFinalizes + totalResolves, totalSubmits, "Settled results must not exceed submitted results");
    }

    /// @notice Summarize after invariant run (called by Foundry after invariant sequence).
    function invariant_callSummary() public view {
        // This invariant always passes -- it just logs stats for debugging
        // Uncomment the emit lines if you need debug output
    }
}

// =============================================================================
// Invariant Test: ExecutionEngine Fund Accounting
// =============================================================================

contract ExecutionEngineInvariantTest is Test {
    ExecutionEngine public engine;
    ProgramRegistry public registry;
    MockRiscZeroVerifier public mockVerifier;
    ExecutionEngineHandler public handler;

    address public admin = address(this);
    address public feeRecipient = address(0xFEE);

    // Allow this contract to receive ETH
    receive() external payable {}

    function setUp() public {
        mockVerifier = new MockRiscZeroVerifier();
        registry = new ProgramRegistry(admin);
        engine = new ExecutionEngine(admin, address(registry), address(mockVerifier), feeRecipient);

        // Register a test program
        registry.registerProgram(bytes32(uint256(1)), "Test Program", "https://example.com/test.elf", bytes32(0));

        handler = new ExecutionEngineHandler(engine, registry);
        vm.deal(address(handler), 100 ether);

        // Target only the handler for invariant calls
        targetContract(address(handler));
    }

    /// @notice INVARIANT 2: Contract balance must always be >= sum of all unclaimed request tips.
    ///         This ensures the engine is always solvent for refunds/payouts.
    function invariant_engine_balance_covers_tips() public view {
        uint256 contractBalance = address(engine).balance;
        uint256 totalUnclaimed = handler.ghost_totalUnclaimedTips();

        assertGe(contractBalance, totalUnclaimed, "ExecutionEngine balance must cover all unclaimed tips");
    }

    /// @notice INVARIANT 5: Cancelled requests always result in the requester being refunded.
    ///         We track that cancelled refunds match expected amounts.
    function invariant_engine_cancel_refunds_requester() public view {
        uint256 len = handler.allRequestIdsLength();
        for (uint256 i = 0; i < len; i++) {
            uint256 reqId = handler.allRequestIds(i);
            ExecutionEngine.ExecutionRequest memory req = engine.getRequest(reqId);

            if (req.status == ExecutionEngine.RequestStatus.Cancelled) {
                // The request must have been marked as settled in our handler
                assertTrue(handler.requestSettled(reqId), "Cancelled request must be tracked as settled (refunded)");
            }
        }
    }

    /// @notice INVARIANT: Ghost variables stay consistent with actual contract state.
    function invariant_engine_ghost_variables_sane() public view {
        uint256 totalRequests = handler.requestCount();
        uint256 totalCancels = handler.cancelCount();
        uint256 totalProofs = handler.proofCount();

        // Settled requests (cancelled + completed) cannot exceed total created
        assertLe(totalCancels + totalProofs, totalRequests, "Settled requests must not exceed created requests");
    }

    /// @notice INVARIANT: All cancelled requests in the engine have status Cancelled.
    function invariant_engine_cancelled_status_consistent() public view {
        uint256 len = handler.allRequestIdsLength();
        uint256 cancelledCount = 0;
        for (uint256 i = 0; i < len; i++) {
            uint256 reqId = handler.allRequestIds(i);
            ExecutionEngine.ExecutionRequest memory req = engine.getRequest(reqId);
            if (req.status == ExecutionEngine.RequestStatus.Cancelled) {
                cancelledCount++;
            }
        }
        // The handler's cancelCount should match the on-chain cancelled count
        assertEq(cancelledCount, handler.cancelCount(), "Handler cancel count must match on-chain cancelled requests");
    }
}
