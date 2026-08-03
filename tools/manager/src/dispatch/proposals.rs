//! Governance proposal planning and orchestration: resolving proposal inputs,
//! rendering plans, creating proposals, and optionally waiting for maturity
//! before execution.

use anyhow::Context as _;
use near_account_id::AccountId;
use serde::Serialize;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_client::Client;
use templar_gateway_methods_spec::proxy_oracle_governance as gov;
use templar_gateway_types::{common::WriteOperationResult, ManagedAccountId};

use crate::commands::proxy_oracle::{CreateProposal, ExecuteProposalArgs};
use crate::context::{print_json, CliContext};

/// Plan or create a governance proposal. Resolves the proposal id (fetching the
/// governance contract's next id when `--id` was omitted) and, for an
/// `oracle add-circuit-breaker` proposal without `--breaker-id`, the set's next breaker
/// id. Print mode emits the selected plan representation without sending.
/// Execution mode logs the resolved id, and `--execute-when-ready` waits for the
/// proposal's TTL to elapse before executing it.
pub(super) async fn create(ctx: CliContext, mut args: CreateProposal) -> anyhow::Result<()> {
    let execute_when_ready = args.execute_when_ready();
    let signer_args = args.signer.clone();
    let governance_id = args.target.resolve(&ctx).await?;

    // Auto-fill the next breaker id for oracle add-circuit-breaker, resolving the proxy
    // oracle (whose set holds the breakers) through the governance contract. This
    // reads the committed set's next id; if a concurrent proposal advances it
    // before this one executes, the contract rejects the stale id (no corruption)
    // and the proposal can simply be retried.
    if let Some(price_id) = args.unresolved_breaker_price_id() {
        let next_id = next_breaker_id(&ctx, &governance_id, price_id).await?;
        tracing::info!(breaker_id = next_id, "auto-fetched next breaker id");
        args.set_breaker_id(next_id);
    }

    let id = match args.id() {
        Some(id) => id,
        None => {
            ctx.client
                .read(gov::NextProposalId {
                    governance_id: governance_id.clone(),
                })
                .await?
        }
    };

    let create_spec = args.try_into_spec(governance_id.clone(), id)?;
    if signer_args.print().is_some() {
        return ctx.write(signer_args, create_spec).await;
    }

    let (signer, secret_key) = signer_args.resolve()?;
    let client = ctx.signing_client(signer.clone(), secret_key)?;
    let create = client.execute_as(signer.clone(), create_spec).await?;
    // Fail fast if the create reverted, before waiting on / executing a proposal
    // that was never created.
    ctx.report_checked(&create)?;
    // Emit the id now so it survives a later wait/execute failure below.
    tracing::info!(proposal_id = id, "created proposal");

    let execute = if execute_when_ready {
        wait_for_maturity(&ctx, &governance_id, id).await?;
        let result = execute_now(&ctx, &client, &signer, &governance_id, id).await?;
        Some(result)
    } else {
        None
    };

    print_json(&CreateProposalOutput {
        id,
        create,
        execute,
    })
}

/// Plan or execute a governance proposal. In execution mode, `--when-ready`
/// waits for its TTL to elapse, so an early call blocks instead of failing on
/// an immature proposal.
pub(super) async fn execute(ctx: CliContext, args: ExecuteProposalArgs) -> anyhow::Result<()> {
    let governance_id = args.target.resolve(&ctx).await?;
    if args.when_ready() {
        wait_for_maturity(&ctx, &governance_id, args.id()).await?;
    }
    ctx.write(args.signer.clone(), args.into_spec(governance_id))
        .await
}

/// Execute proposal `id` on its own (no idempotency key), signed as `signer`
/// through `client`, reporting the tx link.
async fn execute_now(
    ctx: &CliContext,
    client: &Client,
    signer: &ManagedAccountId,
    governance_id: &AccountId,
    id: u32,
) -> anyhow::Result<WriteOperationResult> {
    let result = client
        .execute_as(
            signer.clone(),
            gov::ExecuteProposal {
                governance_id: governance_id.clone(),
                id,
            },
        )
        .await?;
    ctx.report_checked(&result)?;
    Ok(result)
}

/// The next breaker id for `price_id` on the proxy oracle administered by
/// `governance_id`: resolve the oracle, read its circuit breaker set, and take
/// the set's next id (0 when no set exists yet).
async fn next_breaker_id(
    ctx: &CliContext,
    governance_id: &AccountId,
    price_id: PriceIdentifier,
) -> anyhow::Result<u32> {
    use templar_gateway_methods_spec::proxy_oracle;

    let oracle_id = ctx
        .client
        .read(gov::GetProxyOracleId {
            governance_id: governance_id.clone(),
        })
        .await?
        .proxy_oracle_id;
    let set = ctx
        .client
        .read(proxy_oracle::GetProxyCircuitBreakerSet {
            oracle_id,
            id: price_id,
        })
        .await?
        .circuit_breaker_set;
    Ok(set.map_or(0, |set| set.next_id()))
}

/// Machine-readable result of an execution-mode `create-proposal` run. `id` is
/// always present (resolved even when auto-fetched); `execute` is present only
/// when the proposal was executed via `--execute-when-ready`.
#[derive(Serialize)]
struct CreateProposalOutput {
    id: u32,
    create: WriteOperationResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    execute: Option<WriteOperationResult>,
}

/// Block until proposal `id` is executable (`now - created_at >= ttl`), reading
/// its effective TTL back from the governance contract.
async fn wait_for_maturity(
    ctx: &CliContext,
    governance_id: &AccountId,
    id: u32,
) -> anyhow::Result<()> {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    let proposal = ctx
        .client
        .read(gov::GetProposal {
            governance_id: governance_id.clone(),
            id,
        })
        .await?
        .proposal
        .context("created proposal not found when waiting for maturity")?;

    let maturity_ns = proposal
        .created_at
        .as_ns()
        .saturating_add(proposal.ttl.as_ns());
    let now_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0);

    if maturity_ns > now_ns {
        // Small buffer so block time (the chain's authoritative clock) has caught
        // up to the local wall clock before we submit the execute.
        let wait = Duration::from_nanos(maturity_ns - now_ns) + Duration::from_secs(2);
        eprintln!(
            "Waiting {}s for proposal {id} to mature before executing...",
            wait.as_secs()
        );
        tokio::time::sleep(wait).await;
    }

    Ok(())
}
