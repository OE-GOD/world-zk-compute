//! AWS Nitro Enclave attestation document verification.
//!
//! Nitro attestation documents are CBOR-encoded COSE_Sign1 structures.
//! The payload contains PCR values, a public key, user data, and a
//! certificate chain signed by AWS's Nitro root CA.
//!
//! References:
//! - https://docs.aws.amazon.com/enclaves/latest/user/verify-root.html
//! - COSE_Sign1: RFC 8152

use der::{Decode, Encode};
use p384::ecdsa::{signature::Verifier, Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use x509_cert::Certificate;

/// Parsed and verified attestation document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifiedAttestation {
    /// PCR0 -- enclave image hash (48 bytes, hex-encoded)
    pub pcr0: String,
    /// PCR1 -- Linux kernel hash
    pub pcr1: String,
    /// PCR2 -- application hash
    pub pcr2: String,
    /// Enclave's Ethereum address (from public_key field)
    pub enclave_address: String,
    /// User data (hex-encoded) -- typically keccak256(model_hash)
    pub user_data: String,
    /// Module ID from attestation
    pub module_id: String,
    /// Timestamp (ms since epoch)
    pub timestamp: u64,
    /// Whether the certificate chain was verified against AWS root CA
    pub cert_chain_verified: bool,
    /// Nonce from attestation (hex-encoded, if present)
    pub nonce: Option<String>,
}

/// Errors during attestation verification.
#[derive(Debug, thiserror::Error)]
pub enum AttestationError {
    #[error("Base64 decode error: {0}")]
    Base64(String),
    #[error("CBOR decode error: {0}")]
    Cbor(String),
    #[error("Invalid COSE_Sign1 structure: {0}")]
    CoseStructure(String),
    #[error("Missing required field: {0}")]
    MissingField(String),
    #[error("Certificate chain verification failed: {0}")]
    CertChain(String),
    #[error("Attestation expired or too old")]
    Expired,
    #[error("PCR validation failed: {0}")]
    PcrMismatch(String),
    #[error("Nonce validation failed: {0}")]
    NonceMismatch(String),
}

/// AWS Nitro Enclaves Root CA certificate (PEM).
/// Downloaded from: https://aws-nitro-enclaves.amazonaws.com/AWS_NitroEnclaves_Root-G1.zip
const AWS_NITRO_ROOT_CA_PEM: &str = "-----BEGIN CERTIFICATE-----
MIICETCCAZagAwIBAgIRAPkxdWgbkK/hHUbMtOTn+FYwCgYIKoZIzj0EAwMwSTEL
MAkGA1UEBhMCVVMxDzANBgNVBAoMBkFtYXpvbjEMMAoGA1UECwwDQVdTMRswGQYD
VQQDDBJhd3Mubml0cm8tZW5jbGF2ZXMwHhcNMTkxMDI4MTMyODA1WhcNNDkxMDI4
MTQyODA1WjBJMQswCQYDVQQGEwJVUzEPMA0GA1UECgwGQW1hem9uMQwwCgYDVQQL
DANBV1MxGzAZBgNVBAMMEmF3cy5uaXRyby1lbmNsYXZlczB2MBAGByqGSM49AgEG
BSuBBAAiA2IABPwCVOumCMHzaHDimtqQvkY4MpJzbolL//Zy2YlES1BR5TSksfbb
48C8WBoyt7F2Bw7eEtaaP+ohG2bnUs990d0JX28TcPQXCEPZ3BABIeTPYwEoCWZE
h8l5YoQwTcU/9KNCMEAwDwYDVR0TAQH/BAUwAwEB/zAdBgNVHQ4EFgQUkCW1DdkF
R+eWw5b6cp3PmanfS5YwDgYDVR0PAQH/BAQDAgGGMAoGCCqGSM49BAMDA2kAMGYC
MQCjfy+Rocm9Xue4YnwWmNJVA44fA0P5W2OpYow9OYCVRaEevL8uO1XYru5xtMPW
rfMCMQCi85sWBbJwKKXdS6BptQFuZbT73o/gBh1qUxl/nNr12UO8Yfwr6wPLb+6N
IwLz3/Y=
-----END CERTIFICATE-----";

/// Extract the certificate bundle from a parsed attestation payload map.
fn extract_cabundle(
    payload_map: &[(ciborium::Value, ciborium::Value)],
) -> Result<Vec<Vec<u8>>, AttestationError> {
    for (k, v) in payload_map {
        if let ciborium::Value::Text(key) = k {
            if key == "cabundle" {
                if let ciborium::Value::Array(certs) = v {
                    let mut bundle = Vec::new();
                    for cert in certs {
                        if let ciborium::Value::Bytes(der_bytes) = cert {
                            bundle.push(der_bytes.clone());
                        } else {
                            return Err(AttestationError::CertChain(
                                "cabundle entry is not bytes".into(),
                            ));
                        }
                    }
                    return Ok(bundle);
                }
            }
        }
    }
    Err(AttestationError::MissingField("cabundle".into()))
}

/// Decode the embedded AWS Nitro root CA PEM to DER bytes.
fn parse_root_ca_der() -> Result<Vec<u8>, AttestationError> {
    let pem_body: String = AWS_NITRO_ROOT_CA_PEM
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with("-----"))
        .collect();
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(&pem_body)
        .map_err(|e| AttestationError::CertChain(format!("Root CA PEM decode: {}", e)))
}

