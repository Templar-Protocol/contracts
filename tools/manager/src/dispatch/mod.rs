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
    account::AccountNs, contract::ContractNs, ft::FtNs, market::MarketNs,
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
        } => ctx.read(a.into_spec()).await,
        Command::Registry { command } => registry(ctx, command).await,
        Command::Storage { command } => storage(ctx, command).await,
        Command::Ft { command } => ft(ctx, command).await,
        Command::Market { command } => market(ctx, command).await,
        Command::ProxyOracle { command } => proxy_oracle(ctx, command).await,
        Command::ProxyOracleOwner { command } => proxy_oracle_owner(ctx, command).await,
        Command::ProxyOracleGovernance { command } => proxy_oracle_governance(ctx, command).await,
        Command::Redstone { command } => redstone(ctx, command).await,
        Command::RecoverNep141(args) => teardown::recover_nep141(ctx, args).await,
        Command::Read(call) => generic::read(ctx, call).await,
        Command::Write(call) => generic::write(ctx, call).await,
    }
}

async fn account(ctx: CliContext, ns: AccountNs) -> anyhow::Result<()> {
    match ns {
        AccountNs::Get(a) => ctx.read(a.into_spec()).await,
        AccountNs::Delete(a) => ctx.write(a.into_spec()).await,
    }
}

async fn registry(ctx: CliContext, ns: RegistryNs) -> anyhow::Result<()> {
    match ns {
        RegistryNs::ListVersions(a) => ctx.read(a.into_spec()).await,
        RegistryNs::ListDeployments(a) => ctx.read(a.into_spec()).await,
        RegistryNs::ListDeploymentsByKind(a) => ctx.read(a.into_spec()).await,
        RegistryNs::GetDeployment(a) => ctx.read(a.into_spec()).await,
        RegistryNs::AddVersion(a) => ctx.write(a.try_into_spec()?).await,
        RegistryNs::Deploy(a) => ctx.write(a.try_into_spec(&ctx)?).await,
        RegistryNs::RemoveVersion(a) => teardown::remove_version(ctx, a).await,
        RegistryNs::Remove(a) => teardown::registry_remove(ctx, a).await,
        RegistryNs::ClearDeployments(a) => teardown::clear_deployments(ctx, a).await,
    }
}

async fn storage(ctx: CliContext, ns: StorageNs) -> anyhow::Result<()> {
    match ns {
        StorageNs::GetBalanceBounds(a) => ctx.read(a.into_spec()).await,
        StorageNs::GetBalanceOf(a) => ctx.read(a.into_spec()).await,
        StorageNs::Deposit(a) => ctx.write(a.into_spec()).await,
        StorageNs::Unregister(a) => ctx.write(a.into_spec()).await,
        StorageNs::EnsureDeposit(a) => ctx.write(a.try_into_spec()?).await,
    }
}

async fn ft(ctx: CliContext, ns: FtNs) -> anyhow::Result<()> {
    match ns {
        FtNs::GetBalanceOf(a) => ctx.read(a.into_spec()).await,
        FtNs::Transfer(a) => ctx.write(a.into_spec()).await,
        FtNs::TransferCall(a) => ctx.write(a.into_spec()).await,
    }
}

async fn market(ctx: CliContext, ns: MarketNs) -> anyhow::Result<()> {
    match ns {
        MarketNs::Create(a) => ctx.write(a.try_into_spec(&ctx)?).await,
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
        ProxyOracleNs::GetProxy(a) => ctx.read(a.into_spec()).await,
        ProxyOracleNs::ListProxies(a) => ctx.read(a.into_spec()).await,
        ProxyOracleNs::PriceFeedExists(a) => ctx.read(a.into_spec()).await,
        ProxyOracleNs::UpdatePrices(a) => ctx.write(a.into_spec()).await,
    }
}

async fn proxy_oracle_owner(ctx: CliContext, ns: ProxyOracleOwnerNs) -> anyhow::Result<()> {
    match ns {
        ProxyOracleOwnerNs::GetOwner(a) => ctx.read(a.into_spec()).await,
        ProxyOracleOwnerNs::GetProposedOwner(a) => ctx.read(a.into_spec()).await,
        ProxyOracleOwnerNs::ProposeOwner(a) => ctx.write(a.into_spec()).await,
        ProxyOracleOwnerNs::AcceptOwner(a) => ctx.write(a.into_spec()).await,
        ProxyOracleOwnerNs::RenounceOwner(a) => ctx.write(a.into_spec()).await,
    }
}

async fn proxy_oracle_governance(
    ctx: CliContext,
    ns: ProxyOracleGovernanceNs,
) -> anyhow::Result<()> {
    match ns {
        ProxyOracleGovernanceNs::Create(a) => ctx.write(a.try_into_spec(&ctx)?).await,
        ProxyOracleGovernanceNs::CreateProposal(a) => proposals::create(ctx, a).await,
        ProxyOracleGovernanceNs::CancelProposal(a) => ctx.write(a.cancel()).await,
        ProxyOracleGovernanceNs::ExecuteProposal(a) => proposals::execute(ctx, a).await,
        ProxyOracleGovernanceNs::GetProposal(a) => ctx.read(a.get()).await,
        ProxyOracleGovernanceNs::ListProposals(a) => ctx.read(a.into_spec()).await,
        ProxyOracleGovernanceNs::NextProposalId(a) => ctx.read(a.next_proposal_id()).await,
        ProxyOracleGovernanceNs::ProposalCount(a) => ctx.read(a.proposal_count()).await,
        ProxyOracleGovernanceNs::GetOperationTtl(a) => ctx.read(a.into_spec()).await,
        ProxyOracleGovernanceNs::GetProxyOracleId(a) => ctx.read(a.get_proxy_oracle_id()).await,
        ProxyOracleGovernanceNs::HasRole(a) => ctx.read(a.into_spec()).await,
        ProxyOracleGovernanceNs::ListRole(a) => ctx.read(a.into_spec()).await,
        ProxyOracleGovernanceNs::GetRoles(a) => ctx.read(a.into_spec()).await,
    }
}

async fn redstone(ctx: CliContext, ns: RedstoneNs) -> anyhow::Result<()> {
    match ns {
        RedstoneNs::Create(a) => ctx.write(a.try_into_spec(&ctx)?).await,
        RedstoneNs::GetConfig(a) => ctx.read(a.into_spec()).await,
        RedstoneNs::ReadPriceData(a) => ctx.read(a.into_spec()).await,
        RedstoneNs::ListRole(a) => ctx.read(a.into_spec()).await,
        RedstoneNs::SetRole(a) => ctx.write(a.into_spec()).await,
        RedstoneNs::WritePrices(a) => ctx.write(a.try_into_spec()?).await,
        RedstoneNs::UpdatePrices(a) => prices::update_redstone(ctx, a).await,
    }
}
