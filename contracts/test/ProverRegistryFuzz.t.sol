// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/ProverRegistry.sol";
import "../src/ProverReputation.sol";
import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";

/// @dev Simple ERC20 mock with public mint
contract FuzzMockToken is ERC20 {
    constructor() ERC20("FuzzMock", "FMK") {
        _mint(msg.sender, 1_000_000 ether);
    }

    function mint(address to, uint256 amount) external {
        _mint(to, amount);
    }
}

// =============================================================================
// ProverRegistry Fuzz Tests
// =============================================================================

contract ProverRegistryFuzzTest is Test {
    ProverRegistry public registry;
    FuzzMockToken public token;

    address public deployer = address(this);
    uint256 public constant MIN_STAKE = 100 ether;
    uint256 public constant SLASH_BPS = 500; // 5%

    function setUp() public {
        token = new FuzzMockToken();
        registry = new ProverRegistry(address(token), MIN_STAKE, SLASH_BPS);
    }

    // ---- Helpers ----

    /// @dev Register a prover with a given stake amount. Mints tokens and approves.
    function _registerProver(address prover, uint256 stake) internal {
        token.mint(prover, stake);
        vm.startPrank(prover);
        token.approve(address(registry), stake);
        registry.register(stake, "http://test.endpoint");
        vm.stopPrank();
    }

    // ========================================================================
    // Fuzz Test 1: Register and deregister with random addresses and stakes
    // ========================================================================

    /// @notice Fuzz register with random stake values.
    ///         Below minStake should revert. At or above should succeed.
    function testFuzz_registerAndDeregister(address prover, uint256 stake) public {
        vm.assume(prover != address(0));
        vm.assume(prover != address(this));
        vm.assume(prover != address(registry));
        vm.assume(prover != address(token));
        stake = bound(stake, 0, 10_000 ether);

        token.mint(prover, stake);
        vm.startPrank(prover);
        token.approve(address(registry), stake);

        if (stake < MIN_STAKE) {
            vm.expectRevert(ProverRegistry.InsufficientStake.selector);
            registry.register(stake, "http://test.endpoint");
        } else {
            registry.register(stake, "http://test.endpoint");

            // Verify registration state
            ProverRegistry.Prover memory p = registry.getProver(prover);
            assertEq(p.owner, prover, "owner should be the registering address");
            assertEq(p.stake, stake, "stake should match deposited amount");
            assertTrue(p.active, "prover should be active after registration");
            assertEq(p.reputation, 5000, "initial reputation should be 50%");
            assertEq(registry.totalStaked(), stake, "totalStaked should equal deposited stake");

            // Deactivate
            registry.deactivate();
            assertFalse(registry.isActive(prover), "prover should be inactive after deactivation");

            // After deactivating, can withdraw below minimum
            registry.withdrawStake(stake);
            assertEq(registry.totalStaked(), 0, "totalStaked should be zero after full withdrawal");
        }
        vm.stopPrank();
    }

    // ========================================================================
    // Fuzz Test 2: Stake and withdraw with random amounts
    // ========================================================================

    /// @notice Fuzz addStake and withdrawStake with random amounts.
    ///         Active provers cannot withdraw below minStake.
    function testFuzz_stakeAndWithdraw(uint256 addAmount) public {
        addAmount = bound(addAmount, 0, 5_000 ether);

        address prover = address(0xCAFE);
        _registerProver(prover, MIN_STAKE);

        // Add more stake
        if (addAmount > 0) {
            token.mint(prover, addAmount);
            vm.startPrank(prover);
            token.approve(address(registry), addAmount);
            registry.addStake(addAmount);
            vm.stopPrank();
        }

        uint256 totalStake = MIN_STAKE + addAmount;
        ProverRegistry.Prover memory p = registry.getProver(prover);
        assertEq(p.stake, totalStake, "stake should equal initial + added");

        // Try to withdraw everything while active -- should fail if it would go below min
        vm.startPrank(prover);
        if (totalStake > MIN_STAKE) {
            // Can withdraw down to minStake
            uint256 withdrawable = totalStake - MIN_STAKE;
            registry.withdrawStake(withdrawable);

            p = registry.getProver(prover);
            assertEq(p.stake, MIN_STAKE, "stake should be at minimum after partial withdraw");
        }

        // Trying to withdraw 1 more wei while active should fail
        if (registry.isActive(prover)) {
            vm.expectRevert(ProverRegistry.WithdrawalWouldBreachMinimum.selector);
            registry.withdrawStake(1);
        }
        vm.stopPrank();
    }

    // ========================================================================
    // Fuzz Test 3: Slash does not underflow
    // ========================================================================

    /// @notice Fuzz slashing with random slash basis points.
    ///         Ensures stake never underflows, totalStaked stays consistent,
    ///         and prover is deactivated if stake falls below minimum.
    function testFuzz_slashDoesNotUnderflow(uint256 slashBps) public {
        slashBps = bound(slashBps, 1, 5000); // 0.01% to 50% -- valid range

        // Deploy registry with the fuzzed slash basis points
        ProverRegistry fuzzRegistry = new ProverRegistry(address(token), MIN_STAKE, slashBps);

        address prover = address(0xBEEF);
        uint256 stake = MIN_STAKE;
        token.mint(prover, stake);
        vm.startPrank(prover);
        token.approve(address(fuzzRegistry), stake);
        fuzzRegistry.register(stake, "http://test.endpoint");
        vm.stopPrank();

        // Authorize deployer as slasher
        fuzzRegistry.setSlasher(deployer, true);

        uint256 expectedSlash = (stake * slashBps) / 10000;
        uint256 expectedRemaining = stake - expectedSlash;

        uint256 totalStakedBefore = fuzzRegistry.totalStaked();
        uint256 deployerBalBefore = token.balanceOf(deployer);

        // Slash
        fuzzRegistry.slash(prover, "fuzz test slash");

        ProverRegistry.Prover memory p = fuzzRegistry.getProver(prover);
        assertEq(p.stake, expectedRemaining, "stake after slash should match expected");
        assertEq(
            fuzzRegistry.totalStaked(),
            totalStakedBefore - expectedSlash,
            "totalStaked should decrease by slash amount"
        );
        assertEq(
            token.balanceOf(deployer),
            deployerBalBefore + expectedSlash,
            "slashed funds should go to owner/treasury"
        );

        // If remaining stake < minStake, prover should be deactivated
        if (expectedRemaining < MIN_STAKE) {
            assertFalse(fuzzRegistry.isActive(prover), "prover should be deactivated when stake < minStake");
        }
    }

    // ========================================================================
    // Fuzz Test 4: Multiple slashes never underflow
    // ========================================================================

    /// @notice Slash a prover multiple times and verify stake never goes negative.
    function testFuzz_multipleSlashesNeverUnderflow(uint8 slashCount) public {
        slashCount = uint8(bound(slashCount, 1, 20));

        address prover = address(0xBEEF);
        uint256 stake = 1000 ether;
        _registerProver(prover, stake);

        registry.setSlasher(deployer, true);

        for (uint256 i = 0; i < slashCount; i++) {
            ProverRegistry.Prover memory p = registry.getProver(prover);
            if (p.stake == 0) break;

            registry.slash(prover, "multi-slash test");

            p = registry.getProver(prover);
            // Stake must never underflow (would revert with arithmetic error if it did)
            assertLe(p.stake, stake, "stake should never exceed original amount");
        }

        // totalStaked must match remaining individual stake
        ProverRegistry.Prover memory finalP = registry.getProver(prover);
        assertEq(registry.totalStaked(), finalP.stake, "totalStaked should match prover's remaining stake");
    }

    // ========================================================================
    // Fuzz Test 5: Reputation never exceeds MAX_REPUTATION
    // ========================================================================

    /// @notice Record random number of successes and verify reputation caps at 10000.
    function testFuzz_reputationNeverExceedsMax(uint8 successCount) public {
        successCount = uint8(bound(successCount, 1, 200));

        address prover = address(0xBEEF);
        _registerProver(prover, MIN_STAKE);

        registry.setSlasher(deployer, true);

        for (uint256 i = 0; i < successCount; i++) {
            registry.recordSuccess(prover, 1 ether);

            ProverRegistry.Prover memory p = registry.getProver(prover);
            assertLe(p.reputation, 10000, "reputation should never exceed MAX_REPUTATION (10000)");
        }
    }

    // ========================================================================
    // Fuzz Test 6: Double registration always reverts
    // ========================================================================

    /// @notice Any address that already registered should always revert on re-registration.
    function testFuzz_doubleRegistrationReverts(address prover, uint256 stake1, uint256 stake2) public {
        vm.assume(prover != address(0));
        vm.assume(prover != address(this));
        vm.assume(prover != address(registry));
        vm.assume(prover != address(token));
        stake1 = bound(stake1, MIN_STAKE, 5_000 ether);
        stake2 = bound(stake2, MIN_STAKE, 5_000 ether);

        token.mint(prover, stake1 + stake2);
        vm.startPrank(prover);
        token.approve(address(registry), stake1 + stake2);
        registry.register(stake1, "endpoint");

        vm.expectRevert(ProverRegistry.ProverAlreadyRegistered.selector);
        registry.register(stake2, "endpoint2");
        vm.stopPrank();
    }
}

