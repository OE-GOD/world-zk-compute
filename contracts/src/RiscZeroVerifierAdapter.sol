// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import {IProofVerifier} from "./IProofVerifier.sol";
import {IRiscZeroVerifier} from "risc0-ethereum/IRiscZeroVerifier.sol";

/// @title RiscZeroVerifierAdapter
/// @notice Adapts the existing IRiscZeroVerifier to the IProofVerifier interface
/// @dev Wraps the risc0 verifier (or verifier router) so that the ExecutionEngine
///      can use a unified verification interface for all proof systems.
///
///      Proof format: seal bytes (risc0 proof with 4-byte selector prefix)
///      Program ID: imageId (32-byte commitment to the guest ELF)
///      Public data: journal bytes (sha256-hashed for verification)
contract RiscZeroVerifierAdapter is IProofVerifier {
    /// @notice The underlying RISC Zero verifier (or router)
    IRiscZeroVerifier public immutable riscZeroVerifier;

    constructor(address _verifier) {
        riscZeroVerifier = IRiscZeroVerifier(_verifier);
    }

    /// @notice Verify a RISC Zero proof
    /// @param proofData The proof seal (starts with 4-byte selector)
    /// @param programId The image ID of the guest program
    /// @param publicData The journal (public outputs)
    function verify(bytes calldata proofData, bytes32 programId, bytes calldata publicData) external view override {
        bytes32 journalDigest = sha256(publicData);
        // This reverts on invalid proof
        riscZeroVerifier.verify(proofData, programId, journalDigest);
    }

    /// @notice Return the proof system name
    function proofSystem() external pure override returns (string memory) {
        return "risc0";
    }
}
