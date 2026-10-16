use std::{cmp::Ordering, fmt};

use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use serde::{Serialize, Serializer};
use templar_common::Decimal;
use templar_gateway_types::contract::ContractKind;
use templar_gateway_types::version::Version;

use super::assets::{self, AssetSelector};

pub(crate) const MAX_RELATION_DEPTH: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum RootKind {
    Markets,
    ProxyOracles,
    ProxyGovernance,
}

impl RootKind {
    pub(crate) fn node_kind(self) -> NodeKind {
        match self {
            Self::Markets => NodeKind::Market,
            Self::ProxyOracles => NodeKind::Oracle,
            Self::ProxyGovernance => NodeKind::Governance,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) enum NodeKind {
    #[serde(rename = "market")]
    Market,
    #[serde(rename = "proxy_oracle")]
    Oracle,
    #[serde(rename = "proxy_governance")]
    Governance,
}

impl NodeKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Market => "market",
            Self::Oracle => "proxy_oracle",
            Self::Governance => "proxy_governance",
        }
    }

    fn fields(self) -> impl Iterator<Item = Field> {
        COMMON_FIELDS
            .iter()
            .chain(
                if self == Self::Market {
                    MARKET_FIELDS
                } else {
                    &[]
                }
                .iter(),
            )
            .copied()
    }
}

impl fmt::Display for NodeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub(crate) fn kind_name(kind: ContractKind) -> Result<String> {
    kind.to_possible_value()
        .map(|value| value.get_name().to_owned())
        .context("gateway contract kind has no canonical CLI name")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Relation {
    Oracle,
    Governance,
}

// The schema's only relationship definitions. Parsing and discovery share it.
const RELATION_EDGES: &[(NodeKind, Relation, NodeKind)] = &[
    (NodeKind::Market, Relation::Oracle, NodeKind::Oracle),
    (NodeKind::Oracle, Relation::Governance, NodeKind::Governance),
    (NodeKind::Governance, Relation::Oracle, NodeKind::Oracle),
];

impl Relation {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Oracle => "oracle",
            Self::Governance => "governance",
        }
    }

    pub(crate) fn target(self, source: NodeKind) -> Option<NodeKind> {
        RELATION_EDGES
            .iter()
            .find_map(|&(from, relation, to)| (from == source && relation == self).then_some(to))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Field {
    Id,
    Kind,
    Version,
    BorrowAsset,
    CollateralAsset,
    BorrowApr,
    UsageRatio,
    Borrowed,
    Supplied,
    BorrowedUsd,
    SuppliedUsd,
}

const COMMON_FIELDS: &[Field] = &[Field::Id, Field::Kind, Field::Version];
const MARKET_FIELDS: &[Field] = &[
    Field::BorrowAsset,
    Field::CollateralAsset,
    Field::BorrowApr,
    Field::UsageRatio,
    Field::Borrowed,
    Field::Supplied,
    Field::BorrowedUsd,
    Field::SuppliedUsd,
];
const EQUALITY_OPERATORS: &[&str] = &["=", "!="];
const ORDERED_OPERATORS: &[&str] = &["=", "!=", "<", "<=", ">", ">="];

impl Field {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Kind => "kind",
            Self::Version => "version",
            Self::BorrowAsset => "borrow_asset",
            Self::CollateralAsset => "collateral_asset",
            Self::BorrowApr => "borrow_apr",
            Self::UsageRatio => "usage_ratio",
            Self::Borrowed => "borrowed",
            Self::Supplied => "supplied",
            Self::BorrowedUsd => "borrowed_usd",
            Self::SuppliedUsd => "supplied_usd",
        }
    }

    pub(crate) fn is_usd(self) -> bool {
        matches!(self, Self::BorrowedUsd | Self::SuppliedUsd)
    }

    fn is_ordered(self) -> bool {
        !matches!(
            self,
            Self::Id | Self::Kind | Self::BorrowAsset | Self::CollateralAsset
        )
    }

    fn is_percent(self) -> bool {
        matches!(self, Self::BorrowApr | Self::UsageRatio)
    }

    fn description(self) -> (&'static str, Option<&'static str>, &'static str) {
        match self {
            Self::Id => ("text", None, "Contract account ID."),
            Self::Kind => ("text", None, "Live gateway contract kind, e.g. market, proxy_oracle, proxy_governance, or pyth_oracle."),
            Self::Version => ("version", None, "Current NEP-330 version, compared as numeric major.minor.patch components."),
            Self::BorrowAsset => ("asset", None, "Exact borrow asset identity; predicates also accept catalog aliases."),
            Self::CollateralAsset => ("asset", None, "Exact collateral asset identity; predicates also accept catalog aliases."),
            Self::BorrowApr => ("number", Some("fraction/year"), "Current borrow APR; 10% and 0.1 express the same threshold."),
            Self::UsageRatio => ("number", Some("fraction"), "Current usage ratio; 10% and 0.1 express the same threshold."),
            Self::Borrowed => ("number", Some("borrow tokens"), "Current borrowed principal, including in-flight borrowing, excluding accrued interest and fees."),
            Self::Supplied => ("number", Some("borrow tokens"), "Current active supplied principal, including activatable incoming supply; not total supplier claims."),
            Self::BorrowedUsd => ("number", Some("USD"), "Borrowed principal valued using a fresh verified indicative borrow-asset price; unavailable on unsupported providers."),
            Self::SuppliedUsd => ("number", Some("USD"), "Active supplied principal valued using a fresh verified indicative borrow-asset price; not TVL."),
        }
    }
}

