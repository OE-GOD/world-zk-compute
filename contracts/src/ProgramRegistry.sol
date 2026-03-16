// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import {Ownable2Step, Ownable} from "@openzeppelin/contracts/access/Ownable2Step.sol";
import {Pausable} from "@openzeppelin/contracts/utils/Pausable.sol";

/// @title ProgramRegistry
/// @notice Registry for zkVM programs that can be executed on World ZK Compute
/// @dev Programs are identified by their RISC Zero image ID (deterministic hash of the ELF).
///      Admin (via Ownable2Step) can verify/unverify programs and pause registration.
contract ProgramRegistry is Ownable2Step, Pausable {
    // ========================================================================
    // TYPES
    // ========================================================================

    struct Program {
        bytes32 imageId; // RISC Zero image ID (commitment to program)
        address owner; // Who deployed this program
        string name; // Human-readable name
        string programUrl; // URL to download the ELF binary
        bytes32 inputSchema; // Hash of expected input schema (optional)
        uint256 registeredAt; // Block timestamp of registration
        bool active; // Whether program is active
        bool verified; // Whether program has been verified by admin
        address verifierContract; // IProofVerifier address (0x0 = use default risc0)
        string proofSystem; // "risc0" | "remainder" | "ezkl" (empty = risc0)
    }

    // ========================================================================
    // STATE
    // ========================================================================

    /// @notice All registered programs by image ID
    mapping(bytes32 => Program) public programs;

    /// @notice List of all image IDs for enumeration
    bytes32[] public programIds;

    /// @notice Programs owned by each address
    mapping(address => bytes32[]) public ownerPrograms;

    // ========================================================================
    // EVENTS
    // ========================================================================

    event ProgramRegistered(bytes32 indexed imageId, address indexed owner, string name, string programUrl);

    event ProgramUpdated(bytes32 indexed imageId, string programUrl);

    event ProgramDeactivated(bytes32 indexed imageId);
    event ProgramReactivated(bytes32 indexed imageId);

    event ProgramVerified(bytes32 indexed imageId);
    event ProgramUnverified(bytes32 indexed imageId);

    event VerifierUpdated(bytes32 indexed programId, address oldVerifier, address newVerifier);

    event ProgramUrlForceUpdated(bytes32 indexed imageId, string newUrl, address admin);

    // ========================================================================
    // CONSTANTS
    // ========================================================================

    /// @notice Maximum length for program name strings
    uint256 public constant MAX_NAME_LENGTH = 64;

    /// @notice Maximum length for program URL strings
    uint256 public constant MAX_URL_LENGTH = 512;

    // ========================================================================
    // ERRORS
    // ========================================================================

    error ProgramAlreadyRegistered();
    error ProgramNotFound();
    error NotProgramOwner();
    error InvalidImageId();
    error ProgramAlreadyVerified();
    error ProgramNotVerified();
    error ProgramAlreadyActive();
    error ProgramAlreadyInactive();
    error InvalidVerifierContract();
    error StringTooLong(string field, uint256 maxLength);

    // ========================================================================
    // CONSTRUCTOR
    // ========================================================================

    /// @param _admin The initial owner/admin address
    constructor(address _admin) Ownable(_admin) {}

    // ========================================================================
    // REGISTRATION
    // ========================================================================

    /// @notice Register a new zkVM program (backward-compatible, defaults to risc0)
    /// @param imageId The RISC Zero image ID (hash of the ELF binary)
    /// @param name Human-readable name for the program
    /// @param programUrl URL where the ELF binary can be downloaded
    /// @param inputSchema Optional hash of the expected input schema
    function registerProgram(bytes32 imageId, string calldata name, string calldata programUrl, bytes32 inputSchema)
        external
        whenNotPaused
    {
        _registerProgram(imageId, name, programUrl, inputSchema, address(0), "risc0");
    }

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
    ) external whenNotPaused {
        _registerProgram(imageId, name, programUrl, inputSchema, verifierContract, _proofSystem);
    }

    function _registerProgram(
        bytes32 imageId,
        string calldata name,
        string calldata programUrl,
        bytes32 inputSchema,
        address verifierContract,
        string memory _proofSystem
    ) internal {
        if (imageId == bytes32(0)) revert InvalidImageId();
        if (bytes(name).length > MAX_NAME_LENGTH) revert StringTooLong("name", MAX_NAME_LENGTH);
        if (bytes(programUrl).length > MAX_URL_LENGTH) revert StringTooLong("url", MAX_URL_LENGTH);
        if (programs[imageId].registeredAt != 0) revert ProgramAlreadyRegistered();

        programs[imageId] = Program({
            imageId: imageId,
            owner: msg.sender,
            name: name,
            programUrl: programUrl,
            inputSchema: inputSchema,
            registeredAt: block.timestamp,
            active: true,
            verified: false,
            verifierContract: verifierContract,
            proofSystem: _proofSystem
        });

        programIds.push(imageId);
        ownerPrograms[msg.sender].push(imageId);

        emit ProgramRegistered(imageId, msg.sender, name, programUrl);
    }

    /// @notice Update program URL (only program owner)
    /// @param imageId The image ID of the program to update
    /// @param newUrl The new URL where the program binary can be downloaded
    function updateProgramUrl(bytes32 imageId, string calldata newUrl) external {
        if (bytes(newUrl).length > MAX_URL_LENGTH) revert StringTooLong("url", MAX_URL_LENGTH);
        Program storage program = programs[imageId];
        if (program.registeredAt == 0) revert ProgramNotFound();
        if (program.owner != msg.sender) revert NotProgramOwner();

        program.programUrl = newUrl;

        emit ProgramUpdated(imageId, newUrl);
    }

    /// @notice Deactivate a program (only contract owner/admin)
    /// @param imageId The image ID of the program to deactivate
    function deactivateProgram(bytes32 imageId) external onlyOwner {
        Program storage program = programs[imageId];
        if (program.registeredAt == 0) revert ProgramNotFound();
        if (!program.active) revert ProgramAlreadyInactive();

        program.active = false;

        emit ProgramDeactivated(imageId);
    }

    /// @notice Reactivate a program (only contract owner/admin)
    /// @param imageId The image ID of the program to reactivate
    function reactivateProgram(bytes32 imageId) external onlyOwner {
        Program storage program = programs[imageId];
        if (program.registeredAt == 0) revert ProgramNotFound();
        if (program.active) revert ProgramAlreadyActive();

        program.active = true;

        emit ProgramReactivated(imageId);
    }

    /// @notice Update the verifier contract for a program (only program owner)
    /// @param imageId The program ID
    /// @param verifierContract New IProofVerifier address (must be a deployed contract, not EOA or address(0))
    function updateVerifier(bytes32 imageId, address verifierContract) external {
        Program storage program = programs[imageId];
        if (program.registeredAt == 0) revert ProgramNotFound();
        if (program.owner != msg.sender) revert NotProgramOwner();
        if (verifierContract == address(0)) revert InvalidVerifierContract();
        if (verifierContract.code.length == 0) revert InvalidVerifierContract();

        address oldVerifier = program.verifierContract;
        program.verifierContract = verifierContract;

        emit VerifierUpdated(imageId, oldVerifier, verifierContract);
    }

    // ========================================================================
    // ADMIN: VERIFICATION
    // ========================================================================

    /// @notice Mark a program as verified (admin trust signal)
    /// @param imageId The program ID to verify
    function verifyProgram(bytes32 imageId) external onlyOwner {
        Program storage program = programs[imageId];
        if (program.registeredAt == 0) revert ProgramNotFound();
        if (program.verified) revert ProgramAlreadyVerified();

        program.verified = true;

        emit ProgramVerified(imageId);
    }

    /// @notice Remove verification from a program (admin)
    /// @param imageId The program ID to unverify
    function unverifyProgram(bytes32 imageId) external onlyOwner {
        Program storage program = programs[imageId];
        if (program.registeredAt == 0) revert ProgramNotFound();
        if (!program.verified) revert ProgramNotVerified();

        program.verified = false;

        emit ProgramUnverified(imageId);
    }

    /// @notice Emergency: force-update a program URL when the registrant's key is compromised.
    /// @dev This is an admin-only override. Use only when the program owner's private key has been
    ///      compromised and the attacker could call updateProgramUrl to point to a malicious binary.
    ///      Under normal circumstances, the program owner should use updateProgramUrl instead.
    /// @param imageId The image ID of the program to update
    /// @param newUrl The new URL to set
    function adminForceUpdateUrl(bytes32 imageId, string calldata newUrl) external onlyOwner {
        if (bytes(newUrl).length > MAX_URL_LENGTH) revert StringTooLong("url", MAX_URL_LENGTH);
        Program storage program = programs[imageId];
        if (program.registeredAt == 0) revert ProgramNotFound();

        program.programUrl = newUrl;

        emit ProgramUrlForceUpdated(imageId, newUrl, msg.sender);
    }

    // ========================================================================
    // ADMIN: PAUSABLE
    // ========================================================================

    /// @notice Pause program registration (admin only)
    function pause() external onlyOwner {
        _pause();
    }

    /// @notice Unpause program registration (admin only)
    function unpause() external onlyOwner {
        _unpause();
    }

    // ========================================================================
    // VIEW FUNCTIONS
    // ========================================================================

    /// @notice Check if a program is registered and active
    /// @param imageId The image ID of the program to check
    /// @return True if the program is registered and currently active
    function isProgramActive(bytes32 imageId) external view returns (bool) {
        Program storage program = programs[imageId];
        return program.registeredAt != 0 && program.active;
    }

    /// @notice Check if a program is verified by admin
    /// @param imageId The image ID of the program to check
    /// @return True if the program is registered and verified by admin
    function isProgramVerified(bytes32 imageId) external view returns (bool) {
        Program storage program = programs[imageId];
        return program.registeredAt != 0 && program.verified;
    }

    /// @notice Get program details
    /// @param imageId The image ID of the program to retrieve
    /// @return The full Program struct for the given image ID
    function getProgram(bytes32 imageId) external view returns (Program memory) {
        if (programs[imageId].registeredAt == 0) revert ProgramNotFound();
        return programs[imageId];
    }

    /// @notice Get total number of registered programs
    /// @return The count of all registered programs (active and inactive)
    function getProgramCount() external view returns (uint256) {
        return programIds.length;
    }

    /// @notice Get programs owned by an address
    /// @param owner The address whose programs to retrieve
    /// @return Array of image IDs registered by the given owner
    function getOwnerPrograms(address owner) external view returns (bytes32[] memory) {
        return ownerPrograms[owner];
    }

    /// @notice Get programs owned by an address (paginated)
    /// @param owner The address whose programs to retrieve
    /// @param offset The starting index
    /// @param limit The maximum number of items to return
    /// @return Array of image IDs in the requested page range
    function getOwnerPrograms(address owner, uint256 offset, uint256 limit) external view returns (bytes32[] memory) {
        bytes32[] storage programs = ownerPrograms[owner];
        uint256 total = programs.length;
        if (offset >= total) {
            return new bytes32[](0);
        }
        uint256 end = offset + limit;
        if (end > total) {
            end = total;
        }
        bytes32[] memory result = new bytes32[](end - offset);
        for (uint256 i = offset; i < end; i++) {
            result[i - offset] = programs[i];
        }
        return result;
    }

    /// @notice Get the total number of programs owned by an address
    /// @param owner The address whose program count to retrieve
    /// @return The count of programs owned by the address
    function totalOwnerPrograms(address owner) external view returns (uint256) {
        return ownerPrograms[owner].length;
    }

    /// @notice Get all program IDs (paginated)
    /// @return Array of image IDs in the requested page range
    function getAllPrograms(uint256 offset, uint256 limit) external view returns (bytes32[] memory) {
        uint256 total = programIds.length;
        if (offset >= total) {
            return new bytes32[](0);
        }

        uint256 end = offset + limit;
        if (end > total) {
            end = total;
        }

        bytes32[] memory result = new bytes32[](end - offset);
        for (uint256 i = offset; i < end; i++) {
            result[i - offset] = programIds[i];
        }

        return result;
    }
}
