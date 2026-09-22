use templar_primitives::Nanoseconds;

use super::{optional_nanoseconds, reference_freshness_accepts};
use crate::{proxy::FreshnessFilter, Price};

#[kani::proof]
#[kani::solver(kissat)]
fn freshness_matches_optional_inclusive_limits() {
    let now = kani::any::<u64>();
    let publish_time_ns = kani::any::<u64>();
    let max_age = kani::any::<u64>();
    let max_clock_drift = kani::any::<u64>();
    let age_enabled = kani::any::<bool>();
    let clock_drift_enabled = kani::any::<bool>();
    let filter = FreshnessFilter::new(
        optional_nanoseconds(age_enabled, max_age),
        optional_nanoseconds(clock_drift_enabled, max_clock_drift),
    );
    let price = Price {
        price: 1,
        conf: 0,
        expo: 0,
        publish_time_ns: Nanoseconds::from_ns(publish_time_ns),
    };

    let actual = filter.accepts(&price, Nanoseconds::from_ns(now));
    let expected = reference_freshness_accepts(
        publish_time_ns,
        now,
        age_enabled.then_some(max_age),
        clock_drift_enabled.then_some(max_clock_drift),
    );
    assert_eq!(actual, expected);

    kani::cover!(
        now == 10 && publish_time_ns == 5 && age_enabled && max_age == 5 && actual,
        "past publication at the inclusive age boundary is accepted"
    );
    kani::cover!(
        now == 10 && publish_time_ns == 4 && age_enabled && max_age == 5 && !actual,
        "past publication one beyond the age boundary is rejected"
    );
    kani::cover!(
        now == 10 && publish_time_ns == 15 && clock_drift_enabled && max_clock_drift == 5 && actual,
        "future publication at the inclusive drift boundary is accepted"
    );
    kani::cover!(
        now == 10
            && publish_time_ns == 16
            && clock_drift_enabled
            && max_clock_drift == 5
            && !actual,
        "future publication one beyond the drift boundary is rejected"
    );
    kani::cover!(
        now > publish_time_ns && !age_enabled && actual,
        "an old publication remains eligible when age filtering is disabled"
    );
    kani::cover!(
        publish_time_ns > now && !clock_drift_enabled && actual,
        "a future publication remains eligible when drift filtering is disabled"
    );
    kani::cover!(
        publish_time_ns == 0 && !age_enabled && actual,
        "timestamp zero remains in the proof domain"
    );
    kani::cover!(
        now == u64::MAX && publish_time_ns == 0 && !age_enabled && actual,
        "u64 maximum remains in the proof domain"
    );
}
