//! Pre-upgrade preflight for a deployed proxy oracle.
//!
//! The Halborn remediation (#595) changed no persisted layout, so an upgrade needs no state
//! migration. It did add validation that the new runtime applies to *already-stored* state when it
//! loads it. These checks assert that stored state satisfies it, before new code is allowed to run
//! against it.
//!
//! Each check reuses the contract's own validator rather than restating its rules, so the two
//! cannot drift. Reads are deliberately untyped where a deployed contract may still serialize a
//! pre-upgrade shape: a body the new types reject is the finding, not a transport error.

use anyhow::Context as _;
use near_account_id::AccountId;
use serde::Serialize;
use serde_json::Value;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_methods_spec::{
    contract, proxy_oracle, proxy_oracle_governance as gov, registry,
};
use templar_gateway_types::common::{ContractArgs, Pagination};
use templar_gateway_types::contract::ContractKind;
use templar_proxy_oracle_kernel::proxy::{circuit_breaker::CircuitBreakerSet, Proxy};
use templar_proxy_oracle_near_common::{input::Source, proxy::has_zero_weighted_source};
use templar_proxy_oracle_near_governance_common::{
    governance_kernel::Governance, target::method, MAX_PENDING_PROPOSALS,
};

use crate::commands::proxy_oracle::{Preflight, PreflightArgs};
use crate::context::{print_json, CliContext};
use crate::report::Reporter;
use crate::spec::check::{Check, Status};

/// Args for a view keyed by price id. A struct rather than an ad-hoc document so the field name
/// stays checked against the contract's signature.
#[derive(Serialize)]
struct PriceIdArgs {
    id: PriceIdentifier,
}

/// Args for a view keyed by proposal id.
#[derive(Serialize)]
struct ProposalIdArgs {
    id: u32,
}

/// What the new runtime would make of one asset's stored breaker set.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BreakerVerdict {
    /// Decodes, and satisfies the rules enforced at load.
    Usable,
    /// The new runtime would treat this set as absent: reads return `None`, `update_prices` caches
    /// `ResolveFailed`, and the admin breaker methods panic.
    Rejected(String),
}

/// Classify one `get_proxy_circuit_breaker_set` response.
///
/// A body that fails to decode is itself a finding: a deployed `WindowedChangeDelta` still carries
/// the pre-upgrade `max_relative_change_delta` field name, and that rule's semantics changed under
/// the same stored bytes.
fn classify_breaker_set(raw: &Value) -> BreakerVerdict {
    if raw.is_null() {
        return BreakerVerdict::Rejected("no breaker set stored for a configured proxy".to_owned());
    }
    match serde_json::from_value::<CircuitBreakerSet>(raw.clone()) {
        Ok(set) => match set.validate() {
            Ok(()) => BreakerVerdict::Usable,
            Err(error) => BreakerVerdict::Rejected(error.to_string()),
        },
        Err(error) => BreakerVerdict::Rejected(format!("does not decode: {error}")),
    }
}

fn breakers_status(verdicts: &[(PriceIdentifier, BreakerVerdict)]) -> Status {
    let rejected: Vec<String> = verdicts
        .iter()
        .filter_map(|(price_id, verdict)| match verdict {
            BreakerVerdict::Usable => None,
            BreakerVerdict::Rejected(reason) => Some(format!("{price_id}: {reason}")),
        })
        .collect();

    if rejected.is_empty() {
        return Status::passed(format!("{} breaker set(s) load cleanly", verdicts.len()));
    }
    Status::failed(format!(
        "{} breaker set(s) the new runtime would reject — recover each with \
         `admin_set_proxy <id> null` then re-set, since the admin breaker methods panic on an \
         invalid set: {}",
        rejected.len(),
        rejected.join("; "),
    ))
}

fn source_weights_status(proxies: &[(PriceIdentifier, Proxy<Source>)]) -> Status {
    let offenders: Vec<String> = proxies
        .iter()
        .filter(|(_, proxy)| has_zero_weighted_source(proxy))
        .map(|(price_id, _)| price_id.to_string())
        .collect();

    if offenders.is_empty() {
        return Status::passed(format!("{} proxy(s) weight every source", proxies.len()));
    }
    Status::failed(format!(
        "{} proxy(s) carry a zero-weight median source, which no longer counts toward quorum and \
         can start failing as `TooFewValidSources`: {}",
        offenders.len(),
        offenders.join(", "),
    ))
}