/// Verify the root certificate from the cabundle matches the embedded AWS Nitro root CA.
fn verify_root_cert(root_cert: &Certificate) -> Result<(), AttestationError> {
    let root_ca_der = parse_root_ca_der()?;
    let expected_root = Certificate::from_der(&root_ca_der)
        .map_err(|e| AttestationError::CertChain(format!("Failed to parse root CA cert: {}", e)))?;

    // Compare subject public key info (the authoritative identity of the root CA)
    if root_cert.tbs_certificate.subject_public_key_info
        != expected_root.tbs_certificate.subject_public_key_info
    {
        return Err(AttestationError::CertChain(
            "Root cert public key mismatch with AWS Nitro root CA".into(),
        ));
    }

    Ok(())
}

/// Verify that `subject` certificate was signed by `issuer` using P-384 ECDSA with SHA-384.
fn verify_cert_signature(
    issuer: &Certificate,
    subject: &Certificate,
) -> Result<(), AttestationError> {
    // Extract issuer's P-384 public key from the SubjectPublicKeyInfo.
    // The subject_public_key is a BitString; raw_bytes() gives the SEC1-encoded point.
    let pub_key_bytes = issuer
        .tbs_certificate
        .subject_public_key_info
        .subject_public_key
        .raw_bytes();
    let verifying_key = VerifyingKey::from_sec1_bytes(pub_key_bytes)
        .map_err(|e| AttestationError::CertChain(format!("Invalid issuer P-384 key: {}", e)))?;

    // DER-encode the subject's TBS certificate -- this is the data that was signed.
    let tbs_der = subject
        .tbs_certificate
        .to_der()
        .map_err(|e| AttestationError::CertChain(format!("Failed to encode TBS: {}", e)))?;

    // Extract the DER-encoded signature from the subject certificate.
    // X.509 signatures are ASN.1 DER-encoded ECDSA-Sig-Value (SEQUENCE { r INTEGER, s INTEGER }).
    let sig_bytes = subject.signature.raw_bytes();
    let der_sig = p384::ecdsa::DerSignature::from_bytes(sig_bytes)
        .map_err(|e| AttestationError::CertChain(format!("Invalid cert DER signature: {}", e)))?;

    // Verifier::verify() on VerifyingKey<NistP384> hashes msg with SHA-384 then verifies.
    // X.509 certs using ecdsa-with-SHA384 sign over SHA-384(TBS DER), which is exactly
    // what this Verifier impl does. VerifyingKey implements Verifier<DerSignature> directly.
    verifying_key.verify(&tbs_der, &der_sig).map_err(|e| {
        AttestationError::CertChain(format!("Cert signature verification failed: {}", e))
    })?;

    Ok(())
}

/// Verify the COSE_Sign1 signature using the leaf certificate's public key.
///
/// The COSE Sig_structure is: `["Signature1", protected_header, b"", payload]`
/// CBOR-encoded, then signed with ECDSA P-384 (raw r||s, 96 bytes).
fn verify_cose_signature(
    leaf_cert: &Certificate,
    protected_header: &[u8],
    payload_bytes: &[u8],
    signature_bytes: &[u8],
) -> Result<(), AttestationError> {
    // Extract leaf cert's P-384 public key
    let pub_key_bytes = leaf_cert
        .tbs_certificate
        .subject_public_key_info
        .subject_public_key
        .raw_bytes();
    let verifying_key = VerifyingKey::from_sec1_bytes(pub_key_bytes)
        .map_err(|e| AttestationError::CertChain(format!("Invalid leaf P-384 key: {}", e)))?;

    // Construct COSE Sig_structure: ["Signature1", protected, b"", payload]
    let sig_structure = ciborium::Value::Array(vec![
        ciborium::Value::Text("Signature1".to_string()),
        ciborium::Value::Bytes(protected_header.to_vec()),
        ciborium::Value::Bytes(vec![]), // external_aad
        ciborium::Value::Bytes(payload_bytes.to_vec()),
    ]);

    let mut sig_structure_bytes = Vec::new();
    ciborium::into_writer(&sig_structure, &mut sig_structure_bytes).map_err(|e| {
        AttestationError::CertChain(format!("Failed to encode Sig_structure: {}", e))
    })?;

    // P-384 COSE signature is raw r||s (48 + 48 = 96 bytes)
    if signature_bytes.len() != 96 {
        return Err(AttestationError::CertChain(format!(
            "Expected 96-byte P-384 COSE signature, got {} bytes",
            signature_bytes.len()
        )));
    }
    let signature = Signature::from_slice(signature_bytes)
        .map_err(|e| AttestationError::CertChain(format!("Invalid COSE signature: {}", e)))?;

    // Verifier::verify() hashes with SHA-384 internally
    verifying_key
        .verify(&sig_structure_bytes, &signature)
        .map_err(|e| {
            AttestationError::CertChain(format!("COSE signature verification failed: {}", e))
        })?;

    Ok(())
}

/// Verify the full certificate chain from the cabundle against the AWS Nitro root CA,
/// and verify the COSE_Sign1 signature using the leaf certificate's key.
fn verify_cert_chain(
    cabundle: &[Vec<u8>],
    protected_header: &[u8],
    payload_bytes: &[u8],
    signature_bytes: &[u8],
) -> Result<(), AttestationError> {
    if cabundle.is_empty() {
        return Err(AttestationError::CertChain(
            "Empty certificate bundle".into(),
        ));
    }

    // 1. Parse all certificates from DER
    let certs: Vec<Certificate> = cabundle
        .iter()
        .enumerate()
        .map(|(i, der)| {
            Certificate::from_der(der).map_err(|e| {
                AttestationError::CertChain(format!("Failed to parse cert[{}]: {}", i, e))
            })
        })
        .collect::<Result<_, _>>()?;

    // 2. Verify root cert matches the embedded AWS Nitro root CA
    verify_root_cert(&certs[0])?;

    // 3. Verify chain: cert[i] signed cert[i+1] (root signs first intermediate, etc.)
    for i in 1..certs.len() {
        verify_cert_signature(&certs[i - 1], &certs[i])?;
    }

    // 4. Verify COSE_Sign1 signature using leaf cert's public key
    let leaf = certs
        .last()
        .ok_or_else(|| AttestationError::CertChain("No leaf certificate".into()))?;
    verify_cose_signature(leaf, protected_header, payload_bytes, signature_bytes)?;

    Ok(())
}

