use near_sdk::{near, AccountId, Promise};

use crate::{
    asset::{AssetClass, BorrowAsset, CollateralAsset, CollateralAssetAmount, FungibleAssetAmount},
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
    // pub use_exponential_moving_average: bool,
    pub price_maximum_age_s: u32,
}

impl BalanceOracleConfiguration {
    pub fn retrieve_price_pair(&self) -> Promise {
        ext_pyth::ext(self.account_id.clone()).list_ema_prices_no_older_than(
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
            self.collateral_asset_decimals,
            oracle_response
                .get(&self.collateral_asset_price_id)
                .and_then(|o| o.as_ref())
                .ok_or(error::RetrievalError::MissingPrice)?,
            self.borrow_asset_decimals,
            oracle_response
                .get(&self.borrow_asset_price_id)
                .and_then(|o| o.as_ref())
                .ok_or(error::RetrievalError::MissingPrice)?,
        )?)
    }
}

#[derive(Clone, Debug)]
pub struct Price {
    publish_time_s: u64,
    price: u64,
    confidence: u64,
    exponent_10: i32,
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
    }
}

impl TryFrom<pyth::Price> for Price {
    type Error = error::PriceDataError;

    fn try_from(value: pyth::Price) -> Result<Self, Self::Error> {
        (&value).try_into()
    }
}

impl TryFrom<&pyth::Price> for Price {
    type Error = error::PriceDataError;

    fn try_from(pyth_price: &pyth::Price) -> Result<Self, Self::Error> {
        if pyth_price.price.0 < 0 {
            return Err(error::PriceDataError::NegativePrice);
        }

        #[allow(clippy::cast_sign_loss)]
        let price = pyth_price.price.0 as u64;

        if pyth_price.conf.0 >= price {
            return Err(error::PriceDataError::ConfidenceIntervalTooLarge);
        }

        Ok(Self {
            // We assume that it is a current timestamp (>0).
            #[allow(clippy::unwrap_used)]
            publish_time_s: u64::try_from(pyth_price.publish_time).unwrap(),
            price,
            confidence: pyth_price.conf.0,
            exponent_10: pyth_price.expo,
        })
    }
}

#[derive(Clone, Debug)]
pub struct PricePair {
    collateral_asset_decimals: i32,
    collateral_asset_price: Price,
    borrow_asset_decimals: i32,
    borrow_asset_price: Price,
}

impl PricePair {
    /// # Errors
    ///
    /// If the price data are invalid.
    pub fn new(
        collateral_asset_decimals: i32,
        collateral_asset_price: &pyth::Price,
        borrow_asset_decimals: i32,
        borrow_asset_price: &pyth::Price,
    ) -> Result<Self, error::PriceDataError> {
        Ok(Self {
            collateral_asset_decimals,
            collateral_asset_price: collateral_asset_price.try_into()?,
            borrow_asset_decimals,
            borrow_asset_price: borrow_asset_price.try_into()?,
        })
    }
}

pub trait AssetConversion<F: AssetClass, T: AssetClass> {
    fn convert_optimistic(&self, amount: FungibleAssetAmount<F>) -> FungibleAssetAmount<T>;
    fn convert_pessimistic(&self, amount: FungibleAssetAmount<F>) -> FungibleAssetAmount<T>;
}

impl AssetConversion<CollateralAsset, BorrowAsset> for PricePair {
    fn convert_optimistic(
        &self,
        amount: FungibleAssetAmount<CollateralAsset>,
    ) -> FungibleAssetAmount<BorrowAsset> {
        (Decimal::from(amount.as_u128())
            * (u128::from(self.collateral_asset_price.price)
                + u128::from(self.collateral_asset_price.confidence))
            / (u128::from(self.borrow_asset_price.price)
                - u128::from(self.borrow_asset_price.confidence))
            * Decimal::from(10u32).pow(
                self.collateral_asset_price.exponent_10
                    - self.collateral_asset_decimals
                    - self.borrow_asset_price.exponent_10
                    + self.borrow_asset_decimals,
            ))
        .to_u128_ceil()
        .unwrap()
        .into()
    }

