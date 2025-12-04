//! NEAR Intents integration for cross-chain withdrawals
//!
//! This module provides support for creating and executing intents on the NEAR Intents
//! Verifier contract (intents.near). It implements NEP-413 message signing for secure
//! intent execution.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use chrono::{Duration, Utc};
use near_crypto::{PublicKey, SecretKey, Signature};
use near_sdk::borsh::{self, BorshSerialize};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// Verifier contract on NEAR
pub const INTENTS_CONTRACT: &str = "intents.near";

/// Chain IDs for supported networks
pub mod chain_ids {
    pub const ETHEREUM: u32 = 1;
    pub const STELLAR: u32 = 1100;
    pub const SOLANA: u32 = 1151;
    pub const NEAR: u32 = 1313161554; // NEAR mainnet
}

/// Encode receiver address for cross-chain withdrawals
///
/// Encodes the destination address to base58 format required by the bridge.
/// Different chains use different encoding schemes.
fn encode_receiver(chain_id: u32, address: &str) -> Result<String, IntentError> {
    match chain_id {
        // NEAR - pass through
        chain_ids::NEAR => Ok(address.to_string()),

        // Solana - pass through (already in base58)
        chain_ids::SOLANA => Ok(address.to_string()),

        // Stellar - encode as XDR ScVal Address
        chain_ids::STELLAR => {
            use std::str::FromStr;
            use stellar_xdr::curr::{Limited, ScAddress, ScVal, WriteXdr};

            let sc_address = ScAddress::from_str(address).map_err(|_| {
                IntentError::Serialization(format!("Invalid Stellar address: {}", address))
            })?;

            let sc_val = ScVal::Address(sc_address);

            let mut xdr_bytes = Vec::new();
            let mut limited_writer =
                Limited::new(&mut xdr_bytes, stellar_xdr::curr::Limits::none());
            sc_val.write_xdr(&mut limited_writer).map_err(|e| {
                IntentError::Serialization(format!("Failed to encode ScVal to XDR: {}", e))
            })?;

            Ok(bs58::encode(&xdr_bytes).into_string())
        }

        // EVM chains (Ethereum, BSC, Polygon, Arbitrum, etc.)
        // TON and other chains that use hex addresses
        _ => {
            // Decode hex address (strip 0x prefix if present)
            let hex_str = address.strip_prefix("0x").unwrap_or(address);
            let bytes = hex::decode(hex_str)
                .map_err(|e| IntentError::Serialization(format!("Invalid hex address: {}", e)))?;

            Ok(bs58::encode(&bytes).into_string())
        }
    }
}

/// Intent types supported by NEAR Intents
#[derive(Debug, Clone, Serialize, Deserialize, BorshSerialize)]
#[serde(tag = "intent", rename_all = "snake_case")]
#[borsh(use_discriminant = false)]
pub enum Intent {
    /// Transfer tokens within NEAR Intents
    #[serde(rename = "transfer")]
    Transfer {
        receiver_id: String,
        tokens: HashMap<String, String>,
    },
    /// Withdraw NEP-141 tokens to external chain
    #[serde(rename = "ft_withdraw")]
    FtWithdraw {
        token: String,
        receiver_id: String,
        amount: String,
        memo: String,
    },
    /// Withdraw NEP-245 multi-tokens to external chain
    #[serde(rename = "mt_withdraw")]
    MtWithdraw {
        token: String,          // MT contract account ID
        receiver_id: String,    // Receiver account ID
        token_ids: Vec<String>, // Array of token IDs within the MT contract
        amounts: Vec<String>,   // Array of amounts (one per token_id)
        #[serde(skip_serializing_if = "Option::is_none")]
        memo: Option<String>, // Simple withdrawal: base58-encoded address
        #[serde(skip_serializing_if = "Option::is_none")]
        msg: Option<String>, // Gasless withdrawal: JSON with receiver_id, amount_native, block_number
    },
    /// Token difference for swaps
    #[serde(rename = "token_diff")]
    TokenDiff {
        diff: HashMap<String, String>,
        #[borsh(skip)]
        #[serde(skip_serializing_if = "Option::is_none")]
        referral: Option<String>,
    },
}

