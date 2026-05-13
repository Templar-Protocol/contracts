use core::cmp::Ordering;

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

    if relative_abs_change_exceeds(previous, current, min_relative_step_change) {
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
    relative_abs_change(previous, current).is_some_and(|change| change > max_relative_change)
}

pub(super) fn relative_signed_change(first: &Price, last: &Price) -> Option<SignedDecimal> {
    match compare_scaled(first.price, first.expo, last.price, last.expo) {
        Ordering::Equal => Some(SignedDecimal::Positive(Decimal::ZERO)),
        Ordering::Less => Some(SignedDecimal::Positive(relative_abs_change(first, last)?)),
        Ordering::Greater => Some(SignedDecimal::Negative(relative_abs_change(first, last)?)),
    }
}

fn relative_abs_change(first: &Price, last: &Price) -> Option<Decimal> {
    let first_abs = first.price.unsigned_abs();
    if first_abs == 0 {
        return Some(if last.price == 0 {
            Decimal::ZERO
        } else {
            Decimal::MAX
        });
    }

    let last_abs = last.price.unsigned_abs();
    let same_sign = first.price.signum() == last.price.signum();
    let ratio = ratio_to_decimal(last_abs, last.expo, first_abs, first.expo)?;

    Some(if same_sign {
        // `ratio` is `last_abs / first_abs`; distance from 1.0 is the relative move.
        // For example, both 1.20 and 0.80 are 20% moves from the first value.
        ratio.abs_diff(Decimal::ONE)
    } else {
        // Crossing zero traverses the full previous magnitude plus the new opposite magnitude.
        saturating_add_one(ratio)
    })
}

fn ratio_to_decimal(
    numerator: u64,
    numerator_exponent: i32,
    denominator: u64,
    denominator_exponent: i32,
) -> Option<Decimal> {
    if denominator == 0 {
        return None;
    }

    let ratio = Decimal::from(numerator) / denominator;
    let Some(exponent_delta) = numerator_exponent.checked_sub(denominator_exponent) else {
        return Some(if numerator_exponent > denominator_exponent {
            Decimal::MAX
        } else {
            Decimal::MIN
        });
    };

    Some(scale_decimal(ratio, exponent_delta))
}

fn scale_decimal(value: Decimal, exponent: i32) -> Decimal {
    value.mul_pow10(exponent).unwrap_or_else(|| {
        if exponent.is_negative() {
            Decimal::MIN
        } else {
            Decimal::MAX
        }
    })
}

fn saturating_add_one(value: Decimal) -> Decimal {
    saturating_add(Decimal::ONE, value)
}

