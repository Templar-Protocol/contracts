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

use std::collections::{btree_map::Entry, BTreeMap, BTreeSet};

use anyhow::Context as _;
use near_account_id::AccountId;
use near_sdk::borsh::object_length;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_methods_spec::{
    account, contract, proxy_oracle, proxy_oracle_governance as gov, registry,
};
use templar_gateway_types::common::{ContractArgs, Pagination};
use templar_gateway_types::contract::ContractKind;
use templar_gateway_types::Base64Bytes;
use templar_proxy_oracle_kernel::proxy::{
    circuit_breaker::{CircuitBreaker, CircuitBreakerSet},
    Proxy,
};
use templar_proxy_oracle_near_common::{input::Source, state::v1::StorageKey as OracleStorageKey};
use templar_proxy_oracle_near_governance_common::{
    cancellation_event_size, governance_kernel::Governance, policy_recovery_working_set_bytes,
    proposal_body_size, target::method, FunctionCall, GovernancePolicy, LegacyOperation,
    LegacyTtlConfig, Operation, MAX_GOVERNANCE_WORKING_SET_BYTES,
    MAX_PENDING_CANCELLATION_LOG_BYTES, MAX_PENDING_PROPOSALS,
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
        "{} breaker set(s) the new runtime would reject — block the upgrade and inspect the \
         exact old code/state and allocator history; removal/recreation must not reset an \
         unknown breaker-ID allocator: {}",
        rejected.len(),
        rejected.join("; "),
    ))
}

