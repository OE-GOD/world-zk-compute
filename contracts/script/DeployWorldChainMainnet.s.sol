// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/ProgramRegistry.sol";
import "../src/ProverRegistry.sol";
import "../src/ProverReputation.sol";
import "../src/ExecutionEngine.sol";
import "../src/remainder/RemainderVerifier.sol";
import {UUPSProxy} from "../src/Upgradeable.sol";
import "../src/remainder/GKRDAGVerifier.sol";
import "../src/tee/TEEMLVerifier.sol";

/// @title DeployWorldChainMainnet
/// @notice Deploy the full World ZK Compute stack to World Chain Mainnet (chainId 480).
/// @dev Production deployment script with safety checks:
///      - Validates chain ID (must be 480)
///      - Requires explicit ADMIN_ADDRESS (no deployer-as-admin)
///      - Requires STAKING_TOKEN (no placeholder token)
///      - 2-step ownership transfer to ADMIN_ADDRESS
///
///   Usage:
///     DEPLOYER_PRIVATE_KEY=0x... \
///     ADMIN_ADDRESS=0x... \
///     STAKING_TOKEN=0x... \
///     FEE_RECIPIENT=0x... \
///     forge script script/DeployWorldChainMainnet.s.sol:DeployWorldChainMainnet \
///       --rpc-url $WORLD_CHAIN_RPC_URL --broadcast --verify \
///       --etherscan-api-key $WORLDSCAN_API_KEY -vvv --slow
///
///   Required env vars:
///     DEPLOYER_PRIVATE_KEY       -- deployer private key (used only for deployment)
///     ADMIN_ADDRESS              -- multisig/timelock that receives ownership
///     STAKING_TOKEN              -- production ERC-20 staking token address
///     FEE_RECIPIENT              -- protocol fee recipient address
///
///   Optional env vars:
///     MIN_STAKE                  -- minimum prover stake in wei (default: 100e18)
///     SLASH_BPS                  -- slash basis points (default: 500 = 5%)
///     SKIP_CIRCUIT_REGISTRATION  -- set to "true" to skip DAG circuit registration
///     ENCLAVE_KEY                -- TEE enclave key to register
///     ENCLAVE_IMAGE_HASH         -- TEE enclave image hash (requires ENCLAVE_KEY)
contract DeployWorldChainMainnet is Script {
    uint256 constant EXPECTED_CHAIN_ID = 480;

    struct DeployParams {
        uint256 deployerKey;
        address deployer;
        address adminAddr;
        address stakingToken;
        address feeRecipient;
        uint256 minStake;
        uint256 slashBps;
        bool skipCircuit;
    }

    struct Contracts {
        RemainderVerifier remainder;
        ProgramRegistry programRegistry;
        ProverReputation reputation;
        ProverRegistry proverRegistry;
        ExecutionEngine engine;
        TEEMLVerifier teeVerifier;
    }

    function run() external {
        DeployParams memory p;
        p.deployerKey = vm.envUint("DEPLOYER_PRIVATE_KEY");
        p.deployer = vm.addr(p.deployerKey);
        p.adminAddr = vm.envAddress("ADMIN_ADDRESS");
        p.stakingToken = vm.envAddress("STAKING_TOKEN");
        p.feeRecipient = vm.envAddress("FEE_RECIPIENT");
        p.minStake = vm.envOr("MIN_STAKE", uint256(100 ether));
        p.slashBps = vm.envOr("SLASH_BPS", uint256(500));
        p.skipCircuit = vm.envOr("SKIP_CIRCUIT_REGISTRATION", false);

        _validateParams(p);

        vm.startBroadcast(p.deployerKey);

        Contracts memory c = _deployAll(p);
        _wireContracts(c);
        _optionalSetup(c, p);
        _transferOwnership(c, p.adminAddr);

        vm.stopBroadcast();

        _printSummary(c);
    }

    function _validateParams(DeployParams memory p) private view {
        require(block.chainid == EXPECTED_CHAIN_ID, "Expected World Chain Mainnet (chainId 480)");
        require(p.adminAddr != address(0), "ADMIN_ADDRESS must be set for mainnet");
        require(p.adminAddr != p.deployer, "ADMIN_ADDRESS must differ from deployer (use multisig/timelock)");
        require(p.stakingToken != address(0), "STAKING_TOKEN must be set for mainnet");
        require(p.feeRecipient != address(0), "FEE_RECIPIENT must be set for mainnet");

        require(p.stakingToken.code.length > 0, "STAKING_TOKEN has no code deployed");

        console.log("=== WORLD CHAIN MAINNET DEPLOYMENT ===");
        console.log("Chain ID:       ", block.chainid);
        console.log("Deployer:       ", p.deployer);
        console.log("Admin (owner):  ", p.adminAddr);
        console.log("Fee Recipient:  ", p.feeRecipient);
        console.log("Staking Token:  ", p.stakingToken);
        console.log("Min Stake:      ", p.minStake);
        console.log("Slash BPS:      ", p.slashBps);
        console.log("");
    }

    function _deployAll(DeployParams memory p) private returns (Contracts memory c) {
        c.remainder = _deployRemainder(p.deployer);
        console.log("[1/6] RemainderVerifier:   ", address(c.remainder));

        c.programRegistry = new ProgramRegistry(p.deployer);
        console.log("[2/6] ProgramRegistry:     ", address(c.programRegistry));

        c.reputation = new ProverReputation();
        console.log("[3/6] ProverReputation:    ", address(c.reputation));

        c.proverRegistry = new ProverRegistry(p.stakingToken, p.minStake, p.slashBps);
        console.log("[4/6] ProverRegistry:      ", address(c.proverRegistry));

        c.engine = new ExecutionEngine(p.deployer, address(c.programRegistry), address(c.remainder), p.feeRecipient);
        console.log("[5/6] ExecutionEngine:     ", address(c.engine));

        c.teeVerifier = _deployTEE(p.deployer, address(c.remainder));
        console.log("[6/6] TEEMLVerifier:       ", address(c.teeVerifier));
    }

    function _wireContracts(Contracts memory c) private {
        console.log("");
        console.log("Wiring contracts...");

        c.engine.setReputation(address(c.reputation));
        c.reputation.authorizeReporter(address(c.engine));
        console.log("  Engine <-> Reputation: linked");

        c.proverRegistry.setSlasher(address(c.engine), true);
        console.log("  Engine -> ProverRegistry: slasher authorized");
    }

    function _optionalSetup(Contracts memory c, DeployParams memory p) private {
        if (!p.skipCircuit) {
            _registerDAGCircuit(c.remainder);
        } else {
            console.log("  Skipping DAG circuit registration");
        }

        address enclaveKey = vm.envOr("ENCLAVE_KEY", address(0));
        if (enclaveKey != address(0)) {
            bytes32 imageHash = vm.envOr("ENCLAVE_IMAGE_HASH", bytes32(0));
            c.teeVerifier.registerEnclave(enclaveKey, imageHash);
            console.log("  Enclave registered:     ", enclaveKey);
        }
    }

    function _transferOwnership(Contracts memory c, address adminAddr) private {
        console.log("");
        console.log("Transferring ownership to admin...");

        c.remainder.changeAdmin(adminAddr);
        c.programRegistry.transferOwnership(adminAddr);
        c.reputation.transferOwnership(adminAddr);
        c.proverRegistry.transferOwnership(adminAddr);
        c.engine.transferOwnership(adminAddr);
        c.teeVerifier.changeAdmin(adminAddr);

        console.log("  Ownership transfer initiated for all 6 contracts");
    }

    function _printSummary(Contracts memory c) private pure {
        console.log("");
        console.log("=== WORLD CHAIN MAINNET DEPLOYMENT COMPLETE ===");
        console.log("");
        console.log("Contract Addresses (save these!):");
        console.log("  RemainderVerifier:", address(c.remainder));
        console.log("  ProgramRegistry:  ", address(c.programRegistry));
        console.log("  ProverReputation: ", address(c.reputation));
        console.log("  ProverRegistry:   ", address(c.proverRegistry));
        console.log("  ExecutionEngine:  ", address(c.engine));
        console.log("  TEEMLVerifier:    ", address(c.teeVerifier));
        console.log("");
        console.log("Post-deployment checklist:");
        console.log("  1. Admin calls acceptOwnership() on all 6 contracts");
        console.log("  2. Admin pauses contracts until circuit registration is verified");
        console.log("  3. Register ML programs via ProgramRegistry.registerProgram(...)");
        console.log("  4. Admin calls unpause() to enable operations");
        console.log("  5. Provers approve staking tokens and register via ProverRegistry");
        console.log("  6. Monitor events: ProofVerified, DAGProofVerified, ExecutionCompleted");
    }

    // ── DAG Circuit Registration ─────────────────────────────────────────────

    function _registerDAGCircuit(RemainderVerifier verifier) internal {
        string memory json = vm.readFile("test/fixtures/phase1a_dag_fixture.json");
        bytes memory gensHex = vm.parseJsonBytes(json, ".gens_hex");
        bytes32 circuitHash = vm.parseJsonBytes32(json, ".circuit_hash_raw");

        GKRDAGVerifier.DAGCircuitDescription memory desc = _parseDAGDesc(json);

        console.log("  DAG fixture loaded:");
        console.log("    Compute layers:", desc.numComputeLayers);
        console.log("    Input layers:  ", desc.numInputLayers);

        verifier.registerDAGCircuit(circuitHash, abi.encode(desc), "XGBoost-Phase1a", keccak256(gensHex));
        console.log("  DAG circuit registered (hash:", vm.toString(circuitHash), ")");
    }

    function _parseDAGDesc(string memory json) private pure returns (GKRDAGVerifier.DAGCircuitDescription memory desc) {
        desc.numComputeLayers = vm.parseJsonUint(json, ".dag_circuit_description.numComputeLayers");
        desc.numInputLayers = vm.parseJsonUint(json, ".dag_circuit_description.numInputLayers");
        desc.layerTypes = _parseUint8Array(json, ".dag_circuit_description.layerTypes");
        desc.numSumcheckRounds = vm.parseJsonUintArray(json, ".dag_circuit_description.numSumcheckRounds");
        desc.atomOffsets = vm.parseJsonUintArray(json, ".dag_circuit_description.atomOffsets");
        desc.atomTargetLayers = vm.parseJsonUintArray(json, ".dag_circuit_description.atomTargetLayers");
        desc.atomCommitIdxs = vm.parseJsonUintArray(json, ".dag_circuit_description.atomCommitIdxs");
        desc.ptOffsets = vm.parseJsonUintArray(json, ".dag_circuit_description.ptOffsets");
        desc.ptData = vm.parseJsonUintArray(json, ".dag_circuit_description.ptData");
        desc.inputIsCommitted = _parseBoolArray(json, ".dag_circuit_description.inputIsCommitted");
        desc.oracleProductOffsets = vm.parseJsonUintArray(json, ".dag_circuit_description.oracleProductOffsets");
        desc.oracleResultIdxs = vm.parseJsonUintArray(json, ".dag_circuit_description.oracleResultIdxs");
        desc.oracleExprCoeffs = _parseUint256Array(json, ".dag_circuit_description.oracleExprCoeffs");
    }

    // ── Deployment Helpers ──────────────────────────────────────────────────

    function _deployRemainder(address admin) private returns (RemainderVerifier) {
        RemainderVerifier impl = new RemainderVerifier();
        UUPSProxy proxy = new UUPSProxy(address(impl), abi.encodeCall(RemainderVerifier.initialize, (admin)));
        return RemainderVerifier(address(proxy));
    }

    function _deployTEE(address admin, address remainderAddr) private returns (TEEMLVerifier) {
        TEEMLVerifier impl = new TEEMLVerifier();
        UUPSProxy proxy = new UUPSProxy(address(impl), abi.encodeCall(TEEMLVerifier.initialize, (admin, remainderAddr)));
        return TEEMLVerifier(payable(address(proxy)));
    }

    // ── Parsing Helpers ──────────────────────────────────────────────────────

    function _parseUint8Array(string memory json, string memory key) private pure returns (uint8[] memory result) {
        uint256[] memory raw = vm.parseJsonUintArray(json, key);
        result = new uint8[](raw.length);
        for (uint256 i = 0; i < raw.length; i++) {
            result[i] = uint8(raw[i]);
        }
    }

    function _parseBoolArray(string memory json, string memory key) private pure returns (bool[] memory result) {
        bytes memory raw = vm.parseJson(json, key);
        result = abi.decode(raw, (bool[]));
    }

    function _parseUint256Array(string memory json, string memory key) private pure returns (uint256[] memory result) {
        bytes memory raw = vm.parseJson(json, key);
        bytes32[] memory parsed = abi.decode(raw, (bytes32[]));
        result = new uint256[](parsed.length);
        for (uint256 i = 0; i < parsed.length; i++) {
            result[i] = uint256(parsed[i]);
        }
    }
}
