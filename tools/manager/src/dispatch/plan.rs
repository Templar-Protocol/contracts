//! `market plan` / `market apply` — generate a deployment as a file, then send
//! that file. Splitting the two puts a reviewable, editable artifact between
//! them; the artifact itself is [`crate::spec::plan`].

use std::collections::BTreeSet;
use std::io::Write as _;

use anyhow::Context as _;
use near_account_id::AccountId;
use near_api::types::NearToken;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_common::Nanoseconds;
use templar_gateway_client::Client;
use templar_gateway_core::{
    GatewayContext, GatewayResult, OperationPlan, PlanWrite, PlannedTransaction,
};
use templar_gateway_methods_dispatch::Dispatch;
use templar_gateway_methods_spec::{
    market, proxy_oracle, proxy_oracle_governance as gov, registry,
};
use templar_gateway_types::common::{WriteOperationResult, WriteRequest};
use templar_gateway_types::{primitive::PublicKey, Base64Bytes, MethodSpec};
use templar_proxy_oracle_kernel::proxy::Proxy;
use templar_proxy_oracle_near_common::input::Source;
use templar_proxy_oracle_near_governance_common::Operation;

use crate::commands::market::{Apply, Plan};
use crate::commands::proxy_oracle::governance::{uniform_ttls, GovernanceInit};
use crate::context::{print_json, CliContext};
use crate::spec::{
    check::{Check, Status},
    plan::{Derived, PlanFile, PLAN_SCHEMA_VERSION},
    MarketSpec, BORROW_PRICE_ID, COLLATERAL_PRICE_ID,
};

/// Deposits funding each new account's storage and balance, matching
/// `script/deploy.sh`. Constants rather than spec fields: these size a
/// *contract's* storage staking, which follows from the code being deployed
/// rather than from the market. A deployment needing more can raise one by
/// editing the plan.
const GOVERNANCE_DEPOSIT: NearToken = NearToken::from_millinear(3_500);
const ORACLE_DEPOSIT: NearToken = NearToken::from_near(5);
const MARKET_DEPOSIT: NearToken = NearToken::from_millinear(5_500);

/// `market plan` — run the preflight, then write the deployment as a file.
pub(super) async fn plan(ctx: CliContext, args: Plan) -> anyhow::Result<()> {
    let mut spec = crate::spec::extends::load(&args.path)?;
    let spec_digest = crate::spec::plan::digest(&spec)?;

    let mut checks =
        super::preflight::run_all(&ctx, &mut spec, false, args.accept_decimals_mismatch).await?;
    // Plan-time only, deliberately not in `run_all`: `spec check` validates a
    // spec, which stays valid after its market is deployed, while planning a
    // deployment needs its three target accounts free. `registry deploy` fails
    // on an occupied account, so a collision on the *market* — the last step —
    // would otherwise be discovered only after governance and the oracle were
    // deployed and both proposals executed.
    checks.extend(targets_available(&ctx, &spec).await?);
    apply_skips(&mut checks, &args.skip_check)?;

    let failed = crate::spec::check::failures(&checks);
    if failed > 0 {
        print_json(&checks)?;
        anyhow::bail!(
            "{failed} check(s) failed; no plan written. Fix the spec, or re-run \
             with `--skip-check <id>` for a check that is wrong."
        );
    }

    let steps = build(
        &ctx.client,
        &spec,
        &PublicKey::from(args.public_key),
        &args.signer_id,
    )
    .await?;
    let file = PlanFile::new(
        spec.network()?.to_string(),
        spec_digest,
        Derived {
            market_id: spec.market_id()?,
            oracle_id: spec.oracle_id()?,
            governance_id: spec.governance_id()?,
            collateral_decimals: spec.collateral.decimals,
            borrow_decimals: spec.borrow.decimals,
        },
        checks,
        steps,
    )?;

    let rendered = serde_json::to_string_pretty(&file).context("render the plan")?;
    match &args.out {
        Some(path) => {
            std::fs::write(path, format!("{rendered}\n"))
                .with_context(|| format!("write {}", path.display()))?;
            eprintln!("Wrote {} step(s) to {}", file.steps.len(), path.display());
        }
        None => println!("{rendered}"),
    }
    Ok(())
}

