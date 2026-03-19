//! Native Solana SDK-based SPL token transfer handler
//!
//! Uses Solana SDK v3.0+ with SPL token crates for native SPL token transfers.
//! No dependency conflicts with NEAR SDK.

use super::{ExternalChainError, ExternalChainHandler, TransferResult};
use async_trait::async_trait;
use solana_rpc_client::rpc_client::RpcClient;
// Use the Pubkey from solana-program (re-exported by SPL crates)
use spl_associated_token_account::solana_program::{
    hash::Hash, instruction::Instruction, message::Message, pubkey::Pubkey,
};
use spl_associated_token_account::{
    get_associated_token_address, instruction::create_associated_token_account,
};
use spl_token::instruction as token_instruction;
use std::collections::HashMap;
use std::sync::Arc;
use templar_common::config::env::read_env_value;
use tracing::{error, info};

/// Solana chain configuration for SDK-based transfers
#[derive(Debug, Clone)]
pub struct SolanaSdkConfig {
    /// Chain identifier (e.g., "sol:mainnet", "sol:devnet")
    pub chain_id: String,
    /// Human-readable name
    pub name: String,
    /// RPC URL
    pub rpc_url: String,
    /// Token mint addresses (asset symbol -> mint address)
    pub token_mints: HashMap<String, String>,
    /// Token decimals
    pub token_decimals: HashMap<String, u8>,
}

impl SolanaSdkConfig {
    /// Create mainnet beta configuration
    pub fn mainnet() -> Self {
        let mut token_mints = HashMap::new();
        let mut token_decimals = HashMap::new();

        // USDC on Solana
        token_mints.insert(
            "USDC".to_string(),
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
        );
        token_decimals.insert("USDC".to_string(), 6);

        // USDT on Solana
        token_mints.insert(
            "USDT".to_string(),
            "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB".to_string(),
        );
        token_decimals.insert("USDT".to_string(), 6);

        // Wrapped SOL
        token_mints.insert(
            "WSOL".to_string(),
            "So11111111111111111111111111111111111111112".to_string(),
        );
        token_decimals.insert("WSOL".to_string(), 9);

        Self {
            chain_id: "sol:mainnet".to_string(),
            name: "Solana Mainnet".to_string(),
            rpc_url: "https://api.mainnet-beta.solana.com".to_string(),
            token_mints,
            token_decimals,
        }
    }

    /// Create devnet configuration
    pub fn devnet() -> Self {
        let mut token_mints = HashMap::new();
        let mut token_decimals = HashMap::new();

        // Test USDC on devnet
        token_mints.insert(
            "USDC".to_string(),
            "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU".to_string(),
        );
        token_decimals.insert("USDC".to_string(), 6);

        Self {
            chain_id: "sol:devnet".to_string(),
            name: "Solana Devnet".to_string(),
            rpc_url: "https://api.devnet.solana.com".to_string(),
            token_mints,
            token_decimals,
        }
    }
}

/// 64-byte Solana keypair (32 private + 32 public)
struct SolanaKeypair {
    secret_key: [u8; 32],
    public_key: Pubkey,
}

impl SolanaKeypair {
    /// Create from 64-byte array
    fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != 64 {
            return Err(format!("Expected 64 bytes, got {}", bytes.len()));
        }

        let mut secret_key = [0u8; 32];
        secret_key.copy_from_slice(&bytes[..32]);

        let mut public_key_bytes = [0u8; 32];
        public_key_bytes.copy_from_slice(&bytes[32..]);

        let public_key = Pubkey::new_from_array(public_key_bytes);

        Ok(Self {
            secret_key,
            public_key,
        })
    }

    /// Get the public key
    fn pubkey(&self) -> &Pubkey {
        &self.public_key
    }

    /// Sign a message using ed25519
    fn sign(&self, message: &[u8]) -> [u8; 64] {
        use ed25519_dalek::{Signer, SigningKey};

        let signing_key = SigningKey::from_bytes(&self.secret_key);
        let signature = signing_key.sign(message);
        signature.to_bytes()
    }
}

/// Native SDK-based Solana chain handler
pub struct SolanaSdkHandler {
    config: SolanaSdkConfig,
    /// Signer keypair
    keypair: Arc<SolanaKeypair>,
    /// RPC client
    client: Arc<RpcClient>,
}

