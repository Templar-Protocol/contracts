//! EVM chain handler for Ethereum and compatible chains

use async_trait::async_trait;
use tracing::{error, info};

use super::{config::EvmChainConfig, ExternalChainError, ExternalChainHandler, TransferResult};

/// EVM chain handler for Ethereum-compatible chains
pub struct EvmChainHandler {
    config: EvmChainConfig,
    #[allow(dead_code)] // Used when ethereum feature is enabled
    private_key: String,
}

impl EvmChainHandler {
    /// Create a new EVM chain handler
    pub fn new(config: EvmChainConfig, private_key: String) -> Self {
        Self {
            config,
            private_key,
        }
    }

    /// Parse human-readable amount to smallest units
    #[allow(dead_code)] // Used when ethereum feature is enabled
    fn parse_amount(&self, amount_str: &str, decimals: u8) -> Result<u128, String> {
        let parts: Vec<&str> = amount_str.split('.').collect();
        let (whole, frac) = match parts.len() {
            1 => (parts[0], ""),
            2 => (parts[0], parts[1]),
            _ => return Err("Invalid amount format".to_string()),
        };

        let whole_num: u128 = whole
            .parse()
            .map_err(|e| format!("Invalid whole part: {}", e))?;

        let frac_padded = format!("{:0<width$}", frac, width = decimals as usize);
        let frac_trimmed = &frac_padded[..decimals as usize];
        let frac_num: u128 = frac_trimmed
            .parse()
            .map_err(|e| format!("Invalid fractional part: {}", e))?;

        let multiplier = 10u128.pow(decimals as u32);
        whole_num
            .checked_mul(multiplier)
            .ok_or_else(|| "Overflow".to_string())?
            .checked_add(frac_num)
            .ok_or_else(|| "Overflow".to_string())
    }
}

#[async_trait]
impl ExternalChainHandler for EvmChainHandler {
    fn chain_id(&self) -> &str {
        &self.config.chain_id
    }

    fn supports_token(&self, asset: &str) -> bool {
        self.config.get_token_address(asset).is_some()
    }

    async fn transfer_tokens(
        &self,
        to_address: &str,
        asset: &str,
        amount: &str,
        _memo: Option<&str>,
    ) -> Result<TransferResult, ExternalChainError> {
        // EVM chains don't use memos, ignore the parameter
        use ethers::{
            middleware::SignerMiddleware,
            providers::{Http, Middleware, Provider},
            signers::{LocalWallet, Signer},
            types::{Address, TransactionRequest, U256},
        };
        use std::str::FromStr;
        use std::sync::Arc;

        info!(
            chain = %self.config.name,
            chain_id = %self.config.chain_id,
            to = %to_address,
            asset = %asset,
            amount = %amount,
            "Initiating EVM transfer"
        );

        // Get token address
        let token_addr_str = self.config.get_token_address(asset).ok_or_else(|| {
            ExternalChainError::TokenNotSupported {
                asset: asset.to_string(),
                chain: self.config.chain_id.clone(),
            }
        })?;

        let token_address = Address::from_str(token_addr_str)
            .map_err(|e| ExternalChainError::InvalidAddress(format!("Token address: {}", e)))?;

        // Parse destination address
        let to_addr = Address::from_str(to_address)
            .map_err(|e| ExternalChainError::InvalidAddress(e.to_string()))?;

        // Parse amount
        let decimals = self.config.get_token_decimals(asset);
        let amount_raw = self
            .parse_amount(amount, decimals)
            .map_err(ExternalChainError::InvalidAmount)?;
        let amount_u256 = U256::from(amount_raw);

        // Parse private key
        let key_hex = self.private_key.trim_start_matches("0x");
        let wallet: LocalWallet = key_hex
            .parse()
            .map_err(|e| ExternalChainError::InvalidPrivateKey(format!("{}", e)))?;

        // Create provider
        let provider = Provider::<Http>::try_from(&self.config.rpc_url)
            .map_err(|e| ExternalChainError::RpcConnectionFailed(format!("{}", e)))?;

        // Get chain ID from provider to verify we're on the right chain
        let chain_id = provider.get_chainid().await.map_err(|e| {
            ExternalChainError::RpcConnectionFailed(format!("Failed to get chain ID: {}", e))
        })?;

        if chain_id.as_u64() != self.config.native_chain_id {
            return Err(ExternalChainError::RpcConnectionFailed(format!(
                "Chain ID mismatch: expected {}, got {}",
                self.config.native_chain_id,
                chain_id.as_u64()
            )));
        }

        let wallet = wallet.with_chain_id(chain_id.as_u64());
        let client = Arc::new(SignerMiddleware::new(provider, wallet));

        info!(
            token = %token_address,
            to = %to_addr,
            amount = %amount_u256,
            "Preparing ERC-20 transfer"
        );

        // Build ERC-20 transfer call data
        // transfer(address to, uint256 amount) = 0xa9059cbb
        let mut call_data = vec![0xa9, 0x05, 0x9c, 0xbb];
        // Pad address to 32 bytes
        call_data.extend_from_slice(&[0u8; 12]);
        call_data.extend_from_slice(to_addr.as_bytes());
        // Pad amount to 32 bytes
        let mut amount_bytes = [0u8; 32];
        amount_u256.to_big_endian(&mut amount_bytes);
        call_data.extend_from_slice(&amount_bytes);

        // Create transaction
        let tx = TransactionRequest::new().to(token_address).data(call_data);

        // Send transaction
        let client_clone = client.clone();
        drop(client);
        let pending_tx = client_clone.send_transaction(tx, None).await.map_err(|e| {
            error!(error = %e, "Failed to send transaction");
            ExternalChainError::TransactionFailed(format!("Failed to send: {}", e))
        })?;

        let tx_hash = format!("{:?}", pending_tx.tx_hash());
        info!(tx_hash = %tx_hash, "Transaction sent, waiting for confirmation");

        // Wait for confirmation
        match pending_tx.confirmations(1).await {
            Ok(Some(receipt)) => {
                let final_hash = format!("{:?}", receipt.transaction_hash);
                info!(tx_hash = %final_hash, "Transaction confirmed");
                Ok(TransferResult {
                    tx_hash: final_hash,
                    confirmed: true,
                })
            }
            Ok(None) => {
                info!(tx_hash = %tx_hash, "Transaction pending");
                Ok(TransferResult {
                    tx_hash,
                    confirmed: false,
                })
            }
            Err(e) => {
                error!(error = %e, tx_hash = %tx_hash, "Transaction failed");
                Err(ExternalChainError::TransactionFailed(format!(
                    "Confirmation failed: {}",
                    e
                )))
            }
        }
    }
}
