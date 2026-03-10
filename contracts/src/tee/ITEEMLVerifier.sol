// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

/// @title ITEEMLVerifier — Interface for TEE-attested ML inference with ZK dispute resolution
interface ITEEMLVerifier {
    struct EnclaveInfo {
        bool registered;
        bool active;
        bytes32 enclaveImageHash;
        uint256 registeredAt;
    }

    struct MLResult {
        address enclave;
        address submitter;
        bytes32 modelHash;
        bytes32 inputHash;
        bytes32 resultHash;
        bytes result;
        uint256 submittedAt;
        uint256 challengeDeadline;
        uint256 disputeDeadline;
        uint256 challengeBond;
        uint256 proverStakeAmount;
        bool finalized;
        bool challenged;
        address challenger;
    }

    event EnclaveRegistered(address indexed enclaveKey, bytes32 enclaveImageHash);
    event EnclaveRevoked(address indexed enclaveKey);
    event ResultSubmitted(bytes32 indexed resultId, bytes32 modelHash, bytes32 inputHash);
    event ResultChallenged(bytes32 indexed resultId, address challenger);
    event DisputeResolved(bytes32 indexed resultId, bool proverWon);
    event ResultFinalized(bytes32 indexed resultId);
    event ChallengeBondUpdated(uint256 oldAmount, uint256 newAmount);
    event ProverStakeUpdated(uint256 oldAmount, uint256 newAmount);
    event RemainderVerifierUpdated(address oldVerifier, address newVerifier);

    // Admin
    function registerEnclave(address enclaveKey, bytes32 enclaveImageHash) external;
    function revokeEnclave(address enclaveKey) external;
    function setRemainderVerifier(address _verifier) external;
    function setChallengeBondAmount(uint256 _amount) external;
    function setProverStake(uint256 _amount) external;
    function pause() external;
    function unpause() external;

    // Submit
    function submitResult(bytes32 modelHash, bytes32 inputHash, bytes calldata result, bytes calldata attestation)
        external
        payable
        returns (bytes32 resultId);

    // Challenge
    function challenge(bytes32 resultId) external payable;

    // Dispute resolution
    function resolveDispute(
        bytes32 resultId,
        bytes calldata proof,
        bytes32 circuitHash,
        bytes calldata publicInputs,
        bytes calldata gensData
    ) external;

    function resolveDisputeByTimeout(bytes32 resultId) external;

    // Finalize
    function finalize(bytes32 resultId) external;

    // Query
    function getResult(bytes32 resultId) external view returns (MLResult memory);
    function isResultValid(bytes32 resultId) external view returns (bool);
}