/// Validate the ledger through the same constructor the contract's `BorshDeserialize` routes
/// through. `ttls` are irrelevant to the three invariants, so the unit policy stands in for them.
fn governance_ledger_status(next_id: u32, active_ids: &[u32]) -> Status {
    let active: Vec<u64> = active_ids.iter().copied().map(u64::from).collect();
    match Governance::try_from_parts(u64::from(next_id), active, (), MAX_PENDING_PROPOSALS) {
        Ok(_) => Status::passed(format!(
            "{} pending proposal(s), next id {next_id}",
            active_ids.len()
        )),
        Err(error) => Status::failed(format!(
            "the stored ledger no longer deserializes ({error}), which would brick every \
             governance method including `migrate`: next_id {next_id}, active {active_ids:?}"
        )),
    }
}

/// Whether a proposal body dispatches a rearm, in either the current or the pre-upgrade encoding.
///
/// Untyped because a v0 governance deployment answers with an operation enum the current types
/// reject outright, and that body still has to be recognized.
fn is_rearm(operation: &Value) -> bool {
    if operation.get("Rearm").is_some() {
        return true;
    }
    operation
        .get("TargetFunctionCall")
        .and_then(|call| call.get("method_name"))
        .and_then(Value::as_str)
        .is_some_and(|name| name == method::REARM)
}

fn pending_rearm_status(bodies: &[(u32, Value)]) -> Status {
    let queued: Vec<String> = bodies
        .iter()
        .filter(|(_, body)| body.get("operation").is_some_and(is_rearm))
        .map(|(id, _)| id.to_string())
        .collect();

    if queued.is_empty() {
        return Status::passed(format!(
            "no rearm among {} pending proposal(s)",
            bodies.len()
        ));
    }
    Status::failed(format!(
        "{} pending rearm proposal(s) carry the old `armed_after_ns` payload and cannot execute \
         against the new `admin_rearm`; cancel and requeue them: {}",
        queued.len(),
        queued.join(", "),
    ))
}

