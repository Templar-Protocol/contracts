#![no_std]

use soroban_sdk::{contractclient, contracterror, contracttype, Address, BytesN, Env, Symbol, Vec};
use templar_primitives::{Decimal, Nanoseconds};
use templar_proxy_oracle_kernel::{
    proxy::{
        aggregator::Aggregator,
        circuit_breaker::{
            CircuitBreaker, CircuitBreakerRule, CircuitBreakerSetConfig, CumulativeChange,
            MonotonicRun, StepwiseChange, WindowedChangeDelta,
        },
        FreshnessFilter, Proxy,
    },
    Price,
};

pub const DEFAULT_TTL_THRESHOLD: u32 = 518_400;
pub const DEFAULT_TTL_EXTEND_TO: u32 = 3_110_400;
pub const MAX_MANUAL_TRIP_METADATA_LEN: usize = 1024;

pub fn extend_instance_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
}

/// Returns true when `wasm_hash` is all zero bytes.
///
/// Implemented without `Env` so it can be used anywhere `BytesN<32>` is
/// available, including in pure validation helpers.
#[must_use]
pub fn is_zero_wasm_hash(wasm_hash: &BytesN<32>) -> bool {
    wasm_hash.to_array() == [0_u8; 32]
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct PriceData {
    pub price: i128,
    pub timestamp: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub enum Asset {
    Stellar(Address),
    Other(Symbol),
}

/// Decimals-free price representation used internally and across the
/// proxy-oracle ↔ SEP-40 adapter boundary. Adapters convert this to
/// SEP-40 `PriceData` using their own configured decimals.
#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct NormalizedPrice {
    pub mantissa: i64,
    pub expo: i32,
    pub timestamp: u64,
}

/// SEP-40 `PriceFeed` trait. Implemented by external price sources the
/// main proxy oracle consumes, and by the `Sep40Adapter` contracts that
/// re-expose the proxy oracle's normalized prices in SEP-40 form.
#[contractclient(name = "PriceFeedClient")]
pub trait PriceFeedTrait {
    fn base(env: Env) -> Asset;
    fn assets(env: Env) -> Vec<Asset>;
    fn decimals(env: Env) -> u32;
    fn resolution(env: Env) -> u32;
    fn price(env: Env, asset: Asset, timestamp: u64) -> Option<PriceData>;
    fn prices(env: Env, asset: Asset, records: u32) -> Option<Vec<PriceData>>;
    fn lastprice(env: Env, asset: Asset) -> Option<PriceData>;
}

/// Read API exposed by the proxy oracle runtime and consumed by `Sep40Adapter`
/// contracts. Returns kernel-form prices (`NormalizedPrice { mantissa, expo,
/// timestamp }`) post-aggregation; adapters re-scale to their own SEP-40
/// fixed decimals.
///
/// The proxy oracle owns the freshness check (`max_age_secs` from
/// `ProxyConfig`); `aggregated_latest` already applies it before returning.
#[contractclient(name = "ProxyOracleClient")]
pub trait ProxyOracleTrait {
    /// Latest aggregated price for `asset`, post-freshness-check. Returns
    /// `None` if no proxy is registered, the cache is empty / not accepted,
    /// or the cached entry is older than the configured `max_age_secs`.
    fn aggregated_latest(env: Env, asset: Asset) -> Option<NormalizedPrice>;
    /// Last `records` aggregated prices for `asset`, oldest first. Does not
    /// apply a freshness filter; callers that care about staleness should
    /// inspect the returned timestamps.
    fn aggregated_history(env: Env, asset: Asset, records: u32) -> Option<Vec<NormalizedPrice>>;
    fn source_base(env: Env) -> Option<Asset>;
}

/// Convert a normalized exponent-form price to SEP-40 `PriceData` with the
/// given target decimals. Used by `Sep40Adapter` contracts.
pub fn normalized_to_sep40(
    price: &NormalizedPrice,
    decimals: u32,
) -> Result<PriceData, ContractError> {
    let decimals = i32::try_from(decimals).map_err(|_| ContractError::ConversionOverflow)?;
    let scale = decimals
        .checked_add(price.expo)
        .ok_or(ContractError::ConversionOverflow)?;
    let mut value = i128::from(price.mantissa);
    if scale >= 0 {
        value = value
            .checked_mul(
                10_i128
                    .checked_pow(scale.unsigned_abs())
                    .ok_or(ContractError::ConversionOverflow)?,
            )
            .ok_or(ContractError::ConversionOverflow)?;
    } else {
        let divisor = 10_i128
            .checked_pow(scale.unsigned_abs())
            .ok_or(ContractError::ConversionOverflow)?;
        let scaled = value / divisor;
        if value != 0 && scaled == 0 {
            return Err(ContractError::ConversionOverflow);
        }
        value = scaled;
    }
    Ok(PriceData {
        price: value,
        timestamp: price.timestamp,
    })
}

#[contracterror]
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ContractError {
    AlreadyInitialized = 1,
    MissingConfig = 2,
    Unauthorized = 3,
    InvalidInput = 4,
    StorageError = 5,
    SourceUnavailable = 6,
    ResolveFailed = 7,
    ConversionOverflow = 8,
    TooManySources = 9,
    TooManyBreakers = 10,
    BreakerError = 11,
    TooManyAssets = 12,
    TooFewSources = 13,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct SourceConfig {
    pub oracle: Address,
    pub asset: Asset,
    pub expected_decimals: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct MedianConfig {
    pub sources: Vec<SourceConfig>,
    pub min_sources: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub enum AggregatorConfig {
    MedianLow(MedianConfig),
    MedianHigh(MedianConfig),
    Priority(Vec<SourceConfig>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct ProxyConfig {
    pub aggregator: AggregatorConfig,
    pub max_age_secs: u64,
    pub max_clock_drift_secs: u64,
}

pub const MAX_SOURCES_PER_PROXY: u32 = 16;
pub const MAX_SOURCE_AGE_SECS: u64 = 7 * 24 * 60 * 60;
pub const MAX_CLOCK_DRIFT_SECS: u64 = 60 * 60;
pub const MAX_HISTORY_RECORDS: u32 = 32;

impl ProxyConfig {
    /// Convert admitted storage without repeating pairwise ingress validation.
    #[must_use]
    pub fn to_kernel(&self) -> Proxy<SourceConfig> {
        let aggregator = match &self.aggregator {
            AggregatorConfig::MedianLow(config) => {
                let mut aggregator = Aggregator::median_low(config.sources.iter());
                if let Aggregator::MedianLow(median) = &mut aggregator {
                    median.min_sources = config.min_sources;
                }
                aggregator
            }
            AggregatorConfig::MedianHigh(config) => {
                let mut aggregator = Aggregator::median_high(config.sources.iter());
                if let Aggregator::MedianHigh(median) = &mut aggregator {
                    median.min_sources = config.min_sources;
                }
                aggregator
            }
            AggregatorConfig::Priority(sources) => Aggregator::priority(sources.iter()),
        };
        Proxy::new(
            aggregator,
            FreshnessFilter::new(
                Some(Nanoseconds::from_secs(self.max_age_secs)),
                Some(Nanoseconds::from_secs(self.max_clock_drift_secs)),
            ),
        )
    }
}

pub fn parse_proxy_config(config: &ProxyConfig) -> Result<Proxy<SourceConfig>, ContractError> {
    let source_count = match &config.aggregator {
        AggregatorConfig::MedianLow(median) | AggregatorConfig::MedianHigh(median) => {
            median.sources.len()
        }
        AggregatorConfig::Priority(sources) => sources.len(),
    };
    if source_count == 0 {
        return Err(ContractError::TooFewSources);
    }
    if source_count > MAX_SOURCES_PER_PROXY {
        return Err(ContractError::TooManySources);
    }
    if config.max_age_secs > MAX_SOURCE_AGE_SECS
        || config.max_clock_drift_secs > MAX_CLOCK_DRIFT_SECS
    {
        return Err(ContractError::InvalidInput);
    }
    let proxy = config.to_kernel();
    proxy
        .aggregator
        .validate()
        .map_err(|_| ContractError::InvalidInput)?;
    for (index, source) in proxy.sources().enumerate() {
        if source.expected_decimals > 18
            || proxy
                .sources()
                .take(index)
                .any(|prior| prior.oracle == source.oracle)
        {
            return Err(ContractError::InvalidInput);
        }
    }
    Ok(proxy)
}

pub fn parse_breaker_set_config(
    sample_interval_secs: u64,
    history_len: u32,
) -> Result<CircuitBreakerSetConfig, ContractError> {
    let sample_interval_ns =
        Nanoseconds::checked_from_secs(sample_interval_secs).ok_or(ContractError::InvalidInput)?;
    if history_len == 0 || history_len > MAX_HISTORY_RECORDS {
        return Err(ContractError::InvalidInput);
    }
    Ok(CircuitBreakerSetConfig {
        sample_interval_ns,
        history_len,
    })
}

/// Soroban wire format for a 512-bit `Decimal`. Wrapping `BytesN<64>` (the
/// 8 × `u64` words of `Decimal::as_repr` laid out little-endian) gives a
/// compile-time length guarantee at the contract boundary — unlike a raw
/// `Vec<u64>` field, which could arrive empty, short, or oversized and
/// would have to be re-validated by every consumer.
#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct SorobanDecimal(pub BytesN<64>);

impl SorobanDecimal {
    // `From<Decimal> for SorobanDecimal` would be the natural counterpart to
    // the `From<&SorobanDecimal> for Decimal` impl below, but construction
    // needs the `Env` (to allocate the `BytesN<64>` in host memory) and the
    // `From` trait doesn't admit a context parameter — so this stays as a
    // method.
    #[must_use]
    pub fn from_decimal(env: &Env, decimal: Decimal) -> Self {
        // SAFETY: `[u64; 8]` and `[u8; 64]` are POD types of equal size with
        // no invalid bit patterns. `u64::to_le` forces little-endian byte
        // layout regardless of target (no-op on LE, byte-swap on BE), so
        // the wire format is platform-independent.
        let words = decimal.as_repr().map(u64::to_le);
        let bytes: [u8; 64] = unsafe { core::mem::transmute(words) };
        Self(BytesN::from_array(env, &bytes))
    }

    /// Convenience for sites where `.into()` would force the caller to
    /// annotate the destination type (e.g. `let d = sd.to_decimal();` reads
    /// cleaner than `let d: Decimal = sd.into();`). Equivalent to
    /// `Decimal::from(&self)`.
    #[must_use]
    pub fn to_decimal(&self) -> Decimal {
        Decimal::from(self)
    }
}

impl From<&SorobanDecimal> for Decimal {
    fn from(value: &SorobanDecimal) -> Self {
        // SAFETY: see `SorobanDecimal::from_decimal`.
        let raw: [u64; 8] = unsafe { core::mem::transmute(value.0.to_array()) };
        Self::from_repr(raw.map(u64::from_le))
    }
}

impl From<SorobanDecimal> for Decimal {
    fn from(value: SorobanDecimal) -> Self {
        Self::from(&value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct StepwiseChangeConfig {
    pub max_relative_change: SorobanDecimal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct MonotonicRunConfig {
    pub max_streak: u32,
    pub min_relative_step_change: SorobanDecimal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct WindowedChangeDeltaConfig {
    pub window_len: u32,
    pub lookback_windows: u32,
    pub max_relative_mean_change: SorobanDecimal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct CumulativeChangeConfig {
    pub max_relative_change: SorobanDecimal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub enum CircuitBreakerConfig {
    StepwiseChange(StepwiseChangeConfig),
    MonotonicRun(MonotonicRunConfig),
    WindowedChangeDelta(WindowedChangeDeltaConfig),
    CumulativeChange(CumulativeChangeConfig),
}

impl CircuitBreakerConfig {
    pub fn to_kernel(&self, baseline: Option<Price>) -> Result<CircuitBreaker, ContractError> {
        let breaker = match self {
            Self::StepwiseChange(config) => CircuitBreaker::StepwiseChange(StepwiseChange {
                max_relative_change: config.max_relative_change.to_decimal(),
            }),
            Self::MonotonicRun(config) => CircuitBreaker::MonotonicRun(MonotonicRun {
                max_streak: config.max_streak,
                min_relative_step_change: config.min_relative_step_change.to_decimal(),
            }),
            Self::WindowedChangeDelta(config) => {
                CircuitBreaker::WindowedChangeDelta(WindowedChangeDelta {
                    window_len: config.window_len,
                    lookback_windows: config.lookback_windows,
                    max_relative_mean_change: config.max_relative_mean_change.to_decimal(),
                })
            }
            Self::CumulativeChange(config) => CircuitBreaker::CumulativeChange(CumulativeChange {
                baseline: baseline.ok_or(ContractError::InvalidInput)?,
                max_relative_change: config.max_relative_change.to_decimal(),
            }),
        };
        if !breaker.is_valid_for(Nanoseconds::zero(), MAX_HISTORY_RECORDS) {
            return Err(ContractError::InvalidInput);
        }
        Ok(breaker)
    }

    #[must_use]
    pub fn from_kernel(env: &Env, breaker: &CircuitBreaker) -> Self {
        match breaker {
            CircuitBreaker::StepwiseChange(rule) => Self::StepwiseChange(StepwiseChangeConfig {
                max_relative_change: SorobanDecimal::from_decimal(env, rule.max_relative_change),
            }),
            CircuitBreaker::MonotonicRun(rule) => Self::MonotonicRun(MonotonicRunConfig {
                max_streak: rule.max_streak,
                min_relative_step_change: SorobanDecimal::from_decimal(
                    env,
                    rule.min_relative_step_change,
                ),
            }),
            CircuitBreaker::WindowedChangeDelta(rule) => {
                Self::WindowedChangeDelta(WindowedChangeDeltaConfig {
                    window_len: rule.window_len,
                    lookback_windows: rule.lookback_windows,
                    max_relative_mean_change: SorobanDecimal::from_decimal(
                        env,
                        rule.max_relative_mean_change,
                    ),
                })
            }
            CircuitBreaker::CumulativeChange(rule) => {
                Self::CumulativeChange(CumulativeChangeConfig {
                    max_relative_change: SorobanDecimal::from_decimal(
                        env,
                        rule.max_relative_change,
                    ),
                })
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct SetEnforcedConfig {
    pub is_enforced: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct RearmConfig {
    pub arming_delay_secs: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::Env;

    fn sources(env: &Env, count: u32) -> Vec<SourceConfig> {
        let mut sources = Vec::new(env);
        for _ in 0..count {
            sources.push_back(SourceConfig {
                oracle: Address::generate(env),
                asset: Asset::Other(Symbol::new(env, "BTC")),
                expected_decimals: 8,
            });
        }
        sources
    }

    fn proxy_config(aggregator: AggregatorConfig) -> ProxyConfig {
        ProxyConfig {
            aggregator,
            max_age_secs: MAX_SOURCE_AGE_SECS,
            max_clock_drift_secs: MAX_CLOCK_DRIFT_SECS,
        }
    }

    fn aggregators(sources: &Vec<SourceConfig>, min_sources: u32) -> [AggregatorConfig; 3] {
        [
            AggregatorConfig::MedianLow(MedianConfig {
                sources: sources.clone(),
                min_sources,
            }),
            AggregatorConfig::MedianHigh(MedianConfig {
                sources: sources.clone(),
                min_sources,
            }),
            AggregatorConfig::Priority(sources.clone()),
        ]
    }

    #[test]
    fn parser_preserves_method_order_pins_quorum_and_freshness() {
        let env = Env::default();
        let sources = sources(&env, 2);
        for (method, aggregator) in aggregators(&sources, 2).into_iter().enumerate() {
            let config = proxy_config(aggregator);
            let proxy = parse_proxy_config(&config).unwrap();
            assert_eq!(proxy, config.to_kernel());
            assert_eq!(proxy.sources().len(), sources.len() as usize);
            assert!(proxy.sources().zip(sources.iter()).all(|(a, b)| *a == b));
            assert_eq!(
                proxy.freshness_filter,
                FreshnessFilter::new(
                    Some(Nanoseconds::from_secs(MAX_SOURCE_AGE_SECS)),
                    Some(Nanoseconds::from_secs(MAX_CLOCK_DRIFT_SECS)),
                )
            );
            match (&proxy.aggregator, method) {
                (Aggregator::MedianLow(median), 0) => {
                    assert_eq!(median.min_sources, 2);
                    assert!(median.sources.iter().all(|source| source.weight == 1));
                }
                (Aggregator::MedianHigh(median), 1) => {
                    assert_eq!(median.min_sources, 2);
                    assert!(median.sources.iter().all(|source| source.weight == 1));
                }
                (Aggregator::Priority(_), 2) => {}
                _ => panic!("aggregation method changed"),
            }
        }
    }

    #[test]
    fn every_method_rejects_empty_oversized_and_duplicate_oracles() {
        let env = Env::default();
        for (count, error) in [
            (0, ContractError::TooFewSources),
            (MAX_SOURCES_PER_PROXY + 1, ContractError::TooManySources),
        ] {
            for aggregator in aggregators(&sources(&env, count), 1) {
                assert_eq!(parse_proxy_config(&proxy_config(aggregator)), Err(error));
            }
        }
        let mut duplicate = sources(&env, 1);
        duplicate.push_back(duplicate.get(0).unwrap());
        for aggregator in aggregators(&duplicate, 1) {
            assert_eq!(
                parse_proxy_config(&proxy_config(aggregator)),
                Err(ContractError::InvalidInput)
            );
        }
        let mut different_descriptor = duplicate.get(1).unwrap();
        different_descriptor.asset = Asset::Other(Symbol::new(&env, "ETH"));
        different_descriptor.expected_decimals = 9;
        duplicate.set(1, different_descriptor);
        for aggregator in aggregators(&duplicate, 1) {
            assert_eq!(
                parse_proxy_config(&proxy_config(aggregator)),
                Err(ContractError::InvalidInput)
            );
        }
    }

    #[test]
    fn parser_checks_quorum_pins_and_freshness_boundaries() {
        let env = Env::default();
        let mut sources = sources(&env, MAX_SOURCES_PER_PROXY);
        for quorum in [0, MAX_SOURCES_PER_PROXY + 1] {
            for aggregator in aggregators(&sources, quorum).into_iter().take(2) {
                assert_eq!(
                    parse_proxy_config(&proxy_config(aggregator)),
                    Err(ContractError::InvalidInput)
                );
            }
        }
        for decimals in [0, 18, 19] {
            let mut source = sources.get(0).unwrap();
            source.expected_decimals = decimals;
            sources.set(0, source);
            for aggregator in aggregators(&sources, MAX_SOURCES_PER_PROXY) {
                assert_eq!(
                    parse_proxy_config(&proxy_config(aggregator)).is_ok(),
                    decimals <= 18
                );
            }
        }
        let sources = self::sources(&env, 1);
        for aggregator in aggregators(&sources, 1) {
            let mut config = proxy_config(aggregator);
            assert!(parse_proxy_config(&config).is_ok());
            config.max_age_secs = MAX_SOURCE_AGE_SECS + 1;
            assert_eq!(
                parse_proxy_config(&config),
                Err(ContractError::InvalidInput)
            );
            config.max_age_secs = 0;
            config.max_clock_drift_secs = MAX_CLOCK_DRIFT_SECS + 1;
            assert_eq!(
                parse_proxy_config(&config),
                Err(ContractError::InvalidInput)
            );
            config.max_clock_drift_secs = 0;
            assert!(parse_proxy_config(&config).is_ok());
        }
    }

    fn breakers(env: &Env, threshold: Decimal) -> [CircuitBreakerConfig; 4] {
        let threshold = SorobanDecimal::from_decimal(env, threshold);
        [
            CircuitBreakerConfig::StepwiseChange(StepwiseChangeConfig {
                max_relative_change: threshold.clone(),
            }),
            CircuitBreakerConfig::MonotonicRun(MonotonicRunConfig {
                max_streak: MAX_HISTORY_RECORDS - 1,
                min_relative_step_change: threshold.clone(),
            }),
            CircuitBreakerConfig::WindowedChangeDelta(WindowedChangeDeltaConfig {
                window_len: 2,
                lookback_windows: MAX_HISTORY_RECORDS / 2 - 1,
                max_relative_mean_change: threshold.clone(),
            }),
            CircuitBreakerConfig::CumulativeChange(CumulativeChangeConfig {
                max_relative_change: threshold,
            }),
        ]
    }

    #[test]
    fn breaker_projection_round_trips_and_validates_kernel_thresholds() {
        let env = Env::default();
        let baseline = Price {
            price: 100,
            conf: 0,
            expo: 0,
            publish_time_ns: Nanoseconds::zero(),
        };
        for threshold in [Decimal::ZERO, Decimal::ONE_HALF, Decimal::ONE, Decimal::TWO] {
            for config in breakers(&env, threshold) {
                let parsed = config.to_kernel(Some(baseline));
                if threshold == Decimal::ZERO || threshold > Decimal::ONE {
                    assert_eq!(parsed, Err(ContractError::InvalidInput));
                } else {
                    let breaker = parsed.unwrap();
                    assert_eq!(CircuitBreakerConfig::from_kernel(&env, &breaker), config);
                    if let CircuitBreaker::CumulativeChange(rule) = breaker {
                        assert_eq!(rule.baseline, baseline);
                        assert_eq!(config.to_kernel(None), Err(ContractError::InvalidInput));
                        assert_eq!(
                            config.to_kernel(Some(Price {
                                price: 0,
                                ..baseline
                            })),
                            Err(ContractError::InvalidInput)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn breaker_parser_rejects_rules_impossible_for_any_supported_history() {
        let env = Env::default();
        let threshold = SorobanDecimal::from_decimal(&env, Decimal::ONE_HALF);
        for max_streak in [0, MAX_HISTORY_RECORDS] {
            assert_eq!(
                CircuitBreakerConfig::MonotonicRun(MonotonicRunConfig {
                    max_streak,
                    min_relative_step_change: threshold.clone(),
                })
                .to_kernel(None),
                Err(ContractError::InvalidInput)
            );
        }
        for (window_len, lookback_windows) in [(1, 1), (2, 0), (17, 1), (2, u32::MAX)] {
            assert_eq!(
                CircuitBreakerConfig::WindowedChangeDelta(WindowedChangeDeltaConfig {
                    window_len,
                    lookback_windows,
                    max_relative_mean_change: threshold.clone(),
                })
                .to_kernel(None),
                Err(ContractError::InvalidInput)
            );
        }
    }

    #[test]
    fn breaker_set_parser_checks_history_and_seconds_without_saturation() {
        let max_secs = u64::MAX / 1_000_000_000;
        for history_len in [1, MAX_HISTORY_RECORDS] {
            for seconds in [0, max_secs] {
                let config = parse_breaker_set_config(seconds, history_len).unwrap();
                assert_eq!(config.sample_interval_ns.as_secs(), seconds);
                assert_eq!(config.history_len, history_len);
            }
        }
        for history_len in [0, MAX_HISTORY_RECORDS + 1] {
            assert_eq!(
                parse_breaker_set_config(0, history_len),
                Err(ContractError::InvalidInput)
            );
        }
        assert_eq!(
            parse_breaker_set_config(max_secs + 1, 1),
            Err(ContractError::InvalidInput)
        );
    }

    /// Cover the unsafe `transmute` path in `SorobanDecimal` end-to-end:
    /// every value round-trips through `to_decimal()` and both `From` impls.
    #[rstest]
    #[case::zero(Decimal::ZERO)]
    #[case::one_half(Decimal::ONE_HALF)]
    #[case::one(Decimal::ONE)]
    #[case::two(Decimal::TWO)]
    #[case::ln2(Decimal::LN2)]
    #[case::e(Decimal::E)]
    #[case::max(Decimal::MAX)]
    fn soroban_decimal_round_trips(#[case] original: Decimal) {
        let env = Env::default();
        let wrapped = SorobanDecimal::from_decimal(&env, original);
        assert_eq!(wrapped.to_decimal(), original, "to_decimal");
        assert_eq!(Decimal::from(&wrapped), original, "From<&SorobanDecimal>");
        assert_eq!(Decimal::from(wrapped), original, "From<SorobanDecimal>");
    }

    #[test]
    fn normalized_to_sep40_preserves_only_representable_values() {
        let price = |mantissa, expo| NormalizedPrice {
            mantissa,
            expo,
            timestamp: 100,
        };

        assert_eq!(
            normalized_to_sep40(&price(1, -1), 0),
            Err(ContractError::ConversionOverflow)
        );
        assert_eq!(
            normalized_to_sep40(&price(-1, -1), 0),
            Err(ContractError::ConversionOverflow)
        );
        assert_eq!(normalized_to_sep40(&price(10, -1), 0).unwrap().price, 1);
        assert_eq!(normalized_to_sep40(&price(0, -1), 0).unwrap().price, 0);
        assert_eq!(
            normalized_to_sep40(&price(i64::MAX, 120), 18),
            Err(ContractError::ConversionOverflow)
        );
        assert_eq!(
            normalized_to_sep40(&price(500_000, -4), 8).unwrap().price,
            5_000_000_000
        );
    }
}
