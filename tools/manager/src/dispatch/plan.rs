//! `market plan` / `market apply` — generate a deployment as a file, then send
//! that file.
//!
//! The two commands exist because a spec cannot express every market and a check
//! can be wrong. Splitting generation from execution puts a reviewable artifact
//! between the two, and makes the concrete transactions editable when the
//! declarative path falls short.
//!
//! What `plan` produces is exactly what `deploy.sh` runs today, in the same
//! order — see [`build`].

use std::io::Write as _;

use anyhow::Context as _;
use near_account_id::AccountId;
use near_api::types::NearToken;
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
/// `script/deploy.sh`.
///
/// Constants rather than spec fields: these size a *contract's* storage staking,
/// which follows from the code being deployed, not from the market's identity.
/// A deployment that genuinely needs more can raise one by editing the plan,
/// which is what the artifact is for.
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

    let failed = checks
        .iter()
        .filter(|check| check.status.is_failure())
        .count();
    if failed > 0 {
        print_json(&checks)?;
        anyhow::bail!(
            "{failed} check(s) failed; no plan written. Fix the spec, or re-run \
             with `--skip-check <id>` for a check that is wrong."
        );
    }

    let steps = build(&ctx.client, &spec, &args.public_key(), &args.signer_id).await?;
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

    // Provenance is reported, not enforced.
    let drift = file.drift()?;
    eprintln!("\n{}", drift.describe());
    if !drift.is_clean() {
        eprintln!(
            "An edited plan bypasses every spec-level check above — its arguments \
             are already encoded. Read the steps before confirming."
        );
    }

    if !args.yes {
        confirm(&format!(
            "Send {} transaction(s) as {}?",
            file.steps.len(),
            args.signer.account_id().0,
        ))?;
    }

    let plan = file.into_operation_plan()?;
    let (signer, client) = ctx.signing_client_for(&args.signer).await?;
    let output = client
        .execute_request(WriteRequest {
            signer_account_id: signer,
            idempotency_key: None,
            body: PreparedPlan { steps: plan.steps },
        })
        .await?;
    ctx.finish_write(&output)
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

    let governance_id = spec.governance_id()?;
    let price_maximum_age = spec.market.price_maximum_age;
    let full_access_keys = Some(vec![public_key.clone()]);

    // A plan always creates its own governance contract, so its proposal
    // counter always starts at zero. Reading `nextProposalId` would be reading
    // an account this very plan is about to create — and if one already exists,
    // step 1 fails loudly rather than appending proposals to someone else's
    // governance. Resuming onto existing contracts is ENG-546.
    let (collateral_proposal, borrow_proposal) = (0, 1);

    let (collateral_decimals, borrow_decimals) = decimals(spec)?;
    let configuration = spec
        .clone()
        .into_market_configuration(collateral_decimals, borrow_decimals)?;

    let mut steps = Vec::from(oracle_stack(client, signer_id, spec, public_key).await?);

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
        steps.extend(
            set_proxy(
                client,
                signer_id,
                &governance_id,
                spec.governance.ttl_default,
                ProxyProposal {
                    side,
                    price_id,
                    proposal_id,
                    proxy,
                },
            )
            .await?,
        );
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

/// The governance contract and the oracle it owns.
///
/// Emitted as a pair, in this order: the oracle names its governance as
/// `owner_id` at init, so governance must exist first. Reversing them would
/// deploy an oracle owned by an account that does not exist yet.
async fn oracle_stack(
    client: &Client,
    signer_id: &AccountId,
    spec: &MarketSpec,
    public_key: &PublicKey,
) -> anyhow::Result<[(String, PlannedTransaction); 2]> {
    let full_access_keys = Some(vec![public_key.clone()]);
    let governance_id = spec.governance_id()?;
    let oracle_id = spec.oracle_id()?;

    let governance_init = serde_json::to_vec(&GovernanceInit {
        proxy_oracle_id: oracle_id.clone(),
        admin_id: spec.governance.admin.clone(),
        ttls: uniform_ttls(spec.governance.ttl_default),
    })
    .context("encode governance init args")?;

    Ok([
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

/// One feed's proxy configuration, as a governance proposal.
struct ProxyProposal {
    side: &'static str,
    price_id: templar_common::oracle::pyth::PriceIdentifier,
    proposal_id: u32,
    proxy:
        templar_proxy_oracle_kernel::proxy::Proxy<templar_proxy_oracle_near_common::input::Source>,
}

/// Propose a feed's proxy, then execute that proposal.
///
/// Two transactions rather than one: a proposal is always created before it can
/// run, even at `ttl_default = 0`. They are emitted as a pair so a plan can
/// never carry a create without its execute, which would leave the oracle
/// configured for one leg only.
async fn set_proxy(
    client: &Client,
    signer_id: &AccountId,
    governance_id: &AccountId,
    requested_ttl: Nanoseconds,
    proposal: ProxyProposal,
) -> anyhow::Result<[(String, PlannedTransaction); 2]> {
    let ProxyProposal {
        side,
        price_id,
        proposal_id,
        proxy,
    } = proposal;

    Ok([
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
                    requested_ttl,
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
                    governance_id: governance_id.clone(),
                    id: proposal_id,
                },
            )
            .await?,
        ),
    ])
}

