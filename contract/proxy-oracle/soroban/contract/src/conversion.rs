//! Conversions between the Soroban surface types and the kernel's primitives.
//!
//! Sources speak SEP-40 `PriceData { i128, fixed-decimals }`; the kernel
//! speaks `Price { i64, expo }`; the main contract's cache + history speak
//! `NormalizedPrice { i64, expo, timestamp }`. Adapters scale `NormalizedPrice`
//! back to SEP-40 with their own per-adapter decimals.

use soroban_sdk::Vec;
use templar_primitives::{Decimal, Nanoseconds};
use templar_proxy_oracle_kernel::{
    proxy::{
        aggregator::{method::median::MedianLow, Aggregator},
        circuit_breaker::{
            AcceptedHistorySource, CircuitBreaker, MonotonicRun, StepwiseChange,
            WindowedChangeDelta,
        },
        FreshnessFilter, Proxy, WeightedSource,
    },
    Price,
};
use templar_proxy_oracle_soroban_common::{
    CircuitBreakerConfig, ContractError, MonotonicRunConfig as SorobanMonotonicRunConfig,
    NormalizedPrice, PriceData, ProxyConfig, StepwiseChangeConfig as SorobanStepwiseChangeConfig,
    WindowedChangeDeltaConfig as SorobanWindowedChangeDeltaConfig,
};

use crate::MAX_SOURCES_PER_PROXY;

/// Convert a source feed's `PriceData` (decimal-prefixed i128) into the
/// kernel's `Price { i64, expo }` representation, downscaling if `value`
/// doesn't fit in i64.
pub fn source_price_to_kernel(
    source_price: PriceData,
    source_decimals: u32,
) -> Result<Price, ContractError> {
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

pub fn decimal_from_repr(repr: Vec<u64>) -> Result<Decimal, ContractError> {
    if repr.len() != 8 {
        return Err(ContractError::InvalidInput);
    }
    let mut raw = [0_u64; 8];
    for (index, value) in repr.iter().enumerate() {
        raw[index] = value;
    }
    Ok(Decimal::from_repr(raw))
}

pub fn accepted_history_source(value: u32) -> Result<AcceptedHistorySource, ContractError> {
    match value {
        0 => Ok(AcceptedHistorySource::Empty),
        1 => Ok(AcceptedHistorySource::Observed),
        _ => Err(ContractError::InvalidInput),
    }
}

pub fn circuit_breaker_from_config(
    config: CircuitBreakerConfig,
) -> Result<CircuitBreaker, ContractError> {
    match config {
        CircuitBreakerConfig::StepwiseChange(SorobanStepwiseChangeConfig {
            max_relative_change_repr,
        }) => {
            let max_relative_change = decimal_from_repr(max_relative_change_repr)?;
            if max_relative_change.is_zero() {
                return Err(ContractError::InvalidInput);
            }
            Ok(CircuitBreaker::StepwiseChange(StepwiseChange {
                max_relative_change,
            }))
        }
        CircuitBreakerConfig::MonotonicRun(SorobanMonotonicRunConfig {
            max_streak,
            min_relative_step_change_repr,
        }) => {
            if max_streak == 0 {
                return Err(ContractError::InvalidInput);
            }
            let min_relative_step_change = decimal_from_repr(min_relative_step_change_repr)?;
            if min_relative_step_change.is_zero() {
                return Err(ContractError::InvalidInput);
            }
            Ok(CircuitBreaker::MonotonicRun(MonotonicRun {
                max_streak,
                min_relative_step_change,
            }))
        }
        CircuitBreakerConfig::WindowedChangeDelta(SorobanWindowedChangeDeltaConfig {
            window_len,
            lookback_windows,
            max_relative_change_delta_repr,
        }) => {
            if window_len < 2 {
                return Err(ContractError::InvalidInput);
            }
            if lookback_windows == 0 {
                return Err(ContractError::InvalidInput);
            }
            let max_relative_change_delta = decimal_from_repr(max_relative_change_delta_repr)?;
            if max_relative_change_delta.is_zero() {
                return Err(ContractError::InvalidInput);
            }
            Ok(CircuitBreaker::WindowedChangeDelta(WindowedChangeDelta {
                window_len,
                lookback_windows,
                max_relative_change_delta,
            }))
        }
    }
}

pub fn kernel_proxy_from_config(config: &ProxyConfig) -> Proxy<u32> {
    let mut median =
        MedianLow::new((0..config.sources.len()).map(|index| WeightedSource::new(index, 1)));
    median.min_sources = config.min_sources;
    Proxy::new(
        Aggregator::MedianLow(median),
        FreshnessFilter::new(
            config.max_age_secs.map(Nanoseconds::from_secs),
            config.max_clock_drift_secs.map(Nanoseconds::from_secs),
        ),
    )
}

pub fn validate_proxy_config(config: &ProxyConfig) -> Result<(), ContractError> {
    if config.sources.is_empty() || config.sources.len() > MAX_SOURCES_PER_PROXY {
        return Err(ContractError::TooManySources);
    }
    if config.min_sources == 0 || config.min_sources > config.sources.len() {
        return Err(ContractError::InvalidInput);
    }
    let sources_len = config.sources.len();
    for i in 0..sources_len {
        let src_i = config.sources.get(i).ok_or(ContractError::InvalidInput)?;
        for j in (i + 1)..sources_len {
            let src_j = config.sources.get(j).ok_or(ContractError::InvalidInput)?;
            if src_i.oracle == src_j.oracle && src_i.asset == src_j.asset {
                return Err(ContractError::InvalidInput);
            }
        }
    }
    Ok(())
}
