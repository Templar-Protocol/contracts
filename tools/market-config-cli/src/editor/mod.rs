use crate::{CliError, CliResult};
use dialoguer::{theme::ColorfulTheme, Confirm, Input, MultiSelect, Select};
use near_sdk::{
    json_types::{U128, U64},
    AccountId,
};
use std::str::FromStr;
use templar_common::{
    fee::{Fee, TimeBasedFee, TimeBasedFeeFunction},
    interest_rate_strategy::InterestRateStrategy,
    market::{MarketConfiguration, YieldWeights},
    time_chunk::TimeChunkConfiguration,
};

pub mod utils;
use utils::{
    fee_defaults, parse_asset_input, price_id_from_input, prompt_decimal, prompt_decimals,
    EditSection, StrategyDefaults, StrategyKind,
};

pub struct ConfigEditor<'a> {
    theme: &'a ColorfulTheme,
}

impl<'a> ConfigEditor<'a> {
    pub fn new(theme: &'a ColorfulTheme) -> Self {
        Self { theme }
    }

    /// Allow quick edits on a fetched or loaded configuration. Prompts are
    /// grouped so the user can jump straight to the sections they care about.
    /// # Errors
    pub fn edit(&self, mut config: MarketConfiguration) -> CliResult<MarketConfiguration> {
        println!("\n🧩 Select sections to edit (leave empty to keep as-is):");
        let sections = EditSection::ALL;

        let selections = MultiSelect::with_theme(self.theme)
            .with_prompt("Press space to pick sections, enter to continue")
            .items(sections)
            .defaults(&vec![false; sections.len()])
            .interact()
            .map_err(std::io::Error::other)?;

        if selections.is_empty() {
            println!("No edits selected. Keeping configuration as-is.");
            return Ok(config);
        }

        for selection in selections {
            match sections[selection] {
                EditSection::BasicConfiguration => self.edit_basic_config(&mut config)?,
                EditSection::OracleSettings => self.edit_oracle_config(&mut config)?,
                EditSection::RiskParameters => self.edit_risk_parameters(&mut config)?,
                EditSection::InterestRateStrategy => {
                    self.edit_interest_rate_strategy(&mut config)?;
                }
                EditSection::Ranges => self.edit_ranges(&mut config)?,
                EditSection::Fees => self.edit_fees(&mut config)?,
                EditSection::YieldDistribution => self.edit_yield_weights(&mut config)?,
            }
        }

        Ok(config)
    }

    fn edit_basic_config(&self, config: &mut MarketConfiguration) -> CliResult<()> {
        println!("\n📋 Basic Configuration");

        let time_chunk_ms: u64 = Input::with_theme(self.theme)
            .with_prompt("Time chunk duration (ms)")
            .default(config.time_chunk_configuration.duration_ms())
            .interact_text()
            .map_err(std::io::Error::other)?;
        config.time_chunk_configuration = TimeChunkConfiguration::new(time_chunk_ms);

        let borrow_asset_default = config.borrow_asset.to_string();
        let borrow_asset: String = Input::with_theme(self.theme)
            .with_prompt("Borrow asset (nep141:<id> or nep245:<id>:<token_id>)")
            .default(borrow_asset_default)
            .interact_text()
            .map_err(std::io::Error::other)?;
        config.borrow_asset = parse_asset_input(&borrow_asset, "borrow asset")?;

        let collateral_asset_default = config.collateral_asset.to_string();
        let collateral_asset: String = Input::with_theme(self.theme)
            .with_prompt("Collateral asset (nep141:<id> or nep245:<id>:<token_id>)")
            .default(collateral_asset_default)
            .interact_text()
            .map_err(std::io::Error::other)?;
        config.collateral_asset = parse_asset_input(&collateral_asset, "collateral asset")?;

        let protocol_account: String = Input::with_theme(self.theme)
            .with_prompt("Protocol account ID")
            .default(config.protocol_account_id.to_string())
            .interact_text()
            .map_err(std::io::Error::other)?;
        config.protocol_account_id = AccountId::from_str(&protocol_account)
            .map_err(|e| CliError::InvalidInput(format!("Invalid protocol account ID: {e}")))?;

        Ok(())
    }

