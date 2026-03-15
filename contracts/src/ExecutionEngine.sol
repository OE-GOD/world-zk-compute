// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "./ProgramRegistry.sol";
import "./ProverReputation.sol";
import {IProofVerifier} from "./IProofVerifier.sol";
import {IRiscZeroVerifier} from "risc0-ethereum/IRiscZeroVerifier.sol";
import {Ownable2Step, Ownable} from "@openzeppelin/contracts/access/Ownable2Step.sol";
import {Pausable} from "@openzeppelin/contracts/utils/Pausable.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";

/// @title IExecutionCallback
/// @notice Interface for contracts that receive verified computation results
/// @dev Implementors MUST be aware of the following constraints:
///
///      GAS LIMITS: The callback receives all remaining gas from the submitProof transaction
///      after proof verification and prover payment. There is no explicit gas stipend -- the
///      callback inherits whatever gas remains. Keep callback logic minimal and avoid unbounded
///      loops or large storage writes. If the callback consumes too much gas, the entire
///      submitProof transaction will run out of gas (reverting proof recording and prover payment).
///
///      REVERT BEHAVIOR: If onExecutionComplete reverts, the proof is still considered valid and
///      the prover is still paid. The ExecutionEngine catches the revert and emits a CallbackFailed
///      event. The callback contract should NOT rely on reverts to signal invalid results -- the
///      proof has already been cryptographically verified at this point.
///
///      REENTRANCY: The callback is invoked after all state changes (status update, prover payment,
///      fee payment) are complete. The ExecutionEngine uses ReentrancyGuard, so calling back into
///      submitProof or other nonReentrant functions from the callback will revert.
interface IExecutionCallback {
    /// @notice Called by ExecutionEngine after a proof is verified
    /// @param requestId The ID of the completed execution request
    /// @param imageId The program image ID that was executed
    /// @param journal The public outputs (journal) from the execution
    function onExecutionComplete(uint256 requestId, bytes32 imageId, bytes calldata journal) external;
}

