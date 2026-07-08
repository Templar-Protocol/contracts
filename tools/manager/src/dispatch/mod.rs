//! Command dispatch: map each parsed clap command to gateway reads and writes.
//!
//! Most arms are a direct `ctx.read`/`ctx.write` of a typed spec; the multi-step
//! flows live in focused submodules ([`teardown`], [`proposals`], [`prices`],
//! [`generic`]) so this file stays a readable index of the command surface.

mod generic;
mod prices;
mod proposals;
mod teardown;

use crate::cli::Command;
use crate::commands::{
    account::AccountNs, contract::ContractNs, ft::FtNs, market::MarketNs, op::OpNs,
    proxy_oracle::ProxyOracleNs, proxy_oracle_governance::ProxyOracleGovernanceNs,
    proxy_oracle_owner::ProxyOracleOwnerNs, redstone::RedstoneNs, registry::RegistryNs,
    storage::StorageNs,
};
use crate::context::{print_json, CliContext};

pub(crate) async fn dispatch(ctx: CliContext, command: Command) -> anyhow::Result<()> {
    match command {
        Command::Account { command } => account(ctx, command).await,
        Command::Contract {
            command: ContractNs::GetVersion(a),
        } => ctx.read(a.parse()).await,
        Command::Registry { command } => registry(ctx, command).await,
        Command::Storage { command } => storage(ctx, command).await,
        Command::Ft { command } => ft(ctx, command).await,
        Command::Market { command } => market(ctx, command).await,
        Command::ProxyOracle { command } => proxy_oracle(ctx, command).await,
        Command::ProxyOracleOwner { command } => proxy_oracle_owner(ctx, command).await,
        Command::ProxyOracleGovernance { command } => proxy_oracle_governance(ctx, command).await,
        Command::Redstone { command } => redstone(ctx, command).await,
        Command::RecoverNep141(args) => teardown::recover_nep141(ctx, args).await,
        Command::Op {
            command: OpNs::Get(get),
        } => op_get(ctx, get).await,
        Command::Read(call) => generic::read(ctx, call).await,
        Command::Write(call) => generic::write(ctx, call).await,
    }
}

async fn account(ctx: CliContext, ns: AccountNs) -> anyhow::Result<()> {
    match ns {
        AccountNs::Get(a) => ctx.read(a.parse()).await,
        AccountNs::Delete(a) => ctx.write(a.parse()).await,
    }
}

async fn registry(ctx: CliContext, ns: RegistryNs) -> anyhow::Result<()> {
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
        RegistryNs::RemoveVersion(a) => teardown::remove_version(ctx, a).await,
        RegistryNs::Remove(a) => teardown::registry_remove(ctx, a).await,
        RegistryNs::ClearDeployments(a) => teardown::clear_deployments(ctx, a).await,
    }
}

async fn storage(ctx: CliContext, ns: StorageNs) -> anyhow::Result<()> {
    match ns {
        StorageNs::GetBalanceBounds(a) => ctx.read(a.parse()).await,
        StorageNs::GetBalanceOf(a) => ctx.read(a.parse()).await,
        StorageNs::Deposit(a) => ctx.write(a.parse()).await,
        StorageNs::Unregister(a) => ctx.write(a.parse()).await,
        StorageNs::EnsureDeposit(a) => ctx.write(a.parse()?).await,
    }
}

async fn ft(ctx: CliContext, ns: FtNs) -> anyhow::Result<()> {
    match ns {
        FtNs::GetBalanceOf(a) => ctx.read(a.parse()).await,
        FtNs::Transfer(a) => ctx.write(a.parse()).await,
        FtNs::TransferCall(a) => ctx.write(a.parse()).await,
    }
}

async fn market(ctx: CliContext, ns: MarketNs) -> anyhow::Result<()> {
    match ns {
        MarketNs::Create(a) => {
            let mut spec = a.parse()?;
            spec.full_access_keys = Some(ctx.default_full_access_keys()?);
            ctx.write(spec).await
        }
        MarketNs::Remove(a) => {
            let signer = ctx.signer_account()?;
            teardown::remove_market(&ctx, &ctx.client, signer, a.beneficiary_id(), a.force())
                .await?;
            print_json(&serde_json::json!({ "removed": true }))
        }
    }
}

async fn proxy_oracle(ctx: CliContext, ns: ProxyOracleNs) -> anyhow::Result<()> {
    match ns {
        ProxyOracleNs::GetProxy(a) => ctx.read(a.parse()?).await,
        ProxyOracleNs::ListProxies(a) => ctx.read(a.parse()).await,
        ProxyOracleNs::PriceFeedExists(a) => ctx.read(a.parse()?).await,
        ProxyOracleNs::UpdatePrices(a) => ctx.write(a.parse()?).await,
    }
}

async fn proxy_oracle_owner(ctx: CliContext, ns: ProxyOracleOwnerNs) -> anyhow::Result<()> {
    match ns {
        ProxyOracleOwnerNs::GetOwner(a) => ctx.read(a.get_owner()).await,
        ProxyOracleOwnerNs::GetProposedOwner(a) => ctx.read(a.get_proposed_owner()).await,
        ProxyOracleOwnerNs::ProposeOwner(a) => ctx.write(a.parse()).await,
        ProxyOracleOwnerNs::AcceptOwner(a) => ctx.write(a.accept_owner()).await,
        ProxyOracleOwnerNs::RenounceOwner(a) => ctx.write(a.renounce_owner()).await,
    }
}

async fn proxy_oracle_governance(
    ctx: CliContext,
    ns: ProxyOracleGovernanceNs,
) -> anyhow::Result<()> {
    match ns {
        ProxyOracleGovernanceNs::Create(a) => {
            let mut spec = a.parse()?;
            spec.full_access_keys = Some(ctx.default_full_access_keys()?);
            ctx.write(spec).await
        }
        ProxyOracleGovernanceNs::CreateProposal(a) => proposals::create(ctx, a).await,
        ProxyOracleGovernanceNs::CancelProposal(a) => ctx.write(a.cancel()).await,
        ProxyOracleGovernanceNs::ExecuteProposal(a) => proposals::execute(ctx, a).await,
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

async fn redstone(ctx: CliContext, ns: RedstoneNs) -> anyhow::Result<()> {
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
        RedstoneNs::UpdatePrices(a) => prices::update_redstone(ctx, a).await,
    }
}

async fn op_get(ctx: CliContext, get: crate::commands::op::Get) -> anyhow::Result<()> {
    if !ctx.has_operation_store {
        anyhow::bail!("op.get requires --gateway-store-url");
    }
    let request = get.parse();
    let operation = ctx.client.operation(&request.operation_id).await?;
    print_json(&templar_gateway_methods_spec::op::GetResult { operation })
}
