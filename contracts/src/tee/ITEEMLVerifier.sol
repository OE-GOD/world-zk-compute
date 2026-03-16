// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

/// @title ITEEMLVerifier — Interface for TEE-attested ML inference with ZK dispute resolution
/// @notice Defines the public API for submitting TEE-attested ML results, challenging them,
///         and resolving disputes via ZK proofs or timeout.
interface ITEEMLVerifier {
    // ─── Custom Errors ──────────────────────────────────────────────────────
    error ZeroEnclaveKey();
    error AlreadyRegistered();
    error NotRegistered();
    error AlreadyRevoked();
    error ZeroAddress();
    error ZeroAmount();
    error AmountTooHigh();
    error WindowTooShort();
    error WindowTooLong();
    error InsufficientStake();
    error UnregisteredEnclave();
    error EnclaveNotActive();
    error ResultExists();
    error ResultNotFound();
    error AlreadyFinalized();
    error AlreadyChallenged();
    error InsufficientBond();
    error ChallengeWindowClosed();
    error NotChallenged();
    error AlreadyResolved();
    error NoVerifierSet();
    error DeadlineNotReached();
    error NotSubmitter();
    error MaxExtensionsReached();
    error ChallengeWindowNotPassed();
    error ResultIsChallenged();
    error StakeReturnFailed();
    error PayoutFailed();

    /// @notice Information about a registered TEE enclave
    /// @param registered Whether this enclave has been registered
    /// @param active Whether this enclave is currently active (not revoked)
    /// @param enclaveImageHash Hash of the enclave image (e.g., AWS Nitro PCR0)
    /// @param registeredAt Timestamp when the enclave was registered
    struct EnclaveInfo {
        bool registered;
        bool active;
        bytes32 enclaveImageHash;
        uint256 registeredAt;
    }

    /// @notice An ML inference result submitted with TEE attestation
    /// @param enclave Address of the TEE enclave that signed the attestation
    /// @param submitter Address that submitted the result on-chain
    /// @param modelHash Hash identifying the ML model used
    /// @param inputHash Hash of the input data fed to the model
    /// @param resultHash keccak256 hash of the result bytes
    /// @param result Raw result bytes from the ML inference
    /// @param submittedAt Timestamp when the result was submitted
    /// @param challengeDeadline Timestamp after which the result can no longer be challenged
    /// @param disputeDeadline Timestamp by which a ZK proof must be submitted (set on challenge)
    /// @param challengeBond ETH bond posted by the challenger
    /// @param proverStakeAmount ETH stake posted by the submitter
    /// @param finalized Whether the result has been finalized (either unchallenged or dispute resolved)
    /// @param challenged Whether the result has been challenged
    /// @param challenger Address of the challenger (zero if unchallenged)
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

    /// @notice Emitted when a new TEE enclave is registered
    /// @param enclaveKey The enclave's signing key address
    /// @param enclaveImageHash Hash of the enclave image
    event EnclaveRegistered(address indexed enclaveKey, bytes32 enclaveImageHash);

    /// @notice Emitted when a TEE enclave is revoked
    /// @param enclaveKey The revoked enclave's signing key address
    event EnclaveRevoked(address indexed enclaveKey);

    /// @notice Emitted when a new ML result is submitted
    /// @param resultId Unique identifier for this result
    /// @param modelHash Hash of the ML model used
    /// @param inputHash Hash of the input data
    /// @param submitter Address that submitted the result
    event ResultSubmitted(
        bytes32 indexed resultId, bytes32 indexed modelHash, bytes32 inputHash, address indexed submitter
    );

    /// @notice Emitted when a result is challenged
    /// @param resultId The challenged result's identifier
    /// @param challenger Address of the challenger
    event ResultChallenged(bytes32 indexed resultId, address challenger);

    /// @notice Emitted when a dispute is resolved (by ZK proof or timeout)
    /// @param resultId The disputed result's identifier
    /// @param proverWon True if the prover's result was validated, false if challenger won
    event DisputeResolved(bytes32 indexed resultId, bool proverWon);

    /// @notice Emitted when an unchallenged result is finalized
    /// @param resultId The finalized result's identifier
    event ResultFinalized(bytes32 indexed resultId);

    /// @notice Emitted when a result passes its challenge window (legacy, emitted alongside ResultFinalized)
    /// @param resultId The result's identifier
    event ResultExpired(bytes32 indexed resultId);

    /// @notice Emitted when the challenge bond amount is updated
    /// @param oldAmount Previous bond amount in wei
    /// @param newAmount New bond amount in wei
    event ChallengeBondUpdated(uint256 oldAmount, uint256 newAmount);