/// NEP-413 compliant message payload for signing
#[derive(Debug, Clone, Serialize, Deserialize, BorshSerialize)]
pub struct IntentMessage {
    pub signer_id: String,
    pub deadline: String,
    pub intents: Vec<Intent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[borsh(skip)]
    pub nonce: Option<String>,
}

/// Signed payload in NEP-413 format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedPayload {
    pub payload: PayloadWrapper,
    pub standard: String,
    pub signature: String,
    pub public_key: String,
}

/// Wrapper for payload data (NEP-413 format)
/// IMPORTANT: Field order matters for Borsh serialization!
/// NEP-413 specifies: message, nonce, recipient, callbackUrl
#[derive(Debug, Clone, Serialize, Deserialize, BorshSerialize)]
pub struct PayloadWrapper {
    pub message: String,
    #[serde(rename = "nonce")]
    #[borsh(serialize_with = "borsh_serialize_nonce_as_array")]
    pub nonce: String,
    pub recipient: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_url: Option<String>,
}

// Helper function to serialize nonce as [u8; 32] for Borsh
fn borsh_serialize_nonce_as_array<W: std::io::Write>(
    nonce: &String,
    writer: &mut W,
) -> Result<(), std::io::Error> {
    let nonce_bytes = BASE64
        .decode(nonce)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if nonce_bytes.len() != 32 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Nonce must be 32 bytes",
        ));
    }
    let nonce_array: [u8; 32] = nonce_bytes.try_into().unwrap();
    BorshSerialize::serialize(&nonce_array, writer)
}

/// Arguments for execute_intents call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteIntentsArgs {
    pub signed: Vec<SignedPayload>,
}

/// Builder for creating withdrawal intents
pub struct WithdrawalIntentBuilder {
    signer_id: String,
    signer_key: SecretKey,
    deadline_minutes: i64,
}

impl WithdrawalIntentBuilder {
    /// Create a new withdrawal intent builder
    pub fn new(signer_id: String, signer_key: SecretKey) -> Self {
        Self {
            signer_id,
            signer_key,
            deadline_minutes: 5, // Default 5 minute deadline
        }
    }

    /// Set custom deadline in minutes from now
    #[allow(dead_code)]
    pub fn with_deadline_minutes(mut self, minutes: i64) -> Self {
        self.deadline_minutes = minutes;
        self
    }

    /// Create a cross-chain withdrawal intent for NEP-141 tokens
    ///
    /// # Arguments
    /// * `token` - NEAR OMFT token contract (e.g., "eth-0x....omft.near", "sol-EPj....omft.near")
    /// * `amount` - Amount in smallest units
    /// * `destination_address` - Address on destination chain (EVM: 0x..., Solana: base58)
    pub fn build_withdrawal(
        &self,
        token: &str,
        amount: u128,
        destination_address: &str,
    ) -> Result<ExecuteIntentsArgs, IntentError> {
        // Create the withdrawal intent
        let intent = Intent::FtWithdraw {
            token: token.to_string(),
            receiver_id: token.to_string(), // Same as token for withdrawals
            amount: amount.to_string(),
            memo: format!("WITHDRAW_TO:{}", destination_address),
        };

        self.build_intent(intent)
    }

