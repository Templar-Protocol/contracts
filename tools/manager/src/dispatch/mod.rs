//! Command dispatch: map each parsed clap command to gateway reads and writes.
//!
//! Most arms are a direct `ctx.read`/`ctx.write` of a typed spec; the multi-step
//! flows live in focused submodules ([`teardown`], [`proposals`], [`generic`]) so
//! this file stays a readable index of the command surface.

pub(crate) mod generic;
mod proposals;
mod teardown;

use crate::cli::Command;
use crate::commands::{
    account::AccountNs, contract::ContractNs, ft::FtNs, market::MarketNs, oracle::OracleNs,
    proxy_oracle::ProxyOracleNs, proxy_oracle_governance::ProxyOracleGovernanceNs,
    proxy_oracle_owner::ProxyOracleOwnerNs, pyth::PythNs, redstone::RedstoneNs,
    registry::RegistryNs, storage::StorageNs,
};
use crate::context::{all_sources, lazer_source, print_json, redstone_source, CliContext};

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
        Command::Oracle { command } => oracle(ctx, command).await,
        Command::Pyth { command } => pyth(ctx, command).await,
        Command::Redstone { command } => redstone(ctx, command).await,
        Command::RecoverNep141(args) => teardown::recover_nep141(ctx, args).await,
        Command::Read(call) => generic::read(ctx, call).await,
        Command::Write(call) => generic::write(ctx, call).await,
    }
}

async fn account(ctx: CliContext, ns: AccountNs) -> anyhow::Result<()> {
    match ns {
        AccountNs::Get(a) => ctx.read(a.into_spec()).await,
        AccountNs::Delete(a) => ctx.write(a.signer.clone(), a.into_spec()).await,
    }
}

async fn registry(ctx: CliContext, ns: RegistryNs) -> anyhow::Result<()> {
    match ns {
        RegistryNs::ListVersions(a) => ctx.read(a.into_spec()).await,
        RegistryNs::ListDeployments(a) => ctx.read(a.into_spec()).await,
        RegistryNs::ListDeploymentsByKind(a) => ctx.read(a.into_spec()).await,
        RegistryNs::GetDeployment(a) => ctx.read(a.into_spec()).await,
        RegistryNs::AddVersion(a) => ctx.write(a.signer.clone(), a.try_into_spec()?).await,
        RegistryNs::Deploy(a) => ctx.write(a.signer.clone(), a.try_into_spec()?).await,
        RegistryNs::RemoveVersion(a) => teardown::remove_version(ctx, a).await,
        RegistryNs::Remove(a) => teardown::registry_remove(ctx, a).await,
        RegistryNs::ClearDeployments(a) => teardown::clear_deployments(ctx, a).await,
    }
}

async fn storage(ctx: CliContext, ns: StorageNs) -> anyhow::Result<()> {
    match ns {
        StorageNs::GetBalanceBounds(a) => ctx.read(a.into_spec()).await,
        StorageNs::GetBalanceOf(a) => ctx.read(a.into_spec()).await,
        StorageNs::Deposit(a) => ctx.write(a.signer.clone(), a.into_spec()).await,
        StorageNs::Unregister(a) => ctx.write(a.signer.clone(), a.into_spec()).await,
        StorageNs::EnsureDeposit(a) => ctx.write(a.signer.clone(), a.try_into_spec()?).await,
    }
}

async fn ft(ctx: CliContext, ns: FtNs) -> anyhow::Result<()> {
    match ns {
        FtNs::GetBalanceOf(a) => ctx.read(a.into_spec()).await,
        FtNs::Transfer(a) => ctx.write(a.signer.clone(), a.into_spec()).await,
        FtNs::TransferCall(a) => ctx.write(a.signer.clone(), a.into_spec()).await,
    }
}

async fn market(ctx: CliContext, ns: MarketNs) -> anyhow::Result<()> {
    match ns {
        MarketNs::Create(a) => ctx.write(a.signer.clone(), a.try_into_spec()?).await,
        MarketNs::Remove(a) => {
            // `market remove` is self-signed: the signer is the market account
            // being torn down.
            let (market, secret_key) = a.signer.resolve()?;
            let client = ctx.signing_client(market.clone(), secret_key)?;
            teardown::remove_market(&ctx, &client, market, a.beneficiary_id(), a.force()).await?;
            print_json(&serde_json::json!({ "removed": true }))
        }
    }
}