    fn edit_oracle_config(&self, config: &mut MarketConfiguration) -> CliResult<()> {
        println!("\n🔮 Oracle Settings");

        let oracle_id: String = Input::with_theme(self.theme)
            .with_prompt("Oracle account ID")
            .default(config.price_oracle_configuration.account_id.to_string())
            .interact_text()
            .map_err(std::io::Error::other)?;
        config.price_oracle_configuration.account_id = AccountId::from_str(&oracle_id)
            .map_err(|e| CliError::InvalidInput(format!("Invalid oracle account ID: {e}")))?;

        let borrow_price_id_hex: String = Input::with_theme(self.theme)
            .with_prompt("Borrow asset Pyth price ID (64 hex chars)")
            .default(
                config
                    .price_oracle_configuration
                    .borrow_asset_price_id
                    .to_string(),
            )
            .interact_text()
            .map_err(std::io::Error::other)?;
        config.price_oracle_configuration.borrow_asset_price_id =
            price_id_from_input(&borrow_price_id_hex)?;

        let borrow_decimals: i32 = prompt_decimals(
            self.theme,
            "Borrow asset decimals",
            config.price_oracle_configuration.borrow_asset_decimals,
            "Borrow asset decimals",
        )?;
        config.price_oracle_configuration.borrow_asset_decimals = borrow_decimals;

        let collateral_price_id_hex: String = Input::with_theme(self.theme)
            .with_prompt("Collateral asset Pyth price ID (64 hex chars)")
            .default(
                config
                    .price_oracle_configuration
                    .collateral_asset_price_id
                    .to_string(),
            )
            .interact_text()
            .map_err(std::io::Error::other)?;
        config.price_oracle_configuration.collateral_asset_price_id =
            price_id_from_input(&collateral_price_id_hex)?;

        let collateral_decimals: i32 = prompt_decimals(
            self.theme,
            "Collateral asset decimals",
            config.price_oracle_configuration.collateral_asset_decimals,
            "Collateral asset decimals",
        )?;
        config.price_oracle_configuration.collateral_asset_decimals = collateral_decimals;

        let price_max_age: u32 = Input::with_theme(self.theme)
            .with_prompt("Maximum price age (seconds)")
            .default(config.price_oracle_configuration.price_maximum_age_s)
            .interact_text()
            .map_err(std::io::Error::other)?;
        config.price_oracle_configuration.price_maximum_age_s = price_max_age;

        Ok(())
    }

    fn edit_risk_parameters(&self, config: &mut MarketConfiguration) -> CliResult<()> {
        println!("\n⚖️  Risk Parameters");

        let mcr_maintenance = prompt_decimal(
            self.theme,
            "Maintenance collateralization ratio (e.g., 1.25 for 125%)",
            &config.borrow_mcr_maintenance.to_string(),
            "maintenance collateralization ratio",
        )?;
        config.borrow_mcr_maintenance = mcr_maintenance;

        let mcr_liquidation = prompt_decimal(
            self.theme,
            "Liquidation collateralization ratio (e.g., 1.20 for 120%)",
            &config.borrow_mcr_liquidation.to_string(),
            "liquidation collateralization ratio",
        )?;
        config.borrow_mcr_liquidation = mcr_liquidation;

        let max_usage = prompt_decimal(
            self.theme,
            "Maximum usage ratio (e.g., 0.90 for 90%)",
            &config.borrow_asset_maximum_usage_ratio.to_string(),
            "maximum usage ratio",
        )?;
        config.borrow_asset_maximum_usage_ratio = max_usage;

        let liquidation_spread = prompt_decimal(
            self.theme,
            "Maximum liquidator spread (e.g., 0.05 for 5%)",
            &config.liquidation_maximum_spread.to_string(),
            "maximum liquidator spread",
        )?;
        config.liquidation_maximum_spread = liquidation_spread;

        let has_max_duration = Confirm::with_theme(self.theme)
            .with_prompt("Set maximum borrow duration?")
            .default(config.borrow_maximum_duration_ms.is_some())
            .interact()
            .map_err(std::io::Error::other)?;

        config.borrow_maximum_duration_ms = if has_max_duration {
            let default_duration = config.borrow_maximum_duration_ms.map_or(0, |d| d.0);
            let max_duration_ms: u64 = Input::with_theme(self.theme)
                .with_prompt("Maximum borrow duration (milliseconds)")
                .default(default_duration)
                .interact_text()
                .map_err(std::io::Error::other)?;
            Some(U64(max_duration_ms))
        } else {
            None
        };

        Ok(())
    }

