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
mod tests;