async fn proxy_oracle(ctx: CliContext, ns: ProxyOracleNs) -> anyhow::Result<()> {
    match ns {
        ProxyOracleNs::GetProxy(a) => ctx.read(a.into_spec()).await,
        ProxyOracleNs::ListProxies(a) => ctx.read(a.into_spec()).await,
        ProxyOracleNs::PriceFeedExists(a) => ctx.read(a.into_spec()).await,
        ProxyOracleNs::GetProxyCircuitBreakerSet(a) => ctx.read(a.into_spec()).await,
        ProxyOracleNs::UpdatePrices(a) => ctx.write(a.signer.clone(), a.into_spec()).await,
    }
}

/// The `oracle.*` updates, served by the oracle-updates dispatcher. Each arm layers
/// only the payload sources its method fetches from — see [`CliContext::oracle_write`].
async fn oracle(ctx: CliContext, ns: OracleNs) -> anyhow::Result<()> {
    match ns {
        OracleNs::Pyth(a) => {
            let signer = a.signer.clone();
            ctx.oracle_write(signer, a.try_into_spec()?, Ok).await
        }
        OracleNs::RedStone(a) => {
            let (signer, sources) = (a.signer.clone(), a.sources.clone());
            ctx.oracle_write(signer, a.into_spec(), |base| {
                redstone_source(base, &sources)
            })
            .await
        }
        OracleNs::Lazer(a) => {
            let (signer, sources) = (a.signer.clone(), a.sources.clone());
            ctx.oracle_write(signer, a.into_spec(), |base| lazer_source(base, &sources))
                .await
        }
        OracleNs::Prices(a) => {
            let (signer, sources) = (a.signer.clone(), a.sources.clone());
            ctx.oracle_write(signer, a.into_spec(), |base| all_sources(base, &sources))
                .await
        }
    }
}

async fn pyth(ctx: CliContext, ns: PythNs) -> anyhow::Result<()> {
    match ns {
        PythNs::ListEmaPricesNoOlderThan(a) => ctx.read(a.into_spec()).await,
        PythNs::ListEmaPricesUnsafe(a) => ctx.read(a.into_spec()).await,
        PythNs::UpdatePriceFeeds(a) => ctx.write(a.signer.clone(), a.try_into_spec()?).await,
    }
}

async fn proxy_oracle_owner(ctx: CliContext, ns: ProxyOracleOwnerNs) -> anyhow::Result<()> {
    match ns {
        ProxyOracleOwnerNs::GetOwner(a) => ctx.read(a.into_spec()).await,
        ProxyOracleOwnerNs::GetProposedOwner(a) => ctx.read(a.into_spec()).await,
        ProxyOracleOwnerNs::ProposeOwner(a) => ctx.write(a.signer.clone(), a.into_spec()).await,
        ProxyOracleOwnerNs::AcceptOwner(a) => ctx.write(a.signer.clone(), a.into_spec()).await,
        ProxyOracleOwnerNs::RenounceOwner(a) => ctx.write(a.signer.clone(), a.into_spec()).await,
    }
}

async fn proxy_oracle_governance(
    ctx: CliContext,
    ns: ProxyOracleGovernanceNs,
) -> anyhow::Result<()> {
    match ns {
        ProxyOracleGovernanceNs::Create(a) => ctx.write(a.signer.clone(), a.try_into_spec()?).await,
        ProxyOracleGovernanceNs::CreateProposal(a) => proposals::create(ctx, a).await,
        ProxyOracleGovernanceNs::CancelProposal(a) => ctx.write(a.signer.clone(), a.cancel()).await,
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
        RedstoneNs::Create(a) => ctx.write(a.signer.clone(), a.try_into_spec()?).await,
        RedstoneNs::GetConfig(a) => ctx.read(a.into_spec()).await,
        RedstoneNs::ReadPriceData(a) => ctx.read(a.into_spec()).await,
        RedstoneNs::ListRole(a) => ctx.read(a.into_spec()).await,
        RedstoneNs::SetRole(a) => ctx.write(a.signer.clone(), a.into_spec()).await,
        RedstoneNs::WritePrices(a) => ctx.write(a.signer.clone(), a.try_into_spec()?).await,
    }
}
