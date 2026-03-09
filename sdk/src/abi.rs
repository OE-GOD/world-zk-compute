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

sol! {
    #[sol(rpc)]
    contract TEEMLVerifier {
        struct EnclaveInfo {
            bool registered;
            bool active;
            bytes32 enclaveImageHash;
            uint256 registeredAt;
        }

        struct MLResult {
            address enclave;
            address submitter;
            bytes32 modelHash;
            bytes32 inputHash;
            bytes32 resultHash;
            bytes result;
            uint256 submittedAt;
            uint256 challengeDeadline;
            uint256 disputeDeadline;
            uint256 challengeBond;
            uint256 proverStakeAmount;
            bool finalized;
            bool challenged;
            address challenger;
        }

        event EnclaveRegistered(address indexed enclaveKey, bytes32 enclaveImageHash);
        event EnclaveRevoked(address indexed enclaveKey);
        event ResultSubmitted(bytes32 indexed resultId, bytes32 modelHash, bytes32 inputHash);
        event ResultChallenged(bytes32 indexed resultId, address challenger);
        event DisputeResolved(bytes32 indexed resultId, bool proverWon);
        event ResultFinalized(bytes32 indexed resultId);

        function registerEnclave(address enclaveKey, bytes32 enclaveImageHash) external;
        function revokeEnclave(address enclaveKey) external;
        function setRemainderVerifier(address _verifier) external;
        function setChallengeBondAmount(uint256 _amount) external;
        function setProverStake(uint256 _amount) external;

        function submitResult(bytes32 modelHash, bytes32 inputHash, bytes calldata result, bytes calldata attestation) external payable returns (bytes32 resultId);
        function challenge(bytes32 resultId) external payable;
        function resolveDispute(bytes32 resultId, bytes calldata proof, bytes32 circuitHash, bytes calldata publicInputs, bytes calldata gensData) external;
        function resolveDisputeByTimeout(bytes32 resultId) external;
        function finalize(bytes32 resultId) external;

        function getResult(bytes32 resultId) external view returns (MLResult memory);
        function isResultValid(bytes32 resultId) external view returns (bool);

        function admin() external view returns (address);
        function remainderVerifier() external view returns (address);
        function challengeBondAmount() external view returns (uint256);
        function proverStake() external view returns (uint256);
        function disputeResolved(bytes32 resultId) external view returns (bool);
        function disputeProverWon(bytes32 resultId) external view returns (bool);
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
