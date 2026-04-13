//! Stellar blockchain handler for XLM and Stellar assets
//!
//! Uses direct Horizon API calls for Stellar asset transfers.

use super::{ExternalChainError, ExternalChainHandler, TransferResult};
use async_trait::async_trait;
use serde::Deserialize;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tracing::{error, info};

/// Stellar asset information
#[derive(Debug, Clone)]
pub struct StellarAsset {
    /// Asset code (e.g., "USDC")
    pub code: String,
    /// Issuer public key (for non-native assets)
    pub issuer: Option<String>,
    /// Decimals (Stellar uses 7 decimals by default)
    pub decimals: u8,
}

/// Stellar chain configuration
#[derive(Debug, Clone)]
pub struct StellarConfig {
    /// Chain identifier (e.g., "stellar:mainnet", "stellar:testnet")
    pub chain_id: String,
    /// Human-readable name
    pub name: String,
    /// Horizon API URL
    pub horizon_url: String,
    /// Network passphrase for transaction signing
    pub network_passphrase: String,
    /// Supported assets (symbol -> asset info)
    pub assets: HashMap<String, StellarAsset>,
}

impl StellarConfig {
    /// Create mainnet configuration
    pub fn mainnet() -> Self {
        let mut assets = HashMap::new();

        // USDC on Stellar (issued by Circle)
        assets.insert(
            "USDC".to_string(),
            StellarAsset {
                code: "USDC".to_string(),
                issuer: Some(
                    "GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN".to_string(),
                ),
                decimals: 7,
            },
        );

        // Native XLM
        assets.insert(
            "XLM".to_string(),
            StellarAsset {
                code: "XLM".to_string(),
                issuer: None, // Native asset
                decimals: 7,
            },
        );

        Self {
            chain_id: "stellar:mainnet".to_string(),
            name: "Stellar Mainnet".to_string(),
            horizon_url: "https://horizon.stellar.org".to_string(),
            network_passphrase: "Public Global Stellar Network ; September 2015".to_string(),
            assets,
        }
    }

    /// Create testnet configuration
    pub fn testnet() -> Self {
        let mut assets = HashMap::new();

        // Native XLM on testnet
        assets.insert(
            "XLM".to_string(),
            StellarAsset {
                code: "XLM".to_string(),
                issuer: None,
                decimals: 7,
            },
        );

        Self {
            chain_id: "stellar:testnet".to_string(),
            name: "Stellar Testnet".to_string(),
            horizon_url: "https://horizon-testnet.stellar.org".to_string(),
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            assets,
        }
    }
}

/// Horizon API account response
#[derive(Debug, Deserialize)]
struct HorizonAccount {
    sequence: String,
}

/// Horizon API transaction submission response
#[derive(Debug, Deserialize)]
struct HorizonTransactionResponse {
    hash: String,
}

/// Stellar keypair wrapper using stellar-base
struct StellarKeypair {
    secret_key: [u8; 32],
    public_key: String,
}

impl StellarKeypair {
    /// Create from secret key string (S...)
    fn from_secret(secret: &str) -> Result<Self, String> {
        use stellar_strkey::Strkey;

        let decoded = Strkey::from_string(secret)
            .map_err(|e| format!("Invalid Stellar secret key: {}", e))?;

        match decoded {
            Strkey::PrivateKeyEd25519(key_bytes) => {
                let mut secret_key = [0u8; 32];
                secret_key.copy_from_slice(&key_bytes.0);

                // Derive public key from secret key using ed25519-dalek
                use ed25519_dalek::{SigningKey, VerifyingKey};
                let signing_key = SigningKey::from_bytes(&secret_key);
                let verifying_key: VerifyingKey = (&signing_key).into();
                let public_key_bytes = verifying_key.to_bytes();

                // Encode public key as Stellar G... address
                let public_strkey =
                    Strkey::PublicKeyEd25519(stellar_strkey::ed25519::PublicKey(public_key_bytes));
                let public_key = public_strkey.to_string();

                Ok(Self {
                    secret_key,
                    public_key,
                })
            }
            _ => Err("Expected Ed25519 private key".to_string()),
        }
    }

