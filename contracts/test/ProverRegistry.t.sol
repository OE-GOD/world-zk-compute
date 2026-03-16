// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/ProverRegistry.sol";
import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";

contract MockToken is ERC20 {
    constructor() ERC20("Mock", "MCK") {
        _mint(msg.sender, 1_000_000 ether);
    }

    function mint(address to, uint256 amount) external {
        _mint(to, amount);
    }
}

contract ProverRegistryTest is Test {
    // Re-declare events from ProverRegistry so we can use them with vm.expectEmit
    event ProverRegistered(address indexed prover, uint256 stake, string endpoint);
    event ProverDeactivated(address indexed prover);
    event ProverReactivated(address indexed prover);
    event StakeAdded(address indexed prover, uint256 amount, uint256 newTotal);
    event StakeWithdrawn(address indexed prover, uint256 amount, uint256 newTotal);
    event ProverSlashed(address indexed prover, uint256 amount, string reason);
    event ReputationUpdated(address indexed prover, uint256 oldRep, uint256 newRep);
    event RewardDistributed(address indexed prover, uint256 amount);
    event SlasherUpdated(address indexed slasher, bool authorized);

    ProverRegistry public registry;
    MockToken public token;

    address public deployer = address(this);
    address public prover1 = address(0x1001);
    address public prover2 = address(0x1002);
    address public prover3 = address(0x1003);
    address public slasherAddr = address(0x2001);
    address public randomUser = address(0x3001);

    uint256 public constant MIN_STAKE = 100 ether;
    uint256 public constant SLASH_BPS = 500; // 5%
    uint256 public constant STAKE_AMOUNT = 200 ether;

    function setUp() public {
        token = new MockToken();
        registry = new ProverRegistry(address(token), MIN_STAKE, SLASH_BPS);

        // Give provers tokens and approve
        address[3] memory proversArr = [prover1, prover2, prover3];
        for (uint256 i = 0; i < proversArr.length; i++) {
            token.mint(proversArr[i], STAKE_AMOUNT * 50);
            vm.prank(proversArr[i]);
            token.approve(address(registry), type(uint256).max);
        }
    }

    // ========================================================================
    // HELPERS
    // ========================================================================

    function _registerProver(address prover, uint256 stake, string memory endpoint) internal {
        vm.prank(prover);
        registry.register(stake, endpoint);
    }

    function _registerDefaultProver(address prover) internal {
        _registerProver(prover, MIN_STAKE, "https://prover.example.com");
    }

    // ========================================================================
    // 1. testRegisterProver
    //    Register with sufficient stake, verify prover data
    //    (owner, stake, reputation=5000, active=true)
    // ========================================================================

    function testRegisterProver() public {
        uint256 stake = STAKE_AMOUNT;
        string memory endpoint = "https://prover1.example.com";

        uint256 tokenBalBefore = token.balanceOf(prover1);

        vm.prank(prover1);
        vm.expectEmit(true, false, false, true);
        emit ProverRegistered(prover1, stake, endpoint);
        registry.register(stake, endpoint);

        // Verify token transfer
        assertEq(token.balanceOf(prover1), tokenBalBefore - stake);
        assertEq(token.balanceOf(address(registry)), stake);

        // Verify prover data
        ProverRegistry.Prover memory p = registry.getProver(prover1);
        assertEq(p.owner, prover1, "owner should be prover1");
        assertEq(p.stake, stake, "stake should match deposited amount");
        assertEq(p.reputation, 5000, "initial reputation should be 5000 (50%)");
        assertTrue(p.active, "prover should be active");
        assertEq(p.proofsSubmitted, 0);
        assertEq(p.proofsFailed, 0);
        assertEq(p.totalEarnings, 0);
        assertGt(p.registeredAt, 0);
        assertEq(p.lastActiveAt, p.registeredAt);
        assertEq(p.endpoint, endpoint);

        // Verify activeProvers list
        assertEq(registry.activeProverCount(), 1);
        address[] memory active = registry.getActiveProvers();
        assertEq(active[0], prover1);
        assertEq(registry.totalStaked(), stake);
    }

    // ========================================================================
    // 2. testRegisterProverInsufficientStake
    //    Register with less than minStake, expect InsufficientStake revert
    // ========================================================================

    function testRegisterProverInsufficientStake() public {
        vm.prank(prover1);
        vm.expectRevert(ProverRegistry.InsufficientStake.selector);
        registry.register(MIN_STAKE - 1, "endpoint");
    }

    // ========================================================================
    // 3. testRegisterProverDuplicate
    //    Register same address twice, expect ProverAlreadyRegistered revert
    // ========================================================================

    function testRegisterProverDuplicate() public {
        _registerDefaultProver(prover1);

        vm.prank(prover1);
        vm.expectRevert(ProverRegistry.ProverAlreadyRegistered.selector);
        registry.register(MIN_STAKE, "endpoint2");
    }

    // ========================================================================
    // 4. testAddStake
    //    Register, then addStake, verify increased stake and totalStaked
    // ========================================================================

    function testAddStake() public {
        _registerDefaultProver(prover1);
        uint256 addAmount = 50 ether;

        vm.prank(prover1);
        vm.expectEmit(true, false, false, true);
        emit StakeAdded(prover1, addAmount, MIN_STAKE + addAmount);
        registry.addStake(addAmount);

        ProverRegistry.Prover memory p = registry.getProver(prover1);
        assertEq(p.stake, MIN_STAKE + addAmount, "stake should increase by addAmount");
        assertEq(registry.totalStaked(), MIN_STAKE + addAmount, "totalStaked should increase");
    }

    // ========================================================================
    // 5. testWithdrawStake
    //    Register, withdraw partial (keeping above minStake), verify reduced
    //    stake
    // ========================================================================

    function testWithdrawStake() public {
        uint256 initialStake = 500 ether;
        _registerProver(prover1, initialStake, "ep");

        uint256 withdrawAmount = 300 ether; // leaves 200 ether, still >= MIN_STAKE
        uint256 balBefore = token.balanceOf(prover1);

        vm.prank(prover1);
        vm.expectEmit(true, false, false, true);
        emit StakeWithdrawn(prover1, withdrawAmount, initialStake - withdrawAmount);
        registry.withdrawStake(withdrawAmount);

        ProverRegistry.Prover memory p = registry.getProver(prover1);
        assertEq(p.stake, initialStake - withdrawAmount, "stake should decrease");
        assertEq(token.balanceOf(prover1), balBefore + withdrawAmount, "tokens returned to prover");
        assertTrue(p.active, "prover should still be active");
    }

    // ========================================================================
    // 6. testWithdrawStakeBelowMinimumReverts
    //    Active prover can't withdraw below minStake,
    //    expect WithdrawalWouldBreachMinimum
    // ========================================================================

    function testWithdrawStakeBelowMinimumReverts() public {
        _registerDefaultProver(prover1);

        // Even 1 wei would bring 100 ether below MIN_STAKE while active
        vm.prank(prover1);
        vm.expectRevert(ProverRegistry.WithdrawalWouldBreachMinimum.selector);
        registry.withdrawStake(1);
    }

    // ========================================================================
    // 7. testDeactivateProver
    //    Register, deactivate, verify active=false, removed from activeProvers
    // ========================================================================

    function testDeactivateProver() public {
        _registerDefaultProver(prover1);
        _registerDefaultProver(prover2);
        assertEq(registry.activeProverCount(), 2);

        vm.prank(prover1);
        vm.expectEmit(true, false, false, false);
        emit ProverDeactivated(prover1);
        registry.deactivate();

        assertFalse(registry.isActive(prover1), "prover1 should be inactive");
        assertEq(registry.activeProverCount(), 1, "only 1 prover should remain active");

        // prover2 should still be in activeProvers (swap-and-pop moves it to index 0)
        address[] memory active = registry.getActiveProvers();
        assertEq(active[0], prover2, "prover2 should remain in active list");
    }

    // ========================================================================
    // 8. testDeactivateNotRegisteredReverts
    //    Unregistered address deactivating, expect ProverNotRegistered
    // ========================================================================

    function testDeactivateNotRegisteredReverts() public {
        vm.prank(randomUser);
        vm.expectRevert(ProverRegistry.ProverNotRegistered.selector);
        registry.deactivate();
    }

    // ========================================================================
    // 9. testReactivateProver
    //    Register, deactivate, reactivate with sufficient stake, verify
    //    active=true
    // ========================================================================

    function testReactivateProver() public {
        _registerDefaultProver(prover1);

        vm.prank(prover1);
        registry.deactivate();
        assertFalse(registry.isActive(prover1));
        assertEq(registry.activeProverCount(), 0);

        vm.prank(prover1);
        vm.expectEmit(true, false, false, false);
        emit ProverReactivated(prover1);
        registry.reactivate();

        assertTrue(registry.isActive(prover1), "prover should be active again");
        assertEq(registry.activeProverCount(), 1);
    }

    // ========================================================================
    // 10. testSlash
    //     Owner slashes a prover, verify stake reduced, reputation decreased
    //     (95%), slash history recorded
    // ========================================================================

    function testSlash() public {
        uint256 stake = 500 ether;
        _registerProver(prover1, stake, "ep");

        // Expected slash: 5% of 500 ether = 25 ether
        uint256 expectedSlash = (stake * SLASH_BPS) / 10000;
        assertEq(expectedSlash, 25 ether);

        uint256 ownerBalBefore = token.balanceOf(deployer);

        // Owner can slash directly (no need for setSlasher)
        vm.expectEmit(true, false, false, true);
        emit ProverSlashed(prover1, expectedSlash, "misbehavior");
        registry.slash(prover1, "misbehavior");

        ProverRegistry.Prover memory p = registry.getProver(prover1);
        assertEq(p.stake, stake - expectedSlash, "stake should be reduced by 5%");
        assertEq(p.proofsFailed, 1, "proofsFailed should increment");
        // Reputation: 5000 * 95 / 100 = 4750
        assertEq(p.reputation, 4750, "reputation should decrease to 95% of previous");
        assertTrue(p.active, "prover should still be active (475 > 100)");

        // Slashed funds go to owner (treasury)
        assertEq(token.balanceOf(deployer), ownerBalBefore + expectedSlash, "owner receives slashed funds");
        assertEq(registry.totalStaked(), stake - expectedSlash, "totalStaked decremented");

        // Slash history recorded
        (address slashedProver, uint256 amount, string memory reason, uint256 ts) = registry.slashHistory(0);
        assertEq(slashedProver, prover1);
        assertEq(amount, expectedSlash);
        assertEq(reason, "misbehavior");
        assertGt(ts, 0);
    }

    // ========================================================================
    // 11. testSlashUnauthorizedReverts
    //     Non-slasher/non-owner can't slash, expect UnauthorizedSlasher
    // ========================================================================

    function testSlashUnauthorizedReverts() public {
        _registerDefaultProver(prover1);

        vm.prank(randomUser);
        vm.expectRevert(ProverRegistry.UnauthorizedSlasher.selector);
        registry.slash(prover1, "attempt");
    }

    // ========================================================================
    // 12. testSlashDeactivatesIfBelowMinimum
    //     Slash enough to drop below minStake, verify auto-deactivation
    // ========================================================================

    function testSlashDeactivatesIfBelowMinimum() public {
        // Register with exactly minStake = 100 ether
        _registerDefaultProver(prover1);

        // 5% of 100 = 5, leaves 95 ether which is < MIN_STAKE (100)
        registry.slash(prover1, "failed proof");

        ProverRegistry.Prover memory p = registry.getProver(prover1);
        assertEq(p.stake, 95 ether, "stake should be 95 ether after 5% slash");
        assertFalse(p.active, "prover should be auto-deactivated");
        assertEq(registry.activeProverCount(), 0, "no active provers");
    }

    // ========================================================================
    // 13. testRecordSuccess
    //     Authorized slasher records success, verify proofsSubmitted
    //     incremented, reputation increased
    // ========================================================================

    function testRecordSuccess() public {
        _registerDefaultProver(prover1);
        registry.setSlasher(slasherAddr, true);

        uint256 reward = 5 ether;

        vm.prank(slasherAddr);
        vm.expectEmit(true, false, false, true);
        emit RewardDistributed(prover1, reward);
        registry.recordSuccess(prover1, reward);

        ProverRegistry.Prover memory p = registry.getProver(prover1);
        assertEq(p.proofsSubmitted, 1, "proofsSubmitted should increment");
        assertEq(p.totalEarnings, reward, "totalEarnings should reflect reward");
        // Reputation: 5000 + 50 = 5050
        assertEq(p.reputation, 5050, "reputation should increase by 50 (0.5%)");
    }

    // ========================================================================
    // 14. testSelectProver
    //     Register multiple provers, call selectProver with different seeds,
    //     verify returns valid prover
    // ========================================================================

    function testSelectProver() public {
        _registerProver(prover1, 200 ether, "ep1");
        _registerProver(prover2, 300 ether, "ep2");
        _registerProver(prover3, 500 ether, "ep3");

        // Run selection with multiple seeds, all should return a valid active prover
        for (uint256 seed = 0; seed < 20; seed++) {
            address selected = registry.selectProver(seed);
            assertTrue(registry.isActive(selected), "Selected prover must be active");
            assertTrue(registry.isProver(selected), "Selected must be a registered prover");
        }
    }

    // ========================================================================
    // 15. testSelectProverEmpty
    //     No active provers returns address(0)
    // ========================================================================

    function testSelectProverEmpty() public view {
        address selected = registry.selectProver(42);
        assertEq(selected, address(0), "should return zero address when no active provers");
    }

    // ========================================================================
    // 16. testGetTopProvers
    //     Register 3 provers with different reputations,
    //     getTopProvers(2) returns top 2
    // ========================================================================

    function testGetTopProvers() public {
        _registerDefaultProver(prover1);
        _registerDefaultProver(prover2);
        _registerDefaultProver(prover3);

        // Boost prover3 reputation via recordSuccess (20 * 50 = +1000)
        for (uint256 i = 0; i < 20; i++) {
            registry.recordSuccess(prover3, 0);
        }
        // Boost prover2 reputation less (10 * 50 = +500)
        for (uint256 i = 0; i < 10; i++) {
            registry.recordSuccess(prover2, 0);
        }
        // prover1 stays at 5000 (default)

        // prover3: 5000 + 1000 = 6000
        // prover2: 5000 + 500  = 5500
        // prover1: 5000

        address[] memory top = registry.getTopProvers(2);
        assertEq(top.length, 2, "should return exactly 2 provers");
        assertEq(top[0], prover3, "first should be prover3 (highest reputation)");
        assertEq(top[1], prover2, "second should be prover2");
    }

    // ========================================================================
    // 17. testSetSlasher
    //     Owner can set/unset slashers
    // ========================================================================

    function testSetSlasher() public {
        assertFalse(registry.slashers(slasherAddr), "initially not a slasher");

        // Set slasher
        vm.expectEmit(true, false, false, true);
        emit SlasherUpdated(slasherAddr, true);
        registry.setSlasher(slasherAddr, true);
        assertTrue(registry.slashers(slasherAddr), "should be authorized");

        // Unset slasher
        vm.expectEmit(true, false, false, true);
        emit SlasherUpdated(slasherAddr, false);
        registry.setSlasher(slasherAddr, false);
        assertFalse(registry.slashers(slasherAddr), "should be deauthorized");

        // Non-owner cannot set slasher
        vm.prank(randomUser);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, randomUser));
        registry.setSlasher(slasherAddr, true);
    }

    // ========================================================================
    // 18. testWithdrawAfterDeactivation
    //     Deactivated prover can withdraw below minStake
    // ========================================================================

    function testWithdrawAfterDeactivation() public {
        _registerDefaultProver(prover1);

        // Deactivate
        vm.prank(prover1);
        registry.deactivate();
        assertFalse(registry.isActive(prover1));

        // Now can withdraw everything (below minStake is allowed when inactive)
        uint256 balBefore = token.balanceOf(prover1);

        vm.prank(prover1);
        registry.withdrawStake(MIN_STAKE);

        ProverRegistry.Prover memory p = registry.getProver(prover1);
        assertEq(p.stake, 0, "stake should be fully withdrawn");
        assertEq(token.balanceOf(prover1), balBefore + MIN_STAKE, "tokens returned");
        assertFalse(p.active, "prover should remain inactive");
    }

    // ========================================================================
    // ADDITIONAL COVERAGE (beyond the 18 required tests)
    // ========================================================================

    function testConstructorState() public view {
        assertEq(address(registry.stakingToken()), address(token));
        assertEq(registry.minStake(), MIN_STAKE);
        assertEq(registry.slashBasisPoints(), SLASH_BPS);
        assertEq(registry.owner(), deployer);
        assertEq(registry.totalStaked(), 0);
        assertEq(registry.activeProverCount(), 0);
    }

    function testRegisterZeroStakeReverts() public {
        vm.prank(prover1);
        vm.expectRevert(ProverRegistry.InsufficientStake.selector);
        registry.register(0, "endpoint");
    }

    function testAddStakeReactivatesDeactivatedProver() public {
        _registerDefaultProver(prover1);

        vm.prank(prover1);
        registry.deactivate();
        assertFalse(registry.isActive(prover1));

        // Add stake -- prover already has MIN_STAKE which >= minStake, so reactivation triggers
        vm.prank(prover1);
        registry.addStake(1 ether);

        assertTrue(registry.isActive(prover1));
        assertEq(registry.activeProverCount(), 1);
    }

    function testAddStakeNotRegisteredReverts() public {
        vm.prank(randomUser);
        vm.expectRevert(ProverRegistry.ProverNotRegistered.selector);
        registry.addStake(10 ether);
    }

    function testWithdrawStakeNoStakeReverts() public {
        _registerDefaultProver(prover1);

        // Deactivate and withdraw all
        vm.prank(prover1);
        registry.deactivate();
        vm.prank(prover1);
        registry.withdrawStake(MIN_STAKE);

        // Now try to withdraw again with 0 stake
        vm.prank(prover1);
        vm.expectRevert(ProverRegistry.NoStakeToWithdraw.selector);
        registry.withdrawStake(1);
    }

    function testWithdrawStakeNotRegisteredReverts() public {
        vm.prank(randomUser);
        vm.expectRevert(ProverRegistry.ProverNotRegistered.selector);
        registry.withdrawStake(1);
    }

    function testDeactivateAlreadyDeactivatedReverts() public {
        _registerDefaultProver(prover1);

        vm.prank(prover1);
        registry.deactivate();

        vm.prank(prover1);
        vm.expectRevert(ProverRegistry.ProverNotActive.selector);
        registry.deactivate();
    }

    function testReactivateAlreadyActiveNoOp() public {
        _registerDefaultProver(prover1);
        assertTrue(registry.isActive(prover1));

        // Calling reactivate when already active should be a no-op (no revert)
        vm.prank(prover1);
        registry.reactivate();
        assertTrue(registry.isActive(prover1));
        assertEq(registry.activeProverCount(), 1);
    }

    function testReactivateInsufficientStakeReverts() public {
        _registerDefaultProver(prover1);

        // Slash to drop below minimum, auto-deactivating the prover
        // 5% of 100 = 5, leaves 95 < 100
        registry.slash(prover1, "test");
        assertFalse(registry.isActive(prover1));

        // Try to reactivate with insufficient stake (95 ether < 100 ether)
        vm.prank(prover1);
        vm.expectRevert(ProverRegistry.InsufficientStake.selector);
        registry.reactivate();
    }

    function testSlashByOwnerDirectly() public {
        _registerDefaultProver(prover1);

        // Owner can slash without being a registered slasher
        registry.slash(prover1, "owner slash");

        ProverRegistry.Prover memory p = registry.getProver(prover1);
        assertEq(p.proofsFailed, 1);
    }

    function testSlashNotRegisteredReverts() public {
        registry.setSlasher(slasherAddr, true);

        vm.prank(slasherAddr);
        vm.expectRevert(ProverRegistry.ProverNotRegistered.selector);
        registry.slash(randomUser, "not registered");
    }

    function testRecordSuccessReputationCapsAt10000() public {
        _registerDefaultProver(prover1);

        // Each recordSuccess adds 50 to reputation, starting at 5000
        // Need 100 calls to reach 10000 (5000 + 100*50)
        for (uint256 i = 0; i < 105; i++) {
            registry.recordSuccess(prover1, 1 ether);
        }

        ProverRegistry.Prover memory p = registry.getProver(prover1);
        assertEq(p.reputation, 10000, "reputation should be capped at 10000");
        assertEq(p.proofsSubmitted, 105);
        assertEq(p.totalEarnings, 105 ether);
    }

    function testRecordSuccessUnauthorizedReverts() public {
        _registerDefaultProver(prover1);

        vm.prank(randomUser);
        vm.expectRevert(ProverRegistry.UnauthorizedSlasher.selector);
        registry.recordSuccess(prover1, 1 ether);
    }

    function testRecordSuccessNotRegisteredReverts() public {
        vm.expectRevert(ProverRegistry.ProverNotRegistered.selector);
        registry.recordSuccess(randomUser, 1 ether);
    }

    function testSelectProverSingleProver() public {
        _registerDefaultProver(prover1);

        address selected = registry.selectProver(42);
        assertEq(selected, prover1, "only prover should always be selected");
    }

    function testSelectProverWeightedByStake() public {
        // Give prover1 a much larger stake so it dominates selection
        _registerProver(prover1, 9000 ether, "ep1");
        _registerProver(prover2, MIN_STAKE, "ep2");

        uint256 prover1Count = 0;
        for (uint256 seed = 0; seed < 100; seed++) {
            if (registry.selectProver(seed) == prover1) {
                prover1Count++;
            }
        }
        // prover1 has ~98.9% of the weight
        assertGt(prover1Count, 85, "Higher-staked prover should be selected much more often");
    }

    function testGetTopProversRequestMoreThanAvailable() public {
        _registerDefaultProver(prover1);

        address[] memory top = registry.getTopProvers(5);
        assertEq(top.length, 1, "should return only available provers");
        assertEq(top[0], prover1);
    }

    function testGetTopProversEmpty() public view {
        address[] memory top = registry.getTopProvers(3);
        assertEq(top.length, 0, "should return empty array when no provers");
    }

    function testSetMinStake() public {
        uint256 newMin = 200 ether;
        registry.setMinStake(newMin);
        assertEq(registry.minStake(), newMin);
    }

    function testSetMinStakeNonOwnerReverts() public {
        vm.prank(randomUser);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, randomUser));
        registry.setMinStake(200 ether);
    }

    function testSetSlashBasisPoints() public {
        registry.setSlashBasisPoints(2500); // 25%
        assertEq(registry.slashBasisPoints(), 2500);
    }

    function testSetSlashBasisPointsMaxFiftyPercent() public {
        registry.setSlashBasisPoints(5000); // 50% should succeed
        vm.expectRevert("Max 50%");
        registry.setSlashBasisPoints(5001);
    }

    function testSetSlashBasisPointsNonOwnerReverts() public {
        vm.prank(randomUser);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, randomUser));
        registry.setSlashBasisPoints(2500);
    }

    function testIsProver() public {
        assertFalse(registry.isProver(prover1));
        _registerDefaultProver(prover1);
        assertTrue(registry.isProver(prover1));
    }

    function testIsActive() public {
        _registerDefaultProver(prover1);
        assertTrue(registry.isActive(prover1));

        vm.prank(prover1);
        registry.deactivate();
        assertFalse(registry.isActive(prover1));
    }

    function testGetWeight() public {
        _registerDefaultProver(prover1);

        // Weight = stake * reputation / 10000 = 100 ether * 5000 / 10000 = 50 ether
        uint256 expectedWeight = (MIN_STAKE * 5000) / 10000;
        assertEq(registry.getWeight(prover1), expectedWeight);
    }

    function testGetWeightZeroForUnregistered() public view {
        assertEq(registry.getWeight(randomUser), 0);
    }

    function testTotalStakedTracksCorrectly() public {
        _registerProver(prover1, 200 ether, "ep1");
        _registerProver(prover2, 300 ether, "ep2");
        assertEq(registry.totalStaked(), 500 ether);

        vm.prank(prover1);
        registry.addStake(50 ether);
        assertEq(registry.totalStaked(), 550 ether);

        vm.prank(prover2);
        registry.deactivate();
        vm.prank(prover2);
        registry.withdrawStake(100 ether);
        assertEq(registry.totalStaked(), 450 ether);
    }

    function testDeactivateSwapAndPopMiddleElement() public {
        // Register 3 provers: [p1, p2, p3]
        _registerDefaultProver(prover1);
        _registerDefaultProver(prover2);
        _registerDefaultProver(prover3);

        // Deactivate p1 (first element) -- should swap with p3 (last)
        vm.prank(prover1);
        registry.deactivate();

        address[] memory active = registry.getActiveProvers();
        assertEq(active.length, 2);
        // After swap-and-pop: [p3, p2]
        assertEq(active[0], prover3);
        assertEq(active[1], prover2);
    }

    function testMultipleSlashesReduceStake() public {
        _registerProver(prover1, 200 ether, "ep");
        registry.setSlasher(slasherAddr, true);

        // Slash 1: 5% of 200 = 10, remaining 190
        vm.prank(slasherAddr);
        registry.slash(prover1, "slash1");
        assertEq(registry.getProver(prover1).stake, 190 ether);
        assertTrue(registry.isActive(prover1));

        // Slash 2: 5% of 190 = 9.5, remaining 180.5
        vm.prank(slasherAddr);
        registry.slash(prover1, "slash2");
        assertEq(registry.getProver(prover1).stake, 180.5 ether);
        assertTrue(registry.isActive(prover1));

        // Slash 3: 5% of 180.5 = 9.025, remaining 171.475
        vm.prank(slasherAddr);
        registry.slash(prover1, "slash3");
        assertEq(registry.getProver(prover1).stake, 171.475 ether);
        assertTrue(registry.isActive(prover1));
    }

    function testReactivateViaAddStakeAfterSlash() public {
        _registerDefaultProver(prover1);
        registry.setSlasher(slasherAddr, true);

        // Slash to below min (5% of 100 = 5, leaves 95)
        vm.prank(slasherAddr);
        registry.slash(prover1, "bad");
        assertFalse(registry.isActive(prover1));
        assertEq(registry.getProver(prover1).stake, 95 ether);

        // Add stake to reactivate (need at least 5 more to reach minStake)
        vm.prank(prover1);
        registry.addStake(5 ether);

        assertTrue(registry.isActive(prover1));
        assertEq(registry.getProver(prover1).stake, 100 ether);
    }

    function testRegisterExactMinStake() public {
        _registerProver(prover1, MIN_STAKE, "ep");
        assertTrue(registry.isActive(prover1));
        assertEq(registry.getProver(prover1).stake, MIN_STAKE);
    }

    // ========================================================================
    // COMMIT-REVEAL PROVER SELECTION TESTS
    // ========================================================================

    // Re-declare commit-reveal events
    event SelectionRequested(uint256 indexed requestId, address indexed requester, bytes32 commitment);
    event SelectionFulfilled(uint256 indexed requestId, address indexed prover, uint256 seed);

    /// @notice Full commit-reveal flow: request -> advance 1 block -> fulfill -> get valid prover
    function testCommitRevealFullFlow() public {
        _registerProver(prover1, 200 ether, "ep1");
        _registerProver(prover2, 300 ether, "ep2");
        _registerProver(prover3, 500 ether, "ep3");

        bytes32 secret = keccak256("my random secret");
        bytes32 commitment = keccak256(abi.encodePacked(secret));

        // Step 1: Request selection (commit phase)
        vm.prank(prover1);
        vm.expectEmit(true, true, false, true);
        emit SelectionRequested(0, prover1, commitment);
        uint256 requestId = registry.requestProverSelection(commitment);
        assertEq(requestId, 0, "first request should be ID 0");

        // Verify the request was stored
        (bytes32 storedCommitment, uint256 storedBlock, address storedRequester, bool fulfilled) =
            registry.selectionRequests(requestId);
        assertEq(storedCommitment, commitment);
        assertEq(storedBlock, block.number);
        assertEq(storedRequester, prover1);
        assertFalse(fulfilled);

        // Verify nextRequestId incremented
        assertEq(registry.nextRequestId(), 1);

        // Step 2: Advance 1 block (reveal must be in a later block)
        vm.roll(block.number + 1);

        // Step 3: Fulfill (reveal phase)
        address selected = registry.fulfillProverSelection(requestId, secret);

        // Verify a valid active prover was selected
        assertTrue(registry.isActive(selected), "selected prover must be active");
        assertTrue(registry.isProver(selected), "selected must be a registered prover");

        // Verify request marked as fulfilled
        (,,, bool fulfilledAfter) = registry.selectionRequests(requestId);
        assertTrue(fulfilledAfter, "request should be fulfilled");
    }

    /// @notice Wrong secret reverts with InvalidSecret
    function testCommitRevealWrongSecretReverts() public {
        _registerDefaultProver(prover1);

        bytes32 secret = keccak256("correct secret");
        bytes32 commitment = keccak256(abi.encodePacked(secret));

        uint256 requestId = registry.requestProverSelection(commitment);
        vm.roll(block.number + 1);

        bytes32 wrongSecret = keccak256("wrong secret");
        vm.expectRevert(ProverRegistry.InvalidSecret.selector);
        registry.fulfillProverSelection(requestId, wrongSecret);
    }

    /// @notice Same-block reveal reverts with SameBlockReveal
    function testCommitRevealSameBlockReverts() public {
        _registerDefaultProver(prover1);

        bytes32 secret = keccak256("my secret");
        bytes32 commitment = keccak256(abi.encodePacked(secret));

        uint256 requestId = registry.requestProverSelection(commitment);

        // Do NOT advance block -- try to reveal in same block
        vm.expectRevert(ProverRegistry.SameBlockReveal.selector);
        registry.fulfillProverSelection(requestId, secret);
    }

    /// @notice Already fulfilled request reverts with RequestAlreadyFulfilled
    function testCommitRevealAlreadyFulfilledReverts() public {
        _registerDefaultProver(prover1);

        bytes32 secret = keccak256("my secret");
        bytes32 commitment = keccak256(abi.encodePacked(secret));

        uint256 requestId = registry.requestProverSelection(commitment);
        vm.roll(block.number + 1);

        // Fulfill once (success)
        registry.fulfillProverSelection(requestId, secret);

        // Try again (should revert)
        vm.expectRevert(ProverRegistry.RequestAlreadyFulfilled.selector);
        registry.fulfillProverSelection(requestId, secret);
    }

    /// @notice Fulfillment with no active provers reverts with NoActiveProvers
    function testCommitRevealNoActiveProversReverts() public {
        // No provers registered at all
        bytes32 secret = keccak256("my secret");
        bytes32 commitment = keccak256(abi.encodePacked(secret));

        uint256 requestId = registry.requestProverSelection(commitment);
        vm.roll(block.number + 1);

        vm.expectRevert(ProverRegistry.NoActiveProvers.selector);
        registry.fulfillProverSelection(requestId, secret);
    }

    /// @notice Fulfillment with invalid requestId reverts with RequestNotFound
    function testCommitRevealRequestNotFoundReverts() public {
        _registerDefaultProver(prover1);

        bytes32 secret = keccak256("my secret");
        vm.roll(block.number + 1);

        vm.expectRevert(ProverRegistry.RequestNotFound.selector);
        registry.fulfillProverSelection(999, secret);
    }

    /// @notice Selection distributes across provers (statistical test with multiple reveals)
    function testCommitRevealDistribution() public {
        // Register 3 provers with equal stake (selection weight is stake * reputation)
        _registerProver(prover1, 200 ether, "ep1");
        _registerProver(prover2, 200 ether, "ep2");
        _registerProver(prover3, 200 ether, "ep3");

        // Track selections
        uint256 count1 = 0;
        uint256 count2 = 0;
        uint256 count3 = 0;

        uint256 trials = 100;
        for (uint256 i = 0; i < trials; i++) {
            bytes32 secret = keccak256(abi.encodePacked("secret", i));
            bytes32 commitment = keccak256(abi.encodePacked(secret));

            uint256 requestId = registry.requestProverSelection(commitment);
            vm.roll(block.number + 1);

            address selected = registry.fulfillProverSelection(requestId, secret);

            if (selected == prover1) count1++;
            else if (selected == prover2) count2++;
            else if (selected == prover3) count3++;
        }

        // With equal weights, each prover should get roughly 33%.
        // Be generous with bounds: each should get at least 10% (10/100).
        assertGt(count1, 10, "prover1 should be selected at least 10 times out of 100");
        assertGt(count2, 10, "prover2 should be selected at least 10 times out of 100");
        assertGt(count3, 10, "prover3 should be selected at least 10 times out of 100");

        // And total should equal trials
        assertEq(count1 + count2 + count3, trials, "all selections should map to one of the three provers");
    }

    /// @notice Multiple sequential requests get incrementing IDs
    function testCommitRevealSequentialRequestIds() public {
        _registerDefaultProver(prover1);

        bytes32 commitment1 = keccak256(abi.encodePacked(keccak256("s1")));
        bytes32 commitment2 = keccak256(abi.encodePacked(keccak256("s2")));
        bytes32 commitment3 = keccak256(abi.encodePacked(keccak256("s3")));

        uint256 id1 = registry.requestProverSelection(commitment1);
        uint256 id2 = registry.requestProverSelection(commitment2);
        uint256 id3 = registry.requestProverSelection(commitment3);

        assertEq(id1, 0);
        assertEq(id2, 1);
        assertEq(id3, 2);
        assertEq(registry.nextRequestId(), 3);
    }

    /// @notice Single active prover always gets selected in commit-reveal
    function testCommitRevealSingleProver() public {
        _registerDefaultProver(prover1);

        bytes32 secret = keccak256("single prover secret");
        bytes32 commitment = keccak256(abi.encodePacked(secret));

        uint256 requestId = registry.requestProverSelection(commitment);
        vm.roll(block.number + 1);

        address selected = registry.fulfillProverSelection(requestId, secret);
        assertEq(selected, prover1, "only active prover should always be selected");
    }
}
