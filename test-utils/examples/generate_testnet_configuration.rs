#![allow(clippy::unwrap_used)]
//! Used by GitHub Actions to generate default market configuration.

use std::str::FromStr;

use near_sdk::serde_json;
use templar_common::{
    asset::{FungibleAsset, FungibleAssetAmount},
    dec,
    fee::{Fee, TimeBasedFee},
    interest_rate_strategy::InterestRateStrategy,
    market::{BalanceOracleConfiguration, MarketConfiguration, YieldWeights},
    number::Decimal,
    oracle::pyth::PriceIdentifier,
};

pub fn main() {
    println!(
        "{{\"configuration\":{}}}",
        serde_json::to_string(&MarketConfiguration {
            borrow_asset: FungibleAsset::nep141("usdt.fakes.testnet".parse().unwrap()),
            collateral_asset: FungibleAsset::nep141("wrap.testnet".parse().unwrap()),
            balance_oracle: BalanceOracleConfiguration {
                account_id: "pyth-oracle.testnet".parse().unwrap(),
                borrow_asset_price_id: PriceIdentifier(hex_literal::hex!(
                    "27e867f0f4f61076456d1a73b14c7edc1cf5cef4f4d6193a33424288f11bd0f4"
                )),
                borrow_asset_decimals: 6,
                collateral_asset_price_id: PriceIdentifier(hex_literal::hex!(
                    "1fc18861232290221461220bd4e2acd1dcdfbc89c84092c93c18bdc7756c1588"
                )),
                collateral_asset_decimals: 24,
                price_maximum_age_s: 60,
            },
            minimum_initial_collateral_ratio: Decimal::from_str("1.25").unwrap(),
            minimum_collateral_ratio_per_borrow: Decimal::from_str("1.2").unwrap(),
            maximum_borrow_asset_usage_ratio: Decimal::from_str("0.99").unwrap(),
            borrow_origination_fee: Fee::zero(),
            borrow_interest_rate_strategy: InterestRateStrategy::piecewise(
                Decimal::ZERO,
                dec!("0.9"),
                dec!("0.04"),
                dec!("0.6")
            )
            .unwrap(),
            maximum_borrow_duration_ms: None,
            minimum_borrow_amount: FungibleAssetAmount::new(1),
            maximum_borrow_amount: FungibleAssetAmount::new(u128::MAX),
            supply_withdrawal_fee: TimeBasedFee::zero(),
            yield_weights: YieldWeights::new_with_supply_weight(1),
            maximum_liquidator_spread: Decimal::from_str("0.05").unwrap(),
        })
        .unwrap(),
    );
}