/// `market apply` — send a plan file.
pub(super) async fn apply(ctx: CliContext, args: Apply) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(&args.plan)
        .with_context(|| format!("read {}", args.plan.display()))?;
    let file: PlanFile =
        serde_json::from_str(&text).with_context(|| format!("parse {}", args.plan.display()))?;

    ensure_compatible(&file, &ctx.network().to_string())?;
    // Otherwise every guard below passes vacuously and a truncated file applies
    // cleanly, reporting success for having deployed nothing.
    anyhow::ensure!(
        !file.steps.is_empty(),
        "this plan has no steps; nothing would be deployed"
    );
    render(&file);
    report_checks(&file);

    // Re-read, rather than trusting the plan's own `deployment.available.*`. A
    // plan is written to be reviewed, and a target free at plan time can be
    // claimed while that review happens — after which the first six steps
    // succeed and the market deploy fails, which is the half-spent deploy the
    // plan-time check exists to prevent.
    let targets = planned_targets(&file)?;
    ensure_targets_free(&ctx, &targets).await?;

    // Provenance is reported, not enforced.
    let drift = file.drift()?;
    eprintln!("\n{}", drift.describe());
    if !drift.is_clean() {
        eprintln!(
            "An edited plan bypasses every spec-level check above — its arguments \
             are already encoded. Read the steps before confirming."
        );
    }

    // Each step names its own signer; `execute_as` only labels the store
    // record. A credential for some other account fails at step 0 with an
    // opaque executor error, so it is caught here instead.
    let credential = args.signer.account_id().0;
    let signers: BTreeSet<&AccountId> = file.steps.iter().map(|step| &step.signer_id).collect();
    anyhow::ensure!(
        signers.iter().all(|signer| **signer == credential),
        "this plan is signed by {}, but the credential given is for `{credential}`. \
         Re-run with a matching --signer-id, or re-plan.",
        signers
            .iter()
            .map(|signer| format!("`{signer}`"))
            .collect::<Vec<_>>()
            .join(", "),
    );

    // `build` enforces this at plan time, but the value that executes lives in
    // step 0's encoded `init_args`, not in `signer_id`. Guard 7's own advice —
    // "re-run with a matching --signer-id" — invites rewriting the signer
    // fields, which would satisfy it while leaving a stale `admin_id` that
    // makes every proposal revert after the deposits are spent.
    for step in &file.steps {
        for call in &step.function_calls {
            let Some(admin_id) = init_arg(call, "admin_id") else {
                continue;
            };
            anyhow::ensure!(
                admin_id == credential.as_str(),
                "`{}` seats `{admin_id}` as governance admin, but this plan is \
                 applied by `{credential}`, which would not hold the Admin role. \
                 Re-plan with a spec whose `governance.admin` matches.",
                step.label,
            );
        }
    }

    // Every other write this tool performs is a single transaction, so this
    // risk is new here and worth stating before the money is spent rather than
    // after: the operation record lives in an in-memory store
    // (`ClientBuilder`'s default), so an interrupt or an ambiguous RPC result
    // part-way through leaves nothing to resume from or reconcile against.
    // ENG-546 replaces the store and adds resume.
    if file.steps.len() > 1 {
        eprintln!(
            "\nThis sends {} transactions in sequence. They are not journalled: \
             if this is interrupted part-way, no record of what landed is kept, \
             and re-running starts from the first step. Check the deployed \
             accounts by hand before retrying.",
            file.steps.len()
        );
    }

    if !args.yes {
        confirm(&format!(
            "Send {} transaction(s) as {credential}?",
            file.steps.len(),
        ))?;
    }

    // Again, immediately before sending: the prompt above has no time limit, and
    // the check is only worth what it is worth at the moment of the send.
    ensure_targets_free(&ctx, &targets).await?;

    let plan = file.into_operation_plan()?;
    ctx.execute_via::<PlanDispatch, _>(&args.signer, PreparedPlan { steps: plan.steps })
        .await
}

