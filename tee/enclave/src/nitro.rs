//! AWS Nitro Enclave attestation support.
//!
//! When compiled with `--features nitro` and running inside a Nitro enclave,
//! this module calls the NSM device to produce attestation documents.
//! In dev mode (no `nitro` feature), it produces mock attestation documents
//! for testing.

use alloy_primitives::{Address, B256};
use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// Attestation document returned by GET /attestation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttestationDocument {
    /// Base64-encoded CBOR COSE_Sign1 attestation document.
    pub document: String,
    /// Ethereum address of the enclave signer.
    pub enclave_address: String,
    /// Whether this is a real Nitro attestation or a dev-mode mock.
    pub is_nitro: bool,
    /// PCR0 value (hex-encoded, 48 bytes / 96 hex chars).
    pub pcr0: String,
}

/// Nitro attestation provider.
#[derive(Debug)]
pub struct NitroAttestor {
    /// Cached attestation document.
    attestation_doc: Option<AttestationDocument>,
    /// Whether Nitro is enabled.
    nitro_enabled: bool,
}

impl NitroAttestor {
    /// Create a new NitroAttestor.
    ///
    /// If `nitro_enabled` is true and we're inside a Nitro enclave,
    /// this will call NSM to get an attestation document.
    /// Otherwise, produces a mock document.
    ///
    /// The initial attestation is created without a nonce.
    pub fn new(
        nitro_enabled: bool,
        enclave_address: Address,
        model_hash: B256,
    ) -> Result<Self, String> {
        let attestation_doc = if nitro_enabled {
            #[cfg(feature = "nitro")]
            {
                Some(Self::get_nitro_attestation(
                    enclave_address,
                    model_hash,
                    None,
                )?)
            }
            #[cfg(not(feature = "nitro"))]
            {
                let _ = (enclave_address, model_hash);
                return Err(
                    "Nitro enabled but binary not compiled with 'nitro' feature".to_string()
                );
            }
        } else {
            Some(Self::mock_attestation(enclave_address, model_hash, None)?)
        };

        Ok(Self {
            attestation_doc,
            nitro_enabled,
        })
    }

    /// Create a mock-only `NitroAttestor` that cannot fail.
    ///
    /// This is used as a fallback when real Nitro attestation is unavailable.
    /// The underlying CBOR serialization of well-known `BTreeMap` values is
    /// infallible in practice, but we handle the error gracefully regardless.
    pub fn mock_fallback(enclave_address: Address, model_hash: B256) -> Self {
        let attestation_doc = Self::mock_attestation(enclave_address, model_hash, None)
            .unwrap_or_else(|e| {
                tracing::error!("Failed to create mock attestation (unexpected): {}", e);
                // Return a minimal empty attestation document so the server can
                // still start and serve health/info endpoints.
                AttestationDocument {
                    document: String::new(),
                    enclave_address: format!("{}", enclave_address),
                    is_nitro: false,
                    pcr0: "0".repeat(96),
                }
            });
        Self {
            attestation_doc: Some(attestation_doc),
            nitro_enabled: false,
        }
    }

    /// Get the cached attestation document.
    pub fn attestation_document(&self) -> Option<&AttestationDocument> {
        self.attestation_doc.as_ref()
    }

    /// Whether this attestor uses real Nitro attestation.
    pub fn is_nitro(&self) -> bool {
        self.nitro_enabled
    }

    /// Refresh the cached attestation document.
    ///
    /// If Nitro is enabled, calls NSM to get a fresh attestation document.
    /// Otherwise, generates a new mock document.
    /// An optional nonce can be provided to bind the attestation to a specific challenge.
    pub fn refresh(
        &mut self,
        enclave_address: Address,
        model_hash: B256,
        nonce: Option<&[u8]>,
    ) -> Result<(), String> {
        if self.nitro_enabled {
            #[cfg(feature = "nitro")]
            {
                self.attestation_doc = Some(Self::get_nitro_attestation(
                    enclave_address,
                    model_hash,
                    nonce,
                )?);
                return Ok(());
            }
            #[cfg(not(feature = "nitro"))]
            {
                let _ = (enclave_address, model_hash, nonce);
                return Err(
                    "Nitro enabled but binary not compiled with 'nitro' feature".to_string()
                );
            }
        }
        self.attestation_doc = Some(Self::mock_attestation(enclave_address, model_hash, nonce)?);
        Ok(())
    }