enum GovernanceResolution<'a> {
    Resolved(Option<&'a AccountId>),
    Failed(&'a anyhow::Error),
}

fn governance_resolution_failure_checks(error: &anyhow::Error) -> [Check; 2] {
    let detail = format!("could not resolve the governing contract: {error:#}");
    [
        Check::new("upgrade.governance_ledger", Status::failed(detail.clone())),
        Check::new("upgrade.pending_rearm", Status::failed(detail)),
    ]
}

/// Every check, recorded in a stable order. Nothing propagates: a failed read is a failed *check*,
/// so one dead RPC cannot hide the rest.
///
/// Governance checks are skipped only when no governance contract owns the oracle.
async fn run(
    ctx: &CliContext,
    oracle_id: &AccountId,
    governance: GovernanceResolution<'_>,
    reporter: &mut Reporter,
) {
    reporter.phase(&format!("stored state on {oracle_id}"));
    match price_ids(ctx, oracle_id).await {
        Ok(ids) => {
            reporter.record(Check::new(
                "upgrade.breakers",
                breakers(ctx, oracle_id, &ids).await,
            ));
            reporter.record(Check::new(
                "upgrade.source_weights",
                source_weights(ctx, oracle_id, &ids).await,
            ));
        }
        Err(error) => {
            let detail = format!("{error:#}");
            reporter.record(Check::new(
                "upgrade.breakers",
                Status::failed(detail.clone()),
            ));
            reporter.record(Check::new("upgrade.source_weights", Status::failed(detail)));
        }
    }

    let governance_id = match governance {
        GovernanceResolution::Resolved(Some(governance_id)) => governance_id,
        GovernanceResolution::Resolved(None) => {
            let reason = format!("{oracle_id} is not owned by a governance contract");
            reporter.record(Check::new(
                "upgrade.governance_ledger",
                Status::Skipped {
                    reason: reason.clone(),
                },
            ));
            reporter.record(Check::new(
                "upgrade.pending_rearm",
                Status::Skipped { reason },
            ));
            return;
        }
        GovernanceResolution::Failed(error) => {
            reporter.extend(governance_resolution_failure_checks(error));
            return;
        }
    };

    reporter.phase(&format!("governance ledger on {governance_id}"));
    match proposal_ids(ctx, governance_id).await {
        Ok(ids) => {
            reporter.record(Check::new(
                "upgrade.governance_ledger",
                governance_ledger(ctx, governance_id, &ids).await,
            ));
            reporter.record(Check::new(
                "upgrade.pending_rearm",
                pending_rearm(ctx, governance_id, &ids).await,
            ));
        }
        Err(error) => {
            let detail = format!("{error:#}");
            reporter.record(Check::new(
                "upgrade.governance_ledger",
                Status::failed(detail.clone()),
            ));
            reporter.record(Check::new("upgrade.pending_rearm", Status::failed(detail)));
        }
    }
}

async fn price_ids(
    ctx: &CliContext,
    oracle_id: &AccountId,
) -> anyhow::Result<Vec<PriceIdentifier>> {
    Ok(ctx
        .client
        .read(proxy_oracle::ListProxies {
            oracle_id: oracle_id.clone(),
            offset: None,
            count: None,
        })
        .await
        .with_context(|| format!("list proxies on {oracle_id}"))?
        .proxies)
}

/// Read the breaker set as raw JSON rather than through the typed view: a set the current types
/// reject is exactly what this check exists to report, and the typed read would surface it as a
/// transport failure instead.
async fn breakers(ctx: &CliContext, oracle_id: &AccountId, ids: &[PriceIdentifier]) -> Status {
    let mut verdicts = Vec::with_capacity(ids.len());
    for id in ids {
        let args = match serde_json::to_value(PriceIdArgs { id: *id }) {
            Ok(args) => args,
            Err(error) => return Status::failed(format!("encode price id {id}: {error}")),
        };
        let read = ctx
            .client
            .read(contract::ViewFunction {
                contract_id: oracle_id.clone(),
                method_name: "get_proxy_circuit_breaker_set".to_owned().into(),
                args: ContractArgs::Json(args),
            })
            .await;
        match read {
            Ok(result) => verdicts.push((*id, classify_breaker_set(&result.value))),
            Err(error) => {
                return Status::failed(format!("read breaker set for {id} on {oracle_id}: {error}"))
            }
        }
    }
    breakers_status(&verdicts)
}

async fn source_weights(
    ctx: &CliContext,
    oracle_id: &AccountId,
    ids: &[PriceIdentifier],
) -> Status {
    let mut proxies = Vec::with_capacity(ids.len());
    for id in ids {
        match ctx
            .client
            .read(proxy_oracle::GetProxy {
                oracle_id: oracle_id.clone(),
                id: *id,
            })
            .await
        {
            // A listed id whose proxy reads back absent raced a `remove_proxy`; nothing to weigh.
            Ok(result) => proxies.extend(result.proxy.map(|proxy| (*id, proxy))),
            Err(error) => {
                return Status::failed(format!("read proxy {id} on {oracle_id}: {error}"))
            }
        }
    }
    source_weights_status(&proxies)
}

async fn governance_ledger(
    ctx: &CliContext,
    governance_id: &AccountId,
    active_ids: &[u32],
) -> Status {
    match ctx
        .client
        .read(gov::NextProposalId {
            governance_id: governance_id.clone(),
        })
        .await
    {
        Ok(next_id) => governance_ledger_status(next_id, active_ids),
        Err(error) => Status::failed(format!("read next proposal id: {error}")),
    }
}

/// Read every pending proposal body as raw JSON: a v0 governance deployment answers with an
/// operation enum the current types reject, and those bodies still have to be inspected.
async fn pending_rearm(ctx: &CliContext, governance_id: &AccountId, ids: &[u32]) -> Status {
    let mut bodies = Vec::with_capacity(ids.len());
    for &id in ids {
        let args = match serde_json::to_value(ProposalIdArgs { id }) {
            Ok(args) => args,
            Err(error) => return Status::failed(format!("encode proposal id {id}: {error}")),
        };
        let read = ctx
            .client
            .read(contract::ViewFunction {
                contract_id: governance_id.clone(),
                method_name: "get_proposal".to_owned().into(),
                args: ContractArgs::Json(args),
            })
            .await;
        match read {
            Ok(result) => bodies.push((id, result.value)),
            Err(error) => {
                return Status::failed(format!("read proposal {id} on {governance_id}: {error}"))
            }
        }
    }
    pending_rearm_status(&bodies)
}

async fn proposal_ids(ctx: &CliContext, governance_id: &AccountId) -> anyhow::Result<Vec<u32>> {
    Ok(ctx
        .client
        .read(gov::ListProposals {
            governance_id: governance_id.clone(),
            offset: None,
            count: None,
        })
        .await
        .with_context(|| format!("list proposals on {governance_id}"))?
        .ids)
}

/// What a report covers: one oracle, and the governance contract that owns it when there is one.
#[derive(Serialize)]
struct Subject {
    oracle_id: AccountId,
    #[serde(skip_serializing_if = "Option::is_none")]
    governance_id: Option<AccountId>,
}

/// One oracle's verdicts, so a fleet sweep can emit a single document rather than one per account.
#[derive(Serialize)]
struct OracleReport {
    #[serde(flatten)]
    subject: Subject,
    checks: Vec<Check>,
}

/// Every proxy oracle a run covered.
#[derive(Serialize)]
struct FleetReport<'a> {
    oracles: &'a [OracleReport],
}

