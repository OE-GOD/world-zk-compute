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

/// @title DeployMainnet
/// @notice Deploy the full World ZK Compute stack to Arbitrum One (chainId 42161).
/// @dev Production deployment script with safety checks:
///      - Validates chain ID (must be 42161)
///      - Requires explicit ADMIN_ADDRESS (no deployer-as-admin)
///      - Requires RISC_ZERO_VERIFIER (no placeholder)
///      - Requires FEE_RECIPIENT
///      - 2-step ownership transfer to ADMIN_ADDRESS
///      - Struct pattern to avoid stack-too-deep
///
///   Usage:
///     DEPLOYER_PRIVATE_KEY=0x... \
///     ADMIN_ADDRESS=0x... \
///     RISC_ZERO_VERIFIER=0x... \
///     FEE_RECIPIENT=0x... \
///     forge script script/DeployMainnet.s.sol:DeployMainnet \
///       --rpc-url $ARBITRUM_ONE_RPC_URL --broadcast --verify \
///       --etherscan-api-key $ARBISCAN_API_KEY -vvv --slow \
///       --code-size-limit 200000
///
///   Required env vars:
///     DEPLOYER_PRIVATE_KEY       -- deployer private key (used only for deployment)
///     ADMIN_ADDRESS              -- multisig/timelock that receives ownership
///     RISC_ZERO_VERIFIER         -- RISC Zero verifier router on Arbitrum One
///     FEE_RECIPIENT              -- protocol fee recipient address
///
///   Optional env vars:
///     STAKING_TOKEN              -- ERC-20 staking token address (default: address(0) = no staking)
///     MIN_STAKE                  -- minimum prover stake in wei (default: 100e18)
///     SLASH_BPS                  -- slash basis points (default: 500 = 5%)
///     SKIP_CIRCUIT_REGISTRATION  -- set to "true" to skip DAG circuit registration
///     ENCLAVE_KEY                -- TEE enclave key to register
///     ENCLAVE_IMAGE_HASH         -- TEE enclave image hash (requires ENCLAVE_KEY)
contract DeployMainnet is Script {
    uint256 constant EXPECTED_CHAIN_ID = 42161;

    struct DeployParams {
        uint256 deployerKey;
        address deployer;
        address adminAddr;
        address riscZeroVerifier;
        address feeRecipient;
        address stakingToken;
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
        DeployParams memory p = _loadParams();

        _validateParams(p);

        vm.startBroadcast(p.deployerKey);

        Contracts memory c = _deployAll(p);
        _wireContracts(c);
        _optionalSetup(c, p);
        _transferOwnership(c, p.adminAddr);

        vm.stopBroadcast();

        _postDeployCheck(c, p);
        _printSummary(c, p);
    }

    // -- Load Parameters -------------------------------------------------------

    function _loadParams() private view returns (DeployParams memory p) {
        p.deployerKey = vm.envUint("DEPLOYER_PRIVATE_KEY");
        p.deployer = vm.addr(p.deployerKey);
        p.adminAddr = vm.envAddress("ADMIN_ADDRESS");
        p.riscZeroVerifier = vm.envAddress("RISC_ZERO_VERIFIER");
        p.feeRecipient = vm.envAddress("FEE_RECIPIENT");
        p.stakingToken = vm.envOr("STAKING_TOKEN", address(0));
        p.minStake = vm.envOr("MIN_STAKE", uint256(100 ether));
        p.slashBps = vm.envOr("SLASH_BPS", uint256(500));
        p.skipCircuit = vm.envOr("SKIP_CIRCUIT_REGISTRATION", false);
    }

    // -- Validate Parameters ---------------------------------------------------

    function _validateParams(DeployParams memory p) private view {
        // Chain ID guard
        require(block.chainid == EXPECTED_CHAIN_ID, "Expected Arbitrum One (chainId 42161)");

        // Admin safety checks
        require(p.adminAddr != address(0), "ADMIN_ADDRESS must be set for mainnet");
        require(p.adminAddr != p.deployer, "ADMIN_ADDRESS must differ from deployer (use multisig/timelock)");

        // Verifier and fee recipient must be real contracts/addresses
        require(p.riscZeroVerifier != address(0), "RISC_ZERO_VERIFIER must be set for mainnet");
        require(p.feeRecipient != address(0), "FEE_RECIPIENT must be set for mainnet");

        // Verify RISC Zero verifier has code on-chain
        require(p.riscZeroVerifier.code.length > 0, "RISC_ZERO_VERIFIER has no code deployed");

        // If staking token is set, verify it has code
        if (p.stakingToken != address(0)) {
            require(p.stakingToken.code.length > 0, "STAKING_TOKEN has no code deployed");
        }

        _printHeader(p);
    }

    // -- Deploy All Contracts --------------------------------------------------

    function _deployAll(DeployParams memory p) private returns (Contracts memory c) {
        // 1. RemainderVerifier (UUPS proxy)
        c.remainder = _deployRemainder(p.deployer);
        console.log("[1/6] RemainderVerifier:   ", address(c.remainder));

        // 2. ProgramRegistry (deployer is initial owner, transferred later)
        c.programRegistry = new ProgramRegistry(p.deployer);
        console.log("[2/6] ProgramRegistry:     ", address(c.programRegistry));

        // 3. ProverReputation (deployer is initial owner, transferred later)
        c.reputation = new ProverReputation();
        console.log("[3/6] ProverReputation:    ", address(c.reputation));

        // 4. ProverRegistry (if staking token is set)
        if (p.stakingToken != address(0)) {
            c.proverRegistry = new ProverRegistry(p.stakingToken, p.minStake, p.slashBps);
            console.log("[4/6] ProverRegistry:      ", address(c.proverRegistry));
        } else {
            console.log("[4/6] ProverRegistry:       SKIPPED (no STAKING_TOKEN)");
        }

        // 5. ExecutionEngine (deployer is initial owner, transferred later)
        c.engine =
            new ExecutionEngine(p.deployer, address(c.programRegistry), p.riscZeroVerifier, p.feeRecipient);
        console.log("[5/6] ExecutionEngine:     ", address(c.engine));

        // 6. TEEMLVerifier (UUPS proxy, linked to RemainderVerifier)
        c.teeVerifier = _deployTEE(p.deployer, address(c.remainder));
        console.log("[6/6] TEEMLVerifier:       ", address(c.teeVerifier));
    }

    // -- Wire Contracts Together -----------------------------------------------

    function _wireContracts(Contracts memory c) private {
        console.log("");
        console.log("Wiring contracts...");

        c.engine.setReputation(address(c.reputation));
        c.reputation.authorizeReporter(address(c.engine));
        console.log("  Engine <-> Reputation: linked");

        if (address(c.proverRegistry) != address(0)) {
            c.proverRegistry.setSlasher(address(c.engine), true);
            console.log("  Engine -> ProverRegistry: slasher authorized");
        }
    }

    // -- Optional Setup --------------------------------------------------------

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

    // -- 2-Step Ownership Transfer ---------------------------------------------

    function _transferOwnership(Contracts memory c, address adminAddr) private {
        console.log("");
        console.log("Initiating 2-step ownership transfer to admin...");

        // UUPS contracts: changeAdmin (1-step, admin-only)
        c.remainder.changeAdmin(adminAddr);
        console.log("  RemainderVerifier:  admin changed");

        c.teeVerifier.changeAdmin(adminAddr);
        console.log("  TEEMLVerifier:      admin changed");

        // Ownable2Step contracts: transferOwnership (pending until acceptOwnership)
        c.programRegistry.transferOwnership(adminAddr);
        console.log("  ProgramRegistry:    ownership transfer initiated");

        c.reputation.transferOwnership(adminAddr);
        console.log("  ProverReputation:   ownership transfer initiated");

        c.engine.transferOwnership(adminAddr);
        console.log("  ExecutionEngine:    ownership transfer initiated");

        if (address(c.proverRegistry) != address(0)) {
            c.proverRegistry.transferOwnership(adminAddr);
            console.log("  ProverRegistry:     ownership transfer initiated");
        }

        console.log("");
        console.log("  IMPORTANT: Admin must call acceptOwnership() on Ownable2Step contracts:");
        console.log("    - ProgramRegistry");
        console.log("    - ProverReputation");
        console.log("    - ExecutionEngine");
        if (address(c.proverRegistry) != address(0)) {
            console.log("    - ProverRegistry");
        }
    }

    // -- Post-Deploy Verification ----------------------------------------------

    function _postDeployCheck(Contracts memory c, DeployParams memory p) private view {
        console.log("");
        console.log("Post-deploy verification:");

        // Verify RemainderVerifier admin was transferred
        address remAdmin = c.remainder.admin();
        require(remAdmin == p.adminAddr, "RemainderVerifier admin should be ADMIN_ADDRESS");
        console.log("  RemainderVerifier admin: OK (transferred to admin)");

        // Verify TEEMLVerifier admin was transferred
        address teeAdmin = c.teeVerifier.admin();
        require(teeAdmin == p.adminAddr, "TEEMLVerifier admin should be ADMIN_ADDRESS");
        console.log("  TEEMLVerifier admin:     OK (transferred to admin)");

        // Verify TEE -> Remainder link
        address teeRemainder = c.teeVerifier.remainderVerifier();
        require(teeRemainder == address(c.remainder), "TEE -> Remainder link mismatch");
        console.log("  TEE -> Remainder link:   OK");

        // Verify ProgramRegistry pending owner (still owned by deployer until accepted)
        address regOwner = c.programRegistry.owner();
        require(regOwner == p.deployer, "ProgramRegistry owner should still be deployer (pending transfer)");
        console.log("  ProgramRegistry owner:   OK (pending transfer)");

        // Verify ExecutionEngine pending owner
        address engOwner = c.engine.owner();
        require(engOwner == p.deployer, "ExecutionEngine owner should still be deployer (pending transfer)");
        console.log("  ExecutionEngine owner:   OK (pending transfer)");

        // Verify ExecutionEngine fee recipient
        require(c.engine.feeRecipient() == p.feeRecipient, "ExecutionEngine feeRecipient mismatch");
        console.log("  ExecutionEngine fee:     OK");
    }

    // -- DAG Circuit Registration ----------------------------------------------

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

    function _parseDAGDesc(string memory json)
        private
        pure
        returns (GKRDAGVerifier.DAGCircuitDescription memory desc)
    {
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

    // -- Deployment Helpers ----------------------------------------------------

    function _deployRemainder(address admin) private returns (RemainderVerifier) {
        RemainderVerifier impl = new RemainderVerifier();
        UUPSProxy proxy = new UUPSProxy(address(impl), abi.encodeCall(RemainderVerifier.initialize, (admin)));
        return RemainderVerifier(address(proxy));
    }

    function _deployTEE(address admin, address remainderAddr) private returns (TEEMLVerifier) {
        TEEMLVerifier impl = new TEEMLVerifier();
        UUPSProxy proxy =
            new UUPSProxy(address(impl), abi.encodeCall(TEEMLVerifier.initialize, (admin, remainderAddr)));
        return TEEMLVerifier(payable(address(proxy)));
    }

    // -- Output Helpers --------------------------------------------------------

    function _printHeader(DeployParams memory p) private view {
        console.log("=== ARBITRUM ONE MAINNET DEPLOYMENT ===");
        console.log("Chain ID:         ", block.chainid);
        console.log("Deployer:         ", p.deployer);
        console.log("Admin (owner):    ", p.adminAddr);
        console.log("Fee Recipient:    ", p.feeRecipient);
        console.log("RISC Zero:        ", p.riscZeroVerifier);
        if (p.stakingToken != address(0)) {
            console.log("Staking Token:    ", p.stakingToken);
            console.log("Min Stake:        ", p.minStake);
            console.log("Slash BPS:        ", p.slashBps);
        } else {
            console.log("Staking Token:     NONE (ProverRegistry will be skipped)");
        }
        console.log("");
    }

    function _printSummary(Contracts memory c, DeployParams memory p) private pure {
        console.log("");
        console.log("=== ARBITRUM ONE MAINNET DEPLOYMENT COMPLETE ===");
        console.log("");
        console.log("Contract Addresses (save these!):");
        console.log("  RemainderVerifier:", address(c.remainder));
        console.log("  ProgramRegistry:  ", address(c.programRegistry));
        console.log("  ProverReputation: ", address(c.reputation));
        if (address(c.proverRegistry) != address(0)) {
            console.log("  ProverRegistry:   ", address(c.proverRegistry));
        }
        console.log("  ExecutionEngine:  ", address(c.engine));
        console.log("  TEEMLVerifier:    ", address(c.teeVerifier));
        console.log("");
        console.log("Admin:             ", p.adminAddr);
        console.log("");
        console.log("=== POST-DEPLOYMENT CHECKLIST ===");
        console.log("  1. Admin calls acceptOwnership() on ProgramRegistry, ProverReputation,");
        console.log("     ExecutionEngine", (address(c.proverRegistry) != address(0)) ? ", ProverRegistry" : "");
        console.log("  2. Verify all contract admin/owner fields point to ADMIN_ADDRESS");
        console.log("  3. Admin pauses contracts until configuration is verified");
        console.log("  4. Register ML programs via ProgramRegistry.registerProgram(...)");
        console.log("  5. Register TEE enclaves via TEEMLVerifier.registerEnclave(...)");
        console.log("  6. Admin calls unpause() to enable operations");
        console.log("  7. Provers approve staking tokens and register via ProverRegistry");
        console.log("  8. Monitor events: ProofVerified, DAGProofVerified, ExecutionCompleted");
        console.log("  9. Set timelock on RemainderVerifier and TEEMLVerifier via setTimelock()");
    }

    // -- Parsing Helpers -------------------------------------------------------

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

    function _parseUint256Array(string memory json, string memory key)
        private
        pure
        returns (uint256[] memory result)
    {
        bytes memory raw = vm.parseJson(json, key);
        bytes32[] memory parsed = abi.decode(raw, (bytes32[]));
        result = new uint256[](parsed.length);
        for (uint256 i = 0; i < parsed.length; i++) {
            result[i] = uint256(parsed[i]);
        }
    }
}
