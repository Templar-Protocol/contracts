use crate::{logger, ui::prompt::error::map_dialoguer_err, CliError, CliResult, ConfigBuilder};
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};
use templar_common::number::Decimal;

#[derive(Clone, Debug)]
pub struct RangeDefaults {
    pub borrow_min: u128,
    pub borrow_max: Option<u128>,
    pub supply_min: u128,
    pub supply_max: Option<u128>,
    pub withdrawal_min: u128,
    pub withdrawal_max: Option<u128>,
}

#[derive(Clone, Debug)]
pub struct RangeSelection {
    pub borrow_min: u128,
    pub borrow_max: Option<u128>,
    pub supply_min: u128,
    pub supply_max: Option<u128>,
    pub withdrawal_min: u128,
    pub withdrawal_max: Option<u128>,
}

#[derive(Clone, Copy, Debug)]
enum InputMode {
    Base,
    Asset,
    Eth,
    Near,
}

/// Prompt for borrow/supply/withdraw ranges, delegating validation to caller.
/// Validation runs once per full set of inputs; on error we re-prompt all fields.
/// # Errors
#[allow(clippy::too_many_lines)]
#[allow(clippy::too_many_arguments)]
pub fn prompt_ranges_with_validation(
    theme: &ColorfulTheme,
    defaults: &RangeDefaults,
    header: Option<String>,
    asset_decimals: Option<i32>,
    borrow_price_usd: Option<Decimal>,
    eth_price_usd: Option<Decimal>,
    near_price_usd: Option<Decimal>,
    mut hint: impl for<'a> FnMut(&'a str, u128),
    mut validate: impl FnMut(&RangeSelection) -> CliResult,
) -> CliResult<RangeSelection> {
    logger::heading("\n📏 Position Ranges\n");
    if let Some(header) = header {
        println!("{header}\n");
    }

    let input_mode = select_input_mode(
        theme,
        asset_decimals,
        borrow_price_usd.as_ref(),
        eth_price_usd.as_ref(),
        near_price_usd.as_ref(),
    )?;

    loop {
        let borrow_min = prompt_amount(
            theme,
            "Minimum borrow amount",
            input_mode,
            asset_decimals,
            borrow_price_usd.as_ref(),
            eth_price_usd.as_ref(),
            near_price_usd.as_ref(),
            Some(defaults.borrow_min),
        )?;
        hint("Minimum borrow amount", borrow_min);
        let has_borrow_max = Confirm::with_theme(theme)
            .with_prompt("Set maximum borrow amount?")
            .default(defaults.borrow_max.is_some())
            .interact()
            .map_err(|err| map_dialoguer_err(&err))?;
        let borrow_max = if has_borrow_max {
            let value = prompt_amount(
                theme,
                "Maximum borrow amount",
                input_mode,
                asset_decimals,
                borrow_price_usd.as_ref(),
                eth_price_usd.as_ref(),
                near_price_usd.as_ref(),
                defaults.borrow_max,
            )?;
            hint("Maximum borrow amount", value);
            Some(value)
        } else {
            None
        };

        if borrow_min == 0 {
            logger::warn("Borrow range minimum must be greater than zero");
            println!("Please re-enter the borrow range values.\n");
            continue;
        }

        let (supply_min, supply_max) = loop {
            let supply_min = prompt_amount(
                theme,
                "Minimum supply amount",
                input_mode,
                asset_decimals,
                borrow_price_usd.as_ref(),
                eth_price_usd.as_ref(),
                near_price_usd.as_ref(),
                Some(defaults.supply_min),
            )?;
            hint("Minimum supply amount", supply_min);
            let has_supply_max = Confirm::with_theme(theme)
                .with_prompt("Set maximum supply amount?")
                .default(defaults.supply_max.is_some())
                .interact()
                .map_err(|err| map_dialoguer_err(&err))?;
            let supply_max = if has_supply_max {
                let value = prompt_amount(
                    theme,
                    "Maximum supply amount",
                    input_mode,
                    asset_decimals,
                    borrow_price_usd.as_ref(),
                    eth_price_usd.as_ref(),
                    near_price_usd.as_ref(),
                    defaults.supply_max,
                )?;
                hint("Maximum supply amount", value);
                Some(value)
            } else {
                None
            };

            if supply_min == 0 {
                logger::warn("Supply range minimum must be greater than zero");
                println!("Please re-enter the supply range values.\n");
                continue;
            }

            break (supply_min, supply_max);
        };

        let withdrawal_min = prompt_amount(
            theme,
            "Minimum withdrawal amount",
            input_mode,
            asset_decimals,
            borrow_price_usd.as_ref(),
            eth_price_usd.as_ref(),
            near_price_usd.as_ref(),
            Some(defaults.withdrawal_min),
        )?;
        hint("Minimum withdrawal amount", withdrawal_min);

        if withdrawal_min > supply_min {
            logger::warn("Withdrawal minimum cannot be greater than the supply range minimum");
            println!("Please re-enter the withdrawal range.\n");
            continue;
        }
        let has_withdrawal_max = Confirm::with_theme(theme)
            .with_prompt("Set maximum withdrawal amount?")
            .default(defaults.withdrawal_max.is_some())
            .interact()
            .map_err(|err| map_dialoguer_err(&err))?;
        let withdrawal_max = if has_withdrawal_max {
            let value = prompt_amount(
                theme,
                "Maximum withdrawal amount",
                input_mode,
                asset_decimals,
                borrow_price_usd.as_ref(),
                eth_price_usd.as_ref(),
                near_price_usd.as_ref(),
                defaults.withdrawal_max,
            )?;
            hint("Maximum withdrawal amount", value);
            Some(value)
        } else {
            None
        };

        let selection = RangeSelection {
            borrow_min,
            borrow_max,
            supply_min,
            supply_max,
            withdrawal_min,
            withdrawal_max,
        };

        if let Err(e) = validate(&selection) {
            logger::warn(e);
            println!("Please re-enter the range values.\n");
            continue;
        }

        return Ok(selection);
    }
}

