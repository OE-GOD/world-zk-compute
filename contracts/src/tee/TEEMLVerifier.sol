// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import {ITEEMLVerifier} from "./ITEEMLVerifier.sol";
import {ECDSA} from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import {Ownable2Step, Ownable} from "@openzeppelin/contracts/access/Ownable2Step.sol";
import {Pausable} from "@openzeppelin/contracts/utils/Pausable.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";

/// @title TEEMLVerifier — TEE-attested ML inference with ZK dispute resolution
/// @author World ZK Compute
/// @notice Happy path: verify ECDSA attestation from TEE enclave (~3K gas).
///         Dispute path: fall back to existing RemainderVerifier DAG proof verification.
/// @dev Uses Ownable2Step for safe admin transfers, Pausable for emergency stops,
///      and ReentrancyGuard to protect ETH payouts from reentrancy.
contract TEEMLVerifier is ITEEMLVerifier, Ownable2Step, Pausable, ReentrancyGuard {
    using ECDSA for bytes32;

    /// @notice Address of the RemainderVerifier contract used for ZK dispute resolution
    address public remainderVerifier;

    /// @notice Duration of the challenge window after result submission
    uint256 public constant CHALLENGE_WINDOW = 1 hours;

    /// @notice Duration of the dispute window after a challenge is raised
    uint256 public constant DISPUTE_WINDOW = 24 hours;

    /// @notice Minimum ETH bond required to challenge a result
    uint256 public challengeBondAmount = 0.1 ether;

    /// @notice Minimum ETH stake required to submit a result
    uint256 public proverStake = 0.1 ether;

    /// @notice Registered TEE enclave information by signing key address
    mapping(address => EnclaveInfo) public enclaves;

    /// @dev Internal storage for submitted ML results
    mapping(bytes32 => MLResult) internal _results;

    /// @notice Whether a dispute has been resolved for a given result ID
    mapping(bytes32 => bool) public disputeResolved;

    /// @notice Whether the prover won the dispute for a given result ID
    mapping(bytes32 => bool) public disputeProverWon;

    /// @notice Number of deadline extensions used for a given result ID
    mapping(bytes32 => uint256) public disputeExtensions;

    /// @notice Duration added per extension request
    uint256 public constant EXTENSION_PERIOD = 30 minutes;

    /// @notice Maximum number of extensions allowed per dispute
    uint256 public constant MAX_EXTENSIONS = 1;

    /// @notice Initialize the verifier with an admin and RemainderVerifier address
    /// @param _admin Address that will own and administer this contract
    /// @param _remainderVerifier Address of the RemainderVerifier for ZK dispute resolution
    constructor(address _admin, address _remainderVerifier) Ownable(_admin) {
        remainderVerifier = _remainderVerifier;
    }

    // ─── Admin ───────────────────────────────────────────────────────────────

    /// @inheritdoc ITEEMLVerifier
    function registerEnclave(address enclaveKey, bytes32 enclaveImageHash) external onlyOwner {
        require(enclaveKey != address(0), "TEEMLVerifier: zero enclave key");
        require(!enclaves[enclaveKey].registered, "TEEMLVerifier: already registered");

        enclaves[enclaveKey] = EnclaveInfo({
            registered: true, active: true, enclaveImageHash: enclaveImageHash, registeredAt: block.timestamp
        });

        emit EnclaveRegistered(enclaveKey, enclaveImageHash);
    }

    /// @inheritdoc ITEEMLVerifier
    function revokeEnclave(address enclaveKey) external onlyOwner {
        require(enclaves[enclaveKey].registered, "TEEMLVerifier: not registered");
        require(enclaves[enclaveKey].active, "TEEMLVerifier: already revoked");

        enclaves[enclaveKey].active = false;

        emit EnclaveRevoked(enclaveKey);
    }

    /// @inheritdoc ITEEMLVerifier
    function setRemainderVerifier(address _verifier) external onlyOwner {
        require(_verifier != address(0), "TEEMLVerifier: zero address");
        address oldVerifier = remainderVerifier;
        remainderVerifier = _verifier;
        emit RemainderVerifierUpdated(oldVerifier, _verifier);
    }

    /// @inheritdoc ITEEMLVerifier
    function setChallengeBondAmount(uint256 _amount) external onlyOwner {
        require(_amount > 0, "TEEMLVerifier: zero amount");
        require(_amount <= 100 ether, "TEEMLVerifier: amount too high");
        uint256 oldAmount = challengeBondAmount;
        challengeBondAmount = _amount;
        emit ChallengeBondUpdated(oldAmount, _amount);
        emit ConfigUpdated("challengeBondAmount", oldAmount, _amount);
    }

    /// @inheritdoc ITEEMLVerifier
    function setProverStake(uint256 _amount) external onlyOwner {
        require(_amount > 0, "TEEMLVerifier: zero amount");
        require(_amount <= 100 ether, "TEEMLVerifier: amount too high");
        uint256 oldAmount = proverStake;
        proverStake = _amount;
        emit ProverStakeUpdated(oldAmount, _amount);
        emit ConfigUpdated("proverStake", oldAmount, _amount);
    }

    /// @inheritdoc ITEEMLVerifier
    function pause() external onlyOwner {
        _pause();
    }

    /// @inheritdoc ITEEMLVerifier
    function unpause() external onlyOwner {
        _unpause();
    }

    // ─── Submit ──────────────────────────────────────────────────────────────

    /// @inheritdoc ITEEMLVerifier
    /// @dev Verifies the attestation signature against registered enclaves,
    ///      then stores the result with a CHALLENGE_WINDOW deadline.
    function submitResult(bytes32 modelHash, bytes32 inputHash, bytes calldata result, bytes calldata attestation)
        external
        payable
        whenNotPaused
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

        emit ResultSubmitted(resultId, modelHash, inputHash, msg.sender);
    }

    // ─── Challenge ───────────────────────────────────────────────────────────

    /// @inheritdoc ITEEMLVerifier
    /// @dev Sets up a DISPUTE_WINDOW deadline for ZK proof submission.
    function challenge(bytes32 resultId) external payable whenNotPaused {
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

    /// @inheritdoc ITEEMLVerifier
    /// @dev Calls RemainderVerifier.verifyDAGProof() via staticcall. Requires >254M gas
    ///      (supported on Arbitrum in a single tx). If proof is valid, prover wins.
    function resolveDispute(
        bytes32 resultId,
        bytes calldata proof,
        bytes32 circuitHash,
        bytes calldata publicInputs,
        bytes calldata gensData
    ) external nonReentrant {
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

    /// @inheritdoc ITEEMLVerifier
    function resolveDisputeByTimeout(bytes32 resultId) external nonReentrant {
        MLResult storage r = _results[resultId];
        require(r.challenged, "TEEMLVerifier: not challenged");
        require(!disputeResolved[resultId], "TEEMLVerifier: already resolved");
        require(block.timestamp >= r.disputeDeadline, "TEEMLVerifier: deadline not reached");

        _settleDispute(resultId, r, false);
    }

    // ─── Dispute Extension ───────────────────────────────────────────────────

    /// @inheritdoc ITEEMLVerifier
    function extendDisputeWindow(bytes32 resultId) external {
        MLResult storage r = _results[resultId];
        require(r.challenged, "TEEMLVerifier: not challenged");
        require(!disputeResolved[resultId], "TEEMLVerifier: already resolved");
        require(r.submitter == msg.sender, "TEEMLVerifier: not submitter");
        require(disputeExtensions[resultId] < MAX_EXTENSIONS, "TEEMLVerifier: max extensions reached");

        disputeExtensions[resultId] += 1;
        r.disputeDeadline += EXTENSION_PERIOD;

        emit DisputeExtended(resultId, r.disputeDeadline);
    }

    // ─── Finalize ────────────────────────────────────────────────────────────

    /// @inheritdoc ITEEMLVerifier
    /// @dev Returns the prover's stake via low-level call for contract wallet compatibility.
    function finalize(bytes32 resultId) external nonReentrant {
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

        emit ResultExpired(resultId);
        emit ResultFinalized(resultId);
    }

    // ─── Query ───────────────────────────────────────────────────────────────

    /// @inheritdoc ITEEMLVerifier
    function getResult(bytes32 resultId) external view returns (MLResult memory) {
        return _results[resultId];
    }

    /// @inheritdoc ITEEMLVerifier
    /// @dev Returns true if: (1) finalized without challenge, or (2) challenged and dispute
    ///      resolved in prover's favor.
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

    /// @dev Settle a dispute by paying out the combined pot (prover stake + challenger bond)
    ///      to the winner. Prover wins if proofValid is true, challenger wins otherwise.
    /// @param resultId The disputed result identifier
    /// @param r Storage reference to the MLResult
    /// @param proofValid Whether the ZK proof verified successfully
    function _settleDispute(bytes32 resultId, MLResult storage r, bool proofValid) internal {
        disputeResolved[resultId] = true;
        disputeProverWon[resultId] = proofValid;
        r.finalized = true;

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

    /// @dev Produce an Ethereum signed message hash (EIP-191 prefix)
    /// @param hash The message hash to prefix
    /// @return The prefixed hash suitable for ecrecover
    function _toEthSignedMessageHash(bytes32 hash) internal pure returns (bytes32) {
        return keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", hash));
    }

    /// @notice Allow contract to receive ETH (for bond returns)
    receive() external payable {}
}
