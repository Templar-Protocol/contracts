use std::marker::PhantomData;

use primitive_types::U256;

use crate::{
    asset::{AssetClass, BorrowAsset, CollateralAsset, FungibleAssetAmount},
    number::Decimal,
    oracle::pyth,
};

#[derive(Clone, Debug)]
pub struct Price<T: AssetClass> {
    _asset: PhantomData<T>,
    price: u128,
    confidence: u128,
    power_of_10: i32,
}

pub mod error {
    use thiserror::Error;

    #[derive(Clone, Debug, Error)]
    #[error("Bad price data: {0}")]
    pub enum PriceDataError {
        #[error("Reported negative price")]
        NegativePrice,
        #[error("Confidence interval too large")]
        ConfidenceIntervalTooLarge,
        #[error("Exponent out of bounds")]
        ExponentOutOfBounds,
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

    let Some(power_of_10) = pyth_price.expo.checked_sub(decimals) else {
        return Err(error::PriceDataError::ExponentOutOfBounds);
    };

    Ok(Price {
        _asset: PhantomData,
        price: u128::from(price),
        confidence: u128::from(pyth_price.conf.0),
        power_of_10,
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

#[derive(Clone, Debug)]
pub struct PricePair {
    pub collateral: Price<CollateralAsset>,
    pub borrow: Price<BorrowAsset>,
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
            collateral: from_pyth_price(collateral_price, collateral_decimals)?,
            borrow: from_pyth_price(borrow_price, borrow_decimals)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Valuation {
    coefficient: primitive_types::U256,
    power_of_10: i32,
}

impl Valuation {
    fn normalize(&mut self) {
        let add_pow_10 = decimal_trailing_zeroes(self.coefficient);
        self.coefficient /= 10u128.pow(u32::from(add_pow_10));
        self.power_of_10 += i32::from(add_pow_10);
    }

    pub fn optimistic<T: AssetClass>(amount: FungibleAssetAmount<T>, price: &Price<T>) -> Self {
        let mut self_ = Self {
            coefficient: U256::from(u128::from(amount))
                * U256::from(price.price + price.confidence), // guaranteed not to overflow
            power_of_10: price.power_of_10,
        };
        self_.normalize();
        self_
    }

    pub fn pessimistic<T: AssetClass>(amount: FungibleAssetAmount<T>, price: &Price<T>) -> Self {
        let mut self_ = Self {
            coefficient: U256::from(u128::from(amount))
                * U256::from(price.price - price.confidence), // guaranteed not to overflow
            power_of_10: price.power_of_10,
        };
        self_.normalize();
        self_
    }

    pub fn ratio(self, rhs: Self) -> Option<Decimal> {
        if rhs.coefficient.is_zero() {
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

impl From<Valuation> for Decimal {
    fn from(value: Valuation) -> Self {
        Decimal::from(value.coefficient).times_10_to_the(value.power_of_10)
    }
}

fn decimal_trailing_zeroes(mut x: U256) -> u8 {
    let mut zeroes = 0;

    while !x.is_zero() && x % 10 == U256::zero() {
        x /= 10;
        zeroes += 1;
    }

    zeroes
}

#[cfg(test)]
mod tests {
    use rand::Rng;

    use super::*;

    #[test]
    fn trailing_zeroes() {
        assert_eq!(decimal_trailing_zeroes(0.into()), 0);
        assert_eq!(decimal_trailing_zeroes(1.into()), 0);
        assert_eq!(decimal_trailing_zeroes(10.into()), 1);
        assert_eq!(decimal_trailing_zeroes(100.into()), 2);
        assert_eq!(decimal_trailing_zeroes(34_873_400_000u128.into()), 5);
        assert_eq!(decimal_trailing_zeroes(348_734_000_001u128.into()), 0);
        assert_eq!(decimal_trailing_zeroes(7_568_265_868u128.into()), 0);
        assert_eq!(decimal_trailing_zeroes(3_487_340_000_010_000u128.into()), 4);
        assert_eq!(decimal_trailing_zeroes(u128::MAX.into()), 0);

        let mut rng = rand::thread_rng();

        for _ in 0..100 {
            let x: u128 = rng.gen();
            let s_original = x.to_string();
            let s_trimmed = s_original.trim_end_matches('0');
            let zeroes = s_original.len() - s_trimmed.len();
            assert_eq!(
                decimal_trailing_zeroes(x.into()),
                u8::try_from(zeroes).unwrap(),
                "Failed for {x}",
            );
        }
    }

    #[test]
    fn valuations() {
        let first = Valuation::optimistic(
            600u128.into(),
            &Price::<BorrowAsset> {
                _asset: PhantomData,
                price: 100,
                confidence: 0,
                power_of_10: 4,
            },
        );
        let second = Valuation::pessimistic(
            60u128.into(),
            &Price::<BorrowAsset> {
                _asset: PhantomData,
                price: 1000,
                confidence: 0,
                power_of_10: 4,
            },
        );

        assert_eq!(first.ratio(second).unwrap(), Decimal::ONE);
    }
}
