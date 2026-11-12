//! Drive `Proxy::resolve` and the `CircuitBreakerSet` governance API through arbitrary
//! sequences: price moves of any size, sources going absent, stale or future-dated and coming
//! back, and breakers being added, removed, toggled, re-armed and reconfigured between
//! refreshes. Every step is held to the reference model in `templar_fuzz::oracle`:
//!
//! * the resolved value, every emitted event, and the whole set state equal the model's, so a
//!   failed resolve must leave the set untouched;
//! * a priority result is the first surviving source, a median result an endpoint carrying the
//!   weighted median's value;
//! * independently of the model, both histories keep strictly increasing publish times within
//!   capacity, an accepted price coincides with an unblocked set, the set stays valid, and it
//!   round-trips through both storage codecs.
//!
//! Source count, history length and sequence length are capped for speed. Mantissas,
//! thresholds and rule parameters are not narrowed, and `i8` exponents already cross the
//! kernel's 512-bit fail-closed boundary; `fuzz_oracle_deviation_math` owns the full `i32`
//! exponent range.
//!
//! MUTATION-CHECK (P5): in `CircuitBreakerSet::try_accept_price`
//! (contract/proxy-oracle/kernel/src/proxy/circuit_breaker/set.rs), drop
//! `&& accepted_history_should_advance` from the condition that commits accepted history. A
//! refresh whose publish time does not advance then lands in accepted history, and the model
//! comparison must fire.

#![no_main]
#![allow(
    clippy::expect_used,
    reason = "panics on invariant violation are the intended libFuzzer crash signal"
)]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use templar_fuzz::oracle::{self, breaker_set::ModelSet, input::RuleInput};
use templar_primitives::Nanoseconds;
use templar_proxy_oracle_kernel::{
    primitive::AccountId,
    proxy::{
        aggregator::method::{
            median::{MedianHigh, MedianLow},
            Aggregate,
        },
        circuit_breaker::{CircuitBreakerSet, CircuitBreakerSetConfig, Observation, RingBuffer},
        Aggregator, FreshnessFilter, Proxy, ResolveError, WeightedSource,
    },
    Price,
};

const MAX_SOURCES: usize = 4;
const MAX_HISTORY_LEN: u8 = 32;
const MAX_ACTIONS: usize = 64;
/// Start of simulated time, far enough from zero for quotes published in the past.
const GENESIS_S: u64 = 1_000_000;
const BASIS_POINTS: i128 = 10_000;
const ACTOR: AccountId = AccountId::from_bytes([1; 64]);

fn seconds(value: impl Into<u64>) -> Nanoseconds {
    Nanoseconds::from_secs(value.into())
}

/// `now` shifted into the past (positive) or the future (negative) by `offset_s` seconds.
fn before(now: Nanoseconds, offset_s: i16) -> Nanoseconds {
    let offset = seconds(offset_s.unsigned_abs());
    if offset_s >= 0 {
        now.saturating_sub(offset)
    } else {
        now.saturating_add(offset)
    }
}

fn saturating_i64(value: i128) -> i64 {
    i64::try_from(value).unwrap_or(if value < 0 { i64::MIN } else { i64::MAX })
}

#[derive(Arbitrary, Debug, Clone, Copy)]
enum AggregatorKind {
    Priority,
    MedianLow,
    MedianHigh,
}

/// A median quorum: usually one the proxy's sources can meet, sometimes any `u8`.
#[derive(Arbitrary, Debug, Clone, Copy)]
enum QuorumInput {
    Reachable(u8),
    Raw(u8),
}

impl QuorumInput {
    fn get(self) -> u32 {
        match self {
            Self::Reachable(quorum) => {
                u32::try_from(usize::from(quorum) % (MAX_SOURCES + 1)).unwrap_or_default()
            }
            Self::Raw(quorum) => quorum.into(),
        }
    }
}

#[derive(Arbitrary, Debug)]
struct ProxyInput {
    kind: AggregatorKind,
    source_count: u8,
    weights: [u32; MAX_SOURCES],
    min_sources: QuorumInput,
    max_age_s: Option<u16>,
    max_clock_drift_s: Option<u16>,
}

impl ProxyInput {
    fn proxy(&self) -> Proxy<usize> {
        let source_count = usize::from(self.source_count) % (MAX_SOURCES + 1);
        let weighted = || {
            self.weights
                .iter()
                .take(source_count)
                .enumerate()
                .map(|(source, weight)| WeightedSource::new(source, *weight))
        };
        let aggregator = match self.kind {
            AggregatorKind::Priority => Aggregator::priority(0..source_count),
            AggregatorKind::MedianLow => {
                let mut median = MedianLow::new(weighted());
                median.min_sources = self.min_sources.get();
                Aggregator::MedianLow(median)
            }
            AggregatorKind::MedianHigh => {
                let mut median = MedianHigh::new(weighted());
                median.min_sources = self.min_sources.get();
                Aggregator::MedianHigh(median)
            }
        };
        Proxy::new(
            aggregator,
            FreshnessFilter::new(
                self.max_age_s.map(seconds),
                self.max_clock_drift_s.map(seconds),
            ),
        )
    }
}