// =============================================================================
// ProverReputation Fuzz Tests
// =============================================================================

contract ProverReputationFuzzTest is Test {
    ProverReputation public rep;

    address public owner = address(this);
    address public reporter = address(0xBEEF);

    function setUp() public {
        rep = new ProverReputation();
        rep.authorizeReporter(reporter);
    }

    // ---- Helpers ----

    function _registerProver(address prover) internal {
        vm.prank(prover);
        rep.register();
    }

    // ========================================================================
    // Fuzz Test 1: Score never exceeds MAX_SCORE after successes
    // ========================================================================

    /// @notice Record random number of successes, verify score <= MAX_SCORE (10000).
    function testFuzz_scoreNeverExceedsMax(uint8 successCount) public {
        successCount = uint8(bound(successCount, 1, 250));

        address prover = address(0x1111);
        _registerProver(prover);

        for (uint256 i = 0; i < successCount; i++) {
            vm.prank(reporter);
            rep.recordSuccess(prover, 100, 0.1 ether);

            ProverReputation.Reputation memory r = rep.getReputation(prover);
            assertLe(r.score, 10000, "score should never exceed MAX_SCORE (10000)");
        }
    }

    // ========================================================================
    // Fuzz Test 2: Score never goes below 0 after failures
    // ========================================================================

    /// @notice Record random number of failures, verify score >= 0 (which it always is as uint32).
    function testFuzz_scoreNeverUnderflows(uint8 failureCount) public {
        failureCount = uint8(bound(failureCount, 1, 250));

        address prover = address(0x2222);
        _registerProver(prover);

        for (uint256 i = 0; i < failureCount; i++) {
            vm.prank(reporter);
            rep.recordFailure(prover, "fuzz failure");

            ProverReputation.Reputation memory r = rep.getReputation(prover);
            // Score is uint32, so it cannot go below 0. But verify the bound logic works.
            assertLe(r.score, 10000, "score should be within valid range");
        }

        // After many failures, score should be at or near 0
        ProverReputation.Reputation memory finalRep = rep.getReputation(prover);
        if (failureCount >= 25) {
            // INITIAL_SCORE=5000, FAILURE_PENALTY=200; 5000/200 = 25 failures to reach 0
            assertEq(finalRep.score, 0, "score should be 0 after enough failures");
        }
    }

    // ========================================================================
    // Fuzz Test 3: Banned provers stay banned until explicitly unbanned
    // ========================================================================

    /// @notice After banning, all reputation-modifying operations should revert.
    function testFuzz_bannedProverStaysBanned(uint256 actionSeed) public {
        actionSeed = bound(actionSeed, 0, 2);

        address prover = address(0x3333);
        _registerProver(prover);

        // Ban the prover
        rep.ban(prover, "test ban");

        ProverReputation.Reputation memory r = rep.getReputation(prover);
        assertTrue(r.isBanned, "prover should be banned");

        // All reputation-modifying calls should revert with ProverIsBanned
        vm.startPrank(reporter);
        if (actionSeed == 0) {
            vm.expectRevert(ProverReputation.ProverIsBanned.selector);
            rep.recordSuccess(prover, 100, 0.1 ether);
        } else if (actionSeed == 1) {
            vm.expectRevert(ProverReputation.ProverIsBanned.selector);
            rep.recordFailure(prover, "should not work");
        } else {
            vm.expectRevert(ProverReputation.ProverIsBanned.selector);
            rep.recordAbandon(prover, 1);
        }
        vm.stopPrank();

        // Unban
        rep.unban(prover);
        r = rep.getReputation(prover);
        assertFalse(r.isBanned, "prover should be unbanned after unban call");

        // Now operations should work
        vm.prank(reporter);
        rep.recordSuccess(prover, 100, 0.1 ether);
    }

    // ========================================================================
    // Fuzz Test 4: Slash with random penalty basis points
    // ========================================================================

    /// @notice Slash a prover with random penaltyBps. Auto-bans if score reaches 0.
    function testFuzz_slashWithRandomPenalty(uint256 penaltyBps) public {
        penaltyBps = bound(penaltyBps, 1, 10000);

        address prover = address(0x4444);
        _registerProver(prover);

        ProverReputation.Reputation memory before = rep.getReputation(prover);
        uint256 scoreBefore = before.score;

        rep.slash(prover, "fuzz slash", penaltyBps);

        ProverReputation.Reputation memory after_ = rep.getReputation(prover);

        // Verify score decreased correctly
        uint256 expectedPenalty = (scoreBefore * penaltyBps) / 10000;
        uint256 expectedScore = scoreBefore > expectedPenalty ? scoreBefore - expectedPenalty : 0;
        assertEq(after_.score, uint32(expectedScore), "score should decrease by correct penalty amount");

        // If score reached 0, prover should be auto-banned
        if (expectedScore == 0) {
            assertTrue(after_.isBanned, "prover should be auto-banned when score reaches 0");
        }

        assertTrue(after_.isSlashed, "prover should be marked as slashed");
    }

    // ========================================================================
    // Fuzz Test 5: Job counters are always consistent
    // ========================================================================

    /// @notice Mix successes, failures, and abandons randomly. Verify totalJobs = completed + failed + abandoned.
    function testFuzz_jobCountersConsistent(uint8 successes, uint8 failures, uint8 abandons) public {
        successes = uint8(bound(successes, 0, 30));
        failures = uint8(bound(failures, 0, 30));
        abandons = uint8(bound(abandons, 0, 30));

        address prover = address(0x5555);
        _registerProver(prover);

        for (uint256 i = 0; i < successes; i++) {
            vm.prank(reporter);
            rep.recordSuccess(prover, 100, 0.1 ether);
        }

        for (uint256 i = 0; i < failures; i++) {
            vm.prank(reporter);
            // Score may hit 0 and prover gets auto-banned on slash, but recordFailure does not auto-ban
            // Check if prover is banned before calling
            ProverReputation.Reputation memory r = rep.getReputation(prover);
            if (r.isBanned) break;
            rep.recordFailure(prover, "fuzz");
        }

        for (uint256 i = 0; i < abandons; i++) {
            ProverReputation.Reputation memory r = rep.getReputation(prover);
            if (r.isBanned) break;
            vm.prank(reporter);
            rep.recordAbandon(prover, i);
        }

        ProverReputation.Reputation memory final_ = rep.getReputation(prover);
        assertEq(
            final_.totalJobs,
            final_.completedJobs + final_.failedJobs + final_.abandonedJobs,
            "totalJobs must equal sum of completed + failed + abandoned"
        );
    }

    // ========================================================================
    // Fuzz Test 6: Unauthorized caller always reverts
    // ========================================================================

    /// @notice Any non-authorized caller should always be rejected for reputation updates.
    function testFuzz_unauthorizedCallerReverts(address caller) public {
        vm.assume(caller != owner);
        vm.assume(caller != reporter);
        vm.assume(caller != address(0));

        address prover = address(0x6666);
        _registerProver(prover);

        vm.startPrank(caller);

        vm.expectRevert(ProverReputation.NotAuthorized.selector);
        rep.recordSuccess(prover, 100, 0.1 ether);

        vm.expectRevert(ProverReputation.NotAuthorized.selector);
        rep.recordFailure(prover, "unauthorized");

        vm.expectRevert(ProverReputation.NotAuthorized.selector);
        rep.recordAbandon(prover, 1);

        vm.stopPrank();
    }

    // ========================================================================
    // Fuzz Test 7: Tier transitions are monotonic with score
    // ========================================================================

    /// @notice Verify tier assignments match score thresholds after random operations.
    ///         Note: tier is only updated after score-changing operations, not at registration.
    ///         So we require at least one operation to trigger _updateTier.
    function testFuzz_tierMatchesScore(uint8 successes, uint8 failures) public {
        successes = uint8(bound(successes, 1, 200)); // At least 1 operation to trigger tier update
        failures = uint8(bound(failures, 0, 50));

        address prover = address(0x7777);
        _registerProver(prover);

        // Apply successes
        for (uint256 i = 0; i < successes; i++) {
            vm.prank(reporter);
            rep.recordSuccess(prover, 100, 0.1 ether);
        }

        // Apply failures
        for (uint256 i = 0; i < failures; i++) {
            ProverReputation.Reputation memory r = rep.getReputation(prover);
            if (r.isBanned) break;
            vm.prank(reporter);
            rep.recordFailure(prover, "tier test");
        }

        ProverReputation.Reputation memory final_ = rep.getReputation(prover);
        uint32 score = final_.score;
        uint8 tier = final_.tier;

        // Verify tier matches score thresholds (after at least one tier-updating operation)
        if (score >= 9500) {
            assertEq(tier, uint8(ProverReputation.Tier.Diamond), "score >= 9500 should be Diamond");
        } else if (score >= 9000) {
            assertEq(tier, uint8(ProverReputation.Tier.Platinum), "score >= 9000 should be Platinum");
        } else if (score >= 7500) {
            assertEq(tier, uint8(ProverReputation.Tier.Gold), "score >= 7500 should be Gold");
        } else if (score >= 5000) {
            assertEq(tier, uint8(ProverReputation.Tier.Silver), "score >= 5000 should be Silver");
        } else if (score > 0) {
            assertEq(tier, uint8(ProverReputation.Tier.Bronze), "score > 0 should be Bronze");
        } else {
            assertEq(tier, uint8(ProverReputation.Tier.Unranked), "score == 0 should be Unranked");
        }
    }

    // ========================================================================
    // Fuzz Test 8: Double registration always reverts
    // ========================================================================

    function testFuzz_doubleRegistrationReverts(address prover) public {
        vm.assume(prover != address(0));

        vm.prank(prover);
        rep.register();

        vm.prank(prover);
        vm.expectRevert(ProverReputation.AlreadyRegistered.selector);
        rep.register();
    }
}
