//! Amounts as a spec writes them: a number and the unit it is stated in.
//!
//! The unit is mandatory. Whole units and base units differ by a factor of
//! `10^decimals` that varies per asset, so a bare number cannot be read without
//! knowing which the author meant, and reading it wrong is silent — three
//! deployed markets carry a floor ten times below the one they intended.

use std::fmt;
use std::str::FromStr;

use anyhow::Context as _;
use near_sdk::json_types::U64;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use templar_common::{
    asset::{AssetClass, FungibleAssetAmount},
    fee::{Fee, TimeBasedFee, TimeBasedFeeFunction},
    market::AmountRange,
    Decimal,
};

/// Both spellings of each unit parse; the plural is what this module writes.
/// Only `tokens` takes a fraction — an atom is indivisible, and a schema laxer
/// than the parser would pass a document the tool then refuses.
const PATTERN: &str = r"^([0-9]+ (atom|atoms)|[0-9]+(\.[0-9]+)? (token|tokens))$";

const UNIT_HELP: &str = "write `<amount> tokens` for whole units of the asset, or \
                         `<amount> atoms` for indivisible base units";

/// An amount and the unit it is stated in, scaled to base units only once the
/// asset's decimals are known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Amount {
    /// Indivisible base units — the encoding the chain stores.
    Atoms(u128),
    /// Whole units of the asset: `digits / 10^scale`, normalized on
    /// construction so that equal values compare equal.
    Tokens { digits: u128, scale: u8 },
}

impl Amount {
    /// Whole units, normalized.
    pub const fn tokens(digits: u128, scale: u8) -> Self {
        let (mut digits, mut scale) = (digits, scale);
        while scale > 0 && digits % 10 == 0 {
            digits /= 10;
            scale -= 1;
        }
        Self::Tokens { digits, scale }
    }

    /// The whole-unit form of an on-chain value. Always `Tokens`: both forms
    /// are lossless, so choosing between them by magnitude would be invented
    /// policy.
    pub const fn from_base_units(raw: u128, decimals: u8) -> Self {
        Self::tokens(raw, decimals)
    }

    /// The on-chain value, once the asset's decimals are known.
    pub fn to_base_units(self, decimals: u8) -> anyhow::Result<u128> {
        let (digits, scale) = match self {
            Self::Atoms(atoms) => return Ok(atoms),
            Self::Tokens { digits, scale } => (digits, scale),
        };
        anyhow::ensure!(
            scale <= decimals,
            "`{self}` states {scale} decimal places, but the asset has {decimals}"
        );
        10u128
            .checked_pow(u32::from(decimals - scale))
            .and_then(|factor| digits.checked_mul(factor))
            .with_context(|| format!("`{self}` does not fit in u128 at {decimals} decimals"))
    }
}

impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Atoms(atoms) => write!(f, "{atoms} atoms"),
            Self::Tokens { digits, scale: 0 } => write!(f, "{digits} tokens"),
            Self::Tokens { digits, scale } => {
                let scale = usize::from(scale);
                let text = format!("{digits:0>width$}", width = scale + 1);
                let point = text.len() - scale;
                write!(f, "{}.{} tokens", &text[..point], &text[point..])
            }
        }
    }
}

impl FromStr for Amount {
    type Err = anyhow::Error;

    fn from_str(text: &str) -> anyhow::Result<Self> {
        let text = text.trim();
        let (number, unit) = text
            .split_once(char::is_whitespace)
            .with_context(|| format!("`{text}` states no unit; {UNIT_HELP}"))?;
        let unit = unit.trim_start();

        match unit {
            "atom" | "atoms" => {
                anyhow::ensure!(
                    !number.contains('.'),
                    "`{text}` gives a fraction of an atom, which is the indivisible \
                     unit; state it in tokens instead"
                );
                digit_run(number, text)?;
                number
                    .parse()
                    .map(Self::Atoms)
                    .with_context(|| format!("`{text}` does not fit in u128"))
            }
            "token" | "tokens" => {
                let (whole, fraction) = match number.split_once('.') {
                    Some((whole, fraction)) => (whole, fraction),
                    None => (number, ""),
                };
                digit_run(whole, text)?;
                if number.contains('.') {
                    digit_run(fraction, text)?;
                }
                let scale = u8::try_from(fraction.len())
                    .with_context(|| format!("`{text}` states too many decimal places"))?;
                format!("{whole}{fraction}")
                    .parse()
                    .map(|digits| Self::tokens(digits, scale))
                    .with_context(|| format!("`{text}` does not fit in u128"))
            }
            other => anyhow::bail!("`{other}` is not a unit; {UNIT_HELP}"),
        }
    }
}

