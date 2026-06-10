use crate::{
    logger,
    ui::prompt::{error::map_dialoguer_err, wizard::types::prompt_until_valid},
    CliError, CliResult, ConfigBuilder,
};
use dialoguer::{theme::ColorfulTheme, Confirm, Input};
use near_sdk::json_types::U64;
use std::str::FromStr;
use templar_common::{market::MarketConfiguration, Decimal};

/// Prompts for risk parameters during interactive mode.
#[allow(clippy::too_many_lines)]
pub fn prompt_risk_parameters(
    theme: &ColorfulTheme,
    mut builder: ConfigBuilder,
) -> CliResult<ConfigBuilder> {
    logger::heading("\n⚖️  Risk Parameters\n");

    let mcr_maintenance_default = builder
        .borrow_mcr_maintenance_value()
        .map_or_else(|| "1.25".to_string(), |value| value.to_string());
    let mcr_maintenance = prompt_until_valid(
        || {
            Input::with_theme(theme)
                .with_prompt("Maintenance collateralization ratio (e.g., 1.25 for 125%)")
                .default(mcr_maintenance_default.clone())
                .interact_text()
        },
        |value: String| {
            let mcr = Decimal::from_str(&value)
                .map_err(|_| CliError::InvalidInput("Invalid decimal".into()))?;
            if mcr <= Decimal::ONE {
                return Err(CliError::InvalidInput(
                    "Maintenance MCR must be greater than 1.0".into(),
                ));
            }
            Ok(mcr)
        },
    )?;
    builder = builder.borrow_mcr_maintenance(mcr_maintenance);

    let mcr_liquidation_default = builder
        .borrow_mcr_liquidation_value()
        .map_or_else(|| "1.20".to_string(), |value| value.to_string());
    let mcr_liquidation = prompt_until_valid(
        || {
            Input::with_theme(theme)
                .with_prompt("Liquidation collateralization ratio (e.g., 1.20 for 120%)")
                .default(mcr_liquidation_default.clone())
                .interact_text()
        },
        |value: String| {
            let mcr = Decimal::from_str(&value)
                .map_err(|_| CliError::InvalidInput("Invalid decimal".into()))?;
            if mcr <= Decimal::ONE {
                return Err(CliError::InvalidInput(
                    "Liquidation MCR must be greater than 1.0".into(),
                ));
            }
            if mcr > mcr_maintenance {
                return Err(CliError::InvalidInput(
                    "Liquidation MCR must be less than or equal to maintenance MCR".into(),
                ));
            }
            Ok(mcr)
        },
    )?;
    builder = builder.borrow_mcr_liquidation(mcr_liquidation);

    let max_usage_default = builder
        .borrow_max_usage_ratio_value()
        .map_or_else(|| "0.90".to_string(), |value| value.to_string());
    let max_usage = prompt_until_valid(
        || {
            Input::with_theme(theme)
                .with_prompt("Maximum usage ratio (e.g., 0.90 for 90%)")
                .default(max_usage_default.clone())
                .interact_text()
        },
        |value: String| {
            let ratio = Decimal::from_str(&value)
                .map_err(|_| CliError::InvalidInput("Invalid decimal".into()))?;
            if ratio.is_zero() || ratio > Decimal::ONE {
                return Err(CliError::InvalidInput(
                    "Maximum usage ratio must be > 0 and <= 1.0".into(),
                ));
            }
            Ok(ratio)
        },
    )?;
    builder = builder.borrow_max_usage_ratio(max_usage);

    let liquidation_spread_default = builder
        .liquidation_max_spread_value()
        .map_or_else(|| "0.05".to_string(), |value| value.to_string());
    let liquidation_spread = prompt_until_valid(
        || {
            Input::with_theme(theme)
                .with_prompt("Maximum liquidator spread (e.g., 0.05 for 5%)")
                .default(liquidation_spread_default.clone())
                .interact_text()
        },
        |value: String| {
            let spread = Decimal::from_str(&value)
                .map_err(|_| CliError::InvalidInput("Invalid decimal".into()))?;
            if spread < Decimal::ZERO || spread >= Decimal::ONE {
                return Err(CliError::InvalidInput(
                    "Liquidation spread must be >= 0 and < 1.0".into(),
                ));
            }
            Ok(spread)
        },
    )?;
    builder = builder.liquidation_max_spread(liquidation_spread);

    let has_max_duration = Confirm::with_theme(theme)
        .with_prompt("Set maximum borrow duration?")
        .default(true)
        .interact()
        .map_err(|err| map_dialoguer_err(&err))?;

    if has_max_duration {
        let max_duration_ms: u64 = prompt_until_valid(
            || {
                Input::with_theme(theme)
                    .with_prompt("Maximum borrow duration (milliseconds)")
                    .interact_text()
            },
            Ok::<_, CliError>,
        )?;
        builder = builder.borrow_max_duration_ms(Some(max_duration_ms));
    } else {
        builder = builder.borrow_max_duration_ms(None);
    }

    Ok(builder)
}

