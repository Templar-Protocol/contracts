//! `market plan` / `market apply` — generate a deployment as a file, then send
//! that file. Splitting the two puts a reviewable artifact between them; the
//! artifact itself is [`crate::spec::plan`].

use std::collections::BTreeSet;
use std::io::Write as _;

use anyhow::Context as _;
use near_account_id::AccountId;
use near_api::types::NearToken;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_common::Nanoseconds;
use templar_gateway_client::{Client, Network};
use templar_gateway_core::{
    GatewayContext, GatewayResult, OperationPlan, PlanWrite, PlannedTransaction,
};
use templar_gateway_methods_dispatch::Dispatch;
use templar_gateway_methods_spec::{
    market, proxy_oracle, proxy_oracle_governance as gov, registry,
};
use templar_gateway_types::common::{WriteOperationResult, WriteRequest};
use templar_gateway_types::{primitive::PublicKey, Base64Bytes, MethodSpec, ProposalEncoding};
use templar_proxy_oracle_kernel::proxy::Proxy;
use templar_proxy_oracle_near_common::input::Source;
use templar_proxy_oracle_near_governance_common::{GovernancePolicy, Operation};

use crate::commands::market::{Apply, Plan};
use crate::commands::proxy_oracle::governance::GovernanceInit;
use crate::context::{print_json, CliContext};
use crate::spec::journal::{self, Journal};
use crate::spec::{
    check::{Check, Status},
    plan::{PlanFile, PLAN_SCHEMA_VERSION},
    GovernanceSpec, MarketSpec, BORROW_PRICE_ID, COLLATERAL_PRICE_ID,
};

/// Deposits funding each new account's storage and balance. These size a
/// *contract's* storage staking, which follows from the code being deployed
/// rather than from the market.
///
/// The registry refuses a deposit below `1e19 * code.len()`, and the market's is
/// the last step — undersized there, it fails after 9.9 NEAR is spent. Each keeps
/// 50 KB of headroom over its pinned release, held there by
/// `requires_network_deposits_cover_the_released_artifacts`. Over-provisioning
/// burns nothing: the deposit is forwarded to the account being created.
const GOVERNANCE_DEPOSIT: NearToken = NearToken::from_millinear(4_500);
const ORACLE_DEPOSIT: NearToken = NearToken::from_millinear(5_400);
const MARKET_DEPOSIT: NearToken = NearToken::from_millinear(5_800);

/// `market plan` — run the preflight, then write the deployment as a file.
pub(super) async fn plan(ctx: CliContext, args: Plan) -> anyhow::Result<()> {
    let mut spec = crate::spec::extends::load(&args.path)?;

    let mut checks =
        super::preflight::run_all(&ctx, &mut spec, false, args.accept_decimals_mismatch, None)
            .await?;
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
    gate(&checks, spec.market_id()?.as_str())?;

    let public_key = PublicKey::from(args.public_key);
    let steps =
        PlanFile::steps_from(build(&ctx.client, &spec, &public_key, &args.signer_id).await?)?;

    // After the steps exist, because it reads them; before the plan is written,
    // because a signer that cannot pay is a reason not to write one.
    let mut funding = super::funding::checks(&ctx, &steps).await?;
    matched.extend(apply_skips(&mut funding, &args.skip_check));
    ensure_every_skip_matched(&args.skip_check, &matched)?;
    checks.extend(funding);
    gate(&checks, spec.market_id()?.as_str())?;

    let file = PlanFile::from_steps(spec, public_key, checks, steps);

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

    ensure_compatible(&file, ctx.network())?;

    // Each step names its own signer; `execute_as` only labels the store record.
    // A credential for some other account fails at step 0 with an opaque
    // executor error, so it is resolved here and checked below.
    let credential = args.signer.account_id().0;
    ensure_signed_by(&file, &credential)?;

    // What the spec derives right now. Re-derived rather than inspected: a plan
    // is a record of this derivation, so anything an edit could break is caught
    // by one comparison instead of a guard per property.
    let expected =
        PlanFile::steps_from(build(&ctx.client, &file.spec, &file.public_key, &credential).await?)?;

    // Reconciled before anything else that reads the steps: if the journal and
    // the plan disagree, nothing below is meaningful.
    let journal_path = journal::path_for(&args.plan);
    let journal = Journal::load(&journal_path)?;
    let remaining = journal.remaining(&file)?;

    // Before the early return: a plan truncated to exactly its completed prefix
    // leaves nothing outstanding, and would otherwise report success for having
    // deployed part of a market.
    ensure_matches_spec(&file, &expected, &remaining)?;

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
    // An existing oracle's breakers decide whether the market can price at all,
    // so they must be read rather than assumed empty.
    let deployed_oracle = match file.spec.own_proxy_id()? {
        Some(oracle_id) if super::preflight::exists(&ctx, &oracle_id).await? => Some(oracle_id),
        _ => None,
    };

    // Re-run against the chain as it is now, not replayed from the artifact.
    // `file.checks` records what was true when the plan was written, and a feed
    // can go stale or a version be removed while a plan is being read — every
    // one of those is a reason not to send. It is also what makes a non-funding
    // `--skip-check` mean anything here, which `Apply`'s own help promises.
    let mut checks = super::preflight::run_all(
        &ctx,
        &mut file.spec.clone(),
        false,
        // The spec states `decimals` explicitly or it does not, and a declared
        // value disagreeing with the token was reviewed when the plan was
        // written. `apply` cannot re-ask, and its job is catching what *changed*,
        // so the mismatch is reported rather than refused.
        true,
        deployed_oracle.as_ref(),
    )
    .await?;
    checks.extend(super::funding::checks(&ctx, &outstanding).await?);
    let matched = apply_skips(&mut checks, &args.skip_check);
    ensure_every_skip_matched(&args.skip_check, &matched)?;

    let targets = review(&ctx, &file, &checks, &outstanding, &journal_path).await?;
    gate(&checks, file.spec.market_id()?.as_str())?;

    // Resolved once, here, and carried through to the send: the keychain
    // backend discovers keys on chain and may prompt, and a second resolution
    // could hand back a different key than the one checked below.
    let (signer, client, signing_key) = ctx.signing_client_and_key(&args.signer).await?;

    // Asked of the backend, not of `--public-key`: checking a grant against
    // the operator's own assertion checks nothing.
    {
        // Compared through the JSON encoding, which is the form the args carry.
        let mine = serde_json::to_value(signing_key)
            .ok()
            .and_then(|key| key.as_str().map(ToOwned::to_owned))
            .unwrap_or_default();
        ensure_granted_keys_are_yours(&file, &mine)?;
    }

    if !args.yes {
        confirm(&format!(
            "Send {} transaction(s) as {credential}?",
            remaining.len(),
        ))?;
    }

    // Again, immediately before sending: the prompt above has no time limit, and
    // the check is only worth what it is worth at the moment of the send.
    ensure_targets_free(&ctx, &file.spec.registry, &targets).await?;

    send(
        &ctx,
        &client,
        &signer,
        &file,
        remaining,
        journal,
        &journal_path,
    )
    .await
}

