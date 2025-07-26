use near_sdk::{
    env, json_types::Base64VecU8, near, serde::Serialize, serde_json, AccountId, Gas, NearToken,
    Promise,
};

use crate::{
    asset::{BorrowAsset, CollateralAsset},
    oracle::pyth::{ext_pyth, OracleResponse, PriceIdentifier},
    price::{AmountMultiplier, PricePair},
};

#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct DynamicConversionRate {
    pub account_id: AccountId,
    pub method_name: String,
    pub args: Base64VecU8,
    pub gas: Gas,
    pub mul_pow10: i32,
}

impl DynamicConversionRate {
    pub fn promise(&self) -> Promise {
        Promise::new(self.account_id.clone()).function_call(
            self.method_name.clone(),
            self.args.0.clone(),
            NearToken::from_near(0),
            self.gas,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct PriceOracleAssetDescriptor {
    pub price_id: PriceIdentifier,
    pub decimals: i32,
    pub dynamic_conversion_rate: Option<DynamicConversionRate>,
}

impl PriceOracleAssetDescriptor {
    pub fn new(price_id: PriceIdentifier, decimals: i32) -> Self {
        Self {
            price_id,
            decimals,
            dynamic_conversion_rate: None,
        }
    }

    pub fn lst(
        price_id: PriceIdentifier,
        decimals: i32,
        account_id: AccountId,
        method_name: impl Into<String>,
        args_json: &impl Serialize,
        gas: Gas,
        mul_pow10: i32,
    ) -> Self {
        Self {
            price_id,
            decimals,
            dynamic_conversion_rate: Some(DynamicConversionRate {
                account_id,
                method_name: method_name.into(),
                args: serde_json::to_vec(args_json).unwrap().into(),
                gas,
                mul_pow10,
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct PriceOracleConfiguration {
    pub account_id: AccountId,
    pub collateral_asset: PriceOracleAssetDescriptor,
    pub borrow_asset: PriceOracleAssetDescriptor,
    pub price_maximum_age_s: u32,
}

impl PriceOracleConfiguration {
    // Usually seems to take 1.64 TGas.
    pub const GAS_RETRIEVE_PRICE_PAIR: Gas = Gas::from_tgas(3);

    pub fn retrieve_price_pair(&self) -> Promise {
        let mut promise = ext_pyth::ext(self.account_id.clone())
            .with_static_gas(Self::GAS_RETRIEVE_PRICE_PAIR)
            .list_ema_prices_no_older_than(
                vec![self.borrow_asset.price_id, self.collateral_asset.price_id],
                u64::from(self.price_maximum_age_s),
            );

        if let Some(ref borrow_redemption_rate) = self.borrow_asset.dynamic_conversion_rate {
            promise = promise.and(borrow_redemption_rate.promise())
        }

        if let Some(ref collateral_redemption_rate) = self.collateral_asset.dynamic_conversion_rate
        {
            promise = promise.and(collateral_redemption_rate.promise())
        }

        promise
    }

    pub fn retrieve_price_pair_results_len(&self) -> usize {
        1 + self.borrow_asset.dynamic_conversion_rate.is_some() as usize
            + self.collateral_asset.dynamic_conversion_rate.is_some() as usize
    }

    pub fn create_price_pair(
        &self,
        oracle_response: &OracleResponse,
        borrow_multiplier: Option<AmountMultiplier<BorrowAsset>>,
        collateral_multiplier: Option<AmountMultiplier<CollateralAsset>>,
    ) -> Result<PricePair, error::RetrievalError> {
        Ok(PricePair::new(
            oracle_response
                .get(&self.collateral_asset.price_id)
                .and_then(|o| o.as_ref())
                .ok_or(error::RetrievalError::MissingPrice(
                    self.collateral_asset.price_id,
                ))?,
            self.collateral_asset.decimals,
            collateral_multiplier,
            oracle_response
                .get(&self.borrow_asset.price_id)
                .and_then(|o| o.as_ref())
                .ok_or(error::RetrievalError::MissingPrice(
                    self.borrow_asset.price_id,
                ))?,
            self.borrow_asset.decimals,
            borrow_multiplier,
        )?)
    }

    pub fn create_price_pair_from_raw(
        &self,
        callback_results: &[Vec<u8>],
    ) -> Result<PricePair, error::RetrievalError> {
        let expected_len = self.retrieve_price_pair_results_len();
        let actual_len = callback_results.len();
        if expected_len != actual_len {
            env::panic_str(&format!("Invariant violation: Incorrect number of callback results. Expected {expected_len}, got {actual_len}."))
        }

        let oracle_response = serde_json::from_slice::<OracleResponse>(&callback_results[0])
            .unwrap_or_else(|e| {
                env::panic_str(&e.to_string());
            });

        let borrow_multiplier =
            if let Some(ref borrow_conversion) = self.borrow_asset.dynamic_conversion_rate {
                Some(AmountMultiplier::new(
                    serde_json::from_slice(&callback_results[1])?,
                    borrow_conversion.mul_pow10,
                ))
            } else {
                None
            };
        let collateral_multiplier =
            if let Some(ref conversion) = self.collateral_asset.dynamic_conversion_rate {
                let i = 1 + self.borrow_asset.dynamic_conversion_rate.is_some() as usize;
                Some(AmountMultiplier::new(
                    serde_json::from_slice(&callback_results[i])?,
                    conversion.mul_pow10,
                ))
            } else {
                None
            };

        self.create_price_pair(&oracle_response, borrow_multiplier, collateral_multiplier)
    }
}

pub mod error {
    use thiserror::Error;

    use crate::oracle::pyth::PriceIdentifier;

    #[derive(Debug, Error)]
    #[error("Error retrieving price: {0}")]
    pub enum RetrievalError {
        #[error("Missing price data for {}", near_sdk::serde_json::to_string(.0).unwrap())]
        MissingPrice(PriceIdentifier),
        #[error(transparent)]
        ParseError(#[from] near_sdk::serde_json::Error),
        #[error(transparent)]
        PriceData(#[from] crate::price::error::PriceDataError),
    }
}
