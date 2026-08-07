use core::cmp::Ordering;

use primitive_types::U512;
use templar_primitives::Decimal;

use crate::{price::compare_scaled, Price};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StepChange {
    Decrease,
    Minor,
    Increase,
}

pub(super) fn classify_step_change(
    previous: &Price,
    current: &Price,
    min_relative_step_change: Decimal,
) -> StepChange {
    let step_change =
        match compare_scaled(previous.price, previous.expo, current.price, current.expo) {
            Ordering::Less => StepChange::Increase,
            Ordering::Greater => StepChange::Decrease,
            Ordering::Equal => return StepChange::Minor,
        };

    if relative_abs_change_at_least(previous, current, min_relative_step_change) {
        step_change
    } else {
        StepChange::Minor
    }
}

pub(super) fn relative_abs_change_exceeds(
    previous: &Price,
    current: &Price,
    max_relative_change: Decimal,
) -> bool {
    compare_relative_abs_change(previous, current, max_relative_change, false)
}

pub(super) fn relative_abs_change_at_least(
    previous: &Price,
    current: &Price,
    min_relative_change: Decimal,
) -> bool {
    compare_relative_abs_change(previous, current, min_relative_change, true)
}

fn compare_relative_abs_change(
    previous: &Price,
    current: &Price,
    threshold: Decimal,
    inclusive: bool,
) -> bool {
    if previous.price == 0 {
        return current.price != 0;
    }

    let Some((difference, baseline)) = scaled_difference(previous, current) else {
        return true;
    };
    compare_scaled_change(difference, baseline, threshold, inclusive).unwrap_or(true)
}

fn compare_scaled_change(
    difference: U512,
    baseline: U512,
    threshold: Decimal,
    inclusive: bool,
) -> Option<bool> {
    let left = checked_shift_left(difference, 128)?;
    let (right, overflowed) = baseline.overflowing_mul(U512(threshold.as_repr()));
    if overflowed {
        return None;
    }

    Some(if inclusive {
        left >= right
    } else {
        left > right
    })
}

pub(super) fn relative_sum_change_exceeds<'a, I, J>(
    previous: I,
    current: J,
    threshold: Decimal,
) -> bool
where
    I: Clone + Iterator<Item = &'a Price>,
    J: Clone + Iterator<Item = &'a Price>,
{
    let Some(common_exponent) = previous
        .clone()
        .chain(current.clone())
        .map(|price| price.expo)
        .min()
    else {
        return false;
    };
    let Some(previous) = checked_price_sum(previous, common_exponent) else {
        return true;
    };
    let Some(current) = checked_price_sum(current, common_exponent) else {
        return true;
    };
    if previous.is_zero() {
        return !current.is_zero();
    }
    let difference = if previous >= current {
        previous - current
    } else {
        current - previous
    };
    compare_scaled_change(difference, previous, threshold, false).unwrap_or(true)
}
fn checked_price_sum<'a>(
    mut prices: impl Iterator<Item = &'a Price>,
    common_exponent: i32,
) -> Option<U512> {
    prices.try_fold(U512::zero(), |sum, price| {
        if price.price <= 0 {
            return None;
        }
        let exponent = u32::try_from(price.expo.checked_sub(common_exponent)?).ok()?;
        let scaled = checked_scaled_magnitude(u64::try_from(price.price).ok()?, exponent)?;
        let (sum, overflowed) = sum.overflowing_add(scaled);
        (!overflowed).then_some(sum)
    })
}

fn scaled_difference(previous: &Price, current: &Price) -> Option<(U512, U512)> {
    let common_exponent = previous.expo.min(current.expo);
    let same_sign = previous.price.signum() == current.price.signum();
    let previous = checked_scaled_magnitude(
        previous.price.unsigned_abs(),
        u32::try_from(previous.expo.checked_sub(common_exponent)?).ok()?,
    )?;
    let current = checked_scaled_magnitude(
        current.price.unsigned_abs(),
        u32::try_from(current.expo.checked_sub(common_exponent)?).ok()?,
    )?;
    let difference = if same_sign {
        if previous >= current {
            previous - current
        } else {
            current - previous
        }
    } else {
        let (sum, overflowed) = previous.overflowing_add(current);
        if overflowed {
            return None;
        }
        sum
    };
    Some((difference, previous))
}

fn checked_scaled_magnitude(magnitude: u64, exponent: u32) -> Option<U512> {
    let mut factor = U512::one();
    for _ in 0..exponent {
        let (next, overflowed) = factor.overflowing_mul(U512::from(10));
        if overflowed {
            return None;
        }
        factor = next;
    }
    let (scaled, overflowed) = U512::from(magnitude).overflowing_mul(factor);
    (!overflowed).then_some(scaled)
}

fn checked_shift_left(value: U512, bits: usize) -> Option<U512> {
    if value.bits().saturating_add(bits) > 512 {
        None
    } else {
        Some(value << bits)
    }
}