/// The deployment, in the order `deploy.sh` runs it.
///
/// The order is a safety property, not a preference: `registry deploy` fails the
/// whole transaction when the account already exists, and the governance
/// contract must own the oracle before any feed can be configured. Deploying
/// governance first means an oracle can never be handed to an account this plan
/// did not create.
pub(crate) async fn build(
    client: &Client,
    spec: &MarketSpec,
    public_key: &PublicKey,
    signer_id: &AccountId,
) -> anyhow::Result<Vec<(String, PlannedTransaction)>> {
    // A plan is a fixed list of transactions; it cannot encode "wait". With a
    // non-zero TTL the two proxy proposals are not executable when they are
    // created, and the alternative — emitting the creates and dropping the
    // executes — would deploy a market pointing at an unconfigured oracle.
    anyhow::ensure!(
        spec.governance.ttl_default == Nanoseconds::from_ns(0),
        "`governance.ttl_default` is {}ns, so the proxy proposals would not be \
         executable when created, and a plan cannot wait. Deploy with \
         `ttl_default = \"0s\"` and raise the TTL afterwards with a `set-action-ttl` \
         proposal, or run the proposals by hand with `proxy-oracle governance \
         execute-proposal --when-ready`.",
        spec.governance.ttl_default.as_ns(),
    );

    // The proposals are created and executed by `signer_id`, but only
    // `governance.admin` is granted the Admin role at init. Mismatched, the two
    // registry deploys succeed and every proposal reverts — 8.5 NEAR spent on
    // exactly the orphaned half-deployment this tool exists to prevent.
    // `deploy.sh` made this unrepresentable by passing `--admin-id $SIGNER_ID`.
    anyhow::ensure!(
        &spec.governance.admin == signer_id,
        "`governance.admin` is `{}` but this plan is signed by `{signer_id}`, \
         which would not hold the Admin role. Every proxy proposal would revert \
         after the governance and oracle deploys had already spent their \
         deposits. Set them to the same account.",
        spec.governance.admin,
    );

    let price_maximum_age = spec.market.price_maximum_age;
    let full_access_keys = Some(vec![public_key.clone()]);

    // A plan always creates its own governance contract, so its proposal
    // counter always starts at zero. Reading `nextProposalId` would be reading
    // an account this very plan is about to create — and if one already exists,
    // step 1 fails loudly rather than appending proposals to someone else's
    // governance. Resuming onto existing contracts is ENG-546.
    let (collateral_proposal, borrow_proposal) = (0, 1);

    // Resolved by the preflight; the caller refuses before reaching here if a
    // decimals check failed. Guessing would mis-scale every price.
    let (Some(collateral_decimals), Some(borrow_decimals)) =
        (spec.collateral.decimals, spec.borrow.decimals)
    else {
        anyhow::bail!(
            "asset decimals are unresolved, so a market configuration cannot be \
             built. Set `decimals` in the spec, or fix `asset.decimals.*`."
        );
    };
    let configuration = spec
        .clone()
        .into_market_configuration(i32::from(collateral_decimals), i32::from(borrow_decimals))?;

    let mut steps = oracle_stack(client, signer_id, spec, public_key).await?;

    // Both sides project to the same `Proxy<Source>`, so one loop covers them
    // even though `AssetSpec` is generic over the asset class.
    for (side, price_id, proposal_id, proxy) in [
        (
            "collateral",
            COLLATERAL_PRICE_ID,
            collateral_proposal,
            spec.collateral.clone().into_proxy(price_maximum_age),
        ),
        (
            "borrow",
            BORROW_PRICE_ID,
            borrow_proposal,
            spec.borrow.clone().into_proxy(price_maximum_age),
        ),
    ] {
        steps.extend(set_proxy(client, signer_id, spec, side, price_id, proposal_id, proxy).await?);
    }

    steps.extend(
        step(
            client,
            signer_id,
            &format!("deploy market {}", spec.market_id()?),
            market::Create {
                registry_id: spec.registry.clone(),
                name: spec.name.clone(),
                version_key: spec.versions.market.clone(),
                configuration,
                full_access_keys,
                deposit: MARKET_DEPOSIT,
            },
        )
        .await?,
    );

    Ok(steps)
}

/// The governance contract, then the oracle it owns: the oracle names its
/// governance as `owner_id` at init, so governance must exist first.
async fn oracle_stack(
    client: &Client,
    signer_id: &AccountId,
    spec: &MarketSpec,
    public_key: &PublicKey,
) -> anyhow::Result<Vec<(String, PlannedTransaction)>> {
    let full_access_keys = Some(vec![public_key.clone()]);
    let governance_id = spec.governance_id()?;
    let oracle_id = spec.oracle_id()?;

    // Constructing the gateway request directly bypasses the guard
    // `proxy-oracle create` applies, so it is applied here too: a pre-0.3.0
    // `new` ignores `owner_id`, leaving the registry as owner. Governance then
    // cannot configure either proxy — and because `admin_set_proxy` is
    // dispatched detached, the proposals still *report* success and the deploy
    // reaches market creation with an unconfigured oracle.
    crate::commands::proxy_oracle::check_owner_id_is_honored(
        &spec.versions.proxy_oracle,
        &governance_id,
    )?;

    let governance_init = serde_json::to_vec(&GovernanceInit {
        proxy_oracle_id: oracle_id.clone(),
        admin_id: spec.governance.admin.clone(),
        ttls: uniform_ttls(spec.governance.ttl_default),
    })
    .context("encode governance init args")?;

    let mut steps = step(
        client,
        signer_id,
        &format!("deploy governance {governance_id}"),
        registry::Deploy {
            registry_id: spec.registry.clone(),
            name: crate::spec::governance_name(&spec.name),
            version_key: spec.versions.proxy_governance.clone(),
            init_args: Base64Bytes(governance_init),
            full_access_keys: full_access_keys.clone(),
            deposit: GOVERNANCE_DEPOSIT,
        },
    )
    .await?;
    steps.extend(
        step(
            client,
            signer_id,
            &format!("deploy proxy oracle {oracle_id}, owned by governance"),
            proxy_oracle::Create {
                registry_id: spec.registry.clone(),
                name: crate::spec::oracle_name(&spec.name),
                version_key: spec.versions.proxy_oracle.clone(),
                owner_id: Some(governance_id),
                full_access_keys: full_access_keys.clone(),
                deposit: ORACLE_DEPOSIT,
            },
        )
        .await?,
    );
    Ok(steps)
}