    /// Create a cross-chain withdrawal intent for NEP-245 multi-tokens
    ///
    /// # Arguments
    /// * `token` - Full token ID in format "nep245:contract:token_id"
    /// * `amount` - Amount in smallest units
    /// * `destination_address` - Destination address on target chain
    /// * `chain_id` - Target chain ID (1 for Ethereum, 1151 for Solana, etc.)
    /// * `use_gasless` - Whether to use gasless withdrawal via bridge-refuel.hot.tg
    pub fn build_mt_withdrawal(
        &self,
        token: &str,
        amount: u128,
        destination_address: &str,
        chain_id: u32,
        use_gasless: bool,
    ) -> Result<ExecuteIntentsArgs, IntentError> {
        // Token input is in intents format: "nep245:contract:multi_token_id"
        // Parse this format
        let token_str = token.strip_prefix("nep245:").unwrap_or(token);
        let parts: Vec<&str> = token_str.split(':').collect();

        let (contract_id, multi_token_id) = if parts.len() >= 2 {
            (parts[0], parts[1..].join(":"))
        } else {
            return Err(IntentError::Serialization(format!(
                "Invalid NEP-245 token format. Expected 'nep245:contract:multi_token_id', got: {}",
                token
            )));
        };

        // Encode the destination address based on the chain
        let encoded_receiver = encode_receiver(chain_id, destination_address)?;

        // Build intent - always use msg field format
        // Try sending directly to v2_1.omni.hot.tg with msg (not memo)
        let msg_payload = serde_json::json!({
            "receiver_id": encoded_receiver,
            "amount_native": "0",
            "block_number": 0
        });

        let intent = if use_gasless {
            Intent::MtWithdraw {
                token: contract_id.to_string(),
                receiver_id: "bridge-refuel.hot.tg".to_string(),
                token_ids: vec![multi_token_id],
                amounts: vec![amount.to_string()],
                memo: None,
                msg: Some(msg_payload.to_string()),
            }
        } else {
            Intent::MtWithdraw {
                token: contract_id.to_string(),
                receiver_id: contract_id.to_string(),
                token_ids: vec![multi_token_id],
                amounts: vec![amount.to_string()],
                memo: None,
                msg: Some(msg_payload.to_string()),
            }
        };

        self.build_intent(intent)
    }

    /// Build and sign an intent
    fn build_intent(&self, intent: Intent) -> Result<ExecuteIntentsArgs, IntentError> {
        // Create the message
        let deadline = Utc::now() + Duration::minutes(self.deadline_minutes);
        let message = IntentMessage {
            signer_id: self.signer_id.clone(),
            deadline: deadline.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
            intents: vec![intent],
            nonce: None,
        };

        // Serialize message
        let message_json = serde_json::to_string(&message)
            .map_err(|e| IntentError::Serialization(e.to_string()))?;

        let nonce = self.generate_nonce();
        let nonce_b64 = BASE64.encode(&nonce);

        let payload = PayloadWrapper {
            message: message_json,
            nonce: nonce_b64,
            recipient: INTENTS_CONTRACT.to_string(),
            callback_url: None,
        };

        // Sign the payload
        let signed_payload = self.sign_payload(payload)?;

        Ok(ExecuteIntentsArgs {
            signed: vec![signed_payload],
        })
    }

    fn generate_nonce(&self) -> Vec<u8> {
        let mut nonce = vec![0u8; 32];
        let data = format!(
            "{}{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0),
            self.signer_id
        );
        let hash = Sha256::digest(data.as_bytes());
        nonce.copy_from_slice(&hash[..32]);
        nonce
    }

    /// Sign the payload for NEAR Intents using NEP-413 standard
    ///
    /// NEP-413 requires:
    /// 1. Borsh serialize (OFFCHAIN_PREFIX_TAG, payload) tuple
    /// 2. SHA-256 hash the serialized bytes
    /// 3. Sign the hash with Ed25519
    fn sign_payload(&self, payload: PayloadWrapper) -> Result<SignedPayload, IntentError> {
        // NEP-413 OFFCHAIN_PREFIX_TAG = (1 << 31) + 413 = 2147484061
        const OFFCHAIN_PREFIX_TAG: u32 = 2147484061;

        // Borsh serialize (tag, payload) tuple
        let prehash = borsh::to_vec(&(OFFCHAIN_PREFIX_TAG, &payload))
            .map_err(|e| IntentError::Serialization(e.to_string()))?;

        // SHA-256 hash the serialized bytes
        let hash = Sha256::digest(&prehash);

        // Sign the hash
        let signature = self.signer_key.sign(&hash);
        let public_key = self.signer_key.public_key();

        // Debug logging
        tracing::debug!(
            prehash_len = prehash.len(),
            hash_hex = %hex::encode(hash),
            public_key = %format_public_key(&public_key),
            signer_id = %self.signer_id,
            message_preview = %&payload.message[..payload.message.len().min(100)],
            "Signed NEP-413 payload"
        );

        Ok(SignedPayload {
            payload,
            standard: "nep413".to_string(),
            signature: format_signature(&signature),
            public_key: format_public_key(&public_key),
        })
    }
}

