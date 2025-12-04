//! Token Registry - Centralized token information and decimal handling
//!
//! Provides caching and utilities for token decimals, addresses, and amount conversions.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::bridge::{BridgeClient, ChainId, TokenInfo};

/// Token registry that caches token information from the bridge
#[derive(Clone)]
pub struct TokenRegistry {
    /// Cached token info by (chain_id, asset_name)
    cache: Arc<RwLock<HashMap<String, TokenInfo>>>,
    /// Bridge client for fetching token info
    bridge_client: Arc<BridgeClient>,
}

impl TokenRegistry {
    /// Create a new token registry
    pub fn new(bridge_client: Arc<BridgeClient>) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            bridge_client,
        }
    }

    /// Get token info for an asset on a specific chain
    ///
    /// Returns cached info if available, otherwise fetches from bridge
    pub async fn get_token_info(
        &self,
        asset: &str,
        chain: &str,
    ) -> Result<Option<TokenInfo>, String> {
        let cache_key = format!("{}:{}", chain, asset.to_lowercase());

        // Check cache first
        {
            let cache = self.cache.read().await;
            if let Some(info) = cache.get(&cache_key) {
                return Ok(Some(info.clone()));
            }
        }

        // Fetch from bridge
        match self.bridge_client.find_token(asset, chain).await {
            Ok(Some(info)) => {
                // Cache it
                let mut cache = self.cache.write().await;
                cache.insert(cache_key, info.clone());
                Ok(Some(info))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Get decimals for a token on a chain
    pub async fn get_decimals(&self, asset: &str, chain: &str) -> Result<u8, String> {
        if let Some(info) = self.get_token_info(asset, chain).await? {
            Ok(info.decimals)
        } else {
            // Fallback to common defaults
            Ok(TokenDecimals::default_for_asset(asset))
        }
    }

    /// Clear the cache (useful for testing or manual refresh)
    pub async fn clear_cache(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
    }

    /// Preload token info for common assets on a chain
    pub async fn preload_chain(&self, chain: &str) -> Result<usize, String> {
        let tokens = self
            .bridge_client
            .get_supported_tokens(&[chain.to_string()])
            .await
            .map_err(|e| e.to_string())?;

        let mut cache = self.cache.write().await;
        let mut count = 0;

        for token in tokens {
            let cache_key = format!(
                "{}:{}",
                token.chain().unwrap_or_default(),
                token.asset_name.to_lowercase()
            );
            cache.insert(cache_key, token);
            count += 1;
        }

        Ok(count)
    }

    /// Get OMFT token ID for an asset on a destination chain
    ///
    /// This is used for withdrawal intents where we need to specify the
    /// NEAR token ID in the intent message.
    ///
    /// # Example
    /// ```no_run
    /// # use templar_funding_bridge::tokens::TokenRegistry;
    /// # async fn example(registry: &TokenRegistry) -> Result<(), String> {
    /// // Get OMFT token ID for USDT on Ethereum
    /// let omft_id = registry.get_omft_token_id("USDT", "eth:1").await?;
    /// // Returns: "eth-0xdac17f958d2ee523a2206206994597c13d831ec7.omft.near"
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_omft_token_id(&self, asset: &str, chain: &str) -> Result<String, String> {
        // Try to get from cache/bridge API first
        // If network error occurs, fall through to local fallback
        match self.get_token_info(asset, chain).await {
            Ok(Some(info)) => return Ok(info.near_token_id),
            Ok(None) => {} // Not found, continue to fallback
            Err(_) => {}   // Network error, continue to fallback
        }

        // Fallback to generating OMFT ID from known addresses
        let chain_id = crate::bridge::ChainId::parse(chain)
            .ok_or_else(|| format!("Invalid chain format: {}", chain))?;

        // Check for native tokens first
        if asset.to_lowercase() == "eth" && chain_id.chain_type == "eth" {
            return Ok("eth.omft.near".to_string());
        }
        if asset.to_lowercase() == "sol" && chain_id.chain_type == "sol" {
            return Ok("sol.omft.near".to_string());
        }

        // Try to get known token address
        let token_address = match asset.to_lowercase().as_str() {
            "usdc" => TokenAddresses::usdc(&chain_id),
            "usdt" => TokenAddresses::usdt(&chain_id),
            "weth" => TokenAddresses::weth(&chain_id),
            "wbtc" | "btc" => TokenAddresses::wbtc(&chain_id),
            "wsol" => TokenAddresses::wsol(&chain_id),
            _ => None,
        };

        match token_address {
            Some(addr) => {
                // Generate OMFT token ID: {chain_type}-{address}.omft.near
                // e.g., eth-0xdac17f958d2ee523a2206206994597c13d831ec7.omft.near
                Ok(format!("{}-{}.omft.near", chain_id.chain_type, addr))
            }
            None => Err(format!(
                "Unknown token {} on chain {}, please provide OMFT token ID directly",
                asset, chain
            )),
        }
    }

    /// Resolve asset identifier to OMFT token ID
    ///
    /// Handles multiple input formats:
    /// - Simple asset name: "USDT" -> looks up OMFT ID
    /// - Already OMFT format: "nep141:*.omft.near" -> returns as-is
    /// - Chain-specific: "eth:1:0x..." -> converts to OMFT ID
    pub async fn resolve_to_omft(
        &self,
        asset_or_token: &str,
        destination_chain: &str,
    ) -> Result<String, String> {
        // If already in OMFT format, return as-is
        if asset_or_token.contains(".omft.near") {
            // Extract just the OMFT token ID if it has nep141: prefix
            let token_id = asset_or_token
                .strip_prefix("nep141:")
                .unwrap_or(asset_or_token);
            return Ok(token_id.to_string());
        }

        // If in defuse format (eth:1:0x...), convert to OMFT
        if asset_or_token.contains(':') {
            let parts: Vec<&str> = asset_or_token.split(':').collect();
            if parts.len() == 3 {
                let chain_type = parts[0];
                let address = parts[2];
                if address == "native" {
                    return Ok(format!("{}.omft.near", chain_type));
                }
                return Ok(format!("{}-{}.omft.near", chain_type, address));
            }
        }

        // Otherwise, look up asset name
        self.get_omft_token_id(asset_or_token, destination_chain)
            .await
    }

    /// Get bridge client reference for direct API calls
    pub fn bridge_client(&self) -> &BridgeClient {
        &self.bridge_client
    }
}

/// OMFT token ID utilities
pub struct OmftTokenId;

impl OmftTokenId {
    /// Parse OMFT token ID into components
    ///
    /// # Example
    /// ```
    /// use templar_funding_bridge::tokens::OmftTokenId;
    /// let (chain, address) = OmftTokenId::parse("eth-0xdac17f958d2ee523a2206206994597c13d831ec7.omft.near").unwrap();
    /// assert_eq!(chain, "eth");
    /// assert_eq!(address, "0xdac17f958d2ee523a2206206994597c13d831ec7");
    /// ```
    pub fn parse(token_id: &str) -> Option<(String, String)> {
        // Remove .omft.near suffix
        let base = token_id.strip_suffix(".omft.near")?;

        // Check if it's a native token (no dash)
        if !base.contains('-') {
            // Native token like "eth.omft.near"
            return Some((base.to_string(), "native".to_string()));
        }

        // ERC-20 token like "eth-0x....omft.near"
        let dash_pos = base.find('-')?;
        let chain = &base[..dash_pos];
        let address = &base[dash_pos + 1..];

        Some((chain.to_string(), address.to_string()))
    }

    /// Check if token ID is a native token
    pub fn is_native(token_id: &str) -> bool {
        if let Some((_, address)) = Self::parse(token_id) {
            address == "native"
        } else {
            false
        }
    }

    /// Build OMFT token ID from chain and address
    pub fn build(chain_type: &str, address: &str) -> String {
        if address == "native" {
            format!("{}.omft.near", chain_type)
        } else {
            format!("{}-{}.omft.near", chain_type, address)
        }
    }
}

/// Token decimal utilities
pub struct TokenDecimals;

impl TokenDecimals {
    /// Get default decimals for common assets
    pub fn default_for_asset(asset: &str) -> u8 {
        match asset.to_lowercase().as_str() {
            "usdc" | "usdt" | "dai" => 6,
            "eth" | "weth" => 18,
            "wbtc" | "btc" => 8,
            "near" => 24,
            "sol" | "wsol" => 9,
            _ => 18, // Default to 18 for unknown tokens
        }
    }

    /// Convert human-readable amount to smallest units
    ///
    /// # Example
    /// ```
    /// use templar_funding_bridge::tokens::TokenDecimals;
    /// let amount = TokenDecimals::to_smallest_units(1.5, 6); // 1.5 USDC
    /// assert_eq!(amount, Some(1_500_000));
    /// ```
    pub fn to_smallest_units(amount: f64, decimals: u8) -> Option<u128> {
        if amount < 0.0 || !amount.is_finite() {
            return None;
        }

        let multiplier = 10_u128.checked_pow(decimals.into())?;
        let smallest = (amount * multiplier as f64).round();

        if smallest < 0.0 || smallest > u128::MAX as f64 {
            return None;
        }

        Some(smallest as u128)
    }

    /// Convert smallest units to human-readable amount
    ///
    /// # Example
    /// ```
    /// use templar_funding_bridge::tokens::TokenDecimals;
    /// let amount = TokenDecimals::to_human_readable(1_500_000, 6);
    /// assert_eq!(amount, 1.5); // 1.5 USDC
    /// ```
    pub fn to_human_readable(smallest_units: u128, decimals: u8) -> f64 {
        let divisor = 10_u128.pow(decimals.into());
        smallest_units as f64 / divisor as f64
    }

    /// Format amount as string with proper decimal places
    ///
    /// # Example
    /// ```
    /// use templar_funding_bridge::tokens::TokenDecimals;
    /// let formatted = TokenDecimals::format_amount(1_500_000, 6, "USDC");
    /// assert_eq!(formatted, "1.500000 USDC");
    /// ```
    pub fn format_amount(smallest_units: u128, decimals: u8, symbol: &str) -> String {
        let human = Self::to_human_readable(smallest_units, decimals);
        format!("{:.width$} {}", human, symbol, width = decimals as usize)
    }

    /// Parse a string amount to smallest units
    ///
    /// Handles both integer and decimal formats
    pub fn parse_amount(amount_str: &str, decimals: u8) -> Result<u128, String> {
        // Check if it's already in smallest units (no decimal point)
        if !amount_str.contains('.') {
            return amount_str
                .parse::<u128>()
                .map_err(|e| format!("Invalid amount: {}", e));
        }

        // Parse as decimal
        let amount: f64 = amount_str
            .parse()
            .map_err(|e| format!("Invalid decimal: {}", e))?;

        Self::to_smallest_units(amount, decimals).ok_or_else(|| "Amount overflow".to_string())
    }

    /// Convert between different decimal precisions
    ///
    /// Useful for converting between chains with different decimal standards
    pub fn convert_decimals(amount: u128, from_decimals: u8, to_decimals: u8) -> Option<u128> {
        if from_decimals == to_decimals {
            return Some(amount);
        }

        if from_decimals > to_decimals {
            // Reduce precision (divide)
            let diff = from_decimals - to_decimals;
            let divisor = 10_u128.checked_pow(diff.into())?;
            Some(amount / divisor)
        } else {
            // Increase precision (multiply)
            let diff = to_decimals - from_decimals;
            let multiplier = 10_u128.checked_pow(diff.into())?;
            amount.checked_mul(multiplier)
        }
    }
}

/// Common token addresses on different chains
pub struct TokenAddresses;

impl TokenAddresses {
    /// Get USDC contract address for a chain
    pub fn usdc(chain: &ChainId) -> Option<&'static str> {
        match (chain.chain_type.as_str(), chain.chain_id.as_str()) {
            ("eth", "1") => Some("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"),
            ("eth", "42161") => Some("0xaf88d065e77c8cc2239327c5edb3a432268e5831"), // Arbitrum
            ("eth", "8453") => Some("0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"),  // Base
            ("eth", "10") => Some("0x0b2c639c533813f4aa9d7837caf62653d097ff85"),    // Optimism
            ("eth", "137") => Some("0x3c499c542cef5e3811e1192ce70d8cc03d5c3359"),   // Polygon
            ("sol", _) => Some("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"),     // Solana USDC
            _ => None,
        }
    }

    /// Get USDT contract address for a chain
    pub fn usdt(chain: &ChainId) -> Option<&'static str> {
        match (chain.chain_type.as_str(), chain.chain_id.as_str()) {
            ("eth", "1") => Some("0xdac17f958d2ee523a2206206994597c13d831ec7"),
            ("eth", "42161") => Some("0xfd086bc7cd5c481dcc9c85ebe478a1c0b69fcbb9"), // Arbitrum
            ("eth", "10") => Some("0x94b008aa00579c1307b0ef2c499ad98a8ce58e58"),    // Optimism
            ("eth", "137") => Some("0xc2132d05d31c914a87c6611c10748aeb04b58e8f"),   // Polygon
            ("sol", _) => Some("Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"),     // Solana USDT
            _ => None,
        }
    }

    /// Get WETH contract address for a chain
    pub fn weth(chain: &ChainId) -> Option<&'static str> {
        match (chain.chain_type.as_str(), chain.chain_id.as_str()) {
            ("eth", "1") => Some("0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"),
            ("eth", "42161") => Some("0x82af49447d8a07e3bd95bd0d56f35241523fbab1"), // Arbitrum
            ("eth", "8453") => Some("0x4200000000000000000000000000000000000006"),  // Base
            ("eth", "10") => Some("0x4200000000000000000000000000000000000006"),    // Optimism
            _ => None,
        }
    }

    /// Get WBTC contract address for a chain
    pub fn wbtc(chain: &ChainId) -> Option<&'static str> {
        match (chain.chain_type.as_str(), chain.chain_id.as_str()) {
            ("eth", "1") => Some("0x2260fac5e5542a773aa44fbcfedf7c193bc2c599"),
            ("eth", "42161") => Some("0x2f2a2543b76a4166549f7aab2e75bef0aefc5b0f"), // Arbitrum
            _ => None,
        }
    }

    /// Get Wrapped SOL token address
    pub fn wsol(chain: &ChainId) -> Option<&'static str> {
        match (chain.chain_type.as_str(), chain.chain_id.as_str()) {
            ("sol", _) => Some("So11111111111111111111111111111111111111112"), // Wrapped SOL
            _ => None,
        }
    }

    /// Build defuse asset identifier
    pub fn defuse_asset_id(chain: &ChainId, token_address: &str) -> String {
        format!("{}:{}:{}", chain.chain_type, chain.chain_id, token_address)
    }

    /// Build defuse asset identifier for native token
    pub fn defuse_native_id(chain: &ChainId) -> String {
        format!("{}:{}:native", chain.chain_type, chain.chain_id)
    }
}

