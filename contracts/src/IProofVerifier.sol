// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

/// @title IProofVerifier
/// @notice Generic interface for proof verification across different proof systems
/// @dev Adapters implement this interface to wrap system-specific verifiers
///      (RISC Zero, Remainder, eZKL, etc.) behind a unified API.
///
///      The ExecutionEngine uses this interface to route verification to
///      the correct verifier based on the program's registered proof system.
interface IProofVerifier {
    /// @notice Verify a proof — reverts if invalid
    /// @param proofData The proof bytes (format depends on proof system)
    /// @param programId Program identifier (imageId for risc0, circuitHash for Remainder)
    /// @param publicData Public outputs / journal
    function verify(bytes calldata proofData, bytes32 programId, bytes calldata publicData) external view;

    /// @notice Return the name of the proof system
    function proofSystem() external view returns (string memory);
}
