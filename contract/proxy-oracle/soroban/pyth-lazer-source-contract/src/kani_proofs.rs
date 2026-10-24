//! Kani proofs for the pure rules behind `update_price_feeds`: the per-feed
//! admission rule, the ingest window, freshness validation, the feed registry
//! shape and the feed-id → SEP-40 key encoding. Every input ranges over its
//! whole type unless a harness says otherwise. The registry harness bounds
//! the list to `MAX_REGISTRY_IN_PROOF` ids; the 64-id cap is separate.
//!
//! Servability is `normalized_to_sep40` from the audited common crate; the
//! admission harness abstracts it as an arbitrary predicate, so its result
//! holds for any rescale rule, not just the deployed one. Distinct feeds
//! never share a key: the encoding harness shows each key decodes back to its
//! id.

use super::*;

const MICROS: u128 = MICROS_PER_SEC as u128;
const MAX_REGISTRY_IN_PROOF: usize = 4;

fn any_feed() -> FeedFields {
    FeedFields {
        mantissa: kani::any(),
        exponent: kani::any(),
        publish_time_us: kani::any(),
    }
}

/// The admission rule written as its specification.
fn eligible(feed: FeedFields, window: (u64, u64), stored_time_us: Option<u64>) -> bool {
    matches!(
        (feed.mantissa, feed.exponent, feed.publish_time_us),
        (Some(mantissa), Some(_), Some(time))
            if mantissa > 0
                && window.0 <= time
                && time <= window.1
                && stored_time_us.is_none_or(|stored| time > stored)
    )
}

#[kani::proof]
#[kani::solver(kissat)]
fn stored_price_is_positive_fresh_advancing_and_unchanged() {
    let feed = any_feed();
    let window: (u64, u64) = (kani::any(), kani::any());
    let stored_time_us: Option<u64> = kani::any();
    let servable: bool = kani::any();

    let admitted = admit_feed(feed, window, stored_time_us, |_| servable);

    if let Some(stored) = &admitted {
        // The stored price is the verified feed, unrescaled.
        assert_eq!(Some(stored.mantissa), feed.mantissa);
        assert_eq!(Some(stored.expo), feed.exponent.map(i32::from));
        assert_eq!(Some(stored.publish_time_us), feed.publish_time_us);
        assert!(stored.mantissa > 0);
        assert!(window.0 <= stored.publish_time_us && stored.publish_time_us <= window.1);
        if let Some(previous) = stored_time_us {
            assert!(stored.publish_time_us > previous);
        }
    }
    kani::cover!(
        admitted.is_some() && feed.publish_time_us == Some(window.0),
        "the oldest instant of the window is admitted"
    );
    kani::cover!(
        admitted.is_some() && feed.publish_time_us == Some(window.1),
        "the latest instant of the window is admitted"
    );
    kani::cover!(
        admitted.is_some()
            && stored_time_us.is_some_and(|stored| feed.publish_time_us == stored.checked_add(1)),
        "one microsecond past the stored price advances"
    );
}

#[kani::proof]
#[kani::solver(kissat)]
fn admission_skips_exactly_ineligible_or_unservable_feeds() {
    let feed = any_feed();
    let window: (u64, u64) = (kani::any(), kani::any());
    let stored_time_us: Option<u64> = kani::any();
    let servable: bool = kani::any();
    let mut asked: Option<(i64, i32)> = None;

    let admitted = admit_feed(feed, window, stored_time_us, |price| {
        asked = Some((price.mantissa, price.expo));
        servable
    });

    let eligible = eligible(feed, window, stored_time_us);
    // Servability is consulted exactly for eligible feeds, on the feed's own
    // price (`served` derives its time the same way `lastprice` does), and an
    // eligible, servable feed is never skipped.
    assert_eq!(asked.is_some(), eligible);
    if let Some((mantissa, expo)) = asked {
        assert_eq!(Some(mantissa), feed.mantissa);
        assert_eq!(Some(expo), feed.exponent.map(i32::from));
    }
    assert_eq!(admitted.is_some(), eligible && servable);
    kani::cover!(
        admitted.is_none() && stored_time_us.is_some() && stored_time_us == feed.publish_time_us,
        "a replay of the stored publish time is skipped"
    );
    kani::cover!(
        admitted.is_none() && feed.mantissa == Some(0),
        "a zero price is skipped"
    );
    kani::cover!(
        admitted.is_none() && feed.exponent.is_none(),
        "a feed without an exponent is skipped"
    );
    kani::cover!(eligible && !servable, "an unservable scale is skipped");
}

