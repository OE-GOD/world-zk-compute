// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";

/// @title ProverRegistry - Decentralized Prover Network
/// @notice Manages prover registration, staking, reputation, and slashing
/// @dev Provers stake tokens to participate, earn rewards, and can be slashed for misbehavior
contract ProverRegistry is ReentrancyGuard, Ownable {
    // ============================================================
    // TYPES
    // ============================================================

    struct Prover {
        address owner;              // Prover operator address
        uint256 stake;              // Staked amount
        uint256 reputation;         // Reputation score (0-10000 = 0-100.00%)
        uint256 proofsSubmitted;    // Total proofs submitted
        uint256 proofsFailed;       // Failed/slashed proofs
        uint256 totalEarnings;      // Total earnings
        uint256 registeredAt;       // Registration timestamp
        uint256 lastActiveAt;       // Last activity timestamp
        bool active;                // Is currently active
        string endpoint;            // P2P endpoint (optional)
    }

    struct SlashEvent {
        address prover;
        uint256 amount;
        string reason;
        uint256 timestamp;
    }

    // ============================================================
    // STATE
    // ============================================================

    /// @notice Staking token (e.g., WLD or WETH)
    IERC20 public immutable stakingToken;

    /// @notice Minimum stake required to be a prover
    uint256 public minStake;

    /// @notice Slash percentage for failures (basis points, e.g., 500 = 5%)
    uint256 public slashBasisPoints;

    /// @notice Registered provers
    mapping(address => Prover) public provers;

    /// @notice List of active prover addresses
    address[] public activeProvers;

    /// @notice Index of prover in activeProvers array
    mapping(address => uint256) private proverIndex;

    /// @notice Total staked across all provers
    uint256 public totalStaked;

    /// @notice Slash events history
    SlashEvent[] public slashHistory;

    /// @notice Addresses authorized to slash (ExecutionEngine, governance)
    mapping(address => bool) public slashers;

    // ============================================================
    // EVENTS
    // ============================================================

    event ProverRegistered(address indexed prover, uint256 stake, string endpoint);
    event ProverDeactivated(address indexed prover);
    event ProverReactivated(address indexed prover);
    event StakeAdded(address indexed prover, uint256 amount, uint256 newTotal);
    event StakeWithdrawn(address indexed prover, uint256 amount, uint256 newTotal);
    event ProverSlashed(address indexed prover, uint256 amount, string reason);
    event ReputationUpdated(address indexed prover, uint256 oldRep, uint256 newRep);
    event RewardDistributed(address indexed prover, uint256 amount);
    event SlasherUpdated(address indexed slasher, bool authorized);

    // ============================================================
    // ERRORS
    // ============================================================

    error InsufficientStake();
    error ProverNotRegistered();
    error ProverAlreadyRegistered();
    error ProverNotActive();
    error UnauthorizedSlasher();
    error WithdrawalWouldBreachMinimum();
    error NoStakeToWithdraw();

    // ============================================================
    // CONSTRUCTOR
    // ============================================================

    constructor(
        address _stakingToken,
        uint256 _minStake,
        uint256 _slashBasisPoints
    ) Ownable(msg.sender) {
        stakingToken = IERC20(_stakingToken);
        minStake = _minStake;
        slashBasisPoints = _slashBasisPoints;
    }

    // ============================================================
    // PROVER REGISTRATION
    // ============================================================

    /// @notice Register as a prover with initial stake
    /// @param stake Amount to stake
    /// @param endpoint P2P endpoint for coordination (optional)
    function register(uint256 stake, string calldata endpoint) external nonReentrant {
        if (provers[msg.sender].registeredAt != 0) revert ProverAlreadyRegistered();
        if (stake < minStake) revert InsufficientStake();

        // Transfer stake
        stakingToken.transferFrom(msg.sender, address(this), stake);

        // Create prover record
        provers[msg.sender] = Prover({
            owner: msg.sender,
            stake: stake,
            reputation: 5000, // Start at 50%
            proofsSubmitted: 0,
            proofsFailed: 0,
            totalEarnings: 0,
            registeredAt: block.timestamp,
            lastActiveAt: block.timestamp,
            active: true,
            endpoint: endpoint
        });

        // Add to active list
        proverIndex[msg.sender] = activeProvers.length;
        activeProvers.push(msg.sender);

        totalStaked += stake;

        emit ProverRegistered(msg.sender, stake, endpoint);
    }

    /// @notice Add more stake
    /// @param amount Amount to add
    function addStake(uint256 amount) external nonReentrant {
        Prover storage prover = provers[msg.sender];
        if (prover.registeredAt == 0) revert ProverNotRegistered();

        stakingToken.transferFrom(msg.sender, address(this), amount);

        prover.stake += amount;
        totalStaked += amount;

        // Reactivate if was deactivated due to low stake
        if (!prover.active && prover.stake >= minStake) {
            _activateProver(msg.sender);
        }

        emit StakeAdded(msg.sender, amount, prover.stake);
    }

    /// @notice Withdraw stake (must maintain minimum if active)
    /// @param amount Amount to withdraw
    function withdrawStake(uint256 amount) external nonReentrant {
        Prover storage prover = provers[msg.sender];
        if (prover.registeredAt == 0) revert ProverNotRegistered();
        if (prover.stake == 0) revert NoStakeToWithdraw();

        uint256 newStake = prover.stake - amount;

        // If staying active, must maintain minimum
        if (prover.active && newStake < minStake) {
            revert WithdrawalWouldBreachMinimum();
        }

        prover.stake = newStake;
        totalStaked -= amount;

        stakingToken.transfer(msg.sender, amount);

        emit StakeWithdrawn(msg.sender, amount, newStake);
    }

    /// @notice Deactivate (stop receiving jobs, can withdraw below minimum)
    function deactivate() external {
        Prover storage prover = provers[msg.sender];
        if (prover.registeredAt == 0) revert ProverNotRegistered();
        if (!prover.active) revert ProverNotActive();

        _deactivateProver(msg.sender);
    }

    /// @notice Reactivate (must have minimum stake)
    function reactivate() external {
        Prover storage prover = provers[msg.sender];
        if (prover.registeredAt == 0) revert ProverNotRegistered();
        if (prover.active) return; // Already active
        if (prover.stake < minStake) revert InsufficientStake();

        _activateProver(msg.sender);
    }

    // ============================================================
    // SLASHING
    // ============================================================

    /// @notice Slash a prover for misbehavior
    /// @param prover Address to slash
    /// @param reason Reason for slashing
    function slash(address prover, string calldata reason) external nonReentrant {
        if (!slashers[msg.sender] && msg.sender != owner()) revert UnauthorizedSlasher();

        Prover storage p = provers[prover];
        if (p.registeredAt == 0) revert ProverNotRegistered();

        uint256 slashAmount = (p.stake * slashBasisPoints) / 10000;
        if (slashAmount > p.stake) slashAmount = p.stake;

        p.stake -= slashAmount;
        p.proofsFailed++;
        totalStaked -= slashAmount;

        // Update reputation (decrease by 5% per slash)
        uint256 oldRep = p.reputation;
        p.reputation = (p.reputation * 95) / 100;
        emit ReputationUpdated(prover, oldRep, p.reputation);

        // Deactivate if stake falls below minimum
        if (p.stake < minStake && p.active) {
            _deactivateProver(prover);
        }

        // Record slash event
        slashHistory.push(SlashEvent({
            prover: prover,
            amount: slashAmount,
            reason: reason,
            timestamp: block.timestamp
        }));

        // Slashed funds go to treasury (owner)
        stakingToken.transfer(owner(), slashAmount);

        emit ProverSlashed(prover, slashAmount, reason);
    }

    // ============================================================
    // REWARDS & REPUTATION
    // ============================================================

    /// @notice Record successful proof and update reputation
    /// @param prover Prover address
    /// @param reward Reward amount earned
    function recordSuccess(address prover, uint256 reward) external {
        if (!slashers[msg.sender] && msg.sender != owner()) revert UnauthorizedSlasher();

        Prover storage p = provers[prover];
        if (p.registeredAt == 0) revert ProverNotRegistered();

        p.proofsSubmitted++;
        p.totalEarnings += reward;
        p.lastActiveAt = block.timestamp;

        // Increase reputation (by 0.5% per success, max 100%)
        uint256 oldRep = p.reputation;
        p.reputation = p.reputation + 50;
        if (p.reputation > 10000) p.reputation = 10000;

        emit ReputationUpdated(prover, oldRep, p.reputation);
        emit RewardDistributed(prover, reward);
    }

    // ============================================================
    // PROVER SELECTION
    // ============================================================

    /// @notice Get a weighted random prover based on stake and reputation
    /// @param seed Random seed (e.g., blockhash)
    /// @return Selected prover address
    function selectProver(uint256 seed) external view returns (address) {
        uint256 count = activeProvers.length;
        if (count == 0) return address(0);
        if (count == 1) return activeProvers[0];

        // Calculate total weight (stake * reputation)
        uint256 totalWeight = 0;
        for (uint256 i = 0; i < count; i++) {
            Prover storage p = provers[activeProvers[i]];
            totalWeight += (p.stake * p.reputation) / 10000;
        }

        if (totalWeight == 0) return activeProvers[seed % count];

        // Weighted random selection
        uint256 random = uint256(keccak256(abi.encodePacked(seed, block.timestamp))) % totalWeight;
        uint256 cumulative = 0;

        for (uint256 i = 0; i < count; i++) {
            Prover storage p = provers[activeProvers[i]];
            cumulative += (p.stake * p.reputation) / 10000;
            if (random < cumulative) {
                return activeProvers[i];
            }
        }

        return activeProvers[count - 1];
    }

    /// @notice Get top N provers by reputation
    /// @param n Number of provers to return
    /// @return Top prover addresses
    function getTopProvers(uint256 n) external view returns (address[] memory) {
        uint256 count = activeProvers.length;
        if (n > count) n = count;

        // Simple selection (not gas efficient for large n, but fine for small networks)
        address[] memory result = new address[](n);
        bool[] memory used = new bool[](count);

        for (uint256 i = 0; i < n; i++) {
            uint256 maxRep = 0;
            uint256 maxIdx = 0;

            for (uint256 j = 0; j < count; j++) {
                if (!used[j]) {
                    uint256 rep = provers[activeProvers[j]].reputation;
                    if (rep > maxRep) {
                        maxRep = rep;
                        maxIdx = j;
                    }
                }
            }

            result[i] = activeProvers[maxIdx];
            used[maxIdx] = true;
        }

        return result;
    }

    // ============================================================
    // VIEW FUNCTIONS
    // ============================================================

    /// @notice Get prover info
    function getProver(address prover) external view returns (Prover memory) {
        return provers[prover];
    }

    /// @notice Get number of active provers
    function activeProverCount() external view returns (uint256) {
        return activeProvers.length;
    }

    /// @notice Get all active prover addresses
    function getActiveProvers() external view returns (address[] memory) {
        return activeProvers;
    }

    /// @notice Check if address is registered prover
    function isProver(address addr) external view returns (bool) {
        return provers[addr].registeredAt != 0;
    }

    /// @notice Check if prover is active
    function isActive(address addr) external view returns (bool) {
        return provers[addr].active;
    }

    /// @notice Get prover's effective weight (stake * reputation)
    function getWeight(address prover) external view returns (uint256) {
        Prover storage p = provers[prover];
        return (p.stake * p.reputation) / 10000;
    }

    // ============================================================
    // ADMIN FUNCTIONS
    // ============================================================

    /// @notice Update minimum stake requirement
    function setMinStake(uint256 _minStake) external onlyOwner {
        minStake = _minStake;
    }

    /// @notice Update slash percentage
    function setSlashBasisPoints(uint256 _slashBasisPoints) external onlyOwner {
        require(_slashBasisPoints <= 10000, "Max 100%");
        slashBasisPoints = _slashBasisPoints;
    }

    /// @notice Authorize/deauthorize a slasher
    function setSlasher(address slasher, bool authorized) external onlyOwner {
        slashers[slasher] = authorized;
        emit SlasherUpdated(slasher, authorized);
    }

    // ============================================================
    // INTERNAL FUNCTIONS
    // ============================================================

    function _activateProver(address prover) internal {
        Prover storage p = provers[prover];
        p.active = true;

        proverIndex[prover] = activeProvers.length;
        activeProvers.push(prover);

        emit ProverReactivated(prover);
    }

    function _deactivateProver(address prover) internal {
        Prover storage p = provers[prover];
        p.active = false;

        // Remove from active list (swap with last element)
        uint256 idx = proverIndex[prover];
        uint256 lastIdx = activeProvers.length - 1;

        if (idx != lastIdx) {
            address lastProver = activeProvers[lastIdx];
            activeProvers[idx] = lastProver;
            proverIndex[lastProver] = idx;
        }

        activeProvers.pop();
        delete proverIndex[prover];

        emit ProverDeactivated(prover);
    }
}
