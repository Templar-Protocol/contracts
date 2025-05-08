use std::{cmp::Ordering, marker::PhantomData};

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
    power_of_10: i32,
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
        // #[error("Exponent too large")]
        // ExponentTooLarge,
    }
}

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

    Ok(Price {
        _asset: PhantomData,
        price: u128::from(price),
        confidence: u128::from(pyth_price.conf.0),
        // TODO: checked_sub
        power_of_10: pyth_price.expo - decimals,
    })
}

impl<T: AssetClass> Price<T> {
    pub fn upper_bound(&self) -> Decimal {
        Decimal::from(self.price + self.confidence).times_10_to_the(self.power_of_10)
    }

    pub fn lower_bound(&self) -> Decimal {
        Decimal::from(self.price - self.confidence).times_10_to_the(self.power_of_10)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Valuation {
    coefficient: u128,
    power_of_10: i32,
}

impl Valuation {
    fn reduce(&mut self) {
        let add_pow_10 = decimal_trailing_zeroes(self.coefficient);
        self.coefficient /= 10u128.pow(u32::from(add_pow_10));
        self.power_of_10 += i32::from(add_pow_10);
    }

    pub fn optimistic<T: AssetClass>(
        amount: FungibleAssetAmount<T>,
        price: &Price<T>,
    ) -> Option<Self> {
        let mut self_ = Self {
            coefficient: u128::from(amount).checked_mul(
                price.price + price.confidence, // guaranteed not to overflow
            )?,
            power_of_10: price.power_of_10,
        };
        self_.reduce();
        Some(self_)
    }

    pub fn pessimistic<T: AssetClass>(
        amount: FungibleAssetAmount<T>,
        price: &Price<T>,
    ) -> Option<Self> {
        let mut self_ = Self {
            coefficient: u128::from(amount).checked_mul(
                price.price - price.confidence, // guaranteed not to overflow
            )?,
            power_of_10: price.power_of_10,
        };
        self_.reduce();
        Some(self_)
    }

    pub fn ratio(self, rhs: Self) -> Option<Decimal> {
        if rhs.coefficient == 0 {
            return None;
        }

        let d = Decimal::from(self.coefficient) / Decimal::from(rhs.coefficient);

        if let Some(power_of_10) = self.power_of_10.checked_sub(rhs.power_of_10) {
            Some(d.times_10_to_the(power_of_10))
        } else {
            // Difference of two i32's can be greater than i32::MAX
            Some(
                d.times_10_to_the(self.power_of_10)
                    .times_10_to_the(-rhs.power_of_10),
            )
        }
    }
}

impl PartialOrd for Valuation {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        let mut value_self = self.coefficient;
        let mut value_other = other.coefficient;

        match self.power_of_10.cmp(&other.power_of_10) {
            Ordering::Less => {
                value_other *= 10u128.pow(self.power_of_10.abs_diff(other.power_of_10));
            }
            Ordering::Equal => {}
            Ordering::Greater => {
                value_self *= 10u128.pow(self.power_of_10.abs_diff(other.power_of_10));
            }
        }

        value_self.partial_cmp(&value_other)
    }
}

impl From<Valuation> for Decimal {
    fn from(value: Valuation) -> Self {
        Decimal::from(value.coefficient).times_10_to_the(value.power_of_10)
    }
}

fn decimal_trailing_zeroes(mut x: u128) -> u8 {
    let mut zeroes = 0;

    while x > 0 && x % 10 == 0 {
        x /= 10;
        zeroes += 1;
    }

    zeroes
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

#[cfg(test)]
mod tests {
    use rand::Rng;

    use super::*;

    // #[test]
    // fn maximum_positive_exponent() {
    //     let _ = Decimal::TEN.pow(MAXIMUM_POSITIVE_EXPONENT);
    // }

    // #[test]
    // #[should_panic = "arithmetic operation overflow"]
    // fn maximum_positive_exponent_overflow() {
    //     let _ = Decimal::TEN.pow(MAXIMUM_POSITIVE_EXPONENT + 1);
    // }

    #[test]
    fn trailing_zeroes() {
        assert_eq!(decimal_trailing_zeroes(0), 0);
        assert_eq!(decimal_trailing_zeroes(1), 0);
        assert_eq!(decimal_trailing_zeroes(10), 1);
        assert_eq!(decimal_trailing_zeroes(100), 2);
        assert_eq!(decimal_trailing_zeroes(34_873_400_000), 5);
        assert_eq!(decimal_trailing_zeroes(348_734_000_001), 0);
        assert_eq!(decimal_trailing_zeroes(7_568_265_868), 0);
        assert_eq!(decimal_trailing_zeroes(3_487_340_000_010_000), 4);
        assert_eq!(decimal_trailing_zeroes(u128::MAX), 0);

        let mut rng = rand::thread_rng();

        for _ in 0..100 {
            let x: u128 = rng.gen();
            let s_original = x.to_string();
            let s_trimmed = s_original.trim_end_matches('0');
            let zeroes = s_original.len() - s_trimmed.len();
            assert_eq!(
                decimal_trailing_zeroes(x),
                u8::try_from(zeroes).unwrap(),
                "Failed for {x}",
            );
        }
    }
}