    fn edit_interest_rate_strategy(&self, config: &mut MarketConfiguration) -> CliResult<()> {
        println!("\n📈 Interest Rate Strategy");
        let defaults = StrategyDefaults::from_strategy(&config.borrow_interest_rate_strategy)?;

        let strategy_types = StrategyKind::ALL.to_vec();
        let strategy_choice = Select::with_theme(self.theme)
            .with_prompt("Select interest rate model")
            .items(&strategy_types)
            .default(defaults.kind.as_index())
            .interact()
            .map_err(std::io::Error::other)?;

        config.borrow_interest_rate_strategy = match strategy_choice {
            0 => {
                let base = prompt_decimal(
                    self.theme,
                    "Base rate at 0% utilization",
                    &defaults.get("base", "0.0"),
                    "linear base rate",
                )?;
                let top = prompt_decimal(
                    self.theme,
                    "Top rate at 100% utilization",
                    &defaults.get("top", "0.0"),
                    "linear top rate",
                )?;
                InterestRateStrategy::linear(base, top).ok_or_else(|| {
                    CliError::InvalidInput("Invalid linear interest rate parameters".into())
                })?
            }
            1 => {
                let base = prompt_decimal(
                    self.theme,
                    "Starting rate at 0% utilization",
                    &defaults.get("base", "0.0"),
                    "piecewise starting rate",
                )?;
                let optimal = prompt_decimal(
                    self.theme,
                    "Optimal utilization ratio (0-1)",
                    &defaults.get("optimal", "0.8"),
                    "piecewise optimal utilization",
                )?;
                let rate_1 = prompt_decimal(
                    self.theme,
                    "Rate at optimal utilization",
                    &defaults.get("rate_1", "0.0"),
                    "piecewise optimal rate",
                )?;
                let rate_2 = prompt_decimal(
                    self.theme,
                    "Maximum rate at 100% utilization",
                    &defaults.get("rate_2", "0.0"),
                    "piecewise max rate",
                )?;
                InterestRateStrategy::piecewise(base, optimal, rate_1, rate_2).ok_or_else(|| {
                    CliError::InvalidInput("Invalid piecewise interest rate parameters".into())
                })?
            }
            2 => {
                let base = prompt_decimal(
                    self.theme,
                    "Base rate at 0% utilization",
                    &defaults.get("base", "0.0"),
                    "exponential base rate",
                )?;
                let top = prompt_decimal(
                    self.theme,
                    "Top rate at 100% utilization",
                    &defaults.get("top", "0.0"),
                    "exponential top rate",
                )?;
                let eccentricity = prompt_decimal(
                    self.theme,
                    "Curve eccentricity (e.g., 2-12)",
                    &defaults.get("eccentricity", "2.0"),
                    "exponential eccentricity",
                )?;
                InterestRateStrategy::exponential2(base, top, eccentricity).ok_or_else(|| {
                    CliError::InvalidInput("Invalid exponential interest rate parameters".into())
                })?
            }
            _ => config.borrow_interest_rate_strategy.clone(),
        };

        Ok(())
    }