impl fmt::Display for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FieldPath {
    root: NodeKind,
    pub relations: Vec<Relation>,
    pub field: Field,
}

impl FieldPath {
    pub(crate) fn parse(root: NodeKind, text: &str) -> Result<Self> {
        let mut parts = text.split('.').peekable();
        let mut endpoint = root;
        let mut relations = Vec::new();
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                let field = endpoint
                    .fields()
                    .find(|field| field.as_str() == part)
                    .with_context(|| format!("unknown field {part:?} on {endpoint}"))?;
                return Ok(Self {
                    root,
                    relations,
                    field,
                });
            }
            if relations.len() == MAX_RELATION_DEPTH {
                bail!("relationship path exceeds {MAX_RELATION_DEPTH} edges");
            }
            let (relation, target) = RELATION_EDGES
                .iter()
                .find_map(|&(from, relation, to)| {
                    (from == endpoint && relation.as_str() == part).then_some((relation, to))
                })
                .with_context(|| format!("unknown relationship {part:?} on {endpoint}"))?;
            relations.push(relation);
            endpoint = target;
        }
        bail!("empty field path")
    }

    pub(crate) fn ancestor_ids(&self) -> impl Iterator<Item = Self> + '_ {
        (0..=self.relations.len()).map(|depth| Self {
            root: self.root,
            relations: self.relations[..depth].to_vec(),
            field: Field::Id,
        })
    }

    pub(crate) fn endpoint_kind(&self) -> NodeKind {
        // Only parsing constructs paths. Each relation has exactly one target kind.
        match self.relations.last() {
            Some(Relation::Oracle) => NodeKind::Oracle,
            Some(Relation::Governance) => NodeKind::Governance,
            None => self.root,
        }
    }
}

impl fmt::Display for FieldPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for relation in &self.relations {
            write!(f, "{}.", relation.as_str())?;
        }
        self.field.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Value {
    Text(String),
    Version(Version<()>),
    Number(Decimal),
    Asset(String),
}

impl Serialize for Value {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Text(value) | Self::Asset(value) => serializer.serialize_str(value),
            Self::Version(value) => serializer.collect_str(value),
            Self::Number(value) => value.quantize().serialize(serializer),
        }
    }
}

