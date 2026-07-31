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
use crate::spec::journal::{self, Journal};
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

    let mut matched = apply_skips(&mut checks, &args.skip_check);
    // Gated before `build`, which has hard bails of its own: letting it run
    // first would replace a full check report with a single unrelated error.
    gate(&checks)?;

    let steps = build(
        &ctx.client,
        &spec,
        &PublicKey::from(args.public_key),
        &args.signer_id,
    )
    .await?;
    let steps = PlanFile::steps_from(steps)?;

    // After the steps exist, because it reads them; before the plan is written,
    // because a signer that cannot pay is a reason not to write one.
    let mut funding = super::funding::checks(&ctx, &steps).await?;
    matched.extend(apply_skips(&mut funding, &args.skip_check));
    ensure_every_skip_matched(&args.skip_check, &matched)?;
    checks.extend(funding);
    gate(&checks)?;

    let file = PlanFile::from_steps(
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
    // Reconciled before anything else that reads the steps: if the journal and
    // the plan disagree, nothing below is meaningful.
    let journal_path = journal::path_for(&args.plan);
    let mut journal = Journal::load(&journal_path)?;
    let remaining = journal.remaining(&file)?;

    if remaining.len() < file.steps.len() {
        eprintln!(
            "\nResuming: {} of {} step(s) already completed per {}.",
            file.steps.len() - remaining.len(),
            file.steps.len(),
            journal_path.display(),
        );
    }
    if remaining.is_empty() {
        eprintln!("Every step in this plan has already been applied.");
        return Ok(());
    }

    // Against the remaining steps only — a resume must not demand the deposits
    // it has already paid.
    let outstanding: Vec<_> = remaining
        .iter()
        .filter_map(|index| file.steps.get(*index).cloned())
        .collect();
    render(&file);
    report_checks(&file);

    // Re-read, rather than trusting the plan's own `deployment.available.*`. A
    // plan is written to be reviewed, and a target free at plan time can be
    // claimed while that review happens — after which the first six steps
    // succeed and the market deploy fails, which is the half-spent deploy the
    // plan-time check exists to prevent.
    let targets = planned_targets(&file)?;
    ensure_targets_are_distinct(&targets)?;
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
            ensure_init_args_readable(step, call)?;
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

    // Still worth stating before the money moves: the operator should know a
    // partial run is recoverable *and* where the record lives, rather than
    // discovering both after an interruption.
    if remaining.len() > 1 {
        eprintln!(
            "\nThis sends {} transactions in sequence. Each is recorded in {} as \
             it lands, so an interruption resumes from the next incomplete step \
             rather than restarting.",
            remaining.len(),
            journal_path.display(),
        );
    }

    ensure_plan_is_complete(&file)?;
    ensure_plan_is_coherent(&file)?;
    ensure_initializers_are_sound(&file)?;
    ensure_proposals_are_runnable(&file)?;

    // Whoever holds these keys controls the three new accounts. A mistyped or
    // substituted `--public-key` at plan time is invisible in the artifact —
    // the keys live inside the encoded args — so where the applier's own key is
    // derivable it must be the one being granted.
    // Asked of the backend, not taken from `--public-key`: for an external
    // backend the flag is only what the operator *asserted*, and checking a
    // grant against an assertion checks nothing. Errors propagate — an applier
    // who cannot say which key they hold cannot verify the one being handed
    // control of three new accounts, and that is a refusal, not a pass.
    {
        let mine = ctx.signing_public_key(&args.signer).await?;
        // Compared through the JSON encoding, which is the form the args carry.
        let mine = serde_json::to_value(&mine)
            .ok()
            .and_then(|key| key.as_str().map(ToOwned::to_owned))
            .unwrap_or_default();
        for step in &file.steps {
            for call in &step.function_calls {
                let granted = granted_keys(call)?;
                anyhow::ensure!(
                    granted.is_empty() || granted.iter().any(|key| key == &mine),
                    "`{}` grants full access to {}, not to `{mine}`. Applying this \
                     would hand control of the new account to a key you do not \
                     hold; re-plan with your own --public-key, or pass the key \
                     that is named.",
                    step.label,
                    granted
                        .iter()
                        .map(|key| format!("`{key}`"))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
        }
    }

    if !args.yes {
        confirm(&format!(
            "Send {} transaction(s) as {credential}?",
            remaining.len(),
        ))?;
    }

    let mut funding = super::funding::checks(&ctx, &outstanding).await?;
    let matched = apply_skips(&mut funding, &args.skip_check);
    ensure_every_skip_matched(&args.skip_check, &matched)?;
    let short = crate::spec::check::failures(&funding);
    for check in &funding {
        eprintln!("  {} — {:?}", check.id, check.status);
    }
    anyhow::ensure!(
        short == 0,
        "{short} signer(s) cannot cover this plan. Top up and re-run, or pass \
         `--skip-check` if the check is wrong; stopping now costs nothing, \
         stopping at step 4 does not."
    );

    // Again, immediately before sending: the prompt above has no time limit, and
    // the check is only worth what it is worth at the moment of the send.
    ensure_targets_free(&ctx, &targets).await?;

    // One transaction per call, so every outcome can be journalled as it
    // happens. Batching them would leave an interrupted run with nothing
    // recorded — which is the only run a journal exists for.
    let plan = file.clone().into_operation_plan()?;
    for index in remaining {
        let step = &file.steps[index];
        let transaction = plan.steps[index].clone();
        eprintln!("\n[{index}] {}", step.label);

        let output = ctx
            .execute_via::<PlanDispatch, _>(
                &args.signer,
                PreparedPlan {
                    steps: vec![transaction],
                },
            )
            .await
            .with_context(|| {
                format!(
                    "step {index} (`{}`) failed. Completed steps are recorded in \
                     {}; re-run `market apply` to resume from here.",
                    step.label,
                    journal_path.display(),
                )
            })?;

        journal.append(
            &journal_path,
            journal::Entry {
                step: index,
                digest: crate::spec::plan::digest(step)?,
                label: step.label.clone(),
                tx_hash: output
                    .operation
                    .latest_tx_hash()
                    .map(|hash| hash.to_string()),
            },
        )?;
    }
    Ok(())
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
fn apply_skips(checks: &mut [Check], skip: &[String]) -> BTreeSet<String> {
    let mut matched_ids = BTreeSet::new();
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
        if matched {
            matched_ids.insert(id.clone());
        }
    }
    matched_ids
}

/// A skip that matched nothing is a typo, and a typo must not read as a
/// successful suppression. Checked once, after every phase has run, because the
/// funding checks do not exist until the steps are built.
fn ensure_every_skip_matched(skip: &[String], matched: &BTreeSet<String>) -> anyhow::Result<()> {
    for id in skip {
        anyhow::ensure!(
            matched.contains(id),
            "--skip-check `{id}` names no check in this run. Check ids are listed \
             by `spec check`; a typo here would silently suppress nothing."
        );
    }
    Ok(())
}

/// Print the checks and refuse when any failed.
fn gate(checks: &[Check]) -> anyhow::Result<()> {
    let failed = crate::spec::check::failures(checks);
    if failed > 0 {
        print_json(&checks)?;
        anyhow::bail!(
            "{failed} check(s) failed; no plan written. Fix the spec, or re-run \
             with `--skip-check <id>` for a check that is wrong."
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

/// A deployment must still be a deployment.
///
/// Every coherence check below is conditional on finding the component it
/// validates, so *deleting* a step disables its guard rather than tripping it:
/// remove the oracle deploy and governance and the market stay mutually
/// consistent, the proposals fail against an account that does not exist —
/// reporting success, because `admin_set_proxy` is detached — and the market is
/// deployed pointing at nothing.
///
/// This is the same fail-open shape as the swallowed `Err`s fixed earlier, in a
/// form the sweep for those missed: the value is absent rather than unreadable.
fn ensure_plan_is_complete(file: &PlanFile) -> anyhow::Result<()> {
    let (mut governance, mut oracle, mut market, mut proposals) = (0, 0, 0, 0);

    for step in &file.steps {
        for call in &step.function_calls {
            if is_governance_proposal(call)? {
                proposals += 1;
                continue;
            }
            let Some(init) = decoded_init_args(call) else {
                continue;
            };
            if init.get("admin_id").is_some() {
                governance += 1;
            }
            if init.get("owner_id").is_some() {
                oracle += 1;
            }
            if init.get("configuration").is_some() {
                market += 1;
            }
        }
    }

    for (what, found) in [
        ("governance deploy", governance),
        ("proxy-oracle deploy", oracle),
        ("market deploy", market),
    ] {
        anyhow::ensure!(
            found == 1,
            "a deployment plan needs exactly one {what}; this one has {found}. \
             A missing component is not a smaller deployment — the remaining \
             steps still reference it, and the checks that would catch that are \
             skipped when the step they validate is absent."
        );
    }
    anyhow::ensure!(
        proposals >= 2,
        "a deployment plan configures both price feeds, which needs at least a \
         create and an execute per feed; this one has {proposals} proposal \
         step(s). A market whose feeds are never set prices nothing."
    );
    Ok(())
}

/// The proposals must be executable, in this plan, in this order.
///
/// Their own arguments had gone unvalidated: every other guard here checks
/// deploy arguments or the account references between steps. A proposal carries
/// three editable values that decide whether the oracle ever gets configured —
/// and an oracle that is never configured is the failure this whole guard suite
/// exists to prevent, because `admin_set_proxy` is dispatched detached and the
/// step still reports success.
fn ensure_proposals_are_runnable(file: &PlanFile) -> anyhow::Result<()> {
    let mut created: BTreeSet<u64> = BTreeSet::new();

    for step in &file.steps {
        for call in &step.function_calls {
            if !is_governance_proposal(call)? {
                continue;
            }
            let bytes = call.args.to_bytes()?;
            let args: serde_json::Value =
                serde_json::from_slice(&bytes).context("a proposal's arguments are not JSON")?;
            let id = args
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .context("a proposal carries no numeric id")?;

            // `operation` marks a create; its absence marks an execute.
            if args.get("operation").is_some() {
                let ttl = args.get("requested_ttl");
                let zero = ttl.is_none()
                    || ttl.and_then(|ttl| ttl.as_str()) == Some("0")
                    || ttl.and_then(serde_json::Value::as_u64) == Some(0);
                anyhow::ensure!(
                    zero,
                    "`{}` requests a TTL of {}, so the proposal would not be \
                     executable when the next step tries to run it. The effective \
                     TTL is the larger of the requested and configured values, so \
                     raising it here delays execution regardless of the \
                     contract's own zero TTLs.",
                    step.label,
                    ttl.map_or_else(|| "?".to_owned(), ToString::to_string),
                );
                anyhow::ensure!(
                    created.insert(id),
                    "`{}` creates proposal {id}, which this plan already creates. \
                     The governance contract requires each id to be the next one, \
                     so the second would be rejected.",
                    step.label,
                );
            } else {
                anyhow::ensure!(
                    created.contains(&id),
                    "`{}` executes proposal {id}, but this plan does not create it \
                     beforehand. It would run against a proposal that does not \
                     exist yet, leaving the feed unconfigured.",
                    step.label,
                );
            }
        }
    }
    Ok(())
}

/// Re-apply, against the encoded steps, the plan-time guards whose subject an
/// edit can change.
///
/// `build` refuses an oracle version that ignores `owner_id` and a non-zero
/// governance TTL, but both live in step arguments an operator may rewrite, and
/// a plan-time refusal does not bind the file. This is the third guard in this
/// PR whose executing value was editable after the check (`admin_id` was the
/// first), so all of them are re-verified here rather than one at a time.
fn ensure_initializers_are_sound(file: &PlanFile) -> anyhow::Result<()> {
    for step in &file.steps {
        for call in &step.function_calls {
            let Some(init) = decoded_init_args(call) else {
                continue;
            };

            // The oracle: its `new` must honour the owner it is given, or the
            // registry stays owner and governance can never configure a proxy.
            if let Some(owner_id) = init.get("owner_id").and_then(|id| id.as_str()) {
                let bytes = call.args.to_bytes()?;
                let args: serde_json::Value = serde_json::from_slice(&bytes)
                    .context("a registry deploy's arguments are not JSON")?;
                let version_key = args
                    .get("version_key")
                    .and_then(|key| key.as_str())
                    .context("the oracle deploy names no version")?;
                crate::commands::proxy_oracle::check_owner_id_is_honored(
                    version_key,
                    &owner_id
                        .parse()
                        .context("the oracle's owner is not an account")?,
                )?;
            }

            // Governance: a non-zero TTL makes the proposals in this plan
            // unexecutable when created, and a plan cannot wait.
            if let Some(ttls) = init.get("ttls").and_then(|ttls| ttls.as_object()) {
                for (kind, ttl) in ttls {
                    anyhow::ensure!(
                        ttl.as_str() == Some("0") || ttl.as_u64() == Some(0),
                        "`{}` seats a {kind} TTL of {ttl}, so the proposals this \
                         plan creates would not be executable when it runs them. \
                         Deploy with zero TTLs and raise them afterwards.",
                        step.label,
                    );
                }
            }
        }
    }
    Ok(())
}

/// The accounts a deployment creates are named in more places than they are
/// created, and an edit to one is not an edit to the others.
///
/// The oracle is created by its deploy, named by the governance initializer as
/// `proxy_oracle_id`, and pointed at by the market configuration. Governance is
/// created by its deploy, named by the oracle initializer as `owner_id`, and
/// addressed by every proposal step. `planned_targets` follows a renamed
/// account correctly — which is what makes this necessary rather than
/// redundant: the renamed account really is deployed and collision-checked,
/// while every stale reference still points at one the plan never creates.
fn ensure_plan_is_coherent(file: &PlanFile) -> anyhow::Result<()> {
    let (mut oracle, mut governance, mut market) = (None, None, None);
    let (mut governance_says, mut market_says, mut oracle_says) = (None, None, None);

    for step in &file.steps {
        for call in &step.function_calls {
            let Some(init) = decoded_init_args(call) else {
                continue;
            };
            // The oracle's initializer seats an owner; governance's seats an admin.
            if let Some(owner) = init.get("owner_id").and_then(|id| id.as_str()) {
                oracle = deploy_target(step, call)?;
                oracle_says = Some(owner.to_owned());
            }
            if init.get("admin_id").is_some() {
                governance = deploy_target(step, call)?;
            }
            if let Some(named) = init.get("proxy_oracle_id").and_then(|id| id.as_str()) {
                governance_says = Some(named.to_owned());
            }
            if let Some(named) = init
                .pointer("/configuration/price_oracle_configuration/account_id")
                .and_then(|id| id.as_str())
            {
                market_says = Some(named.to_owned());
            }
            // The market is the deploy whose initializer carries a configuration.
            if init.get("configuration").is_some() {
                market = deploy_target(step, call)?;
            }
        }
    }

    for (created, label, references) in [
        (
            oracle.as_ref(),
            "oracle",
            vec![
                ("the governance initializer", governance_says),
                ("the market configuration", market_says),
            ],
        ),
        (
            governance.as_ref(),
            "governance contract",
            vec![("the oracle's owner", oracle_says)],
        ),
    ] {
        let Some(created) = created else { continue };
        for (who, named) in references {
            if let Some(named) = named {
                anyhow::ensure!(
                    named == created.as_str(),
                    "this plan deploys the {label} as `{created}`, but {who} points \
                     at `{named}`. One was edited without the other, so the \
                     deployment would reference an account this plan never creates."
                );
            }
        }
    }

    // A NEP-141 market also registers storage *for the market account*, named
    // in the registration's own args. Editing the market deploy's `name` leaves
    // those registrations pointing at an account this plan never creates, so the
    // new market would be unable to receive its own assets.
    if let Some(market) = market {
        for step in &file.steps {
            for call in &step.function_calls {
                let Some(registered) = storage_registration_target(call)? else {
                    continue;
                };
                anyhow::ensure!(
                    registered == market.as_str(),
                    "`{}` registers storage for `{registered}`, but this plan \
                     deploys the market as `{market}`. One was edited without the \
                     other, so the market would not be registered with its own \
                     token.",
                    step.label,
                );
            }
        }
    }

    // Governance proposals must address the governance account this plan
    // deploys. Identified positively — by carrying a numeric proposal `id` —
    // rather than as "not a deploy": a NEP-141 market also plans a
    // `storage_deposit` addressed to the *token*, which is neither a deploy nor
    // a proposal, and an exclusion rule would refuse every NEP-141 market.
    if let Some(governance) = governance {
        for step in &file.steps {
            for call in &step.function_calls {
                if !is_governance_proposal(call)? {
                    continue;
                }
                anyhow::ensure!(
                    step.receiver_id == governance,
                    "`{}` is addressed to `{}`, but this plan deploys governance \
                     as `{governance}`. The proposal would be sent to an account \
                     this plan never creates.",
                    step.label,
                    step.receiver_id,
                );
            }
        }
    }
    Ok(())
}

/// A registry deploy must carry `init_args` this build can decode.
///
/// Otherwise every guard reading them — the seated `admin_id`, the oracle and
/// governance coherence checks — skips the step silently, which is the same
/// fail-open that has already appeared three times in this file: a guard asked
/// "is this wrong?" and answered "no" because it could not tell.
fn ensure_init_args_readable(
    step: &crate::spec::plan::PlanStep,
    call: &crate::spec::plan::PlanFunctionCall,
) -> anyhow::Result<()> {
    let bytes = call.args.to_bytes()?;
    let Ok(args) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Ok(());
    };
    if args.get("version_key").is_none() {
        return Ok(());
    }
    anyhow::ensure!(
        decoded_init_args(call).is_some(),
        "`{}` deploys from a registry but its `init_args` cannot be decoded, so \
         what the new contract is initialized with cannot be checked or shown",
        step.label,
    );
    Ok(())
}

/// The account a `storage_deposit` registers, if this call is one.
fn storage_registration_target(
    call: &crate::spec::plan::PlanFunctionCall,
) -> anyhow::Result<Option<String>> {
    let bytes = call.args.to_bytes()?;
    let Ok(args) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Ok(None);
    };
    // Keyed on the pair, so an unrelated `account_id` argument cannot be
    // mistaken for a registration.
    if args.get("registration_only").is_none() {
        return Ok(None);
    }
    let Some(account_id) = args.get("account_id") else {
        return Ok(None);
    };
    Ok(Some(
        account_id
            .as_str()
            .context("a storage registration names a non-string account")?
            .to_owned(),
    ))
}

/// Whether a call is a governance proposal: a numeric proposal `id`, and not a
/// registry deploy.
fn is_governance_proposal(call: &crate::spec::plan::PlanFunctionCall) -> anyhow::Result<bool> {
    let bytes = call.args.to_bytes()?;
    let Ok(args) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Ok(false);
    };
    Ok(args.get("version_key").is_none()
        && args.get("id").is_some_and(serde_json::Value::is_number))
}

/// The account a single registry-deploy call would create.
fn deploy_target(
    step: &crate::spec::plan::PlanStep,
    call: &crate::spec::plan::PlanFunctionCall,
) -> anyhow::Result<Option<AccountId>> {
    let bytes = call.args.to_bytes()?;
    let Ok(args) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Ok(None);
    };
    if args.get("version_key").is_none() {
        return Ok(None);
    }
    let name = args.get("name").and_then(serde_json::Value::as_str);
    let target = name.and_then(|name| {
        format!("{name}.{}", step.receiver_id)
            .parse::<AccountId>()
            .ok()
    });
    Ok(Some(target.with_context(|| {
        format!(
            "`{}` deploys from a registry but names no usable sub-account \
             ({name:?}), so the account it would create cannot be checked \
             for a collision",
            step.label
        )
    })?))
}

/// Refuse a plan that creates the same account twice.
///
/// Coherent edits can still collide with each other: renaming two deploys to
/// the same name keeps every reference consistent, and the second deploy fails
/// after the first has spent its deposit.
fn ensure_targets_are_distinct(targets: &[AccountId]) -> anyhow::Result<()> {
    let mut seen = BTreeSet::new();
    for target in targets {
        anyhow::ensure!(
            seen.insert(target),
            "this plan creates `{target}` more than once; the second deploy would \
             fail against the account the first just made"
        );
    }
    Ok(())
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
            for key in granted_keys(call).unwrap_or_default() {
                eprintln!("      grants full access to: {key}");
            }
            if let Some(init_args) = decoded_init_args(call) {
                eprintln!("      init_args: {init_args}");
            }
        }
    }
}

