use crate::{
    logger,
    ui::prompt::{
        error::map_dialoguer_err,
        helpers::{fee_defaults, prompt_decimal},
        wizard::types::prompt_until_valid,
    },
    CliError, CliResult, ConfigBuilder,
};
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};
use near_sdk::json_types::U64;
use std::str::FromStr;
use templar_common::{
    fee::{Fee, TimeBasedFee, TimeBasedFeeFunction},
    market::MarketConfiguration,
    number::Decimal,
};

/// Prompts for fee configuration during interactive mode.
pub fn prompt_fees(theme: &ColorfulTheme, mut builder: ConfigBuilder) -> CliResult<ConfigBuilder> {
    logger::heading("\n💰 Fees\n");

    let has_origination_fee = Confirm::with_theme(theme)
        .with_prompt("Set borrow origination fee?")
        .default(true)
        .interact()
        .map_err(|err| map_dialoguer_err(&err))?;

    if has_origination_fee {
        let fee_types = vec!["Flat amount", "Percentage"];
        let fee_type = Select::with_theme(theme)
            .with_prompt("Fee type")
            .items(&fee_types)
            .default(1)
            .interact()
            .map_err(|err| map_dialoguer_err(&err))?;

        if fee_type == 0 {
            let amount: u128 = prompt_until_valid(
                || {
                    Input::with_theme(theme)
                        .with_prompt("Flat fee amount")
                        .interact_text()
                },
                Ok::<_, CliError>,
            )?;
            builder = builder.borrow_origination_fee(Fee::Flat(amount.into()));
        } else {
            let pct = prompt_until_valid(
                || {
                    Input::with_theme(theme)
                        .with_prompt("Fee percentage (e.g., 0.001 for 0.1%)")
                        .interact_text()
                },
                |value: String| {
                    Decimal::from_str(&value)
                        .map_err(|_| CliError::InvalidInput("Invalid decimal".into()))
                },
            )?;
            builder = builder.borrow_origination_fee(Fee::Proportional(pct));
        }
    } else {
        builder = builder.borrow_origination_fee(Fee::zero());
    }

    builder = builder.supply_withdrawal_fee(TimeBasedFee::zero());

    Ok(builder)
}

/// Edits fee configuration on an existing market configuration.
pub fn edit_fees(theme: &ColorfulTheme, config: &mut MarketConfiguration) -> CliResult<()> {
    logger::heading("\n💰 Fees");

    let (origination_default_idx, origination_default_value) =
        fee_defaults(&config.borrow_origination_fee);

    let origination_fee_type = Select::with_theme(theme)
        .with_prompt("Borrow origination fee type")
        .items(["Flat amount", "Percentage"])
        .default(origination_default_idx)
        .interact()
        .map_err(|err| map_dialoguer_err(&err))?;

    config.borrow_origination_fee = if origination_fee_type == 0 {
        let amount: u128 = Input::with_theme(theme)
            .with_prompt("Flat fee amount")
            .default(origination_default_value.parse().unwrap_or(0))
            .interact_text()
            .map_err(|err| map_dialoguer_err(&err))?;
        Fee::Flat(amount.into())
    } else {
        let percentage = prompt_decimal(
            theme,
            "Fee percentage (e.g., 0.001 for 0.1%)",
            &origination_default_value,
            "origination fee percentage",
        )?;
        Fee::Proportional(percentage)
    };

    let (withdrawal_default_idx, withdrawal_default_value) =
        fee_defaults(&config.supply_withdrawal_fee.fee);

    let withdrawal_fee_type = Select::with_theme(theme)
        .with_prompt("Supply withdrawal fee type")
        .items(["Flat amount", "Percentage"])
        .default(withdrawal_default_idx)
        .interact()
        .map_err(|err| map_dialoguer_err(&err))?;

    let withdrawal_fee = if withdrawal_fee_type == 0 {
        let amount: u128 = Input::with_theme(theme)
            .with_prompt("Withdrawal flat fee amount")
            .default(withdrawal_default_value.parse().unwrap_or(0))
            .interact_text()
            .map_err(|err| map_dialoguer_err(&err))?;
        Fee::Flat(amount.into())
    } else {
        let percentage = prompt_decimal(
            theme,
            "Withdrawal fee percentage",
            &withdrawal_default_value,
            "withdrawal fee percentage",
        )?;
        Fee::Proportional(percentage)
    };

    let duration_default = config.supply_withdrawal_fee.duration.0;
    let duration_ms: u64 = Input::with_theme(theme)
        .with_prompt("Withdrawal fee duration (ms)")
        .default(duration_default)
        .interact_text()
        .map_err(|err| map_dialoguer_err(&err))?;

    let behavior_idx = match config.supply_withdrawal_fee.behavior {
        TimeBasedFeeFunction::Fixed => 0,
        TimeBasedFeeFunction::Linear => 1,
    };

    let behavior_choice = Select::with_theme(theme)
        .with_prompt("Withdrawal fee behavior")
        .items(["Fixed (drops to zero after duration)", "Linear decay"])
        .default(behavior_idx)
        .interact()
        .map_err(|err| map_dialoguer_err(&err))?;

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