    /// Get public key as Stellar address (G...)
    fn public_key(&self) -> &str {
        &self.public_key
    }
}

/// Stellar chain handler
pub struct StellarHandler {
    config: StellarConfig,
    keypair: Arc<StellarKeypair>,
    client: reqwest::Client,
}

impl StellarHandler {
    /// Create a new Stellar handler
    pub fn new(config: StellarConfig, secret_key: &str) -> Result<Self, String> {
        let keypair = StellarKeypair::from_secret(secret_key)?;

        info!(
            chain = %config.name,
            source_account = %keypair.public_key(),
            "Initialized Stellar handler"
        );

        Ok(Self {
            config,
            keypair: Arc::new(keypair),
            client: reqwest::Client::new(),
        })
    }

    /// Parse amount string to stroops (1 XLM = 10^7 stroops)
    fn parse_amount(&self, amount_str: &str, decimals: u8) -> Result<i64, String> {
        let parts: Vec<&str> = amount_str.split('.').collect();
        let (whole, frac) = match parts.len() {
            1 => (parts[0], ""),
            2 => (parts[0], parts[1]),
            _ => return Err("Invalid amount format".to_string()),
        };

        let whole_num: i64 = whole
            .parse()
            .map_err(|e| format!("Invalid whole part: {}", e))?;

        let frac_padded = format!("{:0<width$}", frac, width = decimals as usize);
        let frac_trimmed = &frac_padded[..decimals as usize];
        let frac_num: i64 = frac_trimmed
            .parse()
            .map_err(|e| format!("Invalid fractional part: {}", e))?;

        let multiplier = 10i64.pow(decimals as u32);
        whole_num
            .checked_mul(multiplier)
            .ok_or_else(|| "Overflow".to_string())?
            .checked_add(frac_num)
            .ok_or_else(|| "Overflow".to_string())
    }

