// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/remainder/RemainderVerifier.sol";
import {UUPSProxy} from "../src/Upgradeable.sol";
import "../src/remainder/GKRDAGVerifier.sol";
import "../src/tee/TEEMLVerifier.sol";

/// @title DeployAll
/// @notice Deploy RemainderVerifier + TEEMLVerifier and register DAG circuit.
/// @dev Usage:
///   forge script script/DeployAll.s.sol:DeployAll --rpc-url $RPC --broadcast -vvv
///
///   Env vars:
///     DEPLOYER_KEY               — deployer private key (required)
///     REMAINDER_VERIFIER         — skip RemainderVerifier deploy, use existing (optional)
///     SKIP_CIRCUIT_REGISTRATION  — set to "true" to skip DAG circuit registration (optional)
contract DeployAll is Script {
    function run() external {
        uint256 deployerKey = vm.envUint("DEPLOYER_KEY");
        address deployer = vm.addr(deployerKey);

        // Check if we should reuse an existing RemainderVerifier
        address existingRemainder = vm.envOr("REMAINDER_VERIFIER", address(0));
        bool skipCircuit = vm.envOr("SKIP_CIRCUIT_REGISTRATION", false);

        console.log("Deployer:", deployer);
        console.log("");

        vm.startBroadcast(deployerKey);

        // ── 1. Deploy or reuse RemainderVerifier ─────────────────────────────
        RemainderVerifier remainder;
        if (existingRemainder != address(0)) {
            remainder = RemainderVerifier(payable(existingRemainder));
            console.log("Using existing RemainderVerifier:", existingRemainder);
        } else {
            {
                RemainderVerifier impl = new RemainderVerifier();
                UUPSProxy proxy = new UUPSProxy(address(impl), abi.encodeCall(RemainderVerifier.initialize, (deployer)));
                remainder = RemainderVerifier(address(proxy));
            }
            console.log("RemainderVerifier deployed at:", address(remainder));
        }

        // ── 2. Deploy TEEMLVerifier (UUPS proxy) ──────────────────────────────
        TEEMLVerifier tee;
        {
            TEEMLVerifier teeImpl = new TEEMLVerifier();
            UUPSProxy teeProxy = new UUPSProxy(
                address(teeImpl), abi.encodeCall(TEEMLVerifier.initialize, (deployer, address(remainder)))
            );
            tee = TEEMLVerifier(payable(address(teeProxy)));
        }
        console.log("TEEMLVerifier deployed at:", address(tee));

        // ── 3. Register DAG circuit ──────────────────────────────────────────
        if (!skipCircuit) {
            _registerDAGCircuit(remainder);
        } else {
            console.log("Skipping DAG circuit registration (SKIP_CIRCUIT_REGISTRATION=true)");
        }

        // ── 4. Post-deploy verification ────────────────────────────────────
        console.log("");
        console.log("Post-deploy verification:");
        address teeAdmin = tee.admin();
        console.log("  TEE admin():", teeAdmin);
        require(teeAdmin == deployer, "admin() mismatch: expected deployer");
        console.log("  admin matches deployer: OK");
        console.log("  remainderVerifier():", tee.remainderVerifier());
        require(tee.remainderVerifier() == address(remainder), "remainderVerifier mismatch");
        console.log("  remainderVerifier matches: OK");

        // ── 5. Optional ownership transfer ─────────────────────────────────
        address adminAddr = vm.envOr("ADMIN_ADDRESS", address(0));
        if (adminAddr != address(0) && adminAddr != deployer) {
            tee.changeAdmin(adminAddr);
            console.log("  changeAdmin completed to:", adminAddr);
        }

        vm.stopBroadcast();

        // ── Summary ──────────────────────────────────────────────────────────
        console.log("");
        console.log("=== DeployAll DONE ===");
        console.log("  RemainderVerifier:", address(remainder));
        console.log("  TEEMLVerifier:    ", address(tee));
    }

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

        console.log("DAG fixture loaded:");
        console.log("  Compute layers:", desc.numComputeLayers);
        console.log("  Input layers:", desc.numInputLayers);

        bytes32 gensHash = keccak256(gensHex);
        bytes memory descData = abi.encode(desc);
        verifier.registerDAGCircuit(circuitHash, descData, "XGBoost-Phase1a", gensHash);
        console.log("DAG circuit registered (hash:", vm.toString(circuitHash), ")");
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

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
