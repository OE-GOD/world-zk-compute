use alloy::primitives::{Address, B256, U256};
use alloy::providers::ProviderBuilder;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol;
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::str::FromStr;
use url::Url;

// ---------------------------------------------------------------------------
// Contract ABI (subset needed for admin operations)
// ---------------------------------------------------------------------------

sol! {
    #[sol(rpc)]
    contract TEEMLVerifier {
        function registerEnclave(address enclaveKey, bytes32 enclaveImageHash) external;
        function revokeEnclave(address enclaveKey) external;
        function setRemainderVerifier(address _verifier) external;
        function setChallengeBondAmount(uint256 _amount) external;
        function setProverStake(uint256 _amount) external;

        function pause() external;
        function unpause() external;
        function transferOwnership(address newOwner) external;
        function acceptOwnership() external;

        function owner() external view returns (address);
        function pendingOwner() external view returns (address);
        function paused() external view returns (bool);
        function remainderVerifier() external view returns (address);
        function challengeBondAmount() external view returns (uint256);
        function proverStake() external view returns (uint256);
    }
}

sol! {
    #[sol(rpc)]
    contract RemainderVerifier {
        function admin() external view returns (address);
        function timelock() external view returns (address);
        function paused() external view returns (bool);
        function implementation() external view returns (address);

        function pause() external;
        function unpause() external;
        function changeAdmin(address newAdmin) external;
        function setTimelock(address _timelock) external;

        function registerDAGCircuit(bytes32 circuitHash, bytes calldata descData, string calldata name, bytes32 gensHash) external;
        function deactivateCircuit(bytes32 circuitHash) external;
        function reactivateCircuit(bytes32 circuitHash) external;
        function deactivateDAGCircuit(bytes32 circuitHash) external;
        function reactivateDAGCircuit(bytes32 circuitHash) external;

        function getCircuitHashes() external view returns (bytes32[]);
    }
}

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

/// Admin CLI for TEEMLVerifier and RemainderVerifier contract management.
#[derive(Parser, Debug)]
#[command(name = "admin-cli", version, about)]
struct Cli {
    /// RPC endpoint URL (e.g. http://localhost:8545 or https://sepolia.infura.io/v3/KEY).
    #[arg(long, env = "RPC_URL")]
    rpc_url: String,

    /// Contract address of the TEEMLVerifier.
    #[arg(long, env = "CONTRACT_ADDRESS")]
    contract: String,

    /// Contract address of the RemainderVerifier (required for remainder-* commands).
    #[arg(long, env = "REMAINDER_ADDRESS")]
    remainder_address: Option<String>,

    /// Private key for write operations (hex, with or without 0x prefix).
    /// Not required for read-only commands like `status`.
    #[arg(long, env = "PRIVATE_KEY")]
    private_key: Option<String>,

    /// Simulate the transaction without sending it on-chain.
    #[arg(long, default_value_t = false)]
    dry_run: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug, Clone)]
enum Command {
    /// Print contract status: owner, paused, proverStake, challengeBondAmount, remainderVerifier.
    Status,

    /// Pause the contract (onlyOwner).
    Pause,

    /// Unpause the contract (onlyOwner).
    Unpause,

    /// Register a TEE enclave.
    RegisterEnclave {
        /// Enclave signer address.
        address: String,
        /// Enclave image hash (bytes32 hex).
        image_hash: String,
    },

    /// Revoke a TEE enclave.
    RevokeEnclave {
        /// Enclave signer address to revoke.
        address: String,
    },

    /// Set the prover stake amount (in ETH, converted to wei).
    SetStake {
        /// Amount in ETH (e.g. "0.01").
        amount: String,
    },

    /// Set the challenge bond amount (in ETH, converted to wei).
    SetBond {
        /// Amount in ETH (e.g. "0.005").
        amount: String,
    },

    /// Set the RemainderVerifier contract address.
    SetVerifier {
        /// Address of the new RemainderVerifier.
        address: String,
    },

