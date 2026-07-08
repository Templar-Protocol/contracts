use anyhow::Context as _;
use serde::Serialize;
use serde_json::Value;
use std::io::{Read as _, Write as _};
use templar_gateway_core::{DispatchRead, PlanWrite};
use templar_gateway_methods_dispatch::Dispatch;
use templar_gateway_types::{
    common::{WriteOperationResult, WriteRequest},
    MethodSpec,
};

use super::cli::{Command, GenericMethodCall};
use super::commands::{
    account::AccountNs, contract::ContractNs, ft::FtNs, market::MarketNs, op::OpNs,
    proxy_oracle::CreateProposal, proxy_oracle::ExecuteProposalArgs,
    proxy_oracle::ProxyOracleGovernanceNs, proxy_oracle::ProxyOracleNs,
    proxy_oracle::ProxyOracleOwnerNs, recover::RecoverNep141, redstone::RedstoneNs,
    registry::RegistryNs, storage::StorageNs,
};
use super::CliContext;

pub(super) async fn dispatch(ctx: CliContext, command: Command) -> anyhow::Result<()> {
    match command {
        Command::Account {
            command: AccountNs::Get(a),
        } => ctx.read(a.parse()).await,
        Command::Contract {
            command: ContractNs::GetVersion(a),
        } => ctx.read(a.parse()).await,
        Command::Registry { command } => dispatch_registry(ctx, command).await,
        Command::Storage { command } => dispatch_storage(ctx, command).await,
        Command::Ft { command } => dispatch_ft(ctx, command).await,
        Command::Market { command } => dispatch_market(ctx, command).await,
        Command::ProxyOracle { command } => dispatch_proxy_oracle(ctx, command).await,
        Command::ProxyOracleOwner { command } => dispatch_owner(ctx, command).await,
        Command::ProxyOracleGovernance { command } => dispatch_governance(ctx, command).await,
        Command::Redstone { command } => dispatch_redstone(ctx, command).await,
        Command::RecoverNep141(args) => recover_nep141(ctx, args).await,
        Command::Op {
            command: OpNs::Get(get),
        } => {
            if !ctx.has_operation_store {
                anyhow::bail!("op.get requires --gateway-store-url");
            }
            let request = get.parse();
            let operation = ctx.client.operation(&request.operation_id).await?;
            print_json(&templar_gateway_methods_spec::op::GetResult { operation })
        }
        Command::Read(call) => dispatch_generic_read(ctx, call).await,
        Command::Write(call) => dispatch_generic_write(ctx, call).await,
    }
}

async fn dispatch_registry(ctx: CliContext, ns: RegistryNs) -> anyhow::Result<()> {
    match ns {
        RegistryNs::ListVersions(a) => ctx.read(a.parse()).await,
        RegistryNs::ListDeployments(a) => ctx.read(a.parse()).await,
        RegistryNs::ListDeploymentsByKind(a) => ctx.read(a.parse()).await,
        RegistryNs::GetDeployment(a) => ctx.read(a.parse()).await,
        RegistryNs::AddVersion(a) => ctx.write(a.into_spec()?).await,
        RegistryNs::Deploy(a) => ctx.write(a.parse()?).await,
    }
}

async fn dispatch_storage(ctx: CliContext, ns: StorageNs) -> anyhow::Result<()> {
    match ns {
        StorageNs::GetBalanceBounds(a) => ctx.read(a.parse()).await,
        StorageNs::GetBalanceOf(a) => ctx.read(a.parse()).await,
        StorageNs::Deposit(a) => ctx.write(a.parse()).await,
        StorageNs::Unregister(a) => ctx.write(a.parse()).await,
        StorageNs::EnsureDeposit(a) => ctx.write(a.parse()?).await,
    }
}

async fn dispatch_ft(ctx: CliContext, ns: FtNs) -> anyhow::Result<()> {
    match ns {
        FtNs::GetBalanceOf(a) => ctx.read(a.parse()).await,
        FtNs::Transfer(a) => ctx.write(a.parse()).await,
        FtNs::TransferCall(a) => ctx.write(a.parse()).await,
    }
}

async fn dispatch_market(ctx: CliContext, ns: MarketNs) -> anyhow::Result<()> {
    match ns {
        MarketNs::Create(a) => ctx.write(a.parse()?).await,
    }
}

async fn dispatch_owner(ctx: CliContext, ns: ProxyOracleOwnerNs) -> anyhow::Result<()> {
    match ns {
        ProxyOracleOwnerNs::GetOwner(a) => ctx.read(a.get_owner()).await,
        ProxyOracleOwnerNs::GetProposedOwner(a) => ctx.read(a.get_proposed_owner()).await,
        ProxyOracleOwnerNs::ProposeOwner(a) => ctx.write(a.parse()).await,
        ProxyOracleOwnerNs::AcceptOwner(a) => ctx.write(a.accept_owner()).await,
        ProxyOracleOwnerNs::RenounceOwner(a) => ctx.write(a.renounce_owner()).await,
    }
}

