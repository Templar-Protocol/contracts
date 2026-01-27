//! Configuration from environment variables.

use crate::error::{MonitorError, Result};
use near_sdk::AccountId;
use std::str::FromStr;
use templar_common::asset::{CollateralAsset, FungibleAsset};

#[derive(Debug, Clone)]
pub struct Config {
    // Network
    pub network: String,
    pub rpc_url: String,

    // Registry
    pub registry_account_ids: Vec<AccountId>,

    // Scheduling
    pub scan_time: String,

    // Alerts
    pub at_risk_threshold_percent: u16,
    pub min_position_size_usd: u64,

    // Telegram
    pub telegram_bot_token: String,
    pub telegram_channel_id: String,
    pub telegram_thread_id: Option<i64>,

    // Filtering
    pub ignored_collateral_assets: Vec<FungibleAsset<CollateralAsset>>,
    pub ignored_markets: Vec<AccountId>,
}

impl Config {
    /// Loads configuration from environment variables.
    ///
    /// # Errors
    /// Returns an error if required environment variables are missing or invalid.
    pub fn from_env() -> Result<Self> {
        dotenv::dotenv().ok();

        let network = std::env::var("NETWORK").unwrap_or_else(|_| "mainnet".to_string());
        let rpc_url = std::env::var("RPC_URL")
            .unwrap_or_else(|_| "https://free.rpc.fastnear.com".to_string());

        // Registry
        let registry_ids_str = std::env::var("REGISTRY_ACCOUNT_IDS")
            .map_err(|_| MonitorError::Config("REGISTRY_ACCOUNT_IDS not set".to_string()))?;
        let registry_account_ids = registry_ids_str
            .split(',')
            .map(|s| {
                AccountId::from_str(s.trim())
                    .map_err(|e| MonitorError::Config(format!("Invalid registry ID: {e}")))
            })
            .collect::<Result<Vec<_>>>()?;

        let scan_time = std::env::var("SCAN_TIME").unwrap_or_else(|_| "00:00".to_string());
        validate_scan_time(&scan_time)?;

        let at_risk_threshold_percent = std::env::var("AT_RISK_THRESHOLD_PERCENT")
            .or_else(|_| std::env::var("AT_RISK_ZONE_PERCENT"))
            .or_else(|_| std::env::var("YELLOW_ZONE_PERCENT"))
            .unwrap_or_else(|_| "10".to_string())
            .parse()
            .map_err(|e| MonitorError::Config(format!("Invalid AT_RISK_THRESHOLD_PERCENT: {e}")))?;

        let min_position_size_usd = std::env::var("MIN_POSITION_SIZE_USD")
            .unwrap_or_else(|_| "1000".to_string())
            .parse()
            .map_err(|e| MonitorError::Config(format!("Invalid MIN_POSITION_SIZE_USD: {e}")))?;

        let telegram_bot_token =
            std::env::var("TELEGRAM_BOT_TOKEN").unwrap_or_else(|_| String::new());
        let telegram_channel_id =
            std::env::var("TELEGRAM_CHANNEL_ID").unwrap_or_else(|_| String::new());
        let telegram_thread_id = std::env::var("TELEGRAM_THREAD_ID")
            .ok()
            .and_then(|s| s.parse::<i64>().ok());

        let ignored_collateral_assets =
            parse_asset_list(&std::env::var("IGNORED_COLLATERAL_ASSETS").unwrap_or_default())?;

        let ignored_markets_str = std::env::var("IGNORED_MARKETS").unwrap_or_default();
        let ignored_markets = if ignored_markets_str.is_empty() {
            Vec::new()
        } else {
            ignored_markets_str
                .split(',')
                .map(|s| {
                    AccountId::from_str(s.trim())
                        .map_err(|e| MonitorError::Config(format!("Invalid registry ID: {e}")))
                })
                .collect::<Result<Vec<_>>>()?
        };

        Ok(Config {
            network,
            rpc_url,
            registry_account_ids,
            scan_time,
            at_risk_threshold_percent,
            min_position_size_usd,
            telegram_bot_token,
            telegram_channel_id,
            telegram_thread_id,
            ignored_collateral_assets,
            ignored_markets,
        })
    }
}

fn parse_asset_list(assets_str: &str) -> Result<Vec<FungibleAsset<CollateralAsset>>> {
    if assets_str.is_empty() {
        return Ok(Vec::new());
    }

    assets_str
        .split(',')
        .map(|s| {
            FungibleAsset::from_str(s.trim())
                .map_err(|e| MonitorError::Config(format!("Invalid asset: {e}")))
        })
        .collect()
}