#[kani::proof]
fn ingest_window_matches_exact_arithmetic() {
    let now_secs: u64 = kani::any();
    let freshness = FreshnessConfig {
        max_age_secs: kani::any(),
        max_clock_drift_secs: kani::any(),
    };
    let now_us = u128::from(now_secs) * MICROS;
    let age_us = u128::from(freshness.max_age_secs) * MICROS;
    let drift_us = u128::from(freshness.max_clock_drift_secs) * MICROS;
    let representable = [now_us, age_us, drift_us]
        .iter()
        .all(|value| *value <= u128::from(u64::MAX));

    match freshness_bounds(now_secs, &freshness) {
        Err(error) => {
            assert!(!representable);
            assert_eq!(error, LazerSourceError::ArithmeticOverflow);
        }
        Ok((oldest, latest)) => {
            assert!(representable);
            let time: u64 = kani::any();
            let in_window = oldest <= time && time <= latest;
            let exact =
                u128::from(time) + age_us >= now_us && u128::from(time) <= now_us + drift_us;
            assert_eq!(in_window, exact);
            // Ledger time itself is always ingestible.
            let now = u64::try_from(now_us).expect("representable");
            assert!(oldest <= now && now <= latest);
            kani::cover!(oldest == 0 && now_us > 0, "the window clamps at zero");
            kani::cover!(
                latest == u64::MAX && now_us + drift_us > u128::from(u64::MAX),
                "the window clamps at u64::MAX"
            );
        }
    }
}

#[kani::proof]
fn freshness_validation_bounds_the_window() {
    let freshness = FreshnessConfig {
        max_age_secs: kani::any(),
        max_clock_drift_secs: kani::any(),
    };
    let accepted = validate_freshness(&freshness).is_ok();
    assert_eq!(
        accepted,
        (1..=MAX_INGEST_AGE_SECS).contains(&freshness.max_age_secs)
            && freshness.max_clock_drift_secs <= MAX_INGEST_CLOCK_DRIFT_SECS
    );
    if accepted {
        // A validated config never overflows: only an unrepresentable ledger
        // time can, and that fails the push before any write.
        let now_secs: u64 = kani::any();
        assert_eq!(
            freshness_bounds(now_secs, &freshness).is_err(),
            now_secs > u64::MAX / MICROS_PER_SEC
        );
    }
    kani::cover!(accepted, "a freshness config is accepted");
    kani::cover!(
        !accepted && freshness.max_age_secs == 0,
        "a zero ingest age is rejected"
    );
}

#[kani::proof]
#[kani::solver(kissat)]
#[kani::unwind(11)]
fn feed_id_encoding_is_canonical_decimal() {
    let feed_id: u32 = kani::any();
    let (buffer, start) = decimal_digits(feed_id);
    assert!(start < buffer.len());
    // Fixed-shape walk over the whole buffer: unused leading bytes stay zero,
    // the rest are ASCII digits (so `feed_asset`'s `from_utf8` cannot fail),
    // and their decimal value is the id.
    let mut value = 0_u64;
    for (index, byte) in buffer.iter().enumerate() {
        if index < start {
            assert_eq!(*byte, 0);
        } else {
            assert!(byte.is_ascii_digit());
            value = value * 10 + u64::from(byte - b'0');
        }
    }
    assert_eq!(value, u64::from(feed_id));
    // Canonical: no leading zero except the id 0 itself.
    assert!(buffer[start] != b'0' || start == buffer.len() - 1);
    kani::cover!(feed_id == u32::MAX && start == 0, "ten digits");
    kani::cover!(feed_id == 0 && start == 9, "zero is one digit");
}

#[kani::proof]
#[kani::unwind(6)]
fn feed_registry_is_bounded_strictly_increasing() {
    let ids: [u32; MAX_REGISTRY_IN_PROOF] = kani::any();
    let count: usize = kani::any();
    kani::assume(count <= MAX_REGISTRY_IN_PROOF);
    let listed = &ids[..count];
    let len = u32::try_from(count).expect("small");

    let accepted = is_valid_feed_registry(len, listed.iter().copied());
    let increasing = listed.windows(2).all(|pair| pair[0] < pair[1]);
    assert_eq!(accepted, count >= 1 && increasing);
    if accepted {
        // Unique ids: no feed can be listed twice.
        for (index, id) in listed.iter().enumerate() {
            assert!(!listed[index + 1..].contains(id));
        }
    }
    kani::cover!(accepted && count == MAX_REGISTRY_IN_PROOF, "a full list");
    kani::cover!(
        !accepted && count == 2 && ids[0] == ids[1],
        "a duplicate is rejected"
    );
    kani::cover!(
        !accepted && count == 2 && ids[0] > ids[1],
        "a descending pair is rejected"
    );
}

#[kani::proof]
#[kani::unwind(6)]
fn feed_registry_rejects_out_of_range_lengths() {
    let ids: [u32; MAX_REGISTRY_IN_PROOF] = kani::any();
    let count: usize = kani::any();
    kani::assume(count <= MAX_REGISTRY_IN_PROOF);
    let len: u32 = kani::any();
    kani::assume(len == 0 || len > MAX_SUPPORTED_FEEDS);
    assert!(!is_valid_feed_registry(len, ids[..count].iter().copied()));
}