fn aggregator_config_status(proxies: &[(PriceIdentifier, Proxy<Source>)]) -> Status {
    let offenders: Vec<String> = proxies
        .iter()
        .filter_map(|(price_id, proxy)| {
            proxy
                .aggregator
                .validate()
                .err()
                .map(|error| format!("{price_id}: {error:?}"))
        })
        .collect();

    if offenders.is_empty() {
        return Status::passed(format!(
            "{} proxy(s) have valid aggregator configurations",
            proxies.len()
        ));
    }
    Status::failed(format!(
        "{} proxy(s) have invalid source shape, quorum, or weights: {}",
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

/// Whether a rearm payload is incompatible with the relative-delay contract.
///
/// Untyped because a v0 governance deployment answers with an operation enum the current types
/// reject outright, and that body still has to be recognized.
fn has_incompatible_rearm_payload(operation: &Value) -> bool {
    if operation.get("Rearm").is_some() {
        return true;
    }
    let Some(call) = operation
        .get("TargetFunctionCall")
        .or_else(|| operation.get("AdminFunctionCall"))
    else {
        return false;
    };
    if call.get("method_name").and_then(Value::as_str) != Some(method::REARM) {
        return false;
    }
    // Generic calls may already carry the compatible relative-delay payload.
    FunctionCall::deserialize(call)
        .ok()
        .and_then(|call| serde_json::from_slice::<Value>(&call.args.0).ok())
        .is_none_or(|args| {
            args.get("arming_delay_ns").is_none() || args.get("armed_after_ns").is_some()
        })
}

fn pending_rearm_status(bodies: &[(u32, Value)]) -> Status {
    let queued: Vec<String> = bodies
        .iter()
        .filter(|(_, body)| {
            body.get("operation")
                .is_some_and(has_incompatible_rearm_payload)
        })
        .map(|(id, _)| id.to_string())
        .collect();

    if queued.is_empty() {
        return Status::passed(format!(
            "no incompatible rearm payloads among {} pending proposal(s)",
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

fn governance_resolution_failure_checks(error: &anyhow::Error) -> [Check; 4] {
    let detail = format!("could not resolve the governing contract: {error:#}");
    [
        Check::new("upgrade.governance_ledger", Status::failed(detail.clone())),
        Check::new("upgrade.pending_rearm", Status::failed(detail.clone())),
        Check::new(
            "upgrade.pending_breaker_lifecycles",
            Status::failed(detail.clone()),
        ),
        Check::new("upgrade.pending_resources", Status::failed(detail)),
    ]
}

/// Every check, recorded in a stable order. Nothing propagates: a failed read is a failed *check*,
/// so one dead RPC cannot hide the rest.
///
/// Governance checks are skipped only when no governance contract owns the oracle.
#[allow(
    clippy::too_many_lines,
    reason = "one ordered reporter pass must record every independent upgrade check"
)]
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
    reporter.record(Check::new(
        "upgrade.removed_feed_allocators",
        removed_feed_allocators(ctx, oracle_id).await,
    ));

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
            for check in [
                "upgrade.pending_rearm",
                "upgrade.pending_breaker_lifecycles",
                "upgrade.pending_resources",
            ] {
                reporter.record(Check::new(
                    check,
                    Status::Skipped {
                        reason: reason.clone(),
                    },
                ));
            }
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
            match pending_proposals(ctx, governance_id, &ids).await {
                Ok(bodies) => {
                    reporter.record(Check::new(
                        "upgrade.pending_rearm",
                        pending_rearm_status(&bodies),
                    ));
                    reporter.record(Check::new(
                        "upgrade.pending_breaker_lifecycles",
                        pending_breaker_lifecycles(ctx, oracle_id, &bodies).await,
                    ));
                    reporter.record(Check::new(
                        "upgrade.pending_resources",
                        pending_resources(ctx, governance_id, &bodies).await,
                    ));
                }
                Err(error) => {
                    for check in [
                        "upgrade.pending_rearm",
                        "upgrade.pending_breaker_lifecycles",
                        "upgrade.pending_resources",
                    ] {
                        reporter.record(Check::new(check, Status::failed(format!("{error:#}"))));
                    }
                }
            }
        }
        Err(error) => {
            let detail = format!("{error:#}");
            reporter.record(Check::new(
                "upgrade.governance_ledger",
                Status::failed(detail.clone()),
            ));
            reporter.record(Check::new(
                "upgrade.pending_rearm",
                Status::failed(detail.clone()),
            ));
            reporter.record(Check::new(
                "upgrade.pending_breaker_lifecycles",
                Status::failed(detail.clone()),
            ));
            reporter.record(Check::new(
                "upgrade.pending_resources",
                Status::failed(detail),
            ));
        }
    }
}

/// Raw `UnorderedMap` keys (`prefix ++ 'i' ++ borsh(key)`) of one oracle map, as price ids.
async fn oracle_map_ids(
    ctx: &CliContext,
    oracle_id: &AccountId,
    map: OracleStorageKey,
) -> anyhow::Result<BTreeSet<PriceIdentifier>> {
    let mut prefix = near_sdk::borsh::to_vec(&map)?;
    prefix.push(b'i');
    let entries = ctx
        .client
        .read(account::ViewState {
            account_id: oracle_id.clone(),
            prefix: prefix.clone().into(),
        })
        .await
        .with_context(|| format!("view state of {oracle_id}"))?;
    entries
        .into_iter()
        .map(|entry| {
            let key = entry
                .key
                .0
                .strip_prefix(prefix.as_slice())
                .context("view_state returned a key outside the requested prefix")?;
            Ok(near_sdk::borsh::from_slice(key)?)
        })
        .collect()
}

/// Feeds whose cache epoch outlived their breaker set. Code before allocator tombstones deleted
/// the set on removal, and current code refuses to recreate such a feed rather than guess its
/// next breaker ID.
fn removed_feed_allocator_status(
    epochs: &BTreeSet<PriceIdentifier>,
    breaker_sets: &BTreeSet<PriceIdentifier>,
) -> Status {
    let orphaned: Vec<String> = epochs
        .difference(breaker_sets)
        .map(ToString::to_string)
        .collect();
    if orphaned.is_empty() {
        Status::passed("every feed with a cache epoch retains its breaker allocator")
    } else {
        Status::warned(format!(
            "after upgrade, admin_set_proxy cannot re-add these removed feeds: old code deleted \
             their breaker allocator and no on-chain path restores it. Keep them removed or plan \
             a further code upgrade: {}",
            orphaned.join(", ")
        ))
    }
}

async fn removed_feed_allocators(ctx: &CliContext, oracle_id: &AccountId) -> Status {
    let ids = async {
        Ok::<_, anyhow::Error>((
            oracle_map_ids(ctx, oracle_id, OracleStorageKey::CacheEpochs).await?,
            oracle_map_ids(ctx, oracle_id, OracleStorageKey::CircuitBreakers).await?,
        ))
    }
    .await;
    match ids {
        Ok((epochs, breaker_sets)) => removed_feed_allocator_status(&epochs, &breaker_sets),
        Err(error) => Status::failed(format!(
            "inventory removed-feed allocators: {error:#}; establish it another way before upgrade"
        )),
    }
}

async fn price_ids(
    ctx: &CliContext,
    oracle_id: &AccountId,
) -> anyhow::Result<Vec<PriceIdentifier>> {
    ctx.client
        .read(proxy_oracle::ListProxies {
            oracle_id: oracle_id.clone(),
            offset: None,
            count: None,
        })
        .await
        .with_context(|| format!("list proxies on {oracle_id}"))
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
            Ok(result) => verdicts.push((*id, classify_breaker_set(&result))),
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
            Ok(result) => proxies.extend(result.map(|proxy| (*id, proxy))),
            Err(error) => {
                return Status::failed(format!("read proxy {id} on {oracle_id}: {error}"))
            }
        }
    }
    aggregator_config_status(&proxies)
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
async fn pending_proposals(
    ctx: &CliContext,
    governance_id: &AccountId,
    ids: &[u32],
) -> anyhow::Result<Vec<(u32, Value)>> {
    let mut bodies = Vec::with_capacity(ids.len());
    for &id in ids {
        let args = serde_json::to_value(ProposalIdArgs { id })?;
        let read = ctx
            .client
            .read(contract::ViewFunction {
                contract_id: governance_id.clone(),
                method_name: "get_proposal".to_owned().into(),
                args: ContractArgs::Json(args),
            })
            .await;
        bodies.push((
            id,
            read.with_context(|| format!("read proposal {id} on {governance_id}"))?,
        ));
    }
    Ok(bodies)
}

#[derive(Deserialize)]
struct PendingBreakerArgs {
    id: PriceIdentifier,
    breaker_id: u32,
    #[serde(default)]
    breaker: Option<CircuitBreaker>,
}

#[derive(Clone, Copy)]
enum BreakerIdUse {
    Install,
    Existing,
}

enum PendingBreakerTarget {
    Install {
        id: PriceIdentifier,
        breaker_id: u32,
        breaker: CircuitBreaker,
    },
    Existing(PendingBreakerArgs),
}

impl PendingBreakerTarget {
    fn from_args(args: PendingBreakerArgs, usage: BreakerIdUse) -> anyhow::Result<Self> {
        match usage {
            BreakerIdUse::Install => {
                let breaker = args.breaker.context("add is missing its breaker payload")?;
                Ok(Self::Install {
                    id: args.id,
                    breaker_id: args.breaker_id,
                    breaker,
                })
            }
            BreakerIdUse::Existing => Ok(Self::Existing(args)),
        }
    }

    fn id(&self) -> PriceIdentifier {
        match self {
            Self::Install { id, .. } => *id,
            Self::Existing(args) => args.id,
        }
    }
}

fn pending_breaker_target(body: &Value) -> anyhow::Result<Option<PendingBreakerTarget>> {
    let operation = body
        .get("operation")
        .context("missing proposal body/operation")?;
    for (name, usage) in [
        ("AddCircuitBreaker", BreakerIdUse::Install),
        ("RemoveCircuitBreaker", BreakerIdUse::Existing),
        ("Rearm", BreakerIdUse::Existing),
        ("SetEnforced", BreakerIdUse::Existing),
    ] {
        if let Some(args) = operation.get(name) {
            let args = PendingBreakerArgs::deserialize(args)?;
            return Ok(Some(PendingBreakerTarget::from_args(args, usage)?));
        }
    }
    let Some(raw_call) = operation
        .get("TargetFunctionCall")
        .or_else(|| operation.get("AdminFunctionCall"))
    else {
        return Ok(None);
    };
    let usage = match raw_call.get("method_name").and_then(Value::as_str) {
        Some(method::ADD_CIRCUIT_BREAKER) => BreakerIdUse::Install,
        Some(method::REMOVE_CIRCUIT_BREAKER | method::REARM | method::SET_ENFORCED) => {
            BreakerIdUse::Existing
        }
        _ => return Ok(None),
    };
    let call = FunctionCall::deserialize(raw_call)?;
    let args = serde_json::from_slice(&call.args.0)?;
    Ok(Some(PendingBreakerTarget::from_args(args, usage)?))
}

/// `queued` holds the breaker IDs that queued adds for the same proxy will install.
fn breaker_lifecycle_problem(
    args: &PendingBreakerArgs,
    set: &CircuitBreakerSet,
    queued: &BTreeSet<u32>,
) -> Option<String> {
    (!set.breakers().contains_key(&args.breaker_id) && !queued.contains(&args.breaker_id))
        .then(|| format!("breaker {} no longer exists", args.breaker_id))
}

/// All adds queued for one proxy must be usable in order against the committed
/// allocator. Their expected IDs are stable because no other operation rewinds it.
fn pending_add_problem(set: &CircuitBreakerSet, adds: &[(u32, CircuitBreaker)]) -> Option<String> {
    let mut set = set.clone();
    let mut ordered: Vec<_> = adds.iter().collect();
    ordered.sort_by_key(|(breaker_id, _)| *breaker_id);
    for (breaker_id, breaker) in ordered {
        if let Err(error) = set.add(*breaker_id, breaker.clone()) {
            return Some(format!("add breaker {breaker_id}: {error}"));
        }
    }
    None
}

/// Checks queued adds per proxy, then every queued use of an existing breaker against the committed
/// set plus the IDs those valid adds will install.
fn queued_lifecycle_problems(
    sets: &BTreeMap<PriceIdentifier, Result<CircuitBreakerSet, String>>,
    installs: &BTreeMap<PriceIdentifier, Vec<(u32, CircuitBreaker)>>,
    existing: Vec<(u32, PendingBreakerArgs)>,
) -> Vec<String> {
    let mut problems = Vec::new();
    let mut queued_ids: BTreeMap<PriceIdentifier, BTreeSet<u32>> = BTreeMap::new();
    for (proxy, adds) in installs {
        let Some(Ok(set)) = sets.get(proxy) else {
            continue;
        };
        match pending_add_problem(set, adds) {
            Some(problem) => problems.push(format!("proxy {proxy}: {problem}")),
            None => {
                queued_ids.insert(*proxy, adds.iter().map(|(id, _)| *id).collect());
            }
        }
    }
    let no_queued_ids = BTreeSet::new();
    for (proposal_id, args) in existing {
        let problem = match sets.get(&args.id) {
            Some(Ok(set)) => breaker_lifecycle_problem(
                &args,
                set,
                queued_ids.get(&args.id).unwrap_or(&no_queued_ids),
            ),
            Some(Err(error)) => Some(error.clone()),
            None => Some("lifecycle was not inspected".to_owned()),
        };
        if let Some(problem) = problem {
            problems.push(format!(
                "proposal {proposal_id}, proxy {}: {problem}",
                args.id
            ));
        }
    }
    problems
}

async fn pending_breaker_lifecycles(
    ctx: &CliContext,
    oracle_id: &AccountId,
    bodies: &[(u32, Value)],
) -> Status {
    let mut sets = BTreeMap::new();
    let mut problems = Vec::new();
    let mut installs: BTreeMap<PriceIdentifier, Vec<(u32, CircuitBreaker)>> = BTreeMap::new();
    let mut existing = Vec::new();
    for (proposal_id, body) in bodies {
        let target = match pending_breaker_target(body) {
            Ok(Some(target)) => target,
            Ok(None) => continue,
            Err(error) => {
                problems.push(format!(
                    "proposal {proposal_id}: cannot inspect target: {error:#}"
                ));
                continue;
            }
        };
        let id = target.id();
        let set = match sets.entry(id) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let request_args = match serde_json::to_value(PriceIdArgs { id }) {
                    Ok(args) => args,
                    Err(error) => return Status::failed(format!("encode proxy {id}: {error}")),
                };
                let result = ctx
                    .client
                    .read(contract::ViewFunction {
                        contract_id: oracle_id.clone(),
                        method_name: "get_proxy_circuit_breaker_set".to_owned().into(),
                        args: ContractArgs::Json(request_args),
                    })
                    .await;
                let set = result
                    .map_err(|error| format!("{error:#}"))
                    .and_then(|raw| {
                        let set: CircuitBreakerSet = serde_json::from_value(raw)
                            .map_err(|error| format!("missing/undecodable lifecycle: {error}"))?;
                        set.validate().map_err(|error| error.to_string())?;
                        Ok(set)
                    });
                entry.insert(set)
            }
        };
        match target {
            PendingBreakerTarget::Install {
                id,
                breaker_id,
                breaker,
            } => {
                if set.is_ok() {
                    installs.entry(id).or_default().push((breaker_id, breaker));
                } else if let Err(error) = &set {
                    problems.push(format!("proposal {proposal_id}, proxy {id}: {error}"));
                }
            }
            PendingBreakerTarget::Existing(args) => existing.push((*proposal_id, args)),
        }
    }
    problems.extend(queued_lifecycle_problems(&sets, &installs, existing));
    if problems.is_empty() {
        Status::passed(
            "pending breaker IDs match current lifecycles; compatible adds remain queued. \
             This cannot prove allocator history deleted by old code; inspect the pre-upgrade inventory"
        )
    } else {
        Status::failed(format!(
            "inspect and drain/cancel affected proposals before upgrade; never reselect IDs on an \
             ambiguous outcome: {}",
            problems.join("; ")
        ))
    }
}