fn validate_scan_time(scan_time: &str) -> Result<()> {
    // Interval format (*/N)
    if scan_time.starts_with("*/") {
        let interval_str = scan_time.trim_start_matches("*/");
        let minutes = interval_str.parse::<u32>().map_err(|_| {
            MonitorError::Config(format!(
                "Invalid interval format '{scan_time}': expected */N where N is a positive number"
            ))
        })?;

        if minutes == 0 {
            return Err(MonitorError::Config(
                "Interval must be greater than 0".to_string(),
            ));
        }

        return Ok(());
    }

    // HH:MM format
    let parts: Vec<&str> = scan_time.split(':').collect();
    if parts.len() != 2 {
        return Err(MonitorError::Config(format!(
            "Invalid SCAN_TIME format '{scan_time}': expected HH:MM or */N"
        )));
    }

    let hour = parts[0].parse::<u32>().map_err(|_| {
        MonitorError::Config(format!("Invalid hour in '{scan_time}': expected 00-23"))
    })?;

    let minute = parts[1].parse::<u32>().map_err(|_| {
        MonitorError::Config(format!("Invalid minute in '{scan_time}': expected 00-59"))
    })?;

    if hour > 23 {
        return Err(MonitorError::Config(format!(
            "Hour must be 00-23, got {hour}"
        )));
    }

    if minute > 59 {
        return Err(MonitorError::Config(format!(
            "Minute must be 00-59, got {minute}"
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_scan_time_interval_valid() {
        assert!(validate_scan_time("*/5").is_ok());
        assert!(validate_scan_time("*/10").is_ok());
        assert!(validate_scan_time("*/60").is_ok());
        assert!(validate_scan_time("*/1").is_ok());
    }

    #[test]
    fn test_validate_scan_time_interval_zero() {
        let result = validate_scan_time("*/0");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must be greater than 0"));
    }

    #[test]
    fn test_validate_scan_time_interval_invalid() {
        assert!(validate_scan_time("*/abc").is_err());
        assert!(validate_scan_time("*/-5").is_err());
    }

    #[test]
    fn test_validate_scan_time_hhmm_valid() {
        assert!(validate_scan_time("00:00").is_ok());
        assert!(validate_scan_time("12:30").is_ok());
        assert!(validate_scan_time("23:59").is_ok());
        assert!(validate_scan_time("09:15").is_ok());
    }

    #[test]
    fn test_validate_scan_time_hhmm_invalid_hour() {
        let result = validate_scan_time("24:00");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("00-23"));

        assert!(validate_scan_time("25:30").is_err());
    }

    #[test]
    fn test_validate_scan_time_hhmm_invalid_minute() {
        let result = validate_scan_time("12:60");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("00-59"));

        assert!(validate_scan_time("12:99").is_err());
    }

    #[test]
    fn test_validate_scan_time_invalid_format() {
        assert!(validate_scan_time("invalid").is_err());
        assert!(validate_scan_time("12").is_err());
        assert!(validate_scan_time("12:30:45").is_err());
        assert!(validate_scan_time("").is_err());
    }

    #[test]
    fn test_parse_asset_list_empty() {
        let result = parse_asset_list("");
        assert!(result.is_ok());
        let assets = result.unwrap();
        assert_eq!(assets.len(), 0);
        assert!(assets.is_empty());
    }

    #[test]
    fn test_parse_asset_list_single() {
        let result = parse_asset_list("nep141:token.near");
        assert!(result.is_ok());
        let assets = result.unwrap();
        assert_eq!(assets.len(), 1);
    }

    #[test]
    fn test_parse_asset_list_multiple() {
        let result = parse_asset_list("nep141:token1.near,nep141:token2.near");
        assert!(result.is_ok());
        let assets = result.unwrap();
        assert_eq!(assets.len(), 2);
    }

    #[test]
    fn test_parse_asset_list_with_whitespace() {
        let result = parse_asset_list(" nep141:token1.near , nep141:token2.near ");
        assert!(result.is_ok());
        let assets = result.unwrap();
        assert_eq!(assets.len(), 2);
    }

    #[test]
    fn test_parse_asset_list_invalid() {
        let result = parse_asset_list("invalid-format");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid asset"));
    }

    #[test]
    fn test_parse_asset_list_mixed_valid_invalid() {
        // When one asset is invalid, the whole parse should fail
        let result = parse_asset_list("nep141:valid.near,invalid,nep141:also-valid.near");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_scan_time_edge_cases() {
        // Test maximum valid values
        assert!(validate_scan_time("23:59").is_ok());
        assert!(validate_scan_time("00:00").is_ok());

        // Test with leading zeros
        assert!(validate_scan_time("01:05").is_ok());
        assert!(validate_scan_time("09:09").is_ok());
    }

    #[test]
    fn test_validate_scan_time_boundary_values() {
        // Just at the boundary
        assert!(validate_scan_time("23:59").is_ok());
        assert!(validate_scan_time("24:00").is_err());

        assert!(validate_scan_time("12:59").is_ok());
        assert!(validate_scan_time("12:60").is_err());
    }

    #[test]
    fn test_validate_scan_time_malformed() {
        assert!(validate_scan_time("1:2:3:4").is_err());
        assert!(validate_scan_time(":").is_err());
        assert!(validate_scan_time("::").is_err());
        assert!(validate_scan_time("12:").is_err());
        assert!(validate_scan_time(":30").is_err());
    }

    #[test]
    fn test_validate_scan_time_interval_large() {
        assert!(validate_scan_time("*/1440").is_ok()); // 24 hours
        assert!(validate_scan_time("*/999999").is_ok());
    }

    #[test]
    fn test_validate_scan_time_non_numeric() {
        assert!(validate_scan_time("twelve:thirty").is_err());
        assert!(validate_scan_time("12:thirty").is_err());
        assert!(validate_scan_time("twelve:30").is_err());
    }
}
