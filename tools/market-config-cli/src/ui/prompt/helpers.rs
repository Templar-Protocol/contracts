use crate::{ui::prompt::error::map_dialoguer_err, CliError, CliResult};
use dialoguer::{theme::ColorfulTheme, Input};
use near_sdk::json_types::U128;
use std::str::FromStr;
use templar_common::{asset::AssetClass, fee::Fee, number::Decimal};

/// # Errors
pub fn prompt_decimal(
    theme: &ColorfulTheme,
    prompt: &str,
    default: &str,
    field: &str,
) -> CliResult<Decimal> {
    let value: String = Input::with_theme(theme)
        .with_prompt(prompt)
        .default(default.to_string())
        .interact_text()
        .map_err(|err| map_dialoguer_err(&err))?;
    Decimal::from_str(&value)
        .map_err(|_| CliError::InvalidInput(format!("Invalid decimal for {field}: {value}")))
}

/// Prompt for an integer decimal count with inline bounds checking (0-24).
/// # Errors
pub fn prompt_decimals(
    theme: &ColorfulTheme,
    prompt: &str,
    default: i32,
    field: &str,
) -> CliResult<i32> {
    let value: i32 = Input::with_theme(theme)
        .with_prompt(prompt)
        .default(default)
        .interact_text()
        .map_err(|err| map_dialoguer_err(&err))?;

    if (0..=24).contains(&value) {
        Ok(value)
    } else {
        Err(CliError::InvalidInput(format!(
            "{field} must be between 0 and 24"
        )))
    }
}

pub fn fee_defaults<T: AssetClass>(fee: &Fee<T>) -> (usize, String) {
    match fee {
        Fee::Flat(amount) => (0, U128::from(*amount).0.to_string()),
        Fee::Proportional(pct) => (1, pct.to_string()),
    }
}
