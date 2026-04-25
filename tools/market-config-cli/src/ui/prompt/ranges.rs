use crate::{
    logger,
    ui::prompt::{error::map_dialoguer_err, wizard::types::prompt_until_valid},
    CliError, CliResult, ConfigBuilder,
};
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};
use templar_common::Decimal;

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
        InputMode::Base => prompt_until_valid(
            || {
                let mut input = Input::with_theme(theme).with_prompt(prompt);
                if let Some(default) = default_base_units {
                    if default != 0 {
                        input = input.default(default);
                    }
                }
                input.interact_text()
            },
            Ok,
        ),
        #[allow(clippy::cast_sign_loss)]
        InputMode::Asset => {
            let decimals = asset_decimals.unwrap_or(0);
            let default = default_base_units.map(|value| {
                let scale = Decimal::from_u32(10).pow(decimals);
                let amount = Decimal::from(value) / scale;
                amount.to_fixed(decimals as usize)
            });
            prompt_until_valid(
                || {
                    let mut input = Input::with_theme(theme).with_prompt(prompt);
                    if let Some(default) = default.clone() {
                        input = input.default(default);
                    }
                    input.interact_text()
                },
                |value| asset_units_to_base_units(&value, decimals),
            )
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
            prompt_until_valid(
                || {
                    let mut input = Input::with_theme(theme).with_prompt(prompt);
                    if let Some(default) = default.clone() {
                        input = input.default(default);
                    }
                    input.interact_text()
                },
                |value| eth_units_to_base_units(&value, decimals, borrow_price_usd, eth_price_usd),
            )
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
            prompt_until_valid(
                || {
                    let mut input = Input::with_theme(theme).with_prompt(prompt);
                    if let Some(default) = default.clone() {
                        input = input.default(default);
                    }
                    input.interact_text()
                },
                |value| {
                    near_units_to_base_units(&value, decimals, borrow_price_usd, near_price_usd)
                },
            )
        }
    }
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
    let ceil = base_units.to_u128_ceil().ok_or_else(|| {
        CliError::InvalidInput("Amount is too large to convert to base units".into())
    })?;

    Ok(ceil)
}

#[cfg(test)]
mod tests {
    use super::{
        amount_decimal_to_base_units, asset_units_to_base_units, eth_units_to_base_units,
        near_units_to_base_units,
    };
    use crate::CliError;
    use rstest::rstest;
    use std::str::FromStr;
    use templar_common::Decimal;

    // ===== amount_decimal_to_base_units tests =====

    #[test]
    fn amount_decimal_to_base_units_uses_ceil_for_rounding() {
        let amount = Decimal::from_str("0.04").expect("valid decimal");
        let base_units = amount_decimal_to_base_units(amount, 7).expect("conversion succeeds");
        assert_eq!(base_units, 400_000);
    }

    #[test]
    fn amount_decimal_to_base_units_exact_scale_rounds_exactly() {
        let amount = Decimal::from_str("1.5").expect("valid decimal");
        let base_units = amount_decimal_to_base_units(amount, 2).expect("conversion succeeds");
        assert_eq!(base_units, 150);
    }

    #[rstest]
    #[case("0", 6, 0)]
    #[case("1", 6, 1_000_000)]
    #[case("1.0", 6, 1_000_000)]
    #[case("0.000001", 6, 1)]
    #[case("0.0000001", 6, 1)] // Rounds up due to ceil
    #[case("100", 0, 100)]
    #[case("100.5", 0, 101)] // Rounds up due to ceil
    #[case("1", 18, 1_000_000_000_000_000_000)]
    fn amount_decimal_to_base_units_various_cases(
        #[case] input: &str,
        #[case] decimals: i32,
        #[case] expected: u128,
    ) {
        let amount = Decimal::from_str(input).expect("valid decimal");
        let result = amount_decimal_to_base_units(amount, decimals).expect("conversion succeeds");
        assert_eq!(result, expected);
    }

    // Note: Decimal type from templar_common does not support negative values,
    // so we cannot directly test negative rejection in amount_decimal_to_base_units.
    // The negative check is tested via eth_units_to_base_units and near_units_to_base_units
    // which parse strings and can detect negatives.

    // ===== asset_units_to_base_units tests =====

    #[rstest]
    #[case("1.0", 6, 1_000_000)]
    #[case("0.5", 6, 500_000)]
    #[case("0.001", 6, 1_000)]
    #[case("100", 8, 10_000_000_000)]
    #[case("0", 6, 0)]
    #[case("1.123456", 6, 1_123_456)]
    fn asset_units_to_base_units_valid_inputs(
        #[case] input: &str,
        #[case] decimals: i32,
        #[case] expected: u128,
    ) {
        let result = asset_units_to_base_units(input, decimals).expect("conversion succeeds");
        assert_eq!(result, expected);
    }

    #[rstest]
    #[case("invalid")]
    #[case("abc")]
    #[case("hello world")]
    #[case("12abc")]
    fn asset_units_to_base_units_invalid_inputs(#[case] input: &str) {
        let result = asset_units_to_base_units(input, 6);
        assert!(result.is_err());
    }

    // ===== eth_units_to_base_units tests =====

    #[test]
    fn eth_units_to_base_units_converts_correctly() {
        // 1 ETH at $2000, borrow asset at $1 = 2000 asset units
        let eth_price = Decimal::from(2000u64);
        let borrow_price = Decimal::from(1u64);
        let result = eth_units_to_base_units("1", 6, &borrow_price, &eth_price)
            .expect("conversion succeeds");
        assert_eq!(result, 2_000_000_000); // 2000 * 10^6
    }

