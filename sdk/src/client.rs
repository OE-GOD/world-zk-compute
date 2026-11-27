//! Ethereum client wrapper combining an RPC provider, wallet, and contract address.

use std::fmt;

use alloy::network::{Ethereum, EthereumWallet};
use alloy::primitives::{Address, B256};
use alloy::providers::PendingTransactionBuilder;
use alloy::rpc::types::TransactionReceipt;
use alloy::signers::local::PrivateKeySigner;
use url::Url;

/// SDK client wrapping an alloy provider, wallet, and contract address.
pub struct Client {
    pub(crate) rpc_url: Url,
    pub(crate) wallet: EthereumWallet,
    contract_address: Address,
    signer_address: Address,
}

impl Client {
    /// Create a new client from RPC URL, hex-encoded private key, and contract address.
    pub fn new(rpc_url: &str, private_key: &str, contract_address: &str) -> anyhow::Result<Self> {
        let signer: PrivateKeySigner = private_key.parse()?;
        let signer_address = signer.address();
        let wallet = EthereumWallet::from(signer);
        let url: Url = rpc_url.parse()?;
        Ok(Self {
            rpc_url: url,
            wallet,
            contract_address: contract_address.parse()?,
            signer_address,
        })
    }

    /// Returns the contract address.
    pub fn contract_address(&self) -> Address {
        self.contract_address
    }

    /// Returns the signer (sender) address.
    pub fn signer_address(&self) -> Address {
        self.signer_address
    }
}

/// A transaction was mined but reverted.
///
/// A transaction can pass gas estimation and still revert when it is executed
/// (the state changed in between, or it ran out of gas). Its receipt then has
/// status 0, and nothing it was meant to do happened.
/// [`is_retryable`](crate::retry::is_retryable) is false for this error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionReverted {
    /// Hash of the reverted transaction.
    pub transaction_hash: B256,
    /// Block the transaction was mined in (`None` if the receipt has none).
    pub block_number: Option<u64>,
    /// Gas the transaction used before it reverted.
    pub gas_used: u64,
}

impl fmt::Display for TransactionReverted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "transaction {} was mined in block {} but reverted (gas used: {})",
            self.transaction_hash,
            self.block_number
                .map_or_else(|| "?".to_string(), |n| n.to_string()),
            self.gas_used
        )
    }
}

impl std::error::Error for TransactionReverted {}

/// Turn the receipt of a transaction that was mined but reverted into a
/// [`TransactionReverted`] error.
pub(crate) fn ensure_success(receipt: TransactionReceipt) -> anyhow::Result<TransactionReceipt> {
    if !receipt.status() {
        return Err(TransactionReverted {
            transaction_hash: receipt.transaction_hash,
            block_number: receipt.block_number,
            gas_used: receipt.gas_used,
        }
        .into());
    }
    Ok(receipt)
}

/// Waiting for a sent transaction the way every SDK method needs it.
pub(crate) trait PendingTransactionExt {
    /// Wait until the transaction is mined; fail if it reverted.
    async fn get_successful_receipt(self) -> anyhow::Result<TransactionReceipt>;
}

impl PendingTransactionExt for PendingTransactionBuilder<Ethereum> {
    async fn get_successful_receipt(self) -> anyhow::Result<TransactionReceipt> {
        ensure_success(self.get_receipt().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mined receipt as `eth_getTransactionReceipt` returns it.
    fn receipt_with_status(status: &str) -> TransactionReceipt {
        serde_json::from_value(serde_json::json!({
            "transactionHash": "0x41b145656bb7bd48f6e041176f1837d6faf3ebb454ae7394aa9c0246b36d55ee",
            "transactionIndex": "0x1",
            "blockHash": "0x1111111111111111111111111111111111111111111111111111111111111111",
            "blockNumber": "0x2a",
            "from": "0x3c44cdddb6a900fa2b585dd299e03d12fa4293bc",
            "to": "0xcf7ed3acca5a467e9e704c703e8d87f634fb0fc9",
            "cumulativeGasUsed": "0xc350",
            "gasUsed": "0x61a8",
            "effectiveGasPrice": "0x3b9aca00",
            "contractAddress": null,
            "logs": [],
            "logsBloom": format!("0x{}", "00".repeat(256)),
            "status": status,
            "type": "0x2"
        }))
        .expect("valid receipt JSON")
    }

    #[test]
    fn ensure_success_passes_successful_receipt() {
        let receipt = ensure_success(receipt_with_status("0x1")).expect("status 1 is a success");
        assert_eq!(receipt.block_number, Some(42));
    }

    #[test]
    fn ensure_success_rejects_reverted_receipt() {
        let err = ensure_success(receipt_with_status("0x0")).expect_err("status 0 is a revert");
        let msg = err.to_string();
        assert!(msg.contains("reverted"), "{msg}");
        assert!(
            msg.contains("0x41b145656bb7bd48f6e041176f1837d6faf3ebb454ae7394aa9c0246b36d55ee"),
            "{msg}"
        );
        assert!(msg.contains("block 42"), "{msg}");
        assert_eq!(
            err.downcast_ref::<TransactionReverted>(),
            Some(&TransactionReverted {
                transaction_hash:
                    "0x41b145656bb7bd48f6e041176f1837d6faf3ebb454ae7394aa9c0246b36d55ee"
                        .parse()
                        .unwrap(),
                block_number: Some(42),
                gas_used: 25_000,
            })
        );
    }
}
