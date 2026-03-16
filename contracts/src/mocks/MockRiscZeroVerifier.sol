// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import {IRiscZeroVerifier, Receipt} from "risc0-ethereum/IRiscZeroVerifier.sol";

/// @title MockRiscZeroVerifier
/// @notice Mock verifier for testing (accepts all proofs)
/// @dev In production, use RISC Zero's official verifier contracts
contract MockRiscZeroVerifier is IRiscZeroVerifier {
    error EmptySeal();
    error InvalidImageId();
    error InvalidJournalDigest();
    error InvalidClaimDigest();

    /// @notice Verify a RISC Zero proof
    /// @dev This mock always succeeds - use real verifier in production
    function verify(bytes calldata seal, bytes32 imageId, bytes32 journalDigest) external pure override {
        // In production, this would verify the STARK/Groth16 proof
        // For testing, we just check that seal is not empty
        if (seal.length == 0) revert EmptySeal();
        if (imageId == bytes32(0)) revert InvalidImageId();
        if (journalDigest == bytes32(0)) revert InvalidJournalDigest();
    }

    /// @notice Verify receipt integrity (mock always succeeds)
    function verifyIntegrity(Receipt calldata receipt) external pure override {
        if (receipt.seal.length == 0) revert EmptySeal();
        if (receipt.claimDigest == bytes32(0)) revert InvalidClaimDigest();
    }
}
