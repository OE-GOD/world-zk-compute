// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

/// @title IProgramRegistry -- Interface for the zkVM program registry
/// @notice Defines the public API for registering, managing, and querying zkVM programs
///         that can be executed on World ZK Compute.
interface IProgramRegistry {
    // ========================================================================
    // TYPES
    // ========================================================================

    /// @notice Program metadata and configuration
    struct Program {
        bytes32 imageId;
        address owner;
        string name;
        string programUrl;
        bytes32 inputSchema;
        uint256 registeredAt;
        bool active;
        bool verified;
        address verifierContract;
        string proofSystem;
    }

    // ========================================================================
    // EVENTS
    // ========================================================================

    /// @notice Emitted when a new program is registered
    event ProgramRegistered(bytes32 indexed imageId, address indexed owner, string name, string programUrl);

    /// @notice Emitted when a program URL is updated
    event ProgramUpdated(bytes32 indexed imageId, string programUrl);

    /// @notice Emitted when a program is deactivated by admin
    event ProgramDeactivated(bytes32 indexed imageId);

    /// @notice Emitted when a program is reactivated by admin
    event ProgramReactivated(bytes32 indexed imageId);

    /// @notice Emitted when a program is verified by admin
    event ProgramVerified(bytes32 indexed imageId);

    /// @notice Emitted when a program's verification is removed by admin
    event ProgramUnverified(bytes32 indexed imageId);

    // ========================================================================
    // ERRORS
    // ========================================================================

    /// @notice Thrown when attempting to register a program that already exists
    error ProgramAlreadyRegistered();

    /// @notice Thrown when the specified program does not exist
    error ProgramNotFound();

    /// @notice Thrown when msg.sender is not the program owner
    error NotProgramOwner();

    /// @notice Thrown when imageId is bytes32(0)
    error InvalidImageId();

    /// @notice Thrown when verifying an already-verified program
    error ProgramAlreadyVerified();

    /// @notice Thrown when unverifying a program that is not verified
    error ProgramNotVerified();

    /// @notice Thrown when activating an already-active program
    error ProgramAlreadyActive();

    /// @notice Thrown when deactivating an already-inactive program
    error ProgramAlreadyInactive();

    // ========================================================================
    // EXTERNAL FUNCTIONS
    // ========================================================================

    /// @notice Register a new zkVM program (backward-compatible, defaults to risc0)
    /// @param imageId The RISC Zero image ID (hash of the ELF binary)
    /// @param name Human-readable name for the program
    /// @param programUrl URL where the ELF binary can be downloaded
    /// @param inputSchema Optional hash of the expected input schema
    function registerProgram(bytes32 imageId, string calldata name, string calldata programUrl, bytes32 inputSchema)
        external;

    /// @notice Register a program with a specific proof system verifier
    /// @param imageId The program ID (image ID for risc0, circuit hash for Remainder)
    /// @param name Human-readable name for the program
    /// @param programUrl URL where the program binary can be downloaded
    /// @param inputSchema Optional hash of the expected input schema
    /// @param verifierContract Address of the IProofVerifier for this program (0x0 = default)
    /// @param _proofSystem Proof system identifier ("risc0", "remainder", "ezkl")
    function registerProgramWithVerifier(
        bytes32 imageId,
        string calldata name,
        string calldata programUrl,
        bytes32 inputSchema,
        address verifierContract,
        string calldata _proofSystem
    ) external;

    /// @notice Update program URL (only program owner)
    /// @param imageId The program ID to update
    /// @param newUrl New URL where the program binary can be downloaded
    function updateProgramUrl(bytes32 imageId, string calldata newUrl) external;

    /// @notice Deactivate a program (only contract owner/admin)
    /// @param imageId The program ID to deactivate
    function deactivateProgram(bytes32 imageId) external;

    /// @notice Reactivate a program (only contract owner/admin)
    /// @param imageId The program ID to reactivate
    function reactivateProgram(bytes32 imageId) external;

    /// @notice Update the verifier contract for a program (only program owner)
    /// @param imageId The program ID
    /// @param verifierContract New IProofVerifier address (0x0 = use default)
    function updateVerifier(bytes32 imageId, address verifierContract) external;

    /// @notice Mark a program as verified (admin trust signal)
    /// @param imageId The program ID to verify
    function verifyProgram(bytes32 imageId) external;

    /// @notice Remove verification from a program (admin)
    /// @param imageId The program ID to unverify
    function unverifyProgram(bytes32 imageId) external;

    /// @notice Pause program registration (admin only)
    function pause() external;

    /// @notice Unpause program registration (admin only)
    function unpause() external;

    /// @notice Check if a program is registered and active
    /// @param imageId The program ID to check
    /// @return True if the program exists and is active
    function isProgramActive(bytes32 imageId) external view returns (bool);

    /// @notice Check if a program is verified by admin
    /// @param imageId The program ID to check
    /// @return True if the program exists and is verified
    function isProgramVerified(bytes32 imageId) external view returns (bool);

    /// @notice Get program details
    /// @param imageId The program ID to query
    /// @return The full Program struct
    function getProgram(bytes32 imageId) external view returns (Program memory);

    /// @notice Get total number of registered programs
    /// @return The count of all registered programs
    function getProgramCount() external view returns (uint256);

    /// @notice Get programs owned by an address
    /// @param owner The owner address to query
    /// @return Array of image IDs owned by the address
    function getOwnerPrograms(address owner) external view returns (bytes32[] memory);

    /// @notice Get all program IDs (paginated)
    /// @param offset Starting index
    /// @param limit Maximum number of IDs to return
    /// @return Array of program image IDs
    function getAllPrograms(uint256 offset, uint256 limit) external view returns (bytes32[] memory);
}
