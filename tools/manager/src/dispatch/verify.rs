//! `market verify` — re-run the preflight against a market that already exists.
//!
//! The market contract backstops its own configuration at init; the oracle
//! wiring has no backstop, and `admin_set_proxy` is dispatched detached, so a
//! proposal reports success even when the oracle rejected it. Deployed state is
//! the only witness.
//!
//! Exits non-zero on failure, so it can also run on a schedule.

use anyhow::Context as _;
use near_account_id::AccountId;

use crate::commands::market::Verify;
use crate::context::{print_json, CliContext};
use crate::spec::{
    check::{Check, Status},
    MarketSpec,
};

pub(super) async fn market(ctx: CliContext, args: Verify) -> anyhow::Result<()> {
    let mut spec =
        super::export::reconstruct(&ctx, &args.market_id, &args.governance_admin).await?;

    let oracle_id = spec.reads_oracle_id()?;
    let deployed_proxy = spec.own_proxy_id()?;

    // Fields that never reach the chain cannot be recovered from it, so an
    // intended spec supplies them. Without this the reference cross-check — the
    // only one that catches a transposed feed on a *live* market — reports
    // itself skipped on every run of the command built to monitor live markets.
    let intended = args
        .against
        .as_ref()
        .map(|path| crate::spec::extends::load(path))
        .transpose()?;
    if let Some(intended) = &intended {
        spec.collateral
            .symbol
            .clone_from(&intended.collateral.symbol);
        spec.collateral
            .reference
            .clone_from(&intended.collateral.reference);
        spec.borrow.symbol.clone_from(&intended.borrow.symbol);
        spec.borrow.reference.clone_from(&intended.borrow.reference);
        // The tolerances travel with them. `market export` cannot recover a
        // judgement call either, so it emits the default — leaving these behind
        // verifies a deliberately wider band against 1.5% and fails it, or
        // verifies a narrower one against 1.5% and passes it.
        spec.market
            .reference_tolerance
            .clone_from(&intended.market.reference_tolerance);
        spec.collateral
            .reference_tolerance
            .clone_from(&intended.collateral.reference_tolerance);
        spec.borrow
            .reference_tolerance
            .clone_from(&intended.borrow.reference_tolerance);
    }

    // The same checks `spec check` runs: one that checked less would pass
    // markets a preflight refuses. The proxy is named so the aggregation
    // resolves against the breakers the deployed oracle carries.
    let mut checks = super::preflight::run_all(
        &ctx,
        &mut spec,
        false,
        args.accept_decimals_mismatch,
        deployed_proxy.as_ref(),
    )
    .await?;

    // Only meaningful for a proxy market: a direct market reads an oracle it
    // does not own, and governing it is somebody else's business.
    if !spec.oracle.is_direct() {
        checks.push(admin_holds_the_role(&ctx, &spec, &args.governance_admin).await?);
        checks.extend(governance_controls_the_oracle(&ctx, &spec).await?);
    }

    if let (Some(intended), Some(path)) = (&intended, &args.against) {
        checks.push(matches_intent(&spec, intended, path));
    }

    print_json(&crate::spec::check::Report {
        subject: VerifiedMarket {
            market_id: args.market_id.clone(),
            oracle_id,
        },
        checks: &checks,
    })?;

    crate::spec::check::gate(
        &checks,
        args.market_id.as_str(),
        "this market does not match its spec",
    )
}

/// Everything a spec determines about a deployed market, for comparison
/// against the same projection of what is on chain.
#[derive(serde::Serialize)]
struct Projection {
    configuration: templar_common::market::MarketConfiguration,
    collateral_proxy:
        templar_proxy_oracle_kernel::proxy::Proxy<templar_proxy_oracle_near_common::input::Source>,
    borrow_proxy:
        templar_proxy_oracle_kernel::proxy::Proxy<templar_proxy_oracle_near_common::input::Source>,
    /// Recoverable, so it is compared: a timelock lowered to zero on a live
    /// oracle is exactly the drift this check exists to catch. `admin` is left
    /// out — on the deployed side it is the CLI flag, checked separately.
    governance_ttl: Option<templar_common::Nanoseconds>,
}

/// What `market verify` reports alongside its checks.
#[derive(serde::Serialize)]
struct VerifiedMarket {
    market_id: AccountId,
    oracle_id: AccountId,
}

