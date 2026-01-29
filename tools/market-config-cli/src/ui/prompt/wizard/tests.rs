use super::assets::asset_defaults;
use super::types::AssetStandard;
use crate::ui::prompt::ranges::{apply_ranges_to_builder, RangeSelection};
use crate::ConfigBuilder;
use rstest::rstest;
use std::str::FromStr;
use templar_common::{
    asset::{BorrowAsset, FungibleAsset},
    fee::Fee,
    interest_rate_strategy::InterestRateStrategy,
    market::YieldWeights,
    number::Decimal,
};

fn base_builder() -> ConfigBuilder {
    ConfigBuilder::new()
        .time_chunk_duration_ms(600_000)
        .borrow_asset("usdc.near")
        .unwrap()
        .collateral_asset("wnear.near")
        .unwrap()
        .oracle_account_id("pyth-oracle.near")
        .unwrap()
        .borrow_price_id([0xbb; 32])
        .borrow_decimals(6)
        .collateral_price_id([0xaa; 32])
        .collateral_decimals(24)
        .price_max_age_s(60)
        .borrow_mcr_maintenance(Decimal::from_str("1.25").unwrap())
        .borrow_mcr_liquidation(Decimal::from_str("1.20").unwrap())
        .borrow_max_usage_ratio(Decimal::from_str("0.90").unwrap())
        .borrow_origination_fee(Fee::zero())
        .borrow_interest_rate_strategy(
            InterestRateStrategy::linear(
                Decimal::from_str("0.01").unwrap(),
                Decimal::from_str("0.10").unwrap(),
            )
            .unwrap(),
        )
        .borrow_max_duration_ms(None)
        .supply_withdrawal_fee(templar_common::fee::TimeBasedFee::zero())
        .yield_weights(YieldWeights::new_with_supply_weight(9))
        .protocol_account_id("protocol.near")
        .unwrap()
        .liquidation_max_spread(Decimal::from_str("0.05").unwrap())
}

#[rstest]
#[case(
    FungibleAsset::<BorrowAsset>::nep141("usdc.near".parse().unwrap()),
    AssetStandard::Nep141,
    "usdc.near",
    None
)]
#[case(
    FungibleAsset::<BorrowAsset>::nep245("mt.near".parse().unwrap(), "btc-token".to_string()),
    AssetStandard::Nep245,
    "mt.near",
    Some("btc-token")
)]
fn asset_defaults_handles_assets(
    #[case] asset: FungibleAsset<BorrowAsset>,
    #[case] expected_standard: AssetStandard,
    #[case] expected_contract: &str,
    #[case] expected_token: Option<&str>,
) {
    let (standard, contract, token) = asset_defaults(&asset);
    assert!(
        matches!(standard, s if matches!(expected_standard, AssetStandard::Nep141) && matches!(s, AssetStandard::Nep141)
        || matches!(expected_standard, AssetStandard::Nep245) && matches!(s, AssetStandard::Nep245))
    );
    assert_eq!(contract, expected_contract);
    assert_eq!(token.as_deref(), expected_token);
}

#[rstest]
#[case(Some(10), Some(20), Some(30))]
#[case(None, None, None)]
#[case(Some(50), None, Some(40))]
fn apply_ranges_to_builder_respects_withdrawal_max(
    #[case] borrow_max: Option<u128>,
    #[case] supply_max: Option<u128>,
    #[case] withdrawal_max: Option<u128>,
) {
    let selection = RangeSelection {
        borrow_min: 1,
        borrow_max,
        supply_min: 2,
        supply_max,
        withdrawal_min: 3,
        withdrawal_max,
    };

    let builder = apply_ranges_to_builder(base_builder(), &selection)
        .expect("range application should succeed");
    let config = builder.build().expect("config should build");

    assert_eq!(
        config.borrow_range.maximum.map(u128::from),
        selection.borrow_max
    );
    assert_eq!(
        config.supply_range.maximum.map(u128::from),
        selection.supply_max
    );
    assert_eq!(
        config.supply_withdrawal_range.maximum.map(u128::from),
        selection.withdrawal_max
    );
}
