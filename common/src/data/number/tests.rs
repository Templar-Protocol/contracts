use near_sdk::serde_json;
use primitive_types::U256;
use rand::Rng;
use rstest::rstest;

use super::*;

// These functions are intentionally implemented using mathematical
// operations instead of bitwise operations, so as to test the
// correctness of the mathematical operators.

fn with_upper_u128(n: u128) -> Decimal {
    let mut d = Decimal::from(n);
    d *= Decimal::from(u128::pow(2, 64));
    d *= Decimal::from(u128::pow(2, 64));
    d
}

fn get_upper_u128(mut d: Decimal) -> u128 {
    d /= Decimal::from(u128::pow(2, 64));
    d /= Decimal::from(u128::pow(2, 64));
    d.to_u128_floor().unwrap()
}

#[rstest]
#[case(0, 0)]
#[case(0, 1)]
#[case(1, 0)]
#[case(1, 1)]
#[case(2_934_570_000_008_u128, 9_595_959_283_u128)]
#[case(u128::MAX, 0)]
#[case(0, u128::MAX)]
#[test]
fn addition(#[case] a: u128, #[case] b: u128) {
    assert_eq!(Decimal::from(a) + Decimal::from(b), a + b);
    assert_eq!(
        get_upper_u128(with_upper_u128(a) + with_upper_u128(b)),
        a + b,
    );
}

#[rstest]
#[case(0, 0)]
#[case(1, 0)]
#[case(1, 1)]
#[case(2_934_570_000_008_u128, 9_595_959_283_u128)]
#[case(u128::MAX, 0)]
#[case(u128::MAX, 1)]
#[case(u128::MAX, u128::MAX / 2)]
#[case(u128::MAX, u128::MAX)]
#[test]
fn subtraction(#[case] a: u128, #[case] b: u128) {
    assert_eq!(Decimal::from(a) - Decimal::from(b), a - b);
    assert_eq!(
        get_upper_u128(with_upper_u128(a) - with_upper_u128(b)),
        a - b,
    );
}

#[rstest]
#[case(0, 0)]
#[case(0, 1)]
#[case(1, 0)]
#[case(1, 1)]
#[case(2, 2)]
#[case(u128::MAX, 0)]
#[case(u128::MAX, 1)]
#[case(0, u128::MAX)]
#[case(1, u128::MAX)]
#[test]
fn multiplication(#[case] a: u128, #[case] b: u128) {
    assert_eq!(Decimal::from(a) * Decimal::from(b), a * b);
    assert_eq!(get_upper_u128(with_upper_u128(a) * b), a * b);
    assert_eq!(get_upper_u128(a * with_upper_u128(b)), a * b);
}

#[rstest]
#[case(0, 1)]
#[case(1, 1)]
#[case(1, 2)]
#[case(u128::MAX, u128::MAX)]
#[case(u128::MAX, 1)]
#[case(0, u128::MAX)]
#[case(1, u128::MAX)]
#[case(1, 10)]
#[case(3, 10_000)]
#[test]
fn division(#[case] a: u128, #[case] b: u128) {
    #[allow(clippy::cast_precision_loss)]
    let quotient = a as f64 / b as f64;
    let abs_difference_lte = |d: Decimal, f: f64| (d.to_f64_lossy() - f).abs() <= 1e-200;
    assert!(abs_difference_lte(
        Decimal::from(a) / Decimal::from(b),
        quotient,
    ));
    assert!(abs_difference_lte(
        with_upper_u128(a) / with_upper_u128(b),
        quotient,
    ));
}

#[rstest]
#[case(12, 2)]
#[case(2, 32)]
#[case(1, 0)]
#[case(0, 0)]
#[case(0, 1)]
#[case(1, 1)]
#[test]
fn power(#[case] x: u128, #[case] n: u32) {
    #[allow(clippy::cast_possible_wrap)]
    let n_i32 = n as i32;
    assert_eq!(Decimal::from(x).pow(n_i32), Decimal::from(x.pow(n)));
}

#[test]
#[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
fn pow10_valid_range() {
    assert_eq!(
        Decimal::ONE.mul_pow10(-(FRACTIONAL_DECIMAL_DIGITS as i32) - 1),
        None,
    );
    for i in -(FRACTIONAL_DECIMAL_DIGITS as i32)..=(WHOLE_DECIMAL_DIGITS as i32) {
        eprintln!("10^{i} = {:?}", Decimal::ONE.mul_pow10(i).unwrap());
    }
    assert_eq!(
        Decimal::ONE.mul_pow10((WHOLE_DECIMAL_DIGITS as i32) + 1),
        None,
    );
}

#[rstest]
#[case(0, 0)]
#[case(0, 1)]
#[case(0, -1)]
#[case(1, 0)]
#[case(1, 1)]
#[case(1, -1)]
#[case(1, i32::try_from(WHOLE_DECIMAL_DIGITS).unwrap())]
#[case(1, i32::try_from(FRACTIONAL_DECIMAL_DIGITS).unwrap())]
#[case(1, -i32::try_from(FRACTIONAL_DECIMAL_DIGITS).unwrap())]
#[case(12, 20)]
#[case(12, 0)]
#[case(12, -20)]
#[case(u128::MAX, 0)]
#[case(u128::MAX, -20)]
#[test]
fn mul_pow10(#[case] x: u128, #[case] n: i32) {
    #[allow(clippy::cast_sign_loss)]
    if n >= 0 {
        assert_eq!(
            Decimal::from(x).mul_pow10(n).unwrap(),
            Decimal::from(x) * Decimal::from(10u32).pow(n),
        );
    } else {
        assert!(Decimal::from(x)
            .mul_pow10(n)
            .unwrap()
            .near_equal(Decimal::from(x) / U256::exp10(-n as usize)));
    }
}