    /// Initiate 2-step ownership transfer (Ownable2Step).
    TransferOwnership {
        /// Address of the new owner.
        address: String,
    },

    /// Accept pending ownership transfer (must be called by pending owner).
    AcceptOwnership,

    // ----- RemainderVerifier commands -----
    /// Print RemainderVerifier status: admin, timelock, paused, implementation.
    RemainderStatus,

    /// Pause the RemainderVerifier (admin only).
    RemainderPause,

    /// Unpause the RemainderVerifier (admin only).
    RemainderUnpause,

    /// Change the RemainderVerifier admin address.
    RemainderChangeAdmin {
        /// Address of the new admin.
        new_admin: String,
    },

    /// Set the RemainderVerifier timelock address.
    RemainderSetTimelock {
        /// Address of the timelock contract.
        timelock_address: String,
    },

    /// Deactivate a circuit in the RemainderVerifier.
    RemainderDeactivateCircuit {
        /// Circuit hash (bytes32 hex).
        circuit_hash: String,
    },

    /// Reactivate a circuit in the RemainderVerifier.
    RemainderReactivateCircuit {
        /// Circuit hash (bytes32 hex).
        circuit_hash: String,
    },

    /// List all registered circuit hashes in the RemainderVerifier.
    RemainderListCircuits,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_address(s: &str) -> Result<Address> {
    Address::from_str(s).with_context(|| format!("invalid address: {s}"))
}

fn parse_b256(s: &str) -> Result<B256> {
    B256::from_str(s).with_context(|| format!("invalid bytes32: {s}"))
}

/// Parse an ETH amount string (e.g. "0.01") into wei (U256).
fn parse_ether(s: &str) -> Result<U256> {
    alloy::primitives::utils::parse_ether(s).with_context(|| format!("invalid ETH amount: {s}"))
}

fn format_ether(wei: U256) -> String {
    alloy::primitives::utils::format_ether(wei)
}

fn require_private_key(cli: &Cli) -> Result<&str> {
    cli.private_key
        .as_deref()
        .with_context(|| "write commands require --private-key")
}

fn require_remainder_address(cli: &Cli) -> Result<Address> {
    let addr_str = cli
        .remainder_address
        .as_deref()
        .with_context(|| "remainder-* commands require --remainder-address")?;
    parse_address(addr_str)
}

/// Returns true if the command targets the RemainderVerifier contract.
fn is_remainder_command(cmd: &Command) -> bool {
    matches!(
        cmd,
        Command::RemainderStatus
            | Command::RemainderPause
            | Command::RemainderUnpause
            | Command::RemainderChangeAdmin { .. }
            | Command::RemainderSetTimelock { .. }
            | Command::RemainderDeactivateCircuit { .. }
            | Command::RemainderReactivateCircuit { .. }
            | Command::RemainderListCircuits
    )
}

// ---------------------------------------------------------------------------
// Command execution
// ---------------------------------------------------------------------------

async fn run_status(cli: &Cli) -> Result<()> {
    let contract_addr = parse_address(&cli.contract)?;
    let rpc_url = cli.rpc_url.parse()?;

    let provider = ProviderBuilder::new().connect_http(rpc_url);
    let contract = TEEMLVerifier::new(contract_addr, &provider);

    let owner = contract
        .owner()
        .call()
        .await
        .context("owner() call failed")?;
    let pending = contract
        .pendingOwner()
        .call()
        .await
        .context("pendingOwner() call failed")?;
    let is_paused = contract
        .paused()
        .call()
        .await
        .context("paused() call failed")?;
    let stake = contract
        .proverStake()
        .call()
        .await
        .context("proverStake() call failed")?;
    let bond = contract
        .challengeBondAmount()
        .call()
        .await
        .context("challengeBondAmount() call failed")?;
    let verifier = contract
        .remainderVerifier()
        .call()
        .await
        .context("remainderVerifier() call failed")?;

    println!("TEEMLVerifier @ {contract_addr}");
    println!("  owner:              {owner}");
    println!("  pendingOwner:       {pending}");
    println!("  paused:             {is_paused}");
    println!(
        "  proverStake:        {} wei ({} ETH)",
        stake,
        format_ether(stake)
    );
    println!(
        "  challengeBondAmount:{} wei ({} ETH)",
        bond,
        format_ether(bond)
    );
    println!("  remainderVerifier:  {verifier}");

    Ok(())
}

async fn run_remainder_status(cli: &Cli) -> Result<()> {
    let contract_addr = require_remainder_address(cli)?;
    let rpc_url = cli.rpc_url.parse()?;

    let provider = ProviderBuilder::new().connect_http(rpc_url);
    let contract = RemainderVerifier::new(contract_addr, &provider);

    let admin = contract
        .admin()
        .call()
        .await
        .context("admin() call failed")?;
    let timelock = contract
        .timelock()
        .call()
        .await
        .context("timelock() call failed")?;
    let is_paused = contract
        .paused()
        .call()
        .await
        .context("paused() call failed")?;
    let impl_addr = contract
        .implementation()
        .call()
        .await
        .context("implementation() call failed")?;

    println!("RemainderVerifier @ {contract_addr}");
    println!("  admin:          {admin}");
    println!("  timelock:       {timelock}");
    println!("  paused:         {is_paused}");
    println!("  implementation: {impl_addr}");

    Ok(())
}

async fn run_remainder_list_circuits(cli: &Cli) -> Result<()> {
    let contract_addr = require_remainder_address(cli)?;
    let rpc_url = cli.rpc_url.parse()?;

    let provider = ProviderBuilder::new().connect_http(rpc_url);
    let contract = RemainderVerifier::new(contract_addr, &provider);

    let hashes = contract
        .getCircuitHashes()
        .call()
        .await
        .context("getCircuitHashes() call failed")?;

    println!("RemainderVerifier @ {contract_addr}");
    println!("  registered circuits: {}", hashes.len());
    for (i, h) in hashes.iter().enumerate() {
        println!("  [{i}] {h}");
    }

    Ok(())
}

async fn run_remainder_write_command(cli: &Cli) -> Result<()> {
    let pk_str = require_private_key(cli)?;
    let contract_addr = require_remainder_address(cli)?;
    let rpc_url: Url = cli.rpc_url.parse()?;

    let signer: PrivateKeySigner = pk_str.parse().context("invalid private key")?;
    let sender = signer.address();

    let provider = ProviderBuilder::new()
        .wallet(alloy::network::EthereumWallet::from(signer))
        .connect_http(rpc_url);

    let contract = RemainderVerifier::new(contract_addr, &provider);

    match &cli.command {
        Command::RemainderPause => {
            println!("Pausing RemainderVerifier as {sender} ...");
            if cli.dry_run {
                let _gas = contract.pause().estimate_gas().await?;
                println!("[dry-run] estimated gas: {_gas}");
                return Ok(());
            }
            let receipt = contract.pause().send().await?.get_receipt().await?;
            println!("tx: {}", receipt.transaction_hash);
        }

        Command::RemainderUnpause => {
            println!("Unpausing RemainderVerifier as {sender} ...");
            if cli.dry_run {
                let _gas = contract.unpause().estimate_gas().await?;
                println!("[dry-run] estimated gas: {_gas}");
                return Ok(());
            }
            let receipt = contract.unpause().send().await?.get_receipt().await?;
            println!("tx: {}", receipt.transaction_hash);
        }

        Command::RemainderChangeAdmin { new_admin } => {
            let addr = parse_address(new_admin)?;
            println!("Changing RemainderVerifier admin to {addr} ...");
            if cli.dry_run {
                let _gas = contract.changeAdmin(addr).estimate_gas().await?;
                println!("[dry-run] estimated gas: {_gas}");
                return Ok(());
            }
            let receipt = contract
                .changeAdmin(addr)
                .send()
                .await?
                .get_receipt()
                .await?;
            println!("tx: {}", receipt.transaction_hash);
        }

        Command::RemainderSetTimelock { timelock_address } => {
            let addr = parse_address(timelock_address)?;
            println!("Setting RemainderVerifier timelock to {addr} ...");
            if cli.dry_run {
                let _gas = contract.setTimelock(addr).estimate_gas().await?;
                println!("[dry-run] estimated gas: {_gas}");
                return Ok(());
            }
            let receipt = contract
                .setTimelock(addr)
                .send()
                .await?
                .get_receipt()
                .await?;
            println!("tx: {}", receipt.transaction_hash);
        }

        Command::RemainderDeactivateCircuit { circuit_hash } => {
            let hash = parse_b256(circuit_hash)?;
            println!("Deactivating circuit {hash} ...");
            if cli.dry_run {
                // Try DAG variant first, fall back to legacy
                let _gas = contract.deactivateDAGCircuit(hash).estimate_gas().await?;
                println!("[dry-run] estimated gas: {_gas}");
                return Ok(());
            }
            let receipt = contract
                .deactivateDAGCircuit(hash)
                .send()
                .await?
                .get_receipt()
                .await?;
            println!("tx: {}", receipt.transaction_hash);
        }

        Command::RemainderReactivateCircuit { circuit_hash } => {
            let hash = parse_b256(circuit_hash)?;
            println!("Reactivating circuit {hash} ...");
            if cli.dry_run {
                let _gas = contract.reactivateDAGCircuit(hash).estimate_gas().await?;
                println!("[dry-run] estimated gas: {_gas}");
                return Ok(());
            }
            let receipt = contract
                .reactivateDAGCircuit(hash)
                .send()
                .await?
                .get_receipt()
                .await?;
            println!("tx: {}", receipt.transaction_hash);
        }

        _ => bail!("unexpected command in remainder write handler"),
    }

    Ok(())
}

async fn run_write_command(cli: &Cli) -> Result<()> {
    let pk_str = require_private_key(cli)?;
    let contract_addr = parse_address(&cli.contract)?;
    let rpc_url: Url = cli.rpc_url.parse()?;

    let signer: PrivateKeySigner = pk_str.parse().context("invalid private key")?;
    let sender = signer.address();

    let provider = ProviderBuilder::new()
        .wallet(alloy::network::EthereumWallet::from(signer))
        .connect_http(rpc_url);

    let contract = TEEMLVerifier::new(contract_addr, &provider);

    match &cli.command {
        Command::Pause => {
            println!("Pausing contract as {sender} ...");
            if cli.dry_run {
                let _gas = contract.pause().estimate_gas().await?;
                println!("[dry-run] estimated gas: {_gas}");
                return Ok(());
            }
            let receipt = contract.pause().send().await?.get_receipt().await?;
            println!("tx: {}", receipt.transaction_hash);
        }

        Command::Unpause => {
            println!("Unpausing contract as {sender} ...");
            if cli.dry_run {
                let _gas = contract.unpause().estimate_gas().await?;
                println!("[dry-run] estimated gas: {_gas}");
                return Ok(());
            }
            let receipt = contract.unpause().send().await?.get_receipt().await?;
            println!("tx: {}", receipt.transaction_hash);
        }

        Command::RegisterEnclave {
            address,
            image_hash,
        } => {
            let addr = parse_address(address)?;
            let hash = parse_b256(image_hash)?;
            println!("Registering enclave {addr} with image hash {hash} ...");
            if cli.dry_run {
                let _gas = contract.registerEnclave(addr, hash).estimate_gas().await?;
                println!("[dry-run] estimated gas: {_gas}");
                return Ok(());
            }
            let receipt = contract
                .registerEnclave(addr, hash)
                .send()
                .await?
                .get_receipt()
                .await?;
            println!("tx: {}", receipt.transaction_hash);
        }

        Command::RevokeEnclave { address } => {
            let addr = parse_address(address)?;
            println!("Revoking enclave {addr} ...");
            if cli.dry_run {
                let _gas = contract.revokeEnclave(addr).estimate_gas().await?;
                println!("[dry-run] estimated gas: {_gas}");
                return Ok(());
            }
            let receipt = contract
                .revokeEnclave(addr)
                .send()
                .await?
                .get_receipt()
                .await?;
            println!("tx: {}", receipt.transaction_hash);
        }

        Command::SetStake { amount } => {
            let wei = parse_ether(amount)?;
            println!("Setting prover stake to {amount} ETH ({wei} wei) ...");
            if cli.dry_run {
                let _gas = contract.setProverStake(wei).estimate_gas().await?;
                println!("[dry-run] estimated gas: {_gas}");
                return Ok(());
            }
            let receipt = contract
                .setProverStake(wei)
                .send()
                .await?
                .get_receipt()
                .await?;
            println!("tx: {}", receipt.transaction_hash);
        }

        Command::SetBond { amount } => {
            let wei = parse_ether(amount)?;
            println!("Setting challenge bond to {amount} ETH ({wei} wei) ...");
            if cli.dry_run {
                let _gas = contract.setChallengeBondAmount(wei).estimate_gas().await?;
                println!("[dry-run] estimated gas: {_gas}");
                return Ok(());
            }
            let receipt = contract
                .setChallengeBondAmount(wei)
                .send()
                .await?
                .get_receipt()
                .await?;
            println!("tx: {}", receipt.transaction_hash);
        }

        Command::SetVerifier { address } => {
            let addr = parse_address(address)?;
            println!("Setting remainder verifier to {addr} ...");
            if cli.dry_run {
                let _gas = contract.setRemainderVerifier(addr).estimate_gas().await?;
                println!("[dry-run] estimated gas: {_gas}");
                return Ok(());
            }
            let receipt = contract
                .setRemainderVerifier(addr)
                .send()
                .await?
                .get_receipt()
                .await?;
            println!("tx: {}", receipt.transaction_hash);
        }

        Command::TransferOwnership { address } => {
            let addr = parse_address(address)?;
            println!("Initiating ownership transfer to {addr} ...");
            if cli.dry_run {
                let _gas = contract.transferOwnership(addr).estimate_gas().await?;
                println!("[dry-run] estimated gas: {_gas}");
                return Ok(());
            }
            let receipt = contract
                .transferOwnership(addr)
                .send()
                .await?
                .get_receipt()
                .await?;
            println!("tx: {}", receipt.transaction_hash);
        }

        Command::AcceptOwnership => {
            println!("Accepting ownership as {sender} ...");
            if cli.dry_run {
                let _gas = contract.acceptOwnership().estimate_gas().await?;
                println!("[dry-run] estimated gas: {_gas}");
                return Ok(());
            }
            let receipt = contract
                .acceptOwnership()
                .send()
                .await?
                .get_receipt()
                .await?;
            println!("tx: {}", receipt.transaction_hash);
        }

        Command::Status => {
            bail!("status is handled separately");
        }

        _ => {
            bail!("command is handled separately");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Command::Status => run_status(&cli).await,
        Command::RemainderStatus => run_remainder_status(&cli).await,
        Command::RemainderListCircuits => run_remainder_list_circuits(&cli).await,
        cmd if is_remainder_command(cmd) => run_remainder_write_command(&cli).await,
        _ => run_write_command(&cli).await,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn try_parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(args)
    }

    #[test]
    fn test_status_command() {
        let cli = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x1234567890abcdef1234567890abcdef12345678",
            "status",
        ])
        .unwrap();

        assert_eq!(cli.rpc_url, "http://localhost:8545");
        assert_eq!(cli.contract, "0x1234567890abcdef1234567890abcdef12345678");
        assert!(cli.private_key.is_none());
        assert!(!cli.dry_run);
        assert!(matches!(cli.command, Command::Status));
    }

