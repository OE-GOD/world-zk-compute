// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/Upgradeable.sol";
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

    function _authorizeUpgrade(address) internal override onlyAdmin {}
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

    function _authorizeUpgrade(address) internal override onlyAdmin {}
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

    function _authorizeUpgrade(address) internal override onlyAdmin {}
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
}