    fn edit_ranges(&self, config: &mut MarketConfiguration) -> CliResult<()> {
        println!("\n📏 Position Ranges");

        let borrow_min_default = U128::from(config.borrow_range.minimum).0;
        let borrow_min: u128 = Input::with_theme(self.theme)
            .with_prompt("Minimum borrow amount")
            .default(borrow_min_default)
            .interact_text()
            .map_err(std::io::Error::other)?;

        let has_borrow_max = Confirm::with_theme(self.theme)
            .with_prompt("Set maximum borrow amount?")
            .default(config.borrow_range.maximum.is_some())
            .interact()
            .map_err(std::io::Error::other)?;

        let borrow_max = if has_borrow_max {
            let default = config
                .borrow_range
                .maximum
                .map(U128::from)
                .map(|v| v.0)
                .unwrap_or_default();
            Some(
                Input::with_theme(self.theme)
                    .with_prompt("Maximum borrow amount")
                    .default(default)
                    .interact_text()
                    .map_err(std::io::Error::other)?,
            )
        } else {
            None
        };
        config.borrow_range = (borrow_min, borrow_max)
            .try_into()
            .map_err(|e: std::io::Error| CliError::Validation(e.to_string()))?;

        let supply_min_default = U128::from(config.supply_range.minimum).0;
        let supply_min: u128 = Input::with_theme(self.theme)
            .with_prompt("Minimum supply amount")
            .default(supply_min_default)
            .interact_text()
            .map_err(std::io::Error::other)?;

        let has_supply_max = Confirm::with_theme(self.theme)
            .with_prompt("Set maximum supply amount?")
            .default(config.supply_range.maximum.is_some())
            .interact()
            .map_err(std::io::Error::other)?;

        let supply_max = if has_supply_max {
            let default = config
                .supply_range
                .maximum
                .map(U128::from)
                .map(|v| v.0)
                .unwrap_or_default();
            Some(
                Input::with_theme(self.theme)
                    .with_prompt("Maximum supply amount")
                    .default(default)
                    .interact_text()
                    .map_err(std::io::Error::other)?,
            )
        } else {
            None
        };
        config.supply_range = (supply_min, supply_max)
            .try_into()
            .map_err(|e: std::io::Error| CliError::Validation(e.to_string()))?;

        let withdrawal_min_default = U128::from(config.supply_withdrawal_range.minimum).0;
        let withdrawal_min: u128 = Input::with_theme(self.theme)
            .with_prompt("Minimum withdrawal amount")
            .default(withdrawal_min_default)
            .interact_text()
            .map_err(std::io::Error::other)?;

        let has_withdrawal_max = Confirm::with_theme(self.theme)
            .with_prompt("Set maximum withdrawal amount?")
            .default(config.supply_withdrawal_range.maximum.is_some())
            .interact()
            .map_err(std::io::Error::other)?;

        let withdrawal_max = if has_withdrawal_max {
            let default = config
                .supply_withdrawal_range
                .maximum
                .map(U128::from)
                .map(|v| v.0)
                .unwrap_or_default();
            Some(
                Input::with_theme(self.theme)
                    .with_prompt("Maximum withdrawal amount")
                    .default(default)
                    .interact_text()
                    .map_err(std::io::Error::other)?,
            )
        } else {
            None
        };
        config.supply_withdrawal_range = (withdrawal_min, withdrawal_max)
            .try_into()
            .map_err(|e: std::io::Error| CliError::Validation(e.to_string()))?;

        Ok(())
    }