    #[test]
    fn eth_units_to_base_units_fractional_eth() {
        // 0.5 ETH at $2000, borrow asset at $1 = 1000 asset units
        let eth_price = Decimal::from(2000u64);
        let borrow_price = Decimal::from(1u64);
        let result = eth_units_to_base_units("0.5", 6, &borrow_price, &eth_price)
            .expect("conversion succeeds");
        assert_eq!(result, 1_000_000_000); // 1000 * 10^6
    }

    #[test]
    fn eth_units_to_base_units_different_prices() {
        // 1 ETH at $3000, borrow asset at $0.5 = 6000 asset units
        let eth_price = Decimal::from(3000u64);
        let borrow_price = Decimal::from_str("0.5").expect("valid decimal");
        let result = eth_units_to_base_units("1", 6, &borrow_price, &eth_price)
            .expect("conversion succeeds");
        assert_eq!(result, 6_000_000_000); // 6000 * 10^6
    }

    #[test]
    fn eth_units_to_base_units_zero_amount() {
        let eth_price = Decimal::from(2000u64);
        let borrow_price = Decimal::from(1u64);
        let result = eth_units_to_base_units("0", 6, &borrow_price, &eth_price)
            .expect("conversion succeeds");
        assert_eq!(result, 0);
    }

    #[test]
    fn eth_units_to_base_units_rejects_negative() {
        let eth_price = Decimal::from(2000u64);
        let borrow_price = Decimal::from(1u64);
        let result = eth_units_to_base_units("-1", 6, &borrow_price, &eth_price);
        assert!(result.is_err());
        assert!(matches!(result, Err(CliError::InvalidInput(_))));
    }

    #[test]
    fn eth_units_to_base_units_rejects_zero_eth_price() {
        let eth_price = Decimal::ZERO;
        let borrow_price = Decimal::from(1u64);
        let result = eth_units_to_base_units("1", 6, &borrow_price, &eth_price);
        assert!(result.is_err());
    }

    #[test]
    fn eth_units_to_base_units_rejects_zero_borrow_price() {
        let eth_price = Decimal::from(2000u64);
        let borrow_price = Decimal::ZERO;
        let result = eth_units_to_base_units("1", 6, &borrow_price, &eth_price);
        assert!(result.is_err());
    }

    #[test]
    fn eth_units_to_base_units_rejects_invalid_input() {
        let eth_price = Decimal::from(2000u64);
        let borrow_price = Decimal::from(1u64);
        let result = eth_units_to_base_units("invalid", 6, &borrow_price, &eth_price);
        assert!(result.is_err());
    }

    // ===== near_units_to_base_units tests =====

    #[test]
    fn near_units_to_base_units_converts_correctly() {
        // 1 NEAR at $5, borrow asset at $1 = 5 asset units
        let near_price = Decimal::from(5u64);
        let borrow_price = Decimal::from(1u64);
        let result = near_units_to_base_units("1", 6, &borrow_price, &near_price)
            .expect("conversion succeeds");
        assert_eq!(result, 5_000_000); // 5 * 10^6
    }

    #[test]
    fn near_units_to_base_units_fractional_near() {
        // 10 NEAR at $5, borrow asset at $1 = 50 asset units
        let near_price = Decimal::from(5u64);
        let borrow_price = Decimal::from(1u64);
        let result = near_units_to_base_units("10", 6, &borrow_price, &near_price)
            .expect("conversion succeeds");
        assert_eq!(result, 50_000_000); // 50 * 10^6
    }

    #[test]
    fn near_units_to_base_units_different_prices() {
        // 2 NEAR at $4, borrow asset at $0.5 = 16 asset units
        let near_price = Decimal::from(4u64);
        let borrow_price = Decimal::from_str("0.5").expect("valid decimal");
        let result = near_units_to_base_units("2", 6, &borrow_price, &near_price)
            .expect("conversion succeeds");
        assert_eq!(result, 16_000_000); // 16 * 10^6
    }

    #[test]
    fn near_units_to_base_units_zero_amount() {
        let near_price = Decimal::from(5u64);
        let borrow_price = Decimal::from(1u64);
        let result = near_units_to_base_units("0", 6, &borrow_price, &near_price)
            .expect("conversion succeeds");
        assert_eq!(result, 0);
    }

    #[test]
    fn near_units_to_base_units_rejects_negative() {
        let near_price = Decimal::from(5u64);
        let borrow_price = Decimal::from(1u64);
        let result = near_units_to_base_units("-1", 6, &borrow_price, &near_price);
        assert!(result.is_err());
        assert!(matches!(result, Err(CliError::InvalidInput(_))));
    }

    #[test]
    fn near_units_to_base_units_rejects_zero_near_price() {
        let near_price = Decimal::ZERO;
        let borrow_price = Decimal::from(1u64);
        let result = near_units_to_base_units("1", 6, &borrow_price, &near_price);
        assert!(result.is_err());
    }

    #[test]
    fn near_units_to_base_units_rejects_zero_borrow_price() {
        let near_price = Decimal::from(5u64);
        let borrow_price = Decimal::ZERO;
        let result = near_units_to_base_units("1", 6, &borrow_price, &near_price);
        assert!(result.is_err());
    }

    #[test]
    fn near_units_to_base_units_rejects_invalid_input() {
        let near_price = Decimal::from(5u64);
        let borrow_price = Decimal::from(1u64);
        let result = near_units_to_base_units("not_a_number", 6, &borrow_price, &near_price);
        assert!(result.is_err());
    }
}