    /// @notice Emitted when the prover stake amount is updated
    /// @param oldAmount Previous stake amount in wei
    /// @param newAmount New stake amount in wei
    event ProverStakeUpdated(uint256 oldAmount, uint256 newAmount);

    /// @notice Emitted when the RemainderVerifier address is updated
    /// @param oldVerifier Previous verifier address
    /// @param newVerifier New verifier address
    event RemainderVerifierUpdated(address oldVerifier, address newVerifier);

    /// @notice Emitted when a configuration parameter is updated
    /// @param param Name of the parameter changed
    /// @param oldValue Previous value
    /// @param newValue New value
    event ConfigUpdated(string param, uint256 oldValue, uint256 newValue);

    /// @notice Emitted when a dispute deadline is extended
    /// @param resultId The result whose deadline was extended
    /// @param newDeadline The new dispute deadline timestamp
    event DisputeExtended(bytes32 indexed resultId, uint256 newDeadline);

    /// @notice Register a new TEE enclave. Only callable by the contract owner.
    /// @param enclaveKey The enclave's ECDSA signing key address
    /// @param enclaveImageHash Hash of the enclave image (e.g., AWS Nitro PCR0)
    function registerEnclave(address enclaveKey, bytes32 enclaveImageHash) external;

    /// @notice Revoke a registered TEE enclave. Only callable by the contract owner.
    /// @param enclaveKey The enclave's signing key address to revoke
    function revokeEnclave(address enclaveKey) external;

    /// @notice Set the RemainderVerifier contract used for ZK dispute resolution. Owner only.
    /// @param _verifier Address of the RemainderVerifier contract
    function setRemainderVerifier(address _verifier) external;

    /// @notice Set the minimum challenge bond amount. Owner only.
    /// @param _amount New bond amount in wei (must be > 0 and <= 100 ether)
    function setChallengeBondAmount(uint256 _amount) external;

    /// @notice Set the minimum prover stake amount. Owner only.
    /// @param _amount New stake amount in wei (must be > 0 and <= 100 ether)
    function setProverStake(uint256 _amount) external;

    /// @notice Pause the contract, blocking submissions and challenges. Owner only.
    function pause() external;

    /// @notice Unpause the contract, resuming normal operations. Owner only.
    function unpause() external;

    /// @notice Submit a TEE-attested ML inference result with a prover stake.
    /// @param modelHash Hash identifying the ML model
    /// @param inputHash Hash of the input data
    /// @param result Raw inference result bytes
    /// @param attestation ECDSA signature from a registered TEE enclave over (modelHash, inputHash, resultHash)
    /// @return resultId Unique identifier for the submitted result
    function submitResult(bytes32 modelHash, bytes32 inputHash, bytes calldata result, bytes calldata attestation)
        external
        payable
        returns (bytes32 resultId);

    /// @notice Challenge a submitted result within the challenge window.
    /// @dev Requires sending at least `challengeBondAmount` ETH as the challenge bond.
    /// @param resultId The result to challenge
    function challenge(bytes32 resultId) external payable;

    /// @notice Resolve a dispute by submitting a ZK proof via the RemainderVerifier.
    /// @dev If the proof is valid, the prover wins both stakes. Otherwise, the challenger wins.
    /// @param resultId The disputed result
    /// @param proof The ZK proof bytes
    /// @param circuitHash Hash identifying the verification circuit
    /// @param publicInputs Public inputs for the ZK proof
    /// @param gensData Generator data for the ZK proof verification
    function resolveDispute(
        bytes32 resultId,
        bytes calldata proof,
        bytes32 circuitHash,
        bytes calldata publicInputs,
        bytes calldata gensData
    ) external;

    /// @notice Resolve a dispute by timeout. If the prover fails to submit a ZK proof
    ///         within the dispute window, the challenger wins by default.
    /// @param resultId The disputed result whose deadline has passed
    function resolveDisputeByTimeout(bytes32 resultId) external;

    /// @notice Extend the dispute deadline by EXTENSION_PERIOD. Only the original submitter
    ///         can call this, and only once per dispute (MAX_EXTENSIONS = 1).
    /// @param resultId The disputed result to extend
    function extendDisputeWindow(bytes32 resultId) external;

    /// @notice Finalize an unchallenged result after the challenge window has passed.
    ///         Returns the prover stake to the submitter.
    /// @param resultId The result to finalize
    function finalize(bytes32 resultId) external;

    /// @notice Get the full details of a submitted result.
    /// @param resultId The result to query
    /// @return The MLResult struct with all fields
    function getResult(bytes32 resultId) external view returns (MLResult memory);

    /// @notice Check if a result is considered valid (finalized unchallenged, or dispute resolved in prover's favor).
    /// @param resultId The result to check
    /// @return True if the result is valid
    function isResultValid(bytes32 resultId) external view returns (bool);
}
