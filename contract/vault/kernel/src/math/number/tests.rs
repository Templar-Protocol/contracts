use super::*;
use proptest::prelude::*;

fn expected_floor(x: u128, y: u128, denom: u128) -> U256 {
    let prod = U512::from(x) * U512::from(y);
    let q = prod / U512::from(denom);
    Number::as_u256_trunc(q)
}

fn expected_ceil(x: u128, y: u128, denom: u128) -> U256 {
    let prod = U512::from(x) * U512::from(y);
    let d = U512::from(denom);
    let q = prod / d;
    let r = prod % d;
    let q = if r.is_zero() { q } else { q + U512::from(1u8) };
    Number::as_u256_trunc(q)
}

proptest! {
    #[test]
    fn mul_div_floor_matches_u512(
        x in any::<u128>(),
        y in any::<u128>(),
        denom in 1u128..=u128::MAX,
    ) {
        let floor = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(denom));
        let expected = expected_floor(x, y, denom);
        prop_assert_eq!(floor.0, expected);
    }

    #[test]
    fn mul_div_ceil_matches_u512(
        x in any::<u128>(),
        y in any::<u128>(),
        denom in 1u128..=u128::MAX,
    ) {
        let ceil = Number::mul_div_ceil(Number::from(x), Number::from(y), Number::from(denom));
        let expected = expected_ceil(x, y, denom);
        prop_assert_eq!(ceil.0, expected);
    }

    #[test]
    fn mul_div_zero_denom_is_zero(x in any::<u128>(), y in any::<u128>()) {
        let floor = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(0u128));
        let ceil = Number::mul_div_ceil(Number::from(x), Number::from(y), Number::from(0u128));
        prop_assert!(floor.is_zero());
        prop_assert!(ceil.is_zero());
    }

    // ===================================================================
    // Property: mul_div_floor <= mul_div_ceil (floor never exceeds ceil)
    // Invariant: For all x, y, denom > 0: floor(x*y/d) <= ceil(x*y/d)
    // ===================================================================
    #[test]
    fn mul_div_floor_leq_ceil(
        x in any::<u128>(),
        y in any::<u128>(),
        denom in 1u128..=u128::MAX,
    ) {
        let floor = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(denom));
        let ceil = Number::mul_div_ceil(Number::from(x), Number::from(y), Number::from(denom));
        prop_assert!(floor.0 <= ceil.0, "floor {} > ceil {}", floor.0, ceil.0);
    }

    #[test]
    fn mul_div_ceil_floor_diff_at_most_one(
        x in any::<u128>(),
        y in any::<u128>(),
        denom in 1u128..=u128::MAX,
    ) {
        let floor = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(denom));
        let ceil = Number::mul_div_ceil(Number::from(x), Number::from(y), Number::from(denom));
        let diff = ceil.0.saturating_sub(floor.0);
        prop_assert!(diff <= U256::one(), "diff {} > 1", diff);
    }

    #[test]
    fn mul_div_floor_commutative(
        x in any::<u128>(),
        y in any::<u128>(),
        denom in 1u128..=u128::MAX,
    ) {
        let result1 = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(denom));
        let result2 = Number::mul_div_floor(Number::from(y), Number::from(x), Number::from(denom));
        prop_assert_eq!(result1.0, result2.0);
    }

    #[test]
    fn mul_div_ceil_commutative(
        x in any::<u128>(),
        y in any::<u128>(),
        denom in 1u128..=u128::MAX,
    ) {
        let result1 = Number::mul_div_ceil(Number::from(x), Number::from(y), Number::from(denom));
        let result2 = Number::mul_div_ceil(Number::from(y), Number::from(x), Number::from(denom));
        prop_assert_eq!(result1.0, result2.0);
    }

    #[test]
    fn mul_div_floor_identity_denom_one(
        x in 0u128..=u64::MAX as u128,
        y in 0u128..=u64::MAX as u128,
    ) {
        let result = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(1u128));
        let expected = U256::from(x) * U256::from(y);
        prop_assert_eq!(result.0, expected);
    }

    #[test]
    fn mul_div_floor_zero_factor(
        x in any::<u128>(),
        y in any::<u128>(),
        denom in 1u128..=u128::MAX,
    ) {
        let r1 = Number::mul_div_floor(Number::zero(), Number::from(y), Number::from(denom));
        let r2 = Number::mul_div_floor(Number::from(x), Number::zero(), Number::from(denom));
        prop_assert!(r1.is_zero());
        prop_assert!(r2.is_zero());
    }

    #[test]
    fn mul_div_floor_self_division(
        x in any::<u128>(),
        denom in 1u128..=u128::MAX,
    ) {
        let result = Number::mul_div_floor(Number::from(x), Number::from(denom), Number::from(denom));
        prop_assert_eq!(result.0, U256::from(x));
    }

    #[test]
    fn saturating_add_no_overflow(a in any::<u128>(), b in any::<u128>()) {
        let na = Number::from(a);
        let nb = Number::from(b);
        let result = na.saturating_add(nb);
        prop_assert!(result.0 >= na.0, "saturating_add decreased value");
    }

    #[test]
    fn saturating_sub_no_underflow(a in any::<u128>(), b in any::<u128>()) {
        let na = Number::from(a);
        let nb = Number::from(b);
        let result = na.saturating_sub(nb);
        prop_assert!(result.0 <= na.0, "saturating_sub increased value");
    }

    #[test]
    fn saturating_add_commutative(a in any::<u128>(), b in any::<u128>()) {
        let na = Number::from(a);
        let nb = Number::from(b);
        let r1 = na.saturating_add(nb);
        let r2 = nb.saturating_add(na);
        prop_assert_eq!(r1.0, r2.0);
    }

    #[test]
    fn saturating_add_identity(a in any::<u128>()) {
        let na = Number::from(a);
        let result = na.saturating_add(Number::zero());
        prop_assert_eq!(result.0, na.0);
    }

    #[test]
    fn saturating_sub_identity(a in any::<u128>()) {
        let na = Number::from(a);
        let result = na.saturating_sub(Number::zero());
        prop_assert_eq!(result.0, na.0);
    }

    #[test]
    fn saturating_sub_self_is_zero(a in any::<u128>()) {
        let na = Number::from(a);
        let result = na.saturating_sub(na);
        prop_assert!(result.is_zero());
    }

    #[test]
    fn as_u128_trunc_roundtrip(x in any::<u128>()) {
        let n = Number::from(x);
        let back = n.as_u128_trunc();
        prop_assert_eq!(back, x);
    }

    #[test]
    fn as_u128_saturating_small_values(x in any::<u128>()) {
        let n = Number::from(x);
        let sat = n.as_u128_saturating();
        prop_assert_eq!(sat, x);
    }

    #[test]
    fn mul_div_floor_monotonic_in_x(
        x1 in any::<u128>(),
        x2 in any::<u128>(),
        y in any::<u128>(),
        denom in 1u128..=u128::MAX,
    ) {
        let (lo, hi) = if x1 <= x2 { (x1, x2) } else { (x2, x1) };
        let r_lo = Number::mul_div_floor(Number::from(lo), Number::from(y), Number::from(denom));
        let r_hi = Number::mul_div_floor(Number::from(hi), Number::from(y), Number::from(denom));
        prop_assert!(r_lo.0 <= r_hi.0, "not monotonic: {} > {}", r_lo.0, r_hi.0);
    }

    #[test]
    fn mul_div_floor_monotonic_in_y(
        x in any::<u128>(),
        y1 in any::<u128>(),
        y2 in any::<u128>(),
        denom in 1u128..=u128::MAX,
    ) {
        let (lo, hi) = if y1 <= y2 { (y1, y2) } else { (y2, y1) };
        let r_lo = Number::mul_div_floor(Number::from(x), Number::from(lo), Number::from(denom));
        let r_hi = Number::mul_div_floor(Number::from(x), Number::from(hi), Number::from(denom));
        prop_assert!(r_lo.0 <= r_hi.0, "not monotonic: {} > {}", r_lo.0, r_hi.0);
    }

    #[test]
    fn mul_div_floor_antimonotonic_in_denom(
        x in any::<u128>(),
        y in any::<u128>(),
        d1 in 1u128..=u128::MAX,
        d2 in 1u128..=u128::MAX,
    ) {
        let (lo_d, hi_d) = if d1 <= d2 { (d1, d2) } else { (d2, d1) };
        let r_lo_d = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(lo_d));
        let r_hi_d = Number::mul_div_floor(Number::from(x), Number::from(y), Number::from(hi_d));
        // Smaller denominator => larger result
        prop_assert!(r_lo_d.0 >= r_hi_d.0, "denom monotonicity violated: {} < {}", r_lo_d.0, r_hi_d.0);
    }
}

