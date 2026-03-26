// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/remainder/RemainderVerifier.sol";
import {UUPSProxy} from "../src/Upgradeable.sol";
import "../src/remainder/GKRDAGVerifier.sol";
import "../src/tee/TEEMLVerifier.sol";

/// @title StylusDisputeE2E
/// @notice Full TEEMLVerifier dispute lifecycle E2E test on a local Arbitrum devnode
///         using the Stylus (WASM) verification path.
/// @dev Usage:
///   DEPLOYER_KEY=0x... STYLUS_VERIFIER=0x... \
///     forge script script/StylusDisputeE2E.s.sol:StylusDisputeE2E \
///     --rpc-url $RPC_URL --broadcast --gas-limit 500000000 -vvv
contract StylusDisputeE2E is Script {
    // EIP-712 constants (must match TEEMLVerifier)
    bytes32 constant RESULT_TYPEHASH = keccak256("TEEMLResult(bytes32 modelHash,bytes32 inputHash,bytes32 resultHash)");
    bytes32 constant EIP712_DOMAIN_TYPEHASH =
        keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)");
    bytes32 constant NAME_HASH = keccak256("TEEMLVerifier");
    bytes32 constant VERSION_HASH = keccak256("1");

    struct Keys {
        uint256 deployerKey;
        address deployer;
        uint256 challengerKey;
        address challengerAddr;
        uint256 enclaveKey;
        address enclaveAddr;
        address stylusVerifierAddr;
    }

    struct Fixture {
        bytes proofHex;
        bytes gensHex;
        bytes32 circuitHash;
        bytes publicInputsHex;
        bytes descData;
        bytes32 gensHash;
    }

    function run() external {
        Keys memory k;
        k.deployerKey = vm.envUint("DEPLOYER_KEY");
        k.deployer = vm.addr(k.deployerKey);
        k.stylusVerifierAddr = vm.envAddress("STYLUS_VERIFIER");
        k.challengerKey = uint256(keccak256(abi.encodePacked(k.deployerKey, "challenger")));
        k.challengerAddr = vm.addr(k.challengerKey);
        k.enclaveKey = uint256(keccak256(abi.encodePacked(k.deployerKey, "enclave")));
        k.enclaveAddr = vm.addr(k.enclaveKey);

        Fixture memory fix = _loadFixture();

        console.log("=== Stylus Dispute E2E ===");
        console.log("Deployer:", k.deployer);
        console.log("Challenger:", k.challengerAddr);
        console.log("Enclave:", k.enclaveAddr);
        console.log("Proof size:", fix.proofHex.length, "bytes");

        // Phase 3: Deploy Solidity stack
        (, TEEMLVerifier tee) = _deployContracts(k, fix);

        // Phase 4-5: Submit result + challenge
        bytes32 resultId = _submitAndChallenge(k, tee);

        // Phase 6: Resolve dispute via Stylus
        _resolveDispute(k, tee, resultId, fix);

        // Phase 7: Verify outcome
        _verifyOutcome(tee, resultId);
    }

    function _deployContracts(Keys memory k, Fixture memory fix)
        private
        returns (RemainderVerifier remainder, TEEMLVerifier tee)
    {
        console.log("[Phase 3] Deploying Solidity contracts...");
        vm.startBroadcast(k.deployerKey);

        {
            RemainderVerifier remainderImpl = new RemainderVerifier();
            UUPSProxy remainderProxy =
                new UUPSProxy(address(remainderImpl), abi.encodeCall(RemainderVerifier.initialize, (k.deployer)));
            remainder = RemainderVerifier(address(remainderProxy));
        }
        console.log("  RemainderVerifier:", address(remainder));

        remainder.registerDAGCircuit(fix.circuitHash, fix.descData, "XGBoost-Phase1a", fix.gensHash);
        remainder.setDAGStylusVerifier(fix.circuitHash, k.stylusVerifierAddr);

        {
            TEEMLVerifier teeImpl = new TEEMLVerifier();
            UUPSProxy teeProxy = new UUPSProxy(
                address(teeImpl), abi.encodeCall(TEEMLVerifier.initialize, (k.deployer, address(remainder)))
            );
            tee = TEEMLVerifier(payable(address(teeProxy)));
        }
        tee.setUseStylusVerifier(true);
        console.log("  TEEMLVerifier:", address(tee));

        (bool sent,) = k.challengerAddr.call{value: 1 ether}("");
        require(sent, "Failed to fund challenger");

        vm.stopBroadcast();
    }

    function _submitAndChallenge(Keys memory k, TEEMLVerifier tee) private returns (bytes32 resultId) {
        console.log("[Phase 4] Register enclave + Submit result...");

        bytes32 modelHash = keccak256("xgboost-model-weights");
        bytes32 inputHash = keccak256("test-input-data");
        bytes memory resultData = hex"deadbeef";

        vm.startBroadcast(k.deployerKey);
        tee.registerEnclave(k.enclaveAddr, keccak256("test-enclave-image-v1"));
        bytes memory attestation = _signAttestation(modelHash, inputHash, resultData, k.enclaveKey, address(tee));
        resultId = tee.submitResult{value: 0.1 ether}(modelHash, inputHash, resultData, attestation);
        console.log("  Result submitted");
        vm.stopBroadcast();

        console.log("[Phase 5] Challenging the result...");
        vm.startBroadcast(k.challengerKey);
        tee.challenge{value: 0.1 ether}(resultId);
        vm.stopBroadcast();
    }

    function _resolveDispute(Keys memory k, TEEMLVerifier tee, bytes32 resultId, Fixture memory fix) private {
        console.log("[Phase 6] Resolving dispute via Stylus verification...");
        require(tee.useStylusVerifier(), "Stylus verifier should be enabled");

        uint256 gasBefore = gasleft();
        vm.startBroadcast(k.deployerKey);
        tee.resolveDispute(resultId, fix.proofHex, fix.circuitHash, fix.publicInputsHex, fix.gensHex);
        vm.stopBroadcast();

        console.log("  Gas used (approx):", gasBefore - gasleft());
    }

    function _verifyOutcome(TEEMLVerifier tee, bytes32 resultId) private view {
        console.log("[Phase 7] Verifying outcome...");

        require(tee.disputeResolved(resultId), "Dispute should be resolved");
        require(tee.disputeProverWon(resultId), "Prover should have won");
        require(tee.isResultValid(resultId), "Result should be valid");

        ITEEMLVerifier.MLResult memory result = tee.getResult(resultId);
        require(result.finalized, "Result should be finalized");
        require(result.challenged, "Result should be marked as challenged");

        console.log("=== STYLUS DISPUTE E2E PASSED ===");
    }

    function _signAttestation(
        bytes32 modelHash,
        bytes32 inputHash,
        bytes memory resultData,
        uint256 signerKey,
        address verifierAddr
    ) private view returns (bytes memory) {
        bytes32 resultHash = keccak256(resultData);
        bytes32 structHash = keccak256(abi.encode(RESULT_TYPEHASH, modelHash, inputHash, resultHash));
        bytes32 domainSep =
            keccak256(abi.encode(EIP712_DOMAIN_TYPEHASH, NAME_HASH, VERSION_HASH, block.chainid, verifierAddr));
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", domainSep, structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(signerKey, digest);
        return abi.encodePacked(r, s, v);
    }

    function _loadFixture() private view returns (Fixture memory fix) {
        string memory json = vm.readFile("test/fixtures/phase1a_dag_fixture.json");
        fix.proofHex = vm.parseJsonBytes(json, ".proof_hex");
        fix.gensHex = vm.parseJsonBytes(json, ".gens_hex");
        fix.circuitHash = vm.parseJsonBytes32(json, ".circuit_hash_raw");
        fix.publicInputsHex = vm.parseJsonBytes(json, ".public_inputs_hex");
        fix.gensHash = keccak256(fix.gensHex);

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
        fix.descData = abi.encode(desc);
    }

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
