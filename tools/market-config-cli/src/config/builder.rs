use crate::{CliError, CliResult};
use hex;
use near_sdk::{json_types::U64, AccountId};
use std::{fmt::Display, str::FromStr};
use templar_common::{
    asset::{BorrowAsset, CollateralAsset, FungibleAsset},
    fee::{Fee, TimeBasedFee},
    interest_rate_strategy::InterestRateStrategy,
    market::{MarketConfiguration, PriceOracleConfiguration, ValidAmountRange, YieldWeights},
    number::Decimal,
    oracle::pyth::PriceIdentifier,
    time_chunk::TimeChunkConfiguration,
};

#[derive(Debug, Clone)]
/// Builder for creating ``MarketConfiguration`` instances
pub struct ConfigBuilder {
    time_chunk_duration_ms: Option<u64>,
    borrow_asset: Option<FungibleAsset<BorrowAsset>>,
    collateral_asset: Option<FungibleAsset<CollateralAsset>>,
    oracle_account_id: Option<AccountId>,
    collateral_price_id: Option<PriceIdentifier>,
    collateral_decimals: Option<i32>,
    borrow_price_id: Option<PriceIdentifier>,
    borrow_decimals: Option<i32>,
    price_max_age_s: Option<u32>,
    borrow_mcr_maintenance: Option<Decimal>,
    borrow_mcr_liquidation: Option<Decimal>,
    borrow_max_usage_ratio: Option<Decimal>,
    borrow_origination_fee: Option<Fee<BorrowAsset>>,
    borrow_interest_rate_strategy: Option<InterestRateStrategy>,
    borrow_max_duration_ms: Option<U64>,
    borrow_range: Option<ValidAmountRange<BorrowAsset>>,
    supply_range: Option<ValidAmountRange<BorrowAsset>>,
    supply_withdrawal_range: Option<ValidAmountRange<BorrowAsset>>,
    supply_withdrawal_fee: Option<TimeBasedFee<BorrowAsset>>,
    yield_weights: Option<YieldWeights>,
    protocol_account_id: Option<AccountId>,
    liquidation_max_spread: Option<Decimal>,
}

impl Display for ConfigBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ConfigBuilder {{ ... }}")
    }
}
impl Default for ConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigBuilder {
    pub fn new() -> Self {
        Self {
            time_chunk_duration_ms: None,
            borrow_asset: None,
            collateral_asset: None,
            oracle_account_id: None,
            collateral_price_id: None,
            collateral_decimals: None,
            borrow_price_id: None,
            borrow_decimals: None,
            price_max_age_s: None,
            borrow_mcr_maintenance: None,
            borrow_mcr_liquidation: None,
            borrow_max_usage_ratio: None,
            borrow_origination_fee: None,
            borrow_interest_rate_strategy: None,
            borrow_max_duration_ms: None,
            borrow_range: None,
            supply_range: None,
            supply_withdrawal_range: None,
            supply_withdrawal_fee: None,
            yield_weights: None,
            protocol_account_id: None,
            liquidation_max_spread: None,
        }
    }

    #[must_use]
    pub fn time_chunk_duration_ms(mut self, duration_ms: u64) -> Self {
        self.time_chunk_duration_ms = Some(duration_ms);
        self
    }

    pub fn borrow_asset_ref(&self) -> Option<&FungibleAsset<BorrowAsset>> {
        self.borrow_asset.as_ref()
    }

    pub fn time_chunk_duration_ms_value(&self) -> Option<u64> {
        self.time_chunk_duration_ms
    }

    pub fn price_max_age_s_value(&self) -> Option<u32> {
        self.price_max_age_s
    }

    pub fn borrow_mcr_maintenance_value(&self) -> Option<Decimal> {
        self.borrow_mcr_maintenance
    }

    pub fn borrow_mcr_liquidation_value(&self) -> Option<Decimal> {
        self.borrow_mcr_liquidation
    }

    pub fn borrow_max_usage_ratio_value(&self) -> Option<Decimal> {
        self.borrow_max_usage_ratio
    }

    pub fn liquidation_max_spread_value(&self) -> Option<Decimal> {
        self.liquidation_max_spread
    }

    /// # Errors
    pub fn borrow_fungible_asset(mut self, asset: FungibleAsset<BorrowAsset>) -> CliResult<Self> {
        self.borrow_asset = Some(asset);
        Ok(self)
    }

    /// # Errors
    pub fn borrow_asset(self, account_id: &str) -> CliResult<Self> {
        let account_id = AccountId::from_str(account_id)
            .map_err(|e| CliError::InvalidInput(format!("Invalid borrow asset: {e}")))?;
        self.borrow_fungible_asset(FungibleAsset::nep141(account_id))
    }

    /// # Errors
    pub fn collateral_fungible_asset(
        mut self,
        asset: FungibleAsset<CollateralAsset>,
    ) -> CliResult<Self> {
        self.collateral_asset = Some(asset);
        Ok(self)
    }

