//! Preflight checks over a [`MarketSpec`].
//!
//! Every check has a stable id. `--skip-check <id>` (ENG-544) and the plan
//! artifact both key on these, so ids are part of the tool's contract: renaming
//! one breaks a caller's skip list.
//!
//! This module holds only the checks that need no network. The on-chain reads
//! (ENG-541), aggregation dry-run (ENG-542), and reference cross-check
//! (ENG-543) register alongside them later.

use serde::Serialize;

use super::MarketSpec;

/// A check's verdict. `Skipped` is distinct from `Passed` so a report can never
/// present "not run" as "fine".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

    pub const fn is_failure(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    /// Stable, dotted id — e.g. `config.validate`.
    pub id: String,
    #[serde(flatten)]
    pub status: Status,
}

impl Check {
    fn new(id: impl Into<String>, status: Status) -> Self {
        Self {
            id: id.into(),
            status,
        }
    }
}

/// Run every offline check.
///
/// Decimals are supplied because `config.validate` needs a full
/// [`templar_common::market::MarketConfiguration`], and resolving them is an
/// on-chain read (ENG-541). Until that lands, the spec's own `decimals`
/// override is the only source, and its absence is reported rather than
/// guessed.
pub fn run_offline(spec: &MarketSpec) -> Vec<Check> {
    vec![
        assets_distinct(spec),
        sources(spec),
        validate_configuration(spec),
    ]
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
    let mut problems = Vec::new();
    // The two sides are distinct types (`AssetSpec<CollateralAsset>` /
    // `AssetSpec<BorrowAsset>`), so they cannot share a loop.
    source_problems("collateral", &spec.collateral, &mut problems);
    source_problems("borrow", &spec.borrow, &mut problems);

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
    if asset.sources.iter().all(|source| source.weight() == 0) {
        problems.push(format!("{side} sources all have weight 0"));
    }
}

/// The contract's own invariants — mcr ordering, the interest-rate ceiling at
/// full utilisation, the liquidation-spread bound, withdrawal-range coherence.
///
/// These already run at market init. Running them here moves the failure from
/// "after governance and the oracle are deployed and ~8.5 NEAR is spent" to
/// "before the first transaction".
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