/// Propose a feed's proxy, then execute that proposal. Two transactions: a
/// proposal is always created before it can run, even at `ttl_default = 0`.
async fn set_proxy(
    client: &Client,
    signer_id: &AccountId,
    spec: &MarketSpec,
    side: &str,
    price_id: PriceIdentifier,
    proposal_id: u32,
    proxy: Proxy<Source>,
) -> anyhow::Result<Vec<(String, PlannedTransaction)>> {
    let governance_id = spec.governance_id()?;

    let mut steps = step(
        client,
        signer_id,
        &format!("propose {side} proxy (proposal {proposal_id})"),
        gov::CreateProposal {
            governance_id: governance_id.clone(),
            id: proposal_id,
            operation: Operation::SetProxy {
                id: price_id,
                proxy: Some(proxy),
            },
            requested_ttl: spec.governance.ttl_default,
        },
    )
    .await?;
    steps.extend(
        step(
            client,
            signer_id,
            &format!("execute {side} proxy proposal {proposal_id}"),
            gov::ExecuteProposal {
                governance_id,
                id: proposal_id,
            },
        )
        .await?,
    );
    Ok(steps)
}

/// Plan one write, labelling every transaction it needs.
///
/// Not one transaction per write: `market.create` also registers storage for
/// each NEP-141 asset, so a market with a NEP-141 side plans two or three. An
/// earlier version required exactly one, which made `market plan` usable only
/// for NEP-245 markets. Where a write expands, each transaction is numbered so
/// the labels still describe what is being confirmed.
async fn step<S>(
    client: &Client,
    signer_id: &AccountId,
    label: &str,
    body: S,
) -> anyhow::Result<Vec<(String, PlannedTransaction)>>
where
    S: MethodSpec<Output = WriteOperationResult>,
    Dispatch: PlanWrite<S, GatewayContext>,
{
    let plan = client
        .plan_request(WriteRequest {
            signer_account_id: signer_id.clone().into(),
            idempotency_key: None,
            body,
        })
        .await
        .with_context(|| format!("planning `{}`", S::RPC_METHOD))?;

    anyhow::ensure!(
        !plan.steps.is_empty(),
        "`{}` planned no transactions for `{label}`",
        S::RPC_METHOD
    );

    let total = plan.steps.len();
    Ok(plan
        .steps
        .into_iter()
        .enumerate()
        .map(|(index, transaction)| {
            let label = if total == 1 {
                label.to_owned()
            } else {
                format!("{label} ({}/{total})", index + 1)
            };
            (label, transaction)
        })
        .collect())
}

/// Mark the named checks skipped, recording the verdict each one reached — a
/// bare "skipped" would hide the failure the operator chose to override. An id
/// matching nothing is an error, since a typo would otherwise read as a
/// successful suppression.
fn apply_skips(checks: &mut [Check], skip: &[String]) -> anyhow::Result<()> {
    for id in skip {
        let mut matched = false;
        for check in checks.iter_mut() {
            if &check.id != id {
                continue;
            }
            matched = true;
            let previous = match &check.status {
                Status::Passed { detail } => format!("would have passed: {detail}"),
                Status::Failed { detail } => format!("would have failed: {detail}"),
                Status::Skipped { reason } => format!("was already skipped: {reason}"),
            };
            check.status = Status::Skipped {
                reason: format!("--skip-check {id} ({previous})"),
            };
        }
        anyhow::ensure!(
            matched,
            "--skip-check `{id}` names no check in this run. Check ids are listed \
             by `spec check`; a typo here would silently suppress nothing."
        );
    }
    Ok(())
}

/// The only two hard refusals: this build cannot read a foreign schema
/// faithfully, and a network mismatch would send a mainnet deployment to testnet
/// or the reverse. Everything else is reported and confirmed, never blocked.
fn ensure_compatible(file: &PlanFile, network: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        file.schema == PLAN_SCHEMA_VERSION,
        "this plan declares schema {} but this build speaks {PLAN_SCHEMA_VERSION}. \
         Regenerate it with `market plan`.",
        file.schema,
    );
    anyhow::ensure!(
        file.network == network,
        "this plan is for {} but the CLI is pointed at {network}. Re-run with \
         `--network {}`.",
        file.network,
        file.network,
    );
    Ok(())
}