#[test]
fn number_constants() {
    assert!(Number::ZERO.is_zero());
    assert!(Number::ONE.is_one());
    assert!(Number::zero().is_zero());
    assert!(Number::one().is_one());
}

#[test]
fn number_from_u128_into_u128() {
    let val: u128 = 123456789;
    let n = Number::from(val);
    let back: u128 = n.into();
    assert_eq!(back, val);
}

#[test]
fn number_div_by_u128() {
    let n = Number::from(100u128);
    let result = n / 10u128;
    assert_eq!(u128::from(result), 10);
}

#[test]
fn number_div_by_u256() {
    let n = Number::from(100u128);
    let result = n / U256::from(5u128);
    assert_eq!(u128::from(result), 20);
}

#[test]
fn number_div_by_number() {
    let a = Number::from(100u128);
    let b = Number::from(4u128);
    let result = a / b;
    assert_eq!(u128::from(result), 25);
}

#[test]
fn number_add() {
    let a = Number::from(50u128);
    let b = Number::from(30u128);
    let result = a + b;
    assert_eq!(u128::from(result), 80);
}

#[test]
fn number_sub() {
    let a = Number::from(100u128);
    let b = Number::from(40u128);
    let result = a - b;
    assert_eq!(u128::from(result), 60);
}

#[test]
fn number_from_into_u256() {
    let u = U256::from(999u128);
    let n: Number = u.into();
    let back: U256 = n.into();
    assert_eq!(back, u);
}