/// Format signature in NEAR format (ed25519:base58)
fn format_signature(sig: &Signature) -> String {
    match sig {
        Signature::ED25519(data) => {
            let bytes = data.to_bytes();
            format!("ed25519:{}", bs58::encode(&bytes).into_string())
        }
        Signature::SECP256K1(_) => {
            // SECP256K1 is not commonly used for NEAR Intents
            // ED25519 is the standard key type
            unimplemented!("SECP256K1 signatures not supported for intents")
        }
    }
}

/// Format public key in NEAR format (ed25519:base58)
fn format_public_key(pk: &PublicKey) -> String {
    match pk {
        PublicKey::ED25519(data) => {
            let bytes = data.as_ref();
            format!("ed25519:{}", bs58::encode(bytes).into_string())
        }
        PublicKey::SECP256K1(_) => {
            // SECP256K1 is not commonly used for NEAR Intents
            // ED25519 is the standard key type
            unimplemented!("SECP256K1 keys not supported for intents")
        }
    }
}

/// Errors that can occur during intent operations
#[derive(Debug, thiserror::Error)]
pub enum IntentError {
    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Signing error: {0}")]
    Signing(String),

    #[error("Invalid configuration: {0}")]
    Config(String),
}

/// Helper to construct NEAR OMFT token ID from chain type and token address
///
/// # Examples
/// - `construct_omft_token_id("eth", "native")` -> "eth.omft.near"
/// - `construct_omft_token_id("sol", "EPjFWdd...")` -> "sol-epjfwdd....omft.near"
pub fn construct_omft_token_id(chain_type: &str, token_address: &str) -> String {
    if token_address == "native" {
        // Native token (ETH, SOL, etc.)
        format!("{}.omft.near", chain_type)
    } else {
        // Token with contract address
        format!("{}-{}.omft.near", chain_type, token_address.to_lowercase())
    }
}