#[cfg(test)]
mod tests {
    use core::str::FromStr;

    use templar_primitives::Nanoseconds;

    use super::*;

    fn dec(value: &str) -> Decimal {
        Decimal::from_str(value).unwrap()
    }

    fn price(price: i64, expo: i32) -> Price {
        Price {
            price,
            conf: 0,
            expo,
            publish_time_ns: Nanoseconds::zero(),
        }
    }

    #[rstest::rstest]
    #[case(price(100, 0), price(111, 0), dec("0.10"), StepChange::Increase)]
    #[case(price(10000, -2), price(111, 0), dec("0.10"), StepChange::Increase)]
    #[case(price(1, 2), price(111, 0), dec("0.10"), StepChange::Increase)]
    #[case(price(100, 0), price(89, 0), dec("0.10"), StepChange::Decrease)]
    #[case(price(10000, -2), price(89, 0), dec("0.10"), StepChange::Decrease)]
    #[case(price(1, 2), price(89, 0), dec("0.10"), StepChange::Decrease)]
    #[case(price(100, 0), price(-20, 0), dec("1.0"), StepChange::Decrease)]
    #[case(price(100, 0), price(110, 0), dec("0.1001"), StepChange::Minor)]
    #[case(price(1, -3), price(10, -4), Decimal::ZERO, StepChange::Minor)]
    fn classify_step_change_classifies_thresholded_moves(
        #[case] previous: Price,
        #[case] current: Price,
        #[case] min_relative_step_change: Decimal,
        #[case] expected: StepChange,
    ) {
        assert_eq!(
            classify_step_change(&previous, &current, min_relative_step_change),
            expected
        );
    }

    #[rstest::rstest]
    #[case(price(100, 0), price(111, 0), dec("0.10"), true)]
    #[case(price(100, 0), price(110, 0), dec("0.1001"), false)]
    #[case(price(100, 0), price(89, 0), dec("0.10"), true)]
    #[case(price(100, 0), price(-20, 0), dec("1.0"), true)]
    #[case(price(0, 0), price(0, 0), Decimal::ZERO, false)]
    #[case(price(0, 0), price(100, 0), Decimal::ZERO, true)]
    #[case(price(1, -3), price(10, -4), Decimal::ZERO, false)]
    fn relative_abs_change_exceeds_compares_distance_from_one(
        #[case] previous: Price,
        #[case] current: Price,
        #[case] max_relative_change: Decimal,
        #[case] expected: bool,
    ) {
        assert_eq!(
            relative_abs_change_exceeds(&previous, &current, max_relative_change),
            expected
        );
    }

    #[test]
    fn hal_38_inclusive_minimum_and_strict_maximum_differ_at_boundary() {
        let first = price(100, 0);
        let last = price(150, 0);
        let threshold = dec("0.5");

        assert!(relative_abs_change_at_least(&first, &last, threshold));
        assert!(!relative_abs_change_exceeds(&first, &last, threshold));
    }

    #[test]
    fn exact_relative_comparison_handles_mixed_exponents_and_zero_baselines() {
        assert!(!relative_abs_change_exceeds(
            &price(1, -3),
            &price(10, -4),
            Decimal::ZERO,
        ));
        assert!(relative_abs_change_exceeds(
            &price(0, 0),
            &price(1, 0),
            Decimal::MAX,
        ));
        assert!(relative_abs_change_exceeds(
            &price(1, i32::MIN),
            &price(1, i32::MAX),
            Decimal::MAX,
        ));
    }

    #[test]
    fn exact_window_sum_comparison_is_strict() {
        let previous = [price(100, 0), price(100, 0), price(100, 0)];
        let equal_threshold = [price(150, 0), price(150, 0), price(150, 0)];
        let above_threshold = [price(150, 0), price(150, 0), price(151, 0)];

        assert!(!relative_sum_change_exceeds(
            previous.iter(),
            equal_threshold.iter(),
            dec("0.5"),
        ));
        assert!(relative_sum_change_exceeds(
            previous.iter(),
            above_threshold.iter(),
            dec("0.5"),
        ));
    }

    #[test]
    fn hal_19_extreme_window_values_fail_closed_without_aliasing() {
        let previous = [price(1, -115), price(2, 0)];
        let current = [price(1, -115), price(3, 0)];

        assert!(relative_sum_change_exceeds(
            previous.iter(),
            current.iter(),
            dec("0.1"),
        ));
    }

    #[test]
    fn relative_comparison_overflow_fails_closed() {
        assert!(relative_abs_change_exceeds(
            &price(1, -116),
            &price(2, 0),
            dec("0.1"),
        ));
        assert!(relative_abs_change_exceeds(
            &price(2, 0),
            &price(3, 0),
            Decimal::MAX,
        ));
    }
}