/// Does governance actually control the oracle this market reads?
///
/// `governance.admin` proves only that the admin holds the role on the account
/// a spec *derives*. An oracle owned by anything else answers to that owner with
/// no timelock, and would otherwise verify clean.
async fn governance_controls_the_oracle(
    ctx: &CliContext,
    spec: &MarketSpec,
) -> anyhow::Result<Vec<Check>> {
    use templar_gateway_methods_spec::{owner, proxy_oracle_governance as gov};

    let governance_id = spec.governance_id()?;
    let oracle_id = spec.oracle_id()?;

    let owner_status = match ctx
        .client
        .read(owner::GetOwner {
            contract_id: oracle_id.clone(),
        })
        .await
    {
        Ok(result) => match result.owner {
            Some(owner) if owner == governance_id => {
                Status::passed(format!("`{oracle_id}` is owned by `{governance_id}`"))
            }
            Some(owner) => Status::failed(format!(
                "`{oracle_id}` is owned by `{owner}`, not by `{governance_id}`. \
                 Governance cannot configure this oracle, and `admin_set_proxy` \
                 is dispatched detached, so a proposal against it would still \
                 report success."
            )),
            None => Status::failed(format!(
                "`{oracle_id}` has no owner, so nothing can configure its feeds"
            )),
        },
        Err(error) => Status::failed(format!(
            "could not read the owner of `{oracle_id}`: {error}"
        )),
    };

    let governs_status = match ctx
        .client
        .read(gov::GetProxyOracleId {
            governance_id: governance_id.clone(),
        })
        .await
    {
        Ok(result) if result.proxy_oracle_id == oracle_id => {
            Status::passed(format!("`{governance_id}` governs `{oracle_id}`"))
        }
        Ok(result) => Status::failed(format!(
            "`{governance_id}` governs `{}`, not `{oracle_id}`. Proposals made \
             through it would configure a different oracle.",
            result.proxy_oracle_id
        )),
        Err(error) => Status::failed(format!(
            "could not read which oracle `{governance_id}` governs: {error}"
        )),
    };

    Ok(vec![
        Check::new("governance.owns_oracle", owner_status),
        Check::new("governance.governs_oracle", governs_status),
    ])
}

/// Is `--governance-admin` actually the admin? No view names the role's holder,
/// so it is supplied — and an unverified assertion would flow into every
/// comparison below as fact.
async fn admin_holds_the_role(
    ctx: &CliContext,
    spec: &MarketSpec,
    admin: &near_account_id::AccountId,
) -> anyhow::Result<Check> {
    use templar_gateway_methods_spec::proxy_oracle_governance as gov;
    use templar_proxy_oracle_near_governance_common::Role;

    let governance_id = spec.governance_id()?;
    let status = match ctx
        .client
        .read(gov::HasRole {
            governance_id: governance_id.clone(),
            account_id: admin.clone(),
            role: Role::Admin,
        })
        .await
    {
        Ok(result) if result.has_role => Status::passed(format!("`{admin}` on {governance_id}")),
        Ok(_) => Status::failed(format!(
            "`{admin}` does not hold Admin on `{governance_id}`, so the exported \
             spec names an admin that cannot govern this oracle"
        )),
        Err(error) => Status::failed(format!("could not check the Admin role: {error}")),
    };
    Ok(Check::new("governance.admin", status))
}

/// Does what is deployed still match what was intended?
///
/// Compares on-chain projections, not the specs: a reconstruction cannot
/// recover what never reached the chain, so comparing documents would report
/// drift forever. `versions` is excluded too — a deployment records what it was
/// created with, a spec names what to deploy next.
fn matches_intent(deployed: &MarketSpec, intended: &MarketSpec, path: &std::path::Path) -> Check {
    let id = "verify.matches_intent";

    let projected = |spec: &MarketSpec| -> anyhow::Result<serde_json::Value> {
        let (Some(collateral), Some(borrow)) = (spec.collateral.decimals, spec.borrow.decimals)
        else {
            anyhow::bail!("decimals are unresolved, so no configuration can be projected");
        };
        let age = spec.market.price_maximum_age;
        // Compared as a `Value` because the difference has to be *reported* per
        // key; the projection itself is the typed shape below.
        serde_json::to_value(Projection {
            configuration: spec
                .clone()
                .into_market_configuration(i32::from(collateral), i32::from(borrow))?,
            collateral_proxy: spec.collateral.clone().into_proxy(age),
            borrow_proxy: spec.borrow.clone().into_proxy(age),
            governance_ttl: spec.governance.as_ref().map(|it| it.ttl_default),
        })
        .context("project the spec")
    };

    match (projected(deployed), projected(intended)) {
        (Ok(left), Ok(right)) if left == right => {
            Check::new(id, Status::passed(format!("matches {}", path.display())))
        }
        (Ok(left), Ok(right)) => Check::new(
            id,
            Status::failed(format!(
                "deployed state differs from {} in: {}. Run `market export` to \
                 see what is actually deployed.",
                path.display(),
                differing_keys(&left, &right).join(", ")
            )),
        ),
        // Unprojectable is unknown, and unknown is not a match.
        (left, right) => Check::new(
            id,
            Status::failed(format!(
                "could not compare against {}: {}",
                path.display(),
                left.err()
                    .or(right.err())
                    .map_or_else(|| "unknown".to_owned(), |error| format!("{error:#}"))
            )),
        ),
    }
}

/// Which top-level sections differ, so the failure names where to look rather
/// than dumping two documents.
fn differing_keys(deployed: &serde_json::Value, intended: &serde_json::Value) -> Vec<String> {
    let (Some(left), Some(right)) = (deployed.as_object(), intended.as_object()) else {
        return vec!["the whole document".to_owned()];
    };

    let mut keys: Vec<String> = left
        .keys()
        .chain(right.keys())
        .filter(|key| left.get(*key) != right.get(*key))
        .cloned()
        .collect();
    keys.sort();
    keys.dedup();
    keys
}