/// Send the outstanding steps, journalling each as it lands.
async fn send(
    ctx: &CliContext,
    client: &Client,
    signer: &templar_gateway_types::ManagedAccountId,
    file: &PlanFile,
    remaining: Vec<usize>,
    mut journal: Journal,
    journal_path: &std::path::Path,
) -> anyhow::Result<()> {
    // One transaction per call, so every outcome is journalled as it happens.
    // Batching would leave an interrupted run with nothing recorded — the only
    // run a journal exists for.
    let plan = file.clone().into_operation_plan()?;

    let market_id = file.spec.market_id()?;
    for index in remaining {
        let step = &file.steps[index];

        // `execute_proposal` dispatches `admin_set_proxy` detached, so a proposal
        // reports success even when the oracle rejected it. Read the feeds back
        // before the deploy that spends the market's own 5.8 NEAR — anchored to
        // the step that creates it, which storage registrations follow.
        if creates(step, &market_id)? {
            ensure_feeds_are_configured(ctx, &file.spec).await?;
        }

        eprintln!("\n[{index}] {}", step.label);

        // Recorded before submission. A step that was sent and never resolved
        // must not read as one that never ran: re-sending a registry deploy
        // that actually succeeded strands its deposit, since the failed
        // `create_account` refunds to the registry, not to the operator.
        let entry = journal::Entry {
            step: index,
            digest: journal::executable_digest(step)?,
            label: step.label.clone(),
            outcome: journal::Outcome::Attempted,
            tx_hash: None,
        };
        journal.record(journal_path, entry.clone())?;

        let output = client
            .via::<PlanDispatch>()
            .execute_as(
                signer.clone(),
                PreparedPlan {
                    steps: vec![plan.steps[index].clone()],
                },
            )
            .await
            .with_context(|| {
                format!(
                    "step {index} (`{}`) did not complete. It is recorded as \
                     attempted in {}; re-run `market apply`, which will stop and \
                     ask you to confirm what happened to it.",
                    step.label,
                    journal_path.display(),
                )
            })?;
        ctx.finish_write(&output)?;

        journal.record(
            journal_path,
            journal::Entry {
                outcome: journal::Outcome::Completed,
                tx_hash: output
                    .operation
                    .latest_tx_hash()
                    .map(|hash| hash.to_string()),
                ..entry
            },
        )?;
    }
    Ok(())
}

/// Show the plan, then everything that must hold before it is sent.
///
/// Extracted from `apply` as one unit because it is one phase — nothing here
/// sends anything, and every check is a reason not to. Returns the two values
/// the send itself needs: who is signing, and the accounts that must still be
/// free at the moment of the send.
async fn review(
    ctx: &CliContext,
    file: &PlanFile,
    checks: &[Check],
    outstanding: &[crate::spec::plan::PlanStep],
    journal_path: &std::path::Path,
) -> anyhow::Result<Vec<AccountId>> {
    render(file);
    report_checks(checks);

    // Re-read, not trusted from the plan: a target free at plan time can be
    // claimed while the plan is being reviewed. Over the outstanding steps only,
    // since a resume has already created what its completed steps made.
    let targets = planned_targets(outstanding)?;
    ensure_targets_free(ctx, &file.spec.registry, &targets).await?;

    // Still worth stating before the money moves: the operator should know a
    // partial run is recoverable *and* where the record lives, rather than
    // discovering both after an interruption.
    if outstanding.len() > 1 {
        eprintln!(
            "\nThis sends {} transactions in sequence. Each is recorded in {} as \
             it lands, so an interruption resumes from the next incomplete step \
             rather than restarting.",
            outstanding.len(),
            journal_path.display(),
        );
    }

    Ok(targets)
}

