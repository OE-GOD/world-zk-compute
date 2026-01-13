// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

/// @title IRiscZeroVerifier
/// @notice Standard interface for RISC Zero verifiers
interface IRiscZeroVerifier {
    /// @notice Verify a RISC Zero proof
    /// @param seal The proof seal (SNARK proof)
    /// @param imageId The program image ID
    /// @param journalDigest SHA256 hash of the journal (public outputs)
    function verify(
        bytes calldata seal,
        bytes32 imageId,
        bytes32 journalDigest
    ) external view;
}

/// @title RiscZeroVerifierRouter
/// @notice Routes verification to the appropriate RISC Zero verifier based on proof type
/// @dev Supports multiple verifier versions and proof systems (Groth16, STARK, etc.)
contract RiscZeroVerifierRouter is IRiscZeroVerifier {

    // ========================================================================
    // TYPES
    // ========================================================================

    struct VerifierInfo {
        address verifier;
        string name;
        bool active;
    }

    // ========================================================================
    // STATE
    // ========================================================================

    /// @notice Admin who can add/remove verifiers
    address public admin;

    /// @notice Mapping of selector (first 4 bytes of seal) to verifier
    mapping(bytes4 => VerifierInfo) public verifiers;

    /// @notice List of all registered selectors
    bytes4[] public selectors;

    /// @notice Default verifier for unknown selectors
    address public defaultVerifier;

    // ========================================================================
    // EVENTS
    // ========================================================================

    event VerifierAdded(bytes4 indexed selector, address verifier, string name);
    event VerifierRemoved(bytes4 indexed selector);
    event DefaultVerifierSet(address verifier);
    event AdminTransferred(address indexed oldAdmin, address indexed newAdmin);

    // ========================================================================
    // ERRORS
    // ========================================================================

    error NotAdmin();
    error InvalidSeal();
    error NoVerifierFound();
    error VerifierNotActive();
    error VerificationFailed();

    // ========================================================================
    // CONSTRUCTOR
    // ========================================================================

    constructor(address _admin) {
        admin = _admin;
    }

    // ========================================================================
    // VERIFICATION
    // ========================================================================

    /// @notice Verify a proof by routing to the appropriate verifier
    /// @param seal The proof seal (first 4 bytes determine the verifier)
    /// @param imageId The program image ID
    /// @param journalDigest SHA256 hash of the journal
    function verify(
        bytes calldata seal,
        bytes32 imageId,
        bytes32 journalDigest
    ) external view override {
        if (seal.length < 4) revert InvalidSeal();

        // Extract selector from seal
        bytes4 selector = bytes4(seal[:4]);

        // Find verifier
        VerifierInfo storage info = verifiers[selector];
        address verifier = info.verifier;

        // Fall back to default if not found
        if (verifier == address(0)) {
            verifier = defaultVerifier;
        }

        if (verifier == address(0)) revert NoVerifierFound();
        if (!info.active && verifier != defaultVerifier) revert VerifierNotActive();

        // Call the verifier
        try IRiscZeroVerifier(verifier).verify(seal, imageId, journalDigest) {
            // Verification succeeded
        } catch {
            revert VerificationFailed();
        }
    }

    // ========================================================================
    // ADMIN FUNCTIONS
    // ========================================================================

    modifier onlyAdmin() {
        if (msg.sender != admin) revert NotAdmin();
        _;
    }

    /// @notice Add a new verifier for a specific selector
    function addVerifier(
        bytes4 selector,
        address verifier,
        string calldata name
    ) external onlyAdmin {
        verifiers[selector] = VerifierInfo({
            verifier: verifier,
            name: name,
            active: true
        });
        selectors.push(selector);

        emit VerifierAdded(selector, verifier, name);
    }

    /// @notice Remove a verifier
    function removeVerifier(bytes4 selector) external onlyAdmin {
        verifiers[selector].active = false;
        emit VerifierRemoved(selector);
    }

    /// @notice Set the default verifier
    function setDefaultVerifier(address verifier) external onlyAdmin {
        defaultVerifier = verifier;
        emit DefaultVerifierSet(verifier);
    }

    /// @notice Transfer admin rights
    function transferAdmin(address newAdmin) external onlyAdmin {
        emit AdminTransferred(admin, newAdmin);
        admin = newAdmin;
    }

    // ========================================================================
    // VIEW FUNCTIONS
    // ========================================================================

    /// @notice Get all registered selectors
    function getSelectors() external view returns (bytes4[] memory) {
        return selectors;
    }

    /// @notice Check if a selector has an active verifier
    function hasVerifier(bytes4 selector) external view returns (bool) {
        return verifiers[selector].active || defaultVerifier != address(0);
    }
}