#[derive(Arbitrary, Debug, Clone, Copy)]
struct ConfigInput {
    sample_interval_s: Option<u8>,
    history_len: u8,
}

impl ConfigInput {
    fn config(self) -> CircuitBreakerSetConfig {
        CircuitBreakerSetConfig {
            sample_interval_ns: seconds(self.sample_interval_s.unwrap_or_default()),
            history_len: (self.history_len % (MAX_HISTORY_LEN + 1)).into(),
        }
    }
}

#[derive(Arbitrary, Debug, Clone, Copy)]
struct RawQuote {
    mantissa: i64,
    confidence: u64,
    exponent: i8,
    age_s: i16,
}

impl RawQuote {
    fn price(self, now: Nanoseconds) -> Price {
        Price {
            price: self.mantissa,
            conf: self.confidence,
            expo: self.exponent.into(),
            publish_time_ns: before(now, self.age_s),
        }
    }
}

#[derive(Arbitrary, Debug, Clone, Copy)]
enum Quote {
    Absent,
    /// A move of `change_bps` from the last aggregated price, re-expressed `rescale % 4`
    /// decimal places away so equal values meet at different exponents.
    Drift {
        change_bps: i16,
        confidence_bps: u16,
        rescale: i8,
        age_s: i16,
    },
    Raw(RawQuote),
}

impl Quote {
    fn price(self, level: &Price, now: Nanoseconds) -> Option<Price> {
        match self {
            Self::Absent => None,
            Self::Raw(quote) => Some(quote.price(now)),
            Self::Drift {
                change_bps,
                confidence_bps,
                rescale,
                age_s,
            } => {
                let moved = i128::from(level.price) * (BASIS_POINTS + i128::from(change_bps))
                    / BASIS_POINTS;
                let shift = rescale % 4;
                let factor = 10_i128.pow(u32::from(shift.unsigned_abs()));
                let mantissa = if shift >= 0 {
                    moved * factor
                } else {
                    moved / factor
                };
                let exponent = level.expo.saturating_sub(shift.into());
                let confidence = mantissa.unsigned_abs() * u128::from(confidence_bps)
                    / BASIS_POINTS.unsigned_abs();
                Some(Price {
                    price: saturating_i64(mantissa),
                    conf: u64::try_from(confidence).unwrap_or(u64::MAX),
                    expo: exponent,
                    publish_time_ns: before(now, age_s),
                })
            }
        }
    }
}

/// An installed breaker (by position among installed ids) or any id, to reach `BreakerNotFound`.
#[derive(Arbitrary, Debug, Clone, Copy)]
enum BreakerRef {
    Installed(u8),
    Any(u32),
}

#[derive(Arbitrary, Debug)]
enum Action {
    Refresh {
        advance_s: u16,
        quotes: [Quote; MAX_SOURCES],
    },
    /// A refresh whose quote count may not match the proxy's sources.
    RefreshUnaligned {
        advance_s: u16,
        quotes: Vec<Quote>,
    },
    Add {
        rule: RuleInput<RawQuote>,
        breaker_id: Option<u32>,
    },
    Remove {
        breaker: BreakerRef,
    },
    SetEnforced {
        breaker: BreakerRef,
        is_enforced: bool,
    },
    Rearm {
        breaker: BreakerRef,
        armed_after_offset_s: i16,
    },
    SetConfig(ConfigInput),
    SetManualTrip {
        is_manually_tripped: bool,
        metadata: Option<u8>,
    },
}

#[derive(Arbitrary, Debug)]
struct Scenario {
    proxy: ProxyInput,
    config: ConfigInput,
    initial_mantissa: u32,
    initial_exponent: i8,
    actions: Vec<Action>,
}

struct Harness {
    proxy: Proxy<usize>,
    set: CircuitBreakerSet,
    model: ModelSet,
    now: Nanoseconds,
    /// The last aggregated price, which `Quote::Drift` moves from.
    level: Price,
}

impl Harness {
    fn new(scenario: &Scenario) -> Self {
        let config = scenario.config.config();
        let now = seconds(GENESIS_S);
        Self {
            proxy: scenario.proxy.proxy(),
            set: CircuitBreakerSet::new(config),
            model: ModelSet::new(config),
            now,
            level: Price {
                price: i64::from(scenario.initial_mantissa) + 1,
                conf: 0,
                expo: scenario.initial_exponent.into(),
                publish_time_ns: now,
            },
        }
    }

