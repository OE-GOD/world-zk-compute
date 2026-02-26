// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

/// @title ProgramRegistry
/// @notice Registry for zkVM programs that can be executed on World ZK Compute
/// @dev Programs are identified by their RISC Zero image ID (deterministic hash of the ELF)
contract ProgramRegistry {
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

    // ========================================================================
    // ERRORS
    // ========================================================================

    error ProgramAlreadyRegistered();
    error ProgramNotFound();
    error NotProgramOwner();
    error InvalidImageId();

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
    ) external {
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
        if (programs[imageId].registeredAt != 0) revert ProgramAlreadyRegistered();

        programs[imageId] = Program({
            imageId: imageId,
            owner: msg.sender,
            name: name,
            programUrl: programUrl,
            inputSchema: inputSchema,
            registeredAt: block.timestamp,
            active: true,
            verifierContract: verifierContract,
            proofSystem: _proofSystem
        });

        programIds.push(imageId);
        ownerPrograms[msg.sender].push(imageId);

        emit ProgramRegistered(imageId, msg.sender, name, programUrl);
    }

    /// @notice Update program URL (only owner)
    function updateProgramUrl(bytes32 imageId, string calldata newUrl) external {
        Program storage program = programs[imageId];
        if (program.registeredAt == 0) revert ProgramNotFound();
        if (program.owner != msg.sender) revert NotProgramOwner();

        program.programUrl = newUrl;

        emit ProgramUpdated(imageId, newUrl);
    }

    /// @notice Deactivate a program (only owner)
    function deactivateProgram(bytes32 imageId) external {
        Program storage program = programs[imageId];
        if (program.registeredAt == 0) revert ProgramNotFound();
        if (program.owner != msg.sender) revert NotProgramOwner();

        program.active = false;

        emit ProgramDeactivated(imageId);
    }

    /// @notice Reactivate a program (only owner)
    function reactivateProgram(bytes32 imageId) external {
        Program storage program = programs[imageId];
        if (program.registeredAt == 0) revert ProgramNotFound();
        if (program.owner != msg.sender) revert NotProgramOwner();

        program.active = true;

        emit ProgramReactivated(imageId);
    }

    /// @notice Update the verifier contract for a program (only owner)
    /// @param imageId The program ID
    /// @param verifierContract New IProofVerifier address (0x0 = use default)
    function updateVerifier(bytes32 imageId, address verifierContract) external {
        Program storage program = programs[imageId];
        if (program.registeredAt == 0) revert ProgramNotFound();
        if (program.owner != msg.sender) revert NotProgramOwner();

        program.verifierContract = verifierContract;
    }

    // ========================================================================
    // VIEW FUNCTIONS
    // ========================================================================

    /// @notice Check if a program is registered and active
    function isProgramActive(bytes32 imageId) external view returns (bool) {
        Program storage program = programs[imageId];
        return program.registeredAt != 0 && program.active;
    }

    /// @notice Get program details
    function getProgram(bytes32 imageId) external view returns (Program memory) {
        if (programs[imageId].registeredAt == 0) revert ProgramNotFound();
        return programs[imageId];
    }

    /// @notice Get total number of registered programs
    function getProgramCount() external view returns (uint256) {
        return programIds.length;
    }

    /// @notice Get programs owned by an address
    function getOwnerPrograms(address owner) external view returns (bytes32[] memory) {
        return ownerPrograms[owner];
    }

    /// @notice Get all program IDs (paginated)
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