    fn edit_fees(&self, config: &mut MarketConfiguration) -> CliResult<()> {
        println!("\n💰 Fees");

        let (origination_default_idx, origination_default_value) =
            fee_defaults(&config.borrow_origination_fee);

        let origination_fee_type = Select::with_theme(self.theme)
            .with_prompt("Borrow origination fee type")
            .items(["Flat amount", "Percentage"])
            .default(origination_default_idx)
            .interact()
            .map_err(std::io::Error::other)?;

        config.borrow_origination_fee = if origination_fee_type == 0 {
            let amount: u128 = Input::with_theme(self.theme)
                .with_prompt("Flat fee amount")
                .default(origination_default_value.parse().unwrap_or(0))
                .interact_text()
                .map_err(std::io::Error::other)?;
            Fee::Flat(amount.into())
        } else {
            let percentage = prompt_decimal(
                self.theme,
                "Fee percentage (e.g., 0.001 for 0.1%)",
                &origination_default_value,
                "origination fee percentage",
            )?;
            Fee::Proportional(percentage)
        };

        let (withdrawal_default_idx, withdrawal_default_value) =
            fee_defaults(&config.supply_withdrawal_fee.fee);

        let withdrawal_fee_type = Select::with_theme(self.theme)
            .with_prompt("Supply withdrawal fee type")
            .items(["Flat amount", "Percentage"])
            .default(withdrawal_default_idx)
            .interact()
            .map_err(std::io::Error::other)?;

        let withdrawal_fee = if withdrawal_fee_type == 0 {
            let amount: u128 = Input::with_theme(self.theme)
                .with_prompt("Withdrawal flat fee amount")
                .default(withdrawal_default_value.parse().unwrap_or(0))
                .interact_text()
                .map_err(std::io::Error::other)?;
            Fee::Flat(amount.into())
        } else {
            let percentage = prompt_decimal(
                self.theme,
                "Withdrawal fee percentage",
                &withdrawal_default_value,
                "withdrawal fee percentage",
            )?;
            Fee::Proportional(percentage)
        };

        let duration_default = config.supply_withdrawal_fee.duration.0;
        let duration_ms: u64 = Input::with_theme(self.theme)
            .with_prompt("Withdrawal fee duration (ms)")
            .default(duration_default)
            .interact_text()
            .map_err(std::io::Error::other)?;

        let behavior_idx = match config.supply_withdrawal_fee.behavior {
            TimeBasedFeeFunction::Fixed => 0,
            TimeBasedFeeFunction::Linear => 1,
        };

        let behavior_choice = Select::with_theme(self.theme)
            .with_prompt("Withdrawal fee behavior")
            .items(["Fixed (drops to zero after duration)", "Linear decay"])
            .default(behavior_idx)
            .interact()
            .map_err(std::io::Error::other)?;

        let behavior = if behavior_choice == 0 {
            TimeBasedFeeFunction::Fixed
        } else {
            TimeBasedFeeFunction::Linear
        };

        config.supply_withdrawal_fee = TimeBasedFee {
            fee: withdrawal_fee,
            duration: U64(duration_ms),
            behavior,
        };

        Ok(())
    }

    fn edit_yield_weights(&self, config: &mut MarketConfiguration) -> CliResult<()> {
        println!("\n🎯 Yield Distribution");

        let supply_weight: u16 = Input::with_theme(self.theme)
            .with_prompt("Supplier yield weight")
            .default(config.yield_weights.supply.get())
            .interact_text()
            .map_err(std::io::Error::other)?;

        if supply_weight == 0 {
            return Err(CliError::InvalidInput(
                "Supplier yield weight must be greater than zero".into(),
            ));
        }

        let mut weights = YieldWeights::new_with_supply_weight(supply_weight);

        if !config.yield_weights.r#static.is_empty() {
            println!("Current static recipients:");
            for (account, weight) in &config.yield_weights.r#static {
                println!("- {account}: {weight}");
            }
        }

        let keep_static = Confirm::with_theme(self.theme)
            .with_prompt("Keep existing static recipients?")
            .default(!config.yield_weights.r#static.is_empty())
            .interact()
            .map_err(std::io::Error::other)?;

        if keep_static {
            weights.r#static.clone_from(&config.yield_weights.r#static);
            // weights.r#static = config.yield_weights.r#static.clone();
        } else {
            while Confirm::with_theme(self.theme)
                .with_prompt("Add a static recipient?")
                .default(weights.r#static.is_empty())
                .interact()
                .map_err(std::io::Error::other)?
            {
                let account: String = Input::with_theme(self.theme)
                    .with_prompt("Static recipient account ID")
                    .interact_text()
                    .map_err(std::io::Error::other)?;
                let weight: u16 = Input::with_theme(self.theme)
                    .with_prompt("Static recipient weight")
                    .default(1)
                    .interact_text()
                    .map_err(std::io::Error::other)?;

                let account_id = AccountId::from_str(&account).map_err(|e| {
                    CliError::InvalidInput(format!("Invalid static recipient account: {e}"))
                })?;
                weights = weights.with_static(account_id, weight);
            }
        }

        config.yield_weights = weights;

        Ok(())
    }
}