    fn apply(&mut self, action: &Action) {
        match action {
            Action::Refresh { advance_s, quotes } => {
                self.now = self.now.saturating_add(seconds(*advance_s));
                let source_count = self.proxy.sources().len();
                self.refresh(&quotes[..source_count]);
            }
            Action::RefreshUnaligned { advance_s, quotes } => {
                self.now = self.now.saturating_add(seconds(*advance_s));
                self.refresh(&quotes[..quotes.len().min(MAX_SOURCES + 1)]);
            }
            Action::Add { rule, breaker_id } => {
                let breaker = rule.breaker(|baseline| baseline.price(self.now));
                let breaker_id = breaker_id.unwrap_or(self.set.next_id());
                assert_eq!(
                    self.set.add(breaker_id, breaker.clone()),
                    self.model.add(breaker_id, breaker)
                );
            }
            Action::Remove { breaker } => {
                let breaker_id = self.breaker_id(*breaker);
                assert_eq!(self.set.remove(breaker_id), self.model.remove(breaker_id));
            }
            Action::SetEnforced {
                breaker,
                is_enforced,
            } => {
                let breaker_id = self.breaker_id(*breaker);
                assert_eq!(
                    self.set.set_enforced(breaker_id, *is_enforced),
                    self.model.set_enforced(breaker_id, *is_enforced)
                );
            }
            Action::Rearm {
                breaker,
                armed_after_offset_s,
            } => {
                let breaker_id = self.breaker_id(*breaker);
                let armed_after = before(self.now, *armed_after_offset_s);
                assert_eq!(
                    self.set.rearm(breaker_id, armed_after),
                    self.model.rearm(breaker_id, armed_after)
                );
            }
            Action::SetConfig(config) => {
                let config = config.config();
                assert_eq!(self.set.set_config(config), self.model.set_config(config));
            }
            Action::SetManualTrip {
                is_manually_tripped,
                metadata,
            } => {
                let metadata = metadata.map(|byte| vec![byte]);
                assert_eq!(
                    self.set
                        .set_manual_trip(*is_manually_tripped, ACTOR, metadata.clone()),
                    self.model
                        .set_manual_trip(*is_manually_tripped, ACTOR, metadata)
                );
            }
        }
        assert_eq!(ModelSet::from(&self.set), self.model, "after {action:?}");
        assert_well_formed(&self.set);
    }

    fn breaker_id(&self, breaker: BreakerRef) -> u32 {
        let mut installed = self.set.breakers().keys();
        match breaker {
            BreakerRef::Installed(position) if installed.len() > 0 => {
                let position = usize::from(position) % installed.len();
                *installed.nth(position).expect("position is in range")
            }
            BreakerRef::Installed(position) => position.into(),
            BreakerRef::Any(breaker_id) => breaker_id,
        }
    }

    fn refresh(&mut self, quotes: &[Quote]) {
        let prices = quotes
            .iter()
            .map(|quote| quote.price(&self.level, self.now))
            .collect::<Vec<_>>();
        let resolution = self.proxy.resolve(&mut self.set, prices.clone(), self.now);

        let survivors = prices
            .into_iter()
            .map(|price| {
                price
                    .filter(|price| oracle::survives(&self.proxy.freshness_filter, price, self.now))
            })
            .collect::<Vec<_>>();
        let expected = match oracle::aggregate(&self.proxy.aggregator, &survivors) {
            Err(error) => Err(ResolveError::Aggregation(error)),
            Ok(candidates) => {
                let aggregated = self
                    .proxy
                    .aggregator
                    .aggregate(survivors)
                    .expect("the model found a price to aggregate");
                assert!(
                    candidates.contains(&aggregated),
                    "aggregated {aggregated:?}, expected one of {candidates:?}"
                );
                self.level = aggregated;
                self.model
                    .try_accept_price(aggregated, self.now)
                    .map_err(ResolveError::CircuitBreaker)
            }
        };
        assert_eq!(resolution, expected);
        if let Ok(outcome) = &resolution {
            assert_eq!(outcome.value.is_ok(), !self.set.is_blocking());
        }
    }
}

fn assert_well_formed(set: &CircuitBreakerSet) {
    let capacity = set.accepted_history().capacity();
    assert_eq!(set.observed_history().capacity(), capacity);
    for history in [set.accepted_history(), set.observed_history()] {
        assert!(u32::try_from(history.len()).is_ok_and(|len| len <= capacity));
        assert!(
            publish_times_increase(history),
            "publish times must strictly increase: {history:?}"
        );
    }
    assert!(set
        .breakers()
        .keys()
        .all(|breaker_id| *breaker_id < set.next_id()));
    assert_eq!(set.validate(), Ok(()));
    oracle::assert_storage_round_trips(set);
}

fn publish_times_increase(history: &RingBuffer<Observation>) -> bool {
    history
        .iter()
        .zip(history.iter().skip(1))
        .all(|(earlier, later)| earlier.price.publish_time_ns < later.price.publish_time_ns)
}

fuzz_target!(|scenario: Scenario| {
    let mut harness = Harness::new(&scenario);
    for action in scenario.actions.iter().take(MAX_ACTIONS) {
        harness.apply(action);
    }
});
