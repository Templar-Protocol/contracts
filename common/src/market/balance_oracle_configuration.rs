use std::marker::PhantomData;

use near_sdk::{near, AccountId, Gas, Promise};

use crate::{
    asset::{AssetClass, BorrowAsset, CollateralAsset, FungibleAssetAmount},
    number::Decimal,
    oracle::pyth::{self, ext_pyth, OracleResponse, PriceIdentifier},
};

#[derive(Clone, Debug)]
#[near(serializers = [json, borsh])]
pub struct BalanceOracleConfiguration {
    pub account_id: AccountId,
    pub collateral_asset_price_id: PriceIdentifier,
    pub collateral_asset_decimals: i32,
    pub borrow_asset_price_id: PriceIdentifier,
    pub borrow_asset_decimals: i32,
    pub price_maximum_age_s: u32,
}

impl BalanceOracleConfiguration {
    // Usually seems to take 1.64 TGas.
    pub const GAS_RETRIEVE_PRICE_PAIR: Gas = Gas::from_tgas(3);

    pub fn retrieve_price_pair(&self) -> Promise {
        ext_pyth::ext(self.account_id.clone())
            .with_static_gas(Self::GAS_RETRIEVE_PRICE_PAIR)
            .list_ema_prices_no_older_than(
                vec![self.borrow_asset_price_id, self.collateral_asset_price_id],
                u64::from(self.price_maximum_age_s),
            )
    }

    /// # Errors
    ///
    /// If the response from the oracle does not contain valid prices for the
    /// configured asset pair.
    pub fn create_price_pair(
        &self,
        oracle_response: &OracleResponse,
    ) -> Result<PricePair, error::RetrievalError> {
        Ok(PricePair::new(
            oracle_response
                .get(&self.collateral_asset_price_id)
                .and_then(|o| o.as_ref())
                .ok_or(error::RetrievalError::MissingPrice)?,
            self.collateral_asset_decimals,
            oracle_response
                .get(&self.borrow_asset_price_id)
                .and_then(|o| o.as_ref())
                .ok_or(error::RetrievalError::MissingPrice)?,
            self.borrow_asset_decimals,
        )?)
    }
}

#[derive(Clone, Debug)]
pub struct Price<T: AssetClass> {
    _asset: PhantomData<T>,
    price: u128,
    confidence: u128,
    power_of_10: Decimal,
}

mod error {
    use thiserror::Error;

    #[derive(Clone, Debug, Error)]
    #[error("Error retrieving price: {0}")]
    pub enum RetrievalError {
        #[error("Missing price")]
        MissingPrice,
        #[error(transparent)]
        PriceData(#[from] PriceDataError),
    }

    #[derive(Clone, Debug, Error)]
    #[error("Bad price data: {0}")]
    pub enum PriceDataError {
        #[error("Reported negative price")]
        NegativePrice,
        #[error("Confidence interval too large")]
        ConfidenceIntervalTooLarge,
        #[error("Exponent too large")]
        ExponentTooLarge,
    }
}

// Maximum number of fully-representable whole digits in 384 bits: floor(log_10(2^384)) = 115
// Decimal digits: floor(128 / log_2(10)) = 38
const MAXIMUM_POSITIVE_EXPONENT: i32 = 115 - 38;

fn from_pyth_price<T: AssetClass>(
    pyth_price: &pyth::Price,
    decimals: i32,
) -> Result<Price<T>, error::PriceDataError> {
    let Ok(price) = u64::try_from(pyth_price.price.0) else {
        return Err(error::PriceDataError::NegativePrice);
    };

    if pyth_price.conf.0 >= price {
        return Err(error::PriceDataError::ConfidenceIntervalTooLarge);
    }

    if pyth_price.expo > MAXIMUM_POSITIVE_EXPONENT {
        return Err(error::PriceDataError::ExponentTooLarge);
    }

    // TODO: If price falls below minimum representation, it will get truncated to zero.
    // Is this okay?

    Ok(Price {
        _asset: PhantomData,
        price: u128::from(price),
        confidence: u128::from(pyth_price.conf.0),
        power_of_10: Decimal::TEN.pow(pyth_price.expo - decimals),
    })
}

impl<T: AssetClass> Price<T> {
    pub fn upper_bound(&self) -> Decimal {
        (self.price + self.confidence) * self.power_of_10
    }

    pub fn lower_bound(&self) -> Decimal {
        (self.price - self.confidence) * self.power_of_10
    }

    pub fn value_optimistic(&self, amount: FungibleAssetAmount<T>) -> Decimal {
        Decimal::from(amount) * self.upper_bound()
    }

    pub fn value_pessimistic(&self, amount: FungibleAssetAmount<T>) -> Decimal {
        Decimal::from(amount) * self.lower_bound()
    }
}

#[derive(Clone, Debug)]
pub struct PricePair {
    pub collateral_asset_price: Price<CollateralAsset>,
    pub borrow_asset_price: Price<BorrowAsset>,
}

impl PricePair {
    /// # Errors
    ///
    /// - If the price data are invalid.
    pub fn new(
        collateral_price: &pyth::Price,
        collateral_decimals: i32,
        borrow_price: &pyth::Price,
        borrow_decimals: i32,
    ) -> Result<Self, error::PriceDataError> {
        Ok(Self {
            collateral_asset_price: from_pyth_price(collateral_price, collateral_decimals)?,
            borrow_asset_price: from_pyth_price(borrow_price, borrow_decimals)?,
        })
    }
}
