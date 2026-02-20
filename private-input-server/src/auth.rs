//! Authentication & Authorization
//!
//! Verifies prover identity via EIP-191 signatures and checks on-chain
//! that the prover has claimed the job before releasing data.

use alloy::primitives::Address;
use alloy::signers::Signature;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// Authentication request from the prover (sent as JSON body)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthRequest {
    /// The on-chain request ID
    pub request_id: u64,
    /// The prover's Ethereum address (checksummed)
    pub prover_address: String,
    /// Unix timestamp of the request
    pub timestamp: u64,
    /// EIP-191 signature hex string (0x-prefixed)
    pub signature: String,
}

/// Maximum age of a signed request (5 minutes)
const MAX_TIMESTAMP_AGE_SECS: u64 = 300;

/// Verify the AuthRequest signature and timestamp
///
/// 1. Check timestamp freshness (max 5 min old)
/// 2. Reconstruct expected message string
/// 3. Recover signer from EIP-191 signature
/// 4. Verify recovered address == claimed prover_address
pub fn verify_auth_request(auth: &AuthRequest) -> Result<Address, AuthError> {
    // Step 1: Check timestamp freshness
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    if now.saturating_sub(auth.timestamp) > MAX_TIMESTAMP_AGE_SECS {
        return Err(AuthError::TimestampExpired);
    }

    // Also reject timestamps too far in the future (clock skew protection)
    if auth.timestamp > now + 60 {
        return Err(AuthError::TimestampFuture);
    }

    // Step 2: Parse the claimed prover address
    let claimed_address: Address = auth
        .prover_address
        .parse()
        .map_err(|_| AuthError::InvalidAddress)?;

    // Step 3: Reconstruct the expected message
    let message = format!(
        "world-zk-compute:fetch-input:{}:{}:{}",
        auth.request_id, auth.prover_address, auth.timestamp,
    );

    debug!("Verifying auth message: {}", message);

    // Step 4: Recover signer from EIP-191 signature
    let sig_bytes = hex::decode(auth.signature.trim_start_matches("0x"))
        .map_err(|_| AuthError::InvalidSignature("bad hex".into()))?;

    if sig_bytes.len() != 65 {
        return Err(AuthError::InvalidSignature(format!(
            "expected 65 bytes, got {}",
            sig_bytes.len()
        )));
    }

    let signature = Signature::try_from(sig_bytes.as_slice())
        .map_err(|e| AuthError::InvalidSignature(format!("parse failed: {}", e)))?;

    let recovered = signature
        .recover_address_from_msg(message.as_bytes())
        .map_err(|e| AuthError::InvalidSignature(format!("recovery failed: {}", e)))?;

    // Step 5: Verify recovered address matches claimed address
    if recovered != claimed_address {
        warn!(
            "Address mismatch: recovered={}, claimed={}",
            recovered, claimed_address
        );
        return Err(AuthError::AddressMismatch {
            recovered: recovered.to_string(),
            claimed: claimed_address.to_string(),
        });
    }

    Ok(claimed_address)
}

/// Verify on-chain that the prover has claimed this job
///
/// Makes an RPC call to `ExecutionEngine.getRequest(requestId)` and checks:
/// - Status is Claimed (1)
/// - claimedBy matches the prover address
pub async fn verify_on_chain_claim(
    rpc_url: &str,
    engine_address: &Address,
    request_id: u64,
    prover: &Address,
) -> Result<(), AuthError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| AuthError::RpcError(format!("HTTP client error: {}", e)))?;

    // Encode getRequest(uint256) call
    // Function selector: keccak256("getRequest(uint256)")[:4]
    let selector = &alloy::primitives::keccak256(b"getRequest(uint256)")[..4];
    let mut call_data = selector.to_vec();
    let mut id_bytes = [0u8; 32];
    id_bytes[24..].copy_from_slice(&request_id.to_be_bytes());
    call_data.extend_from_slice(&id_bytes);

    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "eth_call",
        "params": [{
            "to": format!("{}", engine_address),
            "data": format!("0x{}", hex::encode(&call_data))
        }, "latest"],
        "id": 1
    });

    let response = client
        .post(rpc_url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| AuthError::RpcError(format!("RPC request failed: {}", e)))?;

    let result: serde_json::Value = response
        .json()
        .await
        .map_err(|e| AuthError::RpcError(format!("RPC response parse error: {}", e)))?;

    if let Some(error) = result.get("error") {
        return Err(AuthError::RpcError(format!("RPC error: {}", error)));
    }

    let data_hex = result
        .get("result")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AuthError::RpcError("Invalid RPC response".into()))?;

    let bytes = hex::decode(data_hex.trim_start_matches("0x"))
        .map_err(|e| AuthError::RpcError(format!("Bad hex response: {}", e)))?;

    // ABI decode the ExecutionRequest struct
    // The struct fields in order (each 32 bytes):
    // 0: id, 1: imageId, 2: inputDigest, 3: requester, 4: createdAt, 5: expiresAt,
    // 6: callbackContract, 7: status, 8: claimedBy, 9: claimedAt, 10: claimDeadline,
    // 11: tip, 12: maxTip
    // Note: The struct is returned with an ABI offset word (32 bytes) at the start
    if bytes.len() < 32 + 13 * 32 {
        return Err(AuthError::RpcError(format!(
            "Response too short: {} bytes",
            bytes.len()
        )));
    }

    let offset = 32; // Skip ABI offset word

    // Status is field 7 (index 7) - uint8 packed, but ABI returns it as uint256
    let status_offset = offset + 7 * 32;
    let status = bytes[status_offset + 31]; // Last byte of the 32-byte word

    // ClaimedBy is field 8 (index 8) - address (last 20 bytes of 32-byte word)
    let claimer_offset = offset + 8 * 32;
    let claimer = Address::from_slice(&bytes[claimer_offset + 12..claimer_offset + 32]);

    // Status 1 = Claimed
    if status != 1 {
        return Err(AuthError::NotClaimed { status });
    }

    if claimer != *prover {
        return Err(AuthError::WrongClaimer {
            expected: prover.to_string(),
            actual: claimer.to_string(),
        });
    }

    debug!(
        "On-chain claim verified: request={}, prover={}",
        request_id, prover
    );
    Ok(())
}