/// Run the checks against `oracle_id`, streaming verdicts to stderr and returning them.
///
/// Stdout is left alone: this runs inside commands that own that channel for their own result, and
/// two JSON documents on one stream is not a format anything can read.
async fn report(
    ctx: &CliContext,
    oracle_id: &AccountId,
    skip: &[String],
) -> anyhow::Result<OracleReport> {
    let governance_id = crate::resolve::governance_from_oracle(ctx, oracle_id).await;
    let governance = match governance_id.as_ref() {
        Ok(governance_id) => GovernanceResolution::Resolved(governance_id.as_ref()),
        Err(error) => GovernanceResolution::Failed(error),
    };
    let mut reporter = ctx.reporter(skip);
    run(ctx, oracle_id, governance, &mut reporter).await;
    reporter.ensure_every_skip_matched()?;
    reporter.digest();

    Ok(OracleReport {
        subject: Subject {
            oracle_id: oracle_id.clone(),
            governance_id: governance_id.ok().flatten(),
        },
        checks: reporter.into_checks(),
    })
}

/// The gate every upgrade path applies before it submits. `Err` unless every check passed, so a
/// caller that `?`s it cannot proceed.
pub(super) async fn gate(
    ctx: &CliContext,
    oracle_id: &AccountId,
    skip: &[String],
) -> anyhow::Result<()> {
    let report = report(ctx, oracle_id, skip).await?;
    crate::spec::check::gate(
        &report.checks,
        &format!("proxy oracle {oracle_id}"),
        "the upgrade was not submitted",
    )
}

/// Gate an upgrade that reaches the oracle through its governance contract.
///
/// Resolves the governed oracle from `governance_id`, so the caller does not have to know it.
pub(super) async fn gate_governed_oracle(
    ctx: &CliContext,
    governance_id: &AccountId,
    args: &PreflightArgs,
) -> anyhow::Result<()> {
    let oracle_id = ctx
        .client
        .read(gov::GetProxyOracleId {
            governance_id: governance_id.clone(),
        })
        .await
        .with_context(|| format!("read the oracle governed by {governance_id}"))?
        .proxy_oracle_id;
    gate(ctx, &oracle_id, &args.skip_check).await
}