#[test]
fn constants_are_accurate() {
    assert_eq!(Decimal::ZERO.to_u128_floor().unwrap(), 0);
    assert!((Decimal::ONE_HALF.to_f64_lossy() - 0.5_f64).abs() < 1e-200);
    assert_eq!(Decimal::ONE.to_u128_floor().unwrap(), 1);
    assert_eq!(Decimal::TWO.to_u128_floor().unwrap(), 2);
}

#[rstest]
#[case(Decimal::ONE, 0)]
#[case(Decimal::ONE_HALF, 1u128 << 127)]
#[test]
fn get_fractional_dividend(#[case] value: Decimal, #[case] expected: u128) {
    assert_eq!(value.fractional_part_as_u128_dividend(), expected);
}

#[rstest]
#[case(Decimal::ONE)]
#[case(Decimal::TWO)]
#[case(Decimal::ZERO)]
#[case(Decimal::ONE_HALF)]
#[case(Decimal::from(u128::MAX))]
#[case(Decimal::from(u64::MAX) / Decimal::from(u128::MAX))]
#[test]
fn serialization(#[case] value: Decimal) {
    let serialized = serde_json::to_string(&value).unwrap();
    let deserialized: Decimal = serde_json::from_str(&serialized).unwrap();

    assert!(value.near_equal(deserialized));
}

#[test]
fn from_self_string_serialization_precision() {
    const ITERATIONS: usize = 1_024;
    const TRANSFORMATIONS: usize = 16;

    let mut rng = rand::thread_rng();

    let mut max_error = U512::zero();
    let mut error_distribution = [0u32; 16];
    let mut value_with_max_error = Decimal::ZERO;

    #[allow(clippy::cast_possible_truncation)]
    for _ in 0..ITERATIONS {
        let actual = Decimal {
            repr: U512(rng.gen()),
        };

        let mut s = actual.to_fixed(FRACTIONAL_DECIMAL_DIGITS);
        for _ in 0..(TRANSFORMATIONS - 1) {
            s = Decimal::from_str(&s)
                .unwrap()
                .to_fixed(FRACTIONAL_DECIMAL_DIGITS);
        }
        let parsed = Decimal::from_str(&s).unwrap();

        let e = actual.abs_diff(parsed).repr;

        if e > max_error {
            max_error = e;
            value_with_max_error = actual;
        }

        error_distribution[e.0[0] as usize] += 1;
    }

    println!("Error distribution:");
    for (i, x) in error_distribution.iter().enumerate() {
        println!("\t{i}: {x:b}");
    }
    println!("Max error: {:?}", max_error.0);

    assert!(
        max_error <= Decimal::REPR_EPSILON,
        "Stringification error of repr {:?} is repr {:?}",
        value_with_max_error.repr.0,
        max_error.0,
    );
}

#[test]
#[allow(clippy::cast_precision_loss)]
fn from_f64_string_serialization_precision() {
    const ITERATIONS: usize = 10_000;
    let mut rng = rand::thread_rng();
    let epsilon = Decimal {
        repr: Decimal::REPR_EPSILON,
    }
    .to_f64_lossy();

    let t = |f: f64| {
        let actual = f.abs();
        let string = actual.to_string();
        let parsed = Decimal::from_str(&string).unwrap();

        let e = (parsed.to_f64_lossy() - actual).abs();

        assert!(e <= epsilon, "Stringification error of f64 {actual} is {e}");
    };

    for _ in 0..ITERATIONS {
        t(rng.gen::<f64>() * rng.gen::<u128>() as f64);
    }
}

#[test]
fn round_up_repr() {
    let cases = [
        Decimal {
            #[rustfmt::skip]
                repr: U512([ 0x0966_4E4C_9169_501F, 0xB226_2812_5CF2_3CD0, 1, 0, 0, 0, 0, 0 ]),
        },
        Decimal {
            repr: U512([u64::MAX, u64::MAX, 1, 0, 0, 0, 0, 0]),
            // 1.99999999999999999999999999999999999999706126412294428123007815865694438580...
        },
        Decimal {
            repr: U512([u64::MAX - 1, u64::MAX, 1, 0, 0, 0, 0, 0]),
        },
        Decimal { repr: U512::MAX },
        Decimal {
            repr: U512::MAX.saturating_sub(U512::one()),
        },
        Decimal { repr: U512::zero() },
    ];

    for case in cases {
        let p: Decimal = case.to_fixed(FRACTIONAL_DECIMAL_DIGITS).parse().unwrap();

        eprintln!("{:x?}", case.repr.0);
        eprintln!("{:x?}", p.repr.0);
        eprintln!("|{p:?} - {case:?}| = {:?}", p.abs_diff(case).as_repr());

        assert!(p.near_equal(case));
    }
}

#[test]
fn round_up_str() {
    // Cases that are (generally) not evenly representable in binary fraction.
    let cases = [
        "1",
        "0",
        "1.6958947224456518",
        "2.79",
        "0.6",
        "10.6",
        "0.01",
        "0.599999999999999999999999999999999999",
    ];
    for case in cases {
        println!("Testing {case}...");
        let n = Decimal::from_str(case).unwrap();
        let s = n.to_fixed(FRACTIONAL_DECIMAL_DIGITS);
        let parsed = Decimal::from_str(&s).unwrap();
        assert_eq!(n, parsed);
        println!("{n:?}");
        println!();
    }
}
