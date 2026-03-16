// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/Upgradeable.sol";
import "../src/TimelockController.sol";
import "../src/MockRiscZeroVerifier.sol";

/// @dev V2 implementation for testing upgrades — adds a new storage variable
contract UpgradeableExecutionEngineV2 is UUPSUpgradeable {
    // Must maintain same storage layout as V1
    uint256 public constant VERSION = 2;
    address public registry;
    address public verifier;
    uint256 public protocolFeeBps;
    address public feeRecipient;
    uint256 public nextRequestId;

    struct ExecutionRequest {
        uint256 id;
        bytes32 imageId;
        bytes32 inputDigest;
        address requester;
        uint48 createdAt;
        uint48 expiresAt;
        address callbackContract;
        uint8 status;
        address claimedBy;
        uint48 claimDeadline;
        uint256 tip;
    }

    mapping(uint256 => ExecutionRequest) public requests;
    mapping(address => uint256) public proverCompletedCount;
    mapping(address => uint256) public proverEarnings;

    uint256[50] private __gap;

    // V2: new storage after gap
    uint256 public newV2Variable;

    event Initialized(uint256 version);

    function initializeV2(uint256 _newVar) external {
        newV2Variable = _newVar;
        emit Initialized(VERSION);
    }

    function _authorizeUpgrade(address) internal override {}
}

/// @dev Minimal implementation for proxy testing
contract MinimalImpl is UUPSUpgradeable {
    uint256 public value;

    event Initialized(uint256 version);

    function initialize(address _admin) external initializer {
        _setAdmin(_admin);
        value = 42;
        emit Initialized(1);
    }

    function setValue(uint256 _value) external {
        value = _value;
    }

    function _authorizeUpgrade(address) internal override {}
}

/// @dev V2 of minimal implementation
contract MinimalImplV2 is UUPSUpgradeable {
    uint256 public value;
    uint256 public extraValue;

    function setExtraValue(uint256 _value) external {
        extraValue = _value;
    }

    function setValue(uint256 _value) external {
        value = _value;
    }

    function _authorizeUpgrade(address) internal override {}
}

