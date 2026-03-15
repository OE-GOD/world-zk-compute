// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

/// @title ProverReputation
/// @notice Tracks prover reliability and performance on-chain
/// @dev Reputation affects job priority and can enable/disable features
contract ProverReputation {
    // ========================================================================
    // TYPES
    // ========================================================================

    /// @notice Prover reputation data
    struct Reputation {
        // Stats (packed)
        uint64 totalJobs;
        uint64 completedJobs;
        uint64 failedJobs;
        uint64 abandonedJobs;
        // Earnings
        uint256 totalEarnings;
        // Performance (packed)
        uint64 avgProofTimeMs;
        uint64 lastJobAt;
        uint64 lastUpdateAt;
        // Score and tier (packed)
        uint32 score; // 0-10000 basis points
        uint8 tier;
        bool isRegistered;
        bool isSlashed;
        bool isBanned;
    }

    /// @notice Reputation tier levels
    enum Tier {
        Unranked, // New prover, no history
        Bronze, // Score < 5000
        Silver, // Score 5000-7499
        Gold, // Score 7500-8999
        Platinum, // Score 9000-9499
        Diamond // Score 9500+
    }

    /// @notice Slashing event details
    struct SlashEvent {
        uint256 timestamp;
        string reason;
        uint256 penaltyBps; // Basis points reduction
        address reportedBy;
    }

    // ========================================================================
    // CONSTANTS
    // ========================================================================

    uint256 public constant MAX_SCORE = 10000;
    uint256 public constant INITIAL_SCORE = 5000; // Start at 50%

    // Score adjustments (basis points)
    uint256 public constant SUCCESS_BONUS = 50; // +0.5% per success
    uint256 public constant FAILURE_PENALTY = 200; // -2% per failure
    uint256 public constant ABANDON_PENALTY = 500; // -5% per abandon
    uint256 public constant FAST_PROOF_BONUS = 25; // +0.25% for fast proofs

    // Tier thresholds
    uint256 public constant BRONZE_THRESHOLD = 0;
    uint256 public constant SILVER_THRESHOLD = 5000;
    uint256 public constant GOLD_THRESHOLD = 7500;
    uint256 public constant PLATINUM_THRESHOLD = 9000;
    uint256 public constant DIAMOND_THRESHOLD = 9500;

    // Time-based decay
    uint256 public constant DECAY_PERIOD = 30 days;
    uint256 public constant DECAY_RATE = 100; // -1% per period of inactivity

    /// @notice Maximum elapsed time considered for decay calculation (365 days).
    /// @dev If a prover has been inactive longer than MAX_DECAY_PERIOD, the elapsed
    /// time is clamped to this value. This prevents the score from decaying fully to
    /// zero for extremely long absences (365 days / 30 days = 12 periods, retaining
    /// ~88.6% of the score via 0.99^12). It also bounds the loop iteration count,
    /// providing a tighter gas ceiling than the 120-iteration fallback cap.
    uint256 public constant MAX_DECAY_PERIOD = 365 days;

    // Cooldown constraints
    uint256 public constant MIN_COOLDOWN = 1 minutes;
    uint256 public constant MAX_COOLDOWN = 30 days;
    /// @notice Maximum valid timestamp for uint64 fields (year ~2554)
    uint256 public constant MAX_UINT64_TIMESTAMP = type(uint64).max;

    // ========================================================================
    // STATE
    // ========================================================================

    /// @notice Prover reputation data
    mapping(address => Reputation) public reputations;

    /// @notice Slashing history
    mapping(address => SlashEvent[]) public slashHistory;

    /// @notice Authorized reporters (execution engine, etc.)
    mapping(address => bool) public authorizedReporters;

    /// @notice Contract owner
    address public owner;

    /// @notice Total registered provers
    uint256 public totalProvers;

    /// @notice Provers by tier
    mapping(uint8 => uint256) public proversByTier;

    /// @notice Configurable cooldown period between slashes (default 1 hour)
    uint256 public slashCooldown = 1 hours;

    // ========================================================================
    // EVENTS
    // ========================================================================

    event ProverRegistered(address indexed prover, uint256 timestamp);
    event JobCompleted(address indexed prover, uint256 proofTimeMs, uint256 newScore);
    event JobFailed(address indexed prover, string reason, uint256 newScore);
    event JobAbandoned(address indexed prover, uint256 requestId, uint256 newScore);
    event ProverSlashed(address indexed prover, string reason, uint256 penaltyBps);
    event ProverBanned(address indexed prover, string reason);
    event ProverUnbanned(address indexed prover);
    event TierChanged(address indexed prover, uint8 oldTier, uint8 newTier);
    event ReporterAuthorized(address indexed reporter);
    event ReporterRevoked(address indexed reporter);
    event SlashCooldownUpdated(uint256 oldCooldown, uint256 newCooldown);

    // ========================================================================
    // ERRORS
    // ========================================================================

    error NotOwner();
    error NotAuthorized();
    error ProverNotRegistered();
    error ProverIsBanned();
    error AlreadyRegistered();
    error InvalidScore();
    error InvalidCooldown();
    error TimestampOverflow();
    error InvalidTimestamp();
    error SlashCooldownActive();

    // ========================================================================
    // MODIFIERS
    // ========================================================================

    modifier onlyOwner() {
        if (msg.sender != owner) revert NotOwner();
        _;
    }

    modifier onlyAuthorized() {
        if (!authorizedReporters[msg.sender] && msg.sender != owner) revert NotAuthorized();
        _;
    }

    modifier notBanned(address prover) {
        if (reputations[prover].isBanned) revert ProverIsBanned();
        _;
    }

    // ========================================================================
    // CONSTRUCTOR
    // ========================================================================

    constructor() {
        owner = msg.sender;
    }

    // ========================================================================
    // PROVER REGISTRATION
    // ========================================================================

    /// @notice Register as a prover
    function register() external {
        if (reputations[msg.sender].isRegistered) revert AlreadyRegistered();
        _checkTimestamp();

        reputations[msg.sender] = Reputation({
            totalJobs: 0,
            completedJobs: 0,
            failedJobs: 0,
            abandonedJobs: 0,
            totalEarnings: 0,
            avgProofTimeMs: 0,
            lastJobAt: 0,
            lastUpdateAt: uint64(block.timestamp),
            // forge-lint: disable-next-line(unsafe-typecast)
            score: uint32(INITIAL_SCORE),
            tier: uint8(Tier.Unranked),
            isRegistered: true,
            isSlashed: false,
            isBanned: false
        });

        totalProvers++;
        proversByTier[uint8(Tier.Unranked)]++;

        emit ProverRegistered(msg.sender, block.timestamp);
    }

    // ========================================================================
    // REPUTATION UPDATES
    // ========================================================================

    /// @notice Record successful job completion
    /// @param prover Prover address
    /// @param proofTimeMs Time taken to generate proof
    /// @param earnings Amount earned
    function recordSuccess(address prover, uint256 proofTimeMs, uint256 earnings)
        external
        onlyAuthorized
        notBanned(prover)
    {
        Reputation storage rep = reputations[prover];
        if (!rep.isRegistered) revert ProverNotRegistered();
        _checkTimestamp();

        // Update stats
        rep.totalJobs++;
        rep.completedJobs++;
        rep.totalEarnings += earnings;
        rep.lastJobAt = uint64(block.timestamp);
        rep.lastUpdateAt = uint64(block.timestamp);

        // Update rolling average proof time
        // forge-lint: disable-next-line(unsafe-typecast)
        uint64 proofTime64 = uint64(proofTimeMs);
        if (rep.avgProofTimeMs == 0) {
            rep.avgProofTimeMs = proofTime64;
        } else {
            rep.avgProofTimeMs = uint64((uint256(rep.avgProofTimeMs) * 9 + proofTimeMs) / 10);
        }

        // Calculate score adjustment
        uint256 bonus = SUCCESS_BONUS;

        // Extra bonus for fast proofs (under average)
        if (proofTimeMs < rep.avgProofTimeMs) {
            bonus += FAST_PROOF_BONUS;
        }

        // Update score
        uint256 oldScore = rep.score;
        rep.score = _boundScore(uint256(rep.score) + bonus);

        // Update tier if changed
        _updateTier(prover, oldScore);

        emit JobCompleted(prover, proofTimeMs, rep.score);
    }

    /// @notice Record failed job
    /// @param prover Prover address
    /// @param reason Failure reason
    function recordFailure(address prover, string calldata reason) external onlyAuthorized notBanned(prover) {
        Reputation storage rep = reputations[prover];
        if (!rep.isRegistered) revert ProverNotRegistered();
        _checkTimestamp();

        rep.totalJobs++;
        rep.failedJobs++;
        rep.lastJobAt = uint64(block.timestamp);
        rep.lastUpdateAt = uint64(block.timestamp);

        // Apply penalty
        uint256 oldScore = rep.score;
        uint256 currentScore = uint256(rep.score);
        rep.score = _boundScore(currentScore > FAILURE_PENALTY ? currentScore - FAILURE_PENALTY : 0);

        _updateTier(prover, oldScore);

        emit JobFailed(prover, reason, rep.score);
    }

    /// @notice Record abandoned job (claimed but never submitted)
    /// @param prover Prover address
    /// @param requestId Request that was abandoned
    function recordAbandon(address prover, uint256 requestId) external onlyAuthorized notBanned(prover) {
        Reputation storage rep = reputations[prover];
        if (!rep.isRegistered) revert ProverNotRegistered();
        _checkTimestamp();

        rep.totalJobs++;
        rep.abandonedJobs++;
        rep.lastUpdateAt = uint64(block.timestamp);

        // Apply heavy penalty
        uint256 oldScore = rep.score;
        uint256 currentScore = uint256(rep.score);
        rep.score = _boundScore(currentScore > ABANDON_PENALTY ? currentScore - ABANDON_PENALTY : 0);

        _updateTier(prover, oldScore);

        emit JobAbandoned(prover, requestId, rep.score);
    }

    /// @notice Slash a prover for misbehavior
    /// @param prover Prover address
    /// @param reason Slash reason
    /// @param penaltyBps Penalty in basis points
    function slash(address prover, string calldata reason, uint256 penaltyBps) external onlyOwner {
        Reputation storage rep = reputations[prover];
        if (!rep.isRegistered) revert ProverNotRegistered();
        _checkTimestamp();

        // Enforce cooldown between slashes
        SlashEvent[] storage history = slashHistory[prover];
        if (history.length > 0) {
            uint256 lastSlashTime = history[history.length - 1].timestamp;
            if (block.timestamp < lastSlashTime + slashCooldown) revert SlashCooldownActive();
        }

        // Record slash
        slashHistory[prover].push(
            SlashEvent({timestamp: block.timestamp, reason: reason, penaltyBps: penaltyBps, reportedBy: msg.sender})
        );

        rep.isSlashed = true;
        rep.lastUpdateAt = uint64(block.timestamp);

        // Apply penalty
        uint256 oldScore = rep.score;
        uint256 currentScore = uint256(rep.score);
        uint256 penalty = (currentScore * penaltyBps) / 10000;
        rep.score = _boundScore(currentScore > penalty ? currentScore - penalty : 0);

        _updateTier(prover, oldScore);

        emit ProverSlashed(prover, reason, penaltyBps);

        // Auto-ban if score drops to 0
        if (rep.score == 0) {
            rep.isBanned = true;
            emit ProverBanned(prover, "Score reached zero");
        }
    }

    /// @notice Ban a prover
    function ban(address prover, string calldata reason) external onlyOwner {
        Reputation storage rep = reputations[prover];
        if (!rep.isRegistered) revert ProverNotRegistered();

        rep.isBanned = true;
        rep.lastUpdateAt = uint64(block.timestamp);

        emit ProverBanned(prover, reason);
    }

    /// @notice Unban a prover
    function unban(address prover) external onlyOwner {
        Reputation storage rep = reputations[prover];
        if (!rep.isRegistered) revert ProverNotRegistered();

        rep.isBanned = false;
        rep.lastUpdateAt = uint64(block.timestamp);

        emit ProverUnbanned(prover);
    }

    // ========================================================================
    // VIEW FUNCTIONS
    // ========================================================================

    /// @notice Get prover reputation
    function getReputation(address prover) external view returns (Reputation memory) {
        return reputations[prover];
    }

    /// @notice Get prover score (with decay applied)
    function getScore(address prover) external view returns (uint256) {
        return _getScoreWithDecay(prover);
    }

    /// @notice Get prover tier
    function getTier(address prover) external view returns (Tier) {
        return Tier(reputations[prover].tier);
    }

    /// @notice Get success rate (percentage, 2 decimals)
    function getSuccessRate(address prover) external view returns (uint256) {
        Reputation storage rep = reputations[prover];
        if (rep.totalJobs == 0) return 0;
        return (rep.completedJobs * 10000) / rep.totalJobs;
    }

    /// @notice Check if prover is in good standing
    function isGoodStanding(address prover) external view returns (bool) {
        Reputation storage rep = reputations[prover];
        return rep.isRegistered && !rep.isBanned && rep.score >= SILVER_THRESHOLD;
    }

    /// @notice Get slash history
    function getSlashHistory(address prover) external view returns (SlashEvent[] memory) {
        return slashHistory[prover];
    }

    /// @notice Get provers count by tier
    function getProversByTier(Tier tier) external view returns (uint256) {
        return proversByTier[uint8(tier)];
    }

    // ========================================================================
    // ADMIN FUNCTIONS
    // ========================================================================

    /// @notice Authorize a reporter (e.g., execution engine)
    function authorizeReporter(address reporter) external onlyOwner {
        authorizedReporters[reporter] = true;
        emit ReporterAuthorized(reporter);
    }

    /// @notice Revoke reporter authorization
    function revokeReporter(address reporter) external onlyOwner {
        authorizedReporters[reporter] = false;
        emit ReporterRevoked(reporter);
    }

    /// @notice Transfer ownership
    function transferOwnership(address newOwner) external onlyOwner {
        require(newOwner != address(0), "Invalid owner");
        owner = newOwner;
    }

    /// @notice Set the cooldown period between slashes
    /// @param newCooldown New cooldown in seconds (must be >= MIN_COOLDOWN and <= MAX_COOLDOWN)
    function setSlashCooldown(uint256 newCooldown) external onlyOwner {
        if (newCooldown < MIN_COOLDOWN || newCooldown > MAX_COOLDOWN) revert InvalidCooldown();
        uint256 oldCooldown = slashCooldown;
        slashCooldown = newCooldown;
        emit SlashCooldownUpdated(oldCooldown, newCooldown);
    }

    // ========================================================================
    // INTERNAL FUNCTIONS
    // ========================================================================

    /// @notice Get score with time decay applied
    /// @dev Reverts if block.timestamp < lastJobAt (indicates clock manipulation or
    /// corrupt state). Clamps the elapsed time to MAX_DECAY_PERIOD (365 days = 12
    /// periods) so that long-absent provers retain ~88.6% of their score rather than
    /// decaying towards zero. A secondary cap at 120 iterations guards against gas DoS
    /// if MAX_DECAY_PERIOD is ever raised.
    function _getScoreWithDecay(address prover) internal view returns (uint256) {
        Reputation storage rep = reputations[prover];

        if (rep.lastJobAt == 0) {
            return uint256(rep.score);
        }

        // Defensive check: block.timestamp must not be before lastJobAt.
        // This guards against corrupted state or unexpected clock behaviour.
        if (block.timestamp < uint256(rep.lastJobAt)) revert InvalidTimestamp();

        uint256 timeSinceLastJob = block.timestamp - uint256(rep.lastJobAt);

        // Clamp elapsed time to MAX_DECAY_PERIOD so that extremely long absences
        // do not decay the score beyond the 365-day ceiling (12 periods, ~88.6% retained).
        if (timeSinceLastJob > MAX_DECAY_PERIOD) {
            timeSinceLastJob = MAX_DECAY_PERIOD;
        }

        uint256 decayPeriods = timeSinceLastJob / DECAY_PERIOD;

        if (decayPeriods == 0) {
            return uint256(rep.score);
        }

        // Secondary cap: prevent gas DoS if MAX_DECAY_PERIOD is ever raised.
        // 120 periods ≈ 10 years, score * 0.99^120 ≈ 30%.
        uint256 maxPeriods = 120;
        if (decayPeriods > maxPeriods) {
            decayPeriods = maxPeriods;
        }

        // Apply multiplicative decay: score = score * (1 - DECAY_RATE/10000)^periods
        uint256 decayedScore = uint256(rep.score);
        for (uint256 i = 0; i < decayPeriods && decayedScore > 0; i++) {
            uint256 decay = (decayedScore * DECAY_RATE) / 10000;
            if (decay == 0) break; // Score too small for further decay
            decayedScore = decayedScore - decay;
        }

        return decayedScore;
    }

    /// @notice Check that block.timestamp fits in uint64 to prevent overflow in packed fields
    function _checkTimestamp() internal view {
        if (block.timestamp > MAX_UINT64_TIMESTAMP) revert TimestampOverflow();
    }

    /// @notice Bound score to valid range
    function _boundScore(uint256 score) internal pure returns (uint32) {
        return uint32(score > MAX_SCORE ? MAX_SCORE : score);
    }

    /// @notice Update tier based on score
    function _updateTier(address prover, uint256 /* oldScore */) internal {
        Reputation storage rep = reputations[prover];
        uint8 oldTier = rep.tier;
        uint8 newTier = _calculateTier(rep.score);

        if (oldTier != newTier) {
            proversByTier[oldTier]--;
            proversByTier[newTier]++;
            rep.tier = newTier;
            emit TierChanged(prover, oldTier, newTier);
        }
    }

    /// @notice Calculate tier from score
    function _calculateTier(uint32 score) internal pure returns (uint8) {
        if (score >= DIAMOND_THRESHOLD) return uint8(Tier.Diamond);
        if (score >= PLATINUM_THRESHOLD) return uint8(Tier.Platinum);
        if (score >= GOLD_THRESHOLD) return uint8(Tier.Gold);
        if (score >= SILVER_THRESHOLD) return uint8(Tier.Silver);
        if (score > 0) return uint8(Tier.Bronze);
        return uint8(Tier.Unranked);
    }
}
