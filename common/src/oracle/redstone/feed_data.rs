use near_sdk::json_types::{I64, U64};
use primitive_types::U256;

use crate::oracle::{pyth, time::Milliseconds};

use super::SerializableU256;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[near_sdk::near(serializers = [json, borsh])]
pub struct FeedData {
    pub price: SerializableU256,
    /// Package timestamp in milliseconds since Unix epoch.
    pub package_timestamp: Milliseconds,
    /// Write timestamp in milliseconds since Unix epoch.
    pub write_timestamp: Milliseconds,
}

impl FeedData {
    /// Converts this [`FeedData`] to a [`pyth::Price`], with the confidence
    /// set to zero, because RedStone does not provide confidence intervals.
    pub fn to_pyth_price(&self) -> Option<pyth::Price> {
        let (price, exponent) = approximate_u256(self.price.into());
        Some(pyth::Price {
            price: I64(price),
            conf: U64(0),
            expo: exponent.checked_sub(super::DECIMALS)?,
            // Publish time is in seconds, but the RedStone data uses milliseconds.
            publish_time: self.package_timestamp.try_to_pyth()?,
        })
    }
}

/// Use instead of `U256::exp10` to avoid stack overflow for large exponents,
/// since `U256::exp10` uses linear-time recursion.
fn u256_exp10(mut exponent: u32) -> U256 {
    if exponent == 0 {
        return U256::one();
    }
    let mut y = U256::one();
    let mut x = U256::from(10);

    while exponent > 1 {
        if exponent % 2 == 1 {
            y *= x;
        }
        x *= x;
        exponent >>= 1;
    }

    x * y
}

/// Converts a [`U256`] to an `i64` mantissa and an `i32` exponent.
///
/// Rounds down (floor).
#[allow(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    reason = "guaranteed safe"
)]
fn approximate_u256(input: U256) -> (i64, i32) {
    const I64_MAX: U256 = U256([0x7FFF_FFFF_FFFF_FFFF, 0, 0, 0]);

    let mut n = input;
    let mut exponent = 0;

    if let Some(b) = n.bits().checked_sub(64) {
        // 103_873_643 / 345_060_773 ~= log(2)/log(10)
        let e = b * 103_873_643 / 345_060_773;
        let modulus = u256_exp10(e as u32);
        n /= modulus;
        exponent += e;
    }

    while n > I64_MAX {
        n /= 10;
        exponent += 1;
    }

    (n.low_u64() as i64, exponent as i32)
}

#[allow(clippy::cast_sign_loss)]
#[cfg(test)]
mod tests {
    use near_sdk::serde_json;

    use super::*;

    #[rstest::rstest]
    #[case::zero(U256::zero())]
    #[case::one(U256::one())]
    #[case::max(U256::MAX)]
    #[case::normal_fits_i64(U256::from(777_777_777_777_777_777_i64))]
    #[case::large_power_of_2(U256::from(2).pow(255.into()))]
    #[case::large_other(U256::from(123_945).pow(12.into()))]
    fn approximation(#[case] x: U256) {
        let (n, e) = approximate_u256(x);
        eprintln!("{n}*10^{e} ~= {x}");
        assert_eq!(U256::from(n), x / U256::exp10(e as usize));
    }

    #[rstest::rstest]
    fn approximation_exp10() {
        for i in 0..=77_u32 {
            let v = U256::exp10(i as usize);
            let (n, e) = approximate_u256(v);
            eprintln!("{i}:\t{n} * 10^{e}");
            assert_eq!(n.ilog10() + e as u32, i);
        }
    }

    #[test]
    fn json() {
        let fd = FeedData {
            price: U256::from(3333).into(),
            package_timestamp: Milliseconds::from_ms(5555),
            write_timestamp: Milliseconds::from_ms(6666),
        };

        let serialized = serde_json::to_string(&fd).unwrap();

        eprintln!("{serialized}");

        assert_eq!(
            serialized,
            r#"{"price":"3333","package_timestamp":"5555","write_timestamp":"6666"}"#,
        );

        let deserialized: FeedData = serde_json::from_str(&serialized).unwrap();

        assert_eq!(fd, deserialized);
    }
}
