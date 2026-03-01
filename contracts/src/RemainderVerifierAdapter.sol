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
    /// @param proofData ABI-encoded GKR+Hyrax proof (starts with "REM1" selector)
    /// @param programId The circuit hash
    /// @param publicData Length-prefixed payload: [pubInputsLen (32 bytes)] [pubInputs...] [gensData...]
    ///        If publicData.length < 32, it is treated as raw public inputs with no generators.
    function verify(bytes calldata proofData, bytes32 programId, bytes calldata publicData) external view override {
        if (publicData.length < 32) {
            // Legacy/simple path: entire publicData is public inputs, no generators
            remainderVerifier.verifyOrRevert(proofData, programId, publicData, "");
        } else {
            // Split publicData: first 32 bytes = pubInputs byte length
            uint256 pubInputsLen = uint256(bytes32(publicData[:32]));

            if (pubInputsLen + 32 > publicData.length) {
                // Invalid length prefix — treat as raw public inputs
                remainderVerifier.verifyOrRevert(proofData, programId, publicData, "");
            } else {
                bytes calldata pubInputs = publicData[32:32 + pubInputsLen];
                bytes calldata gensData = publicData[32 + pubInputsLen:];
                remainderVerifier.verifyOrRevert(proofData, programId, pubInputs, gensData);
            }
        }
    }

    /// @notice Return the proof system name
    function proofSystem() external pure override returns (string memory) {
        return "remainder";
    }
}
