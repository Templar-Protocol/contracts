use crate::{common::shared::map_dialoguer_err, logger, CliResult, ConfigBuilder};
use dialoguer::{theme::ColorfulTheme, Confirm, Input};

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

/// Prompt for borrow/supply/withdraw ranges, delegating validation to caller.
/// Validation runs once per full set of inputs; on error we re-prompt all fields.
/// # Errors
pub fn prompt_ranges_with_validation(
    theme: &ColorfulTheme,
    defaults: &RangeDefaults,
    header: Option<String>,
    mut hint: impl for<'a> FnMut(&'a str, u128),
    mut validate: impl FnMut(&RangeSelection) -> CliResult<()>,
) -> CliResult<RangeSelection> {
    logger::heading("\n📏 Position Ranges\n");
    if let Some(header) = header {
        println!("{header}\n");
    }

    loop {
        let borrow_min: u128 =
            prompt_u128(theme, "Minimum borrow amount", Some(defaults.borrow_min))?;
        hint("Minimum borrow amount", borrow_min);
        let has_borrow_max = Confirm::with_theme(theme)
            .with_prompt("Set maximum borrow amount?")
            .default(defaults.borrow_max.is_some())
            .interact()
            .map_err(|err| map_dialoguer_err(&err))?;
        let borrow_max = if has_borrow_max {
            let value = prompt_u128(theme, "Maximum borrow amount", defaults.borrow_max)?;
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
            let supply_min: u128 =
                prompt_u128(theme, "Minimum supply amount", Some(defaults.supply_min))?;
            hint("Minimum supply amount", supply_min);
            let has_supply_max = Confirm::with_theme(theme)
                .with_prompt("Set maximum supply amount?")
                .default(defaults.supply_max.is_some())
                .interact()
                .map_err(|err| map_dialoguer_err(&err))?;
            let supply_max = if has_supply_max {
                let value = prompt_u128(theme, "Maximum supply amount", defaults.supply_max)?;
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

        let withdrawal_min: u128 = prompt_u128(
            theme,
            "Minimum withdrawal amount",
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
            let value = prompt_u128(theme, "Maximum withdrawal amount", defaults.withdrawal_max)?;
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

fn prompt_u128(theme: &ColorfulTheme, prompt: &str, default: Option<u128>) -> CliResult<u128> {
    let mut input = Input::with_theme(theme).with_prompt(prompt);
    if let Some(default) = default {
        if default != 0 {
            input = input.default(default);
        }
    }
    input.interact_text().map_err(|err| map_dialoguer_err(&err))
}