    #[test]
    fn test_pause_command() {
        let cli = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x1234567890abcdef1234567890abcdef12345678",
            "--private-key",
            "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
            "pause",
        ])
        .unwrap();

        assert!(cli.private_key.is_some());
        assert!(matches!(cli.command, Command::Pause));
    }

    #[test]
    fn test_unpause_command() {
        let cli = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x0000000000000000000000000000000000000001",
            "--private-key",
            "0xdeadbeef",
            "unpause",
        ])
        .unwrap();

        assert!(matches!(cli.command, Command::Unpause));
    }

    #[test]
    fn test_register_enclave_command() {
        let cli = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x1234567890abcdef1234567890abcdef12345678",
            "--private-key",
            "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
            "register-enclave",
            "0xABCDABCDABCDABCDABCDABCDABCDABCDABCDABCD",
            "0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ])
        .unwrap();

        match &cli.command {
            Command::RegisterEnclave {
                address,
                image_hash,
            } => {
                assert_eq!(address, "0xABCDABCDABCDABCDABCDABCDABCDABCDABCDABCD");
                assert_eq!(
                    image_hash,
                    "0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                );
            }
            _ => panic!("expected RegisterEnclave"),
        }
    }

    #[test]
    fn test_revoke_enclave_command() {
        let cli = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x0000000000000000000000000000000000000001",
            "--private-key",
            "0xaa",
            "revoke-enclave",
            "0xABCDABCDABCDABCDABCDABCDABCDABCDABCDABCD",
        ])
        .unwrap();

        match &cli.command {
            Command::RevokeEnclave { address } => {
                assert_eq!(address, "0xABCDABCDABCDABCDABCDABCDABCDABCDABCDABCD");
            }
            _ => panic!("expected RevokeEnclave"),
        }
    }

    #[test]
    fn test_set_stake_command() {
        let cli = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x0000000000000000000000000000000000000001",
            "--private-key",
            "0xaa",
            "set-stake",
            "0.01",
        ])
        .unwrap();

        match &cli.command {
            Command::SetStake { amount } => {
                assert_eq!(amount, "0.01");
            }
            _ => panic!("expected SetStake"),
        }
    }

    #[test]
    fn test_set_bond_command() {
        let cli = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x0000000000000000000000000000000000000001",
            "--private-key",
            "0xaa",
            "set-bond",
            "0.005",
        ])
        .unwrap();

        match &cli.command {
            Command::SetBond { amount } => {
                assert_eq!(amount, "0.005");
            }
            _ => panic!("expected SetBond"),
        }
    }

    #[test]
    fn test_set_verifier_command() {
        let cli = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x0000000000000000000000000000000000000001",
            "--private-key",
            "0xaa",
            "set-verifier",
            "0x9999999999999999999999999999999999999999",
        ])
        .unwrap();

        match &cli.command {
            Command::SetVerifier { address } => {
                assert_eq!(address, "0x9999999999999999999999999999999999999999");
            }
            _ => panic!("expected SetVerifier"),
        }
    }

    #[test]
    fn test_transfer_ownership_command() {
        let cli = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x0000000000000000000000000000000000000001",
            "--private-key",
            "0xaa",
            "transfer-ownership",
            "0xBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB",
        ])
        .unwrap();

        match &cli.command {
            Command::TransferOwnership { address } => {
                assert_eq!(address, "0xBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB");
            }
            _ => panic!("expected TransferOwnership"),
        }
    }

    #[test]
    fn test_accept_ownership_command() {
        let cli = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x0000000000000000000000000000000000000001",
            "--private-key",
            "0xaa",
            "accept-ownership",
        ])
        .unwrap();

        assert!(matches!(cli.command, Command::AcceptOwnership));
    }

    #[test]
    fn test_dry_run_flag() {
        let cli = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x0000000000000000000000000000000000000001",
            "--private-key",
            "0xaa",
            "--dry-run",
            "pause",
        ])
        .unwrap();

        assert!(cli.dry_run);
        assert!(matches!(cli.command, Command::Pause));
    }

    #[test]
    fn test_missing_rpc_url_fails() {
        let result = try_parse(&[
            "admin-cli",
            "--contract",
            "0x0000000000000000000000000000000000000001",
            "status",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn test_missing_contract_fails() {
        let result = try_parse(&["admin-cli", "--rpc-url", "http://localhost:8545", "status"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_missing_subcommand_fails() {
        let result = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x0000000000000000000000000000000000000001",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_ether_helper() {
        let wei = parse_ether("1.0").unwrap();
        assert_eq!(wei, U256::from(1_000_000_000_000_000_000u64));

        let wei = parse_ether("0.01").unwrap();
        assert_eq!(wei, U256::from(10_000_000_000_000_000u64));

        let wei = parse_ether("0").unwrap();
        assert_eq!(wei, U256::ZERO);
    }

    #[test]
    fn test_parse_address_helper() {
        let addr = parse_address("0x1234567890abcdef1234567890abcdef12345678").unwrap();
        assert_eq!(
            format!("{addr}"),
            "0x1234567890AbcdEF1234567890aBcdef12345678"
        );

        assert!(parse_address("not-an-address").is_err());
    }

    #[test]
    fn test_parse_b256_helper() {
        let hash = parse_b256("0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
            .unwrap();
        assert_eq!(
            format!("{hash}"),
            "0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );

        assert!(parse_b256("0xshort").is_err());
    }

    #[test]
    fn test_format_ether_helper() {
        let s = format_ether(U256::from(1_000_000_000_000_000_000u64));
        assert_eq!(s, "1.000000000000000000");

        let s = format_ether(U256::from(10_000_000_000_000_000u64));
        assert_eq!(s, "0.010000000000000000");
    }

    #[test]
    fn test_require_private_key_missing() {
        let cli = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x0000000000000000000000000000000000000001",
            "pause",
        ])
        .unwrap();

        let result = require_private_key(&cli);
        assert!(result.is_err());
    }

    #[test]
    fn test_require_private_key_present() {
        let cli = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x0000000000000000000000000000000000000001",
            "--private-key",
            "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
            "pause",
        ])
        .unwrap();

        let pk = require_private_key(&cli).unwrap();
        assert!(pk.starts_with("0x"));
    }

    // ----- RemainderVerifier command tests -----

    #[test]
    fn test_remainder_status_command() {
        let cli = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x0000000000000000000000000000000000000001",
            "--remainder-address",
            "0x0000000000000000000000000000000000000002",
            "remainder-status",
        ])
        .unwrap();

        assert!(matches!(cli.command, Command::RemainderStatus));
        assert_eq!(
            cli.remainder_address.as_deref(),
            Some("0x0000000000000000000000000000000000000002")
        );
    }

    #[test]
    fn test_remainder_pause_command() {
        let cli = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x0000000000000000000000000000000000000001",
            "--remainder-address",
            "0x0000000000000000000000000000000000000002",
            "--private-key",
            "0xaa",
            "remainder-pause",
        ])
        .unwrap();

        assert!(matches!(cli.command, Command::RemainderPause));
    }

    #[test]
    fn test_remainder_unpause_command() {
        let cli = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x0000000000000000000000000000000000000001",
            "--remainder-address",
            "0x0000000000000000000000000000000000000002",
            "--private-key",
            "0xaa",
            "remainder-unpause",
        ])
        .unwrap();

        assert!(matches!(cli.command, Command::RemainderUnpause));
    }

    #[test]
    fn test_remainder_change_admin_command() {
        let cli = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x0000000000000000000000000000000000000001",
            "--remainder-address",
            "0x0000000000000000000000000000000000000002",
            "--private-key",
            "0xaa",
            "remainder-change-admin",
            "0xBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB",
        ])
        .unwrap();

        match &cli.command {
            Command::RemainderChangeAdmin { new_admin } => {
                assert_eq!(new_admin, "0xBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB");
            }
            _ => panic!("expected RemainderChangeAdmin"),
        }
    }

    #[test]
    fn test_remainder_set_timelock_command() {
        let cli = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x0000000000000000000000000000000000000001",
            "--remainder-address",
            "0x0000000000000000000000000000000000000002",
            "--private-key",
            "0xaa",
            "remainder-set-timelock",
            "0xCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC",
        ])
        .unwrap();

        match &cli.command {
            Command::RemainderSetTimelock { timelock_address } => {
                assert_eq!(
                    timelock_address,
                    "0xCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC"
                );
            }
            _ => panic!("expected RemainderSetTimelock"),
        }
    }

    #[test]
    fn test_remainder_deactivate_circuit_command() {
        let cli = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x0000000000000000000000000000000000000001",
            "--remainder-address",
            "0x0000000000000000000000000000000000000002",
            "--private-key",
            "0xaa",
            "remainder-deactivate-circuit",
            "0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ])
        .unwrap();

        match &cli.command {
            Command::RemainderDeactivateCircuit { circuit_hash } => {
                assert_eq!(
                    circuit_hash,
                    "0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                );
            }
            _ => panic!("expected RemainderDeactivateCircuit"),
        }
    }

    #[test]
    fn test_remainder_reactivate_circuit_command() {
        let cli = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x0000000000000000000000000000000000000001",
            "--remainder-address",
            "0x0000000000000000000000000000000000000002",
            "--private-key",
            "0xaa",
            "remainder-reactivate-circuit",
            "0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ])
        .unwrap();

        match &cli.command {
            Command::RemainderReactivateCircuit { circuit_hash } => {
                assert_eq!(
                    circuit_hash,
                    "0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                );
            }
            _ => panic!("expected RemainderReactivateCircuit"),
        }
    }

    #[test]
    fn test_remainder_list_circuits_command() {
        let cli = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x0000000000000000000000000000000000000001",
            "--remainder-address",
            "0x0000000000000000000000000000000000000002",
            "remainder-list-circuits",
        ])
        .unwrap();

        assert!(matches!(cli.command, Command::RemainderListCircuits));
    }

    #[test]
    fn test_remainder_address_optional_for_tee_commands() {
        // remainder-address should NOT be required for TEE commands
        let cli = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x0000000000000000000000000000000000000001",
            "status",
        ])
        .unwrap();

        assert!(cli.remainder_address.is_none());
        assert!(matches!(cli.command, Command::Status));
    }

    #[test]
    fn test_require_remainder_address_missing() {
        let cli = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x0000000000000000000000000000000000000001",
            "remainder-status",
        ])
        .unwrap();

        let result = require_remainder_address(&cli);
        assert!(result.is_err());
    }

    #[test]
    fn test_require_remainder_address_present() {
        let cli = try_parse(&[
            "admin-cli",
            "--rpc-url",
            "http://localhost:8545",
            "--contract",
            "0x0000000000000000000000000000000000000001",
            "--remainder-address",
            "0x0000000000000000000000000000000000000002",
            "remainder-status",
        ])
        .unwrap();

        let addr = require_remainder_address(&cli).unwrap();
        assert_eq!(
            format!("{addr}"),
            "0x0000000000000000000000000000000000000002"
        );
    }

    #[test]
    fn test_is_remainder_command() {
        assert!(is_remainder_command(&Command::RemainderStatus));
        assert!(is_remainder_command(&Command::RemainderPause));
        assert!(is_remainder_command(&Command::RemainderUnpause));
        assert!(is_remainder_command(&Command::RemainderChangeAdmin {
            new_admin: "0x00".to_string(),
        }));
        assert!(is_remainder_command(&Command::RemainderSetTimelock {
            timelock_address: "0x00".to_string(),
        }));
        assert!(is_remainder_command(&Command::RemainderDeactivateCircuit {
            circuit_hash: "0x00".to_string(),
        }));
        assert!(is_remainder_command(&Command::RemainderReactivateCircuit {
            circuit_hash: "0x00".to_string(),
        }));
        assert!(is_remainder_command(&Command::RemainderListCircuits));

        // TEE commands should NOT be remainder commands
        assert!(!is_remainder_command(&Command::Status));
        assert!(!is_remainder_command(&Command::Pause));
        assert!(!is_remainder_command(&Command::Unpause));
        assert!(!is_remainder_command(&Command::AcceptOwnership));
    }
}
