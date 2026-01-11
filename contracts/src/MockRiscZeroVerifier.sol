// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

/// @title MockRiscZeroVerifier
/// @notice Mock verifier for testing (accepts all proofs)
/// @dev In production, use RISC Zero's official verifier contracts
contract MockRiscZeroVerifier {

    /// @notice Verify a RISC Zero proof
    /// @dev This mock always succeeds - use real verifier in production
    function verify(
        bytes calldata seal,
        bytes32 imageId,
        bytes32 journalDigest
    ) external pure {
        // In production, this would verify the STARK/Groth16 proof
        // For testing, we just check that seal is not empty
        require(seal.length > 0, "Empty seal");
        require(imageId != bytes32(0), "Invalid image ID");
        require(journalDigest != bytes32(0), "Invalid journal digest");
    }

    /// @notice Verify with journal bytes
    function verifyWithJournal(
        bytes calldata seal,
        bytes32 imageId,
        bytes calldata journal
    ) external pure {
        require(seal.length > 0, "Empty seal");
        require(imageId != bytes32(0), "Invalid image ID");
        require(journal.length > 0, "Empty journal");
    }
}