/// # Errors
pub fn apply_ranges_to_builder(
    mut builder: ConfigBuilder,
    selection: &RangeSelection,
) -> CliResult<ConfigBuilder> {
    builder = builder.borrow_range(selection.borrow_min, selection.borrow_max)?;
    builder = builder.supply_range(selection.supply_min, selection.supply_max)?;
    builder =
        builder.supply_withdrawal_range(selection.withdrawal_min, selection.withdrawal_max)?;
    Ok(builder)
}

fn select_input_mode(
    theme: &ColorfulTheme,
    asset_decimals: Option<i32>,
    borrow_price_usd: Option<&Decimal>,
    eth_price_usd: Option<&Decimal>,
    near_price_usd: Option<&Decimal>,
) -> CliResult<InputMode> {
    let Some(decimals) = asset_decimals else {
        return Ok(InputMode::Base);
    };
    if !(0..=24).contains(&decimals) {
        return Ok(InputMode::Base);
    }

    let mut options = vec![
        ("Base units", InputMode::Base),
        ("Asset units (e.g., 0.01)", InputMode::Asset),
    ];
    let has_eth = borrow_price_usd.is_some() && eth_price_usd.is_some();
    if has_eth {
        options.push(("ETH", InputMode::Eth));
    }
    let has_near = borrow_price_usd.is_some() && near_price_usd.is_some();
    if has_near {
        options.push(("NEAR", InputMode::Near));
    }
    let labels: Vec<&str> = options.iter().map(|(label, _)| *label).collect();

    let selected = Select::with_theme(theme)
        .with_prompt("Select input mode for amounts")
        .items(&labels)
        .default(0)
        .interact()
        .map_err(|err| map_dialoguer_err(&err))?;
    Ok(options
        .get(selected)
        .map_or(InputMode::Base, |(_, mode)| *mode))
}

#[allow(clippy::too_many_arguments)]
fn prompt_amount(
    theme: &ColorfulTheme,
    prompt: &str,
    input_mode: InputMode,
    asset_decimals: Option<i32>,
    borrow_price_usd: Option<&Decimal>,
    eth_price_usd: Option<&Decimal>,
    near_price_usd: Option<&Decimal>,
    default_base_units: Option<u128>,
) -> CliResult<u128> {
    match input_mode {
        InputMode::Base => prompt_u128(theme, prompt, default_base_units),
        #[allow(clippy::cast_sign_loss)]
        InputMode::Asset => {
            let decimals = asset_decimals.unwrap_or(0);
            let default = default_base_units.map(|value| {
                let scale = Decimal::from_u32(10).pow(decimals);
                let amount = Decimal::from(value) / scale;
                amount.to_fixed(decimals as usize)
            });
            let value = prompt_decimal_string(theme, prompt, default)?;
            asset_units_to_base_units(&value, decimals)
        }
        InputMode::Eth => {
            let decimals = asset_decimals.unwrap_or(0);
            let Some(borrow_price_usd) = borrow_price_usd else {
                return Err(CliError::InvalidInput(
                    "Borrow price is unavailable for ETH conversion".into(),
                ));
            };
            let Some(eth_price_usd) = eth_price_usd else {
                return Err(CliError::InvalidInput(
                    "ETH price is unavailable for conversion".into(),
                ));
            };
            let default = default_base_units.map(|value| {
                let scale = Decimal::from_u32(10).pow(decimals);
                let borrow_amount = Decimal::from(value) / scale;
                let borrow_value_usd = borrow_amount * *borrow_price_usd;
                let eth_amount = borrow_value_usd / *eth_price_usd;
                eth_amount.to_fixed(6)
            });
            let value = prompt_decimal_string(theme, prompt, default)?;
            eth_units_to_base_units(&value, decimals, borrow_price_usd, eth_price_usd)
        }
        InputMode::Near => {
            let decimals = asset_decimals.unwrap_or(0);
            let Some(borrow_price_usd) = borrow_price_usd else {
                return Err(CliError::InvalidInput(
                    "Borrow price is unavailable for NEAR conversion".into(),
                ));
            };
            let Some(near_price_usd) = near_price_usd else {
                return Err(CliError::InvalidInput(
                    "NEAR price is unavailable for conversion".into(),
                ));
            };
            let default = default_base_units.map(|value| {
                let scale = Decimal::from_u32(10).pow(decimals);
                let borrow_amount = Decimal::from(value) / scale;
                let borrow_value_usd = borrow_amount * *borrow_price_usd;
                let near_amount = borrow_value_usd / *near_price_usd;
                near_amount.to_fixed(6)
            });
            let value = prompt_decimal_string(theme, prompt, default)?;
            near_units_to_base_units(&value, decimals, borrow_price_usd, near_price_usd)
        }
    }
}