/// Refuse to create the market unless its oracle serves what the spec says.
async fn ensure_feeds_are_configured(ctx: &CliContext, spec: &MarketSpec) -> anyhow::Result<()> {
    let Some(oracle_id) = spec.own_proxy_id()? else {
        return Ok(());
    };
    let age = spec.market.price_maximum_age;

    for (side, id, intended) in [
        (
            "collateral",
            COLLATERAL_PRICE_ID,
            spec.collateral.clone().into_proxy(age),
        ),
        (
            "borrow",
            BORROW_PRICE_ID,
            spec.borrow.clone().into_proxy(age),
        ),
    ] {
        let deployed = super::export::proxy(ctx, &oracle_id, id)
            .await
            .with_context(|| format!("read the {side} proxy back from `{oracle_id}`"))?;
        anyhow::ensure!(
            deployed == intended,
            "`{oracle_id}` does not serve the {side} feed this plan configured. \
             The proposal reported success because `admin_set_proxy` is dispatched \
             detached; creating the market now would put it live against an oracle \
             that cannot price it. Re-run the proposal by hand with \
             `proxy-oracle governance create-proposal --execute-when-ready`."
        );
    }
    Ok(())
}

/// Refuse a plan somebody else is meant to send.
///
/// Ahead of the re-derivation, which takes the credential as an input: a
/// mismatch there surfaces as every step differing, which is true but says
/// nothing about the one thing that is actually wrong.
fn ensure_signed_by(file: &PlanFile, credential: &AccountId) -> anyhow::Result<()> {
    let signers: BTreeSet<&AccountId> = file.steps.iter().map(|step| &step.signer_id).collect();
    anyhow::ensure!(
        signers.iter().all(|signer| *signer == credential),
        "this plan is signed by {}, but the credential given is for `{credential}`. \
         Re-run with a matching --signer-id, or re-plan.",
        signers
            .iter()
            .map(|signer| format!("`{signer}`"))
            .collect::<Vec<_>>()
            .join(", "),
    );
    Ok(())
}

/// Refuse a plan that is not what its spec derives.
///
/// A step the spec no longer derives is accepted only where the journal says it
/// already ran: the gateway stops planning a storage registration once the
/// account is registered, which is what this plan's own completed steps did.
pub(crate) fn ensure_matches_spec(
    file: &PlanFile,
    expected: &[crate::spec::plan::PlanStep],
    remaining: &[usize],
) -> anyhow::Result<()> {
    let mut derived = expected.iter().peekable();
    for (index, step) in file.steps.iter().enumerate() {
        if derived
            .peek()
            .is_some_and(|next| next.executes_the_same_as(step))
        {
            derived.next();
            continue;
        }
        anyhow::ensure!(
            !remaining.contains(&index),
            "step {index} (`{}`) is not what `{}` derives. This artifact records \
             a derivation rather than driving one, so it cannot be edited: change \
             the spec and re-plan, or run the step yourself with the `tmplrmgr` \
             command that performs it.",
            step.label,
            file.spec.name,
        );
    }

    match derived.next() {
        None => Ok(()),
        Some(missing) => anyhow::bail!(
            "this plan is missing `{}`, which `{}` derives. Applying it would \
             deploy part of a market and report success; re-plan.",
            missing.label,
            file.spec.name,
        ),
    }
}