contract UpgradeableTest is Test {
    // Re-declare events for expectEmit
    event Upgraded(address indexed implementation);
    event AdminChanged(address previousAdmin, address newAdmin);
    event Initialized(uint256 version);
    event ExecutionRequested(
        uint256 indexed requestId,
        address indexed requester,
        bytes32 indexed imageId,
        string inputUrl,
        uint8 inputType,
        uint256 tip
    );

    UpgradeableExecutionEngine public impl;
    MockRiscZeroVerifier public mockVerifier;
    UUPSProxy public proxy;
    UpgradeableExecutionEngine public engine; // proxy cast

    address admin = address(this);
    address registry = address(0xBEEF);
    address feeRecipient = address(0xFEE);
    address user1 = address(0x1111);
    address user2 = address(0x2222);

    function setUp() public {
        // Deploy mock verifier
        mockVerifier = new MockRiscZeroVerifier();

        // Deploy implementation
        impl = new UpgradeableExecutionEngine();

        // Deploy proxy with initialization
        bytes memory initData = abi.encodeCall(
            UpgradeableExecutionEngine.initialize, (registry, address(mockVerifier), feeRecipient, admin)
        );
        proxy = new UUPSProxy(address(impl), initData);

        // Cast proxy to implementation interface
        engine = UpgradeableExecutionEngine(payable(address(proxy)));

        // Mock registry.isProgramActive() to return true for any imageId
        vm.mockCall(
            registry,
            abi.encodeWithSignature("isProgramActive(bytes32)"),
            abi.encode(true)
        );
    }

    // ========================================================================
    // 1. Initial state
    // ========================================================================

    function test_initialImplementationSet() public view {
        assertEq(engine.implementation(), address(impl));
    }

    function test_initialAdminSet() public view {
        assertEq(engine.admin(), admin);
    }

    function test_initialStateSetCorrectly() public view {
        assertEq(engine.registry(), registry);
        assertEq(engine.verifier(), address(mockVerifier));
        assertEq(engine.feeRecipient(), feeRecipient);
        assertEq(engine.protocolFeeBps(), 250);
        assertEq(engine.nextRequestId(), 1);
        assertEq(engine.VERSION(), 1);
    }

    function test_proxiableUUID() public view {
        assertEq(engine.proxiableUUID(), 0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc);
    }

    // ========================================================================
    // 2. Proxy delegation
    // ========================================================================

    function test_proxyDelegatesToImplementation() public {
        // Request execution through proxy — should work via delegation
        bytes32 imageId = bytes32(uint256(1));
        bytes32 inputDigest = bytes32(uint256(2));

        vm.deal(user1, 1 ether);
        vm.prank(user1);
        uint256 requestId =
            engine.requestExecution{value: 0.01 ether}(imageId, inputDigest, "https://input.url", address(0), 3600);
        assertEq(requestId, 1);
        assertEq(engine.nextRequestId(), 2);
    }

    function test_proxyReceivesEther() public {
        vm.deal(user1, 1 ether);
        vm.prank(user1);
        (bool success,) = address(proxy).call{value: 0.1 ether}("");
        assertTrue(success);
        assertEq(address(proxy).balance, 0.1 ether);
    }

    // ========================================================================
    // 3. Upgrade
    // ========================================================================

    function test_upgradeToNewImplementation() public {
        UpgradeableExecutionEngineV2 implV2 = new UpgradeableExecutionEngineV2();

        vm.expectEmit(true, false, false, false);
        emit Upgraded(address(implV2));
        engine.upgradeTo(address(implV2));

        assertEq(engine.implementation(), address(implV2));
    }

    function test_upgradeToAndCall() public {
        UpgradeableExecutionEngineV2 implV2 = new UpgradeableExecutionEngineV2();

        bytes memory callData = abi.encodeCall(UpgradeableExecutionEngineV2.initializeV2, (999));

        engine.upgradeToAndCall(address(implV2), callData);

        assertEq(engine.implementation(), address(implV2));

        // Cast to V2 to check new variable
        UpgradeableExecutionEngineV2 engineV2 = UpgradeableExecutionEngineV2(payable(address(proxy)));
        assertEq(engineV2.newV2Variable(), 999);
    }

    function test_statePreservedAfterUpgrade() public {
        // Set some state via V1
        vm.deal(user1, 1 ether);
        vm.prank(user1);
        engine.requestExecution{value: 0.01 ether}(bytes32(uint256(1)), bytes32(uint256(2)), "url", address(0), 3600);
        assertEq(engine.nextRequestId(), 2);

        // Upgrade
        UpgradeableExecutionEngineV2 implV2 = new UpgradeableExecutionEngineV2();
        engine.upgradeTo(address(implV2));

        // State should be preserved
        assertEq(engine.nextRequestId(), 2);
        assertEq(engine.registry(), registry);
        assertEq(engine.verifier(), address(mockVerifier));
        assertEq(engine.feeRecipient(), feeRecipient);
        assertEq(engine.protocolFeeBps(), 250);
    }

    // ========================================================================
    // 4. Access control
    // ========================================================================

    function test_onlyAdminCanUpgrade() public {
        UpgradeableExecutionEngineV2 implV2 = new UpgradeableExecutionEngineV2();

        vm.prank(user1);
        vm.expectRevert(UUPSUpgradeable.NotAdmin.selector);
        engine.upgradeTo(address(implV2));
    }

    function test_onlyAdminCanUpgradeToAndCall() public {
        UpgradeableExecutionEngineV2 implV2 = new UpgradeableExecutionEngineV2();

        vm.prank(user1);
        vm.expectRevert(UUPSUpgradeable.NotAdmin.selector);
        engine.upgradeToAndCall(address(implV2), "");
    }

    function test_onlyAdminCanChangeAdmin() public {
        vm.prank(user1);
        vm.expectRevert(UUPSUpgradeable.NotAdmin.selector);
        engine.changeAdmin(user1);
    }

    function test_onlyAdminCanSetProtocolFee() public {
        vm.prank(user1);
        vm.expectRevert(UUPSUpgradeable.NotAdmin.selector);
        engine.setProtocolFee(500);
    }

    function test_onlyAdminCanSetFeeRecipient() public {
        vm.prank(user1);
        vm.expectRevert(UUPSUpgradeable.NotAdmin.selector);
        engine.setFeeRecipient(user1);
    }

    // ========================================================================
    // 5. Invalid upgrades
    // ========================================================================

    function test_cannotUpgradeToZeroAddress() public {
        vm.expectRevert(UUPSUpgradeable.InvalidImplementation.selector);
        engine.upgradeTo(address(0));
    }

    function test_cannotUpgradeToEOA() public {
        vm.expectRevert(UUPSUpgradeable.InvalidImplementation.selector);
        engine.upgradeTo(user1);
    }

    // ========================================================================
    // 6. Admin management
    // ========================================================================

    function test_changeAdmin() public {
        vm.expectEmit(false, false, false, true);
        emit AdminChanged(admin, user1);
        engine.changeAdmin(user1);

        assertEq(engine.admin(), user1);
    }

    function test_newAdminCanUpgrade() public {
        engine.changeAdmin(user1);

        UpgradeableExecutionEngineV2 implV2 = new UpgradeableExecutionEngineV2();

        vm.prank(user1);
        engine.upgradeTo(address(implV2));
        assertEq(engine.implementation(), address(implV2));
    }

    function test_oldAdminCannotUpgradeAfterChange() public {
        engine.changeAdmin(user1);

        UpgradeableExecutionEngineV2 implV2 = new UpgradeableExecutionEngineV2();

        // Old admin (address(this)) should fail
        vm.expectRevert(UUPSUpgradeable.NotAdmin.selector);
        engine.upgradeTo(address(implV2));
    }

    function test_cannotChangeAdminToZero() public {
        vm.expectRevert("Invalid admin");
        engine.changeAdmin(address(0));
    }

    // ========================================================================
    // 7. Initialization
    // ========================================================================

    function test_cannotReinitialize() public {
        vm.expectRevert(UUPSUpgradeable.AlreadyInitialized.selector);
        engine.initialize(registry, address(mockVerifier), feeRecipient, admin);
    }

    function test_initializeRejectsZeroRegistry() public {
        UpgradeableExecutionEngine freshImpl = new UpgradeableExecutionEngine();
        bytes memory initData = abi.encodeCall(
            UpgradeableExecutionEngine.initialize, (address(0), address(mockVerifier), feeRecipient, admin)
        );
        vm.expectRevert("Invalid registry");
        new UUPSProxy(address(freshImpl), initData);
    }

    function test_initializeRejectsZeroVerifier() public {
        UpgradeableExecutionEngine freshImpl = new UpgradeableExecutionEngine();
        bytes memory initData =
            abi.encodeCall(UpgradeableExecutionEngine.initialize, (registry, address(0), feeRecipient, admin));
        vm.expectRevert("Invalid verifier");
        new UUPSProxy(address(freshImpl), initData);
    }

    function test_initializeRejectsZeroFeeRecipient() public {
        UpgradeableExecutionEngine freshImpl = new UpgradeableExecutionEngine();
        bytes memory initData =
            abi.encodeCall(UpgradeableExecutionEngine.initialize, (registry, address(mockVerifier), address(0), admin));
        vm.expectRevert("Invalid fee recipient");
        new UUPSProxy(address(freshImpl), initData);
    }

    function test_initializeRejectsZeroAdmin() public {
        UpgradeableExecutionEngine freshImpl = new UpgradeableExecutionEngine();
        bytes memory initData = abi.encodeCall(
            UpgradeableExecutionEngine.initialize, (registry, address(mockVerifier), feeRecipient, address(0))
        );
        vm.expectRevert("Invalid admin");
        new UUPSProxy(address(freshImpl), initData);
    }

    // ========================================================================
    // 8. Core functionality through proxy
    // ========================================================================

    function test_requestAndClaimExecution() public {
        bytes32 imageId = bytes32(uint256(1));
        bytes32 inputDigest = bytes32(uint256(2));

        vm.deal(user1, 1 ether);
        vm.prank(user1);
        uint256 requestId = engine.requestExecution{value: 0.01 ether}(imageId, inputDigest, "url", address(0), 3600);

        vm.prank(user2);
        engine.claimExecution(requestId);

        UpgradeableExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);
        assertEq(req.claimedBy, user2);
        assertEq(req.status, 1); // Claimed
    }

    function test_fullCycleThroughProxy() public {
        bytes32 imageId = bytes32(uint256(1));
        bytes32 inputDigest = bytes32(uint256(2));

        vm.deal(user1, 1 ether);
        vm.prank(user1);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, "url", address(0), 3600);

        vm.prank(user2);
        engine.claimExecution(requestId);

        // Submit proof
        bytes memory seal = hex"deadbeef";
        bytes memory journal = hex"cafebabe";
        vm.deal(address(proxy), 1 ether); // Fund proxy for payouts

        vm.prank(user2);
        engine.submitProof(requestId, seal, journal);

        UpgradeableExecutionEngine.ExecutionRequest memory req = engine.getRequest(requestId);
        assertEq(req.status, 2); // Completed

        (uint256 completed, uint256 earnings) = engine.getProverStats(user2);
        assertEq(completed, 1);
        assertGt(earnings, 0);
    }

    function test_adminFunctionsWorkThroughProxy() public {
        engine.setProtocolFee(500);
        assertEq(engine.protocolFeeBps(), 500);

        engine.setFeeRecipient(user2);
        assertEq(engine.feeRecipient(), user2);
    }

    // ========================================================================
    // 9. Minimal proxy upgrade test
    // ========================================================================

    function test_minimalProxyUpgradePreservesState() public {
        MinimalImpl minImpl = new MinimalImpl();
        bytes memory initData = abi.encodeCall(MinimalImpl.initialize, (admin));
        UUPSProxy minProxy = new UUPSProxy(address(minImpl), initData);
        MinimalImpl minEngine = MinimalImpl(payable(address(minProxy)));

        // Set state
        minEngine.setValue(123);
        assertEq(minEngine.value(), 123);

        // Upgrade
        MinimalImplV2 minImplV2 = new MinimalImplV2();
        minEngine.upgradeTo(address(minImplV2));

        // State preserved
        MinimalImplV2 minEngineV2 = MinimalImplV2(payable(address(minProxy)));
        assertEq(minEngineV2.value(), 123);

        // New functionality works
        minEngineV2.setExtraValue(456);
        assertEq(minEngineV2.extraValue(), 456);
    }

    // ========================================================================
    // 10. Reinitialize
    // ========================================================================

    function test_reinitialize() public {
        engine.reinitialize(2);
        // Just checking it doesn't revert — reinitialize is for upgrade-time setup
    }

    function test_reinitializeOnlyAdmin() public {
        vm.prank(user1);
        vm.expectRevert(UUPSUpgradeable.NotAdmin.selector);
        engine.reinitialize(2);
    }

    // ========================================================================
    // 11. Proxy constructor edge cases
    // ========================================================================

    function test_proxyWithoutInitData() public {
        UpgradeableExecutionEngine freshImpl = new UpgradeableExecutionEngine();
        // Deploy proxy without initialization (empty data)
        UUPSProxy freshProxy = new UUPSProxy(address(freshImpl), "");
        UpgradeableExecutionEngine freshEngine = UpgradeableExecutionEngine(payable(address(freshProxy)));

        // Implementation should be set but state uninitialized
        assertEq(freshEngine.implementation(), address(freshImpl));
        assertEq(freshEngine.registry(), address(0));
        assertEq(freshEngine.nextRequestId(), 0);
    }

    function test_storageSlotConstants() public pure {
        // Verify EIP-1967 slot computation
        assertEq(StorageSlot.IMPLEMENTATION_SLOT, bytes32(uint256(keccak256("eip1967.proxy.implementation")) - 1));
        assertEq(StorageSlot.ADMIN_SLOT, bytes32(uint256(keccak256("eip1967.proxy.admin")) - 1));
    }

    // ========================================================================
    // 12. Pause / unpause
    // ========================================================================

    function test_pauseBlocksClaimExecution() public {
        // Create a request first
        bytes32 imageId = bytes32(uint256(1));
        bytes32 inputDigest = bytes32(uint256(2));
        vm.deal(user1, 1 ether);
        vm.prank(user1);
        uint256 requestId = engine.requestExecution{value: 0.01 ether}(imageId, inputDigest, "url", address(0), 3600);

        // Pause
        engine.pause();
        assertTrue(engine.paused());

        // claimExecution should revert
        vm.prank(user2);
        vm.expectRevert(UpgradeableExecutionEngine.EnforcedPause.selector);
        engine.claimExecution(requestId);
    }

    function test_pauseBlocksSubmitProof() public {
        // Create and claim a request
        bytes32 imageId = bytes32(uint256(1));
        bytes32 inputDigest = bytes32(uint256(2));
        vm.deal(user1, 1 ether);
        vm.prank(user1);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, "url", address(0), 3600);
        vm.prank(user2);
        engine.claimExecution(requestId);

        // Pause
        engine.pause();

        // submitProof should revert
        vm.prank(user2);
        vm.expectRevert(UpgradeableExecutionEngine.EnforcedPause.selector);
        engine.submitProof(requestId, hex"deadbeef", hex"cafebabe");
    }

    function test_unpauseRestoresOperations() public {
        // Create a request
        bytes32 imageId = bytes32(uint256(1));
        bytes32 inputDigest = bytes32(uint256(2));
        vm.deal(user1, 1 ether);
        vm.prank(user1);
        uint256 requestId = engine.requestExecution{value: 0.01 ether}(imageId, inputDigest, "url", address(0), 3600);

        // Pause then unpause
        engine.pause();
        engine.unpause();
        assertFalse(engine.paused());

        // claimExecution should work again
        vm.prank(user2);
        engine.claimExecution(requestId);
    }

    function test_onlyAdminCanPause() public {
        vm.prank(user1);
        vm.expectRevert(UUPSUpgradeable.NotAdmin.selector);
        engine.pause();
    }

    function test_onlyAdminCanUnpause() public {
        engine.pause();
        vm.prank(user1);
        vm.expectRevert(UUPSUpgradeable.NotAdmin.selector);
        engine.unpause();
    }

    function test_cannotPauseWhenAlreadyPaused() public {
        engine.pause();
        vm.expectRevert(UpgradeableExecutionEngine.EnforcedPause.selector);
        engine.pause();
    }

    function test_cannotUnpauseWhenNotPaused() public {
        vm.expectRevert(UpgradeableExecutionEngine.ExpectedPause.selector);
        engine.unpause();
    }

    // ========================================================================
    // 13. Event emissions for setProtocolFee / setFeeRecipient
    // ========================================================================

    event ProtocolFeeUpdated(uint256 oldFeeBps, uint256 newFeeBps);
    event FeeRecipientUpdated(address indexed oldRecipient, address indexed newRecipient);

    function test_setProtocolFeeEmitsEvent() public {
        vm.expectEmit(false, false, false, true);
        emit ProtocolFeeUpdated(250, 500);
        engine.setProtocolFee(500);
    }

    function test_setFeeRecipientEmitsEvent() public {
        vm.expectEmit(true, true, false, false);
        emit FeeRecipientUpdated(feeRecipient, user2);
        engine.setFeeRecipient(user2);
    }

    // ========================================================================
    // 14. Transfer uses call instead of transfer (contract wallet compat)
    // ========================================================================

    function test_submitProofPaysViaCall() public {
        // Full cycle -- verifies .call{value} works (contract wallets)
        bytes32 imageId = bytes32(uint256(1));
        bytes32 inputDigest = bytes32(uint256(2));

        vm.deal(user1, 1 ether);
        vm.prank(user1);
        uint256 requestId = engine.requestExecution{value: 0.1 ether}(imageId, inputDigest, "url", address(0), 3600);

        vm.prank(user2);
        engine.claimExecution(requestId);

        vm.deal(address(proxy), 1 ether);
        uint256 balanceBefore = user2.balance;

        vm.prank(user2);
        engine.submitProof(requestId, hex"deadbeef", hex"cafebabe");

        // Prover should have been paid (0.1 ether minus 2.5% fee = 0.0975 ether)
        uint256 expectedPayout = 0.1 ether - (0.1 ether * 250) / 10000;
        assertEq(user2.balance - balanceBefore, expectedPayout);
    }

    // ========================================================================
    // 15. Reentrancy guard on submitProof
    // ========================================================================

    function test_initialReentrancyStatusSet() public view {
        // After initialization, the contract should not be paused
        // and the reentrancy guard should be initialized (submitProof works in full cycle test)
        assertFalse(engine.paused());
    }

    // ========================================================================
    // 16. Timelock integration on UUPSUpgradeable
    // ========================================================================

    event TimelockChanged(address previousTimelock, address newTimelock);

    function test_initialTimelockIsZero() public view {
        assertEq(engine.timelock(), address(0));
    }

    function test_setTimelockByAdmin() public {
        address timelockAddr = address(0xDEAD);
        vm.expectEmit(false, false, false, true);
        emit TimelockChanged(address(0), timelockAddr);
        engine.setTimelock(timelockAddr);
        assertEq(engine.timelock(), timelockAddr);
    }

    function test_onlyAdminCanSetTimelock() public {
        vm.prank(user1);
        vm.expectRevert(UUPSUpgradeable.NotAdmin.selector);
        engine.setTimelock(address(0xDEAD));
    }

    function test_clearTimelock() public {
        engine.setTimelock(address(0xDEAD));
        assertEq(engine.timelock(), address(0xDEAD));
        engine.setTimelock(address(0));
        assertEq(engine.timelock(), address(0));
    }

    function test_timelockSetBlocksDirectAdminCalls() public {
        address timelockAddr = address(0xDEAD);
        engine.setTimelock(timelockAddr);

        // Admin direct calls to timelocked functions should revert
        vm.expectRevert(UUPSUpgradeable.NotTimelocked.selector);
        engine.setProtocolFee(500);

        vm.expectRevert(UUPSUpgradeable.NotTimelocked.selector);
        engine.setFeeRecipient(user2);

        UpgradeableExecutionEngineV2 implV2 = new UpgradeableExecutionEngineV2();
        vm.expectRevert(UUPSUpgradeable.NotTimelocked.selector);
        engine.upgradeTo(address(implV2));
    }

    function test_timelockAddressCanCallTimelockedFunctions() public {
        address timelockAddr = address(0xDEAD);
        engine.setTimelock(timelockAddr);

        // The timelock address itself can call the functions
        vm.prank(timelockAddr);
        engine.setProtocolFee(500);
        assertEq(engine.protocolFeeBps(), 500);

        vm.prank(timelockAddr);
        engine.setFeeRecipient(user2);
        assertEq(engine.feeRecipient(), user2);
    }

    function test_withoutTimelockAdminCanCallDirectly() public {
        // Default: no timelock set, admin can call directly (backwards compatible)
        engine.setProtocolFee(500);
        assertEq(engine.protocolFeeBps(), 500);

        engine.setFeeRecipient(user2);
        assertEq(engine.feeRecipient(), user2);
    }

    function test_nonAdminNonTimelockCannotCallTimelockedFunctions() public {
        // Without timelock: non-admin reverts
        vm.prank(user1);
        vm.expectRevert(UUPSUpgradeable.NotAdmin.selector);
        engine.setProtocolFee(500);

        // With timelock: non-timelock reverts
        engine.setTimelock(address(0xDEAD));
        vm.prank(user1);
        vm.expectRevert(UUPSUpgradeable.NotTimelocked.selector);
        engine.setProtocolFee(500);
    }

    function test_adminNonTimelockedFunctionsStillWorkWithTimelockSet() public {
        // pause/unpause and changeAdmin use onlyAdmin, not onlyTimelocked
        engine.setTimelock(address(0xDEAD));

        // These should still work from admin
        engine.pause();
        assertTrue(engine.paused());
        engine.unpause();
        assertFalse(engine.paused());
    }

    // ========================================================================
    // Storage Layout Safety Tests
    // ========================================================================

    function test_storageLayout_registryAtSlot1() public view {
        // slot 0 is packed: _initialized(uint8) + _initializing(bool) + timelock(address)
        // registry starts at slot 1
        bytes32 val = vm.load(address(engine), bytes32(uint256(1)));
        assertEq(address(uint160(uint256(val))), engine.registry());
    }

    function test_storageLayout_verifierAtSlot2() public view {
        bytes32 val = vm.load(address(engine), bytes32(uint256(2)));
        assertEq(address(uint160(uint256(val))), engine.verifier());
    }

    function test_storageLayout_protocolFeeBpsAtSlot3() public view {
        bytes32 val = vm.load(address(engine), bytes32(uint256(3)));
        assertEq(uint256(val), engine.protocolFeeBps());
    }

    function test_storageLayout_feeRecipientAtSlot4() public view {
        bytes32 val = vm.load(address(engine), bytes32(uint256(4)));
        assertEq(address(uint160(uint256(val))), engine.feeRecipient());
    }

    function test_storageLayout_nextRequestIdAtSlot5() public view {
        bytes32 val = vm.load(address(engine), bytes32(uint256(5)));
        assertEq(uint256(val), engine.nextRequestId());
    }

    function test_storageLayout_gapPreservesUpgradeSpace() public {
        // After upgrade to V2, existing storage must be intact
        // First, store some state
        vm.deal(user1, 1 ether);
        vm.prank(user1);
        uint256 reqId = engine.requestExecution{value: 0.01 ether}(
            bytes32(uint256(1)), bytes32(uint256(2)), "test", address(0), 3600
        );
        uint256 feeBeforeUpgrade = engine.protocolFeeBps();
        address registryBefore = engine.registry();

        // Upgrade to V2
        UpgradeableExecutionEngineV2 v2Impl = new UpgradeableExecutionEngineV2();
        engine.upgradeToAndCall(address(v2Impl), "");
        UpgradeableExecutionEngineV2 v2 = UpgradeableExecutionEngineV2(payable(address(proxy)));

        // Verify storage survived the upgrade
        assertEq(v2.protocolFeeBps(), feeBeforeUpgrade);
        assertEq(v2.registry(), registryBefore);
        assertEq(v2.nextRequestId(), reqId + 1);
        assertEq(v2.VERSION(), 2);
    }
}

