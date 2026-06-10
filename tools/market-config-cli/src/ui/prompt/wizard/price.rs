use crate::{logger, oracle::price_validator::fetch_oracle_price};
use std::cell::RefCell;
use templar_common::{oracle::pyth::Price, Decimal};

use super::types::PriceHintContext;

/// Extracts the USD price from a Pyth Price as f64.
#[allow(
    clippy::cast_precision_loss,
    reason = "Precision loss is acceptable for price hints"
)]
pub fn price_usd(price: &Price) -> Option<f64> {
    let price_raw = price.price.0;
    if price_raw <= 0 {
        return None;
    }
    let price_usd = (price_raw as f64) * 10f64.powi(price.expo);
    if !price_usd.is_finite() || price_usd <= 0.0 {
        return None;
    }
    Some(price_usd)
}

/// Computes the price per unit and total USD value for an amount.
#[allow(
    clippy::cast_precision_loss,
    reason = "Precision loss is acceptable for price hints"
)]
pub fn price_hint_amount(price: &Price, asset_decimals: i32, amount: u128) -> Option<(f64, f64)> {
    let price_usd = price_usd(price)?;

    let units = (amount as f64) / 10f64.powi(asset_decimals);
    if !units.is_finite() {
        return None;
    }

    Some((price_usd, units * price_usd))
}

/// Formats a price value for display.
pub fn format_price(value: f64) -> String {
    if value.abs() >= 1.0 {
        format!("{value:.2}")
    } else {
        format!("{value:.6}")
    }
}

/// Prints a price hint for an amount using the borrow price context.
pub fn print_price_hint(ctx: &RefCell<Option<PriceHintContext>>, label: &str, amount: u128) {
    let ctx_ref = ctx.borrow();
    let Some(ctx) = ctx_ref.as_ref() else {
        return;
    };
    let Some((price_usd_val, total_usd)) =
        price_hint_amount(&ctx.price, ctx.asset_decimals, amount)
    else {
        return;
    };
    println!(
        "At current ${price_usd}, {label} is worth ~${total_usd}",
        price_usd = format_price(price_usd_val),
        total_usd = format_price(total_usd),
    );
}

/// Builds a header line with current price information.
pub fn price_header_line(
    borrow_price_context: &RefCell<Option<PriceHintContext>>,
    collateral_price_context: &RefCell<Option<PriceHintContext>>,
    eth_price_usd: &RefCell<Option<Decimal>>,
    near_price_usd: &RefCell<Option<Decimal>>,
) -> Option<String> {
    let mut lines = Vec::new();

    if let Some(ctx) = borrow_price_context.borrow().as_ref() {
        if let Some(price_usd_val) = price_usd(&ctx.price) {
            lines.push(format!(
                "Borrow/Supply price: ~${}. 1 token = 10^{} base units.",
                format_price(price_usd_val),
                ctx.asset_decimals
            ));
        }
    }

    if let Some(ctx) = collateral_price_context.borrow().as_ref() {
        if let Some(price_usd_val) = price_usd(&ctx.price) {
            lines.push(format!(
                "Collateral price: ~${}. 1 token = 10^{} base units.",
                format_price(price_usd_val),
                ctx.asset_decimals
            ));
        }
    }

    if let Some(eth_price) = eth_price_usd.borrow().as_ref() {
        let eth_price_f64 = eth_price.to_f64_lossy();
        lines.push(format!("ETH price: ~${}", format_price(eth_price_f64)));
    }

    if let Some(near_price) = near_price_usd.borrow().as_ref() {
        let near_price_f64 = near_price.to_f64_lossy();
        lines.push(format!("NEAR price: ~${}", format_price(near_price_f64)));
    }

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

/// Fetches ETH price in USD from Hermes.
pub async fn fetch_eth_price_usd(network: templar_common::utils::Network) -> Option<Decimal> {
    fetch_oracle_price(network, "ETH").await.ok()
}

/// Fetches NEAR price in USD from Hermes.
pub async fn fetch_near_price_usd(network: templar_common::utils::Network) -> Option<Decimal> {
    fetch_oracle_price(network, "NEAR").await.ok()
}

/// Refreshes all price contexts from on-chain and Hermes data.
#[allow(clippy::too_many_arguments)]
pub async fn refresh_price_contexts(
    network: templar_common::utils::Network,
    oracle_account_id: near_sdk::AccountId,
    borrow_price_id: templar_common::oracle::pyth::PriceIdentifier,
    collateral_price_id: templar_common::oracle::pyth::PriceIdentifier,
    borrow_decimals: i32,
    collateral_decimals: i32,
    price_max_age: u32,
    borrow_price_context: &RefCell<Option<PriceHintContext>>,
    collateral_price_context: &RefCell<Option<PriceHintContext>>,
    eth_price_usd: &RefCell<Option<Decimal>>,
    near_price_usd: &RefCell<Option<Decimal>>,
) {
    let client = near_jsonrpc_client::JsonRpcClient::connect(network.rpc_url());
    let prices = match crate::rpc::list_ema_prices_no_older_than(
        &client,
        oracle_account_id,
        vec![borrow_price_id, collateral_price_id],
        u64::from(price_max_age),
    )
    .await
    {
        Ok(prices) => prices,
        Err(err) => {
            logger::warn(format!("Unable to fetch current prices: {err}"));
            *borrow_price_context.borrow_mut() = None;
            *collateral_price_context.borrow_mut() = None;
            return;
        }
    };

    let borrow_price = prices
        .get(&borrow_price_id)
        .and_then(|value| value.as_ref())
        .cloned();

    if let Some(price) = borrow_price {
        *borrow_price_context.borrow_mut() = Some(PriceHintContext {
            price,
            asset_decimals: borrow_decimals,
        });
    } else {
        logger::warn("Borrow price feed returned no price data");
        *borrow_price_context.borrow_mut() = None;
    }

    let collateral_price = prices
        .get(&collateral_price_id)
        .and_then(|value| value.as_ref())
        .cloned();

    if let Some(price) = collateral_price {
        *collateral_price_context.borrow_mut() = Some(PriceHintContext {
            price,
            asset_decimals: collateral_decimals,
        });
    } else {
        *collateral_price_context.borrow_mut() = None;
    }

    let eth_price = fetch_eth_price_usd(network).await;
    if eth_price.is_none() {
        logger::warn("Unable to fetch ETH price from Hermes");
    }
    *eth_price_usd.borrow_mut() = eth_price;

    let near_price = fetch_near_price_usd(network).await;
    if near_price.is_none() {
        logger::warn("Unable to fetch NEAR price from Hermes");
    }
    *near_price_usd.borrow_mut() = near_price;
}