    /// Create a mock attestation document for dev/testing.
    ///
    /// The CBOR serialization here uses deterministic, well-known structures
    /// (BTreeMaps of simple CBOR values), so in practice these serializations
    /// cannot fail. We return `Result` nonetheless to avoid `expect`/`unwrap`
    /// in production code paths.
    fn mock_attestation(
        enclave_address: Address,
        model_hash: B256,
        nonce: Option<&[u8]>,
    ) -> Result<AttestationDocument, String> {
        // Build a mock CBOR structure that mimics Nitro attestation format.
        // This is NOT cryptographically valid -- for dev/testing only.
        use std::collections::BTreeMap;

        let mut payload: BTreeMap<String, serde_cbor::Value> = BTreeMap::new();

        // module_id
        payload.insert(
            "module_id".to_string(),
            serde_cbor::Value::Text("dev-mock-enclave".to_string()),
        );

        // timestamp (milliseconds since epoch)
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or(std::time::Duration::ZERO)
            .as_millis() as i128;
        payload.insert(
            "timestamp".to_string(),
            serde_cbor::Value::Integer(timestamp),
        );

        // digest
        payload.insert(
            "digest".to_string(),
            serde_cbor::Value::Text("SHA384".to_string()),
        );

        // PCRs -- mock with zeros for PCR0
        let mock_pcr0 = vec![0u8; 48]; // 48 bytes for SHA-384
        let mut pcrs: BTreeMap<serde_cbor::Value, serde_cbor::Value> = BTreeMap::new();
        pcrs.insert(
            serde_cbor::Value::Integer(0),
            serde_cbor::Value::Bytes(mock_pcr0.clone()),
        );
        pcrs.insert(
            serde_cbor::Value::Integer(1),
            serde_cbor::Value::Bytes(vec![0u8; 48]),
        );
        pcrs.insert(
            serde_cbor::Value::Integer(2),
            serde_cbor::Value::Bytes(vec![0u8; 48]),
        );
        payload.insert("pcrs".to_string(), serde_cbor::Value::Map(pcrs));

        // public_key -- the enclave's Ethereum address (20 bytes)
        payload.insert(
            "public_key".to_string(),
            serde_cbor::Value::Bytes(enclave_address.as_slice().to_vec()),
        );

        // user_data -- keccak256(model_hash) to bind attestation to model
        let user_data = alloy_primitives::keccak256(model_hash.as_slice());
        payload.insert(
            "user_data".to_string(),
            serde_cbor::Value::Bytes(user_data.as_slice().to_vec()),
        );

        // nonce
        payload.insert(
            "nonce".to_string(),
            match nonce {
                Some(n) => serde_cbor::Value::Bytes(n.to_vec()),
                None => serde_cbor::Value::Null,
            },
        );

        // Encode payload as CBOR.
        let payload_bytes = serde_cbor::to_vec(&payload)
            .map_err(|e| format!("Failed to CBOR-encode mock attestation payload: {}", e))?;

        // Wrap in a mock COSE_Sign1 structure: [protected, unprotected, payload, signature]
        let cose_sign1 = serde_cbor::Value::Array(vec![
            serde_cbor::Value::Bytes(vec![]), // protected header (empty for mock)
            serde_cbor::Value::Map(BTreeMap::new()), // unprotected header
            serde_cbor::Value::Bytes(payload_bytes), // payload
            serde_cbor::Value::Bytes(vec![0u8; 96]), // signature (mock P-384 sig)
        ]);

        // CBOR-encode the COSE_Sign1.
        let doc_bytes = serde_cbor::to_vec(&cose_sign1)
            .map_err(|e| format!("Failed to CBOR-encode mock COSE_Sign1: {}", e))?;
        let doc_base64 = base64::engine::general_purpose::STANDARD.encode(&doc_bytes);

        Ok(AttestationDocument {
            document: doc_base64,
            enclave_address: format!("{}", enclave_address),
            is_nitro: false,
            pcr0: hex::encode(&mock_pcr0),
        })
    }