fn typed_comparison(left: &Value, right: &Value) -> Option<Ordering> {
    match (left, right) {
        (Value::Text(left), Value::Text(right)) | (Value::Asset(left), Value::Asset(right)) => {
            Some(left.cmp(right))
        }
        (Value::Version(left), Value::Version(right)) => Some(left.cmp(right)),
        (Value::Number(left), Value::Number(right)) => Some(left.quantize().cmp(&right.quantize())),
        _ => None,
    }
}

/// Sort a typed column; absent values remain last in either direction.
pub(crate) fn compare_values(
    left: Option<&Value>,
    right: Option<&Value>,
    descending: bool,
) -> Ordering {
    match (left, right) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(left), Some(right)) => {
            // A valid field column is homogeneous. Rank mixed values defensively
            // to keep this comparator a total order without coercing their types.
            let rank = |value: &Value| match value {
                Value::Text(_) => 0,
                Value::Version(_) => 1,
                Value::Number(_) => 2,
                Value::Asset(_) => 3,
            };
            let order =
                typed_comparison(left, right).unwrap_or_else(|| rank(left).cmp(&rank(right)));
            if descending {
                order.reverse()
            } else {
                order
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Operator {
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

impl Operator {
    fn parse(text: &str) -> Result<Self> {
        match text {
            "=" => Ok(Self::Equal),
            "!=" => Ok(Self::NotEqual),
            "<" => Ok(Self::Less),
            "<=" => Ok(Self::LessEqual),
            ">" => Ok(Self::Greater),
            ">=" => Ok(Self::GreaterEqual),
            _ => bail!("unknown operator {text:?}; expected =, !=, <, <=, >, or >="),
        }
    }

    fn matches(self, order: Ordering) -> bool {
        match self {
            Self::Equal => order == Ordering::Equal,
            Self::NotEqual => order != Ordering::Equal,
            Self::Less => order == Ordering::Less,
            Self::LessEqual => order != Ordering::Greater,
            Self::Greater => order == Ordering::Greater,
            Self::GreaterEqual => order != Ordering::Less,
        }
    }
}

#[derive(Debug, Clone)]
enum Expected {
    Scalar(Value),
    Asset(AssetSelector),
}

#[derive(Debug, Clone)]
pub(crate) struct Predicate {
    pub path: FieldPath,
    operator: Operator,
    expected: Expected,
}

impl Predicate {
    pub(crate) fn parse(root: NodeKind, expression: &str) -> Result<Self> {
        let mut parts = expression.split_whitespace();
        let (Some(path), Some(operator), Some(literal), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            bail!("expected a whitespace-separated field, operator, and value");
        };
        let path = FieldPath::parse(root, path)?;
        let operator = Operator::parse(operator)?;
        if !path.field.is_ordered() && !matches!(operator, Operator::Equal | Operator::NotEqual) {
            bail!("{} only supports = and != predicates", path.field);
        }
        let expected = match path.field {
            Field::Id => Expected::Scalar(Value::Text(
                literal
                    .parse::<near_account_id::AccountId>()
                    .context("invalid account ID threshold")?
                    .to_string(),
            )),
            Field::Kind => {
                if ContractKind::from_str(literal, false).is_err() {
                    bail!("unknown gateway contract kind {literal:?}; use snake_case, e.g. proxy_oracle");
                }
                Expected::Scalar(Value::Text(literal.to_owned()))
            }
            Field::Version => {
                // Reject signs accepted by integer FromStr; no semver suffixes.
                if literal.split('.').count() != 3
                    || !literal.split('.').all(|part| {
                        !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
                    })
                {
                    bail!("expected a numeric major.minor.patch version, got {literal:?}");
                }
                Expected::Scalar(Value::Version(
                    literal.parse().context("invalid version threshold")?,
                ))
            }
            Field::BorrowAsset | Field::CollateralAsset => {
                Expected::Asset(assets::resolve(literal)?)
            }
            field => Expected::Scalar(Value::Number(parse_number(literal, field.is_percent())?)),
        };
        Ok(Self {
            path,
            operator,
            expected,
        })
    }

    pub(crate) fn matches(&self, value: Option<&Value>) -> bool {
        let Some(value) = value else {
            return false;
        };
        match (&self.expected, value) {
            (Expected::Scalar(expected), actual) => {
                typed_comparison(actual, expected).is_some_and(|order| self.operator.matches(order))
            }
            (Expected::Asset(expected), Value::Asset(actual)) => {
                let equal = expected.matches(actual);
                match self.operator {
                    Operator::Equal => equal,
                    Operator::NotEqual => !equal,
                    _ => false,
                }
            }
            _ => false,
        }
    }
}

fn parse_number(literal: &str, percent_allowed: bool) -> Result<Decimal> {
    if let Some(percent) = literal.strip_suffix('%') {
        if !percent_allowed {
            bail!("percent units are only valid for borrow_apr and usage_ratio");
        }
        // Shift the input's decimal point, then parse once. Dividing an already
        // quantized Decimal could make 10% differ from the threshold 0.1.
        let (whole, fractional) = percent.split_once('.').unwrap_or((percent, ""));
        if whole.is_empty()
            || !whole.bytes().all(|byte| byte.is_ascii_digit())
            || (whole.len() > 1 && whole.starts_with('0'))
            || !fractional.bytes().all(|byte| byte.is_ascii_digit())
            || (percent.contains('.') && fractional.is_empty())
        {
            bail!("invalid nonnegative percent {literal:?}");
        }
        let mut normalized = if whole.len() > 2 {
            format!(
                "{}.{}{}",
                &whole[..whole.len() - 2],
                &whole[whole.len() - 2..],
                fractional
            )
        } else {
            format!(
                "0.{}{}{}",
                if whole.len() == 1 { "0" } else { "" },
                whole,
                fractional
            )
        };
        while normalized.ends_with('0') {
            normalized.pop();
        }
        if normalized.ends_with('.') {
            normalized.pop();
        }
        normalized
            .parse()
            .with_context(|| format!("percent threshold out of range or precision: {literal:?}"))
    } else {
        literal
            .parse()
            .with_context(|| format!("invalid nonnegative decimal threshold {literal:?}"))
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct FieldDescription {
    pub name: String,
    pub value_type: &'static str,
    pub unit: Option<&'static str>,
    pub operators: &'static [&'static str],
    pub description: &'static str,
}

/// Enumerate shortest paths to each reachable kind without expanding cycles.
pub(crate) fn catalog(root: NodeKind) -> Vec<FieldDescription> {
    let mut result = Vec::new();
    let mut seen = Vec::new();
    let mut endpoint = root;
    let mut prefix = String::new();
    while !seen.contains(&endpoint) {
        seen.push(endpoint);
        for field in endpoint.fields() {
            let (value_type, unit, description) = field.description();
            result.push(FieldDescription {
                name: format!("{prefix}{field}"),
                value_type,
                unit,
                operators: if field.is_ordered() {
                    ORDERED_OPERATORS
                } else {
                    EQUALITY_OPERATORS
                },
                description,
            });
        }
        let Some(&(_, relation, target)) = RELATION_EDGES
            .iter()
            .find(|&&(from, _, _)| from == endpoint)
        else {
            break;
        };
        prefix.push_str(relation.as_str());
        prefix.push('.');
        endpoint = target;
    }
    result
}

pub(crate) fn default_fields(root: NodeKind) -> Vec<FieldPath> {
    root.fields()
        .filter(|field| !field.is_usd())
        .map(|field| FieldPath {
            root,
            relations: Vec::new(),
            field,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number(text: &str) -> Value {
        Value::Number(text.parse().unwrap())
    }
    fn predicate(text: &str) -> Predicate {
        Predicate::parse(NodeKind::Market, text).unwrap()
    }

    #[test]
    fn derived_decimals_respect_fractional_filter_boundaries() {
        let principal = |amount| super::super::prices::token_amount(amount, 6).unwrap();
        let usage =
            |amount: u128| templar_common::market::usage_ratio(1_000_000.into(), amount.into());
        for (field, values, threshold) in [
            (
                "borrowed",
                [principal(99_999), principal(100_000), principal(100_001)],
                "0.1",
            ),
            (
                "usage_ratio",
                [usage(99_999), usage(100_000), usage(100_001)],
                "10%",
            ),
        ] {
            let values = values.map(Value::Number);
            for (operator, expected) in [
                ("=", [false, true, false]),
                ("!=", [true, false, true]),
                ("<", [true, false, false]),
                ("<=", [true, true, false]),
                (">", [false, false, true]),
                (">=", [false, true, true]),
            ] {
                let filter = predicate(&format!("{field} {operator} {threshold}"));
                for (actual, expected) in values.iter().zip(expected) {
                    assert_eq!(filter.matches(Some(actual)), expected, "{field} {operator}");
                }
            }
            for descending in [false, true] {
                assert_eq!(
                    compare_values(Some(&values[1]), Some(&number("0.1")), descending),
                    Ordering::Equal
                );
            }
            for (left_index, left) in values.iter().enumerate() {
                for (right_index, right) in values.iter().enumerate() {
                    let expected = left_index.cmp(&right_index);
                    assert_eq!(compare_values(Some(left), Some(right), false), expected);
                    assert_eq!(
                        compare_values(Some(left), Some(right), true),
                        expected.reverse()
                    );
                }
            }
        }
    }

    #[test]
    fn numeric_comparison_uses_transitive_quantized_keys() {
        let values = [0, 4, 8, 12]
            .map(|repr| Value::Number(Decimal::from_repr([repr, 0, 0, 0, 0, 0, 0, 0])));
        for left in &values {
            for middle in &values {
                for right in &values {
                    if typed_comparison(left, middle).unwrap().is_le()
                        && typed_comparison(middle, right).unwrap().is_le()
                    {
                        assert!(typed_comparison(left, right).unwrap().is_le());
                    }
                }
            }
        }
        assert_eq!(
            typed_comparison(&values[0], &values[1]),
            Some(Ordering::Less)
        );
        assert_eq!(
            typed_comparison(&values[1], &values[2]),
            Some(Ordering::Equal)
        );
        assert_eq!(
            typed_comparison(&Value::Number(Decimal::MAX), &Value::Number(Decimal::MAX)),
            Some(Ordering::Equal)
        );
    }

    #[test]
    fn numeric_output_can_be_reused_as_an_equality_filter() {
        for amount in 0..128 {
            let value = Value::Number(super::super::prices::token_amount(amount, 38).unwrap());
            let literal =
                serde_json::from_value::<String>(serde_json::to_value(&value).unwrap()).unwrap();
            assert!(
                predicate(&format!("borrowed = {literal}")).matches(Some(&value)),
                "amount={amount}, literal={literal}"
            );
        }
    }

    #[test]
    fn numeric_and_version_comparisons_are_typed() {
        assert!(predicate("borrowed > 9").matches(Some(&number("10"))));
        assert!(!predicate("borrowed > 9").matches(Some(&number("2"))));
        assert!(
            predicate("borrowed >= 9007199254740993").matches(Some(&number("9007199254740993")))
        );
        assert!(
            !predicate("borrowed = 9007199254740993").matches(Some(&number("9007199254740992")))
        );
        assert!(
            predicate("version > 0.9.99").matches(Some(&Value::Version("0.10.0".parse().unwrap())))
        );
        assert!(
            predicate("version = 18446744073709551615.0.0").matches(Some(&Value::Version(
                "18446744073709551615.0.0".parse().unwrap()
            )))
        );
        assert!(Predicate::parse(NodeKind::Market, "version = 18446744073709551616.0.0").is_err());
    }

    #[test]
    fn all_ordered_operators_respect_equal_and_adjacent_boundaries() {
        for (operator, expected) in [
            ("=", [false, true, false]),
            ("!=", [true, false, true]),
            ("<", [true, false, false]),
            ("<=", [true, true, false]),
            (">", [false, false, true]),
            (">=", [false, true, true]),
        ] {
            let numeric = predicate(&format!("borrowed {operator} 1"));
            let version = predicate(&format!("version {operator} 1.0.0"));
            for (index, actual) in ["0", "1", "2"].into_iter().enumerate() {
                assert_eq!(numeric.matches(Some(&number(actual))), expected[index]);
            }
            for (index, actual) in ["0.99.99", "1.0.0", "1.0.1"].into_iter().enumerate() {
                assert_eq!(
                    version.matches(Some(&Value::Version(actual.parse().unwrap()))),
                    expected[index]
                );
            }
        }
        assert!(predicate("borrowed = 0").matches(Some(&number("0"))));
        assert!(
            predicate("version = 0.0.0").matches(Some(&Value::Version("0.0.0".parse().unwrap())))
        );
    }

    #[test]
    fn percent_thresholds_normalize_before_decimal_quantization() {
        for (percent, fractional) in [
            ("10%", "0.1"),
            ("0%", "0"),
            ("1%", "0.01"),
            ("0.1%", "0.001"),
            ("100%", "1"),
            ("1234.5%", "12.345"),
        ] {
            assert!(
                predicate(&format!("borrow_apr = {percent}")).matches(Some(&number(fractional)))
            );
            assert!(
                predicate(&format!("usage_ratio = {percent}")).matches(Some(&number(fractional)))
            );
        }
        assert!(predicate("borrow_apr = 0.1").matches(Some(&number("0.1"))));
        for field in ["borrowed", "supplied", "borrowed_usd", "supplied_usd"] {
            assert!(Predicate::parse(NodeKind::Market, &format!("{field} = 10%")).is_err());
        }
    }

    #[test]
    fn malformed_filters_and_incompatible_operators_are_rejected() {
        for expression in [
            "",
            "borrowed",
            "borrowed>10",
            "borrowed >",
            "borrowed > 1 extra",
            "borrowed == 1",
            "id < account.near",
            "kind >= market",
            "borrow_asset > BTC",
            "unknown = 1",
            "borrow_asset = UNKNOWN",
            "kind = oracle",
            "version = 1.2",
            "version = 1.2.3-rc1",
            "version = +1.2.3",
        ] {
            assert!(
                Predicate::parse(NodeKind::Market, expression).is_err(),
                "{expression}"
            );
        }
        for literal in [
            "-1", "+1", "NaN", "inf", "1e3", "1.2.3", "1%%", "01", "1.", "-1%", "+1%", ".1%", "1.%",
        ] {
            assert!(
                Predicate::parse(NodeKind::Market, &format!("borrow_apr = {literal}")).is_err(),
                "{literal}"
            );
        }
        for literal in ["9".repeat(200), format!("0.{}1", "0".repeat(38))] {
            assert!(Predicate::parse(NodeKind::Market, &format!("borrowed = {literal}")).is_err());
        }
    }

    #[test]
    fn missing_values_and_wrong_types_never_match_even_not_equal() {
        for expression in [
            "id != account.near",
            "version != 1.0.0",
            "borrowed != 0",
            "borrow_asset != BTC",
        ] {
            let predicate = predicate(expression);
            assert!(!predicate.matches(None), "{expression}");
        }
        assert!(!predicate("borrowed != 0").matches(Some(&Value::Text("1".into()))));
        assert!(!predicate("borrow_asset != BTC").matches(Some(&Value::Text("WBTC".into()))));
    }

    #[test]
    fn asset_alias_predicates_match_exact_members_only() {
        let btc = Value::Asset("nep245:intents.near:nep141:btc.omft.near".into());
        let direct = Value::Asset("nep141:btc.omft.near".into());
        let wrapped = Value::Asset(
            "nep245:intents.near:nep141:eth-0x2260fac5e5542a773aa44fbcfedf7c193bc2c599.omft.near"
                .into(),
        );
        assert!(predicate("collateral_asset = BTC").matches(Some(&btc)));
        assert!(!predicate("collateral_asset = BTC").matches(Some(&direct)));
        assert!(!predicate("collateral_asset = BTC").matches(Some(&wrapped)));
        assert!(predicate("collateral_asset != BTC").matches(Some(&wrapped)));
        let exact = "nep245:intents.near:nep245:tokens.near:asset:part";
        assert!(predicate(&format!("borrow_asset = {exact}"))
            .matches(Some(&Value::Asset(exact.into()))));
    }

    #[test]
    fn sorting_keeps_missing_last_in_both_directions() {
        for descending in [false, true] {
            assert_eq!(
                compare_values(None, Some(&number("2")), descending),
                Ordering::Greater
            );
            assert_eq!(
                compare_values(Some(&number("2")), None, descending),
                Ordering::Less
            );
            assert_eq!(compare_values(None, None, descending), Ordering::Equal);
        }
        assert_eq!(
            compare_values(Some(&number("10")), Some(&number("2")), false),
            Ordering::Greater
        );
        assert_eq!(
            compare_values(Some(&number("10")), Some(&number("2")), true),
            Ordering::Less
        );
        assert_eq!(
            compare_values(
                Some(&Value::Version("0.10.0".parse().unwrap())),
                Some(&Value::Version("0.9.0".parse().unwrap())),
                false
            ),
            Ordering::Greater
        );
    }

    #[test]
    fn multihop_relations_validate_each_edge_without_banning_cycles() {
        let path = FieldPath::parse(NodeKind::Market, "oracle.governance.oracle.version").unwrap();
        assert_eq!(path.endpoint_kind(), NodeKind::Oracle);
        assert_eq!(path.to_string(), "oracle.governance.oracle.version");
        assert_eq!(
            path.relations,
            [Relation::Oracle, Relation::Governance, Relation::Oracle]
        );
        assert_eq!(
            FieldPath::parse(NodeKind::Oracle, "governance.oracle.governance.id")
                .unwrap()
                .endpoint_kind(),
            NodeKind::Governance
        );
        for invalid in [
            "governance.version",
            "oracle.oracle.version",
            "oracle.borrowed",
            "oracle.governance.borrow_asset",
            "oracle..id",
            "oracle.",
            ".id",
        ] {
            assert!(
                FieldPath::parse(NodeKind::Market, invalid).is_err(),
                "{invalid}"
            );
        }
        let at_limit = format!(
            "{}version",
            "oracle.governance.".repeat(MAX_RELATION_DEPTH / 2)
        );
        assert!(FieldPath::parse(NodeKind::Market, &at_limit).is_ok());
        let too_deep = format!(
            "{}oracle.version",
            "oracle.governance.".repeat(MAX_RELATION_DEPTH / 2)
        );
        assert!(FieldPath::parse(NodeKind::Market, &too_deep).is_err());
    }

    #[test]
    fn values_serialize_as_lossless_human_readable_strings() {
        for (value, text) in [
            (number("9007199254740993.125"), "9007199254740993.125"),
            (Value::Version("0.10.0".parse().unwrap()), "0.10.0"),
            (
                Value::Asset("nep245:intents.near:nep141:btc.omft.near".into()),
                "nep245:intents.near:nep141:btc.omft.near",
            ),
        ] {
            assert_eq!(
                serde_json::to_value(&value).unwrap(),
                serde_json::Value::String(text.into())
            );
        }
    }
}
