//! Offline preflight checks over a [`MarketSpec`]; the on-chain ones live in
//! [`crate::dispatch::preflight`].
//!
//! Check ids are part of the tool's contract: `--skip-check` and the plan
//! artifact both key on them, so renaming one breaks a caller's skip list.

use serde::{Deserialize, Serialize};

use super::MarketSpec;

#[cfg(test)]
pub use super::oracle::AggregatorSpec;

/// A check's verdict. `Skipped` is distinct from `Passed` so a report can never
/// present "not run" as "fine".
///
/// `Deserialize` because the plan artifact (ENG-544) embeds these and is read
/// back on apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum Status {
    Passed { detail: String },
    Failed { detail: String },
    Skipped { reason: String },
}

impl Status {
    pub fn passed(detail: impl Into<String>) -> Self {
        Self::Passed {
            detail: detail.into(),
        }
    }

    pub fn failed(detail: impl Into<String>) -> Self {
        Self::Failed {
            detail: detail.into(),
        }
    }

    pub fn skipped(reason: impl Into<String>) -> Self {
        Self::Skipped {
            reason: reason.into(),
        }
    }

    pub const fn is_failure(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    /// Stable, dotted id — e.g. `config.validate`.
    pub id: String,
    #[serde(flatten)]
    pub status: Status,
}

impl Check {
    pub(crate) fn new(id: impl Into<String>, status: Status) -> Self {
        Self {
            id: id.into(),
            status,
        }
    }
}

/// How many checks failed.
pub fn failures(checks: &[Check]) -> usize {
    checks
        .iter()
        .filter(|check| check.status.is_failure())
        .count()
}

/// A command's check report, as printed.
///
/// A struct rather than an ad-hoc document so the two commands that emit it
/// cannot drift, and so a consumer has a shape to deserialize.
#[derive(Debug, Serialize)]
pub struct Report<'a, T: Serialize> {
    #[serde(flatten)]
    pub subject: T,
    pub checks: &'a [Check],
}

/// The gate every command applies to its checks. `subject` names what was
/// checked and `consequence` what did not happen; only the caller knows either.
pub fn gate(checks: &[Check], subject: &str, consequence: &str) -> anyhow::Result<()> {
    let failed = failures(checks);
    anyhow::ensure!(
        failed == 0,
        "{failed} check(s) failed for {subject}; {consequence}. Fix the spec, or \
         re-run with `--skip-check <id>` for a check that is wrong."
    );
    Ok(())
}

/// Run every offline check.
pub fn run_offline(spec: &MarketSpec) -> Vec<Check> {
    vec![
        assets_distinct(spec),
        sources(spec),
        mode_is_fully_described(spec),
        validate_configuration(spec),
    ]
}

/// One side's sources, without needing the caller to name the generic asset
/// class twice.
fn asset_sources<'a>(spec: &'a MarketSpec, side: &str) -> &'a [super::oracle::SourceSpec] {
    if side == "collateral" {
        &spec.collateral.sources
    } else {
        &spec.borrow.sources
    }
}