    /// Get real Nitro attestation from NSM device.
    /// Only available when compiled with `nitro` feature.
    #[cfg(feature = "nitro")]
    fn get_nitro_attestation(
        enclave_address: Address,
        model_hash: B256,
        nonce: Option<&[u8]>,
    ) -> Result<AttestationDocument, String> {
        use aws_nitro_enclaves_nsm_api::api::{Request, Response};
        use aws_nitro_enclaves_nsm_api::driver;

        // 1. Open NSM device
        let nsm_fd = driver::nsm_init();
        if nsm_fd < 0 {
            return Err("Failed to open NSM device -- not in a Nitro enclave?".to_string());
        }

        // 2. Build attestation request
        // user_data = keccak256(model_hash) to bind attestation to the model
        let user_data = alloy_primitives::keccak256(model_hash.as_slice());
        let request = Request::Attestation {
            public_key: Some(enclave_address.as_slice().to_vec()),
            user_data: Some(user_data.as_slice().to_vec()),
            nonce: nonce.map(|n| n.to_vec()),
        };

        // 3. Send request to NSM
        let response = driver::nsm_process_request(nsm_fd, request);
        driver::nsm_exit(nsm_fd);

        // 4. Extract attestation document
        match response {
            Response::Attestation { document } => {
                let doc_base64 = base64::engine::general_purpose::STANDARD.encode(&document);

                // Parse to extract PCR0
                let cose: serde_cbor::Value = serde_cbor::from_slice(&document)
                    .map_err(|e| format!("Failed to parse attestation COSE envelope: {}", e))?;
                let pcr0 = Self::extract_pcr0_from_cose(&cose)?;

                Ok(AttestationDocument {
                    document: doc_base64,
                    enclave_address: format!("{}", enclave_address),
                    is_nitro: true,
                    pcr0,
                })
            }
            Response::Error(e) => Err(format!("NSM attestation error: {:?}", e)),
            _ => Err("Unexpected NSM response type".to_string()),
        }
    }

    /// Extract PCR0 from a COSE_Sign1 attestation document.
    ///
    /// The structure is: COSE_Sign1 = [protected, unprotected, payload, signature]
    /// where payload is CBOR-encoded map with "pcrs" key containing PCR values.
    #[cfg(feature = "nitro")]
    fn extract_pcr0_from_cose(cose: &serde_cbor::Value) -> Result<String, String> {
        // COSE_Sign1 is a 4-element array
        let items = match cose {
            serde_cbor::Value::Array(items) if items.len() == 4 => items,
            _ => return Err("COSE_Sign1: expected 4-element array".to_string()),
        };

        // Payload is item[2], which is CBOR-encoded bytes
        let payload_bytes = match &items[2] {
            serde_cbor::Value::Bytes(b) => b,
            _ => return Err("COSE_Sign1: expected bytes for payload".to_string()),
        };

        // Parse the payload CBOR
        let payload: std::collections::BTreeMap<String, serde_cbor::Value> =
            serde_cbor::from_slice(payload_bytes)
                .map_err(|e| format!("Failed to parse attestation payload: {}", e))?;

        // Get "pcrs" map
        let pcrs = match payload.get("pcrs") {
            Some(serde_cbor::Value::Map(m)) => m,
            _ => return Err("Attestation payload missing 'pcrs' map".to_string()),
        };

        // Get PCR0 (key = integer 0)
        let pcr0_key = serde_cbor::Value::Integer(0);
        let pcr0_bytes = match pcrs.get(&pcr0_key) {
            Some(serde_cbor::Value::Bytes(b)) => b,
            _ => return Err("PCR0 not found or not bytes in attestation".to_string()),
        };

        Ok(hex::encode(pcr0_bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;

    #[test]
    fn test_mock_attestation() {
        let addr = address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
        let model_hash = B256::ZERO;

        let attestor = NitroAttestor::new(false, addr, model_hash).unwrap();
        let doc = attestor.attestation_document().unwrap();

        assert!(!doc.is_nitro);
        assert_eq!(doc.enclave_address, format!("{}", addr));
        assert_eq!(doc.pcr0.len(), 96); // 48 bytes = 96 hex chars

        // Verify we can decode the base64 document
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&doc.document)
            .unwrap();
        assert!(!decoded.is_empty());
    }

    #[test]
    fn test_mock_attestation_cbor_structure() {
        let addr = address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
        let model_hash = alloy_primitives::keccak256(b"test model");

        let attestor = NitroAttestor::new(false, addr, model_hash).unwrap();
        let doc = attestor.attestation_document().unwrap();

        // Decode base64 -> CBOR -> verify COSE_Sign1 structure
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&doc.document)
            .unwrap();
        let cose: serde_cbor::Value = serde_cbor::from_slice(&decoded).unwrap();

        // COSE_Sign1 is a 4-element array
        if let serde_cbor::Value::Array(items) = cose {
            assert_eq!(items.len(), 4);

            // Parse payload (item 2)
            if let serde_cbor::Value::Bytes(payload_bytes) = &items[2] {
                let payload: std::collections::BTreeMap<String, serde_cbor::Value> =
                    serde_cbor::from_slice(payload_bytes).unwrap();

                assert!(payload.contains_key("module_id"));
                assert!(payload.contains_key("pcrs"));
                assert!(payload.contains_key("public_key"));
                assert!(payload.contains_key("user_data"));
                assert!(payload.contains_key("timestamp"));
            } else {
                panic!("Expected bytes for payload");
            }
        } else {
            panic!("Expected array for COSE_Sign1");
        }
    }

    #[test]
    fn test_nitro_enabled_without_feature_fails() {
        let addr = address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
        let model_hash = B256::ZERO;

        // With nitro_enabled=true but no nitro feature, should fail
        #[cfg(not(feature = "nitro"))]
        {
            let result = NitroAttestor::new(true, addr, model_hash);
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .contains("not compiled with 'nitro' feature"));
        }
    }