    pub fn collateral_asset_ref(&self) -> Option<&FungibleAsset<CollateralAsset>> {
        self.collateral_asset.as_ref()
    }

    /// # Errors
    pub fn collateral_asset(self, account_id: &str) -> CliResult<Self> {
        let account_id = AccountId::from_str(account_id)
            .map_err(|e| CliError::InvalidInput(format!("Invalid collateral asset: {e}")))?;
        self.collateral_fungible_asset(FungibleAsset::nep141(account_id))
    }

    /// # Errors
    pub fn oracle_account_id(mut self, account_id: &str) -> CliResult<Self> {
        self.oracle_account_id = Some(
            AccountId::from_str(account_id)
                .map_err(|e| CliError::InvalidInput(format!("Invalid oracle account: {e}")))?,
        );
        Ok(self)
    }

    #[must_use]
    pub fn collateral_price_id(mut self, price_id: [u8; 32]) -> Self {
        self.collateral_price_id = Some(PriceIdentifier(price_id));
        self
    }

    #[must_use]
    pub fn collateral_decimals(mut self, decimals: i32) -> Self {
        self.collateral_decimals = Some(decimals);
        self
    }

    #[must_use]
    pub fn borrow_price_id(mut self, price_id: [u8; 32]) -> Self {
        self.borrow_price_id = Some(PriceIdentifier(price_id));
        self
    }

    #[must_use]
    pub fn borrow_decimals(mut self, decimals: i32) -> Self {
        self.borrow_decimals = Some(decimals);
        self
    }

    #[must_use]
    pub fn price_max_age_s(mut self, max_age: u32) -> Self {
        self.price_max_age_s = Some(max_age);
        self
    }

    pub fn price_oracle_inputs(
        &self,
    ) -> Option<(AccountId, PriceIdentifier, PriceIdentifier, i32, i32, u32)> {
        Some((
            self.oracle_account_id.clone()?,
            self.borrow_price_id?,
            self.collateral_price_id?,
            self.borrow_decimals?,
            self.collateral_decimals?,
            self.price_max_age_s?,
        ))
    }

    #[must_use]
    pub fn borrow_mcr_maintenance(mut self, mcr: Decimal) -> Self {
        self.borrow_mcr_maintenance = Some(mcr);
        self
    }

    #[must_use]
    pub fn borrow_mcr_liquidation(mut self, mcr: Decimal) -> Self {
        self.borrow_mcr_liquidation = Some(mcr);
        self
    }

    #[must_use]
    pub fn borrow_max_usage_ratio(mut self, ratio: Decimal) -> Self {
        self.borrow_max_usage_ratio = Some(ratio);
        self
    }

    #[must_use]
    pub fn borrow_origination_fee(mut self, fee: Fee<BorrowAsset>) -> Self {
        self.borrow_origination_fee = Some(fee);
        self
    }

    #[must_use]
    pub fn borrow_interest_rate_strategy(mut self, strategy: InterestRateStrategy) -> Self {
        self.borrow_interest_rate_strategy = Some(strategy);
        self
    }

    #[must_use]
    pub fn borrow_max_duration_ms(mut self, duration_ms: Option<u64>) -> Self {
        self.borrow_max_duration_ms = duration_ms.map(U64);
        self
    }

    /// # Errors
    pub fn borrow_range(mut self, min: u128, max: Option<u128>) -> CliResult<Self> {
        self.borrow_range = Some(
            (min, max)
                .try_into()
                .map_err(|e: std::io::Error| CliError::Validation(e.to_string()))?,
        );
        Ok(self)
    }

    /// # Errors
    pub fn supply_range(mut self, min: u128, max: Option<u128>) -> CliResult<Self> {
        self.supply_range = Some(
            (min, max)
                .try_into()
                .map_err(|e: std::io::Error| CliError::Validation(e.to_string()))?,
        );
        Ok(self)
    }

    /// # Errors
    pub fn supply_withdrawal_range(mut self, min: u128, max: Option<u128>) -> CliResult<Self> {
        self.supply_withdrawal_range = Some(
            (min, max)
                .try_into()
                .map_err(|e: std::io::Error| CliError::Validation(e.to_string()))?,
        );
        Ok(self)
    }

    /// # Errors
    #[must_use]
    pub fn supply_withdrawal_fee(mut self, fee: TimeBasedFee<BorrowAsset>) -> Self {
        self.supply_withdrawal_fee = Some(fee);
        self
    }

    #[must_use]
    pub fn yield_weights(mut self, weights: YieldWeights) -> Self {
        self.yield_weights = Some(weights);
        self
    }

    /// # Errors
    pub fn protocol_account_id(mut self, account_id: &str) -> CliResult<Self> {
        self.protocol_account_id = Some(
            AccountId::from_str(account_id)
                .map_err(|e| CliError::InvalidInput(format!("Invalid protocol account: {e}")))?,
        );
        Ok(self)
    }

