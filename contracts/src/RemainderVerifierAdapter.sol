// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import {IProofVerifier} from "./IProofVerifier.sol";
import {RemainderVerifier} from "./remainder/RemainderVerifier.sol";

/// @title RemainderVerifierAdapter
/// @notice Adapts RemainderVerifier to the IProofVerifier interface
/// @dev Wraps the Remainder GKR+Hyrax verifier so that the ExecutionEngine
///      can route Remainder proofs through the same interface as risc0.
///
///      Proof format: ABI-encoded GKR+Hyrax proof (starts with "REM1" selector)
///      Program ID: circuitHash (SHA-256 of circuit description)
///      Public data: public inputs (field elements)
contract RemainderVerifierAdapter is IProofVerifier {
    /// @notice The underlying Remainder verifier
    RemainderVerifier public immutable remainderVerifier;

    constructor(address _verifier) {
        remainderVerifier = RemainderVerifier(_verifier);
    }

    /// @notice Verify a Remainder proof
    /// @param proofData ABI-encoded GKR+Hyrax proof (optionally with appended Pedersen generators)
    /// @param programId The circuit hash
    /// @param publicData Public input values followed by ABI-encoded Pedersen generators
    /// @dev The publicData is split: first N*32 bytes are public inputs, remaining bytes are gensData.
    ///      For now, gensData is passed as empty (PODP generators must be included separately).
    function verify(bytes calldata proofData, bytes32 programId, bytes calldata publicData) external view override {
        // This reverts on invalid proof
        // Pass empty gensData for now — callers needing PODP verification should use verifyOrRevert directly
        remainderVerifier.verifyOrRevert(proofData, programId, publicData, "");
    }

    /// @notice Return the proof system name
    function proofSystem() external pure override returns (string memory) {
        return "remainder";
    }
}