/// Decimals resolved by the preflight. Absent means a check failed, and the
/// caller refuses before reaching here — but building a configuration from
/// guessed decimals would mis-scale every price, so this never assumes.
fn decimals(spec: &MarketSpec) -> anyhow::Result<(i32, i32)> {
    let (Some(collateral), Some(borrow)) = (spec.collateral.decimals, spec.borrow.decimals) else {
        anyhow::bail!(
            "asset decimals are unresolved, so a market configuration cannot be \
             built. Set `decimals` in the spec, or fix `asset.decimals.*`."
        );
    };
    Ok((i32::from(collateral), i32::from(borrow)))
}

/// Plan one write, requiring it to be a single transaction.
///
/// Each of these specs plans to exactly one; a multi-transaction step would make
/// the labels lie about what an operator is confirming.
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

    let [transaction] = <[PlannedTransaction; 1]>::try_from(plan.steps).map_err(|steps| {
        anyhow::anyhow!(
            "`{}` planned {} transactions; the plan builder assumes one per step",
            S::RPC_METHOD,
            steps.len()
        )
    })?;
    Ok(transaction)
}

/// Mark the named checks skipped, preserving the verdict each one reached.
///
/// The original detail is kept in the reason: a reviewer of the plan needs to
/// see *what* was suppressed, and a bare "skipped" would hide the failure the
/// operator chose to override.
///
/// An id that matches nothing is an error. A typo would otherwise read as a
/// successful suppression while the check kept running.
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

/// The only two hard refusals.
///
/// A schema mismatch means this build cannot read the file faithfully, and a
/// network mismatch would send a mainnet deployment to testnet or the reverse.
/// Everything else — including a plan edited beyond recognition — is reported
/// and confirmed rather than blocked, because blocking it would defeat the
/// artifact's purpose.
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
        }
    }
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
/// as every other write — the store, idempotency, and recovery behaviour all
/// come along rather than being reimplemented here.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PreparedPlan {
    steps: Vec<PlannedTransaction>,
}

impl schemars::JsonSchema for PreparedPlan {
    fn schema_name() -> String {
        "PreparedPlan".to_owned()
    }

    /// Never served over the RPC surface — this spec exists only to reach the
    /// executor from inside this binary — so an unconstrained schema is honest
    /// rather than lazy.
    fn json_schema(_generator: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        schemars::schema::Schema::Bool(true)
    }
}

impl MethodSpec for PreparedPlan {
    type Output = WriteOperationResult;
    const RPC_METHOD: &'static str = "market.apply";
}

#[async_trait::async_trait]
impl PlanWrite<PreparedPlan, GatewayContext> for Dispatch {
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

    fn plan_file(schema: u32, network: &str) -> PlanFile {
        PlanFile {
            schema,
            tool_version: "0.1.0".to_owned(),
            network: network.to_owned(),
            spec_digest: "sha256:test".to_owned(),
            step_digests: Vec::new(),
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
        ensure_compatible(&plan_file(PLAN_SCHEMA_VERSION, "mainnet"), "mainnet")
            .expect("same schema and network");
    }

    /// Sending a mainnet deployment to testnet, or the reverse, is not something
    /// a confirmation prompt should be able to wave through.
    #[test]
    fn a_network_mismatch_is_refused() {
        let error = ensure_compatible(&plan_file(PLAN_SCHEMA_VERSION, "mainnet"), "testnet")
            .expect_err("wrong network");

        assert!(
            error.to_string().contains("pointed at testnet"),
            "{error:#}"
        );
    }

    #[test]
    fn a_schema_mismatch_is_refused() {
        let error = ensure_compatible(&plan_file(PLAN_SCHEMA_VERSION + 1, "mainnet"), "mainnet")
            .expect_err("wrong schema");

        assert!(error.to_string().contains("Regenerate it"), "{error:#}");
    }
}