    #[must_use]
    pub fn liquidation_max_spread(mut self, spread: Decimal) -> Self {
        self.liquidation_max_spread = Some(spread);
        self
    }

    /// # Errors
    pub fn build(self) -> CliResult<MarketConfiguration> {
        let config = MarketConfiguration {
            time_chunk_configuration: TimeChunkConfiguration::new(
                self.time_chunk_duration_ms.ok_or_else(|| {
                    CliError::Validation("time_chunk_duration_ms is required".into())
                })?,
            ),
            borrow_asset: self
                .borrow_asset
                .ok_or_else(|| CliError::Validation("borrow_asset is required".into()))?,
            collateral_asset: self
                .collateral_asset
                .ok_or_else(|| CliError::Validation("collateral_asset is required".into()))?,
            price_oracle_configuration: PriceOracleConfiguration {
                account_id: self
                    .oracle_account_id
                    .ok_or_else(|| CliError::Validation("oracle_account_id is required".into()))?,
                collateral_asset_price_id: self.collateral_price_id.ok_or_else(|| {
                    CliError::Validation("collateral_price_id is required".into())
                })?,
                collateral_asset_decimals: self.collateral_decimals.ok_or_else(|| {
                    CliError::Validation("collateral_decimals is required".into())
                })?,
                borrow_asset_price_id: self
                    .borrow_price_id
                    .ok_or_else(|| CliError::Validation("borrow_price_id is required".into()))?,
                borrow_asset_decimals: self
                    .borrow_decimals
                    .ok_or_else(|| CliError::Validation("borrow_decimals is required".into()))?,
                price_maximum_age_s: self
                    .price_max_age_s
                    .ok_or_else(|| CliError::Validation("price_max_age_s is required".into()))?,
            },
            borrow_mcr_maintenance: self
                .borrow_mcr_maintenance
                .ok_or_else(|| CliError::Validation("borrow_mcr_maintenance is required".into()))?,
            borrow_mcr_liquidation: self
                .borrow_mcr_liquidation
                .ok_or_else(|| CliError::Validation("borrow_mcr_liquidation is required".into()))?,
            borrow_asset_maximum_usage_ratio: self
                .borrow_max_usage_ratio
                .ok_or_else(|| CliError::Validation("borrow_max_usage_ratio is required".into()))?,
            borrow_origination_fee: self
                .borrow_origination_fee
                .ok_or_else(|| CliError::Validation("borrow_origination_fee is required".into()))?,
            borrow_interest_rate_strategy: self.borrow_interest_rate_strategy.ok_or_else(|| {
                CliError::Validation("borrow_interest_rate_strategy is required".into())
            })?,
            borrow_maximum_duration_ms: self.borrow_max_duration_ms,
            borrow_range: self
                .borrow_range
                .ok_or_else(|| CliError::Validation("borrow_range is required".into()))?,
            supply_range: self
                .supply_range
                .ok_or_else(|| CliError::Validation("supply_range is required".into()))?,
            supply_withdrawal_range: self.supply_withdrawal_range.ok_or_else(|| {
                CliError::Validation("supply_withdrawal_range is required".into())
            })?,
            supply_withdrawal_fee: self
                .supply_withdrawal_fee
                .ok_or_else(|| CliError::Validation("supply_withdrawal_fee is required".into()))?,
            yield_weights: self
                .yield_weights
                .ok_or_else(|| CliError::Validation("yield_weights is required".into()))?,
            protocol_account_id: self
                .protocol_account_id
                .ok_or_else(|| CliError::Validation("protocol_account_id is required".into()))?,
            liquidation_maximum_spread: self
                .liquidation_max_spread
                .ok_or_else(|| CliError::Validation("liquidation_max_spread is required".into()))?,
        };

        Ok(config)
    }

    #[must_use]
    pub fn overview_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();

        if let Some(duration) = self.time_chunk_duration_ms {
            lines.push(format!("Time chunk: {duration} ms"));
        }

        if let Some(asset) = &self.borrow_asset {
            lines.push(format!("Borrow asset: {asset}"));
        }

        if let Some(asset) = &self.collateral_asset {
            lines.push(format!("Collateral asset: {asset}"));
        }

        if let Some(id) = &self.oracle_account_id {
            lines.push(format!("Oracle account: {id}"));
        }

        if let Some(pid) = &self.borrow_price_id {
            lines.push(format!("Borrow price ID: 0x{}", hex::encode(pid.0)));
        }

        if let Some(pid) = &self.collateral_price_id {
            lines.push(format!("Collateral price ID: 0x{}", hex::encode(pid.0)));
        }

        if let Some(id) = &self.protocol_account_id {
            lines.push(format!("Protocol account: {id}"));
        }

        if let Some(range) = &self.borrow_range {
            lines.push(format!("Borrow range: min {}", range.minimum));
        }

        if let Some(range) = &self.supply_range {
            lines.push(format!("Supply range: min {}", range.minimum));
        }

        lines
    }
}