/// The accounts this plan's steps would create.
///
/// Read off the steps rather than `derived`, because the steps are what
/// executes. Editing a deploy's `name` is a supported hand edit, and `derived`
/// is metadata `into_operation_plan` never consults — checking it would check
/// accounts the plan no longer touches while the edited one collides.
///
/// Works from each call's *bytes*, so re-encoding a deploy's args as `base64`
/// (equally valid in this schema, and executed verbatim) cannot hide its target.
/// A registry deploy is recognised by its argument shape — `name` beside a
/// `version_key` — and creates `{name}` beneath the registry it is addressed to.
///
/// Fails closed. A call carrying a `version_key` whose target cannot be derived
/// is an unreviewable step, not an absent one: every silent `None` here would be
/// an unchecked account, which is exactly what this guard exists to prevent.
fn planned_targets(file: &PlanFile) -> anyhow::Result<Vec<AccountId>> {
    let mut targets = Vec::new();
    for step in &file.steps {
        for call in &step.function_calls {
            let bytes = call.args.to_bytes()?;
            let Ok(args) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                continue;
            };
            if args.get("version_key").is_none() {
                continue;
            }

            let name = args.get("name").and_then(serde_json::Value::as_str);
            let target = name.and_then(|name| {
                format!("{name}.{}", step.receiver_id)
                    .parse::<AccountId>()
                    .ok()
            });
            targets.push(target.with_context(|| {
                format!(
                    "`{}` deploys from a registry but names no usable sub-account \
                     ({name:?}), so the account it would create cannot be checked \
                     for a collision",
                    step.label
                )
            })?);
        }
    }
    Ok(targets)
}

/// Which of those accounts exist right now.
async fn occupied_targets(
    ctx: &CliContext,
    targets: &[AccountId],
) -> anyhow::Result<Vec<AccountId>> {
    let mut occupied = Vec::new();
    for account_id in targets {
        if super::preflight::exists(ctx, account_id).await? {
            occupied.push(account_id.clone());
        }
    }
    Ok(occupied)
}

/// Refuse a plan whose targets are taken.
///
/// Best-effort, and deliberately so. `account::Get` cannot see a target another
/// deploy has *reserved* but not yet created: `deploy_market` writes
/// `RegistryEntry::Reserved` before scheduling `create_account`, and no registry
/// view exposes it — `get_deployment` and `list_deployments` both filter to
/// `Deployed`. Seeing it would need an additive view, the same contract change
/// the soft-deleted-version gap needs (see `preflight::versions`). The registry
/// itself is the authoritative guard and rejects the second deploy, so the
/// unseen case costs a failed step, not a corrupted deployment.
async fn ensure_targets_free(ctx: &CliContext, targets: &[AccountId]) -> anyhow::Result<()> {
    let occupied = occupied_targets(ctx, targets).await?;
    anyhow::ensure!(
        occupied.is_empty(),
        "{} already exist(s), and this plan creates them. Re-plan under a \
         different `name`, or tear the existing deployment down.",
        occupied
            .iter()
            .map(|account_id| format!("`{account_id}`"))
            .collect::<Vec<_>>()
            .join(", "),
    );
    Ok(())
}

/// Every account this deployment creates must be free.
async fn targets_available(ctx: &CliContext, spec: &MarketSpec) -> anyhow::Result<Vec<Check>> {
    let mut checks = Vec::new();
    for (label, account_id) in [
        ("governance", spec.governance_id()?),
        ("oracle", spec.oracle_id()?),
        ("market", spec.market_id()?),
    ] {
        checks.push(Check::new(
            format!("deployment.available.{label}"),
            match super::preflight::exists(ctx, &account_id).await {
                Ok(false) => Status::passed(format!("`{account_id}` is free")),
                Ok(true) => Status::failed(format!(
                    "`{account_id}` already exists; the {label} deploy would fail. \
                     Pick another `name`, or tear the existing deployment down."
                )),
                Err(error) => Status::failed(format!("{error:#}")),
            },
        ));
    }
    Ok(checks)
}

