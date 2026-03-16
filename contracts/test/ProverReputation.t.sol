// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/ProverReputation.sol";
import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";

contract ProverReputationTest is Test {
    // Re-declare events for expectEmit
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

    ProverReputation public rep;

    address owner = address(this);
    address reporter = address(0xBEEF);
    address prover1 = address(0x1111);
    address prover2 = address(0x2222);
    address prover3 = address(0x3333);
    address stranger = address(0xDEAD);

    function setUp() public {
        rep = new ProverReputation();
        rep.authorizeReporter(reporter);
    }

    // ========================================================================
    // 1. Initial state
    // ========================================================================

    function test_initialState_totalProversIsZero() public view {
        assertEq(rep.totalProvers(), 0, "totalProvers should start at 0");
    }

    function test_initialState_ownerIsDeployer() public view {
        assertEq(rep.owner(), owner, "owner should be deployer");
    }

    function test_initialState_unregisteredProverReturnsDefaults() public view {
        ProverReputation.Reputation memory r = rep.getReputation(prover1);
        assertEq(r.score, 0, "unregistered score should be 0");
        assertEq(r.tier, 0, "unregistered tier should be 0");
        assertFalse(r.isRegistered, "should not be registered");
        assertFalse(r.isBanned, "should not be banned");
        assertFalse(r.isSlashed, "should not be slashed");
        assertEq(r.totalJobs, 0, "totalJobs should be 0");
        assertEq(r.completedJobs, 0, "completedJobs should be 0");
        assertEq(r.failedJobs, 0, "failedJobs should be 0");
        assertEq(r.abandonedJobs, 0, "abandonedJobs should be 0");
        assertEq(r.totalEarnings, 0, "totalEarnings should be 0");
    }

    // ========================================================================
    // 2. register()
    // ========================================================================

    function test_register_success() public {
        vm.prank(prover1);
        rep.register();

        ProverReputation.Reputation memory r = rep.getReputation(prover1);
        assertTrue(r.isRegistered, "should be registered");
        assertEq(r.score, 5000, "initial score should be 5000");
        assertEq(r.tier, uint8(ProverReputation.Tier.Unranked), "initial tier should be Unranked");
        assertEq(r.totalJobs, 0, "totalJobs should be 0");
        assertEq(r.completedJobs, 0, "completedJobs should be 0");
        assertEq(r.failedJobs, 0, "failedJobs should be 0");
        assertEq(r.abandonedJobs, 0, "abandonedJobs should be 0");
        assertEq(r.totalEarnings, 0, "totalEarnings should be 0");
        assertEq(r.avgProofTimeMs, 0, "avgProofTimeMs should be 0");
        assertEq(r.lastJobAt, 0, "lastJobAt should be 0");
        assertEq(r.lastUpdateAt, uint64(block.timestamp), "lastUpdateAt should be now");
        assertFalse(r.isBanned, "should not be banned");
        assertFalse(r.isSlashed, "should not be slashed");

        assertEq(rep.totalProvers(), 1, "totalProvers should be 1");
        assertEq(rep.getProversByTier(ProverReputation.Tier.Unranked), 1, "1 Unranked prover");
    }

    function test_register_duplicateReverts() public {
        vm.prank(prover1);
        rep.register();

        vm.expectRevert(ProverReputation.AlreadyRegistered.selector);
        vm.prank(prover1);
        rep.register();
    }

    function test_register_emitsEvent() public {
        vm.expectEmit(true, false, false, true);
        emit ProverRegistered(prover1, block.timestamp);

        vm.prank(prover1);
        rep.register();
    }

    // ========================================================================
    // 3. recordSuccess -- score increases, stats update, tier promotion,
    //    fast proof bonus
    // ========================================================================

    function test_recordSuccess_scoreIncreasesAndStatsUpdate() public {
        _registerProver(prover1);

        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0.01 ether);

        ProverReputation.Reputation memory r = rep.getReputation(prover1);
        // First call: avgProofTimeMs was 0 so set to 1000.
        // proofTimeMs (1000) is NOT < avgProofTimeMs (1000), so no fast bonus.
        // Score = 5000 + 50 = 5050
        assertEq(r.score, 5050, "score should be 5050");
        assertEq(r.totalJobs, 1, "totalJobs should be 1");
        assertEq(r.completedJobs, 1, "completedJobs should be 1");
        assertEq(r.totalEarnings, 0.01 ether, "earnings tracked");
        assertEq(r.avgProofTimeMs, 1000, "avgProofTimeMs should be 1000");
        assertEq(r.lastJobAt, uint64(block.timestamp), "lastJobAt updated");
    }

    function test_recordSuccess_tierPromotion() public {
        _registerProver(prover1);

        // Initial: score=5000, tier=Unranked. After success: score=5050.
        // _calculateTier(5050) = Silver. oldTier=Unranked != Silver => tier changes.
        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0);

        assertEq(uint8(rep.getTier(prover1)), uint8(ProverReputation.Tier.Silver), "should promote to Silver");
        assertEq(rep.getProversByTier(ProverReputation.Tier.Unranked), 0, "0 Unranked");
        assertEq(rep.getProversByTier(ProverReputation.Tier.Silver), 1, "1 Silver");
    }

    function test_recordSuccess_fastProofBonus() public {
        _registerProver(prover1);

        // First success: sets avgProofTimeMs to 1000, score = 5050
        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0);

        // Second success: proofTimeMs=500, avg was 1000 => 500 < 1000 => fast bonus
        // New avg = (1000*9 + 500) / 10 = 950
        // Score = 5050 + 50 + 25 = 5125
        vm.prank(reporter);
        rep.recordSuccess(prover1, 500, 0);

        ProverReputation.Reputation memory r = rep.getReputation(prover1);
        assertEq(r.score, 5125, "score should include FAST_PROOF_BONUS");
        assertEq(r.avgProofTimeMs, 950, "avg proof time rolling");
    }

    function test_recordSuccess_unregisteredReverts() public {
        vm.expectRevert(ProverReputation.ProverNotRegistered.selector);
        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0);
    }

    // ========================================================================
    // 4. recordFailure -- score decreases, stats update, tier demotion
    // ========================================================================

    function test_recordFailure_scorePenaltyAndStats() public {
        _registerProver(prover1);

        vm.prank(reporter);
        rep.recordFailure(prover1, "timeout");

        ProverReputation.Reputation memory r = rep.getReputation(prover1);
        // 5000 - 200 = 4800
        assertEq(r.score, 4800, "score should be 4800");
        assertEq(r.totalJobs, 1, "totalJobs should be 1");
        assertEq(r.failedJobs, 1, "failedJobs should be 1");
        assertEq(r.completedJobs, 0, "completedJobs should be 0");
    }

    function test_recordFailure_tierDemotion() public {
        _registerProver(prover1);

        // Push to Silver first
        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0);
        assertEq(uint8(rep.getTier(prover1)), uint8(ProverReputation.Tier.Silver));

        // Failure: 5050 - 200 = 4850 => Bronze
        vm.prank(reporter);
        rep.recordFailure(prover1, "bad proof");

        assertEq(uint8(rep.getTier(prover1)), uint8(ProverReputation.Tier.Bronze), "should demote to Bronze");
        assertEq(rep.getProversByTier(ProverReputation.Tier.Silver), 0, "0 Silver");
        assertEq(rep.getProversByTier(ProverReputation.Tier.Bronze), 1, "1 Bronze");
    }

    function test_recordFailure_scoreFloorAtZero() public {
        _registerProver(prover1);

        // Slash to bring score very low
        rep.slash(prover1, "bad", 9900); // penalty = 5000*9900/10000 = 4950 => score 50

        // Failure penalty 200 > 50, should go to 0 not underflow
        vm.prank(reporter);
        rep.recordFailure(prover1, "again");

        assertEq(rep.getReputation(prover1).score, 0, "score should floor at 0");
    }

    function test_recordFailure_unregisteredReverts() public {
        vm.expectRevert(ProverReputation.ProverNotRegistered.selector);
        vm.prank(reporter);
        rep.recordFailure(prover1, "fail");
    }

    // ========================================================================
    // 5. recordAbandon -- heavy penalty, stats update
    // ========================================================================

    function test_recordAbandon_heavyPenalty() public {
        _registerProver(prover1);

        vm.prank(reporter);
        rep.recordAbandon(prover1, 42);

        ProverReputation.Reputation memory r = rep.getReputation(prover1);
        // 5000 - 500 = 4500
        assertEq(r.score, 4500, "score should be 4500");
        assertEq(r.totalJobs, 1, "totalJobs should be 1");
        assertEq(r.abandonedJobs, 1, "abandonedJobs should be 1");
    }

    function test_recordAbandon_scoreFloorAtZero() public {
        _registerProver(prover1);

        // Slash down close to zero
        rep.slash(prover1, "bad", 9500); // penalty = 5000*9500/10000 = 4750 => score 250

        vm.prank(reporter);
        rep.recordAbandon(prover1, 99);

        assertEq(rep.getReputation(prover1).score, 0, "score should floor at 0");
    }

    function test_recordAbandon_unregisteredReverts() public {
        vm.expectRevert(ProverReputation.ProverNotRegistered.selector);
        vm.prank(reporter);
        rep.recordAbandon(prover1, 1);
    }

    // ========================================================================
    // 6. Access control -- recordSuccess/recordFailure/recordAbandon revert
    //    for unauthorized
    // ========================================================================

    function test_accessControl_recordSuccess_unauthorizedReverts() public {
        _registerProver(prover1);

        vm.expectRevert(ProverReputation.NotAuthorized.selector);
        vm.prank(stranger);
        rep.recordSuccess(prover1, 1000, 0);
    }

    function test_accessControl_recordFailure_unauthorizedReverts() public {
        _registerProver(prover1);

        vm.expectRevert(ProverReputation.NotAuthorized.selector);
        vm.prank(stranger);
        rep.recordFailure(prover1, "fail");
    }

    function test_accessControl_recordAbandon_unauthorizedReverts() public {
        _registerProver(prover1);

        vm.expectRevert(ProverReputation.NotAuthorized.selector);
        vm.prank(stranger);
        rep.recordAbandon(prover1, 1);
    }

    function test_accessControl_ownerCanCallRecordFunctions() public {
        _registerProver(prover1);

        // Owner is implicitly authorized per the onlyAuthorized modifier
        rep.recordSuccess(prover1, 1000, 0);
        assertEq(rep.getReputation(prover1).completedJobs, 1, "owner can recordSuccess");

        rep.recordFailure(prover1, "test");
        assertEq(rep.getReputation(prover1).failedJobs, 1, "owner can recordFailure");

        rep.recordAbandon(prover1, 1);
        assertEq(rep.getReputation(prover1).abandonedJobs, 1, "owner can recordAbandon");
    }

    // ========================================================================
    // 7. Access control -- slash/ban/unban revert for non-owner
    // ========================================================================

    function test_accessControl_slash_nonOwnerReverts() public {
        _registerProver(prover1);

        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, stranger));
        vm.prank(stranger);
        rep.slash(prover1, "cheat", 1000);
    }

    function test_accessControl_ban_nonOwnerReverts() public {
        _registerProver(prover1);

        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, stranger));
        vm.prank(stranger);
        rep.ban(prover1, "bad actor");
    }

    function test_accessControl_unban_nonOwnerReverts() public {
        _registerProver(prover1);
        rep.ban(prover1, "bad");

        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, stranger));
        vm.prank(stranger);
        rep.unban(prover1);
    }

    // ========================================================================
    // 8. authorizeReporter / revokeReporter
    // ========================================================================

    function test_authorizeReporter_allowsRecording() public {
        address newReporter = address(0xCAFE);
        _registerProver(prover1);

        // Not yet authorized
        vm.expectRevert(ProverReputation.NotAuthorized.selector);
        vm.prank(newReporter);
        rep.recordSuccess(prover1, 1000, 0);

        // Authorize
        rep.authorizeReporter(newReporter);
        assertTrue(rep.authorizedReporters(newReporter), "reporter should be authorized");

        // Now it works
        vm.prank(newReporter);
        rep.recordSuccess(prover1, 1000, 0);
        assertEq(rep.getReputation(prover1).completedJobs, 1, "recording should succeed");
    }

    function test_revokeReporter_blocksRecording() public {
        _registerProver(prover1);

        // Reporter is authorized in setUp. Revoke it.
        rep.revokeReporter(reporter);
        assertFalse(rep.authorizedReporters(reporter), "reporter should be revoked");

        vm.expectRevert(ProverReputation.NotAuthorized.selector);
        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0);
    }

    function test_authorizeReporter_onlyOwner() public {
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, stranger));
        vm.prank(stranger);
        rep.authorizeReporter(stranger);
    }

    function test_revokeReporter_onlyOwner() public {
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, stranger));
        vm.prank(stranger);
        rep.revokeReporter(reporter);
    }

    // ========================================================================
    // 9. slash -- proportional penalty, auto-ban at score 0, slash history
    // ========================================================================

    function test_slash_proportionalPenalty() public {
        _registerProver(prover1);

        // Slash 50% (5000 bps): penalty = 5000 * 5000/10000 = 2500 => score 2500
        rep.slash(prover1, "cheating", 5000);

        ProverReputation.Reputation memory r = rep.getReputation(prover1);
        assertEq(r.score, 2500, "score should be 2500 after 50% slash");
        assertTrue(r.isSlashed, "isSlashed should be true");
    }

    function test_slash_autoBanAtScoreZero() public {
        _registerProver(prover1);

        // Slash 100%
        rep.slash(prover1, "total slash", 10000);

        ProverReputation.Reputation memory r = rep.getReputation(prover1);
        assertEq(r.score, 0, "score should be 0");
        assertTrue(r.isBanned, "should be auto-banned at score 0");
    }

    function test_slash_historyRecorded() public {
        _registerProver(prover1);

        rep.slash(prover1, "offense 1", 1000);
        vm.warp(block.timestamp + 1 hours + 1); // respect cooldown
        rep.slash(prover1, "offense 2", 2000);

        ProverReputation.SlashEvent[] memory history = rep.getSlashHistory(prover1);
        assertEq(history.length, 2, "should have 2 slash events");
        assertEq(history[0].penaltyBps, 1000, "first slash penaltyBps");
        assertEq(keccak256(bytes(history[0].reason)), keccak256("offense 1"), "first slash reason");
        assertEq(history[0].reportedBy, owner, "first slash reportedBy");
        assertEq(history[1].penaltyBps, 2000, "second slash penaltyBps");
        assertEq(keccak256(bytes(history[1].reason)), keccak256("offense 2"), "second slash reason");
    }

    function test_slash_multipleSlashesStackPenalties() public {
        _registerProver(prover1);

        // First slash: 20% of 5000 = 1000 penalty => 4000
        rep.slash(prover1, "first", 2000);
        assertEq(rep.getReputation(prover1).score, 4000, "score after first slash");

        vm.warp(block.timestamp + 1 hours + 1); // respect cooldown

        // Second slash: 50% of 4000 = 2000 penalty => 2000
        rep.slash(prover1, "second", 5000);
        assertEq(rep.getReputation(prover1).score, 2000, "score after second slash");
    }

    function test_slash_unregisteredReverts() public {
        vm.expectRevert(ProverReputation.ProverNotRegistered.selector);
        rep.slash(prover1, "test", 1000);
    }

    // ========================================================================
    // 10. ban / unban -- flags set correctly, banned prover can't have records
    // ========================================================================

    function test_ban_setsFlagCorrectly() public {
        _registerProver(prover1);
        rep.ban(prover1, "malicious");

        assertTrue(rep.getReputation(prover1).isBanned, "should be banned");
    }

    function test_unban_clearsFlagCorrectly() public {
        _registerProver(prover1);
        rep.ban(prover1, "malicious");
        rep.unban(prover1);

        assertFalse(rep.getReputation(prover1).isBanned, "should be unbanned");
    }

    function test_bannedProver_cannotHaveRecords() public {
        _registerProver(prover1);
        rep.ban(prover1, "bad");

        vm.expectRevert(ProverReputation.ProverIsBanned.selector);
        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0);

        vm.expectRevert(ProverReputation.ProverIsBanned.selector);
        vm.prank(reporter);
        rep.recordFailure(prover1, "test");

        vm.expectRevert(ProverReputation.ProverIsBanned.selector);
        vm.prank(reporter);
        rep.recordAbandon(prover1, 1);
    }

    function test_unbannedProver_canRecordAgain() public {
        _registerProver(prover1);
        rep.ban(prover1, "temp ban");

        vm.expectRevert(ProverReputation.ProverIsBanned.selector);
        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0);

        rep.unban(prover1);

        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0);
        assertEq(rep.getReputation(prover1).completedJobs, 1, "should record after unban");
    }

    function test_ban_unregisteredReverts() public {
        vm.expectRevert(ProverReputation.ProverNotRegistered.selector);
        rep.ban(prover1, "test");
    }

    function test_unban_unregisteredReverts() public {
        vm.expectRevert(ProverReputation.ProverNotRegistered.selector);
        rep.unban(prover1);
    }

    // ========================================================================
    // 11. getScore with decay -- warp time past DECAY_PERIOD
    // ========================================================================

    function test_getScore_noDecayWithinPeriod() public {
        _registerProver(prover1);

        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0); // score = 5050

        vm.warp(block.timestamp + 29 days);
        assertEq(rep.getScore(prover1), 5050, "no decay within period");
    }

    function test_getScore_decayAfterOnePeriod() public {
        _registerProver(prover1);

        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0); // score = 5050

        // Warp 30 days (exactly one DECAY_PERIOD)
        vm.warp(block.timestamp + 30 days);

        // One decay period: decay = 5050 * 100/10000 = 50 => 5050 - 50 = 5000
        assertEq(rep.getScore(prover1), 5000, "one period of 1% decay");
    }

    function test_getScore_decayAfterMultiplePeriods() public {
        _registerProver(prover1);

        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0); // score = 5050

        // Warp 91 days (3 full periods)
        vm.warp(block.timestamp + 91 days);

        // Period 1: 5050 - 50 = 5000
        // Period 2: 5000 - 50 = 4950
        // Period 3: 4950 - 49 = 4901
        assertEq(rep.getScore(prover1), 4901, "three periods of decay");
    }

    function test_getScore_noDecayIfNoJobDone() public {
        _registerProver(prover1);

        // lastJobAt is 0 since no job was recorded => no decay applied
        vm.warp(block.timestamp + 365 days);
        assertEq(rep.getScore(prover1), 5000, "raw score returned when lastJobAt is 0");
    }

    function test_getScore_decayIsCapped() public {
        _registerProver(prover1);

        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0);

        // Warp 20 years (way beyond MAX_DECAY_PERIOD of 365 days)
        vm.warp(block.timestamp + 7300 days);

        // Should not revert or run out of gas -- elapsed time clamped to MAX_DECAY_PERIOD
        // 365 days / 30-day period = 12 periods => 5050 * 0.99^12 = 4480
        uint256 score = rep.getScore(prover1);
        assertEq(score, 4480, "score clamped to MAX_DECAY_PERIOD (12 periods)");
    }

    // ========================================================================
    // 12. getTier -- correct tier thresholds
    // ========================================================================

    function test_getTier_allThresholds() public {
        _registerProver(prover1);

        // Initial: Unranked (score 5000, but tier set to Unranked at registration)
        assertEq(uint8(rep.getTier(prover1)), uint8(ProverReputation.Tier.Unranked), "starts Unranked");

        // One success: score=5050 => Silver
        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0);
        assertEq(uint8(rep.getTier(prover1)), uint8(ProverReputation.Tier.Silver), "Silver after first success");

        // Pump to Gold (>= 7500). Currently 5050. Need (7500-5050)/50 = 49 more successes.
        for (uint256 i = 0; i < 49; i++) {
            vm.prank(reporter);
            rep.recordSuccess(prover1, 1000, 0);
        }
        assertEq(rep.getReputation(prover1).score, 7500, "score should be 7500");
        assertEq(uint8(rep.getTier(prover1)), uint8(ProverReputation.Tier.Gold), "Gold at 7500");

        // Pump to Platinum (>= 9000). Need (9000-7500)/50 = 30 more.
        for (uint256 i = 0; i < 30; i++) {
            vm.prank(reporter);
            rep.recordSuccess(prover1, 1000, 0);
        }
        assertEq(rep.getReputation(prover1).score, 9000, "score should be 9000");
        assertEq(uint8(rep.getTier(prover1)), uint8(ProverReputation.Tier.Platinum), "Platinum at 9000");

        // Pump to Diamond (>= 9500). Need (9500-9000)/50 = 10 more.
        for (uint256 i = 0; i < 10; i++) {
            vm.prank(reporter);
            rep.recordSuccess(prover1, 1000, 0);
        }
        assertEq(rep.getReputation(prover1).score, 9500, "score should be 9500");
        assertEq(uint8(rep.getTier(prover1)), uint8(ProverReputation.Tier.Diamond), "Diamond at 9500");
    }

    function test_getTier_bronzeWhenScoreBelow5000() public {
        _registerProver(prover1);

        // Fail once: 5000 - 200 = 4800 => Bronze
        vm.prank(reporter);
        rep.recordFailure(prover1, "fail");

        assertEq(uint8(rep.getTier(prover1)), uint8(ProverReputation.Tier.Bronze), "Bronze when score < 5000");
    }

    function test_getTier_unrankedWhenScoreZero() public {
        _registerProver(prover1);

        // Slash to 0
        rep.slash(prover1, "gone", 10000);

        assertEq(uint8(rep.getTier(prover1)), uint8(ProverReputation.Tier.Unranked), "Unranked at score 0");
    }

    function test_getTier_demotionFromDiamond() public {
        _registerProver(prover1);

        // Pump to Diamond: score 5000 + 100*50 = 10000
        for (uint256 i = 0; i < 100; i++) {
            vm.prank(reporter);
            rep.recordSuccess(prover1, 1000, 0);
        }
        assertEq(uint8(rep.getTier(prover1)), uint8(ProverReputation.Tier.Diamond));

        // Drop below 9500: 23 failures => 10000 - 23*200 = 5400 => Silver
        for (uint256 i = 0; i < 23; i++) {
            vm.prank(reporter);
            rep.recordFailure(prover1, "f");
        }
        assertEq(rep.getReputation(prover1).score, 5400, "score 5400");
        assertEq(uint8(rep.getTier(prover1)), uint8(ProverReputation.Tier.Silver), "demoted to Silver");
    }

    // ========================================================================
    // 13. getSuccessRate -- 0 for no jobs, correct rate after mixed results
    // ========================================================================

    function test_getSuccessRate_zeroForNoJobs() public {
        _registerProver(prover1);
        assertEq(rep.getSuccessRate(prover1), 0, "0 for no jobs");
    }

    function test_getSuccessRate_100Percent() public {
        _registerProver(prover1);

        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0);

        assertEq(rep.getSuccessRate(prover1), 10000, "100% = 10000 bps");
    }

    function test_getSuccessRate_correctAfterMixedResults() public {
        _registerProver(prover1);

        // 3 successes
        for (uint256 i = 0; i < 3; i++) {
            vm.prank(reporter);
            rep.recordSuccess(prover1, 1000, 0);
        }

        // 1 failure
        vm.prank(reporter);
        rep.recordFailure(prover1, "fail");

        // 1 abandon
        vm.prank(reporter);
        rep.recordAbandon(prover1, 99);

        // 3/5 = 60% = 6000 bps
        assertEq(rep.getSuccessRate(prover1), 6000, "60% = 6000 bps");
    }

    function test_getSuccessRate_truncation() public {
        _registerProver(prover1);

        // 1 success + 1 failure + 1 abandon = 1/3 = 3333 bps (truncated)
        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0);
        vm.prank(reporter);
        rep.recordFailure(prover1, "fail");
        vm.prank(reporter);
        rep.recordAbandon(prover1, 1);

        assertEq(rep.getSuccessRate(prover1), 3333, "1/3 truncates to 3333 bps");
    }

    // ========================================================================
    // 14. isGoodStanding -- registered, not banned, score >= 5000
    // ========================================================================

    function test_isGoodStanding_registeredNotBannedHighScore() public {
        _registerProver(prover1);

        // Score 5000 == SILVER_THRESHOLD => good standing
        assertTrue(rep.isGoodStanding(prover1), "good standing at 5000");
    }

    function test_isGoodStanding_falseIfUnregistered() public view {
        assertFalse(rep.isGoodStanding(prover1), "unregistered not in good standing");
    }

    function test_isGoodStanding_falseIfBanned() public {
        _registerProver(prover1);
        rep.ban(prover1, "bad");

        assertFalse(rep.isGoodStanding(prover1), "banned not in good standing");
    }

    function test_isGoodStanding_falseIfScoreBelow5000() public {
        _registerProver(prover1);

        vm.prank(reporter);
        rep.recordFailure(prover1, "fail"); // score = 4800

        assertFalse(rep.isGoodStanding(prover1), "low score not in good standing");
    }

    function test_isGoodStanding_trueAtExactly5000() public {
        _registerProver(prover1);

        // Score is exactly 5000 initially, which is == SILVER_THRESHOLD
        assertTrue(rep.isGoodStanding(prover1), "exactly 5000 is good standing");
    }

    // ========================================================================
    // 15. Multiple provers -- independent tracking
    // ========================================================================

    function test_multipleProvers_independentTracking() public {
        _registerProver(prover1);
        _registerProver(prover2);

        assertEq(rep.totalProvers(), 2, "two provers registered");

        // Success for prover1
        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 1 ether);

        // Failure for prover2
        vm.prank(reporter);
        rep.recordFailure(prover2, "timeout");

        ProverReputation.Reputation memory r1 = rep.getReputation(prover1);
        ProverReputation.Reputation memory r2 = rep.getReputation(prover2);

        assertEq(r1.completedJobs, 1, "prover1 completed 1");
        assertEq(r1.failedJobs, 0, "prover1 failed 0");
        assertEq(r1.score, 5050, "prover1 score 5050");
        assertEq(r1.totalEarnings, 1 ether, "prover1 earnings");

        assertEq(r2.completedJobs, 0, "prover2 completed 0");
        assertEq(r2.failedJobs, 1, "prover2 failed 1");
        assertEq(r2.score, 4800, "prover2 score 4800");
        assertEq(r2.totalEarnings, 0, "prover2 no earnings");
    }

    function test_multipleProvers_slashOneDoesNotAffectOther() public {
        _registerProver(prover1);
        _registerProver(prover2);

        rep.slash(prover1, "bad", 5000);

        assertEq(rep.getReputation(prover1).score, 2500, "prover1 slashed");
        assertEq(rep.getReputation(prover2).score, 5000, "prover2 unaffected");
    }

    // ========================================================================
    // 16. Score capping -- cannot exceed MAX_SCORE (10000)
    // ========================================================================

    function test_scoreCapping_cannotExceedMaxScore() public {
        _registerProver(prover1);

        // 100 successes at +50 each: 5000 + 5000 = 10000
        for (uint256 i = 0; i < 100; i++) {
            vm.prank(reporter);
            rep.recordSuccess(prover1, 1000, 0);
        }
        assertEq(rep.getReputation(prover1).score, 10000, "capped at 10000");

        // One more success should still be 10000
        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0);
        assertEq(rep.getReputation(prover1).score, 10000, "still capped at 10000");
    }

    // ========================================================================
    // 17. Tier counter updates -- proversByTier tracks correctly
    // ========================================================================

    function test_tierCounters_trackCorrectly() public {
        _registerProver(prover1);
        _registerProver(prover2);
        _registerProver(prover3);

        // All start as Unranked
        assertEq(rep.getProversByTier(ProverReputation.Tier.Unranked), 3, "3 Unranked");
        assertEq(rep.getProversByTier(ProverReputation.Tier.Silver), 0, "0 Silver");
        assertEq(rep.getProversByTier(ProverReputation.Tier.Bronze), 0, "0 Bronze");

        // Move prover1 to Silver (one success)
        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0);
        assertEq(rep.getProversByTier(ProverReputation.Tier.Unranked), 2, "2 Unranked");
        assertEq(rep.getProversByTier(ProverReputation.Tier.Silver), 1, "1 Silver");

        // Move prover2 to Bronze (one failure)
        vm.prank(reporter);
        rep.recordFailure(prover2, "fail");
        assertEq(rep.getProversByTier(ProverReputation.Tier.Unranked), 1, "1 Unranked");
        assertEq(rep.getProversByTier(ProverReputation.Tier.Bronze), 1, "1 Bronze");
        assertEq(rep.getProversByTier(ProverReputation.Tier.Silver), 1, "1 Silver still");

        // Move prover3 to Silver (one success)
        vm.prank(reporter);
        rep.recordSuccess(prover3, 1000, 0);
        assertEq(rep.getProversByTier(ProverReputation.Tier.Unranked), 0, "0 Unranked");
        assertEq(rep.getProversByTier(ProverReputation.Tier.Silver), 2, "2 Silver");
        assertEq(rep.getProversByTier(ProverReputation.Tier.Bronze), 1, "1 Bronze");
    }

    // ========================================================================
    // Additional edge case and event tests
    // ========================================================================

    function test_events_jobCompleted() public {
        _registerProver(prover1);

        vm.expectEmit(true, false, false, true);
        emit JobCompleted(prover1, 1000, 5050);

        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0);
    }

    function test_events_jobFailed() public {
        _registerProver(prover1);

        vm.expectEmit(true, false, false, true);
        emit JobFailed(prover1, "timeout", 4800);

        vm.prank(reporter);
        rep.recordFailure(prover1, "timeout");
    }

    function test_events_jobAbandoned() public {
        _registerProver(prover1);

        vm.expectEmit(true, false, false, true);
        emit JobAbandoned(prover1, 42, 4500);

        vm.prank(reporter);
        rep.recordAbandon(prover1, 42);
    }

    function test_events_tierChanged() public {
        _registerProver(prover1);

        vm.expectEmit(true, false, false, true);
        emit TierChanged(prover1, uint8(ProverReputation.Tier.Unranked), uint8(ProverReputation.Tier.Silver));

        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0);
    }

    function test_events_proverSlashed() public {
        _registerProver(prover1);

        vm.expectEmit(true, false, false, true);
        emit ProverSlashed(prover1, "cheat", 3000);

        rep.slash(prover1, "cheat", 3000);
    }

    function test_events_proverBanned() public {
        _registerProver(prover1);

        vm.expectEmit(true, false, false, true);
        emit ProverBanned(prover1, "bad");

        rep.ban(prover1, "bad");
    }

    function test_events_proverUnbanned() public {
        _registerProver(prover1);
        rep.ban(prover1, "bad");

        vm.expectEmit(true, false, false, false);
        emit ProverUnbanned(prover1);

        rep.unban(prover1);
    }

    function test_events_reporterAuthorized() public {
        vm.expectEmit(true, false, false, false);
        emit ReporterAuthorized(stranger);

        rep.authorizeReporter(stranger);
    }

    function test_events_reporterRevoked() public {
        vm.expectEmit(true, false, false, false);
        emit ReporterRevoked(reporter);

        rep.revokeReporter(reporter);
    }

    function test_transferOwnership_twoStep() public {
        address newOwner = address(0xABCD);
        // Step 1: Current owner initiates transfer (sets pendingOwner)
        rep.transferOwnership(newOwner);
        assertEq(rep.owner(), owner, "owner unchanged until accepted");
        assertEq(rep.pendingOwner(), newOwner, "pendingOwner should be set");

        // Step 2: New owner accepts
        vm.prank(newOwner);
        rep.acceptOwnership();
        assertEq(rep.owner(), newOwner, "ownership transferred after acceptance");

        // Old owner can no longer call onlyOwner functions
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, address(this)));
        rep.authorizeReporter(stranger);
    }

    function test_acceptOwnership_wrongCallerReverts() public {
        address newOwner = address(0xABCD);
        rep.transferOwnership(newOwner);

        // Stranger (not the pending owner) cannot accept
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, stranger));
        vm.prank(stranger);
        rep.acceptOwnership();
    }

    function test_transferOwnership_nonOwnerReverts() public {
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, stranger));
        vm.prank(stranger);
        rep.transferOwnership(stranger);
    }

    function test_avgProofTime_rollingAverage() public {
        _registerProver(prover1);

        // First: avg = 1000
        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0);
        assertEq(rep.getReputation(prover1).avgProofTimeMs, 1000, "first avg");

        // Second: (1000*9 + 2000) / 10 = 1100
        vm.prank(reporter);
        rep.recordSuccess(prover1, 2000, 0);
        assertEq(rep.getReputation(prover1).avgProofTimeMs, 1100, "second avg");

        // Third: (1100*9 + 500) / 10 = 1040
        vm.prank(reporter);
        rep.recordSuccess(prover1, 500, 0);
        assertEq(rep.getReputation(prover1).avgProofTimeMs, 1040, "third avg");
    }

    function test_scoreFloor_repeatedFailures() public {
        _registerProver(prover1);

        // 25 failures: 5000 - 25*200 = 0
        // 26th should not underflow
        for (uint256 i = 0; i < 26; i++) {
            vm.prank(reporter);
            rep.recordFailure(prover1, "fail");
        }
        assertEq(rep.getReputation(prover1).score, 0, "floored at 0 after 26 failures");
    }

    function test_slashHistory_emptyForCleanProver() public {
        _registerProver(prover1);

        ProverReputation.SlashEvent[] memory history = rep.getSlashHistory(prover1);
        assertEq(history.length, 0, "no slash history initially");
    }

    function test_getSlashHistory_unregisteredReturnsEmpty() public view {
        ProverReputation.SlashEvent[] memory history = rep.getSlashHistory(stranger);
        assertEq(history.length, 0, "no slash history for unregistered");
    }

    // ========================================================================
    // Decay timestamp safety and MAX_DECAY_PERIOD clamping
    // ========================================================================

    /// @dev Just registered + one success, no time elapsed. Decay should be zero.
    function test_decay_zero_time() public {
        _registerProver(prover1);

        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0); // score = 5050

        // No time passes -- query score in the same block
        uint256 score = rep.getScore(prover1);
        assertEq(score, 5050, "no decay when zero time has elapsed");
    }

    /// @dev 1 day elapsed (< DECAY_PERIOD of 30 days). No decay period completes.
    function test_decay_one_day() public {
        _registerProver(prover1);

        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0); // score = 5050

        vm.warp(block.timestamp + 1 days);

        uint256 score = rep.getScore(prover1);
        assertEq(score, 5050, "no decay after only 1 day (within 30-day period)");
    }

    /// @dev Exactly MAX_DECAY_PERIOD (365 days) elapsed. Should produce 12 full
    /// decay periods (365 / 30 = 12 with remainder 5 days).
    /// 5050 * 0.99^12 = 4480 (integer arithmetic).
    function test_decay_max_period() public {
        _registerProver(prover1);

        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0); // score = 5050

        vm.warp(block.timestamp + 365 days);

        // 12 periods of 1% multiplicative decay on 5050:
        // P1:  5050-50=5000  P2:  5000-50=4950  P3:  4950-49=4901
        // P4:  4901-49=4852  P5:  4852-48=4804  P6:  4804-48=4756
        // P7:  4756-47=4709  P8:  4709-47=4662  P9:  4662-46=4616
        // P10: 4616-46=4570  P11: 4570-45=4525  P12: 4525-45=4480
        uint256 score = rep.getScore(prover1);
        assertEq(score, 4480, "12 periods of decay at exactly MAX_DECAY_PERIOD");
    }

    /// @dev Far beyond MAX_DECAY_PERIOD (e.g. 10 years). The elapsed time should be
    /// clamped to MAX_DECAY_PERIOD, so the result must equal the 365-day case (4480).
    function test_decay_beyond_max() public {
        _registerProver(prover1);

        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0); // score = 5050

        // Warp 10 years into the future
        vm.warp(block.timestamp + 3650 days);

        // Clamped to MAX_DECAY_PERIOD = 365 days = 12 periods => same result as test_decay_max_period
        uint256 score = rep.getScore(prover1);
        assertEq(score, 4480, "decay clamped to MAX_DECAY_PERIOD even after 10 years");
    }

    /// @dev The InvalidTimestamp revert fires when block.timestamp < lastJobAt.
    /// We simulate this by warping backward after recording a job.
    function test_decay_reverts_on_invalid_timestamp() public {
        _registerProver(prover1);

        // Record a success at current timestamp
        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0);

        // Warp backward so block.timestamp < lastJobAt
        vm.warp(block.timestamp - 1);

        vm.expectRevert(ProverReputation.InvalidTimestamp.selector);
        rep.getScore(prover1);
    }

    // ========================================================================
    // T468: TIMESTAMP OVERFLOW AND COOLDOWN TESTS
    // ========================================================================

    event SlashCooldownUpdated(uint256 oldCooldown, uint256 newCooldown);

    function test_timestampOverflow_registerReverts() public {
        vm.warp(uint256(type(uint64).max) + 1);

        vm.expectRevert(ProverReputation.TimestampOverflow.selector);
        vm.prank(prover1);
        rep.register();
    }

    function test_timestampOverflow_recordSuccessReverts() public {
        _registerProver(prover1);

        vm.warp(uint256(type(uint64).max) + 1);

        vm.expectRevert(ProverReputation.TimestampOverflow.selector);
        vm.prank(reporter);
        rep.recordSuccess(prover1, 1000, 0);
    }

    function test_timestampOverflow_recordFailureReverts() public {
        _registerProver(prover1);

        vm.warp(uint256(type(uint64).max) + 1);

        vm.expectRevert(ProverReputation.TimestampOverflow.selector);
        vm.prank(reporter);
        rep.recordFailure(prover1, "fail");
    }

    function test_timestampOverflow_recordAbandonReverts() public {
        _registerProver(prover1);

        vm.warp(uint256(type(uint64).max) + 1);

        vm.expectRevert(ProverReputation.TimestampOverflow.selector);
        vm.prank(reporter);
        rep.recordAbandon(prover1, 1);
    }

    function test_timestampOverflow_slashReverts() public {
        _registerProver(prover1);

        vm.warp(uint256(type(uint64).max) + 1);

        vm.expectRevert(ProverReputation.TimestampOverflow.selector);
        rep.slash(prover1, "bad", 1000);
    }

    function test_timestampSafety_maxUint64Works() public {
        vm.warp(uint256(type(uint64).max));

        vm.prank(prover1);
        rep.register();

        ProverReputation.Reputation memory r = rep.getReputation(prover1);
        assertEq(r.lastUpdateAt, type(uint64).max, "lastUpdateAt should be max uint64");
    }

    function test_slashCooldown_default() public view {
        assertEq(rep.slashCooldown(), 1 hours, "default cooldown should be 1 hour");
    }

    function test_slashCooldown_preventsRapidSlashing() public {
        _registerProver(prover1);

        rep.slash(prover1, "first offense", 1000);

        vm.expectRevert(ProverReputation.SlashCooldownActive.selector);
        rep.slash(prover1, "second offense", 1000);
    }

    function test_slashCooldown_allowsSlashAfterCooldown() public {
        _registerProver(prover1);

        rep.slash(prover1, "first offense", 1000);

        vm.warp(block.timestamp + 1 hours + 1);

        rep.slash(prover1, "second offense", 1000);

        ProverReputation.SlashEvent[] memory history = rep.getSlashHistory(prover1);
        assertEq(history.length, 2, "should have 2 slash events");
    }

    function test_setSlashCooldown_success() public {
        uint256 newCooldown = 2 hours;
        rep.setSlashCooldown(newCooldown);
        assertEq(rep.slashCooldown(), newCooldown, "cooldown should be updated");
    }

    function test_setSlashCooldown_emitsEvent() public {
        vm.expectEmit(false, false, false, true);
        emit SlashCooldownUpdated(1 hours, 2 hours);

        rep.setSlashCooldown(2 hours);
    }

    function test_setSlashCooldown_rejectsZero() public {
        vm.expectRevert(ProverReputation.InvalidCooldown.selector);
        rep.setSlashCooldown(0);
    }

    function test_setSlashCooldown_rejectsBelowMinimum() public {
        vm.expectRevert(ProverReputation.InvalidCooldown.selector);
        rep.setSlashCooldown(30);
    }

    function test_setSlashCooldown_rejectsAboveMaximum() public {
        vm.expectRevert(ProverReputation.InvalidCooldown.selector);
        rep.setSlashCooldown(31 days);
    }

    function test_setSlashCooldown_acceptsMinimum() public {
        rep.setSlashCooldown(1 minutes);
        assertEq(rep.slashCooldown(), 1 minutes);
    }

    function test_setSlashCooldown_acceptsMaximum() public {
        rep.setSlashCooldown(30 days);
        assertEq(rep.slashCooldown(), 30 days);
    }

    function test_setSlashCooldown_onlyOwner() public {
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, stranger));
        vm.prank(stranger);
        rep.setSlashCooldown(2 hours);
    }

    function test_slashCooldown_customCooldownIsRespected() public {
        _registerProver(prover1);

        rep.setSlashCooldown(1 days);

        rep.slash(prover1, "first", 1000);

        vm.warp(block.timestamp + 2 hours);
        vm.expectRevert(ProverReputation.SlashCooldownActive.selector);
        rep.slash(prover1, "second", 1000);

        vm.warp(block.timestamp + 22 hours + 1);
        rep.slash(prover1, "second", 1000);
    }

    function test_cooldownConstants() public view {
        assertEq(rep.MIN_COOLDOWN(), 1 minutes, "MIN_COOLDOWN = 1 minute");
        assertEq(rep.MAX_COOLDOWN(), 30 days, "MAX_COOLDOWN = 30 days");
    }

    // ========================================================================
    // STRING LENGTH BOUNDS (T418)
    // ========================================================================

    function testRecordFailureReasonAtBoundary() public {
        _registerProver(prover1);
        bytes memory longBytes = new bytes(256);
        for (uint256 i = 0; i < 256; i++) {
            longBytes[i] = "A";
        }
        string memory maxReason = string(longBytes);
        rep.recordFailure(prover1, maxReason);
        // Should succeed without revert
    }

    function testRecordFailureReasonTooLong() public {
        _registerProver(prover1);
        bytes memory longBytes = new bytes(257);
        for (uint256 i = 0; i < 257; i++) {
            longBytes[i] = "A";
        }
        string memory tooLong = string(longBytes);
        vm.expectRevert(abi.encodeWithSelector(ProverReputation.StringTooLong.selector, "reason", 256));
        rep.recordFailure(prover1, tooLong);
    }

    function testSlashReasonTooLong() public {
        _registerProver(prover1);
        bytes memory longBytes = new bytes(257);
        for (uint256 i = 0; i < 257; i++) {
            longBytes[i] = "A";
        }
        string memory tooLong = string(longBytes);
        vm.expectRevert(abi.encodeWithSelector(ProverReputation.StringTooLong.selector, "reason", 256));
        rep.slash(prover1, tooLong, 100);
    }

    function testBanReasonTooLong() public {
        _registerProver(prover1);
        bytes memory longBytes = new bytes(257);
        for (uint256 i = 0; i < 257; i++) {
            longBytes[i] = "A";
        }
        string memory tooLong = string(longBytes);
        vm.expectRevert(abi.encodeWithSelector(ProverReputation.StringTooLong.selector, "reason", 256));
        rep.ban(prover1, tooLong);
    }

    // ========================================================================
    // HELPERS
    // ========================================================================

    function _registerProver(address prover) internal {
        vm.prank(prover);
        rep.register();
    }
}