/// Fields belonging to the other oracle mode are refused rather than ignored:
/// both modes share one `AssetSpec`, so an author who wrote `sources` on a
/// direct spec would otherwise believe they are being aggregated.
fn mode_is_fully_described(spec: &MarketSpec) -> Check {
    let id = "config.oracle_mode";
    let direct = spec.oracle.is_direct();
    let mut problems = Vec::new();

    for (side, price_id, sources, aggregator, min_sources, max_age, max_clock_drift) in [
        (
            "collateral",
            spec.collateral.price_id,
            spec.collateral.sources.len(),
            spec.collateral.aggregator,
            spec.collateral.min_sources,
            spec.collateral.max_age,
            spec.collateral.max_clock_drift,
        ),
        (
            "borrow",
            spec.borrow.price_id,
            spec.borrow.sources.len(),
            spec.borrow.aggregator,
            spec.borrow.min_sources,
            spec.borrow.max_age,
            spec.borrow.max_clock_drift,
        ),
    ] {
        // The omission that silently changes behavior: no aggregator deploys
        // `median_low`, where every deployed borrow feed reads `median_high`.
        if !direct && aggregator.is_none() {
            problems.push(format!(
                "{side} states no `aggregator`; a proxy would silently deploy \
                 `median_low`, more permissive than the `median_high` every \
                 deployed borrow feed uses"
            ));
        }
        if direct && aggregator.is_some() {
            problems.push(format!(
                "{side}.aggregator is set, but this market aggregates nothing; \
                 it would be ignored"
            ));
        }
        // Required, and checked here rather than left to `config.validate`:
        // that check skips itself when decimals are unresolved, so an offline
        // run of a direct spec missing a `price_id` exited zero.
        if direct && price_id.is_none() {
            problems.push(format!(
                "{side} states no `price_id`; a pre-existing oracle serves its \
                 own identifiers and this spec has nothing to derive one from"
            ));
        }
        if direct && min_sources > 0 {
            problems.push(format!(
                "{side}.min_sources is set, but this market aggregates nothing; \
                 it would be ignored"
            ));
        }
        if direct && (max_age.is_some() || max_clock_drift.is_some()) {
            problems.push(format!(
                "{side} sets a freshness bound, but the oracle it reads enforces \
                 its own; it would be ignored"
            ));
        }
        // `priority` ranks its sources by position: it carries no weights and
        // no minimum. Accepting either would silently drop what was authored,
        // the same failure the mode fields have.
        if !direct {
            let weighted = aggregator.unwrap_or_default().is_weighted();
            let stated: Vec<_> = asset_sources(spec, side)
                .iter()
                .filter_map(|source| source.weight())
                .collect();
            if weighted && stated.len() != sources {
                problems.push(format!(
                    "{side} uses a median aggregator, which weighs its sources, \
                     but only {} of {sources} state a `weight`",
                    stated.len()
                ));
            }
            if !weighted && !stated.is_empty() {
                problems.push(format!(
                    "{side} uses `priority`, which ranks sources by position; \
                     the {} `weight`(s) stated would be ignored",
                    stated.len()
                ));
            }
            if !weighted && min_sources > 0 {
                problems.push(format!(
                    "{side}.min_sources is set, but `priority` takes the first \
                     source that answers and honors no minimum"
                ));
            }
        }
        if direct && sources > 0 {
            problems.push(format!(
                "{side} names {sources} source(s), but this market reads an \
                 oracle it does not configure; they would be ignored"
            ));
        }
        if !direct && price_id.is_some() {
            problems.push(format!(
                "{side}.price_id is set, but a proxy oracle serves the constants \
                 this tool owns; it would be ignored"
            ));
        }
    }

    if problems.is_empty() {
        Check::new(id, Status::passed(if direct { "direct" } else { "proxy" }))
    } else {
        Check::new(id, Status::failed(problems.join("; ")))
    }
}

/// What the chain said about a token's decimals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnChainDecimals {
    /// `ft_metadata`/`mt_metadata` answered.
    Known(u8),
    /// The contract exists but its metadata is missing or unreadable. Real: at
    /// least one bridged asset shipped without `ft_metadata` populated.
    Unavailable,
}

/// Reconcile the spec's optional `decimals` override against the chain.
///
/// Pure, so the whole matrix is unit-testable without a network — the IO around
/// it is a single view call.
pub fn reconcile_decimals(
    side: &str,
    declared: Option<u8>,
    on_chain: OnChainDecimals,
    accept_mismatch: bool,
) -> (Status, Option<u8>) {
    match (declared, on_chain) {
        (None, OnChainDecimals::Known(actual)) => (
            Status::passed(format!("{actual} (from token metadata)")),
            Some(actual),
        ),
        (None, OnChainDecimals::Unavailable) => (
            Status::failed(format!(
                "the {side} token publishes no readable decimals; set `{side}.decimals` \
                 in the spec, verified against the asset's source chain"
            )),
            None,
        ),
        (Some(declared), OnChainDecimals::Known(actual)) if declared == actual => (
            Status::passed(format!("{actual}, matching token metadata")),
            Some(actual),
        ),
        (Some(declared), OnChainDecimals::Known(actual)) if accept_mismatch => (
            Status::passed(format!(
                "{declared} declared, overriding {actual} from token metadata \
                 (--accept-decimals-mismatch)"
            )),
            Some(declared),
        ),
        (Some(declared), OnChainDecimals::Known(actual)) => (
            Status::failed(format!(
                "{side}.decimals says {declared} but token metadata says {actual}. \
                 One of them is wrong; pass --accept-decimals-mismatch only if the \
                 spec is right and the token is lying."
            )),
            None,
        ),
        (Some(declared), OnChainDecimals::Unavailable) => (
            // Not a failure: this is exactly the case the override exists for.
            // But it is unverified, and the report must not imply otherwise.
            Status::passed(format!(
                "{declared} declared; token publishes no metadata, so this is unverified"
            )),
            Some(declared),
        ),
    }
}

