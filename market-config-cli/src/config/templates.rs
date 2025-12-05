use serde::{Deserialize, Serialize};

/// Common template configurations for different types of markets
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
}