/// The deployment, in order. Governance first, because it must own the oracle
/// before any feed can be configured; see [`ensure_steps_are_ordered`].
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
    if let Some((governance, _, _)) = spec.proxy() {
        anyhow::ensure!(
            governance.ttl_default == Nanoseconds::from_ns(0),
            "`governance.ttl_default` is {}ns, so the proxy proposals would not be \
             executable when created, and a plan cannot wait. Deploy with \
             `ttl_default = \"0s\"` and raise the TTL afterwards with a `set-action-ttl` \
             proposal, or run the proposals by hand with `proxy-oracle governance \
             execute-proposal --when-ready`.",
            governance.ttl_default.as_ns(),
        );

        // The proposals are created and executed by `signer_id`, but only
        // `governance.admin` is granted the Admin role at init. Mismatched, the two
        // registry deploys succeed and every proposal reverts — 9.9 NEAR spent on
        // exactly the orphaned half-deployment this tool exists to prevent.
        // The retired shell deploy made this unrepresentable: it passed the
        // signer as the admin.
        anyhow::ensure!(
            &governance.admin == signer_id,
            "`governance.admin` is `{}` but this plan is signed by `{signer_id}`, \
             which would not hold the Admin role. Every proxy proposal would revert \
             after the governance and oracle deploys had already spent their \
             deposits. Set them to the same account.",
            governance.admin,
        );
    }

    let price_maximum_age = spec.market.price_maximum_age;
    let full_access_keys = Some(vec![public_key.clone()]);

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

    // Re-run here, not left to the `config.validate` check: that check is
    // skippable, and the market enforces this at init. Skipping it can only buy
    // a 9.9 NEAR half-deployment that reverts on the last step.
    configuration
        .validate()
        .map_err(|error| anyhow::anyhow!("{error}"))
        .context("the market contract would reject this configuration at init")?;

    // A direct market reads an oracle that already exists, so a deployment
    // creates only the market: no governance to seat, no proxy to own, no
    // proposals to configure.
    let mut steps = Vec::new();
    if let Some((governance, oracle_version, governance_version)) = spec.proxy() {
        steps = oracle_stack(
            client,
            signer_id,
            spec,
            public_key,
            governance,
            oracle_version,
            governance_version,
        )
        .await?;

        // Proposal ids start at zero because this plan creates the governance
        // contract they run against. Both sides project to the same
        // `Proxy<Source>`, so one loop covers them.
        let governance_id = spec.governance_id()?;
        for feed in [
            Feed {
                side: "collateral",
                price_id: COLLATERAL_PRICE_ID,
                proposal_id: 0,
                proxy: spec.collateral.clone().into_proxy(price_maximum_age),
            },
            Feed {
                side: "borrow",
                price_id: BORROW_PRICE_ID,
                proposal_id: 1,
                proxy: spec.borrow.clone().into_proxy(price_maximum_age),
            },
        ] {
            steps.extend(
                set_proxy(
                    client,
                    signer_id,
                    governance_id.clone(),
                    feed,
                    governance.ttl_default,
                )
                .await?,
            );
        }
    }

    steps.extend(
        step(
            client,
            signer_id,
            &format!("deploy market {}", spec.market_id()?),
            market::Create {
                registry_id: spec.registry.clone(),
                name: spec.name.clone(),
                version_key: spec.market_version.clone(),
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
    governance: &GovernanceSpec,
    oracle_version: &str,
    governance_version: &str,
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
    crate::commands::proxy_oracle::check_owner_id_is_honored(oracle_version, &governance_id)?;

    let governance_init = serde_json::to_vec(&GovernanceInit {
        proxy_oracle_id: oracle_id.clone(),
        admin_id: governance.admin.clone(),
        // One TTL for every reflexive bucket and for the target default, which
        // is what a spec's single `ttl_default` means. A deployment needing
        // per-method timelocks raises them afterwards by proposal.
        policy: GovernancePolicy::uniform(governance.ttl_default)
            .context("build the governance policy from `ttl_default`")?,
    })
    .context("encode governance init args")?;

    let mut steps = step(
        client,
        signer_id,
        &format!("deploy governance {governance_id}"),
        registry::Deploy {
            registry_id: spec.registry.clone(),
            name: crate::spec::governance_name(&spec.name),
            version_key: governance_version.to_owned(),
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
                version_key: oracle_version.to_owned(),
                owner_id: Some(governance_id),
                full_access_keys: full_access_keys.clone(),
                deposit: ORACLE_DEPOSIT,
            },
        )
        .await?,
    );
    Ok(steps)
}

/// One side's feed, as a proposal to configure it.
struct Feed {
    side: &'static str,
    price_id: PriceIdentifier,
    proposal_id: u32,
    proxy: Proxy<Source>,
}

/// Propose a feed's proxy, then execute that proposal. Two transactions: a
/// proposal is always created before it can run, even at `ttl_default = 0`.
async fn set_proxy(
    client: &Client,
    signer_id: &AccountId,
    governance_id: AccountId,
    feed: Feed,
    ttl_default: Nanoseconds,
) -> anyhow::Result<Vec<(String, PlannedTransaction)>> {
    let Feed {
        side,
        price_id,
        proposal_id,
        proxy,
    } = feed;

    let mut steps = step(
        client,
        signer_id,
        &format!("propose {side} proxy (proposal {proposal_id})"),
        gov::CreateProposal {
            governance_id: governance_id.clone(),
            id: proposal_id,
            operation: Operation::TargetFunctionCall(
                templar_proxy_oracle_near_governance_common::target::admin_set_proxy(
                    price_id,
                    Some(proxy),
                    None,
                )
                .context("encode the `admin_set_proxy` call")?,
            ),
            requested_ttl: ttl_default,
            encoding: ProposalEncoding::Json,
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

/// Plan one write, labelling every transaction it needs. Not one per write:
/// `market.create` also registers storage per NEP-141 asset, so a market plans
/// two or three, and each is numbered when it expands.
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
fn gate(checks: &[Check], market_id: &str) -> anyhow::Result<()> {
    if crate::spec::check::failures(checks) > 0 {
        print_json(&checks)?;
        crate::spec::check::gate(checks, market_id, "no plan was written")?;
    }
    Ok(())
}

/// The only two hard refusals: this build cannot read a foreign schema
/// faithfully, and a network mismatch would send a mainnet deployment to testnet
/// or the reverse. Everything else is reported and confirmed, never blocked.
fn ensure_compatible(file: &PlanFile, network: Network) -> anyhow::Result<()> {
    anyhow::ensure!(
        file.schema == PLAN_SCHEMA_VERSION,
        "this plan declares schema {} but this build speaks {PLAN_SCHEMA_VERSION}. \
         Regenerate it with `market plan`.",
        file.schema,
    );
    // Asked of the spec, which derives it from the registry account. A network
    // stated beside the spec would be a second place to be wrong, and the one an
    // edit would reach.
    let declared = file.spec.network()?;
    anyhow::ensure!(
        declared == network,
        "this plan deploys to `{}`, which is {declared}, but the CLI is pointed \
         at {network}. Re-run with `--network {declared}`.",
        file.spec.registry,
    );
    Ok(())
}

/// The accounts these steps would create.
pub(crate) fn planned_targets(
    steps: &[crate::spec::plan::PlanStep],
) -> anyhow::Result<Vec<AccountId>> {
    let mut targets = Vec::new();
    for step in steps {
        for call in &step.function_calls {
            targets.extend(deploy_target(step, call)?);
        }
    }
    Ok(targets)
}

/// Whether this step is the registry deploy that creates `account_id`.
fn creates(step: &crate::spec::plan::PlanStep, account_id: &AccountId) -> anyhow::Result<bool> {
    for call in &step.function_calls {
        if deploy_target(step, call)?.as_ref() == Some(account_id) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The account a single registry-deploy call would create, recognized by `name`
/// beside a `version_key`. Fails closed: a `version_key` whose target cannot be
/// derived is an unreviewable step, not an absent one.
fn deploy_target(
    step: &crate::spec::plan::PlanStep,
    call: &crate::spec::plan::PlanFunctionCall,
) -> anyhow::Result<Option<AccountId>> {
    let Some(args) = json_args(call)? else {
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

/// Why `account_id` cannot be deployed to, if it cannot.
///
/// A free account is not a free name: `registry.deploy` refuses any id its
/// deployment map still holds, and `market remove` deletes the account without
/// removing that entry, so a torn-down name is free and unusable at once.
///
/// Best-effort in one direction: `get_deployment` reports a `Reserved` entry as
/// absent, and no registry view distinguishes it, so a name another deploy has
/// reserved but not yet created is invisible here. That window is a concurrent
/// in-flight deploy of the same name — a failed one removes its entry — and the
/// registry rejects it either way, so the unseen case costs a failed step
/// rather than a bad deployment. Closing it needs an additive view.
async fn target_conflict(
    ctx: &CliContext,
    registry_id: &AccountId,
    account_id: &AccountId,
) -> anyhow::Result<Option<String>> {
    if super::preflight::exists(ctx, account_id).await? {
        return Ok(Some(format!("`{account_id}` already exists")));
    }

    let claimed = ctx
        .client
        .read(registry::GetDeployment {
            registry_id: registry_id.clone(),
            account_id: account_id.clone(),
        })
        .await
        .with_context(|| format!("read `{registry_id}`'s deployment record for `{account_id}`"))?
        .deployment
        .is_some();

    Ok(claimed.then(|| {
        format!(
            "`{registry_id}` still records a deployment for `{account_id}`, so it \
             would be refused as a collision even though the account is gone"
        )
    }))
}

/// The same question `targets_available` asks at plan time, re-asked at the
/// moment of the send: a target free then can be claimed while a plan is read.
async fn ensure_targets_free(
    ctx: &CliContext,
    registry_id: &AccountId,
    targets: &[AccountId],
) -> anyhow::Result<()> {
    let mut conflicts = Vec::new();
    for account_id in targets {
        conflicts.extend(target_conflict(ctx, registry_id, account_id).await?);
    }
    anyhow::ensure!(
        conflicts.is_empty(),
        "this plan cannot be applied: {}. Re-plan under a different `name`, or \
         tear the existing deployment down.",
        conflicts.join("; "),
    );
    Ok(())
}

/// Every account this deployment creates must be free.
async fn targets_available(ctx: &CliContext, spec: &MarketSpec) -> anyhow::Result<Vec<Check>> {
    // Only what this deployment creates. A direct market creates just itself,
    // and reporting the derived proxy names as "free" would tell an operator
    // this deploy is about to make accounts it never touches.
    let targets = if spec.oracle.is_direct() {
        vec![("market", spec.market_id()?)]
    } else {
        vec![
            ("governance", spec.governance_id()?),
            ("oracle", spec.oracle_id()?),
            ("market", spec.market_id()?),
        ]
    };

    let mut checks = Vec::new();
    for (label, account_id) in targets {
        let status = match target_conflict(ctx, &spec.registry, &account_id).await {
            Ok(None) => Status::passed(format!("`{account_id}` is free")),
            Ok(Some(conflict)) => {
                Status::failed(format!("{conflict}; the {label} deploy would fail"))
            }
            Err(error) => Status::failed(format!("{error:#}")),
        };
        checks.push(Check::new(format!("deployment.available.{label}"), status));
    }
    Ok(checks)
}

/// The checks as they stand now, shown before the prompt because `apply` is
/// often run by someone other than whoever planned, and later. Only non-passing
/// ones: burying an override in a wall of green is how it stops being reviewed.
fn report_checks(checks: &[Check]) {
    let notable: Vec<_> = checks
        .iter()
        .filter(|check| !matches!(check.status, Status::Passed { .. }))
        .collect();

    let failed = crate::spec::check::failures(checks);
    eprintln!(
        "\n{} check(s): {} passed, {} not run, {failed} FAILED",
        checks.len(),
        checks.len() - notable.len(),
        notable.len() - failed,
    );
    for check in notable {
        eprintln!("  {} — {:?}", check.id, check.status);
    }
    if failed > 0 {
        eprintln!(
            "These are re-run now, not read from the plan. A check that passed \
             when the plan was written and fails here means the chain moved \
             under it."
        );
    }
}

/// The plan, for a human about to authorize it.
fn render(file: &PlanFile) {
    eprintln!(
        "Plan for {}.{} on {}",
        file.spec.name,
        file.spec.registry,
        file.spec
            .network()
            .map_or_else(|_| "?".to_owned(), |n| n.to_string())
    );
    for (index, step) in file.steps.iter().enumerate() {
        eprintln!("\n  [{index}] {}", step.label);
        eprintln!("      {} -> {}", step.signer_id, step.receiver_id);
        for call in &step.function_calls {
            eprintln!(
                "      {}  deposit {}  gas {}",
                call.method_name, call.deposit, call.gas
            );
            // Reported rather than swallowed: this is the only place an
            // operator sees the keys and the init args, so a display that
            // silently shows neither is worse than one that says why.
            match granted_keys(call) {
                Ok(keys) => {
                    for key in keys {
                        eprintln!("      grants full access to: {key}");
                    }
                }
                Err(error) => eprintln!("      keys unreadable: {error:#}"),
            }
            match decoded_init_args(call) {
                Ok(Some(init_args)) => eprintln!("      init_args: {init_args}"),
                Ok(None) => {}
                Err(error) => eprintln!("      init_args unreadable: {error:#}"),
            }
        }
    }
}

/// Refuse a plan that would leave the applier without a key on an account it
/// creates.
///
/// Presence, not exclusivity: `--with-full-access-key` grants co-owners
/// deliberately, and `render` prints every granted key before the confirmation
/// prompt. The irreversible loss is paying to create an account you cannot use.
fn ensure_granted_keys_are_yours(file: &PlanFile, mine: &str) -> anyhow::Result<()> {
    for step in &file.steps {
        for call in &step.function_calls {
            let granted = granted_keys(call)?;
            anyhow::ensure!(
                granted.is_empty() || granted.iter().any(|key| key == mine),
                "`{}` grants full access to {}, none of which you hold. Applying \
                 this would hand control of the new account to a key you do not \
                 have; re-plan with your own --public-key.",
                step.label,
                granted
                    .iter()
                    .map(|key| format!("`{key}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
    }
    Ok(())
}

/// The keys a deploy grants full access to on the account it creates.
///
/// Rendered because they are otherwise invisible: they sit inside the call's
/// args, which are not printed, yet they decide who controls the three new
/// accounts. A mistyped or substituted `--public-key` at plan time is
/// indistinguishable from a correct one unless an operator can see it.
fn granted_keys(call: &crate::spec::plan::PlanFunctionCall) -> anyhow::Result<Vec<String>> {
    // Opaque (borsh) args grant no keys, as opposed to args this build cannot
    // encode at all — which `json_args` refuses rather than reports as empty.
    let Some(args) = json_args(call)? else {
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

/// A registry deploy's `init_args`, decoded for display and for the guards built
/// on it. Stays base64 in the file: expanding it there would need a byte-exact
/// round trip, which is a schema change.
fn decoded_init_args(
    call: &crate::spec::plan::PlanFunctionCall,
) -> anyhow::Result<Option<serde_json::Value>> {
    use base64::Engine as _;

    let decode = || {
        let args = json_args(call).ok()??;
        let encoded = args.get("init_args")?.as_str()?;
        let init = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .ok()?;
        serde_json::from_slice(&init).ok()
    };
    // Args that cannot be encoded are never "no init args": they cannot be sent
    // at all, so the failure propagates rather than reading as absence.
    call.args.to_bytes()?;
    Ok(decode())
}

/// A call's arguments as JSON, or `None` when they are opaque.
///
/// Opaque is legitimate — `registry.add_version` takes borsh — so each caller
/// decides what absence means. Args this build cannot *encode* are a different
/// failure: the step cannot be sent, so that always propagates rather than
/// reading as "nothing to check".
fn json_args(
    call: &crate::spec::plan::PlanFunctionCall,
) -> anyhow::Result<Option<serde_json::Value>> {
    let bytes = call.args.to_bytes()?;
    Ok(serde_json::from_slice(&bytes).ok())
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

/// A plan that is already built, so it rides the same executor as every other
/// write — store, idempotency and recovery come along rather than being
/// reimplemented here.
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
    use super::{apply_skips, ensure_compatible, Network, PLAN_SCHEMA_VERSION};
    use super::{GOVERNANCE_DEPOSIT, MARKET_DEPOSIT, ORACLE_DEPOSIT};
    use crate::spec::check::{Check, Status};
    use crate::spec::plan::testing::{alpha_market, public_key};
    use crate::spec::plan::{PlanArgs, PlanFile, PlanFunctionCall, PlanStep};
    use rstest::rstest;
    use templar_contract_artifacts::ArtifactId;

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

    /// A suppressed failure must still say what it was: `Skipped` also means
    /// "not run", so hiding a known failure in it turns a red check green with
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

    fn bare_plan(schema: u32) -> PlanFile {
        PlanFile {
            schema,
            tool_version: "0.1.0".to_owned(),
            spec: alpha_market(),
            public_key: public_key(),
            checks: Vec::new(),
            steps: Vec::new(),
        }
    }

    /// One step against `templar-alpha.near`, carrying the given args.
    fn step_with(label: &str, args: PlanArgs) -> PlanStep {
        PlanStep {
            label: label.to_owned(),
            signer_id: "operator.near".parse().expect("valid account"),
            receiver_id: "templar-alpha.near".parse().expect("valid account"),
            function_calls: vec![PlanFunctionCall {
                method_name: "deploy_market".to_owned(),
                args,
                gas: 300_000_000_000_000,
                deposit: near_api::types::NearToken::from_near(5),
            }],
        }
    }

    /// A step whose only distinguishing feature is its label.
    fn step(label: &str) -> PlanStep {
        step_with(label, PlanArgs::Json(serde_json::json!({ "name": label })))
    }

    /// The steps of a plan carrying one deploy with the given args.
    fn steps_with(args: PlanArgs) -> Vec<PlanStep> {
        vec![step_with("deploy market", args)]
    }

    /// The registry refuses `deposit < 1e19 * code.len()`, so a contract that
    /// outgrows its constant fails the deploy — the market's on the last step,
    /// after 9.9 NEAR is spent. Releasing a larger contract must therefore break
    /// a test, not a deployment.
    ///
    /// Network-gated: the released bytes are fetched, not vendored. Cached after
    /// the first run.
    #[rstest]
    #[case(ArtifactId::ProxyGovernance, GOVERNANCE_DEPOSIT)]
    #[case(ArtifactId::ProxyOracle, ORACLE_DEPOSIT)]
    #[case(ArtifactId::Market, MARKET_DEPOSIT)]
    #[tokio::test]
    async fn requires_network_deposits_cover_the_released_artifacts(
        #[case] artifact: ArtifactId,
        #[case] deposit: near_api::types::NearToken,
    ) {
        const YOCTO_PER_BYTE: u128 = 10u128.pow(19);
        const HEADROOM_BYTES: u128 = 50_000;

        let version = artifact
            .metadata()
            .version()
            .expect("a released version is catalogued for this artifact");
        let bytes = templar_contract_artifacts::fetch::released_bytes(artifact, version)
            .await
            .expect("fetch the released wasm");

        let required = bytes.len() as u128 * YOCTO_PER_BYTE;
        let covered = deposit.as_yoctonear();

        assert!(
            covered >= required + HEADROOM_BYTES * YOCTO_PER_BYTE,
            "{artifact:?} {version} is {} bytes, needing {required} yocto, and the \
             deposit is {covered}; raise the constant so it keeps 50 KB of room \
             to grow",
            bytes.len(),
        );
    }

    /// `ensure_targets_free` acts on what executes, which is the steps.
    #[test]
    fn targets_come_from_the_steps() {
        let steps = steps_with(PlanArgs::Json(serde_json::json!({
            "name": "some-market",
            "version_key": "v1.3.0",
        })));

        assert_eq!(
            super::planned_targets(&steps)
                .expect("derivable")
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            vec!["some-market.templar-alpha.near"],
        );
    }

    /// The comparison that replaced the guard suite. Every shape below used to
    /// need a guard of its own; each is now the same mismatch.
    #[rstest]
    #[case::unchanged(&["a", "b", "c"], &[], None)]
    #[case::truncated(&["a", "b"], &[], Some("missing `c`"))]
    #[case::reordered(&["b", "a", "c"], &[0, 1, 2], Some("step 0"))]
    #[case::added(&["a", "x", "b", "c"], &[0, 1, 2, 3], Some("step 1"))]
    #[case::rewritten(&["a", "edited", "c"], &[0, 1, 2], Some("step 1"))]
    fn a_plan_must_be_what_its_spec_derives(
        #[case] steps: &[&str],
        #[case] remaining: &[usize],
        #[case] refusal: Option<&str>,
    ) {
        let expected: Vec<_> = ["a", "b", "c"].into_iter().map(step).collect();
        let mut file = bare_plan(PLAN_SCHEMA_VERSION);
        file.steps = steps.iter().copied().map(step).collect();

        let result = super::ensure_matches_spec(&file, &expected, remaining);
        match refusal {
            None => result.expect("this plan is exactly what the spec derives"),
            Some(fragment) => {
                let error = format!("{:#}", result.expect_err("not the derived plan"));
                assert!(error.contains(fragment), "{error}");
            }
        }
    }

    /// A truncated plan whose journal calls it complete: the case that used to
    /// report success for a market that was never created.
    #[test]
    fn a_plan_truncated_to_its_completed_prefix_is_refused() {
        let expected: Vec<_> = ["a", "b", "c"].into_iter().map(step).collect();
        let mut file = bare_plan(PLAN_SCHEMA_VERSION);
        file.steps = vec![step("a")];

        let error = format!(
            "{:#}",
            super::ensure_matches_spec(&file, &expected, &[]).expect_err("incomplete")
        );
        assert!(error.contains("missing `b`"), "{error}");
    }

    /// The one legitimate divergence: the gateway stops planning a storage
    /// registration once the account is registered, which is what this plan's
    /// own completed step did.
    #[test]
    fn a_completed_step_the_spec_no_longer_derives_is_accepted() {
        let expected = vec![step("a"), step("c")];
        let mut file = bare_plan(PLAN_SCHEMA_VERSION);
        file.steps = vec![step("a"), step("register b"), step("c")];

        super::ensure_matches_spec(&file, &expected, &[2])
            .expect("`register b` has already run, so it is no longer planned");
    }

    /// The same absence, but the step never ran — a plan that would skip work
    /// the spec calls for.
    #[test]
    fn an_outstanding_step_the_spec_does_not_derive_is_refused() {
        let expected = vec![step("a"), step("c")];
        let mut file = bare_plan(PLAN_SCHEMA_VERSION);
        file.steps = vec![step("a"), step("register b"), step("c")];

        let error = format!(
            "{:#}",
            super::ensure_matches_spec(&file, &expected, &[1, 2]).expect_err("never ran")
        );
        assert!(error.contains("register b"), "{error}");
    }

    /// Re-encoding a deploy's args as base64 is legal in this schema and is
    /// executed verbatim, so it must not hide the account being created.
    #[test]
    fn a_base64_encoded_deploy_still_yields_its_target() {
        let raw = serde_json::to_vec(&serde_json::json!({
            "name": "hidden",
            "version_key": "v1.3.0",
        }))
        .expect("encode");
        let steps = steps_with(PlanArgs::Base64(templar_gateway_types::Base64Bytes(raw)));

        assert_eq!(
            super::planned_targets(&steps)
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
        // Uppercase is not a valid NEAR account label.
        let steps = steps_with(PlanArgs::Json(serde_json::json!({
            "name": "Uppercase",
            "version_key": "v1.3.0",
        })));

        let error = super::planned_targets(&steps).expect_err("must not skip silently");
        assert!(
            format!("{error:#}").contains("cannot be checked for a collision"),
            "{error:#}"
        );
    }

    /// The applier must hold a key on every account the plan creates.
    ///
    /// A co-owner alongside is accepted: `--with-full-access-key` grants extra
    /// keys deliberately, and `render` shows them before the prompt.
    #[rstest]
    #[case(&["mine"], true)]
    #[case(&[], true)]
    #[case(&["mine", "theirs"], true)]
    #[case(&["theirs", "mine"], true)]
    #[case(&["theirs"], false)]
    fn every_granted_key_must_be_the_appliers(#[case] granted: &[&str], #[case] accepted: bool) {
        let mut file = bare_plan(PLAN_SCHEMA_VERSION);
        file.steps = steps_with(PlanArgs::Json(serde_json::json!({
            "name": "m",
            "version_key": "v1",
            "full_access_keys": granted,
        })));

        let result = super::ensure_granted_keys_are_yours(&file, "mine");
        assert_eq!(result.is_ok(), accepted, "{granted:?}: {result:?}");
        if !accepted {
            assert!(
                format!("{:#}", result.expect_err("refused")).contains("theirs"),
                "the refusal must name the keys that were granted"
            );
        }
    }

    /// A governance proposal creates no account, so it contributes no target.
    #[test]
    fn only_registry_deploys_are_targets() {
        let steps = steps_with(PlanArgs::Json(
            serde_json::json!({ "id": 0, "requested_ttl": "0" }),
        ));

        assert!(super::planned_targets(&steps)
            .expect("derivable")
            .is_empty());
    }

    #[test]
    fn a_matching_plan_is_accepted() {
        ensure_compatible(&bare_plan(PLAN_SCHEMA_VERSION), Network::Mainnet)
            .expect("an alpha spec is a mainnet spec");
    }

    /// Sending a mainnet deployment to testnet, or the reverse, is not something
    /// a confirmation prompt should be able to wave through.
    #[test]
    fn a_network_mismatch_is_refused() {
        let error = ensure_compatible(&bare_plan(PLAN_SCHEMA_VERSION), Network::Testnet)
            .expect_err("wrong network");

        assert!(
            error.to_string().contains("pointed at testnet"),
            "{error:#}"
        );
    }

    #[test]
    fn a_schema_mismatch_is_refused() {
        let error = ensure_compatible(&bare_plan(PLAN_SCHEMA_VERSION + 1), Network::Mainnet)
            .expect_err("wrong schema");

        assert!(error.to_string().contains("Regenerate it"), "{error:#}");
    }
}
