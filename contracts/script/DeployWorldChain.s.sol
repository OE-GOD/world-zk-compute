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

/// @title DeployWorldChain
/// @notice Deploy the full World ZK Compute stack to World Chain Sepolia (chainId 4801).
/// @dev Does NOT deploy a mock verifier -- World Chain Sepolia supports large contracts
///      (200KB code size, 500M gas limit) so the full RemainderVerifier can be deployed.
///
///   Usage:
///     DEPLOYER_PRIVATE_KEY=0x... forge script script/DeployWorldChain.s.sol:DeployWorldChain \
///       --rpc-url $WORLD_CHAIN_RPC_URL --broadcast --verify \
///       --etherscan-api-key $WORLDSCAN_API_KEY -vvv
///
///   Env vars (required):
///     DEPLOYER_PRIVATE_KEY       -- deployer private key
///
///   Env vars (optional):
///     FEE_RECIPIENT              -- fee recipient address (default: deployer)
///     MIN_STAKE                  -- minimum prover stake in wei (default: 100e18)
///     SLASH_BPS                  -- slash basis points (default: 500 = 5%)
///     ADMIN_ADDRESS              -- transfer ownership to this address after deploy (optional)
///     SKIP_CIRCUIT_REGISTRATION  -- set to "true" to skip DAG circuit registration (optional)
///     ENCLAVE_KEY                -- TEE enclave key to register (optional)
///     ENCLAVE_IMAGE_HASH         -- TEE enclave image hash (optional, requires ENCLAVE_KEY)
///     STAKING_TOKEN              -- existing ERC-20 staking token address (default: deploy new)
contract DeployWorldChain is Script {
    uint256 constant EXPECTED_CHAIN_ID = 4801;

    function run() external {
        uint256 deployerKey = vm.envUint("DEPLOYER_PRIVATE_KEY");
        address deployer = vm.addr(deployerKey);
        address feeRecipient = vm.envOr("FEE_RECIPIENT", deployer);
        uint256 minStake = vm.envOr("MIN_STAKE", uint256(100 ether));
        uint256 slashBps = vm.envOr("SLASH_BPS", uint256(500));
        bool skipCircuit = vm.envOr("SKIP_CIRCUIT_REGISTRATION", false);
        address stakingTokenAddr = vm.envOr("STAKING_TOKEN", address(0));

        // Safety check: confirm we are on World Chain Sepolia
        require(block.chainid == EXPECTED_CHAIN_ID, "Expected World Chain Sepolia (chainId 4801)");

        console.log("=== WORLD CHAIN SEPOLIA DEPLOYMENT ===");
        console.log("Chain ID:       ", block.chainid);
        console.log("Deployer:       ", deployer);
        console.log("Fee Recipient:  ", feeRecipient);
        console.log("Min Stake:      ", minStake);
        console.log("Slash BPS:      ", slashBps);
        console.log("");

        vm.startBroadcast(deployerKey);

        // -- 1. Deploy RemainderVerifier --------------------------------------------------
        RemainderVerifier remainder = _deployRemainder(deployer);
        console.log("[1/6] RemainderVerifier:   ", address(remainder));

        // -- 2. Deploy ProgramRegistry ----------------------------------------------------
        ProgramRegistry programRegistry = new ProgramRegistry(deployer);
        console.log("[2/6] ProgramRegistry:     ", address(programRegistry));

        // -- 3. Deploy ProverReputation ---------------------------------------------------
        ProverReputation reputation = new ProverReputation();
        console.log("[3/6] ProverReputation:    ", address(reputation));

        // -- 4. Deploy ProverRegistry -----------------------------------------------------
        //    Use an existing staking token or deploy a placeholder.
        //    Production deployments should set STAKING_TOKEN to the real ERC-20 address.
        address stakeToken;
        if (stakingTokenAddr != address(0)) {
            stakeToken = stakingTokenAddr;
            console.log("[4/6] StakeToken (reused): ", stakeToken);
        } else {
            // Deploy a minimal placeholder token for testnet use
            stakeToken = address(new _PlaceholderToken(1_000_000 ether));
            console.log("[4a]  StakeToken (new):    ", stakeToken);
        }
        ProverRegistry proverRegistry = new ProverRegistry(stakeToken, minStake, slashBps);
        console.log("[4b]  ProverRegistry:      ", address(proverRegistry));

        // -- 5. Deploy ExecutionEngine ----------------------------------------------------
        //    On World Chain Sepolia there is no RISC Zero verifier router yet, so we use
        //    the RemainderVerifier address as a placeholder verifier. The admin can update
        //    this later via setVerifier() once a verifier router is deployed on-chain.
        ExecutionEngine engine =
            new ExecutionEngine(deployer, address(programRegistry), address(remainder), feeRecipient);
        console.log("[5/6] ExecutionEngine:     ", address(engine));

        // -- 6. Deploy TEEMLVerifier (UUPS proxy) ------------------------------------------
        TEEMLVerifier teeVerifier;
        {
            TEEMLVerifier teeImpl = new TEEMLVerifier();
            UUPSProxy teeProxy = new UUPSProxy(
                address(teeImpl), abi.encodeCall(TEEMLVerifier.initialize, (deployer, address(remainder)))
            );
            teeVerifier = TEEMLVerifier(payable(address(teeProxy)));
        }
        console.log("[6/6] TEEMLVerifier:       ", address(teeVerifier));

        // -- Wiring -----------------------------------------------------------------------
        console.log("");
        console.log("Wiring contracts...");

        engine.setReputation(address(reputation));
        reputation.authorizeReporter(address(engine));
        console.log("  Engine <-> Reputation: linked");

        proverRegistry.setSlasher(address(engine), true);
        console.log("  Engine -> ProverRegistry: slasher authorized");

        // -- Optional: Register DAG circuit -----------------------------------------------
        if (!skipCircuit) {
            _registerDAGCircuit(remainder);
        } else {
            console.log("  Skipping DAG circuit registration (SKIP_CIRCUIT_REGISTRATION=true)");
        }

        // -- Optional: Register TEE enclave -----------------------------------------------
        address enclaveKey = vm.envOr("ENCLAVE_KEY", address(0));
        if (enclaveKey != address(0)) {
            bytes32 imageHash = vm.envOr("ENCLAVE_IMAGE_HASH", bytes32(0));
            teeVerifier.registerEnclave(enclaveKey, imageHash);
            console.log("  Enclave registered:     ", enclaveKey);
        }

        // -- Optional: Ownership transfer -------------------------------------------------
        address adminAddr = vm.envOr("ADMIN_ADDRESS", address(0));
        if (adminAddr != address(0) && adminAddr != deployer) {
            remainder.changeAdmin(adminAddr);
            programRegistry.transferOwnership(adminAddr);
            reputation.transferOwnership(adminAddr);
            proverRegistry.transferOwnership(adminAddr);
            engine.transferOwnership(adminAddr);
            teeVerifier.changeAdmin(adminAddr);
            console.log("  Admin transfer completed for all contracts to:", adminAddr);
        }

        vm.stopBroadcast();

        // -- Summary ----------------------------------------------------------------------
        console.log("");
        console.log("=== WORLD CHAIN SEPOLIA DEPLOYMENT COMPLETE ===");
        console.log("");
        console.log("Contract Addresses:");
        console.log("  RemainderVerifier:", address(remainder));
        console.log("  ProgramRegistry:  ", address(programRegistry));
        console.log("  ProverReputation: ", address(reputation));
        console.log("  StakeToken:       ", stakeToken);
        console.log("  ProverRegistry:   ", address(proverRegistry));
        console.log("  ExecutionEngine:  ", address(engine));
        console.log("  TEEMLVerifier:    ", address(teeVerifier));
        console.log("");
        console.log("Next steps:");
        console.log("  1. Update deployments/world-chain-sepolia.json with the addresses above");
        console.log("  2. Register programs via ProgramRegistry.registerProgram(...)");
        console.log("  3. Fund provers and approve staking tokens");
        console.log("  4. Register TEE enclaves if not done via ENCLAVE_KEY env var");
    }

    // -- DAG Circuit Registration ---------------------------------------------------------

    function _registerDAGCircuit(RemainderVerifier verifier) internal {
        string memory json = vm.readFile("test/fixtures/phase1a_dag_fixture.json");
        bytes memory gensHex = vm.parseJsonBytes(json, ".gens_hex");
        bytes32 circuitHash = vm.parseJsonBytes32(json, ".circuit_hash_raw");

        GKRDAGVerifier.DAGCircuitDescription memory desc;
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

        console.log("  DAG fixture loaded:");
        console.log("    Compute layers:", desc.numComputeLayers);
        console.log("    Input layers:  ", desc.numInputLayers);

        bytes32 gensHash = keccak256(gensHex);
        bytes memory descData = abi.encode(desc);
        verifier.registerDAGCircuit(circuitHash, descData, "XGBoost-Phase1a", gensHash);
        console.log("  DAG circuit registered (hash:", vm.toString(circuitHash), ")");
    }

    // -- Deployment Helpers ---------------------------------------------------------------

    function _deployRemainder(address admin) private returns (RemainderVerifier) {
        RemainderVerifier impl = new RemainderVerifier();
        UUPSProxy proxy = new UUPSProxy(address(impl), abi.encodeCall(RemainderVerifier.initialize, (admin)));
        return RemainderVerifier(address(proxy));
    }

    // -- Parsing Helpers ------------------------------------------------------------------

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

/// @dev Minimal ERC-20 for testnet staking. NOT for production -- use a real token.
contract _PlaceholderToken {
    string public name = "World ZK Stake Token";
    string public symbol = "WZKS";
    uint8 public decimals = 18;
    uint256 public totalSupply;
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    constructor(uint256 initialSupply) {
        totalSupply = initialSupply;
        balanceOf[msg.sender] = initialSupply;
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        return true;
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        balanceOf[msg.sender] -= amount;
        balanceOf[to] += amount;
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        allowance[from][msg.sender] -= amount;
        balanceOf[from] -= amount;
        balanceOf[to] += amount;
        return true;
    }
}
