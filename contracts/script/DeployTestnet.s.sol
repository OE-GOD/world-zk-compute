// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/ProgramRegistry.sol";
import "../src/ProverReputation.sol";
import "../src/ExecutionEngine.sol";
import "../src/remainder/RemainderVerifier.sol";
import {UUPSProxy} from "../src/Upgradeable.sol";
import "../src/remainder/GKRDAGVerifier.sol";
import "../src/tee/TEEMLVerifier.sol";

/// @title DeployTestnet
/// @notice Deploy the full verifiable AI stack to any testnet (Arbitrum Sepolia, etc.).
/// @dev Testnet-specific simplifications vs. mainnet:
///      - No chain ID check (works on any testnet or local Anvil)
///      - Deployer is admin (no separate ADMIN_ADDRESS required)
///      - No staking token required (uses address(0) placeholder)
///      - No 2-step ownership transfer
///
///   Usage:
///     DEPLOYER_PRIVATE_KEY=0x... \
///     forge script script/DeployTestnet.s.sol:DeployTestnet \
///       --rpc-url "$ARBITRUM_SEPOLIA_RPC_URL" --broadcast -vvv \
///       --code-size-limit 200000
///
///   Required env vars:
///     DEPLOYER_PRIVATE_KEY       -- deployer private key (also becomes admin)
///
///   Optional env vars:
///     FEE_RECIPIENT              -- protocol fee recipient (default: deployer)
///     RISC_ZERO_VERIFIER         -- RISC Zero verifier address (default: deployer as placeholder)
///     SKIP_CIRCUIT_REGISTRATION  -- set to "true" to skip DAG circuit registration
///     ENCLAVE_KEY                -- TEE enclave key to register
///     ENCLAVE_IMAGE_HASH         -- TEE enclave image hash (requires ENCLAVE_KEY)
contract DeployTestnet is Script {
    struct DeployParams {
        uint256 deployerKey;
        address deployer;
        address feeRecipient;
        address riscZeroVerifier;
        bool skipCircuit;
    }

    struct Contracts {
        RemainderVerifier remainder;
        ProgramRegistry programRegistry;
        ProverReputation reputation;
        ExecutionEngine engine;
        TEEMLVerifier teeVerifier;
    }

    function run() external {
        DeployParams memory p;
        p.deployerKey = vm.envUint("DEPLOYER_PRIVATE_KEY");
        p.deployer = vm.addr(p.deployerKey);
        p.feeRecipient = vm.envOr("FEE_RECIPIENT", p.deployer);
        p.riscZeroVerifier = vm.envOr("RISC_ZERO_VERIFIER", p.deployer);
        p.skipCircuit = vm.envOr("SKIP_CIRCUIT_REGISTRATION", false);

        _printHeader(p);

        vm.startBroadcast(p.deployerKey);

        Contracts memory c = _deployAll(p);
        _wireContracts(c);
        _optionalSetup(c, p);

        vm.stopBroadcast();

        _postDeployCheck(c, p);
        _printSummary(c, p);
    }

    // ── Deploy All Contracts ─────────────────────────────────────────────────

    function _deployAll(DeployParams memory p) private returns (Contracts memory c) {
        // 1. RemainderVerifier (UUPS proxy)
        c.remainder = _deployRemainder(p.deployer);
        console.log("[1/5] RemainderVerifier:   ", address(c.remainder));

        // 2. ProgramRegistry (deployer is owner)
        c.programRegistry = new ProgramRegistry(p.deployer);
        console.log("[2/5] ProgramRegistry:     ", address(c.programRegistry));

        // 3. ProverReputation
        c.reputation = new ProverReputation();
        console.log("[3/5] ProverReputation:    ", address(c.reputation));

        // 4. ExecutionEngine (deployer is owner)
        c.engine = new ExecutionEngine(p.deployer, address(c.programRegistry), p.riscZeroVerifier, p.feeRecipient);
        console.log("[4/5] ExecutionEngine:     ", address(c.engine));

        // 5. TEEMLVerifier (UUPS proxy, linked to RemainderVerifier)
        c.teeVerifier = _deployTEE(p.deployer, address(c.remainder));
        console.log("[5/5] TEEMLVerifier:       ", address(c.teeVerifier));
    }

    // ── Wire Contracts Together ──────────────────────────────────────────────

    function _wireContracts(Contracts memory c) private {
        console.log("");
        console.log("Wiring contracts...");

        c.engine.setReputation(address(c.reputation));
        c.reputation.authorizeReporter(address(c.engine));
        console.log("  Engine <-> Reputation: linked");
    }

    // ── Optional Setup ───────────────────────────────────────────────────────

    function _optionalSetup(Contracts memory c, DeployParams memory p) private {
        // DAG circuit registration
        if (!p.skipCircuit) {
            _registerDAGCircuit(c.remainder);
        } else {
            console.log("  Skipping DAG circuit registration (SKIP_CIRCUIT_REGISTRATION=true)");
        }

        // TEE enclave registration
        address enclaveKey = vm.envOr("ENCLAVE_KEY", address(0));
        if (enclaveKey != address(0)) {
            bytes32 imageHash = vm.envOr("ENCLAVE_IMAGE_HASH", bytes32(0));
            c.teeVerifier.registerEnclave(enclaveKey, imageHash);
            console.log("  Enclave registered:     ", enclaveKey);
        }
    }

    // ── Post-Deploy Verification ─────────────────────────────────────────────

    function _postDeployCheck(Contracts memory c, DeployParams memory p) private view {
        console.log("");
        console.log("Post-deploy verification:");

        // Verify RemainderVerifier admin
        address remAdmin = c.remainder.admin();
        require(remAdmin == p.deployer, "RemainderVerifier admin mismatch");
        console.log("  RemainderVerifier admin: OK");

        // Verify TEEMLVerifier admin and link
        address teeAdmin = c.teeVerifier.admin();
        require(teeAdmin == p.deployer, "TEEMLVerifier admin mismatch");
        console.log("  TEEMLVerifier admin:     OK");

        address teeRemainder = c.teeVerifier.remainderVerifier();
        require(teeRemainder == address(c.remainder), "TEE -> Remainder link mismatch");
        console.log("  TEE -> Remainder link:   OK");

        // Verify ProgramRegistry owner
        address regOwner = c.programRegistry.owner();
        require(regOwner == p.deployer, "ProgramRegistry owner mismatch");
        console.log("  ProgramRegistry owner:   OK");

        // Verify ExecutionEngine owner
        address engOwner = c.engine.owner();
        require(engOwner == p.deployer, "ExecutionEngine owner mismatch");
        console.log("  ExecutionEngine owner:   OK");
    }

    // ── DAG Circuit Registration ─────────────────────────────────────────────

    function _registerDAGCircuit(RemainderVerifier verifier) internal {
        string memory json = vm.readFile("test/fixtures/phase1a_dag_fixture.json");
        bytes memory gensHex = vm.parseJsonBytes(json, ".gens_hex");
        bytes32 circuitHash = vm.parseJsonBytes32(json, ".circuit_hash_raw");

        GKRDAGVerifier.DAGCircuitDescription memory desc = _parseDAGDesc(json);

        console.log("");
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

    // ── Deployment Helpers ───────────────────────────────────────────────────

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

    // ── Output Helpers ───────────────────────────────────────────────────────

    function _printHeader(DeployParams memory p) private view {
        console.log("=== TESTNET DEPLOYMENT ===");
        console.log("Chain ID:        ", block.chainid);
        console.log("Deployer (admin):", p.deployer);
        console.log("Fee Recipient:   ", p.feeRecipient);
        console.log("RISC Zero:       ", p.riscZeroVerifier);
        console.log("");
    }

    function _printSummary(Contracts memory c, DeployParams memory p) private view {
        console.log("");
        console.log("=== TESTNET DEPLOYMENT COMPLETE ===");
        console.log("");
        console.log("Chain ID:          ", block.chainid);
        console.log("Admin:             ", p.deployer);
        console.log("");
        console.log("Contract Addresses (save these!):");
        console.log("  RemainderVerifier:", address(c.remainder));
        console.log("  ProgramRegistry:  ", address(c.programRegistry));
        console.log("  ProverReputation: ", address(c.reputation));
        console.log("  ExecutionEngine:  ", address(c.engine));
        console.log("  TEEMLVerifier:    ", address(c.teeVerifier));
        console.log("");
        console.log("Next steps:");
        console.log("  1. Register ML programs via ProgramRegistry.registerProgram(...)");
        console.log("  2. Register TEE enclaves via TEEMLVerifier.registerEnclave(...)");
        console.log("  3. Provers can start claiming and proving execution requests");
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