/// @title ExecutionEngine
/// @notice Core engine for verifiable computation on World Chain
/// @dev Handles the full lifecycle: request -> claim -> prove -> callback
///      Uses Ownable2Step for safe ownership transfers and Pausable for emergency stops.
contract ExecutionEngine is Ownable2Step, Pausable, ReentrancyGuard {
    // ========================================================================
    // TYPES
    // ========================================================================

    /// @notice Lifecycle status of an execution request
    enum RequestStatus {
        Pending, // Waiting for prover to claim
        Claimed, // Prover claimed, proof expected
        Completed, // Proof verified, callback executed
        Expired, // Claim expired, can be reclaimed
        Cancelled // Requester cancelled
    }

    /// @notice Storage layout for a single execution request
    /// @dev Fields are packed to minimize storage slots (8 slots total)
    struct ExecutionRequest {
        uint256 id; // Slot 0
        bytes32 imageId; // Slot 1: program to execute
        bytes32 inputDigest; // Slot 2: hash of inputs (actual inputs stored off-chain)
        address requester; // Slot 3: requester(20) + createdAt(6) + expiresAt(6)
        uint48 createdAt;
        uint48 expiresAt;
        address callbackContract; // Slot 4: callback(20) + status(1)
        RequestStatus status;
        address claimedBy; // Slot 5: claimedBy(20) + claimedAt(6) + claimDeadline(6)
        uint48 claimedAt;
        uint48 claimDeadline;
        uint256 tip; // Slot 6: payment for prover
        uint256 maxTip; // Slot 7: starting tip (decreases over time)
    }

    // ========================================================================
    // CONSTANTS
    // ========================================================================

    /// @notice Minimum tip required to create an execution request
    uint256 public constant MIN_TIP = 0.0001 ether;
    /// @notice Default expiration time for requests when none specified
    uint256 public constant DEFAULT_EXPIRATION = 1 hours;
    /// @notice Time window a prover has to submit proof after claiming
    uint256 public constant CLAIM_WINDOW = 10 minutes;
    /// @notice Duration over which the tip linearly decays to 50% of maxTip
    uint256 public constant TIP_DECAY_PERIOD = 30 minutes;
    /// @notice Default protocol fee in basis points (2.5%)
    uint256 private constant DEFAULT_PROTOCOL_FEE_BPS = 250;
    /// @notice Maximum protocol fee in basis points (10%)
    uint256 private constant MAX_FEE_BPS = 1000;

    // ========================================================================
    // STATE
    // ========================================================================

    /// @notice The program registry
    ProgramRegistry public immutable registry;

    /// @notice The RISC Zero verifier contract
    IRiscZeroVerifier public immutable verifier;

    /// @notice ProverReputation contract (optional)
    ProverReputation public reputation;

    /// @notice All execution requests
    mapping(uint256 => ExecutionRequest) public requests;

    /// @notice Next request ID
    uint256 public nextRequestId = 1;

    /// @notice Protocol fee (basis points)
    uint256 public protocolFeeBps = DEFAULT_PROTOCOL_FEE_BPS;

    /// @notice Fee recipient
    address public feeRecipient;

    /// @notice Prover stats
    mapping(address => uint256) public proverCompletedCount;
    mapping(address => uint256) public proverEarnings;

    // ========================================================================
    // EVENTS
    // ========================================================================

    /// @notice Emitted when a new execution request is created
    event ExecutionRequested(
        uint256 indexed requestId,
        address indexed requester,
        bytes32 indexed imageId,
        bytes32 inputDigest,
        string inputUrl,
        uint8 inputType,
        uint256 tip,
        uint256 expiresAt
    );

    /// @notice Emitted when a prover claims an execution request
    event ExecutionClaimed(uint256 indexed requestId, address indexed prover, uint256 claimDeadline);

    /// @notice Emitted when a proof is verified and the prover is paid
    event ExecutionCompleted(uint256 indexed requestId, address indexed prover, bytes32 journalDigest, uint256 payout);

    /// @notice Emitted when a request passes its expiration time
    event ExecutionExpired(uint256 indexed requestId);
    /// @notice Emitted when a requester cancels their pending request
    event ExecutionCancelled(uint256 indexed requestId);
    /// @notice Emitted when a prover's claim deadline passes without proof submission
    event ClaimExpired(uint256 indexed requestId, address indexed prover);
    /// @notice Emitted when an onExecutionComplete callback reverts (proof still valid)
    event CallbackFailed(uint256 indexed requestId, address indexed callbackContract, bytes reason);
    /// @notice Emitted when the reputation contract address is updated
    event ReputationContractSet(address indexed reputation);
    /// @notice Emitted when the protocol fee basis points are changed
    event ProtocolFeeUpdated(uint256 oldFeeBps, uint256 newFeeBps);
    /// @notice Emitted when the fee recipient address is changed
    event FeeRecipientUpdated(address indexed oldRecipient, address indexed newRecipient);

    // ========================================================================
    // ERRORS
    // ========================================================================

    /// @notice Thrown when msg.value is below MIN_TIP
    error InsufficientTip();
    /// @notice Thrown when the requested program is not active in the registry
    error ProgramNotActive();
    /// @notice Thrown when the specified request ID does not exist
    error RequestNotFound();
    /// @notice Thrown when the request is not in Pending status
    error RequestNotPending();
    /// @notice Thrown when the request is not in Claimed status
    error RequestNotClaimed();
    /// @notice Thrown when msg.sender is not the original requester
    error NotRequester();
    /// @notice Thrown when msg.sender is not the prover who claimed the request
    error NotClaimant();
    /// @notice Thrown when trying to reclaim a request whose claim has not yet expired
    error ClaimNotExpired();
    /// @notice Thrown when trying to claim a request past its expiration
    error RequestExpired();
    /// @notice Thrown when proof is submitted after the claim deadline
    error ClaimDeadlinePassed();
    /// @notice Thrown when the proof verification fails
    error InvalidProof();
    /// @notice Thrown when an empty seal is provided to submitProof
    error EmptySeal();
    /// @notice Thrown when an empty journal is provided to submitProof
    error EmptyJournal();
    /// @notice Thrown when an ETH transfer (tip refund or payout) fails
    error TransferFailed();
    /// @notice Thrown when imageId is bytes32(0)
    error ZeroImageId();
    /// @notice Thrown when the computed expiration overflows
    error ExpirationInPast();
    /// @notice Thrown when a zero address is provided where a valid address is required
    error ZeroAddress();

    // ========================================================================
    // CONSTRUCTOR
    // ========================================================================

    /// @notice Deploy a new ExecutionEngine
    /// @param _admin Owner address for Ownable2Step
    /// @param _registry ProgramRegistry contract address
    /// @param _verifier Default IRiscZeroVerifier contract address
    /// @param _feeRecipient Address that receives protocol fees
    constructor(address _admin, address _registry, address _verifier, address _feeRecipient) Ownable(_admin) {
        if (_admin == address(0)) revert ZeroAddress();
        if (_registry == address(0)) revert ZeroAddress();
        if (_verifier == address(0)) revert ZeroAddress();
        if (_feeRecipient == address(0)) revert ZeroAddress();
        registry = ProgramRegistry(_registry);
        verifier = IRiscZeroVerifier(_verifier);
        feeRecipient = _feeRecipient;
    }

    // ========================================================================
    // REQUEST EXECUTION
    // ========================================================================

    /// @notice Request execution of a zkVM program
    /// @param imageId The program to execute
    /// @param inputDigest Hash of the inputs
    /// @param inputUrl URL where prover can fetch inputs
    /// @param callbackContract Contract to receive results (0x0 for no callback)
    /// @param expirationSeconds How long before request expires (0 uses DEFAULT_EXPIRATION)
    /// @param inputType 0 = Public, 1 = Private (event-only, not stored)
    /// @return requestId The unique ID assigned to this request
    function requestExecution(
        bytes32 imageId,
        bytes32 inputDigest,
        string calldata inputUrl,
        address callbackContract,
        uint256 expirationSeconds,
        uint8 inputType
    ) external payable whenNotPaused returns (uint256 requestId) {
        if (imageId == bytes32(0)) revert ZeroImageId();
        if (msg.value < MIN_TIP) revert InsufficientTip();
        if (!registry.isProgramActive(imageId)) revert ProgramNotActive();

        uint256 expiration = expirationSeconds > 0 ? expirationSeconds : DEFAULT_EXPIRATION;
        if (block.timestamp + expiration <= block.timestamp) revert ExpirationInPast();

        requestId = nextRequestId++;

        requests[requestId] = ExecutionRequest({
            id: requestId,
            imageId: imageId,
            inputDigest: inputDigest,
            requester: msg.sender,
            createdAt: uint48(block.timestamp),
            expiresAt: uint48(block.timestamp + expiration),
            callbackContract: callbackContract,
            status: RequestStatus.Pending,
            claimedBy: address(0),
            claimedAt: 0,
            claimDeadline: 0,
            tip: msg.value,
            maxTip: msg.value
        });

        emit ExecutionRequested(
            requestId, msg.sender, imageId, inputDigest, inputUrl, inputType, msg.value, block.timestamp + expiration
        );
    }

    /// @notice Request execution with public input (backward-compatible overload)
    /// @param imageId The program to execute
    /// @param inputDigest Hash of the inputs
    /// @param inputUrl URL where prover can fetch inputs
    /// @param callbackContract Contract to receive results (0x0 for no callback)
    /// @param expirationSeconds How long before request expires (0 uses DEFAULT_EXPIRATION)
    /// @return requestId The unique ID assigned to this request
    function requestExecution(
        bytes32 imageId,
        bytes32 inputDigest,
        string calldata inputUrl,
        address callbackContract,
        uint256 expirationSeconds
    ) external payable whenNotPaused returns (uint256 requestId) {
        if (imageId == bytes32(0)) revert ZeroImageId();
        if (msg.value < MIN_TIP) revert InsufficientTip();
        if (!registry.isProgramActive(imageId)) revert ProgramNotActive();

        uint256 expiration = expirationSeconds > 0 ? expirationSeconds : DEFAULT_EXPIRATION;
        if (block.timestamp + expiration <= block.timestamp) revert ExpirationInPast();

        requestId = nextRequestId++;

        requests[requestId] = ExecutionRequest({
            id: requestId,
            imageId: imageId,
            inputDigest: inputDigest,
            requester: msg.sender,
            createdAt: uint48(block.timestamp),
            expiresAt: uint48(block.timestamp + expiration),
            callbackContract: callbackContract,
            status: RequestStatus.Pending,
            claimedBy: address(0),
            claimedAt: 0,
            claimDeadline: 0,
            tip: msg.value,
            maxTip: msg.value
        });

        emit ExecutionRequested(
            requestId, msg.sender, imageId, inputDigest, inputUrl, 0, msg.value, block.timestamp + expiration
        );
    }

    /// @notice Cancel a pending execution request and refund the tip
    /// @dev This function is deliberately NOT guarded by whenNotPaused so that users can
    ///      always recover their funds even when the contract is paused for emergency.
    ///      Only Pending requests can be cancelled -- once a prover has Claimed the request,
    ///      the requester must wait for the claim to expire (CLAIM_WINDOW) before the request
    ///      can be reclaimed by another prover. The full tip amount is refunded via low-level
    ///      call for contract wallet compatibility. If the ETH transfer to the requester fails
    ///      (e.g., requester is a contract that reverts on receive), the entire transaction
    ///      reverts with TransferFailed.
    /// @param requestId The request to cancel (must be Pending and owned by msg.sender)
    function cancelExecution(uint256 requestId) external nonReentrant {
        ExecutionRequest storage req = requests[requestId];
        if (req.id == 0) revert RequestNotFound();
        if (req.requester != msg.sender) revert NotRequester();
        if (req.status != RequestStatus.Pending) revert RequestNotPending();

        req.status = RequestStatus.Cancelled;

        // Refund tip
        (bool success,) = payable(msg.sender).call{value: req.tip}("");
        if (!success) revert TransferFailed();

        emit ExecutionCancelled(requestId);
    }

    // ========================================================================
    // PROVER OPERATIONS
    // ========================================================================

    /// @notice Claim an execution request (prover)
    /// @dev FRONTRUNNING WARNING: claimExecution is vulnerable to frontrunning. A competing
    ///      prover can observe a pending claimExecution transaction in the mempool and submit
    ///      their own claim with higher gas, winning the claim. Provers should consider using
    ///      Flashbots Protect or private mempools to mitigate this risk.
    ///
    ///      CLAIM ORDERING: Claims are first-come-first-served. The first transaction to be
    ///      mined wins the claim. If the request was previously Claimed but the claim deadline
    ///      (CLAIM_WINDOW = 10 minutes) has passed without proof submission, any prover can
    ///      reclaim it. The abandoned prover's address is recorded in the reputation system
    ///      (if configured) via recordAbandon(), which is wrapped in try/catch so reputation
    ///      failures never block reclaiming.
    ///
    ///      The claim deadline is set to block.timestamp + CLAIM_WINDOW. The prover must call
    ///      submitProof before this deadline or lose the claim.
    /// @param requestId The request to claim
    function claimExecution(uint256 requestId) external nonReentrant whenNotPaused {
        ExecutionRequest storage req = requests[requestId];
        if (req.id == 0) revert RequestNotFound();
        if (block.timestamp > req.expiresAt) revert RequestExpired();

        // If previously claimed but deadline passed, allow reclaim
        if (req.status == RequestStatus.Claimed) {
            if (block.timestamp <= req.claimDeadline) revert ClaimNotExpired();
            // Record abandon in reputation system
            if (address(reputation) != address(0)) {
                try reputation.recordAbandon(req.claimedBy, requestId) {} catch {}
            }
            emit ClaimExpired(requestId, req.claimedBy);
        } else if (req.status != RequestStatus.Pending) {
            revert RequestNotPending();
        }

        req.status = RequestStatus.Claimed;
        req.claimedBy = msg.sender;
        req.claimedAt = uint48(block.timestamp);
        req.claimDeadline = uint48(block.timestamp + CLAIM_WINDOW);

        emit ExecutionClaimed(requestId, msg.sender, req.claimDeadline);
    }

    /// @notice Submit proof for a claimed execution
    /// @dev After proof verification succeeds, the request is marked Completed and the prover
    ///      is paid immediately. The payout is calculated with tip decay (see calculatePayout)
    ///      and a protocol fee is deducted (protocolFeeBps basis points, sent to feeRecipient).
    ///
    ///      CALLBACK BEHAVIOR: If a callbackContract was specified in the request, the engine
    ///      calls IExecutionCallback.onExecutionComplete() after paying the prover. The callback
    ///      is wrapped in try/catch -- if it reverts, the proof is still valid and the prover is
    ///      still paid. A CallbackFailed event is emitted with the revert reason for observability.
    ///
    ///      GAS LIMITS FOR CALLBACKS: The callback receives all remaining gas after proof
    ///      verification and prover payment. Complex callbacks should be designed to stay within
    ///      the block gas limit. Callback contracts that consume excessive gas will cause the
    ///      entire submitProof transaction to run out of gas (the try/catch only catches reverts,
    ///      not out-of-gas at the call site). Callback implementors should keep logic minimal
    ///      and avoid unbounded loops or large storage writes.
    ///
    ///      REVERT BEHAVIOR: If the prover ETH transfer fails, the entire transaction reverts
    ///      (no proof recorded). If the fee transfer fails, it also reverts. Only the callback
    ///      is allowed to fail silently.
    /// @param requestId The request ID
    /// @param seal The proof seal (risc0 seal or Remainder proof)
    /// @param journal The public outputs (journal / public inputs)
    function submitProof(uint256 requestId, bytes calldata seal, bytes calldata journal)
        external
        nonReentrant
        whenNotPaused
    {
        ExecutionRequest storage req = requests[requestId];
        if (req.id == 0) revert RequestNotFound();
        if (req.status != RequestStatus.Claimed) revert RequestNotClaimed();
        if (req.claimedBy != msg.sender) revert NotClaimant();
        if (block.timestamp > req.claimDeadline) revert ClaimDeadlinePassed();
        if (seal.length == 0) revert EmptySeal();
        if (journal.length == 0) revert EmptyJournal();

        // Route verification to the appropriate verifier
        _verifyProof(req.imageId, seal, journal);

        // Mark as completed
        req.status = RequestStatus.Completed;

        // Calculate payout with tip decay
        uint256 payout = calculatePayout(req);
        uint256 fee = (payout * protocolFeeBps) / 10000;
        uint256 proverPayout = payout - fee;

        // Update prover stats
        proverCompletedCount[msg.sender]++;
        proverEarnings[msg.sender] += proverPayout;

        // Update reputation if configured
        if (address(reputation) != address(0)) {
            uint256 proofTimeMs = (block.timestamp - req.claimedAt) * 1000;
            try reputation.recordSuccess(msg.sender, proofTimeMs, proverPayout) {} catch {}
        }

        // Pay prover (using call instead of transfer for contract wallet compatibility)
        (bool proverPaid,) = payable(msg.sender).call{value: proverPayout}("");
        if (!proverPaid) revert TransferFailed();

        // Pay protocol fee
        if (fee > 0) {
            (bool feePaid,) = payable(feeRecipient).call{value: fee}("");
            if (!feePaid) revert TransferFailed();
        }

        emit ExecutionCompleted(requestId, msg.sender, sha256(journal), proverPayout);

        // Execute callback if specified
        if (req.callbackContract != address(0)) {
            try IExecutionCallback(req.callbackContract).onExecutionComplete(requestId, req.imageId, journal) {}
            catch (bytes memory reason) {
                // Callback failed but proof is still valid — emit event for observability
                emit CallbackFailed(requestId, req.callbackContract, reason);
            }
        }
    }

    // ========================================================================
    // VIEW FUNCTIONS
    // ========================================================================

    /// @notice Get current tip for a request (decreases over time via linear decay)
    /// @param requestId The request to query
    /// @return The current tip amount in wei (0 if request is completed/cancelled)
    function getCurrentTip(uint256 requestId) public view returns (uint256) {
        ExecutionRequest storage req = requests[requestId];
        if (req.id == 0) return 0;
        if (req.status != RequestStatus.Pending && req.status != RequestStatus.Claimed) {
            return 0;
        }

        return calculatePayout(req);
    }

    /// @notice Calculate payout with tip decay
    /// @dev Linear decay from maxTip to maxTip/2 over TIP_DECAY_PERIOD, then flat at maxTip/2
    /// @param req The execution request (storage reference)
    /// @return The current payout amount in wei
    function calculatePayout(ExecutionRequest storage req) internal view returns (uint256) {
        uint256 elapsed = block.timestamp - req.createdAt;

        if (elapsed >= TIP_DECAY_PERIOD) {
            // After decay period, tip is at minimum (50% of max)
            return req.maxTip / 2;
        }

        // Linear decay from maxTip to maxTip/2
        uint256 decay = (req.maxTip * elapsed) / (TIP_DECAY_PERIOD * 2);
        return req.maxTip - decay;
    }

    /// @notice Get a paginated list of pending (unclaimed, non-expired) requests
    /// @dev Iterates over all requests sequentially. Gas cost scales linearly with
    ///      nextRequestId, so this is intended for off-chain use only (eth_call).
    ///      For on-chain prover selection, use ExecutionRequested events instead.
    /// @param offset Number of matching requests to skip before returning (for pagination)
    /// @param limit Maximum number of request IDs to return
    /// @return Array of request IDs that are currently Pending and not expired
    function getPendingRequests(uint256 offset, uint256 limit) external view returns (uint256[] memory) {
        uint256[] memory pending = new uint256[](limit);
        uint256 count = 0;
        uint256 checked = 0;

        for (uint256 i = 1; i < nextRequestId && count < limit; i++) {
            ExecutionRequest storage req = requests[i];

            if (req.status == RequestStatus.Pending && block.timestamp <= req.expiresAt) {
                if (checked >= offset) {
                    pending[count++] = i;
                }
                checked++;
            }
        }

        // Resize array
        uint256[] memory result = new uint256[](count);
        for (uint256 i = 0; i < count; i++) {
            result[i] = pending[i];
        }

        return result;
    }

    /// @notice Get the full details of an execution request
    /// @dev Reverts with RequestNotFound if the request ID has never been created.
    ///      Returns a memory copy of the request struct (not a storage reference).
    /// @param requestId The request to query
    /// @return The full ExecutionRequest struct including status, tip, claim info, and timestamps
    function getRequest(uint256 requestId) external view returns (ExecutionRequest memory) {
        if (requests[requestId].id == 0) revert RequestNotFound();
        return requests[requestId];
    }

    /// @notice Get cumulative statistics for a prover address
    /// @dev These counters are incremented in submitProof and accumulate monotonically over
    ///      the lifetime of the contract. Earnings reflect the prover's share after protocol
    ///      fee deduction (not the full tip amount). Note that earnings represent historical
    ///      totals -- they are never decremented, even if the prover has already withdrawn
    ///      (ETH is sent immediately in submitProof, not held in escrow). The earnings value
    ///      is therefore a running total of all payouts sent to the prover, useful for
    ///      reputation scoring and analytics but not an indicator of claimable balance.
    ///      Returns (0, 0) for addresses that have never submitted a proof.
    /// @param prover The prover address to query
    /// @return completed Number of proofs successfully submitted by this prover
    /// @return earnings Total ETH earned from payouts (after protocol fee deduction)
    function getProverStats(address prover) external view returns (uint256 completed, uint256 earnings) {
        return (proverCompletedCount[prover], proverEarnings[prover]);
    }

    // ========================================================================
    // PROOF VERIFICATION ROUTING
    // ========================================================================

    /// @notice Route proof verification to the appropriate verifier based on program registration
    /// @dev If the program has a custom verifier registered in ProgramRegistry (e.g., Remainder GKR,
    ///      eZKL), the proof is forwarded to that verifier via IProofVerifier.verify(). Otherwise,
    ///      the default RISC Zero verifier is used with sha256(journal) as the journal digest.
    ///      Both paths revert on invalid proofs, so callers can assume success if this returns.
    /// @param imageId The program image ID used to look up the verifier
    /// @param seal The proof bytes (format depends on the verifier)
    /// @param journal The public outputs from execution
    function _verifyProof(bytes32 imageId, bytes calldata seal, bytes calldata journal) internal view {
        // Check if program has a custom verifier
        ProgramRegistry.Program memory program = registry.getProgram(imageId);

        if (program.verifierContract != address(0)) {
            // Use program-specific verifier (Remainder, eZKL, etc.)
            // This reverts on invalid proof
            IProofVerifier(program.verifierContract).verify(seal, imageId, journal);
        } else {
            // Default: use the RISC Zero verifier (backward compatible)
            bytes32 journalDigest = sha256(journal);
            verifier.verify(seal, imageId, journalDigest);
        }
    }

    // ========================================================================
    // ADMIN
    // ========================================================================

    /// @notice Pause the contract, blocking new requests, claims, and proof submissions
    /// @dev Only the owner can pause. cancelExecution deliberately remains available
    ///      when paused so users can always recover their funds. Uses OpenZeppelin Pausable.
    function pause() external onlyOwner {
        _pause();
    }

    /// @notice Unpause the contract, re-enabling requests, claims, and proof submissions
    /// @dev Only the owner can unpause. Emits an Unpaused event (from OpenZeppelin Pausable).
    function unpause() external onlyOwner {
        _unpause();
    }

    /// @notice Update the protocol fee charged on each payout
    /// @dev The fee is deducted from the prover's payout and sent to feeRecipient.
    ///      Capped at 1000 bps (10%) to prevent excessive extraction.
    /// @param _feeBps New fee in basis points (100 = 1%, max 1000 = 10%)
    function setProtocolFee(uint256 _feeBps) external onlyOwner {
        require(_feeBps <= MAX_FEE_BPS, "Fee too high"); // Max 10%
        uint256 oldFeeBps = protocolFeeBps;
        protocolFeeBps = _feeBps;
        emit ProtocolFeeUpdated(oldFeeBps, _feeBps);
    }

    /// @notice Update the address that receives protocol fees from payouts
    /// @dev Reverts with ZeroAddress if _recipient is address(0). Takes effect immediately
    ///      for all subsequent proof submissions. Emits FeeRecipientUpdated.
    /// @param _recipient New address to receive protocol fees (must be non-zero)
    function setFeeRecipient(address _recipient) external onlyOwner {
        if (_recipient == address(0)) revert ZeroAddress();
        address oldRecipient = feeRecipient;
        feeRecipient = _recipient;
        emit FeeRecipientUpdated(oldRecipient, _recipient);
    }

    /// @notice Set or update the reputation tracking contract
    /// @dev Pass address(0) to disable reputation tracking. When enabled, recordSuccess
    ///      and recordAbandon are called during proof submission and claim expiration
    ///      respectively. Failures in reputation calls are silently caught (try/catch)
    ///      so they never block core execution logic.
    ///
    ///      REQUIREMENTS: The supplied address must implement the ProverReputation interface
    ///      (recordSuccess(address,uint256,uint256) and recordAbandon(address,uint256)).
    ///      If it does not, the try/catch will silently swallow reverts, effectively
    ///      disabling reputation tracking without any visible error. There is no interface
    ///      check (ERC-165) performed here -- the owner is trusted to provide a valid address.
    ///
    ///      Takes effect immediately for all subsequent proof submissions and claim expirations.
    ///      Changing the reputation contract does not migrate data from the previous contract.
    /// @param _reputation ProverReputation contract address (address(0) to disable)
    function setReputation(address _reputation) external onlyOwner {
        reputation = ProverReputation(_reputation);
        emit ReputationContractSet(_reputation);
    }
}