    #[test]
    fn test_refresh_mock_attestation() {
        let addr = address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
        let model_hash = B256::ZERO;

        let mut attestor = NitroAttestor::new(false, addr, model_hash).unwrap();
        let doc_before = attestor.attestation_document().unwrap().clone();

        // Refresh without nonce
        attestor.refresh(addr, model_hash, None).unwrap();
        let doc_after = attestor.attestation_document().unwrap();

        // Both should be mock attestations
        assert!(!doc_after.is_nitro);
        assert_eq!(doc_after.enclave_address, format!("{}", addr));
        assert_eq!(doc_after.pcr0.len(), 96);

        // Documents should be different (at least the timestamp differs)
        // but both are valid attestation documents
        assert!(!doc_before.document.is_empty());
        assert!(!doc_after.document.is_empty());
    }

    #[test]
    fn test_refresh_with_nonce() {
        let addr = address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
        let model_hash = B256::ZERO;
        let nonce = b"test-nonce-12345";

        let mut attestor = NitroAttestor::new(false, addr, model_hash).unwrap();

        // Refresh with a nonce
        attestor.refresh(addr, model_hash, Some(nonce)).unwrap();
        let doc = attestor.attestation_document().unwrap();

        assert!(!doc.is_nitro);

        // Verify the nonce is embedded in the CBOR payload
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&doc.document)
            .unwrap();
        let cose: serde_cbor::Value = serde_cbor::from_slice(&decoded).unwrap();

        if let serde_cbor::Value::Array(items) = cose {
            if let serde_cbor::Value::Bytes(payload_bytes) = &items[2] {
                let payload: std::collections::BTreeMap<String, serde_cbor::Value> =
                    serde_cbor::from_slice(payload_bytes).unwrap();

                // nonce field should be the bytes we provided
                match payload.get("nonce") {
                    Some(serde_cbor::Value::Bytes(n)) => {
                        assert_eq!(n, nonce);
                    }
                    other => panic!("Expected nonce bytes, got {:?}", other),
                }
            } else {
                panic!("Expected bytes for payload");
            }
        } else {
            panic!("Expected array for COSE_Sign1");
        }
    }

    #[test]
    fn test_mock_attestation_without_nonce_has_null() {
        let addr = address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
        let model_hash = B256::ZERO;

        let attestor = NitroAttestor::new(false, addr, model_hash).unwrap();
        let doc = attestor.attestation_document().unwrap();

        // Verify the nonce is null when not provided
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&doc.document)
            .unwrap();
        let cose: serde_cbor::Value = serde_cbor::from_slice(&decoded).unwrap();

        if let serde_cbor::Value::Array(items) = cose {
            if let serde_cbor::Value::Bytes(payload_bytes) = &items[2] {
                let payload: std::collections::BTreeMap<String, serde_cbor::Value> =
                    serde_cbor::from_slice(payload_bytes).unwrap();

                match payload.get("nonce") {
                    Some(serde_cbor::Value::Null) => {} // expected
                    other => panic!("Expected nonce to be Null, got {:?}", other),
                }
            } else {
                panic!("Expected bytes for payload");
            }
        } else {
            panic!("Expected array for COSE_Sign1");
        }
    }

    #[test]
    fn test_refresh_nitro_enabled_without_feature_fails() {
        let addr = address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
        let model_hash = B256::ZERO;

        // Start with mock (nitro_enabled=false)
        let mut attestor = NitroAttestor::new(false, addr, model_hash).unwrap();

        // Manually flip nitro_enabled to simulate a misconfiguration
        attestor.nitro_enabled = true;

        #[cfg(not(feature = "nitro"))]
        {
            let result = attestor.refresh(addr, model_hash, None);
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .contains("not compiled with 'nitro' feature"));
        }
    }
}