async fn dispatch_governance(ctx: CliContext, ns: ProxyOracleGovernanceNs) -> anyhow::Result<()> {
    match ns {
        ProxyOracleGovernanceNs::Create(a) => ctx.write(a.parse()?).await,
        ProxyOracleGovernanceNs::CreateProposal(a) => create_proposal(ctx, a).await,
        ProxyOracleGovernanceNs::CancelProposal(a) => ctx.write(a.cancel()).await,
        ProxyOracleGovernanceNs::ExecuteProposal(a) => execute_proposal(ctx, a).await,
        ProxyOracleGovernanceNs::GetProposal(a) => ctx.read(a.get()).await,
        ProxyOracleGovernanceNs::ListProposals(a) => ctx.read(a.parse()).await,
        ProxyOracleGovernanceNs::NextProposalId(a) => ctx.read(a.next_proposal_id()).await,
        ProxyOracleGovernanceNs::ProposalCount(a) => ctx.read(a.proposal_count()).await,
        ProxyOracleGovernanceNs::GetOperationTtl(a) => ctx.read(a.parse()).await,
        ProxyOracleGovernanceNs::GetProxyOracleId(a) => ctx.read(a.get_proxy_oracle_id()).await,
        ProxyOracleGovernanceNs::HasRole(a) => ctx.read(a.parse()).await,
        ProxyOracleGovernanceNs::ListRole(a) => ctx.read(a.parse()).await,
        ProxyOracleGovernanceNs::GetRoles(a) => ctx.read(a.parse()).await,
    }
}

/// Create a governance proposal. Resolves the proposal id (fetching the
/// governance contract's next id when `--id` was omitted) and always emits it,
/// so callers — including scripts — can learn the id that was used. With
/// `--execute-when-ready`, waits for the proposal's TTL to elapse and executes.
async fn create_proposal(ctx: CliContext, args: CreateProposal) -> anyhow::Result<()> {
    use templar_gateway_methods_spec::proxy_oracle_governance as gov;

    let governance_id = args.governance_id().clone();
    let execute_when_ready = args.execute_when_ready();

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

    // Keep the operator's idempotency key on the create write only.
    let create = ctx
        .client
        .execute_request(WriteRequest {
            signer_account_id: ctx.signer_account()?,
            idempotency_key: ctx.idempotency_key.clone(),
            body: args.into_spec(id)?,
        })
        .await?;

    let execute = if execute_when_ready {
        wait_for_maturity(&ctx, &governance_id, id).await?;
        let signer = ctx.signer_account()?;
        Some(
            ctx.client
                .execute_as(
                    signer,
                    gov::ExecuteProposal {
                        governance_id: governance_id.clone(),
                        id,
                    },
                )
                .await?,
        )
    } else {
        None
    };

    print_json(&CreateProposalOutput {
        id,
        create,
        execute,
    })
}

/// Execute a governance proposal. With `--when-ready`, wait for its TTL to
/// elapse first, so an early call blocks instead of failing on an immature
/// proposal.
async fn execute_proposal(ctx: CliContext, args: ExecuteProposalArgs) -> anyhow::Result<()> {
    if args.when_ready() {
        wait_for_maturity(&ctx, args.governance_id(), args.id()).await?;
    }
    ctx.write(args.into_spec()).await
}

