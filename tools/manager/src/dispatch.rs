use anyhow::Context as _;
use near_account_id::AccountId;
use serde::Serialize;
use serde_json::{json, Value};
use std::io::{Read as _, Write as _};
use templar_gateway_client::Client;
use templar_gateway_core::{DispatchRead, PlanWrite};
use templar_gateway_methods_dispatch::Dispatch;
use templar_gateway_types::{
    common::{WriteOperationResult, WriteRequest},
    primitive::PublicKey,
    ManagedAccountId, MethodSpec,
};

use super::cli::{Command, GenericMethodCall};
use super::commands::{
    account::AccountNs, contract::ContractNs, ft::FtNs, market::MarketNs, op::OpNs,
    proxy_oracle::CreateProposal, proxy_oracle::ExecuteProposalArgs,
    proxy_oracle::ProxyOracleGovernanceNs, proxy_oracle::ProxyOracleNs,
    proxy_oracle::ProxyOracleOwnerNs, recover::RecoverNep141, redstone as redstone_cmd,
    redstone::RedstoneNs, registry, registry::RegistryNs, storage::StorageNs,
};
use super::CliContext;

pub(super) async fn dispatch(ctx: CliContext, command: Command) -> anyhow::Result<()> {
    match command {
        Command::Account { command } => dispatch_account(ctx, command).await,
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

async fn dispatch_account(ctx: CliContext, ns: AccountNs) -> anyhow::Result<()> {
    match ns {
        AccountNs::Get(a) => ctx.read(a.parse()).await,
        AccountNs::Delete(a) => ctx.write(a.parse()).await,
    }
}

async fn dispatch_registry(ctx: CliContext, ns: RegistryNs) -> anyhow::Result<()> {
    match ns {
        RegistryNs::ListVersions(a) => ctx.read(a.parse()).await,
        RegistryNs::ListDeployments(a) => ctx.read(a.parse()).await,
        RegistryNs::ListDeploymentsByKind(a) => ctx.read(a.parse()).await,
        RegistryNs::GetDeployment(a) => ctx.read(a.parse()).await,
        RegistryNs::AddVersion(a) => ctx.write(a.into_spec()?).await,
        RegistryNs::Deploy(a) => {
            let (no_signer, extra) = a.full_access_key_flags();
            let full_access_keys = ctx.resolve_full_access_keys(no_signer, &extra)?;
            ctx.write(a.into_spec(full_access_keys)?).await
        }
        RegistryNs::RemoveVersion(a) => remove_version(ctx, a).await,
        RegistryNs::Remove(a) => registry_remove(ctx, a).await,
        RegistryNs::ClearDeployments(a) => clear_deployments(ctx, a).await,
    }
}

/// Remove a single registry version, or every version with `--all`.
async fn remove_version(ctx: CliContext, args: registry::RemoveVersion) -> anyhow::Result<()> {
    use templar_gateway_methods_spec::registry as spec;

    // clap's arg group guarantees exactly one of --version-key / --all, so a
    // present single spec is the single-version case; its absence means --all.
    if let Some(spec) = args.single() {
        return ctx.write(spec).await;
    }

    let signer = ctx.signer_account()?;
    let versions = ctx
        .client
        .read(spec::ListVersions {
            registry_id: args.registry_id().clone(),
            args: templar_gateway_types::common::Pagination {
                offset: None,
                limit: None,
            },
        })
        .await?
        .values;

    let mut removed = Vec::new();
    for version_key in versions {
        let result = ctx
            .client
            .execute_as(signer.clone(), args.spec_for(version_key.clone()))
            .await?;
        ctx.report_tx(&result);
        removed.push(version_key);
    }
    print_json(&json!({ "removed": removed }))
}

/// Remove every version from the registry, then delete the (signer) registry
/// account, sweeping its balance to the beneficiary.
async fn registry_remove(ctx: CliContext, args: registry::Remove) -> anyhow::Result<()> {
    use templar_gateway_methods_spec::{account, registry as spec};

    let signer = ctx.signer_account()?;
    let registry_id = signer.0.clone();

    let versions = ctx
        .client
        .read(spec::ListVersions {
            registry_id: registry_id.clone(),
            args: templar_gateway_types::common::Pagination {
                offset: None,
                limit: None,
            },
        })
        .await?
        .values;
    for version_key in versions {
        let result = ctx
            .client
            .execute_as(
                signer.clone(),
                spec::RemoveVersion {
                    registry_id: registry_id.clone(),
                    version_key,
                },
            )
            .await?;
        ctx.report_tx(&result);
    }

    let result = ctx
        .client
        .execute_as(
            signer,
            account::Delete {
                beneficiary_id: args.beneficiary_id().clone(),
            },
        )
        .await?;
    ctx.report_tx(&result);
    print_json(&result)
}

/// Remove every market deployed from the registry, signing each removal as the
/// market account with the shared `--secret-key`.
async fn clear_deployments(
    ctx: CliContext,
    args: registry::ClearDeployments,
) -> anyhow::Result<()> {
    use templar_gateway_methods_spec::registry as spec;

    let beneficiary = args.beneficiary_id();
    let force = args.force();
    let accounts = ctx
        .client
        .read(spec::ListDeployments {
            registry_id: args.registry_id().clone(),
            args: templar_gateway_types::common::Pagination {
                offset: None,
                limit: None,
            },
        })
        .await?
        .account_ids;

    let mut removed = Vec::new();
    for account in accounts {
        let client = ctx.signing_client_for(account.clone())?;
        match remove_market_account(&ctx, &client, account.clone().into(), &beneficiary, force)
            .await
        {
            Ok(()) => removed.push(account),
            Err(error) if force => {
                tracing::warn!(%account, %error, "failed to remove market; continuing (--force)");
            }
            Err(error) => return Err(error.context(format!("remove market {account}"))),
        }
    }
    print_json(&json!({ "removed": removed }))
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
        MarketNs::Create(a) => {
            let mut spec = a.parse()?;
            spec.full_access_keys = Some(ctx.default_full_access_keys()?);
            ctx.write(spec).await
        }
        MarketNs::Remove(a) => {
            let signer = ctx.signer_account()?;
            remove_market_account(&ctx, &ctx.client, signer, a.beneficiary_id(), a.force()).await?;
            print_json(&json!({ "removed": true }))
        }
    }
}

/// Recover a market's assets to the beneficiary, then delete the market account.
/// Reads and writes go through `client`, whose signer must be the market account
/// (its own removal is self-signed).
async fn remove_market_account(
    ctx: &CliContext,
    client: &Client,
    market: ManagedAccountId,
    beneficiary: &AccountId,
    force: bool,
) -> anyhow::Result<()> {
    use templar_gateway_methods_spec::{account, market, token};

    match client
        .read(market::GetConfiguration {
            market_id: market.0.clone(),
        })
        .await
    {
        Ok(configuration) => {
            let assets = [
                token::TokenReference::from(&configuration.borrow_asset),
                token::TokenReference::from(&configuration.collateral_asset),
            ];
            for asset in assets {
                if let Err(error) = recover_token(ctx, client, &market, asset, beneficiary).await {
                    if !force {
                        return Err(error);
                    }
                    tracing::warn!(%error, "failed to recover asset; continuing (--force)");
                }
            }
        }
        Err(error) => {
            if !force {
                return Err(anyhow::Error::from(error).context("read market configuration"));
            }
            tracing::warn!(%error, "failed to read market configuration; continuing (--force)");
        }
    }

    let result = client
        .execute_as(
            market,
            account::Delete {
                beneficiary_id: beneficiary.clone(),
            },
        )
        .await?;
    ctx.report_tx(&result);
    Ok(())
}

/// Transfer a token's full balance from `from` to `beneficiary` if non-zero,
/// using the standard-agnostic `token.transfer` so NEP-245 assets work too, then
/// best-effort reclaim `from`'s storage slot on the token contract.
async fn recover_token(
    ctx: &CliContext,
    client: &Client,
    from: &ManagedAccountId,
    token: templar_gateway_methods_spec::token::TokenReference,
    beneficiary: &AccountId,
) -> anyhow::Result<()> {
    use templar_gateway_methods_spec::{storage, token};

    let contract_id = match &token {
        token::TokenReference::Ft { contract_id }
        | token::TokenReference::Mt { contract_id, .. } => contract_id.clone(),
    };

    let balance = client
        .read(token::GetBalanceOf {
            token: token.clone(),
            account_id: from.0.clone(),
        })
        .await?
        .balance
        .0;
    if balance > 0 {
        let result = client
            .execute_as(
                from.clone(),
                token::Transfer {
                    token,
                    receiver_id: beneficiary.clone(),
                    amount: balance.into(),
                    memo: None,
                },
            )
            .await?;
        ctx.report_tx(&result);
    }

    // Reclaim the storage deposit, but only when it's actually reclaimable:
    // probe the registered slot first. A failed read means the contract has no
    // NEP-145 storage management (e.g. some NEP-245 multi-tokens) — skip it. A
    // present, non-zero slot means unregister should work, so a failure there is
    // a real error and propagates.
    let registered = match client
        .read(storage::GetBalanceOf {
            contract_id: contract_id.clone(),
            account_id: from.0.clone(),
        })
        .await
    {
        Ok(result) => result
            .balance
            .is_some_and(|balance| balance.total.as_yoctonear() > 0),
        Err(error) => {
            // Expected for tokens without NEP-145 storage management, not a fault.
            tracing::info!(%contract_id, %error, "storage_balance_of unavailable; assuming the token does not manage NEP-145 storage");
            false
        }
    };
    if registered {
        let result = client
            .execute_as(
                from.clone(),
                storage::Unregister {
                    contract_id,
                    force: false,
                },
            )
            .await?;
        ctx.report_tx(&result);
    }
    Ok(())
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
        ProxyOracleGovernanceNs::Create(a) => {
            let mut spec = a.parse()?;
            spec.full_access_keys = Some(ctx.default_full_access_keys()?);
            ctx.write(spec).await
        }
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
/// governance contract's next id when `--id` was omitted) and, for an
/// `add-circuit-breaker` proposal without `--breaker-id`, the set's next breaker
/// id. Always emits the resolved proposal id so scripts can learn it. With
/// `--execute-when-ready`, waits for the proposal's TTL to elapse and executes.
async fn create_proposal(ctx: CliContext, mut args: CreateProposal) -> anyhow::Result<()> {
    use templar_gateway_methods_spec::proxy_oracle_governance as gov;

    let governance_id = args.governance_id().clone();
    let execute_when_ready = args.execute_when_ready();

    // Auto-fill the next breaker id for add-circuit-breaker, resolving the proxy
    // oracle (whose set holds the breakers) through the governance contract.
    if let Some(price_id) = args.unresolved_breaker_price_id().map(str::to_owned) {
        let next_id = next_breaker_id(&ctx, &governance_id, &price_id).await?;
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

    // Keep the operator's idempotency key on the create write only.
    let create = ctx
        .client
        .execute_request(WriteRequest {
            signer_account_id: ctx.signer_account()?,
            idempotency_key: ctx.idempotency_key.clone(),
            body: args.into_spec(id)?,
        })
        .await?;
    ctx.report_tx(&create);

    let execute = if execute_when_ready {
        wait_for_maturity(&ctx, &governance_id, id).await?;
        let signer = ctx.signer_account()?;
        let result = ctx
            .client
            .execute_as(
                signer,
                gov::ExecuteProposal {
                    governance_id: governance_id.clone(),
                    id,
                },
            )
            .await?;
        ctx.report_tx(&result);
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

/// The next breaker id for `price_id` on the proxy oracle administered by
/// `governance_id`: resolve the oracle, read its circuit breaker set, and take
/// the set's next id (0 when no set exists yet).
async fn next_breaker_id(
    ctx: &CliContext,
    governance_id: &AccountId,
    price_id: &str,
) -> anyhow::Result<u32> {
    use super::commands::proxy_oracle::parse_price_identifier;
    use templar_gateway_methods_spec::{proxy_oracle, proxy_oracle_governance as gov};

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
            id: parse_price_identifier(price_id)?,
        })
        .await?
        .circuit_breaker_set;
    Ok(set.map_or(0, |set| set.next_id()))
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
        RedstoneNs::Create(a) => {
            let mut spec = a.parse()?;
            spec.full_access_keys = Some(ctx.default_full_access_keys()?);
            ctx.write(spec).await
        }
        RedstoneNs::GetConfig(a) => ctx.read(a.parse()).await,
        RedstoneNs::ReadPriceData(a) => ctx.read(a.parse()).await,
        RedstoneNs::ListRole(a) => ctx.read(a.parse()).await,
        RedstoneNs::SetRole(a) => ctx.write(a.parse()).await,
        RedstoneNs::WritePrices(a) => ctx.write(a.parse()?).await,
        RedstoneNs::UpdatePrices(a) => update_redstone_prices(ctx, a).await,
    }
}

/// Fetch a signed RedStone payload via the Node.js bridge, then write it on-chain
/// through the gateway `redstone.writePrices` operation — the single ergonomic
/// price-push command.
async fn update_redstone_prices(
    ctx: CliContext,
    args: redstone_cmd::UpdatePrices,
) -> anyhow::Result<()> {
    use templar_redstone_bridge::Bridge;
    use tokio::sync::watch;

    let (kill_tx, _kill_rx) = watch::channel(());
    let bridge = Bridge::new(args.node_path(), kill_tx.clone()).context("start RedStone bridge")?;
    tracing::info!(feeds = ?args.feed_ids(), "fetching prices from RedStone bridge");
    let payload = bridge
        .fetch(args.feed_ids().to_vec())
        .await
        .context("fetch RedStone payload")?;
    drop(kill_tx);

    ctx.write(args.write_spec(payload)).await
}

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
        let result = ctx
            .client
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
        ctx.report_tx(&result);
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
    ctx.report_tx(&result);
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
    /// Resolve the full access keys for a from-registry deploy from the CLI
    /// flags and the signer's public key.
    fn resolve_full_access_keys(
        &self,
        no_signer: bool,
        extra: &[near_api::PublicKey],
    ) -> anyhow::Result<Vec<PublicKey>> {
        Ok(registry::resolve_full_access_keys(
            self.signer_public_key()?,
            no_signer,
            extra,
        ))
    }

    /// The default full access keys for a deploy: just the signer's key.
    fn default_full_access_keys(&self) -> anyhow::Result<Vec<PublicKey>> {
        self.resolve_full_access_keys(false, &[])
    }

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
        self.report_tx(&output);
        print_json(&output)
    }
}