/// Decoded parts of a COSE_Sign1 structure:
/// (protected_header, payload_bytes, signature_bytes, payload_map).
type CoseSign1Parts = (
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<(ciborium::Value, ciborium::Value)>,
);

/// Decode the COSE_Sign1 structure from raw document bytes and return
/// (protected_header, payload_bytes, signature_bytes, payload_map).
fn decode_cose_sign1(doc_bytes: &[u8]) -> Result<CoseSign1Parts, AttestationError> {
    let cose_value: ciborium::Value =
        ciborium::from_reader(doc_bytes).map_err(|e| AttestationError::Cbor(e.to_string()))?;

    // Extract the COSE_Sign1 array (handle both tagged and untagged)
    let items = match &cose_value {
        ciborium::Value::Tag(18, inner) => {
            if let ciborium::Value::Array(arr) = inner.as_ref() {
                arr
            } else {
                return Err(AttestationError::CoseStructure(
                    "Expected array inside Tag(18)".into(),
                ));
            }
        }
        ciborium::Value::Array(arr) => arr,
        _ => {
            return Err(AttestationError::CoseStructure(
                "Expected COSE_Sign1 array or Tag(18)".into(),
            ))
        }
    };

    if items.len() != 4 {
        return Err(AttestationError::CoseStructure(format!(
            "Expected 4 items, got {}",
            items.len()
        )));
    }

    // items[0] = protected header (bstr)
    let protected_header = match &items[0] {
        ciborium::Value::Bytes(b) => b.clone(),
        _ => {
            return Err(AttestationError::CoseStructure(
                "Protected header is not bytes".into(),
            ))
        }
    };

    // items[2] = payload (bstr)
    let payload_bytes = match &items[2] {
        ciborium::Value::Bytes(b) => b.clone(),
        _ => {
            return Err(AttestationError::CoseStructure(
                "Payload is not bytes".into(),
            ))
        }
    };

    // items[3] = signature (bstr)
    let signature_bytes = match &items[3] {
        ciborium::Value::Bytes(b) => b.clone(),
        _ => {
            return Err(AttestationError::CoseStructure(
                "Signature is not bytes".into(),
            ))
        }
    };

    // Decode payload map
    let payload_value: ciborium::Value = ciborium::from_reader(&payload_bytes[..])
        .map_err(|e| AttestationError::Cbor(format!("Payload CBOR: {}", e)))?;

    let payload_map = match payload_value {
        ciborium::Value::Map(m) => m,
        _ => {
            return Err(AttestationError::CoseStructure(
                "Payload is not a map".into(),
            ))
        }
    };

    Ok((
        protected_header,
        payload_bytes,
        signature_bytes,
        payload_map,
    ))
}

