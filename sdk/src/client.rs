//! Ethereum client wrapper combining an RPC provider, wallet, and contract address.

use alloy::network::EthereumWallet;
use alloy::primitives::Address;
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
