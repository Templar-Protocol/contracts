use alloc::{vec, vec::Vec};

use templar_primitives::Nanoseconds;

use super::{
    bounded_u8, symbolic_freshness_inputs, symbolic_price_inputs, FreshnessInputs,
    SymbolicPriceInput, MAX_SOURCES,
};
use crate::{
    proxy::{
        aggregator::method::Error as AggregationError,
        circuit_breaker::{CircuitBreaker, CircuitBreakerSet},
        Proxy, ResolveError,
    },
    Price,
};

fn prices_for_count(
    inputs: &[SymbolicPriceInput; MAX_SOURCES],
    count: usize,
) -> Vec<Option<Price>> {
    let prices = [
        inputs[0].present.then_some(inputs[0].price()),
        inputs[1].present.then_some(inputs[1].price()),
        inputs[2].present.then_some(inputs[2].price()),
    ];
    match count {
        0 => vec![],
        1 => vec![prices[0]],
        2 => vec![prices[0], prices[1]],
        3 => vec![prices[0], prices[1], prices[2]],
        _ => panic!("source count is outside its bounded domain"),
    }
}

fn plain_sources_for_count(count: usize) -> Vec<u8> {
    match count {
        0 => vec![],
        1 => vec![0],
        2 => vec![0, 1],
        3 => vec![0, 1, 2],
        _ => panic!("source count is outside its bounded domain"),
    }
}

fn independently_survives(input: SymbolicPriceInput, freshness: FreshnessInputs) -> bool {
    input.present && input.has_valid_confidence() && freshness.accepts(input.publish_time_ns)
}

fn is_exact_freshness_boundary(input: SymbolicPriceInput, freshness: FreshnessInputs) -> bool {
    if freshness.now >= input.publish_time_ns {
        freshness.age_enabled && freshness.now - input.publish_time_ns == freshness.max_age
    } else {
        freshness.clock_drift_enabled
            && input.publish_time_ns - freshness.now == freshness.max_clock_drift
    }
}

#[kani::proof]
#[kani::solver(kissat)]
#[kani::unwind(8)]
fn priority_resolve_selects_first_surviving_source() {
    let source_count = usize::from(bounded_u8(3));
    let inputs = symbolic_price_inputs();
    let freshness = symbolic_freshness_inputs();
    let sources = plain_sources_for_count(source_count);
    let prices = prices_for_count(&inputs, source_count);
    let survivors = [
        source_count > 0 && independently_survives(inputs[0], freshness),
        source_count > 1 && independently_survives(inputs[1], freshness),
        source_count > 2 && independently_survives(inputs[2], freshness),
    ];
    let first_survivor = if survivors[0] {
        Some(inputs[0].price())
    } else if survivors[1] {
        Some(inputs[1].price())
    } else if survivors[2] {
        Some(inputs[2].price())
    } else {
        None
    };
    let has_exact_freshness_boundary = (survivors[0]
        && is_exact_freshness_boundary(inputs[0], freshness))
        || (survivors[1] && is_exact_freshness_boundary(inputs[1], freshness))
        || (survivors[2] && is_exact_freshness_boundary(inputs[2], freshness));

    let proxy = Proxy::priority(sources, freshness.filter());
    let result = proxy.resolve(
        &mut CircuitBreakerSet::<CircuitBreaker>::empty(),
        prices,
        Nanoseconds::from_ns(freshness.now),
    );

    match first_survivor {
        Some(expected) => {
            let outcome = match result {
                Ok(outcome) => outcome,
                Err(error) => panic!("priority resolution unexpectedly failed: {error}"),
            };
            assert_eq!(outcome.value, Ok(expected));
            kani::cover!(true, "filtered priority resolution can succeed");
            kani::cover!(
                source_count >= 2 && !survivors[0] && survivors[1],
                "priority skips an excluded first source for a later survivor"
            );
            kani::cover!(
                has_exact_freshness_boundary,
                "priority accepts a source at an inclusive freshness boundary"
            );
        }
        None => {
            assert_eq!(
                result,
                Err(ResolveError::Aggregation(
                    AggregationError::TooFewValidSources {
                        expected: 1,
                        actual: 0,
                    }
                ))
            );
            kani::cover!(true, "filtered priority resolution can fail");
            kani::cover!(
                source_count == 0,
                "empty priority sources return the exact missing-source error"
            );
            kani::cover!(
                source_count > 0,
                "all-excluded priority sources return the exact missing-source error"
            );
        }
    }
}
