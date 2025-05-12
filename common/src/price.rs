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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Valuation {
    coefficient: primitive_types::U256,
    exponent: i32,
}

impl Valuation {
    fn normalize(&mut self) {
        let add_pow_10 = decimal_trailing_zeros(self.coefficient);
        self.coefficient /= 10u128.pow(u32::from(add_pow_10));
        self.exponent += i32::from(add_pow_10);
    }

    pub fn optimistic<T: AssetClass>(amount: FungibleAssetAmount<T>, price: &Price<T>) -> Self {
        let mut self_ = Self {
            coefficient: U256::from(u128::from(amount))
                * U256::from(price.price + price.confidence), // guaranteed not to overflow
            exponent: price.exponent,
        };
        self_.normalize();
        self_
    }

    pub fn pessimistic<T: AssetClass>(amount: FungibleAssetAmount<T>, price: &Price<T>) -> Self {
        let mut self_ = Self {
            coefficient: U256::from(u128::from(amount))
                * U256::from(price.price - price.confidence), // guaranteed not to overflow
            exponent: price.exponent,
        };
        self_.normalize();
        self_
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

fn decimal_trailing_zeros(mut x: U256) -> u8 {
    let mut total = 0;

    while !x.is_zero() && x % 10 == U256::zero() {
        x /= 10;
        total += 1;
    }

    total
}

#[cfg(test)]
mod tests {
    use rand::Rng;
    use rstest::rstest;

    use crate::dec;

    use super::*;

    #[test]
    fn trailing_zeroes() {
        assert_eq!(decimal_trailing_zeros(0.into()), 0);
        assert_eq!(decimal_trailing_zeros(1.into()), 0);
        assert_eq!(decimal_trailing_zeros(10.into()), 1);
        assert_eq!(decimal_trailing_zeros(100.into()), 2);
        assert_eq!(decimal_trailing_zeros(34_873_400_000u128.into()), 5);
        assert_eq!(decimal_trailing_zeros(348_734_000_001u128.into()), 0);
        assert_eq!(decimal_trailing_zeros(7_568_265_868u128.into()), 0);
        assert_eq!(decimal_trailing_zeros(3_487_340_000_010_000u128.into()), 4);
        assert_eq!(decimal_trailing_zeros(u128::MAX.into()), 0);

        let mut rng = rand::thread_rng();

        for _ in 0..100 {
            let x: u128 = rng.gen();
            let s_original = x.to_string();
            let s_trimmed = s_original.trim_end_matches('0');
            let zeroes = s_original.len() - s_trimmed.len();
            assert_eq!(
                decimal_trailing_zeros(x.into()),
                u8::try_from(zeroes).unwrap(),
                "Failed for {x}",
            );
        }
    }

    #[test]
    fn valuation_eq() {
        let first = Valuation::optimistic(
            1000u128.into(),
            &Price::<BorrowAsset> {
                _asset: PhantomData,
                price: 250,
                confidence: 12,
                exponent: -5,
            },
        );

        assert_eq!(
            first,
            Valuation::pessimistic(
                1u128.into(),
                &Price::<BorrowAsset> {
                    _asset: PhantomData,
                    price: 265,
                    confidence: 3,
                    exponent: -2,
                },
            ),
        );
        assert_ne!(
            first,
            Valuation::optimistic(
                10u128.into(),
                &Price::<BorrowAsset> {
                    _asset: PhantomData,
                    price: 262,
                    confidence: 0,
                    exponent: -2,
                },
            ),
        );
        assert_ne!(
            first,
            Valuation::optimistic(
                1u128.into(),
                &Price::<BorrowAsset> {
                    _asset: PhantomData,
                    price: 263,
                    confidence: 0,
                    exponent: -2,
                },
            ),
        );
        assert_ne!(
            first,
            Valuation::optimistic(
                1u128.into(),
                &Price::<BorrowAsset> {
                    _asset: PhantomData,
                    price: 262,
                    confidence: 1,
                    exponent: -2,
                },
            ),
        );
        assert_ne!(
            first,
            Valuation::optimistic(
                1u128.into(),
                &Price::<BorrowAsset> {
                    _asset: PhantomData,
                    price: 262,
                    confidence: 0,
                    exponent: -3,
                },
            ),
        );
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