fn prompt_u128(theme: &ColorfulTheme, prompt: &str, default: Option<u128>) -> CliResult<u128> {
    let mut input = Input::with_theme(theme).with_prompt(prompt);
    if let Some(default) = default {
        if default != 0 {
            input = input.default(default);
        }
    }
    input.interact_text().map_err(|err| map_dialoguer_err(&err))
}

fn prompt_decimal_string(
    theme: &ColorfulTheme,
    prompt: &str,
    default: Option<String>,
) -> CliResult<String> {
    let mut input = Input::with_theme(theme).with_prompt(prompt);
    if let Some(default) = default {
        input = input.default(default);
    }
    input.interact_text().map_err(|err| map_dialoguer_err(&err))
}

fn asset_units_to_base_units(value: &str, decimals: i32) -> CliResult<u128> {
    let amount = value
        .parse::<Decimal>()
        .map_err(|_| CliError::InvalidInput(format!("Invalid amount: {value}")))?;
    amount_decimal_to_base_units(amount, decimals)
}

fn eth_units_to_base_units(
    value: &str,
    decimals: i32,
    borrow_price_usd: &Decimal,
    eth_price_usd: &Decimal,
) -> CliResult<u128> {
    let amount = value
        .parse::<Decimal>()
        .map_err(|_| CliError::InvalidInput(format!("Invalid amount: {value}")))?;
    if amount < Decimal::ZERO {
        return Err(CliError::InvalidInput(
            "Amount must be greater than or equal to zero".into(),
        ));
    }
    if eth_price_usd.is_zero() || borrow_price_usd.is_zero() {
        return Err(CliError::InvalidInput(
            "ETH or borrow price is unavailable for conversion".into(),
        ));
    }
    let borrow_amount = amount * *eth_price_usd / *borrow_price_usd;
    amount_decimal_to_base_units(borrow_amount, decimals)
}

fn near_units_to_base_units(
    value: &str,
    decimals: i32,
    borrow_price_usd: &Decimal,
    near_price_usd: &Decimal,
) -> CliResult<u128> {
    let amount = value
        .parse::<Decimal>()
        .map_err(|_| CliError::InvalidInput(format!("Invalid amount: {value}")))?;
    if amount < Decimal::ZERO {
        return Err(CliError::InvalidInput(
            "Amount must be greater than or equal to zero".into(),
        ));
    }
    if near_price_usd.is_zero() || borrow_price_usd.is_zero() {
        return Err(CliError::InvalidInput(
            "NEAR or borrow price is unavailable for conversion".into(),
        ));
    }
    let borrow_amount = amount * *near_price_usd / *borrow_price_usd;
    amount_decimal_to_base_units(borrow_amount, decimals)
}

fn amount_decimal_to_base_units(amount: Decimal, decimals: i32) -> CliResult<u128> {
    if amount < Decimal::ZERO {
        return Err(CliError::InvalidInput(
            "Amount must be greater than or equal to zero".into(),
        ));
    }

    let scale = Decimal::from_u32(10).pow(decimals);
    let base_units = amount * scale;
    let floor = base_units.to_u128_floor().ok_or_else(|| {
        CliError::InvalidInput("Amount is too large to convert to base units".into())
    })?;

    Ok(floor)
}
