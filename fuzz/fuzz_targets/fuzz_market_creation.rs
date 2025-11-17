#![no_main]
#![cfg(not(target_arch = "wasm32"))]

use std::str::FromStr;

use libfuzzer_sys::fuzz_target;
use near_sdk::AccountId;
use templar_common::{
    asset::FungibleAsset,
    fee::{Fee, TimeBasedFee},
    interest_rate_strategy::InterestRateStrategy,
    market::{MarketConfiguration, PriceOracleConfiguration, ValidAmountRange, YieldWeights},
    number::Decimal,
    oracle::pyth::PriceIdentifier,
    time_chunk::TimeChunkConfiguration,
};

pub const DEFAULT_COLLATERAL_PRICE_ID: PriceIdentifier = PriceIdentifier(hex_literal::hex!(
    "cccccccc232290221461220bd4e2acd1dcdfbc89c84092c93c18bdc7756c1588"
));
pub const DEFAULT_BORROW_PRICE_ID: PriceIdentifier = PriceIdentifier(hex_literal::hex!(
    "bbbbbbbbf4f61076456d1a73b14c7edc1cf5cef4f4d6193a33424288f11bd0f4"
));

fn create_account_id(seed: u8) -> AccountId {
    let name = format!("account{seed}.testnet");
    #[allow(clippy::unwrap_used, reason = "Fuzzing with valid inputs")]
    AccountId::from_str(&name).unwrap()
}

#[allow(clippy::too_many_arguments)]
fn try_create_market_config(
    mcr_maintenance_num: u128,
    mcr_liquidation_num: u128,
    usage_ratio_num: u128,
    liquidation_spread_num: u128,
    borrow_min: u128,
    borrow_max: Option<u128>,
    supply_min: u128,
    supply_max: Option<u128>,
    withdrawal_min: u128,
    withdrawal_max: Option<u128>,
    same_asset: bool,
) -> Option<MarketConfiguration> {
    // Create decimals (divide by 1000 to get values in [0, ~340_282])
    let mcr_maintenance = Decimal::from(mcr_maintenance_num);
    let mcr_liquidation = Decimal::from(mcr_liquidation_num);
    let usage_ratio = Decimal::from(usage_ratio_num % 1001); // [0, 1]
    let liquidation_spread = Decimal::from(liquidation_spread_num % 1000); // [0, 0.999]

    let borrow_asset = FungibleAsset::nep141(create_account_id(1));
    let collateral_asset = if same_asset {
        borrow_asset.clone().coerce()
    } else {
        FungibleAsset::nep141(create_account_id(2))
    };

    // Try to create ranges
    let borrow_range = ValidAmountRange::try_from((borrow_min, borrow_max)).ok()?;
    let supply_range = ValidAmountRange::try_from((supply_min, supply_max)).ok()?;
    let supply_withdrawal_range =
        ValidAmountRange::try_from((withdrawal_min, withdrawal_max)).ok()?;

    let config = MarketConfiguration {
        time_chunk_configuration: TimeChunkConfiguration::new(86_400_000), // 1 day
        borrow_asset,
        collateral_asset,
        price_oracle_configuration: PriceOracleConfiguration {
            account_id: create_account_id(3),
            borrow_asset_price_id: DEFAULT_BORROW_PRICE_ID,
            borrow_asset_decimals: 24,
            collateral_asset_price_id: DEFAULT_COLLATERAL_PRICE_ID,
            collateral_asset_decimals: 24,
            price_maximum_age_s: 60,
        },
        borrow_mcr_maintenance: mcr_maintenance,
        borrow_mcr_liquidation: mcr_liquidation,
        borrow_asset_maximum_usage_ratio: usage_ratio,
        borrow_origination_fee: Fee::zero(),
        #[allow(clippy::unwrap_used, reason = "Fuzzing with valid inputs")]
        borrow_interest_rate_strategy: InterestRateStrategy::linear(
            Decimal::from(5u128),  // 5% base
            Decimal::from(50u128), // 50% max
        )
        .unwrap(),
        borrow_maximum_duration_ms: None,
        borrow_range,
        supply_range,
        supply_withdrawal_range,
        supply_withdrawal_fee: TimeBasedFee::zero(),
        yield_weights: YieldWeights::new_with_supply_weight(1),
        protocol_account_id: create_account_id(4),
        liquidation_maximum_spread: liquidation_spread,
    };

    Some(config)
}

fuzz_target!(|data: (
    u128,
    u128,
    u128,
    u128,
    u128,
    u128,
    u128,
    u128,
    u128,
    u128,
    bool
)| {
    let (
        mcr_maintenance_num,
        mcr_liquidation_num,
        usage_ratio_num,
        liquidation_spread_num,
        borrow_min,
        borrow_max_raw,
        supply_min,
        supply_max_raw,
        withdrawal_min,
        withdrawal_max_raw,
        same_asset,
    ) = data;

    // Convert to options (0 means None)
    let borrow_max = (borrow_max_raw != 0).then_some(borrow_max_raw);
    let supply_max = (supply_max_raw != 0).then_some(supply_max_raw);
    let withdrawal_max = (withdrawal_max_raw != 0).then_some(withdrawal_max_raw);

    let Some(config) = try_create_market_config(
        mcr_maintenance_num,
        mcr_liquidation_num,
        usage_ratio_num,
        liquidation_spread_num,
        borrow_min,
        borrow_max,
        supply_min,
        supply_max,
        withdrawal_min,
        withdrawal_max,
        same_asset,
    ) else {
        return; // Invalid ranges, skip
    };

    // Test validation
    let validation_result = config.validate();

    // Check invariants
    if same_asset {
        // Same asset should always fail validation
        assert!(
            validation_result.is_err(),
            "Same borrow/collateral asset should be invalid"
        );
    }

    if config.borrow_mcr_maintenance <= Decimal::ONE {
        assert!(
            validation_result.is_err(),
            "MCR maintenance <= 1 should be invalid"
        );
    }

    if config.borrow_mcr_liquidation <= Decimal::ONE {
        assert!(
            validation_result.is_err(),
            "MCR liquidation <= 1 should be invalid"
        );
    }

    if config.borrow_mcr_maintenance < config.borrow_mcr_liquidation {
        assert!(
            validation_result.is_err(),
            "MCR maintenance < liquidation should be invalid"
        );
    }

    if config.borrow_asset_maximum_usage_ratio.is_zero()
        || config.borrow_asset_maximum_usage_ratio > Decimal::ONE
    {
        assert!(
            validation_result.is_err(),
            "Usage ratio out of (0, 1] should be invalid"
        );
    }

    if config.liquidation_maximum_spread >= Decimal::ONE {
        assert!(
            validation_result.is_err(),
            "Liquidation spread >= 1 should be invalid"
        );
    }

    // If all checks pass, validation should succeed
    if !same_asset
        && config.borrow_mcr_maintenance > Decimal::ONE
        && config.borrow_mcr_liquidation > Decimal::ONE
        && config.borrow_mcr_maintenance >= config.borrow_mcr_liquidation
        && !config.borrow_asset_maximum_usage_ratio.is_zero()
        && config.borrow_asset_maximum_usage_ratio <= Decimal::ONE
        && config.liquidation_maximum_spread < Decimal::ONE
    {
        assert!(
            validation_result.is_ok(),
            "Valid config should pass validation: {validation_result:?}",
        );
    }
});