/// The checks the plan carries.
///
/// `apply` is often run by someone other than whoever planned, and a plan can be
/// hand-edited, so the verdicts travel with the artifact and are shown before
/// the prompt. Only non-passing ones are printed: a suppressed check records the
/// failure it overrode, and burying that in a wall of green is how an override
/// stops being reviewed.
fn report_checks(file: &PlanFile) {
    let notable: Vec<_> = file
        .checks
        .iter()
        .filter(|check| !matches!(check.status, Status::Passed { .. }))
        .collect();

    let failed = crate::spec::check::failures(&file.checks);
    eprintln!(
        "\n{} check(s): {} passed, {} not run, {failed} FAILED",
        file.checks.len(),
        file.checks.len() - notable.len(),
        notable.len() - failed,
    );
    for check in notable {
        eprintln!("  {} — {:?}", check.id, check.status);
    }
    if failed > 0 {
        eprintln!(
            "This plan carries FAILED checks. `market plan` refuses to write one, \
             so this file was edited or produced by another build."
        );
    }
}

/// The plan, for a human about to authorize it.
fn render(file: &PlanFile) {
    eprintln!(
        "Plan for {} on {} (spec {})",
        file.derived.market_id, file.network, file.spec_digest
    );
    for (index, step) in file.steps.iter().enumerate() {
        eprintln!("\n  [{index}] {}", step.label);
        eprintln!("      {} -> {}", step.signer_id, step.receiver_id);
        for call in &step.function_calls {
            eprintln!(
                "      {}  deposit {}  gas {}",
                call.method_name, call.deposit, call.gas
            );
            if let Some(init_args) = decoded_init_args(call) {
                eprintln!("      init_args: {init_args}");
            }
        }
    }
}

/// One field of a registry deploy's decoded `init_args`, as a string.
fn init_arg(call: &crate::spec::plan::PlanFunctionCall, field: &str) -> Option<String> {
    Some(decoded_init_args(call)?.get(field)?.as_str()?.to_owned())
}

/// A registry deploy's `init_args`, decoded.
///
/// `registry.deploy` takes them as base64 *inside* its JSON args, so the market
/// configuration — the MCRs, the rate curve, the oracle account — is the one
/// part of a deployment the artifact cannot show as JSON. Telling an operator to
/// read the steps while hiding the payload that matters most would be hollow, so
/// it is decoded for display. It stays base64 in the file: expanding it there
/// would have to survive a byte-exact round trip, which is a schema change
/// rather than a rendering one.
/// Works from the call's bytes, like [`planned_targets`], so re-encoding the
/// *outer* args as base64 cannot hide the payload from either the display or
/// the `admin_id` guard built on it.
fn decoded_init_args(call: &crate::spec::plan::PlanFunctionCall) -> Option<serde_json::Value> {
    use base64::Engine as _;

    let bytes = call.args.to_bytes().ok()?;
    let args: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let encoded = args.get("init_args")?.as_str()?;
    let init = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    serde_json::from_slice(&init).ok()
}

/// Ask before spending real NEAR. Anything but `y` aborts.
fn confirm(question: &str) -> anyhow::Result<()> {
    eprint!("\n{question} [y/N] ");
    std::io::stderr().flush().ok();

    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .context("read confirmation from stdin")?;

    anyhow::ensure!(
        answer.trim().eq_ignore_ascii_case("y"),
        "aborted; nothing was sent"
    );
    Ok(())
}

/// A plan that is already built, so an edited plan file rides the same executor
/// as every other write — store, idempotency and recovery come along rather than
/// being reimplemented here.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PreparedPlan {
    steps: Vec<PlannedTransaction>,
}

impl schemars::JsonSchema for PreparedPlan {
    fn schema_name() -> String {
        "PreparedPlan".to_owned()
    }

    /// Never served over the RPC surface, so there is no schema to describe.
    fn json_schema(_generator: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        schemars::schema::Schema::Bool(true)
    }
}

impl MethodSpec for PreparedPlan {
    type Output = WriteOperationResult;
    const RPC_METHOD: &'static str = "manager.applyPlan";
}

/// A dispatcher local to this binary, reached with `Client::via`.
///
/// The impl deliberately does *not* target `methods_dispatch::Dispatch`: that
/// would bolt a method onto the shared dispatcher from a leaf tool, giving it a
/// capability `methods-spec` never declares and that never appears in
/// `METHODS.md`. A local ZST keeps the passthrough where it belongs.
pub(crate) struct PlanDispatch;

