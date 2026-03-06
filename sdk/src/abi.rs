use alloy::sol;

sol! {
    #[sol(rpc)]
    contract RemainderVerifier {
        // --- Structs ---

        struct DAGCircuitDescription {
            uint256 numComputeLayers;
            uint256 numInputLayers;
            uint8[] layerTypes;
            uint256[] numSumcheckRounds;
            uint256[] atomOffsets;
            uint256[] atomTargetLayers;
            uint256[] atomCommitIdxs;
            uint256[] ptOffsets;
            uint256[] ptData;
            bool[] inputIsCommitted;
            uint256[] oracleProductOffsets;
            uint256[] oracleResultIdxs;
            uint256[] oracleExprCoeffs;
        }

        // --- Errors ---

        error NotAdmin();
        error CircuitNotRegistered();
        error CircuitNotActive();
        error InvalidProofSelector();
        error InvalidProofLength();
        error ProofVerificationFailed();
        error InvalidGenerators();

        // --- Events ---

        event CircuitRegistered(bytes32 indexed circuitHash, string name);
        event CircuitDeactivated(bytes32 indexed circuitHash);
        event ProofVerified(bytes32 indexed circuitHash, bool valid);
        event AdminTransferred(address indexed oldAdmin, address indexed newAdmin);
        event DAGBatchSessionStarted(bytes32 indexed sessionId, bytes32 indexed circuitHash, uint256 totalBatches);
        event DAGBatchCompleted(bytes32 indexed sessionId, uint256 batchIdx);
        event DAGBatchFinalized(bytes32 indexed sessionId, bytes32 indexed circuitHash);

        // --- Admin Functions ---

        function registerDAGCircuit(
            bytes32 circuitHash,
            bytes calldata descData,
            string calldata name,
            bytes32 gensHash
        ) external;

        function setDAGStylusVerifier(
            bytes32 circuitHash,
            address stylusVerifier
        ) external;

        function setDAGCircuitGroth16Verifier(
            bytes32 circuitHash,
            address verifier,
            uint256 groth16InputCount
        ) external;

        // --- Single-TX Verification ---

        function verifyDAGProof(
            bytes calldata proof,
            bytes32 circuitHash,
            bytes calldata publicInputs,
            bytes calldata gensData
        ) external view returns (bool valid);

        function verifyDAGProofStylus(
            bytes calldata proof,
            bytes32 circuitHash,
            bytes calldata publicInputs,
            bytes calldata gensData
        ) external view returns (bool valid);

        function verifyDAGWithGroth16(
            bytes calldata innerProof,
            bytes32 circuitHash,
            bytes calldata publicInputs,
            bytes calldata gensData,
            uint256[8] calldata groth16Proof,
            uint256[] calldata groth16Outputs
        ) external view;

        // --- Batch Verification ---

        function startDAGBatchVerify(
            bytes calldata proof,
            bytes32 circuitHash,
            bytes calldata publicInputs,
            bytes calldata gensData
        ) external returns (bytes32 sessionId);

        function continueDAGBatchVerify(
            bytes32 sessionId,
            bytes calldata proof,
            bytes calldata publicInputs,
            bytes calldata gensData
        ) external;

        function finalizeDAGBatchVerify(
            bytes32 sessionId,
            bytes calldata proof,
            bytes calldata publicInputs,
            bytes calldata gensData
        ) external returns (bool finalized);

        function cleanupDAGBatchSession(bytes32 sessionId) external;

        function getDAGBatchSession(bytes32 sessionId) external view returns (
            bytes32 circuitHash,
            uint256 nextBatchIdx,
            uint256 totalBatches,
            bool finalized,
            uint256 finalizeInputIdx,
            uint256 finalizeGroupsDone
        );

        // --- Query ---

        function isDAGCircuitActive(bytes32 circuitHash) external view returns (bool);
    }
}

#[cfg(test)]
mod tests {
    use super::RemainderVerifier::DAGCircuitDescription;
    use alloy::primitives::U256;
    use alloy::sol_types::SolType;

    #[test]
    fn test_dag_description_abi_encode_decode() {
        let desc = DAGCircuitDescription {
            numComputeLayers: U256::from(2),
            numInputLayers: U256::from(1),
            layerTypes: vec![0, 1],
            numSumcheckRounds: vec![U256::from(3), U256::from(4)],
            atomOffsets: vec![U256::from(0), U256::from(1), U256::from(2)],
            atomTargetLayers: vec![U256::from(1), U256::from(2)],
            atomCommitIdxs: vec![U256::from(0), U256::from(0)],
            ptOffsets: vec![U256::from(0), U256::from(1), U256::from(2)],
            ptData: vec![U256::from(0), U256::from(1)],
            inputIsCommitted: vec![true],
            oracleProductOffsets: vec![U256::from(0), U256::from(1), U256::from(2)],
            oracleResultIdxs: vec![U256::from(0), U256::from(0)],
            oracleExprCoeffs: vec![U256::from(1), U256::from(1)],
        };

        let encoded = DAGCircuitDescription::abi_encode(&desc);
        assert!(!encoded.is_empty());

        let decoded = DAGCircuitDescription::abi_decode(&encoded).unwrap();
        assert_eq!(decoded.numComputeLayers, U256::from(2));
        assert_eq!(decoded.numInputLayers, U256::from(1));
        assert_eq!(decoded.layerTypes.len(), 2);
    }
}
