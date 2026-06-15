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
//! Validity is **structural**, not checked at runtime: characters are drawn
//! from a lowercase-alphanumeric type, separators only ever sit between
//! non-empty segments, and a named account always begins with a mandatory
//! two-character head segment — so every candidate satisfies NEAR's charset,
//! separator, and 2-char-minimum rules by construction. The only runtime step
//! is capping to the 64-char maximum, which preserves the leading head chars.
//! [`ArbitraryAccountId::into_account_id`] therefore effectively always returns
//! `Some`; it still routes through real `AccountId::from_str` as a backstop.

use arbitrary::Arbitrary;
use near_sdk::AccountId;
use std::str::FromStr;

/// Lowercase alphanumerics — the character class of a named-account *segment*
/// (the maximal run between separators), and of the hex address forms.
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

/// An arbitrary, valid NEAR account id covering all four account categories.
/// Convert with [`ArbitraryAccountId::into_account_id`].
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

/// A named account. The mandatory [`HeadSegment`] start is what guarantees the
/// rendered id meets NEAR's 2-char minimum without any runtime padding.
#[derive(Arbitrary, Debug)]
pub struct NamedAccount {
    /// The first label's leading segment (>= 2 chars by construction).
    head: HeadSegment,
    /// Remaining segments of the first label, each preceded by one separator.
    first_label_rest: Vec<(Separator, Segment)>,
    /// Additional `.`-separated labels after the first.
    other_labels: Vec<Label>,
    /// Trailing top-level suffix; `None` is a suffix-less top-level account
    /// (e.g. `registrar`).
    suffix: Option<Suffix>,
}

/// The first segment of a named account's first label: at least two
/// alphanumeric characters. This is the structural proof that a named id is
/// never shorter than NEAR's 2-char minimum — truncation to the 64-char max
/// keeps these leading characters.
#[derive(Arbitrary, Debug)]
struct HeadSegment {
    first: AlnumChar,
    second: AlnumChar,
    rest: Vec<AlnumChar>,
}

/// A maximal alphanumeric run (>= 1 char, no separators).
#[derive(Arbitrary, Debug)]
struct Segment {
    first: AlnumChar,
    rest: Vec<AlnumChar>,
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

/// A lowercase-alphanumeric character (`a-z0-9`), valid-by-construction.
#[derive(Arbitrary, Debug)]
struct AlnumChar(u8);

#[derive(Arbitrary, Debug)]
enum Separator {
    Hyphen,
    Underscore,
}

/// A trailing top-level suffix. Absence (a suffix-less top-level account) is
/// represented by `Option::None` at the use site rather than a variant here.
#[derive(Arbitrary, Debug)]
enum Suffix {
    /// One of [`KNOWN_SUFFIXES`].
    Known(u8),
    /// An arbitrary-derived suffix, so suffixes aren't limited to the known set.
    Custom(Label),
}

impl ArbitraryAccountId {
    /// Render to a real [`AccountId`], or `None` if real validation rejects the
    /// candidate. Construction is valid-by-design, so this is effectively
    /// always `Some`.
    #[must_use]
    pub fn into_account_id(self) -> Option<AccountId> {
        AccountId::from_str(&self.to_candidate()).ok()
    }

    /// The candidate string, valid-by-construction. Named ids are >= 2 chars
    /// structurally (the head segment) and capped to <= 64 by [`cap_len`].
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
        // First label: the >= 2-char head, then any separator-joined segments.
        let mut first_label = self.head.render();
        for (sep, seg) in &self.first_label_rest {
            first_label.push(sep.as_char());
            first_label.push_str(&seg.render());
        }

        let mut parts = vec![first_label];
        parts.extend(self.other_labels.iter().map(Label::render));
        match &self.suffix {
            None => {}
            Some(Suffix::Known(i)) => {
                parts.push(KNOWN_SUFFIXES[*i as usize % KNOWN_SUFFIXES.len()].to_string());
            }
            Some(Suffix::Custom(label)) => parts.push(label.render()),
        }
        parts.join(".")
    }
}

impl HeadSegment {
    fn render(&self) -> String {
        let mut out = String::new();
        out.push(self.first.as_char());
        out.push(self.second.as_char());
        for c in &self.rest {
            out.push(c.as_char());
        }
        out
    }
}

impl Segment {
    fn render(&self) -> String {
        let mut out = String::new();
        out.push(self.first.as_char());
        for c in &self.rest {
            out.push(c.as_char());
        }
        out
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

impl AlnumChar {
    fn as_char(&self) -> char {
        ALNUM[self.0 as usize % ALNUM.len()] as char
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

/// Cap a named candidate at NEAR's 64-char maximum, trimming any separator left
/// dangling at the cut so the tail stays a valid label. The candidate is >= 2
/// chars by construction (its first label opens with a 2-char head segment) and
/// truncation keeps those leading chars, so the result stays within 2..=64.
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

    fn seg(c: u8) -> Segment {
        Segment {
            first: AlnumChar(c),
            rest: vec![],
        }
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

    /// The whole point of the restructure: even the smallest representable
    /// named account is a valid 2-char id, with no runtime padding.
    #[test]
    fn min_length_is_structural() {
        let smallest = NamedAccount {
            head: HeadSegment {
                first: AlnumChar(0),
                second: AlnumChar(0),
                rest: vec![],
            },
            first_label_rest: vec![],
            other_labels: vec![],
            suffix: None,
        };
        let id = ArbitraryAccountId::Named(smallest).into_account_id();
        assert_eq!(id.map(|a| a.to_string()).as_deref(), Some("aa"));
    }

    #[test]
    fn renderers_are_valid_by_construction() {
        // A segment is never empty.
        assert_eq!(seg(0).render(), "a");

        // Mixed separators across the first label and an extra `.`-part render
        // to a valid id.
        let mixed = NamedAccount {
            head: HeadSegment {
                first: AlnumChar(0),
                second: AlnumChar(1),
                rest: vec![],
            },
            first_label_rest: vec![(Separator::Hyphen, seg(2))],
            other_labels: vec![Label {
                first: seg(3),
                rest: vec![(Separator::Underscore, seg(4))],
            }],
            suffix: Some(Suffix::Known(0)),
        };
        assert!(ArbitraryAccountId::Named(mixed).into_account_id().is_some());

        // Over-long named candidates are capped to a valid id in 2..=64.
        let long = NamedAccount {
            head: HeadSegment {
                first: AlnumChar(0),
                second: AlnumChar(0),
                rest: (0..200).map(|_| AlnumChar(0)).collect(),
            },
            first_label_rest: vec![],
            other_labels: vec![],
            suffix: Some(Suffix::Known(0)),
        };
        let id = ArbitraryAccountId::Named(long)
            .into_account_id()
            .expect("capped long candidate should still validate");
        assert!((2..=64).contains(&id.to_string().len()));
    }
}
