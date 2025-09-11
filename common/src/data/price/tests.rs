use rstest::rstest;

use crate::dec;

use super::*;

#[test]
fn valuation_eq() {
    let o = Valuation::optimistic(
        1000u128.into(),
        &Price::<BorrowAsset> {
            _asset: PhantomData,
            price: 250,
            confidence: 12,
            exponent: -5,
        },
    );

    assert_eq!(o.coefficient, U256::from(1000 * (250 + 12)));
    assert_eq!(o.exponent, -5);

    let p = Valuation::pessimistic(
        1000u128.into(),
        &Price::<BorrowAsset> {
            _asset: PhantomData,
            price: 250,
            confidence: 12,
            exponent: -5,
        },
    );

    assert_eq!(p.coefficient, U256::from(1000 * (250 - 12)));
    assert_eq!(p.exponent, -5);
}

#[test]
fn valuation_ratio_equal() {
    let first = Valuation::optimistic(
        600u128.into(),
        &Price::<BorrowAsset> {
            _asset: PhantomData,
            price: 100,
            confidence: 0,
            exponent: 4,
        },
    );
    let second = Valuation::pessimistic(
        60u128.into(),
        &Price::<BorrowAsset> {
            _asset: PhantomData,
            price: 1000,
            confidence: 0,
            exponent: 4,
        },
    );

    assert_eq!(first.ratio(second).unwrap(), Decimal::ONE);
}

#[rstest]
#[case(8, 1, 8, 0,      dec!("1"))]
#[case(1, 25, 1, -2,    dec!("4"))]
#[case(0, 1, 1, 0,      dec!("0"))]
#[case(800, 2, 4, 2,    dec!("1"))]
#[case(u128::MAX, 1, 1, i32::MIN, Decimal::MAX)]
#[case(1, 1, 1, i32::MAX, Decimal::MIN)]
// The following case returns a power of 2. Whereas the *correct* answer is
// 1e+115, the approximation 2^382 is about 9.85e+114. Keep in mind Decimal
// only supports a total of 115 whole decimal digits.
#[case(u128::MAX, u128::MAX, 1, -115, Decimal::pow2_int(382).unwrap())]
#[case(1, 1, 1, 39, Decimal::ZERO)]
#[test]
fn valuation_ratios(
    #[case] value: u128,
    #[case] divisor_value: u128,
    #[case] divisor_price: u128,
    #[case] divisor_exponent: i32,
    #[case] expected_result: impl Into<Decimal>,
) {
    let dividend = Valuation::optimistic(
        value.into(),
        &Price::<BorrowAsset> {
            _asset: PhantomData,
            price: 1,
            confidence: 0,
            exponent: 0,
        },
    );

    let divisor = Valuation::optimistic(
        divisor_value.into(),
        &Price::<BorrowAsset> {
            _asset: PhantomData,
            price: divisor_price,
            confidence: 0,
            exponent: divisor_exponent,
        },
    );

    println!("{dividend:?}");
    println!("{divisor:?}");

    assert_eq!(dividend.ratio(divisor).unwrap(), expected_result.into());
}
