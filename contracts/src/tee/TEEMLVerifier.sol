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
///      Uses EIP-712 structured data signing for cross-chain replay protection.
///      Storage is packed for gas efficiency:
///      - PackedEnclaveInfo: 2 slots (was 3) — registered+active+registeredAt packed with address key
///      - PackedMLResult: saves 4 slots per result via uint40 timestamps and bool packing
contract TEEMLVerifier is ITEEMLVerifier, Ownable2Step, Pausable, ReentrancyGuard {
    using ECDSA for bytes32;

    // ─── EIP-712 Constants ──────────────────────────────────────────────────
    // Domain separator fields per EIP-712
    bytes32 private constant EIP712_DOMAIN_TYPEHASH =
        keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)");

    bytes32 private constant NAME_HASH = keccak256("TEEMLVerifier");
    bytes32 private constant VERSION_HASH = keccak256("1");

    /// @dev EIP-712 type hash for TEE attestation result submissions
    bytes32 public constant RESULT_TYPEHASH =
        keccak256("TEEMLResult(bytes32 modelHash,bytes32 inputHash,bytes32 resultHash)");

    /// @dev Cached domain separator (recomputed if chainId changes after a fork)
    bytes32 private _cachedDomainSeparator;

    /// @dev Chain ID at the time the domain separator was cached
    uint256 private _cachedChainId;

    // ─── Packed Storage Structs ─────────────────────────────────────────────
    // These internal structs optimize storage layout. The public interface
    // (ITEEMLVerifier.EnclaveInfo / ITEEMLVerifier.MLResult) is unchanged.

    /// @dev Packed enclave info for storage efficiency.
    ///      Slot 0: registered (1) + active (1) + registeredAt (5) = 7 bytes (fits in one slot)
    ///      Slot 1: enclaveImageHash (32 bytes)
    ///      Total: 2 slots (down from 3)
    struct PackedEnclaveInfo {
        bool registered;
        bool active;
        uint40 registeredAt;
        bytes32 enclaveImageHash;
    }

    /// @dev Packed ML result for storage efficiency.
    ///      Slot 0: enclave (20) + submittedAt (5) + challengeDeadline (5) + finalized (1) + challenged (1) = 32
    ///      Slot 1: submitter (20) + disputeDeadline (5) = 25 bytes
    ///      Slot 2: challenger (20) = 20 bytes
    ///      Slot 3: modelHash (32)
    ///      Slot 4: inputHash (32)
    ///      Slot 5: resultHash (32)
    ///      Slot 6+: result (dynamic bytes)
    ///      Slot N: challengeBond (32)
    ///      Slot N+1: proverStakeAmount (32)
    ///      Total: ~10 slots (down from ~14)
    struct PackedMLResult {
        // --- Slot 0: 20 + 5 + 5 + 1 + 1 = 32 bytes ---
        address enclave;
        uint40 submittedAt;
        uint40 challengeDeadline;
        bool finalized;
        bool challenged;
        // --- Slot 1: 20 + 5 = 25 bytes ---
        address submitter;
        uint40 disputeDeadline;
        // --- Slot 2: 20 bytes ---
        address challenger;
        // --- Slot 3-5: 32 bytes each ---
        bytes32 modelHash;
        bytes32 inputHash;
        bytes32 resultHash;
        // --- Dynamic slot(s) ---
        bytes result;
        // --- Slot N, N+1: 32 bytes each (ETH amounts need full uint256) ---
        uint256 challengeBond;
        uint256 proverStakeAmount;
    }

    /// @notice Address of the RemainderVerifier contract used for ZK dispute resolution
    address public remainderVerifier;

    /// @notice Duration of the challenge window after result submission
    uint256 public challengeWindow = 1 hours;

    /// @notice Duration of the dispute window after a challenge is raised
    uint256 public disputeWindow = 24 hours;

    /// @notice Minimum ETH bond required to challenge a result
    uint256 public challengeBondAmount = 0.1 ether;

    /// @notice Minimum ETH stake required to submit a result
    uint256 public proverStake = 0.1 ether;

    /// @dev Packed enclave storage (replaces mapping to EnclaveInfo)
    mapping(address => PackedEnclaveInfo) internal _enclaves;

    /// @dev Packed result storage (replaces mapping to MLResult)
    mapping(bytes32 => PackedMLResult) internal _results;

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
        _cachedChainId = block.chainid;
        _cachedDomainSeparator = _computeDomainSeparator();
    }

    // ─── Admin ───────────────────────────────────────────────────────────────

    /// @inheritdoc ITEEMLVerifier
    function registerEnclave(address enclaveKey, bytes32 enclaveImageHash) external onlyOwner {
        if (enclaveKey == address(0)) revert ZeroEnclaveKey();
        if (_enclaves[enclaveKey].registered) revert AlreadyRegistered();

        _enclaves[enclaveKey] = PackedEnclaveInfo({
            registered: true,
            active: true,
            // forge-lint: disable-next-line(unsafe-typecast)
            registeredAt: uint40(block.timestamp),
            enclaveImageHash: enclaveImageHash
        });

        emit EnclaveRegistered(enclaveKey, enclaveImageHash);
    }

    /// @inheritdoc ITEEMLVerifier
    function revokeEnclave(address enclaveKey) external onlyOwner {
        if (!_enclaves[enclaveKey].registered) revert NotRegistered();
        if (!_enclaves[enclaveKey].active) revert AlreadyRevoked();

        _enclaves[enclaveKey].active = false;

        emit EnclaveRevoked(enclaveKey);
    }

    /// @notice Read enclave info in the public interface format
    /// @param enclaveKey The enclave's signing key address
    /// @return info The EnclaveInfo struct (unpacked from storage)
    function enclaves(address enclaveKey) external view returns (EnclaveInfo memory info) {
        PackedEnclaveInfo storage p = _enclaves[enclaveKey];
        info = EnclaveInfo({
            registered: p.registered,
            active: p.active,
            enclaveImageHash: p.enclaveImageHash,
            registeredAt: uint256(p.registeredAt)
        });
    }

    /// @inheritdoc ITEEMLVerifier
    function setRemainderVerifier(address _verifier) external onlyOwner {
        if (_verifier == address(0)) revert ZeroAddress();
        address oldVerifier = remainderVerifier;
        remainderVerifier = _verifier;
        emit RemainderVerifierUpdated(oldVerifier, _verifier);
    }

    /// @inheritdoc ITEEMLVerifier
    function setChallengeBondAmount(uint256 _amount) external onlyOwner {
        if (_amount == 0) revert ZeroAmount();
        if (_amount > 100 ether) revert AmountTooHigh();
        uint256 oldAmount = challengeBondAmount;
        challengeBondAmount = _amount;
        emit ChallengeBondUpdated(oldAmount, _amount);
        emit ConfigUpdated("challengeBondAmount", oldAmount, _amount);
    }

    /// @inheritdoc ITEEMLVerifier
    function setProverStake(uint256 _amount) external onlyOwner {
        if (_amount == 0) revert ZeroAmount();
        if (_amount > 100 ether) revert AmountTooHigh();
        uint256 oldAmount = proverStake;
        proverStake = _amount;
        emit ProverStakeUpdated(oldAmount, _amount);
        emit ConfigUpdated("proverStake", oldAmount, _amount);
    }

    /// @notice Update the challenge window duration
    /// @param _duration New challenge window in seconds (min 10 min, max 7 days)
    function setChallengeWindow(uint256 _duration) external onlyOwner {
        if (_duration < 10 minutes) revert WindowTooShort();
        if (_duration > 7 days) revert WindowTooLong();
        uint256 oldDuration = challengeWindow;
        challengeWindow = _duration;
        emit ConfigUpdated("challengeWindow", oldDuration, _duration);
    }

    /// @notice Update the dispute window duration
    /// @param _duration New dispute window in seconds (min 1 hour, max 30 days)
    function setDisputeWindow(uint256 _duration) external onlyOwner {
        if (_duration < 1 hours) revert WindowTooShort();
        if (_duration > 30 days) revert WindowTooLong();
        uint256 oldDuration = disputeWindow;
        disputeWindow = _duration;
        emit ConfigUpdated("disputeWindow", oldDuration, _duration);
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
    ///      then stores the result with a challengeWindow deadline.
    function submitResult(bytes32 modelHash, bytes32 inputHash, bytes calldata result, bytes calldata attestation)
        external
        payable
        whenNotPaused
        nonReentrant
        returns (bytes32 resultId)
    {
        if (msg.value < proverStake) revert InsufficientStake();

        bytes32 resultHash = keccak256(result);
        bytes32 structHash = keccak256(abi.encode(RESULT_TYPEHASH, modelHash, inputHash, resultHash));
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", DOMAIN_SEPARATOR(), structHash));

        address signer = digest.recover(attestation);
        if (!_enclaves[signer].registered) revert UnregisteredEnclave();
        if (!_enclaves[signer].active) revert EnclaveNotActive();

        resultId = keccak256(abi.encodePacked(msg.sender, modelHash, inputHash, block.number));
        if (_results[resultId].submittedAt != 0) revert ResultExists();

        _results[resultId] = PackedMLResult({
            enclave: signer,
            // forge-lint: disable-next-line(unsafe-typecast)
            submittedAt: uint40(block.timestamp),
            // forge-lint: disable-next-line(unsafe-typecast)
            challengeDeadline: uint40(block.timestamp + challengeWindow),
            finalized: false,
            challenged: false,
            submitter: msg.sender,
            disputeDeadline: 0,
            challenger: address(0),
            modelHash: modelHash,
            inputHash: inputHash,
            resultHash: resultHash,
            result: result,
            challengeBond: 0,
            proverStakeAmount: msg.value
        });

        emit ResultSubmitted(resultId, modelHash, inputHash, msg.sender);
    }

    // ─── Challenge ───────────────────────────────────────────────────────────

    /// @inheritdoc ITEEMLVerifier
    /// @dev Sets up a disputeWindow deadline for ZK proof submission.
    function challenge(bytes32 resultId) external payable whenNotPaused nonReentrant {
        PackedMLResult storage r = _results[resultId];
        if (r.submittedAt == 0) revert ResultNotFound();
        if (r.finalized) revert AlreadyFinalized();
        if (r.challenged) revert AlreadyChallenged();
        if (msg.value < challengeBondAmount) revert InsufficientBond();
        if (block.timestamp >= r.challengeDeadline) revert ChallengeWindowClosed();

        r.challenged = true;
        r.challenger = msg.sender;
        r.challengeBond = msg.value;
        // forge-lint: disable-next-line(unsafe-typecast)
        r.disputeDeadline = uint40(block.timestamp + disputeWindow);

        emit ResultChallenged(resultId, msg.sender);
    }

    // ─── Dispute Resolution ──────────────────────────────────────────────────

    /// @inheritdoc ITEEMLVerifier
    /// @dev Calls RemainderVerifier.verifyDAGProof() via call. Requires >254M gas
    ///      (supported on Arbitrum in a single tx). If proof is valid, prover wins.
    function resolveDispute(
        bytes32 resultId,
        bytes calldata proof,
        bytes32 circuitHash,
        bytes calldata publicInputs,
        bytes calldata gensData
    ) external nonReentrant {
        PackedMLResult storage r = _results[resultId];
        if (!r.challenged) revert NotChallenged();
        if (disputeResolved[resultId]) revert AlreadyResolved();
        if (remainderVerifier == address(0)) revert NoVerifierSet();

        // Call the existing single-tx DAG proof verification
        // This requires >254M gas (supported on Arbitrum in a single tx)
        // Uses call instead of staticcall so RemainderVerifier can emit events.
        // Reentrancy is prevented by the nonReentrant modifier on this function.
        (bool success, bytes memory returnData) = remainderVerifier.call(
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
        PackedMLResult storage r = _results[resultId];
        if (!r.challenged) revert NotChallenged();
        if (disputeResolved[resultId]) revert AlreadyResolved();
        if (block.timestamp < r.disputeDeadline) revert DeadlineNotReached();

        _settleDispute(resultId, r, false);
    }

    // ─── Dispute Extension ───────────────────────────────────────────────────

    /// @inheritdoc ITEEMLVerifier
    function extendDisputeWindow(bytes32 resultId) external nonReentrant {
        PackedMLResult storage r = _results[resultId];
        if (!r.challenged) revert NotChallenged();
        if (disputeResolved[resultId]) revert AlreadyResolved();
        if (r.submitter != msg.sender) revert NotSubmitter();
        if (disputeExtensions[resultId] >= MAX_EXTENSIONS) revert MaxExtensionsReached();

        disputeExtensions[resultId] += 1;
        // forge-lint: disable-next-line(unsafe-typecast)
        r.disputeDeadline = uint40(uint256(r.disputeDeadline) + EXTENSION_PERIOD);

        emit DisputeExtended(resultId, uint256(r.disputeDeadline));
    }

    // ─── Finalize ────────────────────────────────────────────────────────────

    /// @inheritdoc ITEEMLVerifier
    /// @dev Returns the prover's stake via low-level call for contract wallet compatibility.
    function finalize(bytes32 resultId) external nonReentrant {
        PackedMLResult storage r = _results[resultId];
        if (r.submittedAt == 0) revert ResultNotFound();
        if (block.timestamp < r.challengeDeadline) revert ChallengeWindowNotPassed();
        if (r.challenged) revert ResultIsChallenged();
        if (r.finalized) revert AlreadyFinalized();

        r.finalized = true;

        // Return prover stake to submitter
        if (r.proverStakeAmount > 0) {
            (bool sent,) = r.submitter.call{value: r.proverStakeAmount}("");
            if (!sent) revert StakeReturnFailed();
        }

        emit ResultExpired(resultId);
        emit ResultFinalized(resultId);
    }

    // ─── Query ───────────────────────────────────────────────────────────────

    /// @inheritdoc ITEEMLVerifier
    function getResult(bytes32 resultId) external view returns (MLResult memory) {
        PackedMLResult storage p = _results[resultId];
        return MLResult({
            enclave: p.enclave,
            submitter: p.submitter,
            modelHash: p.modelHash,
            inputHash: p.inputHash,
            resultHash: p.resultHash,
            result: p.result,
            submittedAt: uint256(p.submittedAt),
            challengeDeadline: uint256(p.challengeDeadline),
            disputeDeadline: uint256(p.disputeDeadline),
            challengeBond: p.challengeBond,
            proverStakeAmount: p.proverStakeAmount,
            finalized: p.finalized,
            challenged: p.challenged,
            challenger: p.challenger
        });
    }

    /// @inheritdoc ITEEMLVerifier
    /// @dev Returns true if: (1) finalized without challenge, or (2) challenged and dispute
    ///      resolved in prover's favor.
    function isResultValid(bytes32 resultId) external view returns (bool) {
        PackedMLResult storage r = _results[resultId];
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
    function _settleDispute(bytes32 resultId, PackedMLResult storage r, bool proofValid) internal {
        disputeResolved[resultId] = true;
        disputeProverWon[resultId] = proofValid;
        r.finalized = true;

        uint256 totalPot = r.challengeBond + r.proverStakeAmount;

        if (proofValid) {
            // Prover was honest — submitter wins both stakes
            (bool sent,) = r.submitter.call{value: totalPot}("");
            if (!sent) revert PayoutFailed();
        } else {
            // Prover was dishonest — challenger wins both stakes
            (bool sent,) = r.challenger.call{value: totalPot}("");
            if (!sent) revert PayoutFailed();
        }

        emit DisputeResolved(resultId, proofValid);
    }

    // ─── EIP-712 Domain Separator ──────────────────────────────────────────

    /// @notice Returns the EIP-712 domain separator for this contract.
    ///         Recomputes if the chain ID has changed (fork protection).
    /// @return The domain separator bytes32 value
    function DOMAIN_SEPARATOR() public view returns (bytes32) {
        if (block.chainid == _cachedChainId) {
            return _cachedDomainSeparator;
        }
        return _computeDomainSeparator();
    }

    /// @dev Computes the EIP-712 domain separator using current chain ID and contract address
    function _computeDomainSeparator() internal view returns (bytes32) {
        return keccak256(abi.encode(EIP712_DOMAIN_TYPEHASH, NAME_HASH, VERSION_HASH, block.chainid, address(this)));
    }

    /// @notice Allow contract to receive ETH (for bond returns)
    receive() external payable {}
}
