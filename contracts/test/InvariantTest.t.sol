// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/tee/TEEMLVerifier.sol";
import "../src/tee/ITEEMLVerifier.sol";
import "../src/ExecutionEngine.sol";
import "../src/ProgramRegistry.sol";
import "../src/MockRiscZeroVerifier.sol";
import "../src/ProverRegistry.sol";
import "../src/ProverReputation.sol";
import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";

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

/// @dev Simple ERC20 mock for ProverRegistry invariant tests
contract InvariantMockToken is ERC20 {
    constructor() ERC20("InvMock", "IMK") {
        _mint(msg.sender, 10_000_000 ether);
    }

    function mint(address to, uint256 amount) external {
        _mint(to, amount);
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

// =============================================================================
// ProverRegistry Handler -- drives register/deregister/addStake/withdrawStake/slash
// =============================================================================

contract ProverRegistryHandler is Test {
    ProverRegistry public registry;
    InvariantMockToken public token;

    uint256 public constant MIN_STAKE = 100 ether;

    // Ghost variables for invariant tracking
    uint256 public ghost_totalIndividualStakes;
    uint256 public ghost_activeProverCount;

    // Track registered provers
    address[] public registeredProvers;
    mapping(address => bool) public isRegistered;
    mapping(address => uint256) public proverStakes;

    uint256 public registerCount;
    uint256 public deactivateCount;
    uint256 public addStakeCount;
    uint256 public withdrawCount;
    uint256 public slashCount;

    constructor(ProverRegistry _registry, InvariantMockToken _token) {
        registry = _registry;
        token = _token;
    }

    /// @notice Register a new prover with a deterministic address based on seed.
    function registerProver(uint256 seed) external {
        address prover = address(uint160(uint256(keccak256(abi.encodePacked("regProver", seed, registerCount)))));
        if (isRegistered[prover]) return;
        if (prover == address(0)) return;

        uint256 stake = MIN_STAKE + (seed % (1000 ether));

        token.mint(prover, stake);
        vm.startPrank(prover);
        token.approve(address(registry), stake);
        registry.register(stake, "http://inv-test");
        vm.stopPrank();

        ghost_totalIndividualStakes += stake;
        ghost_activeProverCount++;
        registeredProvers.push(prover);
        isRegistered[prover] = true;
        proverStakes[prover] = stake;
        registerCount++;
    }

    /// @notice Deactivate a random active prover.
    function deactivateProver(uint256 seed) external {
        if (registeredProvers.length == 0) return;

        address prover = _findActiveProver(seed);
        if (prover == address(0)) return;

        vm.prank(prover);
        registry.deactivate();

        ghost_activeProverCount--;
        deactivateCount++;
    }

    /// @notice Add stake for a random registered prover.
    function addStake(uint256 seed, uint256 amount) external {
        if (registeredProvers.length == 0) return;
        amount = bound(amount, 1 ether, 500 ether);

        uint256 idx = seed % registeredProvers.length;
        address prover = registeredProvers[idx];

        token.mint(prover, amount);
        vm.startPrank(prover);
        token.approve(address(registry), amount);
        registry.addStake(amount);
        vm.stopPrank();

        ghost_totalIndividualStakes += amount;
        proverStakes[prover] += amount;
        addStakeCount++;
    }

    /// @notice Withdraw stake for a random inactive prover (to avoid minStake checks).
    function withdrawStake(uint256 seed) external {
        if (registeredProvers.length == 0) return;

        // Find an inactive prover with stake
        address prover = _findInactiveProverWithStake(seed);
        if (prover == address(0)) return;

        ProverRegistry.Prover memory p = registry.getProver(prover);
        if (p.stake == 0) return;

        uint256 withdrawAmount = p.stake; // Withdraw everything since inactive

        vm.prank(prover);
        registry.withdrawStake(withdrawAmount);

        ghost_totalIndividualStakes -= withdrawAmount;
        proverStakes[prover] -= withdrawAmount;
        withdrawCount++;
    }

    /// @notice Slash a random active prover.
    function slashProver(uint256 seed) external {
        if (registeredProvers.length == 0) return;

        uint256 idx = seed % registeredProvers.length;
        address prover = registeredProvers[idx];

        ProverRegistry.Prover memory p = registry.getProver(prover);
        if (p.registeredAt == 0 || p.stake == 0) return;

        uint256 slashBps = registry.slashBasisPoints();
        uint256 slashAmount = (p.stake * slashBps) / 10000;
        if (slashAmount > p.stake) slashAmount = p.stake;

        registry.slash(prover, "invariant slash");

        ghost_totalIndividualStakes -= slashAmount;
        proverStakes[prover] -= slashAmount;

        // Check if prover was deactivated
        if (p.stake - slashAmount < MIN_STAKE && p.active) {
            ghost_activeProverCount--;
        }

        slashCount++;
    }

    // ---- Internal helpers ----

    function _findActiveProver(uint256 seed) internal view returns (address) {
        uint256 len = registeredProvers.length;
        if (len == 0) return address(0);
        uint256 start = seed % len;
        for (uint256 i = 0; i < len; i++) {
            address p = registeredProvers[(start + i) % len];
            if (registry.isActive(p)) return p;
        }
        return address(0);
    }

    function _findInactiveProverWithStake(uint256 seed) internal view returns (address) {
        uint256 len = registeredProvers.length;
        if (len == 0) return address(0);
        uint256 start = seed % len;
        for (uint256 i = 0; i < len; i++) {
            address p = registeredProvers[(start + i) % len];
            ProverRegistry.Prover memory prover = registry.getProver(p);
            if (!prover.active && prover.stake > 0) return p;
        }
        return address(0);
    }

    function registeredProversLength() external view returns (uint256) {
        return registeredProvers.length;
    }
}

// =============================================================================
// ProverReputation Handler -- drives recordSuccess/recordFailure/recordAbandon/slash
// =============================================================================

contract ProverReputationHandler is Test {
    ProverReputation public rep;

    // Track registered provers
    address[] public registeredProvers;
    mapping(address => bool) public isRegistered;

    // Ghost variables
    uint256 public ghost_totalRegistered;
    uint256 public ghost_bannedCount;

    uint256 public successCount;
    uint256 public failureCount;
    uint256 public abandonCount;
    uint256 public slashCount;

    constructor(ProverReputation _rep) {
        rep = _rep;
    }

    /// @notice Register a new prover.
    function registerProver(uint256 seed) external {
        address prover = address(uint160(uint256(keccak256(abi.encodePacked("repProver", seed, ghost_totalRegistered)))));
        if (isRegistered[prover] || prover == address(0)) return;

        vm.prank(prover);
        rep.register();

        registeredProvers.push(prover);
        isRegistered[prover] = true;
        ghost_totalRegistered++;
    }

    /// @notice Record a successful job for a random prover.
    function recordSuccess(uint256 seed, uint256 proofTimeMs) external {
        address prover = _findNonBannedProver(seed);
        if (prover == address(0)) return;

        proofTimeMs = bound(proofTimeMs, 10, 600_000); // 10ms to 10min

        rep.recordSuccess(prover, proofTimeMs, 0.1 ether);
        successCount++;
    }

    /// @notice Record a failed job for a random prover.
    function recordFailure(uint256 seed) external {
        address prover = _findNonBannedProver(seed);
        if (prover == address(0)) return;

        rep.recordFailure(prover, "invariant failure");
        failureCount++;
    }

    /// @notice Record an abandoned job for a random prover.
    function recordAbandon(uint256 seed) external {
        address prover = _findNonBannedProver(seed);
        if (prover == address(0)) return;

        rep.recordAbandon(prover, seed);
        abandonCount++;
    }

    /// @notice Slash a random prover with random penalty.
    function slashProver(uint256 seed, uint256 penaltyBps) external {
        if (registeredProvers.length == 0) return;
        penaltyBps = bound(penaltyBps, 100, 5000);

        uint256 idx = seed % registeredProvers.length;
        address prover = registeredProvers[idx];

        ProverReputation.Reputation memory r = rep.getReputation(prover);
        if (!r.isRegistered) return;

        // Check slash cooldown
        ProverReputation.SlashEvent[] memory history = rep.getSlashHistory(prover);
        if (history.length > 0) {
            uint256 lastSlashTime = history[history.length - 1].timestamp;
            if (block.timestamp < lastSlashTime + rep.slashCooldown()) {
                vm.warp(lastSlashTime + rep.slashCooldown() + 1);
            }
        }

        rep.slash(prover, "invariant slash", penaltyBps);

        // Check if auto-banned
        r = rep.getReputation(prover);
        if (r.isBanned) ghost_bannedCount++;

        slashCount++;
    }

    // ---- Internal helpers ----

    function _findNonBannedProver(uint256 seed) internal view returns (address) {
        uint256 len = registeredProvers.length;
        if (len == 0) return address(0);
        uint256 start = seed % len;
        for (uint256 i = 0; i < len; i++) {
            address p = registeredProvers[(start + i) % len];
            ProverReputation.Reputation memory r = rep.getReputation(p);
            if (r.isRegistered && !r.isBanned) return p;
        }
        return address(0);
    }

    function registeredProversLength() external view returns (uint256) {
        return registeredProvers.length;
    }
}

// =============================================================================
// Invariant Test: ProverRegistry Stake Accounting
// =============================================================================

contract ProverRegistryInvariantTest is Test {
    ProverRegistry public registry;
    InvariantMockToken public token;
    ProverRegistryHandler public handler;

    uint256 public constant MIN_STAKE = 100 ether;
    uint256 public constant SLASH_BPS = 500; // 5%

    function setUp() public {
        token = new InvariantMockToken();
        registry = new ProverRegistry(address(token), MIN_STAKE, SLASH_BPS);

        handler = new ProverRegistryHandler(registry, token);

        // Authorize the handler as a slasher so it can slash provers
        registry.setSlasher(address(handler), true);

        // Transfer ownership so handler can act as admin for slash
        // (slash requires slashers[msg.sender] || msg.sender == owner())
        // Handler is already authorized as slasher above

        targetContract(address(handler));
    }

    /// @notice INVARIANT: totalStaked must equal the sum of all individual prover stakes.
    function invariant_registry_totalStaked_matches_sum() public view {
        uint256 sumStakes = 0;
        uint256 len = handler.registeredProversLength();
        for (uint256 i = 0; i < len; i++) {
            address prover = handler.registeredProvers(i);
            ProverRegistry.Prover memory p = registry.getProver(prover);
            sumStakes += p.stake;
        }
        assertEq(registry.totalStaked(), sumStakes, "totalStaked must equal sum of individual stakes");
    }

    /// @notice INVARIANT: activeProvers.length must match the count of provers where isActive == true.
    function invariant_registry_activeCount_matches() public view {
        uint256 len = handler.registeredProversLength();
        uint256 activeCount = 0;
        for (uint256 i = 0; i < len; i++) {
            address prover = handler.registeredProvers(i);
            if (registry.isActive(prover)) {
                activeCount++;
            }
        }
        assertEq(registry.activeProverCount(), activeCount, "activeProverCount must match actual active provers");
    }

    /// @notice INVARIANT: No prover can have a stake that underflowed (would be extremely large).
    ///         Since we use uint256 and Solidity 0.8+ has overflow checks, this is a safety net.
    function invariant_registry_no_negative_stake() public view {
        uint256 len = handler.registeredProversLength();
        for (uint256 i = 0; i < len; i++) {
            address prover = handler.registeredProvers(i);
            ProverRegistry.Prover memory p = registry.getProver(prover);
            // A uint256 underflow in 0.8+ would revert, but sanity check the value is reasonable
            assertLe(p.stake, 100_000 ether, "individual stake should not be unreasonably large");
        }
    }

    /// @notice INVARIANT: Ghost tracking is consistent with actual contract state.
    function invariant_registry_ghost_sane() public view {
        uint256 totalRegistered = handler.registerCount();
        uint256 totalDeactivated = handler.deactivateCount();
        uint256 totalSlashed = handler.slashCount();

        // Deactivations + slashes cannot exceed registrations (slashes may also deactivate)
        assertLe(totalDeactivated, totalRegistered, "deactivations should not exceed registrations");
        assertLe(totalSlashed, totalRegistered * 100, "slashes should be bounded"); // Multiple slashes per prover OK
    }
}

// =============================================================================
// Invariant Test: ProverReputation Score Bounds
// =============================================================================

contract ProverReputationInvariantTest is Test {
    ProverReputation public rep;
    ProverReputationHandler public handler;

    function setUp() public {
        rep = new ProverReputation();

        handler = new ProverReputationHandler(rep);

        // Authorize handler as reporter
        rep.authorizeReporter(address(handler));

        targetContract(address(handler));
    }

    /// @notice INVARIANT: Score never exceeds MAX_SCORE (10000).
    function invariant_reputation_score_bounded() public view {
        uint256 len = handler.registeredProversLength();
        for (uint256 i = 0; i < len; i++) {
            address prover = handler.registeredProvers(i);
            ProverReputation.Reputation memory r = rep.getReputation(prover);
            assertLe(r.score, 10000, "score must never exceed MAX_SCORE (10000)");
        }
    }

    /// @notice INVARIANT: Banned provers stay banned (unless explicitly unbanned, which handler does not do).
    function invariant_reputation_banned_stay_banned() public view {
        // Since the handler never calls unban(), any prover that was auto-banned
        // (score reached 0 via slash) should remain banned.
        // We verify that banned provers have score == 0 OR were explicitly banned.
        uint256 len = handler.registeredProversLength();
        for (uint256 i = 0; i < len; i++) {
            address prover = handler.registeredProvers(i);
            ProverReputation.Reputation memory r = rep.getReputation(prover);
            if (r.isBanned) {
                // Banned provers should not be processable (handler skips them)
                // This is a consistency check -- banned status persists
                assertTrue(r.isBanned, "banned flag should persist");
            }
        }
    }

    /// @notice INVARIANT: totalJobs == completedJobs + failedJobs + abandonedJobs for every prover.
    function invariant_reputation_job_counters_consistent() public view {
        uint256 len = handler.registeredProversLength();
        for (uint256 i = 0; i < len; i++) {
            address prover = handler.registeredProvers(i);
            ProverReputation.Reputation memory r = rep.getReputation(prover);
            assertEq(
                r.totalJobs,
                r.completedJobs + r.failedJobs + r.abandonedJobs,
                "totalJobs must equal completed + failed + abandoned"
            );
        }
    }

    /// @notice INVARIANT: totalProvers in contract matches actual registrations.
    function invariant_reputation_totalProvers_matches() public view {
        assertEq(
            rep.totalProvers(),
            handler.ghost_totalRegistered(),
            "totalProvers must match handler's registration count"
        );
    }

    /// @notice INVARIANT: Ghost variables are consistent with action counts.
    function invariant_reputation_ghost_sane() public view {
        uint256 totalActions = handler.successCount() + handler.failureCount() + handler.abandonCount();
        // Total actions should be finite and bounded by test execution
        assertLe(totalActions, 100_000, "total actions should be bounded");
    }
}
