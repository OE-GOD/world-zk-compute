//! Sepolia integration tests using raw HTTP JSON-RPC via reqwest.
//!
//! All tests are `#[ignore]`d by default and only run when the required
//! environment variables are set.
//!
//! Required env vars:
//!   - `ALCHEMY_SEPOLIA_RPC_URL` -- Sepolia JSON-RPC endpoint
//!   - `TEE_VERIFIER_ADDRESS`    -- deployed TEEMLVerifier contract address
//!
//! Run with:
//! ```sh
//! ALCHEMY_SEPOLIA_RPC_URL=https://eth-sepolia.g.alchemy.com/v2/KEY \
//! TEE_VERIFIER_ADDRESS=0x... \
//!     cargo test -p world-zk-sdk --test integration_sepolia -- --ignored
//! ```

use std::env;

/// Read the Sepolia RPC URL from the environment.
/// Panics if not set (tests are `#[ignore]`d so this only runs when invoked explicitly).
fn rpc_url() -> String {
    env::var("ALCHEMY_SEPOLIA_RPC_URL").expect("ALCHEMY_SEPOLIA_RPC_URL must be set")
}

/// Read the TEE verifier contract address from the environment.
/// Panics if not set.
fn verifier_address() -> String {
    env::var("TEE_VERIFIER_ADDRESS").expect("TEE_VERIFIER_ADDRESS must be set")
}

/// Send a JSON-RPC request and return the parsed response body.
async fn rpc_call(
    client: &reqwest::Client,
    url: &str,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let resp = client
        .post(url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": 1
        }))
        .send()
        .await
        .expect("Failed to send RPC request");

    assert!(
        resp.status().is_success(),
        "RPC returned non-200 status: {}",
        resp.status()
    );

    resp.json::<serde_json::Value>()
        .await
        .expect("Failed to parse JSON response")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires Sepolia RPC"]
async fn test_sepolia_rpc_connectivity() {
    let url = rpc_url();
    let client = reqwest::Client::new();

    let body = rpc_call(&client, &url, "eth_blockNumber", serde_json::json!([])).await;

    let block_hex = body["result"]
        .as_str()
        .expect("Missing 'result' field in eth_blockNumber response");

    let block_number =
        u64::from_str_radix(block_hex.trim_start_matches("0x"), 16).expect("Invalid hex block number");

    assert!(
        block_number > 0,
        "Block number should be positive, got {block_number}"
    );

    println!("Sepolia block number: {block_number}");
}

#[tokio::test]
#[ignore = "requires Sepolia RPC"]
async fn test_sepolia_contract_has_code() {
    let url = rpc_url();
    let address = verifier_address();
    let client = reqwest::Client::new();

    let body = rpc_call(
        &client,
        &url,
        "eth_getCode",
        serde_json::json!([address, "latest"]),
    )
    .await;

    let code = body["result"]
        .as_str()
        .expect("Missing 'result' field in eth_getCode response");

    // An address with no deployed code returns "0x" (length 2).
    // Any deployed contract returns "0x<bytecode>" with length > 2.
    assert!(
        code.len() > 2,
        "No bytecode at {address} (got '{code}'). Is the contract deployed on Sepolia?"
    );

    let code_bytes = (code.len() - 2) / 2;
    println!("Contract {address} has {code_bytes} bytes of deployed code");
}

#[tokio::test]
#[ignore = "requires Sepolia RPC"]
async fn test_sepolia_owner_query() {
    let url = rpc_url();
    let address = verifier_address();
    let client = reqwest::Client::new();

    // owner() function selector: keccak256("owner()")[:4] = 0x8da5cb5b
    let body = rpc_call(
        &client,
        &url,
        "eth_call",
        serde_json::json!([
            {
                "to": address,
                "data": "0x8da5cb5b"
            },
            "latest"
        ]),
    )
    .await;

    let raw = body["result"]
        .as_str()
        .expect("Missing 'result' field in eth_call response");

    // ABI-encoded address: 0x + 64 hex chars (32 bytes, zero-padded on the left)
    assert_eq!(
        raw.len(),
        66,
        "Unexpected return data length for owner(): expected 66 chars (0x + 64 hex), got {}",
        raw.len()
    );

    // Extract the address from the last 40 hex characters
    let owner_hex = &raw[raw.len() - 40..];
    let zero_addr = "0".repeat(40);
    assert_ne!(
        owner_hex, zero_addr,
        "owner() returned the zero address; expected a real admin address"
    );

    println!("Contract owner: 0x{owner_hex}");
}