/// A market whose two sides are the same asset prices itself against itself.
fn assets_distinct(spec: &MarketSpec) -> Check {
    let id = "config.assets_distinct";
    let collateral = spec.collateral.asset.to_string();
    let borrow = spec.borrow.asset.to_string();

    if collateral == borrow {
        return Check::new(
            id,
            Status::failed(format!("collateral and borrow are both `{collateral}`")),
        );
    }
    Check::new(id, Status::passed(format!("{collateral} / {borrow}")))
}

/// `min_sources` above the number of sources can never resolve, and a source
/// set whose weights are all zero has no median to take.
fn sources(spec: &MarketSpec) -> Check {
    let id = "config.sources";
    // A direct market aggregates nothing — whoever owns the oracle it reads
    // configured that — so there are no sources here to be unsatisfiable.
    if spec.oracle.is_direct() {
        return Check::new(
            id,
            Status::Skipped {
                reason: "this market reads an existing oracle, which aggregates \
                         on its own behalf"
                    .to_owned(),
            },
        );
    }
    let mut problems = Vec::new();
    // The two sides are distinct types (`AssetSpec<CollateralAsset>` /
    // `AssetSpec<BorrowAsset>`), so they cannot share a loop.
    source_problems("collateral", &spec.collateral, &mut problems);
    source_problems("borrow", &spec.borrow, &mut problems);
    for (side, asset_min, aggregator) in [
        (
            "collateral",
            spec.collateral.min_sources,
            spec.collateral.aggregator,
        ),
        ("borrow", spec.borrow.min_sources, spec.borrow.aggregator),
    ] {
        // `priority` honors no minimum, and `config.oracle_mode` refuses a
        // priority asset that states one. Requiring a minimum here as well made
        // the two checks contradict each other, so no correctly authored
        // priority spec could pass either.
        if aggregator.unwrap_or_default().is_weighted() && asset_min == 0 {
            problems.push(format!(
                "{side}.min_sources is 0; state it explicitly (every deployed \
                 proxy carries at least 1)"
            ));
        }
    }

    if problems.is_empty() {
        Check::new(
            id,
            Status::passed(format!(
                "{} collateral / {} borrow sources",
                spec.collateral.sources.len(),
                spec.borrow.sources.len()
            )),
        )
    } else {
        Check::new(id, Status::failed(problems.join("; ")))
    }
}

fn source_problems<A: templar_common::asset::AssetClass>(
    side: &str,
    asset: &super::oracle::AssetSpec<A>,
    problems: &mut Vec<String>,
) {
    if asset.sources.is_empty() {
        problems.push(format!("{side} has no sources"));
        return;
    }

    let count = u32::try_from(asset.sources.len()).unwrap_or(u32::MAX);
    if asset.min_sources > count {
        problems.push(format!(
            "{side} requires {} of {count} sources",
            asset.min_sources
        ));
    }
    // Weights only mean something to a median. A `priority` asset ranks by
    // position and carries none, so treating its absent weights as zeroes
    // reported every correctly authored priority spec as unresolvable.
    if asset.aggregator.unwrap_or_default().is_weighted()
        && asset
            .sources
            .iter()
            .all(|source| source.weight().unwrap_or(0) == 0)
    {
        problems.push(format!("{side} sources all have weight 0"));
    }
}

/// The contract's own init-time invariants, run here so the failure lands
/// before the first transaction rather than after ~8.5 NEAR is spent.
fn validate_configuration(spec: &MarketSpec) -> Check {
    let id = "config.validate";

    let (Some(collateral_decimals), Some(borrow_decimals)) =
        (spec.collateral.decimals, spec.borrow.decimals)
    else {
        return Check::new(
            id,
            Status::Skipped {
                reason:
                    "asset decimals resolve on chain (ENG-541); set `decimals` to check offline"
                        .to_owned(),
            },
        );
    };

    let configuration = spec
        .clone()
        .into_market_configuration(i32::from(collateral_decimals), i32::from(borrow_decimals));

    match configuration {
        Err(error) => Check::new(id, Status::failed(format!("{error:#}"))),
        Ok(configuration) => match configuration.validate() {
            Ok(()) => Check::new(id, Status::passed("MarketConfiguration::validate")),
            Err(error) => Check::new(id, Status::failed(error.to_string())),
        },
    }
}
