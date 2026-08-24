//! Target resolution for `proxy-oracle`/`proxy-oracle gov` commands: turn a
//! `--market-id` (or, for governance, an `--oracle-id`) into the concrete oracle
//! or governance account a command operates on, asserting the resolved contract
//! is the expected kind before any write is planned.
//!
//! Each resolution is a handful of extra view calls — fine for an ops CLI. The
//! assertions are hard errors, not warnings: a wrong-target admin write is the
//! failure mode this prevents, so a market whose oracle isn't a proxy oracle (or
//! a proxy oracle not owned by a governance contract) fails with a diagnostic
//! naming the resolution step that broke.

use anyhow::{bail, Context as _};
use clap::{ArgGroup, Args};
use near_account_id::AccountId;
use templar_gateway_methods_spec::{contract, market, owner, proxy_oracle_governance as gov};
use templar_gateway_types::contract::ContractKind;

use crate::context::CliContext;

/// Target for `proxy-oracle` commands: an explicit proxy-oracle account, or a
/// market whose configured oracle is resolved and asserted to be a proxy oracle.
#[derive(Args, Debug)]
#[command(group(ArgGroup::new("proxy_oracle_target").required(true).args(["oracle_id", "market_id"])))]
pub(crate) struct OracleTarget {
    /// Proxy-oracle account to target.
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: Option<AccountId>,
    /// Market whose configured proxy oracle is the target.
    #[arg(long, value_name = "ACCOUNT_ID")]
    market_id: Option<AccountId>,
}

impl OracleTarget {
    /// Build a target from already-parsed selectors, for a command whose own argument group is
    /// wider than this one's — e.g. one that also accepts a registry to sweep.
    pub(crate) const fn from_parts(
        oracle_id: Option<AccountId>,
        market_id: Option<AccountId>,
    ) -> Self {
        Self {
            oracle_id,
            market_id,
        }
    }

    /// The proxy-oracle account: `--oracle-id` verbatim, or the oracle resolved
    /// from `--market-id`.
    pub(crate) async fn resolve(&self, ctx: &CliContext) -> anyhow::Result<AccountId> {
        match (&self.oracle_id, &self.market_id) {
            (Some(oracle_id), _) => Ok(oracle_id.clone()),
            (_, Some(market_id)) => resolve_oracle_from_market(ctx, market_id).await,
            (None, None) => bail!("either --oracle-id or --market-id is required"),
        }
    }
}

/// Target for `proxy-oracle gov` commands: an explicit governance account, the
/// governance contract that owns a proxy oracle, or the one reached through a
/// market's proxy oracle. Each indirect path confirms the resolved contract
/// actually governs that oracle.
#[derive(Args, Debug)]
#[command(group(ArgGroup::new("governance_target").required(true).args(["governance_id", "oracle_id", "market_id"])))]
#[allow(clippy::struct_field_names)]
pub(crate) struct GovernanceTarget {
    /// Governance contract account to target.
    #[arg(long, value_name = "ACCOUNT_ID")]
    governance_id: Option<AccountId>,
    /// Proxy oracle whose governance contract is the target.
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: Option<AccountId>,
    /// Market whose proxy oracle's governance contract is the target.
    #[arg(long, value_name = "ACCOUNT_ID")]
    market_id: Option<AccountId>,
}

impl GovernanceTarget {
    /// The governance account: `--governance-id` verbatim, the governance
    /// contract owning `--oracle-id`, or the one reached via `--market-id`'s
    /// proxy oracle.
    pub(crate) async fn resolve(&self, ctx: &CliContext) -> anyhow::Result<AccountId> {
        match (&self.governance_id, &self.oracle_id, &self.market_id) {
            (Some(governance_id), _, _) => Ok(governance_id.clone()),
            (_, Some(oracle_id), _) => governance_from_oracle(ctx, oracle_id).await,
            (_, _, Some(market_id)) => resolve_governance_from_market(ctx, market_id).await,
            (None, None, None) => {
                bail!("one of --governance-id, --oracle-id, or --market-id is required")
            }
        }
    }
}

/// Read a market's configured price-oracle account (no kind assertion).
async fn read_market_oracle(ctx: &CliContext, market_id: &AccountId) -> anyhow::Result<AccountId> {
    Ok(ctx
        .client
        .read(market::GetConfiguration {
            market_id: market_id.clone(),
        })
        .await
        .with_context(|| format!("read market.getConfiguration for {market_id}"))?
        .price_oracle_configuration
        .account_id)
}

/// Read a market's configured oracle and assert it is a proxy oracle. Used on the
/// `--market-id` proxy-oracle path, which has no later round-trip to prove it.
async fn resolve_oracle_from_market(
    ctx: &CliContext,
    market_id: &AccountId,
) -> anyhow::Result<AccountId> {
    let oracle_id = read_market_oracle(ctx, market_id).await?;

    let kind = ctx
        .client
        .read(contract::GetKind {
            contract_id: oracle_id.clone(),
        })
        .await
        .with_context(|| format!("read contract.getKind for oracle {oracle_id}"))?
        .kind;
    ensure_proxy_oracle(kind, &oracle_id)?;

    Ok(oracle_id)
}