/// The state begins with the existing shared header on both supported layouts.
/// v0 -> v1 replaces only its TTL policy representation; the rest is byte-compatible.
fn governance_state_budget(state: &[u8], version: u32) -> anyhow::Result<(u64, u64)> {
    let state_bytes = u64::try_from(state.len())?;
    match version {
        0 => {
            let header =
                <Governance<LegacyTtlConfig> as near_sdk::borsh::BorshDeserialize>::deserialize(
                    &mut &state[..],
                )?;
            let old_policy_bytes = u64::try_from(object_length(header.ttls())?)?;
            let policy = (*header.ttls()).into_policy();
            let new_policy_bytes = u64::try_from(object_length(&policy)?)?;
            let state_bytes = state_bytes
                .checked_sub(old_policy_bytes)
                .and_then(|bytes| bytes.checked_add(new_policy_bytes))
                .context("migrated governance state size overflow")?;
            let recovery = policy_recovery_working_set_bytes(state_bytes, &policy)
                .context("policy recovery size overflow")?;
            Ok((state_bytes, recovery))
        }
        1 => {
            let header =
                <Governance<GovernancePolicy> as near_sdk::borsh::BorshDeserialize>::deserialize(
                    &mut &state[..],
                )?;
            let recovery = policy_recovery_working_set_bytes(state_bytes, header.ttls())
                .context("policy recovery size overflow")?;
            Ok((state_bytes, recovery))
        }
        _ => anyhow::bail!("unsupported governance state version {version}"),
    }
}