/// Gate the execution of an already-queued proposal, which is a no-op unless that proposal is what
/// upgrades the oracle.
///
/// Re-checking here is the point: a proposal matures on a timelock, so state can drift between
/// the create that first ran these checks and the execute that lands the new code.
pub(super) async fn gate_queued_upgrade(
    ctx: &CliContext,
    governance_id: &AccountId,
    proposal_id: u32,
    args: &PreflightArgs,
) -> anyhow::Result<()> {
    if !upgrades_the_oracle(ctx, governance_id, proposal_id).await? {
        return Ok(());
    }
    gate_governed_oracle(ctx, governance_id, args).await
}

/// Whether proposal `proposal_id` dispatches an upgrade at the governed oracle.
async fn upgrades_the_oracle(
    ctx: &CliContext,
    governance_id: &AccountId,
    proposal_id: u32,
) -> anyhow::Result<bool> {
    let args = serde_json::to_value(ProposalIdArgs { id: proposal_id })?;
    let body = ctx
        .client
        .read(contract::ViewFunction {
            contract_id: governance_id.clone(),
            method_name: "get_proposal".to_owned().into(),
            args: ContractArgs::Json(args),
        })
        .await
        .with_context(|| format!("read proposal {proposal_id} on {governance_id}"))?
        .value;

    Ok(body
        .get("operation")
        .and_then(|operation| operation.get("TargetFunctionCall"))
        .and_then(|call| call.get("method_name"))
        .and_then(Value::as_str)
        .is_some_and(|name| name == method::UPGRADE))
}

/// `proxy-oracle preflight` — the same checks on their own, for sweeping the fleet before anyone
/// composes an upgrade and for re-checking after remediating one.
pub(super) async fn command(ctx: CliContext, args: Preflight) -> anyhow::Result<()> {
    let oracle_ids = resolve_targets(&ctx, &args).await?;
    anyhow::ensure!(
        !oracle_ids.is_empty(),
        "no proxy oracle deployments to check"
    );

    // Every oracle is checked before anything is reported, so one bad deployment cannot hide the
    // rest of the fleet.
    let mut oracles = Vec::with_capacity(oracle_ids.len());
    for oracle_id in &oracle_ids {
        oracles.push(report(&ctx, oracle_id, &args.skip_check).await?);
    }
    print_json(&FleetReport { oracles: &oracles })?;

    let failed: Vec<&str> = oracles
        .iter()
        .filter(|oracle| crate::spec::check::failures(&oracle.checks) > 0)
        .map(|oracle| oracle.subject.oracle_id.as_str())
        .collect();
    anyhow::ensure!(
        failed.is_empty(),
        "{} of {} proxy oracle(s) failed preflight: {}",
        failed.len(),
        oracles.len(),
        failed.join(", "),
    );
    Ok(())
}

