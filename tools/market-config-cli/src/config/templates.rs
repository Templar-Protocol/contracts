use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::str::FromStr;
use templar_common::number::Decimal;

use crate::{CliError, CliResult, ConfigBuilder};

/// Common template configurations for different types of markets. These are arbitrary presets
/// meant to provide sensible defaults for various risk profiles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigTemplate {
    pub name: String,
    pub description: String,
    pub template_data: serde_json::Value,
}

impl ConfigTemplate {
    /// Conservative stablecoin lending market (e.g., USDC/USDT)
    pub fn conservative_stablecoin() -> Self {
        Self {
            name: "Conservative Stablecoin".into(),
            description: "Low-risk stablecoin lending with tight parameters".into(),
            template_data: serde_json::json!({
                "time_chunk_duration_ms": 600_000, // 10 minutes
                "borrow_mcr_maintenance": "1.05",
                "borrow_mcr_liquidation": "1.03",
                "borrow_asset_maximum_usage_ratio": "0.95",
                "liquidation_maximum_spread": "0.02",
                "price_maximum_age_s": 60,
            }),
        }
    }

    /// Standard crypto lending market (e.g., USDC/NEAR)
    pub fn standard_crypto() -> Self {
        Self {
            name: "Standard Crypto".into(),
            description: "Standard parameters for volatile crypto collateral".into(),
            template_data: serde_json::json!({
                "time_chunk_duration_ms": 600_000, // 10 minutes
                "borrow_mcr_maintenance": "1.25",
                "borrow_mcr_liquidation": "1.20",
                "borrow_asset_maximum_usage_ratio": "0.90",
                "liquidation_maximum_spread": "0.05",
                "price_maximum_age_s": 60,
            }),
        }
    }

    /// High volatility market (e.g., USDC/altcoin)
    pub fn high_volatility() -> Self {
        Self {
            name: "High Volatility".into(),
            description: "Conservative parameters for highly volatile collateral".into(),
            template_data: serde_json::json!({
                "time_chunk_duration_ms": 300_000, // 5 minutes
                "borrow_mcr_maintenance": "1.50",
                "borrow_mcr_liquidation": "1.40",
                "borrow_asset_maximum_usage_ratio": "0.80",
                "liquidation_maximum_spread": "0.10",
                "price_maximum_age_s": 30,
            }),
        }
    }

    /// List all available templates
    pub fn list_all() -> Vec<Self> {
        vec![
            Self::conservative_stablecoin(),
            Self::standard_crypto(),
            Self::high_volatility(),
        ]
    }

    /// Get template by name
    pub fn by_name(name: &str) -> Option<Self> {
        Self::list_all()
            .into_iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
    }

    /// Apply template defaults to a `ConfigBuilder`.
    /// # Errors
    pub fn apply_to_builder(&self, mut builder: ConfigBuilder) -> CliResult<ConfigBuilder> {
        let data = &self.template_data;

        if let Some(value) = read_u64(data, "time_chunk_duration_ms")? {
            builder = builder.time_chunk_duration_ms(value);
        }
        if let Some(value) = read_decimal(data, "borrow_mcr_maintenance")? {
            builder = builder.borrow_mcr_maintenance(value);
        }
        if let Some(value) = read_decimal(data, "borrow_mcr_liquidation")? {
            builder = builder.borrow_mcr_liquidation(value);
        }
        if let Some(value) = read_decimal(data, "borrow_asset_maximum_usage_ratio")? {
            builder = builder.borrow_max_usage_ratio(value);
        }
        if let Some(value) = read_decimal(data, "liquidation_maximum_spread")? {
            builder = builder.liquidation_max_spread(value);
        }
        if let Some(value) = read_u32(data, "price_maximum_age_s")? {
            builder = builder.price_max_age_s(value);
        }

        Ok(builder)
    }
}

fn read_u64(data: &Value, key: &str) -> CliResult<Option<u64>> {
    match data.get(key) {
        None => Ok(None),
        Some(Value::Number(num)) => num
            .as_u64()
            .ok_or_else(|| CliError::InvalidInput(format!("Invalid {key}: expected u64")))
            .map(Some),
        Some(Value::String(value)) => value
            .parse::<u64>()
            .map(Some)
            .map_err(|e| CliError::InvalidInput(format!("Invalid {key}: {e}"))),
        Some(_) => Err(CliError::InvalidInput(format!(
            "Invalid {key}: expected number or string"
        ))),
    }
}

fn read_u32(data: &Value, key: &str) -> CliResult<Option<u32>> {
    match data.get(key) {
        None => Ok(None),
        Some(Value::Number(num)) => num
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| CliError::InvalidInput(format!("Invalid {key}: expected u32")))
            .map(Some),
        Some(Value::String(value)) => value
            .parse::<u32>()
            .map(Some)
            .map_err(|e| CliError::InvalidInput(format!("Invalid {key}: {e}"))),
        Some(_) => Err(CliError::InvalidInput(format!(
            "Invalid {key}: expected number or string"
        ))),
    }
}

fn read_decimal(data: &Value, key: &str) -> CliResult<Option<Decimal>> {
    match data.get(key) {
        None => Ok(None),
        Some(Value::String(value)) => Decimal::from_str(value)
            .map(Some)
            .map_err(|e| CliError::InvalidInput(format!("Invalid {key}: {e}"))),
        Some(Value::Number(num)) => Decimal::from_str(&num.to_string())
            .map(Some)
            .map_err(|e| CliError::InvalidInput(format!("Invalid {key}: {e}"))),
        Some(_) => Err(CliError::InvalidInput(format!(
            "Invalid {key}: expected number or string"
        ))),
    }
}