// =============================================================================
// TimelockController Tests
// =============================================================================

contract TimelockControllerTest is Test {
    TimelockController public timelockCtl;
    UpgradeableExecutionEngine public impl;
    MockRiscZeroVerifier public mockVerifier;
    UUPSProxy public proxy;
    UpgradeableExecutionEngine public engine;

    address owner = address(this);
    address nonOwner = address(0x1111);
    address registryAddr = address(0xBEEF);
    address feeRecipientAddr = address(0xFEE);

    uint256 constant MIN_DELAY = 48 hours;

    // Re-declare events for expectEmit
    event OperationScheduled(
        bytes32 indexed id, address indexed target, bytes data, uint256 delay, uint256 readyTimestamp
    );
    event OperationExecuted(bytes32 indexed id);
    event OperationCancelled(bytes32 indexed id);
    event MinDelayUpdated(uint256 oldDelay, uint256 newDelay);
    event OwnershipTransferred(address indexed previousOwner, address indexed newOwner);
    event TimelockChanged(address previousTimelock, address newTimelock);
    event ProtocolFeeUpdated(uint256 oldFeeBps, uint256 newFeeBps);
    event FeeRecipientUpdated(address indexed oldRecipient, address indexed newRecipient);

    function setUp() public {
        // Deploy timelock
        timelockCtl = new TimelockController(MIN_DELAY, owner);

        // Deploy upgradeable engine through proxy
        mockVerifier = new MockRiscZeroVerifier();
        impl = new UpgradeableExecutionEngine();
        bytes memory initData = abi.encodeCall(
            UpgradeableExecutionEngine.initialize, (registryAddr, address(mockVerifier), feeRecipientAddr, owner)
        );
        proxy = new UUPSProxy(address(impl), initData);
        engine = UpgradeableExecutionEngine(payable(address(proxy)));

        // Wire up: set the timelock on the engine
        engine.setTimelock(address(timelockCtl));
    }

    // ========================================================================
    // 1. TimelockController constructor
    // ========================================================================

    function test_constructorSetsOwnerAndDelay() public view {
        assertEq(timelockCtl.owner(), owner);
        assertEq(timelockCtl.minDelay(), MIN_DELAY);
    }

    function test_constructorRevertsZeroDelay() public {
        vm.expectRevert(TimelockController.ZeroDelay.selector);
        new TimelockController(0, owner);
    }

    function test_constructorRevertsZeroOwner() public {
        vm.expectRevert(TimelockController.ZeroAddress.selector);
        new TimelockController(MIN_DELAY, address(0));
    }

    // ========================================================================
    // 2. Schedule
    // ========================================================================

    function test_scheduleOperation() public {
        bytes32 id = keccak256("test-op-1");
        bytes memory data = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (500));

        vm.expectEmit(true, true, false, true);
        emit OperationScheduled(id, address(engine), data, MIN_DELAY, block.timestamp + MIN_DELAY);

        timelockCtl.schedule(id, address(engine), data, MIN_DELAY);

        (address target, bytes memory opData, uint256 readyTs, bool executed, bool cancelled) =
            timelockCtl.getOperation(id);
        assertEq(target, address(engine));
        assertEq(opData, data);
        assertEq(readyTs, block.timestamp + MIN_DELAY);
        assertFalse(executed);
        assertFalse(cancelled);
    }

    function test_scheduleWithLongerDelay() public {
        bytes32 id = keccak256("test-op-long");
        bytes memory data = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (500));
        uint256 longerDelay = 7 days;

        timelockCtl.schedule(id, address(engine), data, longerDelay);

        (,, uint256 readyTs,,) = timelockCtl.getOperation(id);
        assertEq(readyTs, block.timestamp + longerDelay);
    }

    function test_scheduleRevertsDelayBelowMinimum() public {
        bytes32 id = keccak256("test-op-short");
        bytes memory data = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (500));

        vm.expectRevert(TimelockController.DelayBelowMinimum.selector);
        timelockCtl.schedule(id, address(engine), data, MIN_DELAY - 1);
    }

    function test_scheduleRevertsDuplicateId() public {
        bytes32 id = keccak256("test-op-dup");
        bytes memory data = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (500));

        timelockCtl.schedule(id, address(engine), data, MIN_DELAY);

        vm.expectRevert(TimelockController.OperationAlreadyScheduled.selector);
        timelockCtl.schedule(id, address(engine), data, MIN_DELAY);
    }

    function test_scheduleRevertsZeroTarget() public {
        bytes32 id = keccak256("test-op-zero");
        bytes memory data = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (500));

        vm.expectRevert(TimelockController.ZeroAddress.selector);
        timelockCtl.schedule(id, address(0), data, MIN_DELAY);
    }

    function test_onlyOwnerCanSchedule() public {
        bytes32 id = keccak256("test-op-nonowner");
        bytes memory data = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (500));

        vm.prank(nonOwner);
        vm.expectRevert(TimelockController.NotOwner.selector);
        timelockCtl.schedule(id, address(engine), data, MIN_DELAY);
    }

    // ========================================================================
    // 3. Execute
    // ========================================================================

    function test_executeAfterDelay() public {
        bytes32 id = keccak256("test-exec-1");
        bytes memory data = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (500));

        timelockCtl.schedule(id, address(engine), data, MIN_DELAY);

        // Warp to exactly the ready time
        vm.warp(block.timestamp + MIN_DELAY);

        vm.expectEmit(true, false, false, false);
        emit OperationExecuted(id);

        timelockCtl.execute(id);

        // Verify the fee was actually changed
        assertEq(engine.protocolFeeBps(), 500);

        // Check operation is marked executed
        (,, uint256 readyTs, bool executed, bool cancelled) = timelockCtl.getOperation(id);
        assertTrue(executed);
        assertFalse(cancelled);
        assertGt(readyTs, 0);
    }

    function test_executeRevertsBeforeDelay() public {
        bytes32 id = keccak256("test-exec-early");
        bytes memory data = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (500));

        timelockCtl.schedule(id, address(engine), data, MIN_DELAY);

        // Warp to 1 second before ready
        vm.warp(block.timestamp + MIN_DELAY - 1);

        vm.expectRevert(TimelockController.OperationNotReady.selector);
        timelockCtl.execute(id);
    }

    function test_executeRevertsNotScheduled() public {
        bytes32 id = keccak256("nonexistent");

        vm.expectRevert(TimelockController.OperationNotScheduled.selector);
        timelockCtl.execute(id);
    }

    function test_executeRevertsAlreadyExecuted() public {
        bytes32 id = keccak256("test-exec-double");
        bytes memory data = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (500));

        timelockCtl.schedule(id, address(engine), data, MIN_DELAY);
        vm.warp(block.timestamp + MIN_DELAY);
        timelockCtl.execute(id);

        vm.expectRevert(TimelockController.OperationAlreadyExecuted.selector);
        timelockCtl.execute(id);
    }

    function test_executeRevertsCancelled() public {
        bytes32 id = keccak256("test-exec-cancelled");
        bytes memory data = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (500));

        timelockCtl.schedule(id, address(engine), data, MIN_DELAY);
        timelockCtl.cancel(id);

        vm.warp(block.timestamp + MIN_DELAY);

        vm.expectRevert(TimelockController.OperationCancelledError.selector);
        timelockCtl.execute(id);
    }

    function test_anyoneCanExecuteAfterDelay() public {
        bytes32 id = keccak256("test-exec-anyone");
        bytes memory data = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (500));

        timelockCtl.schedule(id, address(engine), data, MIN_DELAY);
        vm.warp(block.timestamp + MIN_DELAY);

        // Non-owner can execute
        vm.prank(nonOwner);
        timelockCtl.execute(id);

        assertEq(engine.protocolFeeBps(), 500);
    }

    // ========================================================================
    // 4. Cancel
    // ========================================================================

    function test_cancelPendingOperation() public {
        bytes32 id = keccak256("test-cancel-1");
        bytes memory data = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (500));

        timelockCtl.schedule(id, address(engine), data, MIN_DELAY);

        vm.expectEmit(true, false, false, false);
        emit OperationCancelled(id);

        timelockCtl.cancel(id);

        (,,,, bool cancelled) = timelockCtl.getOperation(id);
        assertTrue(cancelled);
    }

    function test_cancelRevertsNotScheduled() public {
        bytes32 id = keccak256("nonexistent-cancel");

        vm.expectRevert(TimelockController.OperationNotScheduled.selector);
        timelockCtl.cancel(id);
    }

    function test_cancelRevertsAlreadyExecuted() public {
        bytes32 id = keccak256("test-cancel-executed");
        bytes memory data = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (500));

        timelockCtl.schedule(id, address(engine), data, MIN_DELAY);
        vm.warp(block.timestamp + MIN_DELAY);
        timelockCtl.execute(id);

        vm.expectRevert(TimelockController.OperationAlreadyExecuted.selector);
        timelockCtl.cancel(id);
    }

    function test_cancelRevertsAlreadyCancelled() public {
        bytes32 id = keccak256("test-cancel-twice");
        bytes memory data = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (500));

        timelockCtl.schedule(id, address(engine), data, MIN_DELAY);
        timelockCtl.cancel(id);

        vm.expectRevert(TimelockController.OperationCancelledError.selector);
        timelockCtl.cancel(id);
    }

    function test_onlyOwnerCanCancel() public {
        bytes32 id = keccak256("test-cancel-nonowner");
        bytes memory data = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (500));

        timelockCtl.schedule(id, address(engine), data, MIN_DELAY);

        vm.prank(nonOwner);
        vm.expectRevert(TimelockController.NotOwner.selector);
        timelockCtl.cancel(id);
    }

    // ========================================================================
    // 5. Full E2E: schedule through timelock, execute on engine
    // ========================================================================

    function test_e2eSetProtocolFeeViaTimelock() public {
        // The engine has timelock set in setUp()
        assertEq(engine.timelock(), address(timelockCtl));

        // Admin cannot directly call setProtocolFee anymore
        vm.expectRevert(UUPSUpgradeable.NotTimelocked.selector);
        engine.setProtocolFee(500);

        // Schedule through timelock
        bytes32 id = keccak256("set-fee-500");
        bytes memory data = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (500));
        timelockCtl.schedule(id, address(engine), data, MIN_DELAY);

        // Cannot execute yet
        vm.expectRevert(TimelockController.OperationNotReady.selector);
        timelockCtl.execute(id);

        // Wait for delay
        vm.warp(block.timestamp + MIN_DELAY);

        // Execute
        timelockCtl.execute(id);
        assertEq(engine.protocolFeeBps(), 500);
    }

    function test_e2eSetFeeRecipientViaTimelock() public {
        address newRecipient = address(0x9999);

        bytes32 id = keccak256("set-recipient");
        bytes memory data = abi.encodeCall(UpgradeableExecutionEngine.setFeeRecipient, (newRecipient));
        timelockCtl.schedule(id, address(engine), data, MIN_DELAY);

        vm.warp(block.timestamp + MIN_DELAY);
        timelockCtl.execute(id);

        assertEq(engine.feeRecipient(), newRecipient);
    }

    function test_e2eUpgradeViaTimelock() public {
        UpgradeableExecutionEngineV2 implV2 = new UpgradeableExecutionEngineV2();

        bytes32 id = keccak256("upgrade-to-v2");
        bytes memory data = abi.encodeCall(UUPSUpgradeable.upgradeTo, (address(implV2)));
        timelockCtl.schedule(id, address(engine), data, MIN_DELAY);

        vm.warp(block.timestamp + MIN_DELAY);
        timelockCtl.execute(id);

        assertEq(engine.implementation(), address(implV2));
    }

    function test_e2eCancelPreventExecution() public {
        bytes32 id = keccak256("cancel-me");
        bytes memory data = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (999));
        timelockCtl.schedule(id, address(engine), data, MIN_DELAY);

        // Cancel before delay passes
        timelockCtl.cancel(id);

        // Even after delay, execution should fail
        vm.warp(block.timestamp + MIN_DELAY);
        vm.expectRevert(TimelockController.OperationCancelledError.selector);
        timelockCtl.execute(id);

        // Fee should be unchanged
        assertEq(engine.protocolFeeBps(), 250);
    }

    // ========================================================================
    // 6. Backwards compatibility (no timelock set)
    // ========================================================================

    function test_backwardsCompatibleWithoutTimelock() public {
        // Deploy a fresh engine without timelock
        UpgradeableExecutionEngine freshImpl = new UpgradeableExecutionEngine();
        bytes memory initData = abi.encodeCall(
            UpgradeableExecutionEngine.initialize, (registryAddr, address(mockVerifier), feeRecipientAddr, owner)
        );
        UUPSProxy freshProxy = new UUPSProxy(address(freshImpl), initData);
        UpgradeableExecutionEngine freshEngine = UpgradeableExecutionEngine(payable(address(freshProxy)));

        // No timelock set -- admin can call directly
        assertEq(freshEngine.timelock(), address(0));

        freshEngine.setProtocolFee(500);
        assertEq(freshEngine.protocolFeeBps(), 500);

        freshEngine.setFeeRecipient(address(0x9999));
        assertEq(freshEngine.feeRecipient(), address(0x9999));

        UpgradeableExecutionEngineV2 implV2 = new UpgradeableExecutionEngineV2();
        freshEngine.upgradeTo(address(implV2));
        assertEq(freshEngine.implementation(), address(implV2));
    }

    // ========================================================================
    // 7. View functions
    // ========================================================================

    function test_isOperationPending() public {
        bytes32 id = keccak256("pending-check");
        bytes memory data = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (500));

        assertFalse(timelockCtl.isOperationPending(id));

        timelockCtl.schedule(id, address(engine), data, MIN_DELAY);
        assertTrue(timelockCtl.isOperationPending(id));

        vm.warp(block.timestamp + MIN_DELAY);
        assertTrue(timelockCtl.isOperationPending(id)); // Still pending until executed

        timelockCtl.execute(id);
        assertFalse(timelockCtl.isOperationPending(id)); // No longer pending
    }

    function test_isOperationReady() public {
        bytes32 id = keccak256("ready-check");
        bytes memory data = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (500));

        assertFalse(timelockCtl.isOperationReady(id));

        timelockCtl.schedule(id, address(engine), data, MIN_DELAY);
        assertFalse(timelockCtl.isOperationReady(id)); // Not ready yet

        vm.warp(block.timestamp + MIN_DELAY);
        assertTrue(timelockCtl.isOperationReady(id)); // Now ready

        timelockCtl.execute(id);
        assertFalse(timelockCtl.isOperationReady(id)); // No longer ready (executed)
    }

    function test_isOperationPendingCancelled() public {
        bytes32 id = keccak256("pending-cancelled");
        bytes memory data = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (500));

        timelockCtl.schedule(id, address(engine), data, MIN_DELAY);
        assertTrue(timelockCtl.isOperationPending(id));

        timelockCtl.cancel(id);
        assertFalse(timelockCtl.isOperationPending(id));
        assertFalse(timelockCtl.isOperationReady(id));
    }

    // ========================================================================
    // 8. Admin functions on TimelockController itself
    // ========================================================================

    function test_setMinDelay() public {
        uint256 newDelay = 72 hours;
        vm.expectEmit(false, false, false, true);
        emit MinDelayUpdated(MIN_DELAY, newDelay);
        timelockCtl.setMinDelay(newDelay);
        assertEq(timelockCtl.minDelay(), newDelay);
    }

    function test_setMinDelayRevertsZero() public {
        vm.expectRevert(TimelockController.ZeroDelay.selector);
        timelockCtl.setMinDelay(0);
    }

    function test_onlyOwnerCanSetMinDelay() public {
        vm.prank(nonOwner);
        vm.expectRevert(TimelockController.NotOwner.selector);
        timelockCtl.setMinDelay(72 hours);
    }

    function test_transferOwnership() public {
        vm.expectEmit(true, true, false, false);
        emit OwnershipTransferred(owner, nonOwner);
        timelockCtl.transferOwnership(nonOwner);
        assertEq(timelockCtl.owner(), nonOwner);
    }

    function test_transferOwnershipRevertsZero() public {
        vm.expectRevert(TimelockController.ZeroAddress.selector);
        timelockCtl.transferOwnership(address(0));
    }

    function test_onlyOwnerCanTransferOwnership() public {
        vm.prank(nonOwner);
        vm.expectRevert(TimelockController.NotOwner.selector);
        timelockCtl.transferOwnership(nonOwner);
    }

    function test_newOwnerCanScheduleAfterTransfer() public {
        timelockCtl.transferOwnership(nonOwner);

        bytes32 id = keccak256("new-owner-op");
        bytes memory data = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (500));

        vm.prank(nonOwner);
        timelockCtl.schedule(id, address(engine), data, MIN_DELAY);

        assertTrue(timelockCtl.isOperationPending(id));
    }

    function test_oldOwnerCannotScheduleAfterTransfer() public {
        timelockCtl.transferOwnership(nonOwner);

        bytes32 id = keccak256("old-owner-op");
        bytes memory data = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (500));

        vm.expectRevert(TimelockController.NotOwner.selector);
        timelockCtl.schedule(id, address(engine), data, MIN_DELAY);
    }

    // ========================================================================
    // 9. Execute with failing target call
    // ========================================================================

    function test_executeRevertsOnTargetRevert() public {
        // Schedule a call that will revert (fee too high: > 1000 bps)
        bytes32 id = keccak256("bad-fee");
        bytes memory data = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (9999));

        timelockCtl.schedule(id, address(engine), data, MIN_DELAY);
        vm.warp(block.timestamp + MIN_DELAY);

        vm.expectRevert(UpgradeableExecutionEngine.FeeTooHigh.selector);
        timelockCtl.execute(id);
    }

    // ========================================================================
    // 10. Multiple operations
    // ========================================================================

    function test_multipleOperationsIndependent() public {
        bytes32 id1 = keccak256("op-1");
        bytes32 id2 = keccak256("op-2");
        bytes memory data1 = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (500));
        bytes memory data2 = abi.encodeCall(UpgradeableExecutionEngine.setFeeRecipient, (address(0x9999)));

        timelockCtl.schedule(id1, address(engine), data1, MIN_DELAY);
        timelockCtl.schedule(id2, address(engine), data2, MIN_DELAY);

        vm.warp(block.timestamp + MIN_DELAY);

        // Execute in any order
        timelockCtl.execute(id2);
        assertEq(engine.feeRecipient(), address(0x9999));
        assertEq(engine.protocolFeeBps(), 250); // Unchanged

        timelockCtl.execute(id1);
        assertEq(engine.protocolFeeBps(), 500);
    }

    function test_cancelOneExecuteOther() public {
        bytes32 id1 = keccak256("cancel-this");
        bytes32 id2 = keccak256("keep-this");
        bytes memory data1 = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (100));
        bytes memory data2 = abi.encodeCall(UpgradeableExecutionEngine.setProtocolFee, (500));

        timelockCtl.schedule(id1, address(engine), data1, MIN_DELAY);
        timelockCtl.schedule(id2, address(engine), data2, MIN_DELAY);

        timelockCtl.cancel(id1);

        vm.warp(block.timestamp + MIN_DELAY);

        vm.expectRevert(TimelockController.OperationCancelledError.selector);
        timelockCtl.execute(id1);

        timelockCtl.execute(id2);
        assertEq(engine.protocolFeeBps(), 500);
    }
}
