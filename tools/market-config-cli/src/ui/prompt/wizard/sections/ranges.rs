use crate::{
    ui::prompt::{
        ranges::{apply_ranges_to_builder, prompt_ranges_with_validation, RangeDefaults},
        wizard::{
            price::{price_header_line, print_price_hint, refresh_price_contexts},
            types::{price_decimal, PriceHintContext},
        },
    },
    CliError, CliResult, ConfigBuilder,
};
use dialoguer::theme::ColorfulTheme;
use near_sdk::json_types::U128;
use std::cell::RefCell;
use templar_common::{market::MarketConfiguration, number::Decimal, utils::Network};

/// Prompts for position ranges during interactive mode.
pub fn prompt_ranges(
    theme: &ColorfulTheme,
    builder: ConfigBuilder,
    borrow_price_context: &RefCell<Option<PriceHintContext>>,
    collateral_price_context: &RefCell<Option<PriceHintContext>>,
    eth_price_usd: &RefCell<Option<Decimal>>,
    near_price_usd: &RefCell<Option<Decimal>>,
) -> CliResult<ConfigBuilder> {
    let defaults = RangeDefaults {
        borrow_min: 1_000_000,
        borrow_max: None,
        supply_min: 1_000_000,
        supply_max: None,
        withdrawal_min: 1_000_000,
        withdrawal_max: None,
    };

    let mut hint = |label: &str, amount: u128| {
        print_price_hint(borrow_price_context, label, amount);
    };
    let asset_decimals = builder
        .price_oracle_inputs()
        .map(|(_, _, _, borrow_decimals, _, _)| borrow_decimals);
    let borrow_price_usd = borrow_price_context
        .borrow()
        .as_ref()
        .and_then(|ctx| price_decimal(&ctx.price));
    let eth_price = *eth_price_usd.borrow();
    let near_price = *near_price_usd.borrow();
    let selection = prompt_ranges_with_validation(
        theme,
        &defaults,
        price_header_line(
            borrow_price_context,
            collateral_price_context,
            eth_price_usd,
            near_price_usd,
        ),
        asset_decimals,
        borrow_price_usd,
        eth_price,
        near_price,
        &mut hint,
        |selection| {
            let mut tmp = builder.clone();
            tmp = apply_ranges_to_builder(tmp, selection)?;
            let _ = tmp;
            Ok(())
        },
    )?;

    apply_ranges_to_builder(builder, &selection)
}

/// Edits position ranges on an existing market configuration.
pub async fn edit_ranges(
    theme: &ColorfulTheme,
    config: &mut MarketConfiguration,
    network: Network,
    borrow_price_context: &RefCell<Option<PriceHintContext>>,
    collateral_price_context: &RefCell<Option<PriceHintContext>>,
    eth_price_usd: &RefCell<Option<Decimal>>,
    near_price_usd: &RefCell<Option<Decimal>>,
) -> CliResult<()> {
    refresh_price_contexts(
        network,
        config.price_oracle_configuration.account_id.clone(),
        config.price_oracle_configuration.borrow_asset_price_id,
        config.price_oracle_configuration.collateral_asset_price_id,
        config.price_oracle_configuration.borrow_asset_decimals,
        config.price_oracle_configuration.collateral_asset_decimals,
        config.price_oracle_configuration.price_maximum_age_s,
        borrow_price_context,
        collateral_price_context,
        eth_price_usd,
        near_price_usd,
    )
    .await;

    let defaults = RangeDefaults {
        borrow_min: U128::from(config.borrow_range.minimum).0,
        borrow_max: config.borrow_range.maximum.map(|v| U128::from(v).0),
        supply_min: U128::from(config.supply_range.minimum).0,
        supply_max: config.supply_range.maximum.map(|v| U128::from(v).0),
        withdrawal_min: U128::from(config.supply_withdrawal_range.minimum).0,
        withdrawal_max: config
            .supply_withdrawal_range
            .maximum
            .map(|v| U128::from(v).0),
    };

    let mut hint = |label: &str, amount: u128| {
        print_price_hint(borrow_price_context, label, amount);
    };
    let asset_decimals = Some(config.price_oracle_configuration.borrow_asset_decimals);
    let borrow_price_usd = borrow_price_context
        .borrow()
        .as_ref()
        .and_then(|ctx| price_decimal(&ctx.price));
    let eth_price = *eth_price_usd.borrow();
    let near_price = *near_price_usd.borrow();
    let selection = prompt_ranges_with_validation(
        theme,
        &defaults,
        price_header_line(
            borrow_price_context,
            collateral_price_context,
            eth_price_usd,
            near_price_usd,
        ),
        asset_decimals,
        borrow_price_usd,
        eth_price,
        near_price,
        &mut hint,
        |sel| {
            let _: templar_common::market::ValidAmountRange<templar_common::asset::BorrowAsset> =
                (sel.borrow_min, sel.borrow_max)
                    .try_into()
                    .map_err(|e: std::io::Error| CliError::Validation(e.to_string()))?;
            let _: templar_common::market::ValidAmountRange<templar_common::asset::BorrowAsset> =
                (sel.supply_min, sel.supply_max)
                    .try_into()
                    .map_err(|e: std::io::Error| CliError::Validation(e.to_string()))?;
            let _: templar_common::market::ValidAmountRange<templar_common::asset::BorrowAsset> =
                (sel.withdrawal_min, sel.withdrawal_max)
                    .try_into()
                    .map_err(|e: std::io::Error| CliError::Validation(e.to_string()))?;
            Ok(())
        },
    )?;

    config.borrow_range = (selection.borrow_min, selection.borrow_max)
        .try_into()
        .map_err(|e: std::io::Error| CliError::Validation(e.to_string()))?;
    config.supply_range = (selection.supply_min, selection.supply_max)
        .try_into()
        .map_err(|e: std::io::Error| CliError::Validation(e.to_string()))?;
    config.supply_withdrawal_range = (selection.withdrawal_min, selection.withdrawal_max)
        .try_into()
        .map_err(|e: std::io::Error| CliError::Validation(e.to_string()))?;

    Ok(())
}