/// Edits risk parameters on an existing market configuration.
#[allow(clippy::too_many_lines)]
pub fn edit_risk_parameters(
    theme: &ColorfulTheme,
    config: &mut MarketConfiguration,
) -> CliResult<()> {
    logger::heading("\n⚖️  Risk Parameters");

    let mcr_maintenance_default = config.borrow_mcr_maintenance.to_string();
    let mcr_maintenance = prompt_until_valid(
        || {
            Input::with_theme(theme)
                .with_prompt("Maintenance collateralization ratio (e.g., 1.25 for 125%)")
                .default(mcr_maintenance_default.clone())
                .interact_text()
        },
        |value: String| {
            let mcr = Decimal::from_str(&value)
                .map_err(|_| CliError::InvalidInput("Invalid decimal".into()))?;
            if mcr <= Decimal::ONE {
                return Err(CliError::InvalidInput(
                    "Maintenance MCR must be greater than 1.0".into(),
                ));
            }
            Ok(mcr)
        },
    )?;
    config.borrow_mcr_maintenance = mcr_maintenance;

    let mcr_liquidation_default = config.borrow_mcr_liquidation.to_string();
    let mcr_liquidation = prompt_until_valid(
        || {
            Input::with_theme(theme)
                .with_prompt("Liquidation collateralization ratio (e.g., 1.20 for 120%)")
                .default(mcr_liquidation_default.clone())
                .interact_text()
        },
        |value: String| {
            let mcr = Decimal::from_str(&value)
                .map_err(|_| CliError::InvalidInput("Invalid decimal".into()))?;
            if mcr <= Decimal::ONE {
                return Err(CliError::InvalidInput(
                    "Liquidation MCR must be greater than 1.0".into(),
                ));
            }
            if mcr > mcr_maintenance {
                return Err(CliError::InvalidInput(
                    "Liquidation MCR must be less than or equal to maintenance MCR".into(),
                ));
            }
            Ok(mcr)
        },
    )?;
    config.borrow_mcr_liquidation = mcr_liquidation;

    let max_usage_default = config.borrow_asset_maximum_usage_ratio.to_string();
    let max_usage = prompt_until_valid(
        || {
            Input::with_theme(theme)
                .with_prompt("Maximum usage ratio (e.g., 0.90 for 90%)")
                .default(max_usage_default.clone())
                .interact_text()
        },
        |value: String| {
            let ratio = Decimal::from_str(&value)
                .map_err(|_| CliError::InvalidInput("Invalid decimal".into()))?;
            if ratio.is_zero() || ratio > Decimal::ONE {
                return Err(CliError::InvalidInput(
                    "Maximum usage ratio must be > 0 and <= 1.0".into(),
                ));
            }
            Ok(ratio)
        },
    )?;
    config.borrow_asset_maximum_usage_ratio = max_usage;

    let liquidation_spread_default = config.liquidation_maximum_spread.to_string();
    let liquidation_spread = prompt_until_valid(
        || {
            Input::with_theme(theme)
                .with_prompt("Maximum liquidator spread (e.g., 0.05 for 5%)")
                .default(liquidation_spread_default.clone())
                .interact_text()
        },
        |value: String| {
            let spread = Decimal::from_str(&value)
                .map_err(|_| CliError::InvalidInput("Invalid decimal".into()))?;
            if spread < Decimal::ZERO || spread >= Decimal::ONE {
                return Err(CliError::InvalidInput(
                    "Liquidation spread must be >= 0 and < 1.0".into(),
                ));
            }
            Ok(spread)
        },
    )?;
    config.liquidation_maximum_spread = liquidation_spread;

    let has_max_duration = Confirm::with_theme(theme)
        .with_prompt("Set maximum borrow duration?")
        .default(config.borrow_maximum_duration_ms.is_some())
        .interact()
        .map_err(|err| map_dialoguer_err(&err))?;

    config.borrow_maximum_duration_ms = if has_max_duration {
        let default_duration = config.borrow_maximum_duration_ms.map_or(0, |d| d.0);
        let max_duration_ms: u64 = Input::with_theme(theme)
            .with_prompt("Maximum borrow duration (milliseconds)")
            .default(default_duration)
            .interact_text()
            .map_err(|err| map_dialoguer_err(&err))?;
        Some(U64(max_duration_ms))
    } else {
        None
    };

    Ok(())
}
