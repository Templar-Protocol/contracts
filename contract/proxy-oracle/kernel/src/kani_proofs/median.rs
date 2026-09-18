use super::{bounded_i8, bounded_u8, MAX_SOURCES};
use crate::proxy::aggregator::method::median::median_indices_for_proof;

fn endpoint_occurrences(source_ids: &[u8; 6], endpoint_count: usize, source_id: u8) -> u8 {
    u8::from(endpoint_count > 0 && source_ids[0] == source_id)
        + u8::from(endpoint_count > 1 && source_ids[1] == source_id)
        + u8::from(endpoint_count > 2 && source_ids[2] == source_id)
        + u8::from(endpoint_count > 3 && source_ids[3] == source_id)
        + u8::from(endpoint_count > 4 && source_ids[4] == source_id)
        + u8::from(endpoint_count > 5 && source_ids[5] == source_id)
}

fn source_weight(source_id: u8, source_weights: &[u32; MAX_SOURCES]) -> u32 {
    match source_id {
        0 => source_weights[0],
        1 => source_weights[1],
        2 => source_weights[2],
        _ => 0,
    }
}

fn active_weight_sum(weights: &[u32; 6], endpoint_count: usize) -> u128 {
    match endpoint_count {
        2 => u128::from(weights[0]) + u128::from(weights[1]),
        4 => {
            u128::from(weights[0])
                + u128::from(weights[1])
                + u128::from(weights[2])
                + u128::from(weights[3])
        }
        6 => {
            u128::from(weights[0])
                + u128::from(weights[1])
                + u128::from(weights[2])
                + u128::from(weights[3])
                + u128::from(weights[4])
                + u128::from(weights[5])
        }
        _ => 0,
    }
}

fn prove_weighted_median_index(high: bool) {
    let source_count = usize::from(bounded_u8(MAX_SOURCES as u8));
    kani::assume(source_count > 0);
    let endpoint_count = source_count * 2;
    let values = [
        bounded_i8(-8, 8),
        bounded_i8(-8, 8),
        bounded_i8(-8, 8),
        bounded_i8(-8, 8),
        bounded_i8(-8, 8),
        bounded_i8(-8, 8),
    ];
    kani::assume(values[0] <= values[1]);
    if endpoint_count >= 4 {
        kani::assume(values[1] <= values[2]);
        kani::assume(values[2] <= values[3]);
    }
    if endpoint_count == 6 {
        kani::assume(values[3] <= values[4]);
        kani::assume(values[4] <= values[5]);
    }

    let source_ids = [
        bounded_u8(2),
        bounded_u8(2),
        bounded_u8(2),
        bounded_u8(2),
        bounded_u8(2),
        bounded_u8(2),
    ];
    kani::assume(source_ids[0] < source_count as u8);
    kani::assume(source_ids[1] < source_count as u8);
    if endpoint_count >= 4 {
        kani::assume(source_ids[2] < source_count as u8);
        kani::assume(source_ids[3] < source_count as u8);
    }
    if endpoint_count == 6 {
        kani::assume(source_ids[4] < source_count as u8);
        kani::assume(source_ids[5] < source_count as u8);
    }
    kani::assume(endpoint_occurrences(&source_ids, endpoint_count, 0) == 2);
    if source_count >= 2 {
        kani::assume(endpoint_occurrences(&source_ids, endpoint_count, 1) == 2);
    }
    if source_count == 3 {
        kani::assume(endpoint_occurrences(&source_ids, endpoint_count, 2) == 2);
    }

    let source_weights = [
        u32::from(bounded_u8(3)),
        u32::from(bounded_u8(3)),
        u32::from(bounded_u8(3)),
    ];
    let weights = [
        source_weight(source_ids[0], &source_weights),
        source_weight(source_ids[1], &source_weights),
        source_weight(source_ids[2], &source_weights),
        source_weight(source_ids[3], &source_weights),
        source_weight(source_ids[4], &source_weights),
        source_weight(source_ids[5], &source_weights),
    ];
    let entries = [
        (values[0], weights[0]),
        (values[1], weights[1]),
        (values[2], weights[2]),
        (values[3], weights[3]),
        (values[4], weights[4]),
        (values[5], weights[5]),
    ];
    let active_entries = &entries[..endpoint_count];
    let (low, high_index) = median_indices_for_proof(active_entries);
    assert!(low < endpoint_count);
    assert!(high_index < endpoint_count);
    assert!(low <= high_index);
    let index = if high { high_index } else { low };
    let selected = values[index];
    let upper_bound = match endpoint_count {
        2 => values[1],
        4 => values[3],
        6 => values[5],
        _ => values[0],
    };
    assert!(selected >= values[0]);
    assert!(selected <= upper_bound);
    kani::cover!(source_count == 1, "one-source endpoint shape is reachable");
    kani::cover!(source_count == 2, "two-source endpoint shape is reachable");
    kani::cover!(
        source_count == 3,
        "three-source endpoint shape is reachable"
    );

    let total_weight = active_weight_sum(&weights, endpoint_count);
    if total_weight == 0 {
        let expected = ((endpoint_count - 1) / 2, endpoint_count / 2);
        assert_eq!((low, high_index), expected);
        kani::cover!(
            source_count == 1,
            "one-source all-zero weights use positional middle indices"
        );
        kani::cover!(
            source_count == 2,
            "two-source all-zero weights use positional middle indices"
        );
        kani::cover!(
            source_count == 3,
            "three-source all-zero weights use positional middle indices"
        );
        return;
    }
    let target = if high {
        total_weight / 2 + 1
    } else {
        total_weight.div_ceil(2)
    };
    let weight_before = match index {
        0 => 0,
        1 => u128::from(weights[0]),
        2 => u128::from(weights[0]) + u128::from(weights[1]),
        3 => u128::from(weights[0]) + u128::from(weights[1]) + u128::from(weights[2]),
        4 => {
            u128::from(weights[0])
                + u128::from(weights[1])
                + u128::from(weights[2])
                + u128::from(weights[3])
        }
        5 => {
            u128::from(weights[0])
                + u128::from(weights[1])
                + u128::from(weights[2])
                + u128::from(weights[3])
                + u128::from(weights[4])
        }
        _ => 0,
    };
    let selected_weight = match index {
        0 => weights[0],
        1 => weights[1],
        2 => weights[2],
        3 => weights[3],
        4 => weights[4],
        5 => weights[5],
        _ => 0,
    };
    assert!(weight_before < target);
    assert!(weight_before + u128::from(selected_weight) >= target);
    kani::cover!(
        source_weights[0] == 1
            && (source_count < 2 || source_weights[1] == 1)
            && (source_count < 3 || source_weights[2] == 1)
            && low + 1 == high_index,
        "equal source weights distinguish lower and upper medians"
    );
    kani::cover!(
        weights[0] == 0 && selected_weight > 0,
        "zero leading endpoint weight is skipped"
    );
}

#[kani::proof]
#[kani::solver(kissat)]
#[kani::unwind(24)]
fn median_low_index_satisfies_weighted_rank() {
    prove_weighted_median_index(false);
}

#[kani::proof]
#[kani::solver(kissat)]
#[kani::unwind(24)]
fn median_high_index_satisfies_weighted_rank() {
    prove_weighted_median_index(true);
}