/// Amount validation utilities
pub struct AmountValidator;

impl AmountValidator {
    /// Validate that amount meets minimum requirements
    pub fn check_minimum(
        amount: u128,
        min_amount: Option<&str>,
        decimals: u8,
    ) -> Result<(), String> {
        if let Some(min_str) = min_amount {
            let min: u128 = min_str.parse().map_err(|_| "Invalid min amount format")?;
            if amount < min {
                let human_amount = TokenDecimals::to_human_readable(amount, decimals);
                let human_min = TokenDecimals::to_human_readable(min, decimals);
                return Err(format!(
                    "Amount {} is below minimum {}",
                    human_amount, human_min
                ));
            }
        }
        Ok(())
    }

    /// Validate amount is non-zero
    pub fn check_non_zero(amount: u128) -> Result<(), String> {
        if amount == 0 {
            return Err("Amount must be greater than zero".to_string());
        }
        Ok(())
    }

    /// Validate amount doesn't exceed maximum
    pub fn check_maximum(amount: u128, max_amount: u128) -> Result<(), String> {
        if amount > max_amount {
            return Err(format!("Amount {} exceeds maximum {}", amount, max_amount));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_smallest_units() {
        // USDC (6 decimals)
        assert_eq!(TokenDecimals::to_smallest_units(1.0, 6), Some(1_000_000));
        assert_eq!(TokenDecimals::to_smallest_units(1.5, 6), Some(1_500_000));
        assert_eq!(
            TokenDecimals::to_smallest_units(100.25, 6),
            Some(100_250_000)
        );

        // ETH (18 decimals)
        assert_eq!(
            TokenDecimals::to_smallest_units(1.0, 18),
            Some(1_000_000_000_000_000_000)
        );
        assert_eq!(
            TokenDecimals::to_smallest_units(0.1, 18),
            Some(100_000_000_000_000_000)
        );

        // Edge cases
        assert_eq!(TokenDecimals::to_smallest_units(0.0, 6), Some(0));
        assert_eq!(TokenDecimals::to_smallest_units(-1.0, 6), None);
        assert_eq!(TokenDecimals::to_smallest_units(f64::INFINITY, 6), None);
    }

    #[test]
    fn test_to_human_readable() {
        assert_eq!(TokenDecimals::to_human_readable(1_000_000, 6), 1.0);
        assert_eq!(TokenDecimals::to_human_readable(1_500_000, 6), 1.5);
        assert_eq!(
            TokenDecimals::to_human_readable(1_000_000_000_000_000_000, 18),
            1.0
        );
    }

    #[test]
    fn test_format_amount() {
        let formatted = TokenDecimals::format_amount(1_500_000, 6, "USDC");
        assert!(formatted.contains("1.5"));
        assert!(formatted.contains("USDC"));
    }

    #[test]
    fn test_parse_amount() {
        // Integer format (already in smallest units)
        assert_eq!(TokenDecimals::parse_amount("1000000", 6), Ok(1_000_000));

        // Decimal format
        assert_eq!(TokenDecimals::parse_amount("1.5", 6), Ok(1_500_000));
        assert_eq!(TokenDecimals::parse_amount("100.25", 6), Ok(100_250_000));

        // Invalid
        assert!(TokenDecimals::parse_amount("invalid", 6).is_err());
    }

    #[test]
    fn test_convert_decimals() {
        // Same decimals
        assert_eq!(
            TokenDecimals::convert_decimals(1_000_000, 6, 6),
            Some(1_000_000)
        );

        // Reduce precision (6 -> 4)
        assert_eq!(
            TokenDecimals::convert_decimals(1_500_000, 6, 4),
            Some(15_000)
        );

        // Increase precision (6 -> 8)
        assert_eq!(
            TokenDecimals::convert_decimals(1_500_000, 6, 8),
            Some(150_000_000)
        );

        // ETH to USDC decimals (18 -> 6)
        let eth_amount = 1_000_000_000_000_000_000_u128; // 1 ETH
        let usdc_equiv = TokenDecimals::convert_decimals(eth_amount, 18, 6);
        assert_eq!(usdc_equiv, Some(1_000_000)); // 1.0 in USDC decimals
    }

    #[test]
    fn test_default_decimals() {
        assert_eq!(TokenDecimals::default_for_asset("USDC"), 6);
        assert_eq!(TokenDecimals::default_for_asset("usdc"), 6);
        assert_eq!(TokenDecimals::default_for_asset("ETH"), 18);
        assert_eq!(TokenDecimals::default_for_asset("wbtc"), 8);
        assert_eq!(TokenDecimals::default_for_asset("NEAR"), 24);
        assert_eq!(TokenDecimals::default_for_asset("SOL"), 9);
        assert_eq!(TokenDecimals::default_for_asset("unknown"), 18);
    }

    #[test]
    fn test_usdc_addresses() {
        let eth = ChainId::ethereum_mainnet();
        assert_eq!(
            TokenAddresses::usdc(&eth),
            Some("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48")
        );

        let arb = ChainId::arbitrum();
        assert_eq!(
            TokenAddresses::usdc(&arb),
            Some("0xaf88d065e77c8cc2239327c5edb3a432268e5831")
        );

        let base = ChainId::base();
        assert_eq!(
            TokenAddresses::usdc(&base),
            Some("0x833589fcd6edb6e08f4c7c32d4f71b54bda02913")
        );
    }

    #[test]
    fn test_defuse_asset_id() {
        let eth = ChainId::ethereum_mainnet();
        let usdc_addr = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48";

        let id = TokenAddresses::defuse_asset_id(&eth, usdc_addr);
        assert_eq!(id, "eth:1:0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48");

        let native_id = TokenAddresses::defuse_native_id(&eth);
        assert_eq!(native_id, "eth:1:native");
    }

    #[test]
    fn test_amount_validator_minimum() {
        // Valid amount
        assert!(AmountValidator::check_minimum(1_000_000, Some("500000"), 6).is_ok());

        // Below minimum
        assert!(AmountValidator::check_minimum(100_000, Some("500000"), 6).is_err());

        // No minimum
        assert!(AmountValidator::check_minimum(1, None, 6).is_ok());
    }

    #[test]
    fn test_amount_validator_non_zero() {
        assert!(AmountValidator::check_non_zero(1).is_ok());
        assert!(AmountValidator::check_non_zero(0).is_err());
    }

    #[test]
    fn test_amount_validator_maximum() {
        assert!(AmountValidator::check_maximum(100, 1000).is_ok());
        assert!(AmountValidator::check_maximum(1500, 1000).is_err());
    }

    #[test]
    fn test_omft_token_id_parse() {
        // Native ETH token
        let (chain, address) = OmftTokenId::parse("eth.omft.near").unwrap();
        assert_eq!(chain, "eth");
        assert_eq!(address, "native");

        // ERC-20 token
        let (chain, address) =
            OmftTokenId::parse("eth-0xdac17f958d2ee523a2206206994597c13d831ec7.omft.near").unwrap();
        assert_eq!(chain, "eth");
        assert_eq!(address, "0xdac17f958d2ee523a2206206994597c13d831ec7");

        // Invalid format
        assert!(OmftTokenId::parse("invalid").is_none());
        assert!(OmftTokenId::parse("eth.somewhere.near").is_none());
    }

    #[test]
    fn test_omft_token_id_is_native() {
        assert!(OmftTokenId::is_native("eth.omft.near"));
        assert!(OmftTokenId::is_native("sol.omft.near"));
        assert!(!OmftTokenId::is_native(
            "eth-0xdac17f958d2ee523a2206206994597c13d831ec7.omft.near"
        ));
    }

    #[test]
    fn test_omft_token_id_build() {
        // Native token
        assert_eq!(OmftTokenId::build("eth", "native"), "eth.omft.near");

        // ERC-20 token
        assert_eq!(
            OmftTokenId::build("eth", "0xdac17f958d2ee523a2206206994597c13d831ec7"),
            "eth-0xdac17f958d2ee523a2206206994597c13d831ec7.omft.near"
        );
    }

    #[tokio::test]
    async fn test_token_registry_resolve_to_omft() {
        let bridge_client = BridgeClient::new("https://test.api".to_string());
        let bridge_client_arc = Arc::new(bridge_client);
        let registry = TokenRegistry::new(bridge_client_arc);

        // Already OMFT format
        let result = registry
            .resolve_to_omft("eth.omft.near", "eth:1")
            .await
            .unwrap();
        assert_eq!(result, "eth.omft.near");

        // With nep141: prefix
        let result = registry
            .resolve_to_omft(
                "nep141:eth-0xdac17f958d2ee523a2206206994597c13d831ec7.omft.near",
                "eth:1",
            )
            .await
            .unwrap();
        assert_eq!(
            result,
            "eth-0xdac17f958d2ee523a2206206994597c13d831ec7.omft.near"
        );

        // Defuse format - native
        let result = registry
            .resolve_to_omft("eth:1:native", "eth:1")
            .await
            .unwrap();
        assert_eq!(result, "eth.omft.near");

        // Defuse format - ERC-20
        let result = registry
            .resolve_to_omft("eth:1:0xdac17f958d2ee523a2206206994597c13d831ec7", "eth:1")
            .await
            .unwrap();
        assert_eq!(
            result,
            "eth-0xdac17f958d2ee523a2206206994597c13d831ec7.omft.near"
        );
    }

    #[tokio::test]
    async fn test_token_registry_get_omft_token_id_fallback() {
        let bridge_client = BridgeClient::new("https://test.api".to_string());
        let bridge_client_arc = Arc::new(bridge_client);
        let registry = TokenRegistry::new(bridge_client_arc);

        // USDT on Ethereum (uses fallback address)
        let result = registry.get_omft_token_id("USDT", "eth:1").await.unwrap();
        assert_eq!(
            result,
            "eth-0xdac17f958d2ee523a2206206994597c13d831ec7.omft.near"
        );

        // USDC on Arbitrum
        let result = registry
            .get_omft_token_id("USDC", "eth:42161")
            .await
            .unwrap();
        assert_eq!(
            result,
            "eth-0xaf88d065e77c8cc2239327c5edb3a432268e5831.omft.near"
        );

        // Native ETH
        let result = registry.get_omft_token_id("ETH", "eth:1").await.unwrap();
        assert_eq!(result, "eth.omft.near");

        // Unknown token should fail
        let result = registry.get_omft_token_id("UNKNOWN", "eth:1").await;
        assert!(result.is_err());
    }
}
