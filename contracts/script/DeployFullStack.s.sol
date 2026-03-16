// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/ProgramRegistry.sol";
import "../src/ProverRegistry.sol";
import "../src/ProverReputation.sol";
import "../src/ExecutionEngine.sol";
import "../src/mocks/MockRiscZeroVerifier.sol";
import "../src/tee/TEEMLVerifier.sol";

/// @dev Minimal ERC20 for ProverRegistry staking in local deployments
contract DeployToken {
    string public name = "Stake Token";
    string public symbol = "STK";
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

/// @title DeployFullStack
/// @notice Deploy the complete World ZK Compute system stack for local/testnet
/// @dev Deploys and wires together all core contracts:
///      MockVerifier + ProgramRegistry + ProverReputation + ProverRegistry
///      + ExecutionEngine + TEEMLVerifier
///
///   Usage (local):
///     forge script script/DeployFullStack.s.sol:DeployFullStack \
///       --rpc-url http://localhost:8545 --broadcast -vvv
///
///   Env vars:
///     PRIVATE_KEY           — deployer private key (required)
///     FEE_RECIPIENT         — fee recipient address (default: deployer)
///     MIN_STAKE             — minimum prover stake in wei (default: 100e18)
///     SLASH_BPS             — slash basis points (default: 500 = 5%)
///     REMAINDER_VERIFIER    — existing RemainderVerifier address (default: 0x0 = skip)
///     ENCLAVE_KEY           — TEE enclave key to register (optional)
///     ENCLAVE_IMAGE_HASH    — TEE enclave image hash (optional, requires ENCLAVE_KEY)
contract DeployFullStack is Script {
    function run() external {
        uint256 deployerKey = vm.envUint("PRIVATE_KEY");
        address deployer = vm.addr(deployerKey);
        address feeRecipient = vm.envOr("FEE_RECIPIENT", deployer);
        uint256 minStake = vm.envOr("MIN_STAKE", uint256(100 ether));
        uint256 slashBps = vm.envOr("SLASH_BPS", uint256(500));
        address remainderAddr = vm.envOr("REMAINDER_VERIFIER", address(0));

        console.log("=== FULL STACK DEPLOYMENT ===");
        console.log("Deployer:       ", deployer);
        console.log("Fee Recipient:  ", feeRecipient);
        console.log("Min Stake:      ", minStake);
        console.log("Slash BPS:      ", slashBps);
        console.log("");

        vm.startBroadcast(deployerKey);

        // ── 1. Deploy Mock Verifier ────────────────────────────────────────────
        MockRiscZeroVerifier mockVerifier = new MockRiscZeroVerifier();
        console.log("[1/7] MockRiscZeroVerifier:", address(mockVerifier));

        // ── 2. Deploy ProgramRegistry ──────────────────────────────────────────
        ProgramRegistry programRegistry = new ProgramRegistry(deployer);
        console.log("[2/7] ProgramRegistry:     ", address(programRegistry));

        // ── 3. Deploy ProverReputation ─────────────────────────────────────────
        ProverReputation reputation = new ProverReputation();
        console.log("[3/7] ProverReputation:    ", address(reputation));

        // ── 4. Deploy Stake Token + ProverRegistry ─────────────────────────────
        DeployToken stakeToken = new DeployToken(1_000_000 ether);
        console.log("[4a]  StakeToken:          ", address(stakeToken));

        ProverRegistry proverRegistry = new ProverRegistry(address(stakeToken), minStake, slashBps);
        console.log("[4b]  ProverRegistry:      ", address(proverRegistry));

        // ── 5. Deploy ExecutionEngine ──────────────────────────────────────────
        ExecutionEngine engine =
            new ExecutionEngine(deployer, address(programRegistry), address(mockVerifier), feeRecipient);
        console.log("[5/7] ExecutionEngine:     ", address(engine));

        // ── 6. Deploy TEEMLVerifier ────────────────────────────────────────────
        TEEMLVerifier teeVerifier = new TEEMLVerifier(deployer, remainderAddr);
        console.log("[6/7] TEEMLVerifier:       ", address(teeVerifier));

        // ── 7. Wire contracts together ─────────────────────────────────────────
        console.log("");
        console.log("Wiring contracts...");

        // ExecutionEngine <-> ProverReputation
        engine.setReputation(address(reputation));
        reputation.authorizeReporter(address(engine));
        console.log("  Engine -> Reputation: linked");

        // ProverRegistry: authorize engine as slasher
        proverRegistry.setSlasher(address(engine), true);
        console.log("  Engine -> ProverRegistry: slasher authorized");

        // ── 8. Optional: Register TEE enclave ──────────────────────────────────
        address enclaveKey = vm.envOr("ENCLAVE_KEY", address(0));
        if (enclaveKey != address(0)) {
            bytes32 imageHash = vm.envOr("ENCLAVE_IMAGE_HASH", bytes32(0));
            teeVerifier.registerEnclave(enclaveKey, imageHash);
            console.log("  Enclave registered:     ", enclaveKey);
        }

        vm.stopBroadcast();

        // ── Summary ────────────────────────────────────────────────────────────
        console.log("");
        console.log("=== DEPLOYMENT COMPLETE ===");
        console.log("");
        console.log("Contract Addresses:");
        console.log("  MockRiscZeroVerifier:", address(mockVerifier));
        console.log("  ProgramRegistry:     ", address(programRegistry));
        console.log("  ProverReputation:    ", address(reputation));
        console.log("  StakeToken:          ", address(stakeToken));
        console.log("  ProverRegistry:      ", address(proverRegistry));
        console.log("  ExecutionEngine:     ", address(engine));
        console.log("  TEEMLVerifier:       ", address(teeVerifier));
        console.log("");
        console.log("Next steps:");
        console.log(
            "  1. Register programs: cast send <ProgramRegistry> 'registerProgram(bytes32,string,string,bytes32)' ..."
        );
        console.log("  2. Register provers:  Approve STK tokens, then call register(stake, endpoint)");
        console.log("  3. Fund requester:    Send ETH to requester address for tips");
        console.log("  4. Submit requests:   cast send <ExecutionEngine> 'requestExecution(...)' --value 0.01ether ...");
    }
}