/// Resolve a market's governance contract: read the market's oracle, then the
/// governance contract that owns it. No proxy-oracle `getKind` assertion here —
/// the `getProxyOracleId` round-trip in [`governance_from_oracle`] is
/// itself proof the oracle is a proxy oracle governed by that account.
async fn resolve_governance_from_market(
    ctx: &CliContext,
    market_id: &AccountId,
) -> anyhow::Result<AccountId> {
    let oracle_id = read_market_oracle(ctx, market_id).await?;
    governance_from_oracle(ctx, &oracle_id).await
}

/// Resolve a proxy oracle's governance contract: read the oracle's owner, then
/// confirm it governs that oracle by round-tripping `getProxyOracleId`.
pub(crate) async fn governance_from_oracle(
    ctx: &CliContext,
    oracle_id: &AccountId,
) -> anyhow::Result<AccountId> {
    let owner = ctx
        .client
        .read(owner::GetOwner {
            contract_id: oracle_id.clone(),
        })
        .await
        .with_context(|| format!("read owner.getOwner for oracle {oracle_id}"))?
        .owner;
    let governance_id = ensure_owner_present(owner, oracle_id)?;

    let governed = ctx
        .client
        .read(gov::GetProxyOracleId {
            governance_id: governance_id.clone(),
        })
        .await
        .with_context(|| {
            format!("read proxyOracleGovernance.getProxyOracleId for {governance_id}")
        })?
        .proxy_oracle_id;
    ensure_governs(oracle_id, &governance_id, &governed)?;

    Ok(governance_id)
}

/// Assert `oracle_id` is a proxy oracle, naming the failed step otherwise.
fn ensure_proxy_oracle(kind: ContractKind, oracle_id: &AccountId) -> anyhow::Result<()> {
    if kind == ContractKind::ProxyOracle {
        return Ok(());
    }
    bail!(
        "resolution step 'assert proxy oracle' failed: the market's configured oracle \
         {oracle_id} is a {kind:?}, not a proxy oracle"
    );
}

/// Take a proxy oracle's owner as its governance contract, failing when the
/// oracle has no owner.
fn ensure_owner_present(
    owner: Option<AccountId>,
    oracle_id: &AccountId,
) -> anyhow::Result<AccountId> {
    owner.with_context(|| {
        format!(
            "resolution step 'read oracle owner' failed: proxy oracle {oracle_id} has no owner, \
             so no governance contract can be resolved"
        )
    })
}

/// Confirm the resolved governance contract governs the resolved oracle, naming
/// the failed step otherwise.
fn ensure_governs(
    oracle_id: &AccountId,
    governance_id: &AccountId,
    governed: &AccountId,
) -> anyhow::Result<()> {
    if governed == oracle_id {
        return Ok(());
    }
    bail!(
        "resolution step 'confirm governance' failed: {governance_id} governs {governed}, \
         not the resolved oracle {oracle_id}"
    );
}

#[cfg(test)]
mod tests {
    use super::{ensure_governs, ensure_owner_present, ensure_proxy_oracle};
    use near_account_id::AccountId;
    use templar_gateway_types::contract::ContractKind;

    fn account(id: &str) -> AccountId {
        id.parse().expect("valid account id")
    }

    #[test]
    fn proxy_oracle_kind_passes_others_fail_by_step() {
        let oracle = account("oracle.testnet");
        ensure_proxy_oracle(ContractKind::ProxyOracle, &oracle).expect("proxy oracle passes");

        let error = ensure_proxy_oracle(ContractKind::PythOracle, &oracle)
            .expect_err("a non-proxy oracle must fail");
        let message = error.to_string();
        assert!(message.contains("assert proxy oracle"), "{message}");
        assert!(message.contains("oracle.testnet"), "{message}");
    }

    #[test]
    fn missing_owner_fails_naming_the_step() {
        let oracle = account("oracle.testnet");
        let governance = ensure_owner_present(Some(account("gov.testnet")), &oracle)
            .expect("an owner resolves to the governance account");
        assert_eq!(governance.as_str(), "gov.testnet");

        let error = ensure_owner_present(None, &oracle).expect_err("an unowned oracle must fail");
        let message = error.to_string();
        assert!(message.contains("read oracle owner"), "{message}");
        assert!(message.contains("oracle.testnet"), "{message}");
    }

    #[test]
    fn governance_must_govern_the_resolved_oracle() {
        let oracle = account("oracle.testnet");
        let governance = account("gov.testnet");
        ensure_governs(&oracle, &governance, &oracle).expect("a matching round-trip passes");

        let error = ensure_governs(&oracle, &governance, &account("other.testnet"))
            .expect_err("governing a different oracle must fail");
        let message = error.to_string();
        assert!(message.contains("confirm governance"), "{message}");
        assert!(message.contains("other.testnet"), "{message}");
    }
}