/// Parse fields from a CBOR payload map into a VerifiedAttestation.
fn parse_fields_from_map(
    payload_map: &[(ciborium::Value, ciborium::Value)],
) -> Result<VerifiedAttestation, AttestationError> {
    // Helper to get a text field
    let get_text = |key: &str| -> Result<String, AttestationError> {
        for (k, v) in payload_map {
            if let ciborium::Value::Text(k_str) = k {
                if k_str == key {
                    if let ciborium::Value::Text(val) = v {
                        return Ok(val.clone());
                    }
                }
            }
        }
        Err(AttestationError::MissingField(key.to_string()))
    };

    // Helper to get bytes field (Bytes or Null -> empty)
    let get_bytes = |key: &str| -> Result<Vec<u8>, AttestationError> {
        for (k, v) in payload_map {
            if let ciborium::Value::Text(k_str) = k {
                if k_str == key {
                    match v {
                        ciborium::Value::Bytes(val) => return Ok(val.clone()),
                        ciborium::Value::Null => return Ok(vec![]),
                        _ => {}
                    }
                }
            }
        }
        Err(AttestationError::MissingField(key.to_string()))
    };

    // Helper to get optional bytes field (returns None if field is Null or missing)
    let get_optional_bytes = |key: &str| -> Option<Vec<u8>> {
        for (k, v) in payload_map {
            if let ciborium::Value::Text(k_str) = k {
                if k_str == key {
                    match v {
                        ciborium::Value::Bytes(val) if !val.is_empty() => return Some(val.clone()),
                        _ => return None,
                    }
                }
            }
        }
        None
    };

    // Helper to get integer field
    let get_integer = |key: &str| -> Result<u64, AttestationError> {
        for (k, v) in payload_map {
            if let ciborium::Value::Text(k_str) = k {
                if k_str == key {
                    if let ciborium::Value::Integer(val) = v {
                        let n: i128 = (*val).into();
                        return Ok(n as u64);
                    }
                }
            }
        }
        Err(AttestationError::MissingField(key.to_string()))
    };

    // Extract fields
    let module_id = get_text("module_id")?;
    let timestamp = get_integer("timestamp")?;
    let public_key_bytes = get_bytes("public_key")?;
    let user_data_bytes = get_bytes("user_data").unwrap_or_default();

    // Extract nonce (optional -- may be Null or missing)
    let nonce = get_optional_bytes("nonce").map(hex::encode);

    // Extract PCRs
    let mut pcr0 = String::new();
    let mut pcr1 = String::new();
    let mut pcr2 = String::new();

    for (k, v) in payload_map {
        if let ciborium::Value::Text(k_str) = k {
            if k_str == "pcrs" {
                if let ciborium::Value::Map(pcr_map) = v {
                    for (pk, pv) in pcr_map {
                        if let (ciborium::Value::Integer(idx), ciborium::Value::Bytes(val)) =
                            (pk, pv)
                        {
                            let n: i128 = (*idx).into();
                            match n {
                                0 => pcr0 = hex::encode(val),
                                1 => pcr1 = hex::encode(val),
                                2 => pcr2 = hex::encode(val),
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    }

    if pcr0.is_empty() {
        return Err(AttestationError::MissingField("pcrs[0]".to_string()));
    }

    // Format enclave address from public_key bytes
    let enclave_address = format!("0x{}", hex::encode(&public_key_bytes));

    Ok(VerifiedAttestation {
        pcr0,
        pcr1,
        pcr2,
        enclave_address,
        user_data: hex::encode(&user_data_bytes),
        module_id,
        timestamp,
        cert_chain_verified: false,
        nonce,
    })
}

/// Parse a Nitro attestation document from base64-encoded CBOR.
///
/// This performs structural validation but does NOT verify the
/// P-384 signature or certificate chain (that requires the full
/// cert chain verification path).
pub fn parse_attestation(base64_doc: &str) -> Result<VerifiedAttestation, AttestationError> {
    use base64::Engine;
    let doc_bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_doc)
        .map_err(|e| AttestationError::Base64(e.to_string()))?;

    let (_, _, _, payload_map) = decode_cose_sign1(&doc_bytes)?;
    parse_fields_from_map(&payload_map)
}

/// Verify the attestation document's certificate chain against the AWS root CA.
///
/// For real Nitro attestations this:
/// 1. Extracts the certificate chain from the payload's cabundle
/// 2. Verifies each cert in the chain up to the AWS Nitro root CA
/// 3. Verifies the COSE_Sign1 signature using the leaf certificate's key
///
/// For dev-mode (mock) attestation documents (all-zero PCR0), this returns
/// the parsed doc with `cert_chain_verified = false`.
pub fn verify_attestation(base64_doc: &str) -> Result<VerifiedAttestation, AttestationError> {
    use base64::Engine;
    let doc_bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_doc)
        .map_err(|e| AttestationError::Base64(e.to_string()))?;

    let (protected_header, payload_bytes, signature_bytes, payload_map) =
        decode_cose_sign1(&doc_bytes)?;
    let mut attestation = parse_fields_from_map(&payload_map)?;

    // Check if this is a mock attestation (all-zero PCR0)
    let is_mock = attestation.pcr0.chars().all(|c| c == '0');

    if is_mock {
        tracing::warn!(
            "Mock attestation document detected -- skipping certificate chain verification"
        );
        return Ok(attestation);
    }

    // For real Nitro attestations, verify the P-384 certificate chain.
    let cabundle = extract_cabundle(&payload_map)?;
    verify_cert_chain(
        &cabundle,
        &protected_header,
        &payload_bytes,
        &signature_bytes,
    )?;

    tracing::info!(
        "Nitro attestation cert chain verified successfully. PCR0={}",
        attestation.pcr0
    );
    attestation.cert_chain_verified = true;

    Ok(attestation)
}

/// Validate PCR0 against an expected value.
pub fn validate_pcr0(
    attestation: &VerifiedAttestation,
    expected_pcr0: &str,
) -> Result<(), AttestationError> {
    let expected = expected_pcr0.strip_prefix("0x").unwrap_or(expected_pcr0);
    if attestation.pcr0 != expected {
        return Err(AttestationError::PcrMismatch(format!(
            "Expected PCR0={}, got {}",
            expected, attestation.pcr0
        )));
    }
    Ok(())
}

/// Validate attestation freshness (not older than max_age_secs).
pub fn validate_freshness(
    attestation: &VerifiedAttestation,
    max_age_secs: u64,
) -> Result<(), AttestationError> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| AttestationError::Expired)?
        .as_millis() as u64;

    let age_secs = (now_ms.saturating_sub(attestation.timestamp)) / 1000;
    if age_secs > max_age_secs {
        return Err(AttestationError::Expired);
    }
    Ok(())
}

/// Validate that the attestation nonce matches the expected value.
pub fn validate_nonce(
    attestation: &VerifiedAttestation,
    expected_nonce_hex: &str,
) -> Result<(), AttestationError> {
    let expected = expected_nonce_hex
        .strip_prefix("0x")
        .unwrap_or(expected_nonce_hex);
    match &attestation.nonce {
        Some(nonce) if nonce == expected => Ok(()),
        Some(nonce) => Err(AttestationError::NonceMismatch(format!(
            "Expected nonce={}, got {}",
            expected, nonce
        ))),
        None => Err(AttestationError::NonceMismatch(
            "No nonce present in attestation".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to create a mock attestation doc (same format as enclave's mock)
    fn create_mock_doc() -> String {
        create_mock_doc_with_nonce(None)
    }

    fn create_mock_doc_with_nonce(nonce: Option<&[u8]>) -> String {
        let mut payload_items: Vec<(ciborium::Value, ciborium::Value)> = Vec::new();

        payload_items.push((
            ciborium::Value::Text("module_id".to_string()),
            ciborium::Value::Text("test-mock".to_string()),
        ));

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        payload_items.push((
            ciborium::Value::Text("timestamp".to_string()),
            ciborium::Value::Integer(timestamp.into()),
        ));

        payload_items.push((
            ciborium::Value::Text("digest".to_string()),
            ciborium::Value::Text("SHA384".to_string()),
        ));

        let mock_pcr0 = vec![0u8; 48];
        let pcrs = vec![
            (
                ciborium::Value::Integer(0.into()),
                ciborium::Value::Bytes(mock_pcr0),
            ),
            (
                ciborium::Value::Integer(1.into()),
                ciborium::Value::Bytes(vec![0u8; 48]),
            ),
            (
                ciborium::Value::Integer(2.into()),
                ciborium::Value::Bytes(vec![0u8; 48]),
            ),
        ];
        payload_items.push((
            ciborium::Value::Text("pcrs".to_string()),
            ciborium::Value::Map(pcrs),
        ));

        let addr_bytes = vec![
            0xf3, 0x9f, 0xd6, 0xe5, 0x1a, 0xad, 0x88, 0xf6, 0xf4, 0xce, 0x6a, 0xb8, 0x82, 0x72,
            0x79, 0xcf, 0xff, 0xb9, 0x22, 0x66,
        ];
        payload_items.push((
            ciborium::Value::Text("public_key".to_string()),
            ciborium::Value::Bytes(addr_bytes),
        ));

        let user_data = vec![1u8; 32];
        payload_items.push((
            ciborium::Value::Text("user_data".to_string()),
            ciborium::Value::Bytes(user_data),
        ));

        // Add nonce
        let nonce_value = match nonce {
            Some(n) => ciborium::Value::Bytes(n.to_vec()),
            None => ciborium::Value::Null,
        };
        payload_items.push((ciborium::Value::Text("nonce".to_string()), nonce_value));

        let payload_value = ciborium::Value::Map(payload_items);

        // Serialize payload
        let mut payload_bytes = Vec::new();
        ciborium::into_writer(&payload_value, &mut payload_bytes).unwrap();

        // COSE_Sign1 array
        let cose = ciborium::Value::Array(vec![
            ciborium::Value::Bytes(vec![]),
            ciborium::Value::Map(vec![]),
            ciborium::Value::Bytes(payload_bytes),
            ciborium::Value::Bytes(vec![0u8; 96]),
        ]);

        let mut doc_bytes = Vec::new();
        ciborium::into_writer(&cose, &mut doc_bytes).unwrap();

        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(&doc_bytes)
    }

    #[test]
    fn test_parse_mock_attestation() {
        let doc = create_mock_doc();
        let result = parse_attestation(&doc).unwrap();

        assert_eq!(result.module_id, "test-mock");
        assert_eq!(result.pcr0.len(), 96); // 48 bytes hex
        assert!(result.pcr0.chars().all(|c| c == '0'));
        assert!(result.enclave_address.starts_with("0x"));
        assert!(!result.user_data.is_empty());
    }

    #[test]
    fn test_verify_mock_attestation() {
        let doc = create_mock_doc();
        let result = verify_attestation(&doc).unwrap();

        assert!(!result.cert_chain_verified); // Mock -> no cert chain
        assert_eq!(result.module_id, "test-mock");
    }

    #[test]
    fn test_validate_freshness() {
        let doc = create_mock_doc();
        let attestation = parse_attestation(&doc).unwrap();

        // Should be fresh (just created)
        validate_freshness(&attestation, 60).unwrap();
    }

    #[test]
    fn test_validate_freshness_expired() {
        let doc = create_mock_doc();
        let mut attestation = parse_attestation(&doc).unwrap();

        // Set timestamp to 10 minutes ago
        attestation.timestamp = attestation.timestamp.saturating_sub(600_000);

        let result = validate_freshness(&attestation, 300);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_pcr0_match() {
        let doc = create_mock_doc();
        let attestation = parse_attestation(&doc).unwrap();

        let expected_pcr0 = "0".repeat(96);
        validate_pcr0(&attestation, &expected_pcr0).unwrap();
    }

    #[test]
    fn test_validate_pcr0_mismatch() {
        let doc = create_mock_doc();
        let attestation = parse_attestation(&doc).unwrap();

        let result = validate_pcr0(&attestation, "deadbeef");
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_base64() {
        let result = parse_attestation("not-valid-base64!!!");
        assert!(result.is_err());
    }

    // --- New tests for nonce and cert chain ---

    #[test]
    fn test_nonce_extraction_null() {
        let doc = create_mock_doc();
        let result = parse_attestation(&doc).unwrap();
        assert!(result.nonce.is_none(), "Null nonce should parse as None");
    }

    #[test]
    fn test_nonce_extraction_present() {
        let nonce_bytes = b"deadbeef01234567deadbeef01234567";
        let doc = create_mock_doc_with_nonce(Some(nonce_bytes));
        let result = parse_attestation(&doc).unwrap();
        assert!(result.nonce.is_some());
        assert_eq!(result.nonce.unwrap(), hex::encode(nonce_bytes));
    }

    #[test]
    fn test_validate_nonce_match() {
        let nonce_bytes = vec![0xde, 0xad, 0xbe, 0xef];
        let doc = create_mock_doc_with_nonce(Some(&nonce_bytes));
        let result = parse_attestation(&doc).unwrap();
        validate_nonce(&result, "deadbeef").unwrap();
    }

    #[test]
    fn test_validate_nonce_mismatch() {
        let nonce_bytes = vec![0xde, 0xad, 0xbe, 0xef];
        let doc = create_mock_doc_with_nonce(Some(&nonce_bytes));
        let result = parse_attestation(&doc).unwrap();
        let err = validate_nonce(&result, "cafebabe");
        assert!(err.is_err());
    }

    #[test]
    fn test_validate_nonce_missing() {
        let doc = create_mock_doc();
        let result = parse_attestation(&doc).unwrap();
        let err = validate_nonce(&result, "anything");
        assert!(err.is_err());
    }

    #[test]
    fn test_mock_attestation_skips_cert_chain() {
        // Mock attestation (all-zero PCR0) should not attempt cert chain verification
        // even when verify_attestation is called.
        let doc = create_mock_doc();
        let result = verify_attestation(&doc).unwrap();
        assert!(!result.cert_chain_verified);
    }

    #[test]
    fn test_root_ca_parses() {
        // Verify we can decode the embedded AWS Nitro root CA PEM
        let root_ca_der = parse_root_ca_der().unwrap();
        assert!(!root_ca_der.is_empty());
        let cert = Certificate::from_der(&root_ca_der).unwrap();
        // Verify it's a P-384 key (OID 1.3.132.0.34 = secp384r1)
        let pub_key_bytes = cert
            .tbs_certificate
            .subject_public_key_info
            .subject_public_key
            .raw_bytes();
        // P-384 uncompressed point is 97 bytes (0x04 + 48 + 48)
        assert_eq!(pub_key_bytes.len(), 97);
        assert_eq!(pub_key_bytes[0], 0x04); // uncompressed point prefix
                                            // Verify we can construct a VerifyingKey from it
        let _vk = VerifyingKey::from_sec1_bytes(pub_key_bytes).unwrap();
    }

    #[test]
    fn test_extract_cabundle_missing() {
        // Payload with no cabundle should return MissingField
        let payload_map = vec![(
            ciborium::Value::Text("module_id".to_string()),
            ciborium::Value::Text("test".to_string()),
        )];
        let result = extract_cabundle(&payload_map);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_cabundle_empty_array() {
        let payload_map = vec![(
            ciborium::Value::Text("cabundle".to_string()),
            ciborium::Value::Array(vec![]),
        )];
        let result = extract_cabundle(&payload_map).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_decode_cose_sign1_structure() {
        // Build a minimal COSE_Sign1 and verify decode
        let mut payload_bytes = Vec::new();
        let payload = ciborium::Value::Map(vec![(
            ciborium::Value::Text("module_id".to_string()),
            ciborium::Value::Text("test".to_string()),
        )]);
        ciborium::into_writer(&payload, &mut payload_bytes).unwrap();

        let cose = ciborium::Value::Array(vec![
            ciborium::Value::Bytes(vec![0xA1]), // protected header
            ciborium::Value::Map(vec![]),       // unprotected
            ciborium::Value::Bytes(payload_bytes.clone()),
            ciborium::Value::Bytes(vec![0u8; 96]), // signature
        ]);
        let mut doc_bytes = Vec::new();
        ciborium::into_writer(&cose, &mut doc_bytes).unwrap();

        let (ph, pb, sig, map) = decode_cose_sign1(&doc_bytes).unwrap();
        assert_eq!(ph, vec![0xA1]);
        assert_eq!(pb, payload_bytes);
        assert_eq!(sig.len(), 96);
        assert!(!map.is_empty());
    }

    // --- P-384 certificate chain end-to-end tests ---

    /// Helper: Generate a P-384 signing key.
    fn gen_p384_key() -> p384::ecdsa::SigningKey {
        use p384::elliptic_curve::rand_core::OsRng;
        p384::ecdsa::SigningKey::random(&mut OsRng)
    }

    /// Helper: Build a minimal self-signed X.509 certificate DER from a P-384 key.
    /// Uses raw DER construction (no builder) to keep it simple.
    fn build_self_signed_cert_der(signing_key: &p384::ecdsa::SigningKey) -> Vec<u8> {
        use p384::ecdsa::signature::Signer;

        let verifying_key = signing_key.verifying_key();
        let pub_bytes = verifying_key.to_sec1_bytes();

        // Build a minimal TBS Certificate in DER
        let tbs = build_minimal_tbs_der(&pub_bytes);

        // Sign the TBS with SHA-384 + ECDSA
        let sig: p384::ecdsa::DerSignature = signing_key.sign(&tbs);
        let sig_bytes = sig.as_bytes();

        // Wrap: SEQUENCE { tbs, signatureAlgorithm, signatureValue }
        let sig_algo = ecdsa_with_sha384_algo_der();
        let sig_bitstring = der_bitstring(sig_bytes);

        der_sequence(&[&tbs, &sig_algo, &sig_bitstring])
    }

    /// Build a minimal TBS Certificate DER.
    fn build_minimal_tbs_der(pub_key_sec1: &[u8]) -> Vec<u8> {
        // version [0] EXPLICIT INTEGER 2 (v3)
        let version = &[0xa0, 0x03, 0x02, 0x01, 0x02];

        // serialNumber INTEGER 1
        let serial = &[0x02, 0x01, 0x01];

        // signature algorithm: ecdsa-with-SHA384
        let sig_algo = ecdsa_with_sha384_algo_der();

        // issuer: CN=test
        let issuer = der_name("test");

        // validity: notBefore/notAfter (generalized time)
        let validity = build_validity_der();

        // subject: CN=test
        let subject = der_name("test");

        // subjectPublicKeyInfo: P-384
        let spki = build_spki_der(pub_key_sec1);

        der_sequence(&[
            version, serial, &sig_algo, &issuer, &validity, &subject, &spki,
        ])
    }

    /// Build a cert signed by an issuer key.
    fn build_signed_cert_der(
        issuer_key: &p384::ecdsa::SigningKey,
        subject_key: &p384::ecdsa::SigningKey,
        issuer_name: &str,
        subject_name: &str,
    ) -> Vec<u8> {
        use p384::ecdsa::signature::Signer;

        let verifying_key = subject_key.verifying_key();
        let pub_bytes = verifying_key.to_sec1_bytes();

        let tbs = build_tbs_with_names(&pub_bytes, issuer_name, subject_name);

        let sig: p384::ecdsa::DerSignature = issuer_key.sign(&tbs);
        let sig_bytes = sig.as_bytes();

        let sig_algo = ecdsa_with_sha384_algo_der();
        let sig_bitstring = der_bitstring(sig_bytes);

        der_sequence(&[&tbs, &sig_algo, &sig_bitstring])
    }

    fn build_tbs_with_names(pub_key_sec1: &[u8], issuer: &str, subject: &str) -> Vec<u8> {
        let version = &[0xa0, 0x03, 0x02, 0x01, 0x02];
        let serial = &[0x02, 0x01, 0x01];
        let sig_algo = ecdsa_with_sha384_algo_der();
        let issuer_dn = der_name(issuer);
        let validity = build_validity_der();
        let subject_dn = der_name(subject);
        let spki = build_spki_der(pub_key_sec1);
        der_sequence(&[
            version,
            serial,
            &sig_algo,
            &issuer_dn,
            &validity,
            &subject_dn,
            &spki,
        ])
    }

    /// ecdsa-with-SHA384 AlgorithmIdentifier DER
    fn ecdsa_with_sha384_algo_der() -> Vec<u8> {
        // SEQUENCE { OID 1.2.840.10045.4.3.3 }
        let oid = &[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x03];
        der_sequence(&[oid])
    }

    /// Build SubjectPublicKeyInfo for P-384
    fn build_spki_der(pub_key_sec1: &[u8]) -> Vec<u8> {
        // algorithm: SEQUENCE { OID ecPublicKey, OID secp384r1 }
        let ec_oid = &[0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01]; // 1.2.840.10045.2.1
        let curve_oid = &[0x06, 0x05, 0x2b, 0x81, 0x04, 0x00, 0x22]; // 1.3.132.0.34
        let algo = der_sequence(&[ec_oid, curve_oid]);
        let pub_bits = der_bitstring(pub_key_sec1);
        der_sequence(&[&algo, &pub_bits])
    }

    /// Build Validity (notBefore/notAfter) with wide window
    fn build_validity_der() -> Vec<u8> {
        // UTCTime "190101000000Z" to "491231235959Z"
        let not_before = der_utctime("190101000000Z");
        let not_after = der_utctime("491231235959Z");
        der_sequence(&[&not_before, &not_after])
    }

    /// Build a Name with just CN=value
    fn der_name(cn: &str) -> Vec<u8> {
        let cn_oid = &[0x06, 0x03, 0x55, 0x04, 0x03]; // 2.5.4.3
        let cn_val = der_utf8string(cn);
        let attr = der_sequence(&[cn_oid, &cn_val]);
        let rdn = der_set(&[&attr]);
        der_sequence(&[&rdn])
    }

    fn der_utctime(s: &str) -> Vec<u8> {
        let mut out = vec![0x17, s.len() as u8];
        out.extend_from_slice(s.as_bytes());
        out
    }

    fn der_utf8string(s: &str) -> Vec<u8> {
        let mut out = vec![0x0c, s.len() as u8];
        out.extend_from_slice(s.as_bytes());
        out
    }

    fn der_bitstring(data: &[u8]) -> Vec<u8> {
        // BIT STRING: tag 0x03, length = data.len()+1, unused_bits = 0, data
        let len = data.len() + 1;
        let mut out = vec![0x03];
        out.extend_from_slice(&der_length(len));
        out.push(0x00); // unused bits
        out.extend_from_slice(data);
        out
    }

    fn der_sequence(items: &[&[u8]]) -> Vec<u8> {
        let total: usize = items.iter().map(|i| i.len()).sum();
        let mut out = vec![0x30];
        out.extend_from_slice(&der_length(total));
        for item in items {
            out.extend_from_slice(item);
        }
        out
    }

    fn der_set(items: &[&[u8]]) -> Vec<u8> {
        let total: usize = items.iter().map(|i| i.len()).sum();
        let mut out = vec![0x31];
        out.extend_from_slice(&der_length(total));
        for item in items {
            out.extend_from_slice(item);
        }
        out
    }

    fn der_length(len: usize) -> Vec<u8> {
        if len < 128 {
            vec![len as u8]
        } else if len < 256 {
            vec![0x81, len as u8]
        } else {
            vec![0x82, (len >> 8) as u8, (len & 0xff) as u8]
        }
    }

    #[test]
    fn test_self_signed_cert_chain_verification() {
        // Generate a root key and self-signed cert
        let root_key = gen_p384_key();
        let root_cert_der = build_self_signed_cert_der(&root_key);

        // Verify the cert parses
        let root_cert = Certificate::from_der(&root_cert_der).unwrap();

        // Verify the self-signed cert signature
        verify_cert_signature(&root_cert, &root_cert).unwrap();
    }

    #[test]
    fn test_two_level_cert_chain_verification() {
        // Root key + cert
        let root_key = gen_p384_key();
        let root_cert_der = build_self_signed_cert_der(&root_key);
        let root_cert = Certificate::from_der(&root_cert_der).unwrap();

        // Leaf key + cert signed by root
        let leaf_key = gen_p384_key();
        let leaf_cert_der = build_signed_cert_der(&root_key, &leaf_key, "root", "leaf");
        let leaf_cert = Certificate::from_der(&leaf_cert_der).unwrap();

        // Verify root signed leaf
        verify_cert_signature(&root_cert, &leaf_cert).unwrap();
    }

    #[test]
    fn test_cert_chain_wrong_signer_fails() {
        // Root key + cert
        let root_key = gen_p384_key();
        let root_cert_der = build_self_signed_cert_der(&root_key);
        let root_cert = Certificate::from_der(&root_cert_der).unwrap();

        // Leaf key + cert signed by a DIFFERENT key (not root)
        let other_key = gen_p384_key();
        let leaf_key = gen_p384_key();
        let leaf_cert_der = build_signed_cert_der(&other_key, &leaf_key, "root", "leaf");
        let leaf_cert = Certificate::from_der(&leaf_cert_der).unwrap();

        // Should fail — root didn't sign this
        let result = verify_cert_signature(&root_cert, &leaf_cert);
        assert!(result.is_err());
    }

    #[test]
    fn test_cose_sign1_e2e_with_test_cert() {
        use p384::ecdsa::signature::Signer;

        // Generate key + self-signed cert
        let leaf_key = gen_p384_key();
        let leaf_cert_der = build_self_signed_cert_der(&leaf_key);
        let leaf_cert = Certificate::from_der(&leaf_cert_der).unwrap();

        // Build a COSE protected header: {1: -35} (alg: ES384)
        let protected_header_map = ciborium::Value::Map(vec![(
            ciborium::Value::Integer(1.into()),
            ciborium::Value::Integer(ciborium::value::Integer::from(-35)),
        )]);
        let mut protected_bytes = Vec::new();
        ciborium::into_writer(&protected_header_map, &mut protected_bytes).unwrap();

        // Build payload
        let payload = ciborium::Value::Map(vec![(
            ciborium::Value::Text("module_id".to_string()),
            ciborium::Value::Text("test-e2e".to_string()),
        )]);
        let mut payload_bytes = Vec::new();
        ciborium::into_writer(&payload, &mut payload_bytes).unwrap();

        // Build Sig_structure and sign
        let sig_structure = ciborium::Value::Array(vec![
            ciborium::Value::Text("Signature1".to_string()),
            ciborium::Value::Bytes(protected_bytes.clone()),
            ciborium::Value::Bytes(vec![]),
            ciborium::Value::Bytes(payload_bytes.clone()),
        ]);
        let mut sig_input = Vec::new();
        ciborium::into_writer(&sig_structure, &mut sig_input).unwrap();

        // Sign with raw r||s (96 bytes)
        let sig: Signature = leaf_key.sign(&sig_input);
        let sig_raw = sig.to_bytes();
        assert_eq!(sig_raw.len(), 96);

        // Verify COSE signature
        verify_cose_signature(&leaf_cert, &protected_bytes, &payload_bytes, &sig_raw).unwrap();
    }

    #[test]
    fn test_full_cert_chain_and_cose_e2e() {
        use p384::ecdsa::signature::Signer;

        // 3-level chain: root -> intermediate -> leaf
        let root_key = gen_p384_key();
        let root_cert_der = build_self_signed_cert_der(&root_key);

        let int_key = gen_p384_key();
        let int_cert_der = build_signed_cert_der(&root_key, &int_key, "root", "intermediate");

        let leaf_key = gen_p384_key();
        let leaf_cert_der = build_signed_cert_der(&int_key, &leaf_key, "intermediate", "leaf");

        // Parse all certs
        let root_cert = Certificate::from_der(&root_cert_der).unwrap();
        let int_cert = Certificate::from_der(&int_cert_der).unwrap();
        let leaf_cert = Certificate::from_der(&leaf_cert_der).unwrap();

        // Verify chain signatures: root->self, root->int, int->leaf
        verify_cert_signature(&root_cert, &root_cert).unwrap();
        verify_cert_signature(&root_cert, &int_cert).unwrap();
        verify_cert_signature(&int_cert, &leaf_cert).unwrap();

        // Build COSE_Sign1 signed by leaf
        let protected_header_map = ciborium::Value::Map(vec![(
            ciborium::Value::Integer(1.into()),
            ciborium::Value::Integer(ciborium::value::Integer::from(-35)),
        )]);
        let mut protected_bytes = Vec::new();
        ciborium::into_writer(&protected_header_map, &mut protected_bytes).unwrap();

        let payload = ciborium::Value::Map(vec![(
            ciborium::Value::Text("module_id".to_string()),
            ciborium::Value::Text("e2e-chain-test".to_string()),
        )]);
        let mut payload_bytes = Vec::new();
        ciborium::into_writer(&payload, &mut payload_bytes).unwrap();

        let sig_structure = ciborium::Value::Array(vec![
            ciborium::Value::Text("Signature1".to_string()),
            ciborium::Value::Bytes(protected_bytes.clone()),
            ciborium::Value::Bytes(vec![]),
            ciborium::Value::Bytes(payload_bytes.clone()),
        ]);
        let mut sig_input = Vec::new();
        ciborium::into_writer(&sig_structure, &mut sig_input).unwrap();

        let sig: Signature = leaf_key.sign(&sig_input);
        let sig_raw = sig.to_bytes();

        // Verify COSE signature with leaf cert
        verify_cose_signature(&leaf_cert, &protected_bytes, &payload_bytes, &sig_raw).unwrap();

        // Cross-signer should fail: root cert's key should NOT verify COSE signed by leaf
        let result = verify_cose_signature(&root_cert, &protected_bytes, &payload_bytes, &sig_raw);
        assert!(result.is_err(), "Wrong key should fail COSE verification");
    }
}