/// Construct destination address memo
pub fn construct_withdraw_memo(destination_address: &str) -> String {
    format!("WITHDRAW_TO:{}", destination_address)
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_crypto::KeyType;

    fn test_key() -> SecretKey {
        SecretKey::from_seed(KeyType::ED25519, "test-seed")
    }

    #[test]
    fn test_construct_omft_token_id_native() {
        let token_id = construct_omft_token_id("eth", "native");
        assert_eq!(token_id, "eth.omft.near");
    }

    #[test]
    fn test_construct_omft_token_id_erc20() {
        let token_id = construct_omft_token_id("eth", "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48");
        assert_eq!(
            token_id,
            "eth-0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48.omft.near"
        );
    }

    #[test]
    fn test_construct_withdraw_memo() {
        let memo = construct_withdraw_memo("0x19f897E4c0882D800F633Ac13B8D488CD03f02CF");
        assert_eq!(
            memo,
            "WITHDRAW_TO:0x19f897E4c0882D800F633Ac13B8D488CD03f02CF"
        );
    }

    #[test]
    fn test_withdrawal_intent_builder_creation() {
        let builder = WithdrawalIntentBuilder::new("test.near".to_string(), test_key());
        assert_eq!(builder.signer_id, "test.near");
    }

    #[test]
    fn test_build_withdrawal_intent() {
        let builder = WithdrawalIntentBuilder::new("treasury.near".to_string(), test_key());

        let args = builder
            .build_withdrawal(
                "eth-0xdac17f958d2ee523a2206206994597c13d831ec7.omft.near",
                1_000_000u128,
                "0x19f897E4c0882D800F633Ac13B8D488CD03f02CF",
            )
            .expect("Should build withdrawal intent");

        assert_eq!(args.signed.len(), 1);
        assert_eq!(args.signed[0].standard, "nep413");
        assert!(args.signed[0].signature.starts_with("ed25519:"));
        assert!(args.signed[0].public_key.starts_with("ed25519:"));

        // Verify the payload contains the withdrawal
        let payload = &args.signed[0].payload;
        assert_eq!(payload.recipient, "intents.near");
        assert!(!payload.nonce.is_empty());

        // Parse the message
        let message: IntentMessage =
            serde_json::from_str(&payload.message).expect("Should parse message");
        assert_eq!(message.signer_id, "treasury.near");
        assert_eq!(message.intents.len(), 1);

        // Verify the intent
        match &message.intents[0] {
            Intent::FtWithdraw {
                token,
                amount,
                memo,
                ..
            } => {
                assert_eq!(
                    token,
                    "eth-0xdac17f958d2ee523a2206206994597c13d831ec7.omft.near"
                );
                assert_eq!(amount, "1000000");
                assert_eq!(
                    memo,
                    "WITHDRAW_TO:0x19f897E4c0882D800F633Ac13B8D488CD03f02CF"
                );
            }
            _ => panic!("Expected FtWithdraw intent"),
        }
    }

    #[test]
    fn test_intent_serialization() {
        let intent = Intent::FtWithdraw {
            token: "eth.omft.near".to_string(),
            receiver_id: "eth.omft.near".to_string(),
            amount: "1000000".to_string(),
            memo: "WITHDRAW_TO:0x123".to_string(),
        };

        let json = serde_json::to_string(&intent).expect("Should serialize");
        assert!(json.contains("\"intent\":\"ft_withdraw\""));
        assert!(json.contains("\"token\":\"eth.omft.near\""));
        assert!(json.contains("\"memo\":\"WITHDRAW_TO:0x123\""));
    }

    #[test]
    fn test_transfer_intent_serialization() {
        let mut tokens = HashMap::new();
        tokens.insert("nep141:usdc.near".to_string(), "1000000".to_string());

        let intent = Intent::Transfer {
            receiver_id: "user.near".to_string(),
            tokens,
        };

        let json = serde_json::to_string(&intent).expect("Should serialize");
        assert!(json.contains("\"intent\":\"transfer\""));
        assert!(json.contains("\"receiver_id\":\"user.near\""));
    }

    #[test]
    fn test_token_diff_intent_serialization() {
        let mut diff = HashMap::new();
        diff.insert("nep141:usdc.near".to_string(), "-1000000".to_string());
        diff.insert("nep141:usdt.near".to_string(), "999000".to_string());

        let intent = Intent::TokenDiff {
            diff,
            referral: Some("referrer.near".to_string()),
        };

        let json = serde_json::to_string(&intent).expect("Should serialize");
        assert!(json.contains("\"intent\":\"token_diff\""));
        assert!(json.contains("\"referral\":\"referrer.near\""));
    }

    #[test]
    fn test_deadline_calculation() {
        let builder = WithdrawalIntentBuilder::new("test.near".to_string(), test_key());
        let args = builder
            .build_withdrawal("eth.omft.near", 1000, "0x123")
            .expect("Should build");

        let message: IntentMessage =
            serde_json::from_str(&args.signed[0].payload.message).expect("Should parse message");

        // Parse deadline and verify it's in the future
        let deadline =
            chrono::DateTime::parse_from_rfc3339(&message.deadline).expect("Should parse deadline");
        assert!(deadline > Utc::now());
    }
}
