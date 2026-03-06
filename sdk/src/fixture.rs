use alloy::primitives::{B256, U256};
use alloy::sol_types::SolType;
use serde::Deserialize;
use std::path::Path;

use crate::abi::RemainderVerifier::DAGCircuitDescription;

/// Parsed proof data ready for contract calls.
pub struct ProofData {
    pub proof_bytes: Vec<u8>,
    pub circuit_hash: B256,
    pub public_inputs: Vec<u8>,
    pub gens_data: Vec<u8>,
}

/// Top-level fixture loaded from `phase1a_dag_fixture.json`.
#[derive(Debug, Deserialize)]
pub struct DAGFixture {
    pub circuit_hash_raw: String,
    pub proof_hex: String,
    pub gens_hex: String,
    pub public_inputs_hex: String,
    pub dag_circuit_description: DAGCircuitDescriptionJson,
}

/// JSON representation of the DAG circuit description.
/// Field names match the Rust fixture generator output (camelCase).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DAGCircuitDescriptionJson {
    pub num_compute_layers: u64,
    pub num_input_layers: u64,
    pub layer_types: Vec<u8>,
    pub num_sumcheck_rounds: Vec<u64>,
    pub atom_offsets: Vec<u64>,
    pub atom_target_layers: Vec<u64>,
    pub atom_commit_idxs: Vec<u64>,
    pub pt_offsets: Vec<u64>,
    pub pt_data: Vec<u64>,
    pub input_is_committed: Vec<bool>,
    pub oracle_product_offsets: Vec<u64>,
    pub oracle_result_idxs: Vec<u64>,
    /// Hex strings (0x-prefixed) representing Fr field elements.
    pub oracle_expr_coeffs: Vec<String>,
    // Metadata fields (not part of Solidity struct)
    #[serde(default)]
    pub incoming_offsets: Vec<u64>,
    #[serde(default)]
    pub incoming_atom_idx: Vec<u64>,
}

impl DAGFixture {
    /// Load a fixture from a JSON file path.
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let data = std::fs::read_to_string(path)?;
        let fixture: DAGFixture = serde_json::from_str(&data)?;
        Ok(fixture)
    }

    /// Convert to proof data (hex-decoded bytes).
    pub fn to_proof_data(&self) -> anyhow::Result<ProofData> {
        let proof_bytes = hex::decode(self.proof_hex.trim_start_matches("0x"))?;
        let circuit_hash: B256 = self.circuit_hash_raw.parse()?;
        let public_inputs = hex::decode(self.public_inputs_hex.trim_start_matches("0x"))?;
        let gens_data = hex::decode(self.gens_hex.trim_start_matches("0x"))?;
        Ok(ProofData {
            proof_bytes,
            circuit_hash,
            public_inputs,
            gens_data,
        })
    }

    /// Convert the circuit description to the ABI struct.
    pub fn to_dag_description(&self) -> anyhow::Result<DAGCircuitDescription> {
        self.dag_circuit_description.to_abi()
    }

    /// ABI-encode the circuit description (for `registerDAGCircuit`'s `descData` parameter).
    pub fn encode_dag_description(&self) -> anyhow::Result<Vec<u8>> {
        let desc = self.to_dag_description()?;
        Ok(DAGCircuitDescription::abi_encode(&desc))
    }
}

impl DAGCircuitDescriptionJson {
    /// Convert to the alloy sol! ABI struct.
    pub fn to_abi(&self) -> anyhow::Result<DAGCircuitDescription> {
        let oracle_expr_coeffs: Result<Vec<U256>, _> = self
            .oracle_expr_coeffs
            .iter()
            .map(|s| s.parse::<U256>())
            .collect();

        Ok(DAGCircuitDescription {
            numComputeLayers: U256::from(self.num_compute_layers),
            numInputLayers: U256::from(self.num_input_layers),
            layerTypes: self.layer_types.clone(),
            numSumcheckRounds: self
                .num_sumcheck_rounds
                .iter()
                .map(|&v| U256::from(v))
                .collect(),
            atomOffsets: self.atom_offsets.iter().map(|&v| U256::from(v)).collect(),
            atomTargetLayers: self
                .atom_target_layers
                .iter()
                .map(|&v| U256::from(v))
                .collect(),
            atomCommitIdxs: self
                .atom_commit_idxs
                .iter()
                .map(|&v| U256::from(v))
                .collect(),
            ptOffsets: self.pt_offsets.iter().map(|&v| U256::from(v)).collect(),
            ptData: self.pt_data.iter().map(|&v| U256::from(v)).collect(),
            inputIsCommitted: self.input_is_committed.clone(),
            oracleProductOffsets: self
                .oracle_product_offsets
                .iter()
                .map(|&v| U256::from(v))
                .collect(),
            oracleResultIdxs: self
                .oracle_result_idxs
                .iter()
                .map(|&v| U256::from(v))
                .collect(),
            oracleExprCoeffs: oracle_expr_coeffs?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../contracts/test/fixtures/phase1a_dag_fixture.json")
    }

    #[test]
    fn test_load_fixture() {
        let path = fixture_path();
        if !path.exists() {
            eprintln!("Skipping: fixture not found at {:?}", path);
            return;
        }
        let fixture = DAGFixture::load(&path).unwrap();
        assert_eq!(fixture.dag_circuit_description.num_compute_layers, 88);
        assert_eq!(fixture.dag_circuit_description.num_input_layers, 2);
        assert_eq!(fixture.dag_circuit_description.layer_types.len(), 88);
        assert_eq!(
            fixture.dag_circuit_description.oracle_expr_coeffs.len(),
            134
        );
    }

    #[test]
    fn test_to_proof_data() {
        let path = fixture_path();
        if !path.exists() {
            eprintln!("Skipping: fixture not found at {:?}", path);
            return;
        }
        let fixture = DAGFixture::load(&path).unwrap();
        let proof = fixture.to_proof_data().unwrap();
        assert!(!proof.proof_bytes.is_empty());
        assert!(!proof.circuit_hash.is_zero());
        assert!(!proof.public_inputs.is_empty());
        assert!(!proof.gens_data.is_empty());
    }

    #[test]
    fn test_to_dag_description_abi_roundtrip() {
        let path = fixture_path();
        if !path.exists() {
            eprintln!("Skipping: fixture not found at {:?}", path);
            return;
        }
        let fixture = DAGFixture::load(&path).unwrap();
        let desc = fixture.to_dag_description().unwrap();
        assert_eq!(desc.numComputeLayers, U256::from(88));
        assert_eq!(desc.numInputLayers, U256::from(2));

        // ABI encode + decode roundtrip
        let encoded = DAGCircuitDescription::abi_encode(&desc);
        let decoded = DAGCircuitDescription::abi_decode(&encoded).unwrap();
        assert_eq!(decoded.numComputeLayers, desc.numComputeLayers);
        assert_eq!(decoded.layerTypes.len(), desc.layerTypes.len());
        assert_eq!(decoded.oracleExprCoeffs.len(), desc.oracleExprCoeffs.len());
    }
}
