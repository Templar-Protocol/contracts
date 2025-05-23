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
    exponent: i32,
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

    let Some(exponent) = pyth_price.expo.checked_sub(decimals) else {
        return Err(error::PriceDataError::ExponentOutOfBounds);
    };

    Ok(Price {
        _asset: PhantomData,
        price: u128::from(price),
        confidence: u128::from(pyth_price.conf.0),
        exponent,
    })
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

#[derive(Debug, Clone, Copy)]
pub struct Valuation {
    coefficient: primitive_types::U256,
    exponent: i32,
}

impl Valuation {
    pub fn optimistic<T: AssetClass>(amount: FungibleAssetAmount<T>, price: &Price<T>) -> Self {
        Self {
            coefficient: U256::from(u128::from(amount))
                * U256::from(price.price + price.confidence), // guaranteed not to overflow
            exponent: price.exponent,
        }
    }

    pub fn pessimistic<T: AssetClass>(amount: FungibleAssetAmount<T>, price: &Price<T>) -> Self {
        Self {
            coefficient: U256::from(u128::from(amount))
                * U256::from(price.price - price.confidence), // guaranteed not to overflow
            exponent: price.exponent,
        }
    }

    /// Returns the ratio between this and another `Valuation`.
    /// When the two `Valuation`s are within a few orders of magnitude of each
    /// other, the ratio will be as accurate as `Decimal` can represent.
    /// Otherwise, it will return a power of two close to the correct ratio.
    /// If the ratio is outside the representable range of `Decimal`, it will
    /// return `Decimal::MAX` if the ratio is too large, and `Decimal::MIN`
    /// (zero) if the ratio is too small.
    #[allow(clippy::cast_possible_wrap)]
    pub fn ratio(self, rhs: Self) -> Option<Decimal> {
        if rhs.coefficient.is_zero() {
            // div0
            return None;
        }

        if let Some(combined_exponents) = self
            .exponent
            .checked_sub(rhs.exponent)
            .and_then(|pow| Decimal::from(self.coefficient).mul_pow10(pow))
        {
            return Some(combined_exponents / Decimal::from(rhs.coefficient));
        }

        // Exact value calculation failed. This can happen when the difference
        // in exponents is extremely large, or when `self.coefficient` is
        // extremely small or extremely large.
        //
        // Approximate by logarithm instead.
        //
        // 345_060_773 / 103_873_643 (=3.321928094887362) is a close approximation of log2(10) (=3.32192809488736234...)
        let self_log2 =
            i64::from(self.exponent) * 345_060_773 / 103_873_643 + self.coefficient.bits() as i64;
        let rhs_log2 =
            i64::from(rhs.exponent) * 345_060_773 / 103_873_643 + rhs.coefficient.bits() as i64;

        let result_log2 = self_log2 - rhs_log2;

        Some(if result_log2 >= 0 {
            u32::try_from(result_log2)
                .ok()
                .and_then(Decimal::pow2_int)
                .unwrap_or(Decimal::MAX)
        } else {
            result_log2
                .checked_neg()
                .and_then(|n| u32::try_from(n).ok())
                .and_then(Decimal::pow2_int)
                .map_or(Decimal::MIN, |r| Decimal::ONE / r)
        })
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use crate::dec;

    use super::*;

    #[test]
    fn valuation_eq() {
        let o = Valuation::optimistic(
            1000u128.into(),
            &Price::<BorrowAsset> {
                _asset: PhantomData,
                price: 250,
                confidence: 12,
                exponent: -5,
            },
        );

        assert_eq!(o.coefficient, U256::from(1000 * (250 + 12)));
        assert_eq!(o.exponent, -5);

        let p = Valuation::pessimistic(
            1000u128.into(),
            &Price::<BorrowAsset> {
                _asset: PhantomData,
                price: 250,
                confidence: 12,
                exponent: -5,
            },
        );

        assert_eq!(p.coefficient, U256::from(1000 * (250 - 12)));
        assert_eq!(p.exponent, -5);
    }

    #[test]
    fn valuation_ratio_equal() {
        let first = Valuation::optimistic(
            600u128.into(),
            &Price::<BorrowAsset> {
                _asset: PhantomData,
                price: 100,
                confidence: 0,
                exponent: 4,
            },
        );
        let second = Valuation::pessimistic(
            60u128.into(),
            &Price::<BorrowAsset> {
                _asset: PhantomData,
                price: 1000,
                confidence: 0,
                exponent: 4,
            },
        );

        assert_eq!(first.ratio(second).unwrap(), Decimal::ONE);
    }

    #[rstest]
    #[case(8, 1, 8, 0,      dec!("1"))]
    #[case(1, 25, 1, -2,    dec!("4"))]
    #[case(0, 1, 1, 0,      dec!("0"))]
    #[case(800, 2, 4, 2,    dec!("1"))]
    #[case(u128::MAX, 1, 1, i32::MIN, Decimal::MAX)]
    #[case(1, 1, 1, i32::MAX, Decimal::MIN)]
    // The following case returns a power of 2. Whereas the *correct* answer is
    // 1e+115, the approximation 2^382 is about 9.85e+114. Keep in mind Decimal
    // only supports a total of 115 whole decimal digits.
    #[case(u128::MAX, u128::MAX, 1, -115, Decimal::pow2_int(382).unwrap())]
    #[case(1, 1, 1, 39, Decimal::ZERO)]
    #[test]
    fn valuation_ratios(
        #[case] value: u128,
        #[case] divisor_value: u128,
        #[case] divisor_price: u128,
        #[case] divisor_exponent: i32,
        #[case] expected_result: impl Into<Decimal>,
    ) {
        let dividend = Valuation::optimistic(
            value.into(),
            &Price::<BorrowAsset> {
                _asset: PhantomData,
                price: 1,
                confidence: 0,
                exponent: 0,
            },
        );

        let divisor = Valuation::optimistic(
            divisor_value.into(),
            &Price::<BorrowAsset> {
                _asset: PhantomData,
                price: divisor_price,
                confidence: 0,
                exponent: divisor_exponent,
            },
        );

        println!("{dividend:?}");
        println!("{divisor:?}");

        assert_eq!(dividend.ratio(divisor).unwrap(), expected_result.into());
    }
}