/// A non-empty run of ASCII digits, checked before parsing so that `+1` and
/// `1_000` are refused by name rather than by `u128`'s opaque parse error.
fn digit_run(part: &str, text: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()),
        "`{text}` is not a plain decimal number: digits, with at most one `.` \
         between them, and no sign, exponent or separators"
    );
    Ok(())
}

impl Serialize for Amount {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Amount {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl serde::de::Visitor<'_> for Visitor {
            type Value = Amount;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "an amount and its unit as a quoted string ({UNIT_HELP})")
            }

            fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<Amount, E> {
                text.parse()
                    .map_err(|error| E::custom(format!("{error:#}")))
            }
        }

        deserializer.deserialize_str(Visitor)
    }
}

impl JsonSchema for Amount {
    fn schema_name() -> String {
        "Amount".to_owned()
    }

    fn json_schema(generator: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        let mut schema = <String as JsonSchema>::json_schema(generator).into_object();
        schema.string().pattern = Some(PATTERN.to_owned());
        schema.metadata().description = Some(format!("An amount and its unit — {UNIT_HELP}."));
        schema.into()
    }
}

/// A spec's amount range. Mirrors [`AmountRange`], whose bounds carry no unit
/// and so cannot be authored directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Range {
    pub minimum: Amount,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<Amount>,
}

impl Range {
    /// The min/max invariant is left to `ValidAmountRange`, which the caller
    /// still goes through.
    pub fn into_on_chain<A: AssetClass>(self, decimals: u8) -> anyhow::Result<AmountRange<A>> {
        Ok(AmountRange {
            minimum: FungibleAssetAmount::new(self.minimum.to_base_units(decimals)?),
            maximum: self
                .maximum
                .map(|maximum| maximum.to_base_units(decimals))
                .transpose()?
                .map(FungibleAssetAmount::new),
        })
    }

    pub fn from_on_chain<A: AssetClass>(range: &AmountRange<A>, decimals: u8) -> Self {
        Self {
            minimum: Amount::from_base_units(u128::from(range.minimum), decimals),
            maximum: range
                .maximum
                .map(|maximum| Amount::from_base_units(u128::from(maximum), decimals)),
        }
    }

    /// Every amount stated here, for the callers that need their scales.
    pub fn amounts(&self) -> impl Iterator<Item = Amount> + '_ {
        std::iter::once(self.minimum).chain(self.maximum)
    }
}

/// [`Fee`] with its flat amount carrying a unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum FeeSpec {
    Flat(Amount),
    Proportional(#[schemars(with = "String")] Decimal),
}

impl FeeSpec {
    pub fn into_on_chain<A: AssetClass>(self, decimals: u8) -> anyhow::Result<Fee<A>> {
        Ok(match self {
            Self::Flat(amount) => {
                Fee::Flat(FungibleAssetAmount::new(amount.to_base_units(decimals)?))
            }
            Self::Proportional(factor) => Fee::Proportional(factor),
        })
    }

    pub fn from_on_chain<A: AssetClass>(fee: &Fee<A>, decimals: u8) -> Self {
        match fee {
            Fee::Flat(amount) => Self::Flat(Amount::from_base_units(u128::from(*amount), decimals)),
            Fee::Proportional(factor) => Self::Proportional(*factor),
        }
    }

    pub const fn amount(&self) -> Option<Amount> {
        match self {
            Self::Flat(amount) => Some(*amount),
            Self::Proportional(_) => None,
        }
    }
}

/// [`TimeBasedFee`] with its flat amount carrying a unit. Only the amount is
/// re-modeled; `duration` and `behavior` stay the on-chain types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimeBasedFeeSpec {
    pub fee: FeeSpec,
    #[schemars(with = "String")]
    pub duration: U64,
    #[schemars(with = "String")]
    pub behavior: TimeBasedFeeFunction,
}

impl TimeBasedFeeSpec {
    pub fn into_on_chain<A: AssetClass>(self, decimals: u8) -> anyhow::Result<TimeBasedFee<A>> {
        Ok(TimeBasedFee {
            fee: self.fee.into_on_chain(decimals)?,
            duration: self.duration,
            behavior: self.behavior,
        })
    }