fn pending_operation_for_budget(raw: &Value) -> anyhow::Result<Operation> {
    if let Ok(operation) = Operation::deserialize(raw) {
        return Ok(operation);
    }
    // The actual released v0 view used an untagged code blob for AdminUpgrade.
    // Lift it into the existing migration DTO, without modifying stored bodies.
    let legacy = if let Some(args) = raw
        .get("AdminUpgrade")
        .filter(|args| args.get("code").is_some_and(Value::is_string))
    {
        LegacyOperation::AdminUpgrade {
            code: templar_common::upgrade::UpgradeSource::Code(
                near_sdk::json_types::Base64VecU8::deserialize(
                    args.get("code").context("missing legacy upgrade code")?,
                )?,
            ),
            migrate_args: near_sdk::json_types::Base64VecU8::deserialize(
                args.get("migrate_args")
                    .context("missing legacy migration arguments")?,
            )?,
        }
    } else {
        LegacyOperation::deserialize(raw)?
    };
    Ok(legacy.try_into()?)
}

fn pending_resource_status(
    state_bytes: u64,
    recovery_bytes: u64,
    bodies: &[(u32, Value)],
) -> Status {
    let mut working_set = state_bytes;
    let mut cancellation_logs = 0_u64;
    for (id, body) in bodies {
        let parsed = (|| -> anyhow::Result<_> {
            let operation = pending_operation_for_budget(
                body.get("operation")
                    .context("missing proposal operation")?,
            )?;
            let creator = AccountId::deserialize(
                body.get("created_by").context("missing proposal creator")?,
            )?;
            Ok((
                proposal_body_size(&operation, &creator)?,
                cancellation_event_size(*id, &operation)?,
            ))
        })();
        let (bytes, log_bytes) = match parsed {
            Ok(sizes) => sizes,
            Err(error) => {
                return Status::failed(format!("inspect proposal {id} resources: {error:#}"))
            }
        };
        let Some(next_bytes) = working_set.checked_add(bytes) else {
            return Status::failed("pending proposal working-set size overflow");
        };
        let Some(next_logs) = cancellation_logs.checked_add(log_bytes) else {
            return Status::failed("pending cancellation log size overflow");
        };
        working_set = next_bytes;
        cancellation_logs = next_logs;
    }
    let detail = format!(
        "working set {working_set}/{MAX_GOVERNANCE_WORKING_SET_BYTES} bytes; \
         policy recovery {recovery_bytes}/{MAX_GOVERNANCE_WORKING_SET_BYTES} bytes; \
         cancellation logs {cancellation_logs}/{MAX_PENDING_CANCELLATION_LOG_BYTES} bytes"
    );
    if working_set > MAX_GOVERNANCE_WORKING_SET_BYTES
        || recovery_bytes > MAX_GOVERNANCE_WORKING_SET_BYTES
        || cancellation_logs > MAX_PENDING_CANCELLATION_LOG_BYTES
    {
        Status::failed(format!(
            "{detail}; drain oversized legacy queues or reconfigure oversized policy on old code \
             before upgrading; never replace an ambiguous request"
        ))
    } else {
        Status::passed(format!(
            "{detail}. Fitting legacy queues are measured on first admission/revocation; \
             compatible pending bodies remain executable/cancellable"
        ))
    }
}