/// One explicit oracle, or every proxy oracle a registry has deployed.
async fn resolve_targets(ctx: &CliContext, args: &Preflight) -> anyhow::Result<Vec<AccountId>> {
    let Some(registry_id) = &args.registry_id else {
        return Ok(vec![args.target().resolve(ctx).await?]);
    };
    ctx.client
        .read(registry::ListDeploymentsByKind {
            registry_id: registry_id.clone(),
            args: Pagination::default(),
            kind: ContractKind::ProxyOracle,
        })
        .await
        .map(|result| result.account_ids)
        // Registries deployed before the kind-filtered listing view cannot enumerate anything, and
        // the mainnet one is among them — so say what to do instead rather than surfacing a bare
        // `MethodNotFound` from a view the operator never named.
        .map_err(|error| {
            anyhow::anyhow!(
                "could not list proxy oracles deployed by {registry_id}: {error}. A registry \
                 predating `list_deployments_by_kind` cannot be swept; pass `--oracle-id` (or \
                 `--market-id`) once per oracle instead."
            )
        })
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use serde_json::json;
    use templar_common::Decimal;
    use templar_common::Nanoseconds;
    use templar_proxy_oracle_kernel::proxy::{
        aggregator::{
            method::{median::Median, priority::Priority},
            Aggregator,
        },
        circuit_breaker::{
            CircuitBreaker, CircuitBreakerSet, CircuitBreakerSetConfig, MonotonicRun,
        },
        FreshnessFilter, Proxy, WeightedSource,
    };
    use templar_proxy_oracle_near_common::request::OracleRequest;
    use templar_proxy_oracle_near_governance_common::MAX_PENDING_PROPOSALS;

    use super::{
        breakers_status, classify_breaker_set, governance_ledger_status,
        governance_resolution_failure_checks, is_rearm, pending_rearm_status,
        source_weights_status, BreakerVerdict, PriceIdentifier, Source, Status, Value,
    };

    fn price_id(byte: u8) -> PriceIdentifier {
        PriceIdentifier([byte; 32])
    }

    fn source() -> Source {
        OracleRequest::redstone("redstone.near".parse().unwrap(), "BTC").into()
    }

    fn proxy(weights: &[u32]) -> Proxy<Source> {
        Proxy {
            aggregator: Aggregator::MedianLow(Median::new(
                weights
                    .iter()
                    .map(|weight| WeightedSource::new(source(), *weight)),
            )),
            freshness_filter: FreshnessFilter::new(None, None),
        }
    }

    fn breaker_set(history_len: u32) -> CircuitBreakerSet {
        CircuitBreakerSet::new(CircuitBreakerSetConfig {
            sample_interval_ns: Nanoseconds::zero(),
            history_len,
        })
    }

    /// A set the new runtime accepts: an empty one, which is what `set_proxy` writes.
    #[test]
    fn an_empty_breaker_set_is_usable() {
        let empty: CircuitBreakerSet = CircuitBreakerSet::empty();
        let raw = serde_json::to_value(empty).unwrap();
        assert_eq!(classify_breaker_set(&raw), BreakerVerdict::Usable);
    }

    /// `MonotonicRun` now requires an unsampled history, so a set pairing the two is inert.
    #[test]
    fn a_monotonic_run_under_a_sample_interval_is_rejected() {
        let mut set = breaker_set(4);
        set.add(
            0,
            CircuitBreaker::MonotonicRun(MonotonicRun {
                max_streak: 1,
                min_relative_step_change: Decimal::ONE_HALF,
            }),
        )
        .unwrap();

        let mut raw = serde_json::to_value(set).unwrap();
        raw["sample_interval_ns"] = json!("1000000000");

        assert!(matches!(
            classify_breaker_set(&raw),
            BreakerVerdict::Rejected(_)
        ));
    }

    /// A relative threshold above 100% can never be reached, so it is no longer installable.
    #[test]
    fn a_threshold_above_one_is_rejected() {
        let mut raw = serde_json::to_value(breaker_set(4)).unwrap();
        raw["next_id"] = json!(1);
        raw["breakers"] = json!({
            "0": {
                "breaker": { "StepwiseChange": { "max_relative_change": "2" } },
                "is_enforced": true,
                "status": { "ArmedAfter": { "timestamp_ns": "0" } },
            }
        });

        assert!(matches!(
            classify_breaker_set(&raw),
            BreakerVerdict::Rejected(_)
        ));
    }

    /// The case this check exists for: a deployed `WindowedChangeDelta` still carries the old field
    /// name, so the body does not decode at all.
    #[test]
    fn a_pre_upgrade_windowed_rule_is_rejected_rather_than_erroring() {
        let mut raw = serde_json::to_value(breaker_set(8)).unwrap();
        raw["next_id"] = json!(1);
        raw["breakers"] = json!({
            "0": {
                "breaker": { "WindowedChangeDelta": {
                    "window_len": 2,
                    "lookback_windows": 1,
                    "max_relative_change_delta": "0.1",
                } },
                "is_enforced": true,
                "status": { "ArmedAfter": { "timestamp_ns": "0" } },
            }
        });

        let BreakerVerdict::Rejected(reason) = classify_breaker_set(&raw) else {
            panic!("a pre-upgrade windowed rule must be rejected");
        };
        assert!(reason.contains("does not decode"), "{reason}");
    }

    /// A configured proxy with no stored set fails closed: the new runtime panics on it.
    #[test]
    fn a_missing_breaker_set_is_rejected() {
        assert!(matches!(
            classify_breaker_set(&Value::Null),
            BreakerVerdict::Rejected(_)
        ));
    }

    #[test]
    fn breakers_status_names_every_rejected_feed() {
        let status = breakers_status(&[
            (price_id(1), BreakerVerdict::Usable),
            (price_id(2), BreakerVerdict::Rejected("inert".to_owned())),
        ]);
        let Status::Failed { detail } = status else {
            panic!("a rejected set must fail the check");
        };
        assert!(detail.contains(&price_id(2).to_string()), "{detail}");
        assert!(!detail.contains(&price_id(1).to_string()), "{detail}");
    }

    #[rstest]
    #[case::weighted(&[1, 1], false)]
    #[case::zero_weighted(&[1, 0], true)]
    fn source_weights_status_flags_only_zero_weights(#[case] weights: &[u32], #[case] fails: bool) {
        let status = source_weights_status(&[(price_id(1), proxy(weights))]);
        assert_eq!(status.is_failure(), fails, "{status:?}");
    }

    /// `Priority` carries no weights, so it can never trip this check.
    #[test]
    fn a_priority_aggregator_is_never_flagged() {
        let proxy = Proxy {
            aggregator: Aggregator::Priority(Priority::new(vec![source()])),
            freshness_filter: FreshnessFilter::new(None, None),
        };
        assert!(!source_weights_status(&[(price_id(1), proxy)]).is_failure());
    }

    #[rstest]
    #[case::sound(3, &[0, 1], false)]
    #[case::empty(0, &[], false)]
    #[case::out_of_order(3, &[1, 0], true)]
    #[case::duplicated(3, &[1, 1], true)]
    #[case::equal_to_next(3, &[3], true)]
    #[case::beyond_next(3, &[4], true)]
    fn governance_ledger_status_enforces_the_stored_invariants(
        #[case] next_id: u32,
        #[case] active_ids: &[u32],
        #[case] fails: bool,
    ) {
        let status = governance_ledger_status(next_id, active_ids);
        assert_eq!(status.is_failure(), fails, "{status:?}");
    }

    #[test]
    fn governance_ledger_status_rejects_more_pending_than_the_cap() {
        let active_ids: Vec<u32> = (0..=MAX_PENDING_PROPOSALS).collect();
        let next_id = u32::try_from(active_ids.len()).unwrap();
        assert!(governance_ledger_status(next_id, &active_ids).is_failure());
    }

    #[test]
    fn governance_resolution_failure_fails_both_governance_checks() {
        let error = anyhow::anyhow!("proxy RPC unavailable");
        let checks = governance_resolution_failure_checks(&error);

        assert_eq!(checks[0].id, "upgrade.governance_ledger");
        assert_eq!(checks[1].id, "upgrade.pending_rearm");
        for check in checks {
            let Status::Failed { detail } = check.status else {
                panic!("a resolution failure must fail the check");
            };
            assert!(detail.contains("proxy RPC unavailable"), "{detail}");
        }
    }

    #[rstest]
    #[case::current(json!({"TargetFunctionCall": {"method_name": "admin_rearm"}}), true)]
    #[case::legacy(json!({"Rearm": {"breaker_id": 0}}), true)]
    #[case::other_target(json!({"TargetFunctionCall": {"method_name": "admin_set_proxy"}}), false)]
    #[case::reflexive(json!({"Reflexive": {"SetRole": {}}}), false)]
    fn is_rearm_recognizes_both_encodings(#[case] operation: Value, #[case] expected: bool) {
        assert_eq!(is_rearm(&operation), expected);
    }

    #[test]
    fn pending_rearm_status_names_the_proposals_to_cancel() {
        let bodies = vec![
            (
                7,
                json!({"operation": {"TargetFunctionCall": {"method_name": "admin_rearm"}}}),
            ),
            (
                8,
                json!({"operation": {"TargetFunctionCall": {"method_name": "admin_upgrade"}}}),
            ),
        ];
        let Status::Failed { detail } = pending_rearm_status(&bodies) else {
            panic!("a queued rearm must fail the check");
        };
        assert!(detail.contains('7'), "{detail}");
        assert!(!detail.contains('8'), "{detail}");
    }

    #[test]
    fn pending_rearm_status_passes_an_empty_queue() {
        assert!(!pending_rearm_status(&[]).is_failure());
    }
}