/// Machine-readable result of a `create-proposal` run. `id` is always present
/// (resolved even when auto-fetched); `execute` is present only when the
/// proposal was executed via `--execute-when-ready`.
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
    governance_id: &near_account_id::AccountId,
    id: u32,
) -> anyhow::Result<()> {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use templar_gateway_methods_spec::proxy_oracle_governance as gov;

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

async fn dispatch_proxy_oracle(ctx: CliContext, ns: ProxyOracleNs) -> anyhow::Result<()> {
    match ns {
        ProxyOracleNs::GetProxy(a) => ctx.read(a.parse()?).await,
        ProxyOracleNs::ListProxies(a) => ctx.read(a.parse()).await,
        ProxyOracleNs::PriceFeedExists(a) => ctx.read(a.parse()?).await,
        ProxyOracleNs::UpdatePrices(a) => ctx.write(a.parse()?).await,
    }
}

async fn dispatch_redstone(ctx: CliContext, ns: RedstoneNs) -> anyhow::Result<()> {
    match ns {
        RedstoneNs::GetConfig(a) => ctx.read(a.parse()).await,
        RedstoneNs::ReadPriceData(a) => ctx.read(a.parse()).await,
        RedstoneNs::ListRole(a) => ctx.read(a.parse()).await,
        RedstoneNs::SetRole(a) => ctx.write(a.parse()).await,
        RedstoneNs::WritePrices(a) => ctx.write(a.parse()?).await,
    }
}

/// Recover a NEP-141 balance from the signer to a beneficiary, then unregister
/// the signer's storage — the multi-step orchestration the old `recover_nep141`
/// command performed, now expressed over the gateway's standard-agnostic
/// `token.*` operations. Each write is executed on its own (no shared
/// idempotency key), so a re-run re-reads the chain rather than replaying.
async fn recover_nep141(ctx: CliContext, args: RecoverNep141) -> anyhow::Result<()> {
    use templar_gateway_methods_spec::{storage, token};

    let signer = ctx.signer_account()?;
    let account_id = signer.0.clone();
    let token = token::TokenReference::Ft {
        contract_id: args.token_id.clone(),
    };

    let balance = ctx
        .client
        .read(token::GetBalanceOf {
            token: token.clone(),
            account_id: account_id.clone(),
        })
        .await?
        .balance
        .0;

    if balance > 0 {
        ctx.client
            .execute_as(
                signer.clone(),
                token::Transfer {
                    token: token.clone(),
                    receiver_id: args.beneficiary_id.clone(),
                    amount: balance.into(),
                    memo: None,
                },
            )
            .await?;
    }

    // Re-read before unregistering: a failed/partial transfer must not lead to
    // unregistering storage while tokens remain (which would strand them).
    let remaining = ctx
        .client
        .read(token::GetBalanceOf { token, account_id })
        .await?
        .balance
        .0;
    if remaining != 0 {
        anyhow::bail!(
            "non-zero balance ({remaining}) remains after transferring to {}; \
             refusing to unregister storage",
            args.beneficiary_id
        );
    }

    let result = ctx
        .client
        .execute_as(
            signer,
            storage::Unregister {
                contract_id: args.token_id,
                force: args.force,
            },
        )
        .await?;
    print_json(&result)
}

async fn dispatch_generic_read(ctx: CliContext, call: GenericMethodCall) -> anyhow::Result<()> {
    let method = call.method.clone();
    let params = load_generic_params(call)?;

    macro_rules! try_read {
        ($spec:ty) => {
            if method == <$spec as MethodSpec>::RPC_METHOD {
                let request: $spec = serde_json::from_value(params)
                    .with_context(|| format!("parse parameters for {method}"))?;
                return ctx.read(request).await;
            }
        };
    }
    templar_gateway_methods_spec::for_each_read_method!(try_read);
    anyhow::bail!("unsupported read method {method}");
}

async fn dispatch_generic_write(ctx: CliContext, call: GenericMethodCall) -> anyhow::Result<()> {
    let method = call.method.clone();
    let params = load_generic_params(call)?;

    macro_rules! try_write {
        ($spec:ty) => {
            if method == <$spec as MethodSpec>::RPC_METHOD {
                let body: $spec = serde_json::from_value(params)
                    .with_context(|| format!("parse parameters for {method}"))?;
                return ctx.write(body).await;
            }
        };
    }
    templar_gateway_methods_spec::for_each_write_method!(try_write);
    anyhow::bail!("unsupported write method {method}");
}

fn load_generic_params(call: GenericMethodCall) -> anyhow::Result<Value> {
    if let Some(json) = call.json {
        return serde_json::from_str(&json).context("parse --json method parameters");
    }
    if let Some(path) = call.json_file {
        if path == std::path::Path::new("-") {
            let mut input = String::new();
            std::io::stdin()
                .read_to_string(&mut input)
                .context("read JSON parameters from stdin")?;
            return serde_json::from_str(&input).context("parse JSON method parameters");
        }
        let input = std::fs::read_to_string(&path)
            .with_context(|| format!("read JSON parameters from {}", path.display()))?;
        return serde_json::from_str(&input).context("parse JSON method parameters");
    }
    anyhow::bail!("missing method parameters (use --json or --json-file)")
}

fn print_json(output: &impl Serialize) -> anyhow::Result<()> {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    serde_json::to_writer(&mut lock, output)?;
    writeln!(lock)?;
    Ok(())
}

impl CliContext {
    async fn read<S>(&self, request: S) -> anyhow::Result<()>
    where
        S: MethodSpec,
        Dispatch: DispatchRead<S, templar_gateway_core::GatewayContext>,
    {
        let output = self.client.read(request).await?;
        print_json(&output)
    }

    async fn write<S>(&self, body: S) -> anyhow::Result<()>
    where
        S: MethodSpec<Output = WriteOperationResult>,
        Dispatch: PlanWrite<S, templar_gateway_core::GatewayContext>,
    {
        let output = self
            .client
            .execute_request(WriteRequest {
                signer_account_id: self.signer_account()?,
                idempotency_key: self.idempotency_key.clone(),
                body,
            })
            .await?;
        print_json(&output)
    }
}