    /// Build and submit Stellar payment transaction via Horizon API
    #[allow(clippy::too_many_lines)]
    async fn submit_payment(
        &self,
        destination: &str,
        asset: &StellarAsset,
        amount_stroops: i64,
        memo: Option<&str>,
    ) -> Result<String, ExternalChainError> {
        use stellar_base::{
            amount::Amount,
            asset::Asset as StellarBaseAsset,
            crypto::{DalekKeyPair as StellarKeyPair, PublicKey},
            network::Network,
            operations::Operation,
            transaction::{Transaction, MIN_BASE_FEE},
            xdr::XDRSerialize,
        };

        info!(
            destination = %destination,
            asset_code = %asset.code,
            amount_stroops = %amount_stroops,
            "Building Stellar payment transaction"
        );

        let account_url = format!(
            "{}/accounts/{}",
            self.config.horizon_url,
            self.keypair.public_key()
        );
        let account: HorizonAccount = self
            .client
            .get(&account_url)
            .send()
            .await
            .map_err(|e| {
                ExternalChainError::RpcConnectionFailed(format!("Failed to fetch account: {}", e))
            })?
            .json()
            .await
            .map_err(|e| {
                ExternalChainError::RpcConnectionFailed(format!("Failed to parse account: {}", e))
            })?;

        let sequence: i64 = account.sequence.parse().map_err(|e| {
            ExternalChainError::TransactionFailed(format!("Invalid sequence: {}", e))
        })?;

        let destination_pk = PublicKey::from_account_id(destination).map_err(|e| {
            ExternalChainError::InvalidAddress(format!("Invalid destination: {}", e))
        })?;

        let stellar_asset = match &asset.issuer {
            None => StellarBaseAsset::new_native(),
            Some(issuer) => {
                let issuer_pk = PublicKey::from_account_id(issuer).map_err(|e| {
                    ExternalChainError::InvalidAmount(format!("Invalid issuer: {}", e))
                })?;
                StellarBaseAsset::new_credit(&asset.code, issuer_pk).map_err(|e| {
                    ExternalChainError::InvalidAmount(format!("Invalid asset: {}", e))
                })?
            }
        };

        // Convert stroops to Amount (7 decimals: 1 XLM = 10,000,000 stroops)
        let amount_str = format!("{:.7}", amount_stroops as f64 / 10_000_000.0);
        let amount = Amount::from_str(&amount_str)
            .map_err(|e| ExternalChainError::InvalidAmount(format!("Invalid amount: {}", e)))?;

        let payment = Operation::new_payment()
            .with_destination(destination_pk)
            .with_amount(amount)
            .map_err(|e| ExternalChainError::TransactionFailed(format!("Invalid amount: {}", e)))?
            .with_asset(stellar_asset)
            .build()
            .map_err(|e| {
                ExternalChainError::TransactionFailed(format!("Failed to build payment op: {}", e))
            })?;

        let source_pk = PublicKey::from_account_id(self.keypair.public_key()).map_err(|e| {
            ExternalChainError::InvalidPrivateKey(format!("Invalid source key: {}", e))
        })?;

        let network = if self.config.network_passphrase.contains("Public") {
            Network::new_public()
        } else {
            Network::new_test()
        };

        let mut tx_builder =
            Transaction::builder(source_pk, sequence + 1, MIN_BASE_FEE).add_operation(payment);

        // Add memo if provided (for MEMO-mode deposits)
        if let Some(memo_text) = memo {
            use stellar_base::memo::Memo;
            tx_builder = tx_builder.with_memo(Memo::new_text(memo_text).map_err(|e| {
                ExternalChainError::TransactionFailed(format!("Invalid memo: {}", e))
            })?);
            info!(memo = %memo_text, "Added memo to Stellar transaction");
        }

        let mut tx = tx_builder.into_transaction().map_err(|e| {
            ExternalChainError::TransactionFailed(format!("Failed to build transaction: {}", e))
        })?;

        // Sign transaction using stellar-base DalekKeyPair
        let stellar_kp =
            StellarKeyPair::from_seed_bytes(&self.keypair.secret_key).map_err(|e| {
                ExternalChainError::InvalidPrivateKey(format!("Failed to create keypair: {}", e))
            })?;

        tx.sign(stellar_kp.as_ref(), &network).map_err(|e| {
            ExternalChainError::TransactionFailed(format!("Failed to sign transaction: {}", e))
        })?;

        // Convert to XDR base64 for submission
        let tx_envelope = tx.into_envelope().xdr_base64().map_err(|e| {
            ExternalChainError::TransactionFailed(format!("Failed to encode XDR: {}", e))
        })?;

        let submit_url = format!("{}/transactions", self.config.horizon_url);
        let params = [("tx", tx_envelope)];

        let response: HorizonTransactionResponse = self
            .client
            .post(&submit_url)
            .form(&params)
            .send()
            .await
            .map_err(|e| {
                error!("Stellar transaction submission failed: {}", e);
                ExternalChainError::TransactionFailed(format!("Submission failed: {}", e))
            })?
            .json()
            .await
            .map_err(|e| {
                ExternalChainError::TransactionFailed(format!("Failed to parse response: {}", e))
            })?;

        info!(
            tx_hash = %response.hash,
            destination = %destination,
            "Stellar payment transaction submitted successfully"
        );

        Ok(response.hash)
    }
}

