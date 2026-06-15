//! An [`Arbitrary`] NEAR account id spanning all four account categories, so
//! fuzz targets can exercise the protocol's distinct `AccountId` validation
//! paths instead of only the `<name>.near` shape.
//!
//! Categories (per
//! <https://docs.near.org/protocol/accounts-contracts/account-id>):
//! 1. **Native implicit** — exactly 64 lowercase-hex chars (ed25519-derived).
//! 2. **Native named** — `.`-separated labels, each a run of alphanumeric
//!    segments joined by single `-`/`_`, with an optional top-level suffix.
//! 3. **Ethereum-like** — `0x` + 40 lowercase-hex chars.
//! 4. **Deterministic** — `0s` + 40 lowercase-hex chars (global contracts).
//!
//! All four satisfy the same underlying validity rules (lowercase
//! `[a-z0-9._-]`, length 2..=64, and — for named ids — no leading/trailing/
//! consecutive separators within a `.`-separated part); the eth-like and
//! deterministic forms are simply specific shapes inside the named charset.
//! Construction is valid-by-design, so [`ArbitraryAccountId::into_account_id`]
//! returns `Some` for the vast majority of inputs; it still routes through real
//! `AccountId::from_str` validation, yielding `None` only for the rare edge
//! (e.g. a candidate truncated below the 2-char minimum).

use arbitrary::Arbitrary;
use near_sdk::AccountId;
use std::str::FromStr;

/// Lowercase alphanumerics — the character class of a named-account *segment*
/// (the maximal run between separators).
const ALNUM: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// A spread of real top-level account names / suffixes: suffix-less top-level
/// accounts (`registrar`, `system`) plus short TLDs beyond `.near`/`.testnet`.
const KNOWN_SUFFIXES: &[&str] = &[
    "near",
    "testnet",
    "tg",
    "signer",
    "sol",
    "system",
    "registrar",
    "pool",
    "factory",
];

/// An arbitrary, (almost-always) valid NEAR account id covering all four
/// account categories. Convert with [`ArbitraryAccountId::into_account_id`].
#[derive(Arbitrary, Debug)]
pub enum ArbitraryAccountId {
    /// Native implicit: exactly 64 lowercase-hex chars.
    NativeImplicit([u8; 32]),
    /// Ethereum-like implicit: `0x` + 40 lowercase-hex chars.
    EthImplicit([u8; 20]),
    /// Deterministic / global-contract: `0s` + 40 lowercase-hex chars.
    Deterministic([u8; 20]),
    /// Native named: labels joined by `.`, with an optional top-level suffix.
    Named(NamedAccount),
}

/// A named account: leading labels plus a trailing top-level suffix.
#[derive(Arbitrary, Debug)]
pub struct NamedAccount {
    /// Leading labels (each becomes one `.`-separated part); at least one part
    /// is always emitted.
    labels: Vec<Label>,
    /// Trailing top-level suffix (or none, for a suffix-less top-level account).
    suffix: Suffix,
}

/// One `.`-separated part: alphanumeric segments joined by single separators,
/// e.g. `foo`, `foo-bar`, `a_b-c`.
#[derive(Arbitrary, Debug)]
struct Label {
    first: Segment,
    /// Additional segments, each preceded by exactly one separator (so a label
    /// never has a leading, trailing, or doubled separator).
    rest: Vec<(Separator, Segment)>,
}

/// A maximal alphanumeric run (no separators).
#[derive(Arbitrary, Debug)]
struct Segment(Vec<u8>);

#[derive(Arbitrary, Debug)]
enum Separator {
    Hyphen,
    Underscore,
}

#[derive(Arbitrary, Debug)]
enum Suffix {
    /// Suffix-less top-level account (e.g. `registrar`).
    None,
    /// One of [`KNOWN_SUFFIXES`].
    Known(u8),
    /// An arbitrary-derived suffix, so suffixes aren't limited to the known set.
    Custom(Label),
}

impl ArbitraryAccountId {
    /// Render to a real [`AccountId`], or `None` if the candidate fails
    /// validation (rare; e.g. a named id truncated below 2 chars).
    #[must_use]
    pub fn into_account_id(self) -> Option<AccountId> {
        AccountId::from_str(&self.to_candidate()).ok()
    }