/// Authentication errors
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Timestamp expired (max {} seconds)", MAX_TIMESTAMP_AGE_SECS)]
    TimestampExpired,

    #[error("Timestamp is too far in the future")]
    TimestampFuture,

    #[error("Invalid prover address format")]
    InvalidAddress,

    #[error("Invalid signature: {0}")]
    InvalidSignature(String),

    #[error("Address mismatch: recovered={recovered}, claimed={claimed}")]
    AddressMismatch { recovered: String, claimed: String },

    #[error("Job is not in Claimed status (status={status})")]
    NotClaimed { status: u8 },

    #[error("Wrong claimer: expected={expected}, actual={actual}")]
    WrongClaimer { expected: String, actual: String },

    #[error("RPC error: {0}")]
    RpcError(String),
}

impl AuthError {
    /// HTTP status code for this error
    pub fn status_code(&self) -> axum::http::StatusCode {
        use axum::http::StatusCode;
        match self {
            AuthError::TimestampExpired | AuthError::TimestampFuture => StatusCode::UNAUTHORIZED,
            AuthError::InvalidAddress => StatusCode::BAD_REQUEST,
            AuthError::InvalidSignature(_) => StatusCode::UNAUTHORIZED,
            AuthError::AddressMismatch { .. } => StatusCode::UNAUTHORIZED,
            AuthError::NotClaimed { .. } => StatusCode::FORBIDDEN,
            AuthError::WrongClaimer { .. } => StatusCode::FORBIDDEN,
            AuthError::RpcError(_) => StatusCode::BAD_GATEWAY,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::signers::local::PrivateKeySigner;
    use alloy::signers::Signer;

    #[tokio::test]
    async fn test_verify_valid_auth_request() {
        let signer = PrivateKeySigner::random();
        let address = signer.address();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let message = format!(
            "world-zk-compute:fetch-input:{}:{}:{}",
            42,
            address.to_checksum(None),
            now,
        );
        let sig = signer.sign_message(message.as_bytes()).await.unwrap();

        let auth = AuthRequest {
            request_id: 42,
            prover_address: address.to_checksum(None),
            timestamp: now,
            signature: format!("0x{}", hex::encode(sig.as_bytes())),
        };

        let result = verify_auth_request(&auth);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), address);
    }

    #[test]
    fn test_verify_expired_timestamp() {
        let auth = AuthRequest {
            request_id: 42,
            prover_address: "0x0000000000000000000000000000000000000001".to_string(),
            timestamp: 1000, // Very old
            signature: "0x".to_string() + &"00".repeat(65),
        };

        let result = verify_auth_request(&auth);
        assert!(matches!(result, Err(AuthError::TimestampExpired)));
    }

    #[tokio::test]
    async fn test_verify_wrong_signer() {
        let signer1 = PrivateKeySigner::random();
        let signer2 = PrivateKeySigner::random();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Sign with signer1 but claim to be signer2
        let message = format!(
            "world-zk-compute:fetch-input:{}:{}:{}",
            42,
            signer2.address().to_checksum(None),
            now,
        );
        let sig = signer1.sign_message(message.as_bytes()).await.unwrap();

        let auth = AuthRequest {
            request_id: 42,
            prover_address: signer2.address().to_checksum(None),
            timestamp: now,
            signature: format!("0x{}", hex::encode(sig.as_bytes())),
        };

        let result = verify_auth_request(&auth);
        assert!(matches!(result, Err(AuthError::AddressMismatch { .. })));
    }
}