impl SolanaSdkHandler {
    /// Create a new handler from keypair bytes
    pub fn new(config: SolanaSdkConfig, keypair_bytes: &[u8]) -> Result<Self, String> {
        let keypair = SolanaKeypair::from_bytes(keypair_bytes)?;

        // Use simple RPC client - commitment config not needed for basic operations
        let client = RpcClient::new(config.rpc_url.clone());

        Ok(Self {
            config,
            keypair: Arc::new(keypair),
            client: Arc::new(client),
        })
    }

    /// Create handler from base58-encoded keypair
    pub fn from_base58(config: SolanaSdkConfig, keypair_base58: &str) -> Result<Self, String> {
        let bytes = bs58::decode(keypair_base58)
            .into_vec()
            .map_err(|e| format!("Invalid base58 keypair: {e}"))?;

        Self::new(config, &bytes)
    }

    /// Create handler from JSON keypair file content (array of u8)
    pub fn from_json_bytes(config: SolanaSdkConfig, json_bytes: &str) -> Result<Self, String> {
        let bytes: Vec<u8> =
            serde_json::from_str(json_bytes).map_err(|e| format!("Invalid JSON keypair: {e}"))?;

        Self::new(config, &bytes)
    }

    /// Get the public key as base58 string
    pub fn public_key(&self) -> String {
        self.keypair.pubkey().to_string()
    }

    /// Execute SPL token transfer
    async fn transfer_spl(
        &self,
        mint_address: &str,
        recipient: &str,
        amount: u64,
        decimals: u8,
    ) -> Result<String, ExternalChainError> {
        let mint_pubkey: Pubkey = mint_address.parse().map_err(|e| {
            ExternalChainError::InvalidAddress(format!("Invalid mint address: {e}"))
        })?;

        let recipient_pubkey: Pubkey = recipient.parse().map_err(|e| {
            ExternalChainError::InvalidAddress(format!("Invalid recipient address: {e}"))
        })?;

        let source_pubkey = *self.keypair.pubkey();

        let source_ata = get_associated_token_address(&source_pubkey, &mint_pubkey);
        let dest_ata = get_associated_token_address(&recipient_pubkey, &mint_pubkey);

        info!(
            source_ata = %source_ata,
            dest_ata = %dest_ata,
            "Derived associated token accounts"
        );

        // Convert Pubkey to Address for RPC client
        let dest_ata_address: solana_sdk::pubkey::Pubkey = dest_ata.to_string().parse().unwrap();

        let dest_account = self.client.get_account(&dest_ata_address);

        let mut instructions: Vec<Instruction> = vec![];

        if dest_account.is_err() {
            info!("Creating destination associated token account");
            instructions.push(create_associated_token_account(
                &source_pubkey,    // payer
                &recipient_pubkey, // wallet
                &mint_pubkey,      // mint
                &spl_token::id(),  // token program
            ));
        }

        let transfer_ix = token_instruction::transfer_checked(
            &spl_token::id(),
            &source_ata,
            &mint_pubkey,
            &dest_ata,
            &source_pubkey,
            &[],
            amount,
            decimals,
        )
        .map_err(|e| {
            ExternalChainError::TransactionFailed(format!("Failed to create instruction: {e}"))
        })?;

        instructions.push(transfer_ix);

        let blockhash_response = self.client.get_latest_blockhash().map_err(|e| {
            ExternalChainError::RpcConnectionFailed(format!("Failed to get blockhash: {e}"))
        })?;

        // Convert to solana-program Hash type
        let blockhash = Hash::new_from_array(blockhash_response.to_bytes());

        let message = Message::new_with_blockhash(&instructions, Some(&source_pubkey), &blockhash);

        let message_data = message.serialize();

        let signature = self.keypair.sign(&message_data);

        // Build transaction bytes manually
        // Transaction format: [num_signatures][signature][message]
        let mut tx_bytes = vec![];

        // Compact-u16 for number of signatures (1)
        tx_bytes.push(1u8);

        tx_bytes.extend_from_slice(&signature);

        tx_bytes.extend_from_slice(&message_data);

        // Send raw transaction via JSON-RPC
        let tx_base58 = bs58::encode(&tx_bytes).into_string();

        let rpc_request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendTransaction",
            "params": [
                tx_base58,
                {
                    "encoding": "base58",
                    "preflightCommitment": "confirmed"
                }
            ]
        });

        let http_client = reqwest::Client::new();
        let response = http_client
            .post(&self.config.rpc_url)
            .json(&rpc_request)
            .send()
            .await
            .map_err(|e| {
                error!(error = %e, "RPC request failed");
                ExternalChainError::RpcConnectionFailed(format!("RPC request failed: {e}"))
            })?;

        let json_response: serde_json::Value = response.json().await.map_err(|e| {
            ExternalChainError::RpcConnectionFailed(format!("Failed to parse response: {e}"))
        })?;

        if let Some(error) = json_response.get("error") {
            let error_msg = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error");
            error!(error = %error_msg, "Transaction failed");
            return Err(ExternalChainError::TransactionFailed(format!(
                "Transaction failed: {error_msg}"
            )));
        }

        let signature = json_response
            .get("result")
            .and_then(|r| r.as_str())
            .ok_or_else(|| {
                ExternalChainError::TransactionFailed("Missing signature in response".to_string())
            })?;

        info!(signature = %signature, "Transaction submitted");

        Ok(signature.to_string())
    }
}

