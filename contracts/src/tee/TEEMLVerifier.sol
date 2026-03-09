// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import {ITEEMLVerifier} from "./ITEEMLVerifier.sol";
import {ECDSA} from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";

/// @title TEEMLVerifier — TEE-attested ML inference with ZK dispute resolution
/// @notice Happy path: verify ECDSA attestation from TEE enclave (~3K gas).
///         Dispute path: fall back to existing RemainderVerifier DAG proof verification.
contract TEEMLVerifier is ITEEMLVerifier {
    using ECDSA for bytes32;

    address public admin;
    address public remainderVerifier;

    uint256 public constant CHALLENGE_WINDOW = 1 hours;
    uint256 public constant DISPUTE_WINDOW = 24 hours;

    uint256 public challengeBondAmount = 0.1 ether;
    uint256 public proverStake = 0.1 ether;

    mapping(address => EnclaveInfo) public enclaves;
    mapping(bytes32 => MLResult) internal _results;
    mapping(bytes32 => bool) public disputeResolved;
    mapping(bytes32 => bool) public disputeProverWon;

    modifier onlyAdmin() {
        require(msg.sender == admin, "TEEMLVerifier: not admin");
        _;
    }

    constructor(address _admin, address _remainderVerifier) {
        require(_admin != address(0), "TEEMLVerifier: zero admin");
        admin = _admin;
        remainderVerifier = _remainderVerifier;
    }

    // ─── Admin ───────────────────────────────────────────────────────────────

    function registerEnclave(address enclaveKey, bytes32 enclaveImageHash) external onlyAdmin {
        require(enclaveKey != address(0), "TEEMLVerifier: zero enclave key");
        require(!enclaves[enclaveKey].registered, "TEEMLVerifier: already registered");

        enclaves[enclaveKey] = EnclaveInfo({
            registered: true, active: true, enclaveImageHash: enclaveImageHash, registeredAt: block.timestamp
        });

        emit EnclaveRegistered(enclaveKey, enclaveImageHash);
    }

    function revokeEnclave(address enclaveKey) external onlyAdmin {
        require(enclaves[enclaveKey].registered, "TEEMLVerifier: not registered");
        require(enclaves[enclaveKey].active, "TEEMLVerifier: already revoked");

        enclaves[enclaveKey].active = false;

        emit EnclaveRevoked(enclaveKey);
    }

    function setRemainderVerifier(address _verifier) external onlyAdmin {
        remainderVerifier = _verifier;
    }

    function setChallengeBondAmount(uint256 _amount) external onlyAdmin {
        challengeBondAmount = _amount;
    }

    function setProverStake(uint256 _amount) external onlyAdmin {
        proverStake = _amount;
    }

    // ─── Submit ──────────────────────────────────────────────────────────────

    function submitResult(bytes32 modelHash, bytes32 inputHash, bytes calldata result, bytes calldata attestation)
        external
        payable
        returns (bytes32 resultId)
    {
        require(msg.value >= proverStake, "TEEMLVerifier: insufficient stake");

        bytes32 resultHash = keccak256(result);
        bytes32 message = keccak256(abi.encodePacked(modelHash, inputHash, resultHash));
        bytes32 ethSignedHash = _toEthSignedMessageHash(message);

        address signer = ethSignedHash.recover(attestation);
        require(enclaves[signer].registered, "TEEMLVerifier: unregistered enclave");
        require(enclaves[signer].active, "TEEMLVerifier: enclave revoked");

        resultId = keccak256(abi.encodePacked(msg.sender, modelHash, inputHash, block.number));
        require(_results[resultId].submittedAt == 0, "TEEMLVerifier: result exists");

        _results[resultId] = MLResult({
            enclave: signer,
            submitter: msg.sender,
            modelHash: modelHash,
            inputHash: inputHash,
            resultHash: resultHash,
            result: result,
            submittedAt: block.timestamp,
            challengeDeadline: block.timestamp + CHALLENGE_WINDOW,
            disputeDeadline: 0,
            challengeBond: 0,
            proverStakeAmount: msg.value,
            finalized: false,
            challenged: false,
            challenger: address(0)
        });

        emit ResultSubmitted(resultId, modelHash, inputHash);
    }

    // ─── Challenge ───────────────────────────────────────────────────────────

    function challenge(bytes32 resultId) external payable {
        MLResult storage r = _results[resultId];
        require(r.submittedAt != 0, "TEEMLVerifier: result not found");
        require(!r.finalized, "TEEMLVerifier: already finalized");
        require(!r.challenged, "TEEMLVerifier: already challenged");
        require(msg.value >= challengeBondAmount, "TEEMLVerifier: insufficient bond");
        require(block.timestamp < r.challengeDeadline, "TEEMLVerifier: window closed");

        r.challenged = true;
        r.challenger = msg.sender;
        r.challengeBond = msg.value;
        r.disputeDeadline = block.timestamp + DISPUTE_WINDOW;

        emit ResultChallenged(resultId, msg.sender);
    }

    // ─── Dispute Resolution ──────────────────────────────────────────────────

    /// @notice Resolve a dispute by submitting a ZK proof via the existing RemainderVerifier.
    ///         If the ZK proof verifies, the TEE result was correct — prover wins both stakes.
    ///         If the ZK proof does not verify, the TEE result was wrong — challenger wins both stakes.
    function resolveDispute(
        bytes32 resultId,
        bytes calldata proof,
        bytes32 circuitHash,
        bytes calldata publicInputs,
        bytes calldata gensData
    ) external {
        MLResult storage r = _results[resultId];
        require(r.challenged, "TEEMLVerifier: not challenged");
        require(!disputeResolved[resultId], "TEEMLVerifier: already resolved");
        require(remainderVerifier != address(0), "TEEMLVerifier: no verifier set");

        // Call the existing single-tx DAG proof verification
        // This requires >254M gas (supported on Arbitrum in a single tx)
        (bool success, bytes memory returnData) = remainderVerifier.staticcall(
            abi.encodeWithSignature(
                "verifyDAGProof(bytes,bytes32,bytes,bytes)", proof, circuitHash, publicInputs, gensData
            )
        );

        bool proofValid = false;
        if (success && returnData.length >= 32) {
            proofValid = abi.decode(returnData, (bool));
        }

        _settleDispute(resultId, r, proofValid);
    }

    /// @notice Resolve a dispute by timeout. If the prover fails to submit a ZK proof
    ///         within the dispute window, the challenger wins by default.
    function resolveDisputeByTimeout(bytes32 resultId) external {
        MLResult storage r = _results[resultId];
        require(r.challenged, "TEEMLVerifier: not challenged");
        require(!disputeResolved[resultId], "TEEMLVerifier: already resolved");
        require(block.timestamp >= r.disputeDeadline, "TEEMLVerifier: deadline not reached");

        _settleDispute(resultId, r, false);
    }

    // ─── Finalize ────────────────────────────────────────────────────────────

    function finalize(bytes32 resultId) external {
        MLResult storage r = _results[resultId];
        require(r.submittedAt != 0, "TEEMLVerifier: result not found");
        require(block.timestamp >= r.challengeDeadline, "TEEMLVerifier: window not passed");
        require(!r.challenged, "TEEMLVerifier: result is challenged");
        require(!r.finalized, "TEEMLVerifier: already finalized");

        r.finalized = true;

        // Return prover stake to submitter
        if (r.proverStakeAmount > 0) {
            (bool sent,) = r.submitter.call{value: r.proverStakeAmount}("");
            require(sent, "TEEMLVerifier: stake return failed");
        }

        emit ResultFinalized(resultId);
    }

    // ─── Query ───────────────────────────────────────────────────────────────

    function getResult(bytes32 resultId) external view returns (MLResult memory) {
        return _results[resultId];
    }

    function isResultValid(bytes32 resultId) external view returns (bool) {
        MLResult storage r = _results[resultId];
        if (r.finalized && !r.challenged) {
            return true;
        }
        if (r.challenged && disputeResolved[resultId] && disputeProverWon[resultId]) {
            return true;
        }
        return false;
    }

    // ─── Internal ────────────────────────────────────────────────────────────

    function _settleDispute(bytes32 resultId, MLResult storage r, bool proofValid) internal {
        disputeResolved[resultId] = true;
        disputeProverWon[resultId] = proofValid;

        uint256 totalPot = r.challengeBond + r.proverStakeAmount;

        if (proofValid) {
            // Prover was honest — submitter wins both stakes
            (bool sent,) = r.submitter.call{value: totalPot}("");
            require(sent, "TEEMLVerifier: payout failed");
        } else {
            // Prover was dishonest — challenger wins both stakes
            (bool sent,) = r.challenger.call{value: totalPot}("");
            require(sent, "TEEMLVerifier: payout failed");
        }

        emit DisputeResolved(resultId, proofValid);
    }

    function _toEthSignedMessageHash(bytes32 hash) internal pure returns (bytes32) {
        return keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", hash));
    }

    /// @notice Allow contract to receive ETH (for bond returns)
    receive() external payable {}
}