fn saturating_add(left: Decimal, right: Decimal) -> Decimal {
    if right > Decimal::MAX - left {
        Decimal::MAX
    } else {
        left + right
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SignedDecimal {
    Negative(Decimal),
    Positive(Decimal),
}

impl SignedDecimal {
    pub(super) fn abs_diff(self, other: Self) -> Decimal {
        match (self, other) {
            (Self::Positive(left), Self::Positive(right))
            | (Self::Negative(left), Self::Negative(right)) => left.abs_diff(right),
            (Self::Positive(left), Self::Negative(right))
            | (Self::Negative(left), Self::Positive(right)) => saturating_add(left, right),
        }
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

    fn assert_decimal_near(actual: Decimal, expected: Decimal) {
        assert!(
            actual.near_equal(expected),
            "actual={actual:?}, expected={expected:?}"
        );
    }

    fn assert_option_decimal_near(actual: Option<Decimal>, expected: Option<Decimal>) {
        match (actual, expected) {
            (Some(actual), Some(expected)) => assert_decimal_near(actual, expected),
            (actual, expected) => assert_eq!(actual, expected),
        }
    }

    fn assert_signed_decimal_near(actual: Option<SignedDecimal>, expected: Option<SignedDecimal>) {
        match (actual, expected) {
            (Some(SignedDecimal::Positive(actual)), Some(SignedDecimal::Positive(expected)))
            | (Some(SignedDecimal::Negative(actual)), Some(SignedDecimal::Negative(expected))) => {
                assert_decimal_near(actual, expected);
            }
            (actual, expected) => assert_eq!(actual, expected),
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

    #[rstest::rstest]
    #[case(
        price(100, 0),
        price(120, 0),
        Some(SignedDecimal::Positive(dec("0.2")))
    )]
    #[case(price(100, 0), price(80, 0), Some(SignedDecimal::Negative(dec("0.2"))))]
    #[case(price(1, -3), price(10, -4), Some(SignedDecimal::Positive(Decimal::ZERO)))]
    #[case(price(100, 0), price(-20, 0), Some(SignedDecimal::Negative(dec("1.2"))))]
    #[case(price(-100, 0), price(20, 0), Some(SignedDecimal::Positive(dec("1.2"))))]
    #[case(price(0, 0), price(0, 0), Some(SignedDecimal::Positive(Decimal::ZERO)))]
    #[case(price(0, 0), price(20, 0), Some(SignedDecimal::Positive(Decimal::MAX)))]
    fn relative_signed_change_keeps_direction(
        #[case] first: Price,
        #[case] last: Price,
        #[case] expected: Option<SignedDecimal>,
    ) {
        assert_signed_decimal_near(relative_signed_change(&first, &last), expected);
    }

    #[rstest::rstest]
    #[case(price(100, 0), price(120, 0), Some(dec("0.2")))]
    #[case(price(100, 0), price(80, 0), Some(dec("0.2")))]
    #[case(price(100, 0), price(-20, 0), Some(dec("1.2")))]
    #[case(price(0, 0), price(0, 0), Some(Decimal::ZERO))]
    #[case(price(0, 0), price(20, 0), Some(Decimal::MAX))]
    fn relative_abs_change_returns_magnitude_only(
        #[case] first: Price,
        #[case] last: Price,
        #[case] expected: Option<Decimal>,
    ) {
        assert_option_decimal_near(relative_abs_change(&first, &last), expected);
    }

    #[rstest::rstest]
    #[case(120, 0, 100, 0, Some(dec("1.2")))]
    #[case(1, -3, 10, -4, Some(Decimal::ONE))]
    #[case(1, 0, 0, 0, None)]
    #[case(1, i32::MAX, 1, i32::MIN, Some(Decimal::MAX))]
    #[case(1, i32::MIN, 1, i32::MAX, Some(Decimal::MIN))]
    fn ratio_to_decimal_scales_exponents(
        #[case] numerator: u64,
        #[case] numerator_exponent: i32,
        #[case] denominator: u64,
        #[case] denominator_exponent: i32,
        #[case] expected: Option<Decimal>,
    ) {
        assert_option_decimal_near(
            ratio_to_decimal(
                numerator,
                numerator_exponent,
                denominator,
                denominator_exponent,
            ),
            expected,
        );
    }

    #[rstest::rstest]
    #[case(Decimal::from(12_u64), 1, Decimal::from(120_u64))]
    #[case(dec("1.2"), -1, dec("0.12"))]
    #[case(Decimal::ONE, 1_000, Decimal::MAX)]
    #[case(Decimal::ONE, -1_000, Decimal::MIN)]
    fn scale_decimal_saturates_when_pow10_is_out_of_range(
        #[case] value: Decimal,
        #[case] exponent: i32,
        #[case] expected: Decimal,
    ) {
        assert_decimal_near(scale_decimal(value, exponent), expected);
    }

    #[rstest::rstest]
    #[case(dec("0.2"), dec("1.2"))]
    #[case(Decimal::MAX, Decimal::MAX)]
    fn saturating_add_one_saturates_at_decimal_max(
        #[case] value: Decimal,
        #[case] expected: Decimal,
    ) {
        assert_eq!(saturating_add_one(value), expected);
    }

    #[rstest::rstest]
    #[case(dec("0.2"), dec("0.3"), dec("0.5"))]
    #[case(Decimal::MAX, Decimal::ONE, Decimal::MAX)]
    fn saturating_add_saturates_at_decimal_max(
        #[case] left: Decimal,
        #[case] right: Decimal,
        #[case] expected: Decimal,
    ) {
        assert_eq!(saturating_add(left, right), expected);
    }

    #[rstest::rstest]
    #[case(
        SignedDecimal::Positive(dec("0.4")),
        SignedDecimal::Positive(dec("0.1")),
        dec("0.3")
    )]
    #[case(
        SignedDecimal::Negative(dec("0.4")),
        SignedDecimal::Negative(dec("0.1")),
        dec("0.3")
    )]
    #[case(
        SignedDecimal::Positive(dec("0.4")),
        SignedDecimal::Negative(dec("0.1")),
        dec("0.5")
    )]
    #[case(
        SignedDecimal::Positive(Decimal::MAX),
        SignedDecimal::Negative(Decimal::ONE),
        Decimal::MAX
    )]
    fn signed_decimal_abs_diff_accounts_for_sign(
        #[case] left: SignedDecimal,
        #[case] right: SignedDecimal,
        #[case] expected: Decimal,
    ) {
        assert_eq!(left.abs_diff(right), expected);
    }
}
