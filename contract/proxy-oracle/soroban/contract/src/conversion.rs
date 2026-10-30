//! Conversions between the Soroban surface types and the kernel's primitives.
//!
//! Sources speak SEP-40 `PriceData { i128, fixed-decimals }`; the kernel
//! speaks `Price { i64, expo }`; the main contract's cache + history speak
//! `NormalizedPrice { i64, expo, timestamp }`. Adapters scale `NormalizedPrice`
//! back to SEP-40 with their own per-adapter decimals.

use templar_primitives::Nanoseconds;
use templar_proxy_oracle_kernel::Price;
use templar_proxy_oracle_soroban_common::{ContractError, NormalizedPrice, PriceData};

/// Convert a source feed's `PriceData` (decimal-prefixed i128) into the
/// kernel's `Price { i64, expo }` representation, downscaling if `value`
/// doesn't fit in i64.
pub fn source_price_to_kernel(
    source_price: PriceData,
    source_decimals: u32,
) -> Result<Price, ContractError> {
    if source_decimals > 18 {
        return Err(ContractError::InvalidInput);
    }
    let mut value = source_price.price;
    let mut expo = i32::try_from(source_decimals)
        .map_err(|_| ContractError::ConversionOverflow)?
        .checked_neg()
        .ok_or(ContractError::ConversionOverflow)?;
    while value > i128::from(i64::MAX) || value < i128::from(i64::MIN) {
        value /= 10;
        expo = expo
            .checked_add(1)
            .ok_or(ContractError::ConversionOverflow)?;
    }
    Ok(Price {
        price: i64::try_from(value).map_err(|_| ContractError::ConversionOverflow)?,
        conf: 0,
        expo,
        publish_time_ns: Nanoseconds::from_secs(source_price.timestamp),
    })
}

pub fn kernel_price_to_normalized(price: Price) -> NormalizedPrice {
    NormalizedPrice {
        mantissa: price.price,
        expo: price.expo,
        timestamp: price.publish_time_ns.as_secs(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_decimals_are_bounded_at_sep40_maximum() {
        let price = PriceData {
            price: 1,
            timestamp: 100,
        };
        assert!(source_price_to_kernel(price.clone(), 18).is_ok());
        assert_eq!(
            source_price_to_kernel(price, 19),
            Err(ContractError::InvalidInput)
        );
    }
}