    pub fn from_on_chain<A: AssetClass>(fee: &TimeBasedFee<A>, decimals: u8) -> Self {
        Self {
            fee: FeeSpec::from_on_chain(&fee.fee, decimals),
            duration: fee.duration,
            behavior: fee.behavior.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Amount;
    use rstest::rstest;

    #[rstest]
    #[case("0.04 tokens", 7, 400_000)]
    #[case("0.04 tokens", 6, 40_000)]
    #[case("0.004 tokens", 7, 40_000)]
    #[case("1 tokens", 6, 1_000_000)]
    #[case("1 token", 6, 1_000_000)]
    #[case("1000 tokens", 6, 1_000_000_000)]
    #[case("0 tokens", 6, 0)]
    #[case("1 atom", 6, 1)]
    #[case("1 atoms", 24, 1)]
    #[case("0 atoms", 6, 0)]
    #[case("400000 atoms", 7, 400_000)]
    fn an_amount_scales_by_the_asset_decimals(
        #[case] text: &str,
        #[case] decimals: u8,
        #[case] expected: u128,
    ) {
        let amount: Amount = text
            .parse()
            .unwrap_or_else(|e| panic!("parse {text}: {e:#}"));
        assert_eq!(
            amount
                .to_base_units(decimals)
                .unwrap_or_else(|e| panic!("scale {text}: {e:#}")),
            expected,
        );
    }

    /// The whole point: the same text means the same value on either asset, so
    /// a line copied between markets cannot silently change magnitude.
    #[test]
    fn the_same_text_means_the_same_value_at_any_decimals() {
        let amount: Amount = "0.04 tokens".parse().expect("parse");
        for (decimals, expected) in [
            (6u8, 40_000u128),
            (7, 400_000),
            (18, 40_000_000_000_000_000),
        ] {
            assert_eq!(
                amount.to_base_units(decimals).expect("scale"),
                expected,
                "0.04 tokens at {decimals} decimals"
            );
        }
    }

    #[rstest]
    #[case::no_unit("400000")]
    #[case::unknown_unit("1 atomz")]
    #[case::fractional_atom("0.5 atoms")]
    #[case::exponent("1e6 tokens")]
    #[case::negative("-1 atoms")]
    #[case::separator("1_000 tokens")]
    #[case::no_number("tokens")]
    #[case::leading_point(".5 tokens")]
    #[case::trailing_point("5. tokens")]
    #[case::two_points("1.2.3 tokens")]
    #[case::plus("+1 atoms")]
    #[case::empty("")]
    fn a_malformed_amount_is_refused(#[case] text: &str) {
        assert!(
            text.parse::<Amount>().is_err(),
            "`{text}` must not parse as an amount"
        );
    }

    /// More precision than the asset can hold is a mistake, not something to
    /// round away silently.
    #[test]
    fn precision_beyond_the_asset_is_refused() {
        let amount: Amount = "0.0000001 tokens".parse().expect("parse");
        let error = amount
            .to_base_units(6)
            .expect_err("7 decimal places must not fit a 6-decimal asset");
        assert!(
            format!("{error:#}").contains("but the asset has 6"),
            "{error:#}"
        );
    }

    #[rstest]
    #[case(400_000, 7, "0.04 tokens")]
    #[case(40_000, 6, "0.04 tokens")]
    #[case(1, 7, "0.0000001 tokens")]
    #[case(1_000_000_000, 6, "1000 tokens")]
    #[case(0, 6, "0 tokens")]
    #[case(100_000_000_000_000, 6, "100000000 tokens")]
    fn a_base_unit_value_renders_in_whole_units(
        #[case] raw: u128,
        #[case] decimals: u8,
        #[case] expected: &str,
    ) {
        let amount = Amount::from_base_units(raw, decimals);
        assert_eq!(amount.to_string(), expected);
        assert_eq!(
            amount.to_base_units(decimals).expect("round trip"),
            raw,
            "{expected} must scale back to what it came from"
        );
    }

    /// Equal values compare equal however they were written, or a spec that
    /// round-trips through this type would stop equalling itself.
    #[test]
    fn trailing_zeros_do_not_change_a_value() {
        let (terse, padded): (Amount, Amount) = (
            "0.04 tokens".parse().expect("a"),
            "0.0400 tokens".parse().expect("b"),
        );
        assert_eq!(terse, padded);
        assert_eq!(padded.to_string(), "0.04 tokens");
    }

    #[test]
    fn both_spellings_parse_and_the_plural_is_written() {
        for (singular, plural) in [("1 atom", "1 atoms"), ("1 token", "1 tokens")] {
            let parsed: Amount = singular.parse().expect("singular parses");
            assert_eq!(parsed, plural.parse::<Amount>().expect("plural parses"));
            assert_eq!(parsed.to_string(), plural);
        }
    }
}
