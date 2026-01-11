//! zkVM execution and proof generation

use alloy::primitives::B256;
use risc0_zkvm::{default_prover, ExecutorEnv, Receipt};
use std::path::PathBuf;
use tracing::{info, debug};

/// Execute a zkVM program and generate a proof
///
/// # Arguments
/// * `image_id` - The program's image ID
/// * `input_url` - URL to fetch inputs from
/// * `input_digest` - Expected hash of inputs
///
/// # Returns
/// * `(seal, journal)` - The proof seal and public outputs
pub async fn execute_and_prove(
    image_id: &B256,
    input_url: &str,
    input_digest: &B256,
) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    info!("Fetching inputs from: {}", input_url);

    // Fetch inputs
    let input_bytes = fetch_inputs(input_url).await?;

    // Verify input digest
    let computed_digest = compute_digest(&input_bytes);
    if computed_digest != *input_digest {
        anyhow::bail!(
            "Input digest mismatch: expected {}, got {}",
            input_digest,
            computed_digest
        );
    }

    info!("Input digest verified");

    // Fetch the program ELF
    // In production, this would be fetched from the program registry
    let elf = fetch_program_elf(image_id).await?;

    info!("Running zkVM...");
    let start = std::time::Instant::now();

    // Build executor environment with inputs
    let env = ExecutorEnv::builder()
        .write_slice(&input_bytes)
        .build()?;

    // Get prover and generate proof
    let prover = default_prover();
    let prove_info = prover.prove(env, &elf)?;

    let elapsed = start.elapsed();
    info!("Proof generated in {:.2?}", elapsed);

    let receipt = prove_info.receipt;

    // Verify locally before submitting
    receipt.verify_integrity()?;
    info!("Local verification passed");

    // Extract seal and journal
    let seal = extract_seal(&receipt)?;
    let journal = receipt.journal.bytes.clone();

    info!(
        "Proof ready: seal={} bytes, journal={} bytes",
        seal.len(),
        journal.len()
    );

    Ok((seal, journal))
}

/// Fetch inputs from URL
async fn fetch_inputs(url: &str) -> anyhow::Result<Vec<u8>> {
    // Handle different URL schemes
    if url.starts_with("ipfs://") {
        fetch_from_ipfs(url).await
    } else if url.starts_with("http://") || url.starts_with("https://") {
        fetch_from_http(url).await
    } else if url.starts_with("data:") {
        // Data URL (base64 encoded)
        parse_data_url(url)
    } else {
        anyhow::bail!("Unsupported URL scheme: {}", url)
    }
}

async fn fetch_from_http(url: &str) -> anyhow::Result<Vec<u8>> {
    let response = reqwest::get(url).await?;
    let bytes = response.bytes().await?;
    Ok(bytes.to_vec())
}

async fn fetch_from_ipfs(url: &str) -> anyhow::Result<Vec<u8>> {
    // Convert ipfs:// to HTTP gateway URL
    let cid = url.trim_start_matches("ipfs://");
    let gateway_url = format!("https://ipfs.io/ipfs/{}", cid);
    fetch_from_http(&gateway_url).await
}

fn parse_data_url(url: &str) -> anyhow::Result<Vec<u8>> {
    // Parse data:application/octet-stream;base64,XXXX
    let parts: Vec<&str> = url.splitn(2, ',').collect();
    if parts.len() != 2 {
        anyhow::bail!("Invalid data URL format");
    }

    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD.decode(parts[1])?;
    Ok(decoded)
}

/// Compute SHA256 digest of data
fn compute_digest(data: &[u8]) -> B256 {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    B256::from_slice(&result)
}

/// Fetch program ELF from registry or cache
async fn fetch_program_elf(image_id: &B256) -> anyhow::Result<Vec<u8>> {
    // In production, this would:
    // 1. Check local cache
    // 2. Query program registry for URL
    // 3. Download and verify ELF matches image ID

    // For now, look in local programs directory
    let programs_dir = PathBuf::from("./programs");
    let elf_path = programs_dir.join(format!("{}.elf", hex::encode(image_id)));

    if elf_path.exists() {
        debug!("Loading ELF from cache: {:?}", elf_path);
        let elf = std::fs::read(&elf_path)?;
        return Ok(elf);
    }

    anyhow::bail!(
        "Program ELF not found for image ID {}. \
        Please download and place in ./programs/",
        image_id
    )
}

/// Extract the proof seal from receipt
fn extract_seal(receipt: &Receipt) -> anyhow::Result<Vec<u8>> {
    // Serialize the receipt's inner proof
    let seal = bincode::serialize(&receipt.inner)?;
    Ok(seal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_digest() {
        let data = b"hello world";
        let digest = compute_digest(data);
        assert!(!digest.is_zero());
    }

    #[test]
    fn test_parse_data_url() {
        let url = "data:application/octet-stream;base64,SGVsbG8gV29ybGQ=";
        let decoded = parse_data_url(url).unwrap();
        assert_eq!(decoded, b"Hello World");
    }
}