#[async_trait::async_trait]
impl PlanWrite<PreparedPlan, GatewayContext> for PlanDispatch {
    async fn plan(
        request: WriteRequest<PreparedPlan>,
        _context: GatewayContext,
    ) -> GatewayResult<OperationPlan> {
        Ok(OperationPlan {
            steps: request.body.steps,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_skips, ensure_compatible, PLAN_SCHEMA_VERSION};
    use crate::spec::check::{Check, Status};
    use crate::spec::plan::PlanArgs;
    use crate::spec::plan::{Derived, PlanFile};

    fn checks() -> Vec<Check> {
        vec![
            Check {
                id: "config.validate".to_owned(),
                status: Status::passed("ok"),
            },
            Check {
                id: "reference.price.collateral".to_owned(),
                status: Status::failed("off by 4%"),
            },
        ]
    }

    /// The point of `--skip-check`: one verdict is suppressed, everything else
    /// is untouched.
    #[test]
    fn skipping_leaves_every_other_check_alone() {
        let mut checks = checks();
        apply_skips(&mut checks, &["reference.price.collateral".to_owned()])
            .expect("a real id should skip");

        assert!(matches!(checks[0].status, Status::Passed { .. }));
        assert!(matches!(checks[1].status, Status::Skipped { .. }));
    }

    /// A suppressed failure must still say what it was.
    ///
    /// This is the same trap that produced four false-passes elsewhere in this
    /// tool: `Skipped` is how "not run" is reported, and using it to hide a
    /// known failure without recording the failure turns a red check green with
    /// nothing left to review.
    #[test]
    fn a_skipped_check_records_the_verdict_it_suppressed() {
        let mut checks = checks();
        apply_skips(&mut checks, &["reference.price.collateral".to_owned()]).expect("skip");

        let Status::Skipped { reason } = &checks[1].status else {
            panic!("expected Skipped, got {:?}", checks[1].status);
        };
        assert!(
            reason.contains("would have failed") && reason.contains("off by 4%"),
            "the suppressed verdict must survive in the report: {reason}"
        );
    }

    /// A typo must not read as a successful suppression.
    #[test]
    fn an_unknown_skip_id_is_an_error() {
        let error = apply_skips(&mut checks(), &["config.validte".to_owned()])
            .expect_err("a typo names no check");

        assert!(error.to_string().contains("names no check"), "{error:#}");
    }

    fn bare_plan(schema: u32, network: &str) -> PlanFile {
        PlanFile {
            schema,
            tool_version: "0.1.0".to_owned(),
            network: network.to_owned(),
            spec_digest: "sha256:test".to_owned(),
            step_digests: Vec::new(),
            summary_digest: "sha256:test".to_owned(),
            derived: Derived {
                market_id: "m.near".parse().expect("valid account"),
                oracle_id: "o.near".parse().expect("valid account"),
                governance_id: "g.near".parse().expect("valid account"),
                collateral_decimals: Some(6),
                borrow_decimals: Some(7),
            },
            checks: Vec::new(),
            steps: Vec::new(),
        }
    }

    /// Editing a deploy's `name` is a supported hand edit, and the steps are
    /// what executes — so the collision check must follow the edit, not the
    /// `derived` metadata that `into_operation_plan` never reads.
    #[test]
    fn targets_come_from_the_steps_not_the_metadata() {
        use crate::spec::plan::{PlanFunctionCall, PlanStep};

        let mut file = bare_plan(PLAN_SCHEMA_VERSION, "mainnet");
        file.steps.push(PlanStep {
            label: "deploy market".to_owned(),
            signer_id: "operator.near".parse().expect("valid account"),
            receiver_id: "templar-alpha.near".parse().expect("valid account"),
            function_calls: vec![PlanFunctionCall {
                method_name: "deploy_market".to_owned(),
                args: PlanArgs::Json(serde_json::json!({
                    "name": "edited-by-hand",
                    "version_key": "v1.3.0",
                })),
                gas: 300_000_000_000_000,
                deposit: near_api::types::NearToken::from_near(5),
            }],
        });

        assert_eq!(
            super::planned_targets(&file)
                .expect("derivable")
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            vec!["edited-by-hand.templar-alpha.near"],
            "the edited name is what gets deployed, so it is what gets checked"
        );
        assert_ne!(
            file.derived.market_id.as_str(),
            "edited-by-hand.templar-alpha.near",
            "and it deliberately disagrees with the stale metadata"
        );
    }

    /// Re-encoding a deploy's args as base64 is legal in this schema and is
    /// executed verbatim, so it must not hide the account being created.
    #[test]
    fn a_base64_encoded_deploy_still_yields_its_target() {
        use crate::spec::plan::{PlanFunctionCall, PlanStep};

        let raw = serde_json::to_vec(&serde_json::json!({
            "name": "hidden",
            "version_key": "v1.3.0",
        }))
        .expect("encode");

        let mut file = bare_plan(PLAN_SCHEMA_VERSION, "mainnet");
        file.steps.push(PlanStep {
            label: "deploy market".to_owned(),
            signer_id: "operator.near".parse().expect("valid account"),
            receiver_id: "templar-alpha.near".parse().expect("valid account"),
            function_calls: vec![PlanFunctionCall {
                method_name: "deploy_market".to_owned(),
                args: PlanArgs::Base64(templar_gateway_types::Base64Bytes(raw.clone())),
                gas: 300_000_000_000_000,
                deposit: near_api::types::NearToken::from_near(5),
            }],
        });

        assert_eq!(
            super::planned_targets(&file)
                .expect("derivable")
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            vec!["hidden.templar-alpha.near"],
        );
    }

    /// A deploy whose target cannot be derived is an unreviewable step, not an
    /// absent one — every silent skip here would be an unchecked account.
    #[test]
    fn an_underivable_target_fails_closed() {
        use crate::spec::plan::{PlanFunctionCall, PlanStep};

        let mut file = bare_plan(PLAN_SCHEMA_VERSION, "mainnet");
        file.steps.push(PlanStep {
            label: "deploy market".to_owned(),
            signer_id: "operator.near".parse().expect("valid account"),
            receiver_id: "templar-alpha.near".parse().expect("valid account"),
            function_calls: vec![PlanFunctionCall {
                method_name: "deploy_market".to_owned(),
                // Uppercase is not a valid NEAR account label.
                args: PlanArgs::Json(serde_json::json!({
                    "name": "Uppercase",
                    "version_key": "v1.3.0",
                })),
                gas: 300_000_000_000_000,
                deposit: near_api::types::NearToken::from_near(5),
            }],
        });

        let error = super::planned_targets(&file).expect_err("must not skip silently");
        assert!(
            format!("{error:#}").contains("cannot be checked for a collision"),
            "{error:#}"
        );
    }

    /// The `admin_id` guard reads the same bytes the executor sends, so
    /// re-encoding the outer args as base64 cannot hide a stale admin — the
    /// exact bypass that `planned_targets` had.
    #[test]
    fn the_admin_guard_sees_through_base64_args() {
        use crate::spec::plan::PlanFunctionCall;
        use base64::Engine as _;

        let init = serde_json::to_vec(&serde_json::json!({
            "proxy_oracle_id": "o.near",
            "admin_id": "someone-else.near",
        }))
        .expect("encode init args");
        let outer = serde_json::to_vec(&serde_json::json!({
            "name": "gov",
            "version_key": "v1",
            "init_args": base64::engine::general_purpose::STANDARD.encode(&init),
        }))
        .expect("encode outer args");

        let call = PlanFunctionCall {
            method_name: "deploy_market".to_owned(),
            args: PlanArgs::Base64(templar_gateway_types::Base64Bytes(outer)),
            gas: 300_000_000_000_000,
            deposit: near_api::types::NearToken::from_near(3),
        };

        assert_eq!(
            super::init_arg(&call, "admin_id").as_deref(),
            Some("someone-else.near"),
            "the seated admin must be visible regardless of arg representation"
        );
    }

    /// A governance proposal creates no account, so it contributes no target.
    #[test]
    fn only_registry_deploys_are_targets() {
        use crate::spec::plan::{PlanFunctionCall, PlanStep};

        let mut file = bare_plan(PLAN_SCHEMA_VERSION, "mainnet");
        file.steps.push(PlanStep {
            label: "propose".to_owned(),
            signer_id: "operator.near".parse().expect("valid account"),
            receiver_id: "gov.near".parse().expect("valid account"),
            function_calls: vec![PlanFunctionCall {
                method_name: "create_proposal".to_owned(),
                args: PlanArgs::Json(serde_json::json!({ "id": 0, "requested_ttl": "0" })),
                gas: 300_000_000_000_000,
                deposit: near_api::types::NearToken::from_yoctonear(1),
            }],
        });

        assert!(super::planned_targets(&file).expect("derivable").is_empty());
    }

    #[test]
    fn a_matching_plan_is_accepted() {
        ensure_compatible(&bare_plan(PLAN_SCHEMA_VERSION, "mainnet"), "mainnet")
            .expect("same schema and network");
    }

    /// Sending a mainnet deployment to testnet, or the reverse, is not something
    /// a confirmation prompt should be able to wave through.
    #[test]
    fn a_network_mismatch_is_refused() {
        let error = ensure_compatible(&bare_plan(PLAN_SCHEMA_VERSION, "mainnet"), "testnet")
            .expect_err("wrong network");

        assert!(
            error.to_string().contains("pointed at testnet"),
            "{error:#}"
        );
    }

    #[test]
    fn a_schema_mismatch_is_refused() {
        let error = ensure_compatible(&bare_plan(PLAN_SCHEMA_VERSION + 1, "mainnet"), "mainnet")
            .expect_err("wrong schema");

        assert!(error.to_string().contains("Regenerate it"), "{error:#}");
    }
}