/// The keys a deploy grants full access to on the account it creates.
///
/// Rendered because they are otherwise invisible: they sit inside the call's
/// args, which are not printed, yet they decide who controls the three new
/// accounts. A mistyped or substituted `--public-key` at plan time is
/// indistinguishable from a correct one unless an operator can see it.
fn granted_keys(call: &crate::spec::plan::PlanFunctionCall) -> anyhow::Result<Vec<String>> {
    let bytes = call.args.to_bytes()?;
    let Ok(args) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        // Not JSON at all (borsh), so it grants no keys — as opposed to args
        // this build could not read, which `to_bytes` already refused above.
        return Ok(Vec::new());
    };
    let Some(keys) = args.get("full_access_keys") else {
        return Ok(Vec::new());
    };
    let keys = keys.as_array().context(
        "`full_access_keys` is not a list, so the keys granted control of this \
         account cannot be read",
    )?;
    keys.iter()
        .map(|key| {
            key.as_str().map(ToOwned::to_owned).context(
                "a granted full-access key is not a string, so it cannot be \
                 checked against yours",
            )
        })
        .collect()
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
        apply_skips(&mut checks, &["reference.price.collateral".to_owned()]);

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
        apply_skips(&mut checks, &["reference.price.collateral".to_owned()]);

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
        let skip = ["config.validte".to_owned()];
        let matched = apply_skips(&mut checks(), &skip);
        let error =
            super::ensure_every_skip_matched(&skip, &matched).expect_err("a typo names no check");

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

    /// Editing the oracle deploy's `name` without updating the two places that
    /// reference it produces a live market wired to an oracle nobody deployed.
    #[test]
    fn an_inconsistently_renamed_oracle_is_refused() {
        use crate::spec::plan::{PlanFunctionCall, PlanStep};
        use base64::Engine as _;

        fn deploy(label: &str, name: &str, init: serde_json::Value) -> PlanStep {
            let encoded = base64::engine::general_purpose::STANDARD
                .encode(serde_json::to_vec(&init).expect("encode init"));
            PlanStep {
                label: label.to_owned(),
                signer_id: "operator.near".parse().expect("valid account"),
                receiver_id: "templar-alpha.near".parse().expect("valid account"),
                function_calls: vec![PlanFunctionCall {
                    method_name: "deploy_market".to_owned(),
                    args: PlanArgs::Json(serde_json::json!({
                        "name": name,
                        "version_key": "v1",
                        "init_args": encoded,
                    })),
                    gas: 300_000_000_000_000,
                    deposit: near_api::types::NearToken::from_near(5),
                }],
            }
        }

        let mut file = bare_plan(PLAN_SCHEMA_VERSION, "mainnet");
        // The oracle is renamed...
        file.steps.push(deploy(
            "deploy oracle",
            "renamed-oracle",
            serde_json::json!({ "owner_id": "gov.templar-alpha.near" }),
        ));
        // ...but the market still points at the original.
        file.steps.push(deploy(
            "deploy market",
            "mkt",
            serde_json::json!({
                "configuration": {
                    "price_oracle_configuration": {
                        "account_id": "proxy-oracle-original.templar-alpha.near"
                    }
                }
            }),
        ));

        let error = super::ensure_plan_is_coherent(&file).expect_err("inconsistent");
        assert!(
            format!("{error:#}").contains("renamed-oracle.templar-alpha.near"),
            "{error:#}"
        );
        assert!(
            format!("{error:#}").contains("market configuration"),
            "{error:#}"
        );
    }

    /// Renaming the governance deploy leaves the oracle owned by, and the
    /// proposals addressed to, an account the plan never creates.
    #[test]
    fn a_renamed_governance_is_refused() {
        use crate::spec::plan::{PlanFunctionCall, PlanStep};
        use base64::Engine as _;

        let encode = |init: &serde_json::Value| {
            base64::engine::general_purpose::STANDARD
                .encode(serde_json::to_vec(init).expect("encode init"))
        };
        let deploy = |label: &str, name: &str, init: serde_json::Value| PlanStep {
            label: label.to_owned(),
            signer_id: "operator.near".parse().expect("valid account"),
            receiver_id: "templar-alpha.near".parse().expect("valid account"),
            function_calls: vec![PlanFunctionCall {
                method_name: "deploy_market".to_owned(),
                args: PlanArgs::Json(serde_json::json!({
                    "name": name,
                    "version_key": "v1",
                    "init_args": encode(&init),
                })),
                gas: 300_000_000_000_000,
                deposit: near_api::types::NearToken::from_near(5),
            }],
        };

        let mut file = bare_plan(PLAN_SCHEMA_VERSION, "mainnet");
        file.steps.push(deploy(
            "deploy governance",
            "gov-renamed",
            serde_json::json!({ "proxy_oracle_id": "o.templar-alpha.near", "admin_id": "a.near" }),
        ));
        // The oracle still names the *old* governance as its owner.
        file.steps.push(deploy(
            "deploy oracle",
            "o",
            serde_json::json!({ "owner_id": "gov.templar-alpha.near" }),
        ));

        let error = super::ensure_plan_is_coherent(&file).expect_err("inconsistent");
        assert!(
            format!("{error:#}").contains("gov-renamed.templar-alpha.near"),
            "{error:#}"
        );
    }

    /// A NEP-141 market plans a `storage_deposit` addressed to the *token* —
    /// neither a deploy nor a proposal. An exclusion rule ("everything that is
    /// not a deploy must address governance") refused every such market, which
    /// is the case this plan builder was fixed to support in the first place.
    #[test]
    fn a_token_storage_deposit_does_not_have_to_address_governance() {
        use crate::spec::plan::{PlanFunctionCall, PlanStep};
        use base64::Engine as _;

        let init = base64::engine::general_purpose::STANDARD.encode(
            serde_json::to_vec(&serde_json::json!({
                "proxy_oracle_id": "o.templar-alpha.near",
                "admin_id": "a.near",
            }))
            .expect("encode"),
        );
        let mut file = bare_plan(PLAN_SCHEMA_VERSION, "mainnet");
        file.steps.push(PlanStep {
            label: "deploy governance".to_owned(),
            signer_id: "operator.near".parse().expect("valid account"),
            receiver_id: "templar-alpha.near".parse().expect("valid account"),
            function_calls: vec![PlanFunctionCall {
                method_name: "deploy_market".to_owned(),
                args: PlanArgs::Json(serde_json::json!({
                    "name": "gov", "version_key": "v1", "init_args": init,
                })),
                gas: 300_000_000_000_000,
                deposit: near_api::types::NearToken::from_near(3),
            }],
        });
        file.steps.push(PlanStep {
            label: "register storage for usdc.near".to_owned(),
            signer_id: "operator.near".parse().expect("valid account"),
            receiver_id: "usdc.near".parse().expect("valid account"),
            function_calls: vec![PlanFunctionCall {
                method_name: "storage_deposit".to_owned(),
                args: PlanArgs::Json(serde_json::json!({
                    "account_id": "mkt.templar-alpha.near",
                    "registration_only": true,
                })),
                gas: 300_000_000_000_000,
                deposit: near_api::types::NearToken::from_millinear(10),
            }],
        });

        super::ensure_plan_is_coherent(&file)
            .expect("a token storage deposit is not a misrouted proposal");
    }

    /// Editing the market deploy's `name` leaves its storage registrations
    /// pointing at an account the plan never creates, so the market could not
    /// receive its own assets. Third instance of the same shape: one reference
    /// edited, the rest stale.
    #[test]
    fn a_renamed_market_orphans_its_storage_registrations() {
        use crate::spec::plan::{PlanFunctionCall, PlanStep};
        use base64::Engine as _;

        let init = base64::engine::general_purpose::STANDARD.encode(
            serde_json::to_vec(&serde_json::json!({ "configuration": { "x": 1 } }))
                .expect("encode"),
        );
        let mut file = bare_plan(PLAN_SCHEMA_VERSION, "mainnet");
        file.steps.push(PlanStep {
            label: "deploy market".to_owned(),
            signer_id: "operator.near".parse().expect("valid account"),
            receiver_id: "templar-alpha.near".parse().expect("valid account"),
            function_calls: vec![PlanFunctionCall {
                method_name: "deploy_market".to_owned(),
                args: PlanArgs::Json(serde_json::json!({
                    "name": "mkt-renamed", "version_key": "v1", "init_args": init,
                })),
                gas: 300_000_000_000_000,
                deposit: near_api::types::NearToken::from_near(5),
            }],
        });
        file.steps.push(PlanStep {
            label: "register storage".to_owned(),
            signer_id: "operator.near".parse().expect("valid account"),
            receiver_id: "usdc.near".parse().expect("valid account"),
            function_calls: vec![PlanFunctionCall {
                method_name: "storage_deposit".to_owned(),
                args: PlanArgs::Json(serde_json::json!({
                    "account_id": "mkt.templar-alpha.near",
                    "registration_only": true,
                })),
                gas: 300_000_000_000_000,
                deposit: near_api::types::NearToken::from_millinear(10),
            }],
        });

        let error = super::ensure_plan_is_coherent(&file).expect_err("stale registration");
        assert!(
            format!("{error:#}").contains("mkt-renamed.templar-alpha.near"),
            "{error:#}"
        );
    }

    /// A non-zero governance TTL makes this plan's own proposals unexecutable
    /// when it runs them. Refused at plan time, and again here because the
    /// value lives in editable step arguments.
    #[test]
    fn a_non_zero_ttl_in_an_edited_plan_is_refused() {
        use crate::spec::plan::{PlanFunctionCall, PlanStep};
        use base64::Engine as _;

        let init = base64::engine::general_purpose::STANDARD.encode(
            serde_json::to_vec(&serde_json::json!({
                "proxy_oracle_id": "o.near",
                "admin_id": "a.near",
                "ttls": { "set_proxy": "600000000000", "rearm": "0" },
            }))
            .expect("encode"),
        );
        let mut file = bare_plan(PLAN_SCHEMA_VERSION, "mainnet");
        file.steps.push(PlanStep {
            label: "deploy governance".to_owned(),
            signer_id: "operator.near".parse().expect("valid account"),
            receiver_id: "templar-alpha.near".parse().expect("valid account"),
            function_calls: vec![PlanFunctionCall {
                method_name: "deploy_market".to_owned(),
                args: PlanArgs::Json(serde_json::json!({
                    "name": "gov", "version_key": "v1", "init_args": init,
                })),
                gas: 300_000_000_000_000,
                deposit: near_api::types::NearToken::from_near(3),
            }],
        });

        let error = super::ensure_initializers_are_sound(&file).expect_err("non-zero ttl");
        assert!(format!("{error:#}").contains("set_proxy"), "{error:#}");
    }

    /// Two deploys renamed to the same account keep every reference coherent
    /// and still collide with each other.
    #[test]
    fn duplicate_targets_are_refused() {
        let shared: near_account_id::AccountId =
            "same.templar-alpha.near".parse().expect("valid account");
        let error = super::ensure_targets_are_distinct(&[shared.clone(), shared])
            .expect_err("two deploys, one account");

        assert!(format!("{error:#}").contains("more than once"), "{error:#}");
    }

    /// Proposal arguments decide whether the oracle is ever configured, and an
    /// unconfigured oracle still reports success (`admin_set_proxy` is
    /// dispatched detached), so these are refused rather than discovered later.
    #[rstest::rstest]
    #[case::raised_ttl(
        serde_json::json!({ "id": 0, "operation": {}, "requested_ttl": "600000000000" }),
        None,
        "not be executable"
    )]
    #[case::execute_without_create(serde_json::json!({ "id": 7 }), None, "does not create it")]
    #[case::duplicate_create(
        serde_json::json!({ "id": 0, "operation": {}, "requested_ttl": "0" }),
        Some(serde_json::json!({ "id": 0, "operation": {}, "requested_ttl": "0" })),
        "already creates"
    )]
    fn unrunnable_proposals_are_refused(
        #[case] first: serde_json::Value,
        #[case] second: Option<serde_json::Value>,
        #[case] expected: &str,
    ) {
        use crate::spec::plan::{PlanFunctionCall, PlanStep};

        let proposal = |args: serde_json::Value| PlanStep {
            label: "proposal".to_owned(),
            signer_id: "operator.near".parse().expect("valid account"),
            receiver_id: "gov.near".parse().expect("valid account"),
            function_calls: vec![PlanFunctionCall {
                method_name: "create_proposal".to_owned(),
                args: PlanArgs::Json(args),
                gas: 300_000_000_000_000,
                deposit: near_api::types::NearToken::from_yoctonear(1),
            }],
        };

        let mut file = bare_plan(PLAN_SCHEMA_VERSION, "mainnet");
        file.steps.push(proposal(first));
        if let Some(second) = second {
            file.steps.push(proposal(second));
        }

        let error = super::ensure_proposals_are_runnable(&file).expect_err("unrunnable");
        assert!(format!("{error:#}").contains(expected), "{error:#}");
    }

    /// Deleting a step disables the guard that validates it, so completeness is
    /// checked before coherence. Same fail-open shape as the swallowed `Err`s,
    /// in the form where the value is absent rather than unreadable.
    #[test]
    fn a_truncated_plan_is_refused() {
        use crate::spec::plan::{PlanFunctionCall, PlanStep};
        use base64::Engine as _;

        let deploy = |init: serde_json::Value| PlanStep {
            label: "deploy".to_owned(),
            signer_id: "operator.near".parse().expect("valid account"),
            receiver_id: "templar-alpha.near".parse().expect("valid account"),
            function_calls: vec![PlanFunctionCall {
                method_name: "deploy_market".to_owned(),
                args: PlanArgs::Json(serde_json::json!({
                    "name": "x",
                    "version_key": "v1",
                    "init_args": base64::engine::general_purpose::STANDARD
                        .encode(serde_json::to_vec(&init).expect("encode")),
                })),
                gas: 300_000_000_000_000,
                deposit: near_api::types::NearToken::from_near(5),
            }],
        };

        let mut file = bare_plan(PLAN_SCHEMA_VERSION, "mainnet");
        // Governance and market, but no oracle: the two survivors reference an
        // oracle nothing deploys, and every oracle check would be skipped.
        file.steps.push(deploy(
            serde_json::json!({ "proxy_oracle_id": "o.near", "admin_id": "a.near" }),
        ));
        file.steps
            .push(deploy(serde_json::json!({ "configuration": { "x": 1 } })));

        let error = super::ensure_plan_is_complete(&file).expect_err("incomplete");
        assert!(
            format!("{error:#}").contains("proxy-oracle deploy"),
            "{error:#}"
        );
    }

    /// A coherent plan passes.
    #[test]
    fn a_coherent_oracle_reference_is_accepted() {
        use crate::spec::plan::{PlanFunctionCall, PlanStep};
        use base64::Engine as _;

        let encoded = base64::engine::general_purpose::STANDARD.encode(
            serde_json::to_vec(&serde_json::json!({ "owner_id": "gov.near" })).expect("encode"),
        );
        let mut file = bare_plan(PLAN_SCHEMA_VERSION, "mainnet");
        file.steps.push(PlanStep {
            label: "deploy oracle".to_owned(),
            signer_id: "operator.near".parse().expect("valid account"),
            receiver_id: "templar-alpha.near".parse().expect("valid account"),
            function_calls: vec![PlanFunctionCall {
                method_name: "deploy_market".to_owned(),
                args: PlanArgs::Json(serde_json::json!({
                    "name": "o",
                    "version_key": "v1",
                    "init_args": encoded,
                })),
                gas: 300_000_000_000_000,
                deposit: near_api::types::NearToken::from_near(5),
            }],
        });

        super::ensure_plan_is_coherent(&file).expect("nothing contradicts it");
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
