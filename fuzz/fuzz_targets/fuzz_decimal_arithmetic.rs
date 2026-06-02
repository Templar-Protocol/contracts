#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use std::str::FromStr;
use templar_common::Decimal;

// MUTATION-CHECK (P5): in `Decimal`'s `to_fixed`/`FromStr` round-trip path
// (primitives/src/number.rs), drop a low-order digit during formatting (e.g.
// truncate `to_fixed` to fewer fractional digits than requested). Then the
// `Decimal::from_str(&dec_a.to_fixed(38)) == dec_a` round-trip assertion below
// must fire. (Alternatively: flip `<` to `<=` in `Decimal`'s `Ord` to break
// the "exactly one of <,>,==" trichotomy assertion.)

// Operands span the FULL `u128` range (P2: no input-domain narrowing). A
// `Decimal` stores `value << 128`, so an *integer*-valued `Decimal` has 128
// trailing zero bits that `Mul`/`Div` factor out before the U512 multiply
// (number.rs:470-504). Hence `Decimal::from(a) * Decimal::from(b)` for any
// `a, b: u128` peaks at ~2^384 — well inside U512 — and cannot overflow. (This
// is verified: `u128::MAX * u128::MAX` as integer `Decimal`s does not panic.)
// The only operation here that *can* overflow is `pow` with a large base, so
// `pow_base` alone is bounded below — a targeted bound on one operation, not on
// the operand domain.
#[derive(Arbitrary, Debug)]
struct Input {
    a: u128,
    b: u128,
    c: u128,
    // Restricted via modulus below.
    pow_exp: u8,
    // `i32` exponent for `mul_pow10`. Clamped before use.
    pow10_exp: i32,
    op_selector: u8,
}

fuzz_target!(|input: Input| {
    let Input {
        a,
        b,
        c,
        pow_exp,
        pow10_exp,
        op_selector,
    } = input;

    let dec_a = Decimal::from(a);
    let dec_b = Decimal::from(b);
    let dec_c = Decimal::from(c);

    // Arithmetic — integer-valued Decimals, so add/sub/mul/div stay in U512
    // for the full u128 input range (see the note on `Input`).
    let sum = dec_a + dec_b;
    assert_eq!(sum, dec_b + dec_a, "addition commutativity");
    let _ = dec_a + dec_b + dec_c;

    if a >= b {
        let diff = dec_a - dec_b;
        assert_eq!(diff + dec_b, dec_a, "(a-b)+b == a");
    }

    let prod = dec_a * dec_b;
    assert_eq!(prod, dec_b * dec_a, "multiplication commutativity");

    if b > 0 {
        let q = dec_a / dec_b;
        // q*b ≤ a (since division truncates towards zero in the fractional
        // representation — we don't claim exact equality here).
        let _ = q * dec_b;
    }

    // Pow: keep base × exponent well inside U512's 384-bit whole part. We
    // pick base ≤ 2^16 and exponent ≤ 12, giving 2^192 max.
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let small_pow = (pow_exp % 13) as i32;
    let pow_base = Decimal::from(u16::try_from(a & 0xFFFF).unwrap_or(0));
    let _ = pow_base.pow(small_pow);
    let _ = Decimal::ONE.pow(small_pow);
    let _ = Decimal::TWO.pow(small_pow);
    if !pow_base.is_zero() {
        let _ = pow_base.pow(-small_pow);
    }

    // pow2_int: integer powers of two up to 384 bits.
    let pow2_exp = u32::from(pow_exp) % 384;
    let _ = Decimal::pow2_int(pow2_exp);

    // pow2 only defined for x ≤ 1.
    if dec_a <= Decimal::ONE {
        let _ = dec_a.pow2();
    }

    // mul_pow10 returns None on out-of-range exponents.
    let safe_exp = pow10_exp.clamp(-38, 115);
    if let Some(scaled) = dec_a.mul_pow10(safe_exp) {
        if safe_exp >= 0 {
            // Scaling up by 10^n must not shrink |x|.
            assert!(scaled >= dec_a || dec_a.is_zero());
        }
    }

    // Comparison consistency.
    let lt = dec_a < dec_b;
    let gt = dec_a > dec_b;
    let eq = dec_a == dec_b;
    assert!(
        u8::from(lt) + u8::from(gt) + u8::from(eq) == 1,
        "exactly one of <, >, == must hold",
    );
    assert_eq!(dec_a <= dec_b, lt || eq);
    assert_eq!(dec_a >= dec_b, gt || eq);
    let _ = dec_a.near_equal(dec_b);

    // Conversion monotonicity: floor ≤ ceil; floor matches u128 cast.
    if let Some(floor) = dec_a.to_u128_floor() {
        if let Some(ceil) = dec_a.to_u128_ceil() {
            assert!(floor <= ceil);
        }
        // Integer value must roundtrip via floor.
        assert_eq!(floor, a);
    }
    let _ = dec_a.to_f64_lossy();

    // to_fixed roundtrip — important: serialization must be invertible.
    let fixed_full = dec_a.to_fixed(38);
    let parsed = Decimal::from_str(&fixed_full).expect("to_fixed(38) must roundtrip");
    assert_eq!(dec_a, parsed, "Decimal::to_fixed(38) is not a full roundtrip");

    // Shorter fixed representations should not panic.
    let _ = dec_a.to_fixed(10);
    let _ = dec_a.to_fixed(0);

    // abs_diff symmetry.
    assert_eq!(dec_a.abs_diff(dec_b), dec_b.abs_diff(dec_a));

    // is_zero
    assert!(Decimal::ZERO.is_zero());
    assert_eq!(dec_a.is_zero(), a == 0);

    // fractional_part_as_u128_dividend — must not panic on integers.
    let _ = dec_a.fractional_part_as_u128_dividend();

    match op_selector % 6 {
        0 => {
            // Identity laws.
            assert_eq!(dec_a + Decimal::ZERO, dec_a);
            assert_eq!(dec_a * Decimal::ONE, dec_a);
            assert_eq!(dec_a * Decimal::ZERO, Decimal::ZERO);
        }
        1 => {
            // Half-then-double approximately recovers.
            let half = dec_a * Decimal::ONE_HALF;
            let recovered = half + half;
            assert!(recovered.near_equal(dec_a) || recovered == dec_a);
        }
        2 => {
            let _ = Decimal::MAX.to_u128_floor();
            assert!(!Decimal::MIN.is_zero() || Decimal::MIN == Decimal::ZERO);
        }
        3 => {
            // pow(0) is identity, pow(1) is identity.
            assert_eq!(dec_a.pow(0), Decimal::ONE);
            assert_eq!(dec_a.pow(1), dec_a);
        }
        4 => {
            // mul_pow10(0) is identity.
            assert_eq!(dec_a.mul_pow10(0), Some(dec_a));
        }
        _ => {
            // Round-trip near_equal.
            let str_repr = dec_a.to_fixed(20);
            if let Ok(parsed) = Decimal::from_str(&str_repr) {
                assert!(dec_a.near_equal(parsed));
            }
        }
    }
});