    fn convert_pessimistic(
        &self,
        amount: FungibleAssetAmount<CollateralAsset>,
    ) -> FungibleAssetAmount<BorrowAsset> {
        (Decimal::from(amount.as_u128())
            * (u128::from(self.collateral_asset_price.price)
                - u128::from(self.collateral_asset_price.confidence))
            / (u128::from(self.borrow_asset_price.price)
                + u128::from(self.borrow_asset_price.confidence))
            * Decimal::from(10u32).pow(
                self.collateral_asset_price.exponent_10
                    - self.collateral_asset_decimals
                    - self.borrow_asset_price.exponent_10
                    + self.borrow_asset_decimals,
            ))
        .to_u128_floor()
        .unwrap()
        .into()
    }
}

impl AssetConversion<BorrowAsset, CollateralAsset> for PricePair {
    fn convert_optimistic(
        &self,
        amount: FungibleAssetAmount<BorrowAsset>,
    ) -> FungibleAssetAmount<CollateralAsset> {
        (Decimal::from(amount.as_u128())
            * (u128::from(self.borrow_asset_price.price)
                + u128::from(self.borrow_asset_price.confidence))
            / (u128::from(self.collateral_asset_price.price)
                - u128::from(self.collateral_asset_price.confidence))
            * Decimal::from(10u32).pow(
                self.borrow_asset_price.exponent_10
                    - self.borrow_asset_decimals
                    - self.collateral_asset_price.exponent_10
                    + self.collateral_asset_decimals,
            ))
        .to_u128_ceil()
        .unwrap()
        .into()
    }

    fn convert_pessimistic(
        &self,
        amount: FungibleAssetAmount<BorrowAsset>,
    ) -> FungibleAssetAmount<CollateralAsset> {
        (Decimal::from(amount.as_u128())
            * (u128::from(self.borrow_asset_price.price)
                - u128::from(self.borrow_asset_price.confidence))
            / (u128::from(self.collateral_asset_price.price)
                + u128::from(self.collateral_asset_price.confidence))
            * Decimal::from(10u32).pow(
                self.borrow_asset_price.exponent_10
                    - self.borrow_asset_decimals
                    - self.collateral_asset_price.exponent_10
                    + self.collateral_asset_decimals,
            ))
        .to_u128_floor()
        .unwrap()
        .into()
    }
}

pub trait AssetValuation<T: AssetClass> {
    fn value_optimistic(&self, amount: FungibleAssetAmount<T>) -> Decimal;
    fn value_pessimistic(&self, amount: FungibleAssetAmount<T>) -> Decimal;
}

impl AssetValuation<BorrowAsset> for PricePair {
    fn value_optimistic(&self, amount: FungibleAssetAmount<BorrowAsset>) -> Decimal {
        Decimal::from(amount.as_u128())
            * (u128::from(self.borrow_asset_price.price)
                + u128::from(self.borrow_asset_price.confidence))
            * Decimal::from(10u32)
                .pow(self.borrow_asset_price.exponent_10 - self.borrow_asset_decimals)
    }

    fn value_pessimistic(&self, amount: FungibleAssetAmount<BorrowAsset>) -> Decimal {
        Decimal::from(amount.as_u128())
            * (u128::from(self.borrow_asset_price.price)
                - u128::from(self.borrow_asset_price.confidence))
            * Decimal::from(10u32)
                .pow(self.borrow_asset_price.exponent_10 - self.borrow_asset_decimals)
    }
}

impl AssetValuation<CollateralAsset> for PricePair {
    fn value_optimistic(&self, amount: CollateralAssetAmount) -> Decimal {
        Decimal::from(amount.as_u128())
            * (u128::from(self.collateral_asset_price.price)
                + u128::from(self.collateral_asset_price.confidence))
            * Decimal::from(10u32)
                .pow(self.collateral_asset_price.exponent_10 - self.collateral_asset_decimals)
    }

    fn value_pessimistic(&self, amount: CollateralAssetAmount) -> Decimal {
        Decimal::from(amount.as_u128())
            * (u128::from(self.collateral_asset_price.price)
                - u128::from(self.collateral_asset_price.confidence))
            * Decimal::from(10u32)
                .pow(self.collateral_asset_price.exponent_10 - self.collateral_asset_decimals)
    }
}
