#![allow(
    clippy::unwrap_used,
    reason = "test harness helpers intentionally fail fast on setup and assertion errors"
)]

use std::{num::NonZero, str::FromStr};

use near_sdk::{
    json_types::{I64, U64},
    AccountId,
};
use templar_common::{
    asset::FungibleAsset,
    dec,
    fee::{Fee, TimeBasedFee},
    interest_rate_strategy::InterestRateStrategy,
    market::{MarketConfiguration, PriceOracleConfiguration, YieldWeights},
    oracle::pyth::{self, PriceIdentifier, PythTimestamp},
    vault::{
        prelude::{Wad, MAX_MANAGEMENT_FEE_WAD, MAX_PERFORMANCE_FEE_WAD},
        Fee as VaultFee, Fees as VaultFees, VaultConfiguration,
    },
    Decimal,
};

pub const DEFAULT_COLLATERAL_PRICE_ID: PriceIdentifier = PriceIdentifier(hex_literal::hex!(
    "cccccccc232290221461220bd4e2acd1dcdfbc89c84092c93c18bdc7756c1588"
));
pub const DEFAULT_BORROW_PRICE_ID: PriceIdentifier = PriceIdentifier(hex_literal::hex!(
    "bbbbbbbbf4f61076456d1a73b14c7edc1cf5cef4f4d6193a33424288f11bd0f4"
));

pub mod partial;
pub mod pyth_price_id;
pub mod test_signer;

pub fn to_price(price: f64) -> pyth::Price {
    pyth::Price {
        price: I64((price * 10000.0) as i64),
        conf: U64(0),
        expo: -4,
        publish_time: PythTimestamp::from_secs(0),
    }
}

pub fn market_configuration(
    price_oracle_id: AccountId,
    borrow_asset_id: AccountId,
    collateral_asset_id: AccountId,
    protocol_account_id: AccountId,
    yield_weights: YieldWeights,
) -> MarketConfiguration {
    MarketConfiguration {
        time_chunk_configuration: templar_common::time_chunk::TimeChunkConfiguration::new(1),
        borrow_asset: FungibleAsset::nep141(borrow_asset_id),
        collateral_asset: FungibleAsset::nep141(collateral_asset_id),
        price_oracle_configuration: PriceOracleConfiguration {
            account_id: price_oracle_id,
            collateral_asset_price_id: DEFAULT_COLLATERAL_PRICE_ID,
            collateral_asset_decimals: 24,
            borrow_asset_price_id: DEFAULT_BORROW_PRICE_ID,
            borrow_asset_decimals: 24,
            price_maximum_age_s: 60,
        },
        borrow_mcr_maintenance: Decimal::from_str("1.25").unwrap(),
        borrow_mcr_liquidation: Decimal::from_str("1.2").unwrap(),
        borrow_asset_maximum_usage_ratio: Decimal::from_str("0.99").unwrap(),
        borrow_origination_fee: Fee::Proportional(Decimal::from_str("0.1").unwrap()),
        borrow_interest_rate_strategy: InterestRateStrategy::piecewise(
            Decimal::ZERO,
            dec!("0.9"),
            dec!("0.04"),
            dec!("0.6"),
        )
        .unwrap(),
        borrow_maximum_duration_ms: None,
        borrow_range: (1, None).try_into().unwrap(),
        supply_range: (1, None).try_into().unwrap(),
        supply_withdrawal_range: (1, None).try_into().unwrap(),
        supply_withdrawal_fee: TimeBasedFee::zero(),
        liquidation_maximum_spread: Decimal::from_str("0.05").unwrap(),
        yield_weights,
        protocol_account_id,
    }
}

pub fn vault_configuration(
    owner_id: AccountId,
    curator_id: AccountId,
    _guardian_id: AccountId,
    sentinel_id: AccountId,
    borrow_asset_id: AccountId,
    skim_recipient_id: AccountId,
    fee_recipient_id: AccountId,
) -> VaultConfiguration {
    VaultConfiguration {
        owner: owner_id,
        curator: curator_id,
        sentinel: sentinel_id,
        underlying_token: FungibleAsset::nep141(borrow_asset_id),
        initial_timelock_ns: templar_common::vault::MIN_TIMELOCK_NS.into(),
        fees: VaultFees {
            performance: VaultFee {
                fee: Wad::from(MAX_PERFORMANCE_FEE_WAD),
                recipient: fee_recipient_id.clone(),
            },
            management: VaultFee {
                fee: Wad::from(MAX_MANAGEMENT_FEE_WAD),
                recipient: fee_recipient_id,
            },
            max_total_assets_growth_rate: None,
        },
        skim_recipient: skim_recipient_id,
        name: "Vault".to_string(),
        symbol: "VAULT".to_string(),
        decimals: NonZero::new(24).unwrap(),
        restrictions: None,
        refresh_cooldown_ns: None,
        idle_resync_cooldown_ns: None,
        withdrawal_cooldown_ns: Some(U64(0)),
    }
}