async fn governance_state_entry(
    ctx: &CliContext,
    governance_id: &AccountId,
    key: &[u8],
) -> anyhow::Result<Option<Base64Bytes>> {
    let entries = ctx
        .client
        .read(account::ViewState {
            account_id: governance_id.clone(),
            prefix: key.to_vec().into(),
        })
        .await?;
    Ok(entries
        .into_iter()
        .find(|entry| entry.key.0 == key)
        .map(|entry| entry.value))
}

async fn pending_resources(
    ctx: &CliContext,
    governance_id: &AccountId,
    bodies: &[(u32, Value)],
) -> Status {
    let budget = async {
        let state = governance_state_entry(ctx, governance_id, b"STATE")
            .await?
            .context("governance STATE missing")?;
        let version = governance_state_entry(ctx, governance_id, b"__v")
            .await?
            .map(|bytes| near_sdk::borsh::from_slice::<u32>(&bytes))
            .transpose()?
            .unwrap_or(0);
        governance_state_budget(&state, version)
    }
    .await;
    match budget {
        Ok((state_bytes, recovery)) => pending_resource_status(state_bytes, recovery, bodies),
        Err(error) => Status::failed(format!("inspect governance resource budget: {error:#}")),
    }
}

async fn proposal_ids(ctx: &CliContext, governance_id: &AccountId) -> anyhow::Result<Vec<u32>> {
    ctx.client
        .read(gov::ListProposals {
            governance_id: governance_id.clone(),
            offset: None,
            count: None,
        })
        .await
        .with_context(|| format!("list proposals on {governance_id}"))
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
        .with_context(|| format!("read the oracle governed by {governance_id}"))?;
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
        .with_context(|| format!("read proposal {proposal_id} on {governance_id}"))?;

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
            StepwiseChange,
        },
        FreshnessFilter, Proxy, WeightedSource,
    };
    use templar_proxy_oracle_near_common::request::OracleRequest;
    use templar_proxy_oracle_near_governance_common::MAX_PENDING_PROPOSALS;
    use templar_proxy_oracle_near_governance_common::{
        governance_kernel as kernel_governance, target, GovernancePolicy, LegacyOperation,
        LegacyTtlConfig,
    };

    use super::{
        aggregator_config_status, breaker_lifecycle_problem, breakers_status, classify_breaker_set,
        governance_ledger_status, governance_resolution_failure_checks, governance_state_budget,
        has_incompatible_rearm_payload, pending_add_problem, pending_breaker_target,
        pending_rearm_status, pending_resource_status, queued_lifecycle_problems,
        removed_feed_allocator_status, BTreeMap, BTreeSet, BreakerVerdict, PendingBreakerArgs,
        PendingBreakerTarget, PriceIdentifier, Serialize, Source, Status, Value,
    };

    fn price_id(byte: u8) -> PriceIdentifier {
        PriceIdentifier([byte; 32])
    }

    fn sample_breaker() -> CircuitBreaker {
        CircuitBreaker::MonotonicRun(MonotonicRun {
            max_streak: 1,
            min_relative_step_change: Decimal::ONE_HALF,
        })
    }

    fn source() -> Source {
        OracleRequest::redstone("redstone.near".parse().unwrap(), "BTC").into()
    }

    fn proxy(weights: &[u32]) -> Proxy<Source> {
        Proxy {
            aggregator: Aggregator::MedianLow(Median::new(weights.iter().enumerate().map(
                |(index, weight)| {
                    WeightedSource::new(
                        OracleRequest::redstone(
                            "redstone.near".parse().unwrap(),
                            format!("ASSET{index}"),
                        ),
                        *weight,
                    )
                },
            ))),
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
    #[case::empty(&[], true)]
    fn aggregator_config_status_rejects_invalid_sources(
        #[case] weights: &[u32],
        #[case] fails: bool,
    ) {
        let status = aggregator_config_status(&[(price_id(1), proxy(weights))]);
        assert_eq!(status.is_failure(), fails, "{status:?}");
    }

    #[test]
    fn priority_checks_descriptors_without_a_quorum() {
        let proxy = Proxy {
            aggregator: Aggregator::Priority(Priority::new(vec![source()])),
            freshness_filter: FreshnessFilter::new(None, None),
        };
        assert!(!aggregator_config_status(&[(price_id(1), proxy)]).is_failure());
        let duplicate = Proxy {
            aggregator: Aggregator::Priority(Priority::new(vec![source(), source()])),
            freshness_filter: FreshnessFilter::new(None, None),
        };
        assert!(aggregator_config_status(&[(price_id(1), duplicate)]).is_failure());
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
    fn governance_resolution_failure_fails_every_governance_check() {
        let error = anyhow::anyhow!("proxy RPC unavailable");
        let checks = governance_resolution_failure_checks(&error);

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
        assert_eq!(has_incompatible_rearm_payload(&operation), expected);
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

    #[test]
    fn pending_ids_distinguish_compatible_adds_from_stale_lifecycles() {
        let mut set = breaker_set(4);
        let args = PendingBreakerArgs {
            id: price_id(1),
            breaker_id: 0,
            breaker: None,
        };
        let none = BTreeSet::new();
        assert!(pending_add_problem(&set, &[(0, sample_breaker())]).is_none());
        assert!(breaker_lifecycle_problem(&args, &set, &none).is_some());
        assert!(breaker_lifecycle_problem(&args, &set, &BTreeSet::from([0])).is_none());
        set.add(0, sample_breaker()).unwrap();
        assert!(pending_add_problem(&set, &[(0, sample_breaker())]).is_some());
        assert!(breaker_lifecycle_problem(&args, &set, &none).is_none());
        set.reset_preserving_next_id();
        set.set_config(CircuitBreakerSetConfig {
            sample_interval_ns: Nanoseconds::zero(),
            history_len: 4,
        })
        .unwrap();
        assert!(breaker_lifecycle_problem(&args, &set, &none).is_some());
        assert!(pending_add_problem(&set, &[(0, sample_breaker())]).is_some());
        assert!(pending_add_problem(&set, &[(1, sample_breaker())]).is_none());
    }

    #[test]
    fn removed_feed_without_breaker_set_is_reported() {
        let epochs = BTreeSet::from([price_id(1), price_id(2), price_id(3)]);
        let sets = BTreeSet::from([price_id(1), price_id(3)]);
        let Status::Warned { detail } = removed_feed_allocator_status(&epochs, &sets) else {
            panic!("an epoch without a breaker set must be reported");
        };
        assert!(detail.contains(&price_id(2).to_string()), "{detail}");
        assert!(!detail.contains(&price_id(1).to_string()), "{detail}");
        assert!(matches!(
            removed_feed_allocator_status(&sets, &sets),
            Status::Passed { .. }
        ));
    }

    #[test]
    fn queued_add_satisfies_later_operations_on_the_breaker_it_installs() {
        let use_of = |breaker_id| PendingBreakerArgs {
            id: price_id(1),
            breaker_id,
            breaker: None,
        };
        let sets = BTreeMap::from([(price_id(1), Ok(breaker_set(4)))]);
        let installs = BTreeMap::from([(price_id(1), vec![(0, sample_breaker())])]);
        assert!(queued_lifecycle_problems(&sets, &installs, vec![(2, use_of(0))]).is_empty());
        let problems = queued_lifecycle_problems(&sets, &installs, vec![(2, use_of(1))]);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("breaker 1 no longer exists"));
        let invalid = BTreeMap::from([(price_id(1), vec![(1, sample_breaker())])]);
        let problems = queued_lifecycle_problems(&sets, &invalid, vec![(2, use_of(1))]);
        assert_eq!(problems.len(), 2, "{problems:?}");
    }

    #[test]
    fn pending_adds_apply_runtime_rule_and_capacity_validation() {
        assert!(pending_add_problem(
            &breaker_set(4),
            &[(0, sample_breaker()), (1, sample_breaker())]
        )
        .is_none());
        assert!(pending_add_problem(
            &breaker_set(4),
            &[(0, sample_breaker()), (0, sample_breaker())]
        )
        .is_some());
        assert!(pending_add_problem(
            &breaker_set(4),
            &[(0, sample_breaker()), (2, sample_breaker())]
        )
        .is_some());
        let mut raw = serde_json::to_value(breaker_set(4)).unwrap();
        raw["next_id"] = json!(u32::MAX);
        let exhausted: CircuitBreakerSet = serde_json::from_value(raw).unwrap();
        assert!(pending_add_problem(&exhausted, &[(u32::MAX, sample_breaker())]).is_some());
        let invalid = CircuitBreaker::StepwiseChange(StepwiseChange {
            max_relative_change: Decimal::from(2_u8),
        });
        assert!(pending_add_problem(&breaker_set(4), &[(0, invalid)]).is_some());
    }

    #[rstest]
    #[case::legacy(true)]
    #[case::current(false)]
    fn pending_add_keeps_its_expected_id_in_both_encodings(#[case] legacy: bool) {
        use templar_proxy_oracle_near_governance_common::target;
        let breaker = CircuitBreaker::MonotonicRun(MonotonicRun {
            max_streak: 1,
            min_relative_step_change: Decimal::ONE_HALF,
        });
        let operation = if legacy {
            serde_json::to_value(LegacyOperation::AddCircuitBreaker {
                id: price_id(3),
                breaker_id: 7,
                breaker,
            })
            .unwrap()
        } else {
            json!({"TargetFunctionCall": target::admin_add_circuit_breaker(
                price_id(3), 7, breaker, None,
            ).unwrap()})
        };
        let target = pending_breaker_target(&json!({"operation": operation.clone()}))
            .unwrap()
            .unwrap();
        let PendingBreakerTarget::Install { id, breaker_id, .. } = target else {
            panic!("expected an install target");
        };
        assert_eq!(id, price_id(3));
        assert_eq!(breaker_id, 7);
        assert!(pending_breaker_target(&Value::Null).is_err());

        let invalid = if legacy {
            let mut operation = operation;
            operation["AddCircuitBreaker"]
                .as_object_mut()
                .unwrap()
                .remove("breaker");
            operation
        } else {
            let mut call =
                target::admin_add_circuit_breaker(price_id(3), 7, sample_breaker(), None).unwrap();
            call.args.0 = serde_json::to_vec(&MissingBreakerArgs {
                id: price_id(3),
                breaker_id: 7,
            })
            .unwrap();
            json!({"TargetFunctionCall": call})
        };
        let Err(error) = pending_breaker_target(&json!({"operation": invalid})) else {
            panic!("missing breaker payload must be rejected");
        };
        assert!(error
            .to_string()
            .contains("add is missing its breaker payload"));
    }

    #[derive(Serialize)]
    struct MissingBreakerArgs {
        id: PriceIdentifier,
        breaker_id: u32,
    }

    /// The pre-relative-delay `admin_rearm` argument shape.
    #[derive(Serialize)]
    struct AbsoluteRearmArgs {
        id: PriceIdentifier,
        breaker_id: u32,
        armed_after_ns: Nanoseconds,
    }

    #[test]
    fn pending_relative_rearm_is_compatible_but_absolute_rearm_requires_intervention() {
        use templar_proxy_oracle_near_governance_common::target;
        let mut call = target::admin_rearm(price_id(4), 2, Nanoseconds::from_ns(5), None).unwrap();
        assert!(!has_incompatible_rearm_payload(
            &json!({"TargetFunctionCall": call})
        ));
        call.args.0 = serde_json::to_vec(&AbsoluteRearmArgs {
            id: price_id(4),
            breaker_id: 2,
            armed_after_ns: Nanoseconds::from_ns(5),
        })
        .unwrap();
        assert!(has_incompatible_rearm_payload(
            &json!({"TargetFunctionCall": call})
        ));
    }

    #[test]
    fn v0_and_v1_state_budgets_use_their_actual_policy_encodings() {
        let legacy_ttls = LegacyTtlConfig {
            set_proxy: Nanoseconds::zero(),
            configure_circuit_breakers: Nanoseconds::zero(),
            add_circuit_breaker: Nanoseconds::zero(),
            remove_circuit_breaker: Nanoseconds::zero(),
            set_manual_trip: Nanoseconds::zero(),
            rearm: Nanoseconds::zero(),
            set_enforced: Nanoseconds::zero(),
            set_action_ttl: Nanoseconds::zero(),
            set_role: Nanoseconds::zero(),
            admin_upgrade: Nanoseconds::zero(),
            admin_function_call: Nanoseconds::zero(),
        };
        let policy = legacy_ttls.into_policy();
        let v0 = kernel_governance::Governance::new(legacy_ttls, MAX_PENDING_PROPOSALS);
        let v0_bytes = near_sdk::borsh::to_vec(&v0).unwrap();
        let v1 = kernel_governance::Governance::new(policy, MAX_PENDING_PROPOSALS);
        let v1_bytes = near_sdk::borsh::to_vec(&v1).unwrap();
        // The v0 estimate must equal the header the migration actually writes.
        let (state_bytes, _) = governance_state_budget(&v0_bytes, 0).unwrap();
        let (v1_state_bytes, _) = governance_state_budget(&v1_bytes, 1).unwrap();
        assert_eq!(state_bytes, u64::try_from(v1_bytes.len()).unwrap());
        assert_eq!(v1_state_bytes, u64::try_from(v1_bytes.len()).unwrap());
        assert!(governance_state_budget(&v0_bytes, 2).is_err());
    }

    #[test]
    fn legacy_and_current_pending_bodies_share_one_resource_budget() {
        let creator: near_account_id::AccountId = "delegate.near".parse().unwrap();
        let small = json!({
            "operation": {"TargetFunctionCall": target::admin_rearm(
                price_id(1), 0, Nanoseconds::from_secs(1), None,
            ).unwrap()},
            "created_by": "delegate.near",
        });
        let legacy = json!({
            "operation": {"AddCircuitBreaker": {
                "id": price_id(2), "breaker_id": 0,
                "breaker": {"StepwiseChange": {"max_relative_change": "0.5"}},
            }},
            "created_by": creator,
        });
        let v1 = kernel_governance::Governance::new(
            GovernancePolicy::uniform(Nanoseconds::zero()).unwrap(),
            MAX_PENDING_PROPOSALS,
        );
        let state_bytes = near_sdk::borsh::to_vec(&v1).unwrap().len() as u64;
        let status = pending_resource_status(state_bytes, state_bytes, &[(1, small), (2, legacy)]);
        let Status::Passed { detail } = status else {
            panic!("compatible pending bodies must stay executable: {status:?}");
        };
        assert!(detail.starts_with("working set"), "{detail}");

        let oversized = (0..64_u64)
            .map(|id| {
                (
                    u32::try_from(id).unwrap(),
                    json!({
                        "operation": {"TargetFunctionCall": templar_proxy_oracle_near_governance_common::FunctionCall {
                            method_name: "admin_rearm".to_owned(),
                            args: near_sdk::json_types::Base64VecU8(vec![0; 40_000]),
                            attached_deposit: near_sdk::json_types::U128(0),
                            gas: near_sdk::Gas::from_tgas(1),
                        }},
                        "created_by": creator,
                    }),
                )
            })
            .collect::<Vec<_>>();
        assert!(pending_resource_status(state_bytes, state_bytes, &oversized).is_failure());
    }
}