#[async_trait]
impl ExternalChainHandler for SolanaSdkHandler {
    fn chain_id(&self) -> &str {
        &self.config.chain_id
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn supports_token(&self, asset: &str) -> bool {
        self.config.token_mints.contains_key(asset)
    }

    async fn transfer_tokens(
        &self,
        to_address: &str,
        asset: &str,
        amount: &str,
        _memo: Option<&str>,
    ) -> Result<TransferResult, ExternalChainError> {
        // Solana uses unique addresses, not memos
        info!(
            chain = %self.config.chain_id,
            to = %to_address,
            asset = %asset,
            amount = %amount,
            "Initiating Solana SDK SPL token transfer"
        );

        // Verify token is supported
        let mint = self.config.token_mints.get(asset).ok_or_else(|| {
            ExternalChainError::TokenNotSupported {
                asset: asset.to_string(),
                chain: self.config.chain_id.clone(),
            }
        })?;

        let decimals = self.config.token_decimals.get(asset).copied().unwrap_or(6);

        // Parse amount
        let amount_float: f64 = amount
            .parse()
            .map_err(|e| ExternalChainError::InvalidAmount(format!("Invalid amount: {e}")))?;

        if amount_float <= 0.0 {
            return Err(ExternalChainError::InvalidAmount(
                "Amount must be positive".to_string(),
            ));
        }

        let amount_raw = (amount_float * 10f64.powi(i32::from(decimals))) as u64;

        info!(
            mint = %mint,
            decimals = %decimals,
            amount_raw = %amount_raw,
            "Parsed transfer parameters"
        );

        // Execute transfer
        let signature = self
            .transfer_spl(mint, to_address, amount_raw, decimals)
            .await?;

        info!(signature = %signature, "SPL token transfer completed");

        Ok(TransferResult {
            tx_hash: signature,
            confirmed: true,
        })
    }
}

/// Create Solana SDK handler from environment variables
///
/// Required environment variables:
/// - `SOLANA_PRIVATE_KEY`: Base58-encoded keypair
///
/// Optional:
/// - `SOLANA_RPC_URL`: Custom RPC URL (overrides default)
pub fn solana_sdk_handler(
    private_key: &str,
    rpc_url: Option<&str>,
) -> Option<Box<dyn ExternalChainHandler>> {
    let mut config = SolanaSdkConfig::mainnet();

    if let Some(rpc_url) = rpc_url {
        config.rpc_url = rpc_url.to_string();
    }

    let handler = SolanaSdkHandler::from_base58(config.clone(), private_key).ok();

    if let Some(h) = handler {
        info!(
            chain_id = %h.config.chain_id,
            rpc_url = %h.config.rpc_url,
            public_key = %h.public_key(),
            "Configured Solana SDK handler"
        );
        Some(Box::new(h))
    } else {
        None
    }
}

pub fn solana_sdk_handler_from_env() -> Option<Box<dyn ExternalChainHandler>> {
    let private_key = read_env_value("SOLANA_PRIVATE_KEY")?;
    let rpc_url = read_env_value("SOLANA_RPC_URL");
    solana_sdk_handler(&private_key, rpc_url.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_solana_sdk_config_mainnet() {
        let config = SolanaSdkConfig::mainnet();
        assert_eq!(config.chain_id, "sol:mainnet");
        assert!(config.token_mints.contains_key("USDC"));
        assert!(config.token_mints.contains_key("USDT"));
        assert_eq!(config.token_decimals.get("USDC"), Some(&6));
    }

    #[test]
    fn test_solana_sdk_config_devnet() {
        let config = SolanaSdkConfig::devnet();
        assert_eq!(config.chain_id, "sol:devnet");
        assert!(config.token_mints.contains_key("USDC"));
    }

    #[test]
    fn test_keypair_creation() {
        // Generate a test keypair
        use ed25519_dalek::SigningKey;
        use rand::rngs::OsRng;

        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();

        let mut keypair_bytes = [0u8; 64];
        keypair_bytes[..32].copy_from_slice(signing_key.as_bytes());
        keypair_bytes[32..].copy_from_slice(verifying_key.as_bytes());

        let keypair = SolanaKeypair::from_bytes(&keypair_bytes);
        assert!(keypair.is_ok());
    }

    #[test]
    fn test_keypair_signing() {
        use ed25519_dalek::{SigningKey, Verifier};
        use rand::rngs::OsRng;

        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();

        let mut keypair_bytes = [0u8; 64];
        keypair_bytes[..32].copy_from_slice(signing_key.as_bytes());
        keypair_bytes[32..].copy_from_slice(verifying_key.as_bytes());

        let keypair = SolanaKeypair::from_bytes(&keypair_bytes).unwrap();

        let message = b"test message";
        let signature_bytes = keypair.sign(message);

        // Verify signature
        let signature = ed25519_dalek::Signature::from_bytes(&signature_bytes);
        assert!(verifying_key.verify(message, &signature).is_ok());
    }

    #[test]
    fn test_handler_creation_with_json_bytes() {
        use ed25519_dalek::SigningKey;
        use rand::rngs::OsRng;

        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();

        let mut keypair_bytes = vec![0u8; 64];
        keypair_bytes[..32].copy_from_slice(signing_key.as_bytes());
        keypair_bytes[32..].copy_from_slice(verifying_key.as_bytes());

        let json = serde_json::to_string(&keypair_bytes).unwrap();

        let config = SolanaSdkConfig::devnet();
        let handler = SolanaSdkHandler::from_json_bytes(config, &json);

        assert!(handler.is_ok());
        let handler = handler.unwrap();
        assert_eq!(handler.chain_id(), "sol:devnet");
        assert!(handler.supports_token("USDC"));
        assert!(!handler.supports_token("UNKNOWN"));
    }

    #[test]
    fn test_handler_public_key() {
        use ed25519_dalek::SigningKey;
        use rand::rngs::OsRng;

        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();

        let mut keypair_bytes = vec![0u8; 64];
        keypair_bytes[..32].copy_from_slice(signing_key.as_bytes());
        keypair_bytes[32..].copy_from_slice(verifying_key.as_bytes());

        let json = serde_json::to_string(&keypair_bytes).unwrap();

        let config = SolanaSdkConfig::devnet();
        let handler = SolanaSdkHandler::from_json_bytes(config, &json).unwrap();

        let pubkey = handler.public_key();
        assert!(!pubkey.is_empty());
        // Base58 encoded 32-byte key should be 32-44 chars
        assert!(pubkey.len() >= 32 && pubkey.len() <= 44);
    }

    #[tokio::test]
    async fn test_transfer_unsupported_token() {
        use ed25519_dalek::SigningKey;
        use rand::rngs::OsRng;

        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();

        let mut keypair_bytes = vec![0u8; 64];
        keypair_bytes[..32].copy_from_slice(signing_key.as_bytes());
        keypair_bytes[32..].copy_from_slice(verifying_key.as_bytes());

        let json = serde_json::to_string(&keypair_bytes).unwrap();

        let config = SolanaSdkConfig::devnet();
        let handler = SolanaSdkHandler::from_json_bytes(config, &json).unwrap();

        let result = handler
            .transfer_tokens("11111111111111111111111111111111", "UNKNOWN", "100", None)
            .await;

        assert!(matches!(
            result,
            Err(ExternalChainError::TokenNotSupported { .. })
        ));
    }

    #[tokio::test]
    async fn test_transfer_invalid_amount() {
        use ed25519_dalek::SigningKey;
        use rand::rngs::OsRng;

        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();

        let mut keypair_bytes = vec![0u8; 64];
        keypair_bytes[..32].copy_from_slice(signing_key.as_bytes());
        keypair_bytes[32..].copy_from_slice(verifying_key.as_bytes());

        let json = serde_json::to_string(&keypair_bytes).unwrap();

        let config = SolanaSdkConfig::devnet();
        let handler = SolanaSdkHandler::from_json_bytes(config, &json).unwrap();

        let result = handler
            .transfer_tokens("11111111111111111111111111111111", "USDC", "invalid", None)
            .await;

        assert!(matches!(result, Err(ExternalChainError::InvalidAmount(_))));
    }

    #[test]
    fn test_solana_config_token_decimals() {
        let config = SolanaSdkConfig::mainnet();

        // USDC and USDT should be 6 decimals
        assert_eq!(config.token_decimals.get("USDC"), Some(&6));
        assert_eq!(config.token_decimals.get("USDT"), Some(&6));

        // Wrapped SOL should be 9 decimals
        assert_eq!(config.token_decimals.get("WSOL"), Some(&9));
    }

    #[test]
    fn test_solana_config_token_mints() {
        let config = SolanaSdkConfig::mainnet();

        // Check that major tokens have mints configured
        assert!(config.token_mints.contains_key("USDC"));
        assert!(config.token_mints.contains_key("USDT"));
        assert!(config.token_mints.contains_key("WSOL"));

        // Token mints should be valid base58 strings
        for mint in config.token_mints.values() {
            assert!(!mint.is_empty());
            assert!(mint.len() >= 32);
        }
    }

    #[test]
    fn test_solana_mainnet_vs_devnet() {
        let mainnet = SolanaSdkConfig::mainnet();
        let devnet = SolanaSdkConfig::devnet();

        assert_ne!(mainnet.chain_id, devnet.chain_id);
        assert_ne!(mainnet.rpc_url, devnet.rpc_url);

        // Mainnet should have more tokens configured
        assert!(mainnet.token_mints.len() >= devnet.token_mints.len());
    }

    #[test]
    fn test_handler_supports_multiple_tokens() {
        use ed25519_dalek::SigningKey;
        use rand::rngs::OsRng;

        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();

        let mut keypair_bytes = vec![0u8; 64];
        keypair_bytes[..32].copy_from_slice(signing_key.as_bytes());
        keypair_bytes[32..].copy_from_slice(verifying_key.as_bytes());

        let json = serde_json::to_string(&keypair_bytes).unwrap();

        let config = SolanaSdkConfig::mainnet();
        let handler = SolanaSdkHandler::from_json_bytes(config, &json).unwrap();

        // Should support all configured tokens (case sensitive)
        assert!(handler.supports_token("USDC"));
        assert!(handler.supports_token("USDT"));
        assert!(handler.supports_token("WSOL"));

        // Should not support tokens that aren't configured
        assert!(!handler.supports_token("BTC"));
        assert!(!handler.supports_token("UNKNOWN"));
    }

    #[test]
    fn test_handler_invalid_json_bytes() {
        let config = SolanaSdkConfig::mainnet();

        // Invalid JSON
        let result = SolanaSdkHandler::from_json_bytes(config.clone(), "not json");
        assert!(result.is_err());

        // Valid JSON but wrong format
        let result = SolanaSdkHandler::from_json_bytes(config.clone(), "[]");
        assert!(result.is_err());

        // Valid JSON but wrong length
        let short_array = vec![1u8, 2, 3];
        let json = serde_json::to_string(&short_array).unwrap();
        let result = SolanaSdkHandler::from_json_bytes(config, &json);
        assert!(result.is_err());
    }
}
