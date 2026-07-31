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
    render(&file);
    report_checks(&file);

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

    if !args.yes {
        confirm(&format!(
            "Send {} transaction(s) as {credential}?",
            file.steps.len(),
        ))?;
    }

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

    steps.push((
        format!("deploy market {}", spec.market_id()?),
        step(
            client,
            signer_id,
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
    ));

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

    let governance_init = serde_json::to_vec(&GovernanceInit {
        proxy_oracle_id: oracle_id.clone(),
        admin_id: spec.governance.admin.clone(),
        ttls: uniform_ttls(spec.governance.ttl_default),
    })
    .context("encode governance init args")?;

    Ok(vec![
        (
            format!("deploy governance {governance_id}"),
            step(
                client,
                signer_id,
                registry::Deploy {
                    registry_id: spec.registry.clone(),
                    name: crate::spec::governance_name(&spec.name),
                    version_key: spec.versions.proxy_governance.clone(),
                    init_args: Base64Bytes(governance_init),
                    full_access_keys: full_access_keys.clone(),
                    deposit: GOVERNANCE_DEPOSIT,
                },
            )
            .await?,
        ),
        (
            format!("deploy proxy oracle {oracle_id}, owned by governance"),
            step(
                client,
                signer_id,
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
        ),
    ])
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

    Ok(vec![
        (
            format!("propose {side} proxy (proposal {proposal_id})"),
            step(
                client,
                signer_id,
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
            .await?,
        ),
        (
            format!("execute {side} proxy proposal {proposal_id}"),
            step(
                client,
                signer_id,
                gov::ExecuteProposal {
                    governance_id,
                    id: proposal_id,
                },
            )
            .await?,
        ),
    ])
}

/// Plan one write, requiring it to be a single transaction — a multi-transaction
/// step would make the labels lie about what an operator is confirming.
async fn step<S>(
    client: &Client,
    signer_id: &AccountId,
    body: S,
) -> anyhow::Result<PlannedTransaction>
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
        .await?;

    crate::context::single_transaction(plan)
        .with_context(|| format!("planning `{}`", S::RPC_METHOD))
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

/// A registry deploy's `init_args`, decoded.
///
/// `registry.deploy` takes them as base64 *inside* its JSON args, so the market
/// configuration — the MCRs, the rate curve, the oracle account — is the one
/// part of a deployment the artifact cannot show as JSON. Telling an operator to
/// read the steps while hiding the payload that matters most would be hollow, so
/// it is decoded for display. It stays base64 in the file: expanding it there
/// would have to survive a byte-exact round trip, which is a schema change
/// rather than a rendering one.
fn decoded_init_args(call: &crate::spec::plan::PlanFunctionCall) -> Option<String> {
    use base64::Engine as _;

    let crate::spec::plan::PlanArgs::Json(args) = &call.args else {
        return None;
    };
    let encoded = args.get("init_args")?.as_str()?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    serde_json::to_string(&value).ok()
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