    /// The candidate string. Valid-by-construction except for the 2..=64 length
    /// bound, which [`Self::into_account_id`] enforces via real validation.
    fn to_candidate(&self) -> String {
        match self {
            Self::NativeImplicit(bytes) => to_hex(bytes),
            Self::EthImplicit(bytes) => format!("0x{}", to_hex(bytes)),
            Self::Deterministic(bytes) => format!("0s{}", to_hex(bytes)),
            Self::Named(named) => cap_len(named.render()),
        }
    }
}

impl NamedAccount {
    fn render(&self) -> String {
        let mut parts: Vec<String> = self.labels.iter().map(Label::render).collect();
        if parts.is_empty() {
            // Guarantee at least one non-empty part so the join never produces
            // a leading/trailing dot.
            parts.push("a".to_string());
        }
        match &self.suffix {
            Suffix::None => {}
            Suffix::Known(i) => {
                parts.push(KNOWN_SUFFIXES[*i as usize % KNOWN_SUFFIXES.len()].to_string());
            }
            Suffix::Custom(label) => parts.push(label.render()),
        }
        parts.join(".")
    }
}

impl Label {
    fn render(&self) -> String {
        let mut out = self.first.render();
        for (sep, seg) in &self.rest {
            out.push(sep.as_char());
            out.push_str(&seg.render());
        }
        out
    }
}

impl Segment {
    fn render(&self) -> String {
        let mut out: String = self
            .0
            .iter()
            .map(|b| ALNUM[*b as usize % ALNUM.len()] as char)
            .collect();
        if out.is_empty() {
            // A segment must be non-empty for the surrounding label to be valid.
            out.push('a');
        }
        out
    }
}

impl Separator {
    fn as_char(&self) -> char {
        match self {
            Self::Hyphen => '-',
            Self::Underscore => '_',
        }
    }
}

/// Lowercase-hex encode (2 chars per byte).
fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(b & 0x0f), 16).unwrap_or('0'));
    }
    out
}

/// Cap a named candidate at the 64-char maximum, trimming any separator left
/// dangling at the cut so the tail stays a valid label.
fn cap_len(mut s: String) -> String {
    const MAX: usize = 64;
    if s.len() > MAX {
        s.truncate(MAX);
        while matches!(s.as_bytes().last(), Some(b'.' | b'-' | b'_')) {
            s.pop();
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid(s: &str) -> bool {
        AccountId::from_str(s).is_ok()
    }

    #[test]
    fn all_four_categories_validate() {
        assert!(valid(&"f".repeat(64)), "native implicit (64 hex)");
        assert!(valid(&format!("0x{}", "a".repeat(40))), "eth-like");
        assert!(valid(&format!("0s{}", "b".repeat(40))), "deterministic");
        assert!(valid("alice.near"), "named");
    }

    #[test]
    fn named_variety_validates() {
        // suffix-less top-level accounts
        assert!(valid("registrar"));
        assert!(valid("system"));
        // separators (single, mixed) and multiple `.`-parts
        assert!(valid("a-b_c.def.near"));
        assert!(valid("foo-bar.testnet"));
        // suffixes beyond near/testnet
        assert!(valid("x.tg"));
        assert!(valid("dao.signer"));
    }

    #[test]
    fn invalid_shapes_are_rejected() {
        assert!(!valid("a"), "too short (< 2)");
        assert!(!valid(&"a".repeat(65)), "too long (> 64)");
        assert!(!valid("-bad.near"), "leading separator");
        assert!(!valid("bad-.near"), "trailing separator");
        assert!(!valid("a..b"), "consecutive dots");
        assert!(!valid("a__b.near"), "consecutive separators");
        assert!(!valid("Alice.near"), "uppercase");
    }

    #[test]
    fn renderers_are_valid_by_construction() {
        // Empty segment falls back to a single alnum char.
        assert_eq!(Segment(vec![]).render(), "a");
        // A label with separators renders without leading/trailing/doubled seps.
        let label = Label {
            first: Segment(vec![0]),
            rest: vec![
                (Separator::Hyphen, Segment(vec![1])),
                (Separator::Underscore, Segment(vec![2])),
            ],
        };
        let rendered = label.render();
        assert!(
            valid(&format!("{rendered}.near")),
            "rendered label: {rendered}"
        );
        // Over-long named candidates are capped to a valid <=64 id.
        let long = NamedAccount {
            labels: vec![Label {
                first: Segment(vec![0; 200]),
                rest: vec![],
            }],
            suffix: Suffix::Known(0),
        };
        let id = ArbitraryAccountId::Named(long).into_account_id();
        assert!(id.is_some(), "capped long candidate should still validate");
        assert!(id.unwrap().as_str().len() <= 64);
    }
}