#[test]
fn as_u128_saturating_large_value() {
    // Create a Number that exceeds u128::MAX
    let large = Number(U256::from(u128::MAX) + U256::from(1u128));
    assert_eq!(large.as_u128_saturating(), u128::MAX);
}

#[test]
fn number_is_zero_is_one() {
    let zero = Number::from(0u128);
    let one = Number::from(1u128);
    let two = Number::from(2u128);

    assert!(zero.is_zero());
    assert!(!zero.is_one());

    assert!(!one.is_zero());
    assert!(one.is_one());

    assert!(!two.is_zero());
    assert!(!two.is_one());
}

#[cfg(feature = "postcard")]
#[test]
fn postcard_roundtrip_small_number() {
    let number = Number::from(123_456_789u128);
    let bytes = postcard::to_allocvec(&number).expect("serialize number");
    let decoded: Number = postcard::from_bytes(&bytes).expect("deserialize number");
    assert_eq!(decoded, number);
}

#[cfg(all(feature = "postcard", feature = "soroban"))]
#[test]
fn soroban_postcard_uses_compact_u128_encoding() {
    let number = Number::from(7u128);
    let bytes = postcard::to_allocvec(&number).expect("serialize number");
    assert!(
        bytes.len() < 32,
        "expected compact u128 encoding, got {} bytes",
        bytes.len()
    );
}

#[cfg(all(feature = "postcard", feature = "soroban"))]
#[test]
fn soroban_postcard_rejects_large_number_on_serialize() {
    let large = Number(U256::from(u128::MAX) + U256::from(99u128));
    assert!(postcard::to_allocvec(&large).is_err());
}

#[cfg(all(feature = "postcard", not(feature = "soroban")))]
#[test]
fn non_soroban_postcard_keeps_32_byte_payload() {
    let number = Number::from(7u128);
    let bytes = postcard::to_allocvec(&number).expect("serialize number");
    assert_eq!(bytes.len(), 33, "expected 1-byte length + 32-byte payload");
}
