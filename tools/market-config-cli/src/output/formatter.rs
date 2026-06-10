use crate::CliResult;
use std::path::Path;
use templar_common::market::MarketConfiguration;

pub struct ConfigFormatter;

impl ConfigFormatter {
    pub fn new() -> Self {
        Self
    }

    /// Format configuration as pretty-printed JSON
    /// # Errors
    pub fn to_json(&self, config: &MarketConfiguration) -> CliResult<String> {
        Ok(serde_json::to_string_pretty(config)?)
    }

    /// Format configuration as compact JSON
    /// # Errors
    pub fn to_json_compact(&self, config: &MarketConfiguration) -> CliResult<String> {
        Ok(serde_json::to_string(config)?)
    }

    /// Write configuration to a file as JSON
    /// # Errors
    pub fn write_to_file(&self, config: &MarketConfiguration, path: &Path) -> CliResult<()> {
        let json = self.to_json(config)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Format configuration as a human-readable summary
    pub fn to_summary(&self, config: &MarketConfiguration) -> String {
        format!(
            r"
Market Configuration Summary
============================

Assets:
  Borrow:     {}
  Collateral: {}

Oracle:
  Contract:   {}
  Max Age:    {}s

Risk Parameters:
  Maintenance MCR:    {}
  Liquidation MCR:    {}
  Max Usage Ratio:    {}
  Liquidation Spread: {}

Interest Rate:
  Strategy: {}

Ranges:
  Borrow:     {} - {}
  Supply:     {} - {}
  Withdrawal: {} - {}

Yield Distribution:
  Supply Weight: {}
  Total Weight:  {}
",
            config.borrow_asset.contract_id(),
            config.collateral_asset.contract_id(),
            config.price_oracle_configuration.account_id,
            config.price_oracle_configuration.price_maximum_age_s,
            config.borrow_mcr_maintenance,
            config.borrow_mcr_liquidation,
            config.borrow_asset_maximum_usage_ratio,
            config.liquidation_maximum_spread,
            match &config.borrow_interest_rate_strategy {
                templar_common::interest_rate_strategy::InterestRateStrategy::Linear { .. } =>
                    "Linear",
                templar_common::interest_rate_strategy::InterestRateStrategy::Piecewise {
                    ..
                } => "Piecewise",
                templar_common::interest_rate_strategy::InterestRateStrategy::Exponential2 {
                    ..
                } => "Exponential2",
            },
            config.borrow_range.minimum,
            config
                .borrow_range
                .maximum
                .map_or_else(|| "unlimited".to_string(), |m| m.to_string()),
            config.supply_range.minimum,
            config
                .supply_range
                .maximum
                .map_or_else(|| "unlimited".to_string(), |m| m.to_string()),
            config.supply_withdrawal_range.minimum,
            config
                .supply_withdrawal_range
                .maximum
                .map_or_else(|| "unlimited".to_string(), |m| m.to_string()),
            config.yield_weights.supply.get(),
            config.yield_weights.total_weight().get(),
        )
    }

    /// Display configuration to stdout
    /// # Errors
    pub fn display(&self, config: &MarketConfiguration) -> CliResult<()> {
        println!("{}", self.to_summary(config));
        Ok(())
    }
}

impl Default for ConfigFormatter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::AccountId;
    use std::str::FromStr;
    use tempfile::NamedTempFile;
    use templar_common::{
        asset::FungibleAsset,
        fee::{Fee, TimeBasedFee},
        interest_rate_strategy::InterestRateStrategy,
        market::{PriceOracleConfiguration, YieldWeights},
        oracle::pyth::PriceIdentifier,
        time_chunk::TimeChunkConfiguration,
        Decimal,
    };

    fn create_test_config() -> MarketConfiguration {
        MarketConfiguration {
            time_chunk_configuration: TimeChunkConfiguration::new(600_000),
            borrow_asset: FungibleAsset::nep141(AccountId::from_str("usdc.near").unwrap()),
            collateral_asset: FungibleAsset::nep141(AccountId::from_str("wnear.near").unwrap()),
            price_oracle_configuration: PriceOracleConfiguration {
                account_id: AccountId::from_str("pyth-oracle.near").unwrap(),
                collateral_asset_price_id: PriceIdentifier([0xaa; 32]),
                collateral_asset_decimals: 24,
                borrow_asset_price_id: PriceIdentifier([0xbb; 32]),
                borrow_asset_decimals: 6,
                price_maximum_age_s: 60,
            },
            borrow_mcr_maintenance: Decimal::from(125u32) / 100u32,
            borrow_mcr_liquidation: Decimal::from(120u32) / 100u32,
            borrow_asset_maximum_usage_ratio: Decimal::from(99u32) / 100u32,
            borrow_origination_fee: Fee::zero(),
            borrow_interest_rate_strategy: InterestRateStrategy::linear(
                Decimal::from(5u32) / 100u32,
                Decimal::from(10u32) / 100u32,
            )
            .unwrap(),
            borrow_maximum_duration_ms: None,
            borrow_range: (1_000_000, None).try_into().unwrap(),
            supply_range: (1_000_000, None).try_into().unwrap(),
            supply_withdrawal_range: (1_000_000, None).try_into().unwrap(),
            supply_withdrawal_fee: TimeBasedFee::zero(),
            yield_weights: YieldWeights::new_with_supply_weight(10),
            protocol_account_id: AccountId::from_str("protocol.near").unwrap(),
            liquidation_maximum_spread: Decimal::from(5u32) / 100u32,
        }
    }

    #[test]
    fn test_to_json() {
        let formatter = ConfigFormatter::new();
        let config = create_test_config();
        let json = formatter.to_json(&config).unwrap();
        assert!(json.contains("borrow_asset"));
        assert!(json.contains("collateral_asset"));
    }

    #[test]
    fn test_to_json_compact() {
        let formatter = ConfigFormatter::new();
        let config = create_test_config();
        let json = formatter.to_json_compact(&config).unwrap();
        assert!(!json.contains('\n')); // Compact format should not have newlines
    }

    #[test]
    fn test_write_to_file() {
        let formatter = ConfigFormatter::new();
        let config = create_test_config();
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path();

        formatter.write_to_file(&config, path).unwrap();

        let content = std::fs::read_to_string(path).unwrap();
        assert!(content.contains("borrow_asset"));
    }

    #[test]
    fn test_to_summary() {
        let formatter = ConfigFormatter::new();
        let config = create_test_config();
        let summary = formatter.to_summary(&config);
        assert!(summary.contains("usdc.near"));
        assert!(summary.contains("wnear.near"));
        assert!(summary.contains("Linear"));
    }
}