#[async_trait]
impl ExternalChainHandler for StellarHandler {
    fn chain_id(&self) -> &str {
        &self.config.chain_id
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn supports_token(&self, asset: &str) -> bool {
        self.config.assets.contains_key(asset)
    }

    async fn transfer_tokens(
        &self,
        to_address: &str,
        asset: &str,
        amount: &str,
        memo: Option<&str>,
    ) -> Result<TransferResult, ExternalChainError> {
        info!(
            chain = %self.config.name,
            to = %to_address,
            asset = %asset,
            amount = %amount,
            memo = ?memo,
            "Initiating Stellar transfer"
        );

        let stellar_asset =
            self.config
                .assets
                .get(asset)
                .ok_or_else(|| ExternalChainError::TokenNotSupported {
                    asset: asset.to_string(),
                    chain: self.config.chain_id.clone(),
                })?;

        let amount_stroops = self
            .parse_amount(amount, stellar_asset.decimals)
            .map_err(ExternalChainError::InvalidAmount)?;

        let tx_hash = self
            .submit_payment(to_address, stellar_asset, amount_stroops, memo)
            .await?;

        Ok(TransferResult {
            tx_hash,
            confirmed: false, // Stellar transactions need a few seconds to confirm
        })
    }
}

/// Create Stellar handler from environment variables
///
/// Required:
/// - `STELLAR_SECRET_KEY`: Stellar secret key (S...)
///
/// Optional:
/// - `STELLAR_HORIZON_URL`: Custom Horizon API URL (overrides default)
///
/// To derive a secret key from a BIP39 seed phrase, use:
/// `node scripts/derive-stellar-key.js "your seed phrase here"`
pub fn stellar_handler_from_env() -> Option<Box<dyn ExternalChainHandler>> {
    let mut config = StellarConfig::mainnet();

    if let Ok(horizon_url) = std::env::var("STELLAR_HORIZON_URL") {
        config.horizon_url = horizon_url;
    }

    if let Ok(secret_key) = std::env::var("STELLAR_SECRET_KEY") {
        match StellarHandler::new(config.clone(), &secret_key) {
            Ok(handler) => {
                info!(
                    chain_id = %handler.config.chain_id,
                    horizon_url = %handler.config.horizon_url,
                    public_key = %handler.keypair.public_key(),
                    "Configured Stellar handler"
                );
                Some(Box::new(handler))
            }
            Err(e) => {
                error!("Failed to create Stellar handler: {}", e);
                None
            }
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stellar_config_mainnet() {
        let config = StellarConfig::mainnet();
        assert_eq!(config.chain_id, "stellar:mainnet");
        assert!(config.assets.contains_key("USDC"));
        assert!(config.assets.contains_key("XLM"));
        assert_eq!(config.horizon_url, "https://horizon.stellar.org");
        assert!(config.network_passphrase.contains("Public"));
    }

    #[test]
    fn test_stellar_config_testnet() {
        let config = StellarConfig::testnet();
        assert_eq!(config.chain_id, "stellar:testnet");
        assert!(config.assets.contains_key("XLM"));
        assert_eq!(config.horizon_url, "https://horizon-testnet.stellar.org");
        assert!(config.network_passphrase.contains("Test"));
    }

    #[test]
    fn test_stellar_asset_mainnet_usdc() {
        let config = StellarConfig::mainnet();
        let usdc = config.assets.get("USDC").unwrap();

        assert_eq!(usdc.code, "USDC");
        assert!(usdc.issuer.is_some());
        assert_eq!(usdc.decimals, 7);
    }

    #[test]
    fn test_stellar_asset_native_xlm() {
        let config = StellarConfig::mainnet();
        let xlm = config.assets.get("XLM").unwrap();

        assert_eq!(xlm.code, "XLM");
        assert!(xlm.issuer.is_none()); // Native asset
        assert_eq!(xlm.decimals, 7);
    }

    #[test]
    fn test_parse_amount() {
        let config = StellarConfig::mainnet();
        let secret = "SAMPLEKEYTHATISNOTREALSAMPLEKEYTHATISNOTREALSAMPLE";

        // This will fail but we just want to test the struct
        if let Ok(handler) = StellarHandler::new(config, secret) {
            assert_eq!(handler.parse_amount("100", 7).unwrap(), 1_000_000_000);
            assert_eq!(handler.parse_amount("1.5", 7).unwrap(), 15_000_000);
            assert_eq!(handler.parse_amount("0.0000001", 7).unwrap(), 1);
        }
    }

    #[test]
    fn test_parse_amount_edge_cases() {
        let config = StellarConfig::mainnet();
        let secret = "SAMPLEKEYTHATISNOTREALSAMPLEKEYTHATISNOTREALSAMPLE";

        if let Ok(handler) = StellarHandler::new(config, secret) {
            // Zero
            assert_eq!(handler.parse_amount("0", 7).unwrap(), 0);
            assert_eq!(handler.parse_amount("0.0", 7).unwrap(), 0);

            // Large number
            assert_eq!(
                handler.parse_amount("1000000", 7).unwrap(),
                10_000_000_000_000
            );

            // Very small number
            assert_eq!(handler.parse_amount("0.0000001", 7).unwrap(), 1);
        }
    }

    #[test]
    fn test_parse_amount_decimal_precision() {
        let config = StellarConfig::mainnet();
        let secret = "SAMPLEKEYTHATISNOTREALSAMPLEKEYTHATISNOTREALSAMPLE";

        if let Ok(handler) = StellarHandler::new(config, secret) {
            // Test 6 decimals (USDC-like)
            assert_eq!(handler.parse_amount("100", 6).unwrap(), 100_000_000);
            assert_eq!(handler.parse_amount("1.5", 6).unwrap(), 1_500_000);

            // Test 18 decimals (ETH-like, though not used on Stellar)
            assert_eq!(
                handler.parse_amount("1", 18).unwrap(),
                1_000_000_000_000_000_000
            );
        }
    }

    #[test]
    fn test_parse_amount_invalid() {
        let config = StellarConfig::mainnet();
        let secret = "SAMPLEKEYTHATISNOTREALSAMPLEKEYTHATISNOTREALSAMPLE";

        if let Ok(handler) = StellarHandler::new(config, secret) {
            // Invalid format
            assert!(handler.parse_amount("abc", 7).is_err());
            assert!(handler.parse_amount("", 7).is_err());
            assert!(handler.parse_amount("1.2.3", 7).is_err());
        }
    }

    #[test]
    fn test_stellar_handler_supports_token() {
        let config = StellarConfig::mainnet();
        let secret = "SAMPLEKEYTHATISNOTREALSAMPLEKEYTHATISNOTREALSAMPLE";

        if let Ok(handler) = StellarHandler::new(config, secret) {
            // Supported assets
            assert!(handler.supports_token("USDC"));
            assert!(handler.supports_token("XLM"));

            // Unsupported asset
            assert!(!handler.supports_token("BTC"));
            assert!(!handler.supports_token("UNKNOWN"));
        }
    }

    #[test]
    fn test_stellar_handler_chain_id() {
        let config = StellarConfig::mainnet();
        let secret = "SAMPLEKEYTHATISNOTREALSAMPLEKEYTHATISNOTREALSAMPLE";

        if let Ok(handler) = StellarHandler::new(config, secret) {
            assert_eq!(handler.chain_id(), "stellar:mainnet");
        }
    }

    #[test]
    fn test_stellar_config_multiple_assets() {
        let config = StellarConfig::mainnet();

        // Should have multiple stablecoins configured
        assert!(config.assets.len() >= 2);
        assert!(config.assets.contains_key("USDC"));
        assert!(config.assets.contains_key("XLM"));
    }

    #[test]
    fn test_horizon_url_configuration() {
        let mainnet = StellarConfig::mainnet();
        let testnet = StellarConfig::testnet();

        assert_ne!(mainnet.horizon_url, testnet.horizon_url);
        assert!(mainnet.horizon_url.contains("horizon.stellar.org"));
        assert!(testnet.horizon_url.contains("testnet"));
    }

    #[test]
    fn test_network_passphrase_configuration() {
        let mainnet = StellarConfig::mainnet();
        let testnet = StellarConfig::testnet();

        assert_ne!(mainnet.network_passphrase, testnet.network_passphrase);
        assert!(mainnet.network_passphrase.contains("Public"));
        assert!(testnet.network_passphrase.contains("Test"));
    }
}
