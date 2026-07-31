//! `market verify` — re-run the preflight against a market that already exists.
//!
//! The closure for the gap ENG-544 opened deliberately. A hand-edited plan
//! bypasses every spec-level check, because by then the arguments are already
//! encoded. The market contract still runs `MarketConfiguration::validate()` at
//! init, so that path has a backstop; **the oracle wiring has none** — and
//! `admin_set_proxy` is dispatched detached, so the proposal that should have
//! configured a feed reports success even when the oracle rejected it. Deployed
//! state is the only witness.
//!
//! Useful independently, too: confirm a market still resolves prices after an
//! upstream adapter is migrated or a source reconfigured. Exits non-zero on
//! failure so it can run on a schedule.

use crate::commands::market::Verify;
use crate::context::{print_json, CliContext};
use crate::spec::{
    check::{Check, Status},
    MarketSpec,
};

pub(super) async fn market(ctx: CliContext, args: Verify) -> anyhow::Result<()> {
    let mut spec =
        super::export::reconstruct(&ctx, &args.market_id, &args.governance_admin).await?;

    let oracle_id = spec.oracle_id()?;

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
    }

    // The same checks `spec check` runs, against the reconstructed spec — not a
    // parallel set. A verify that checked less than a preflight would pass
    // markets a preflight would refuse. The oracle is named so the aggregation
    // resolves against the breakers the deployed one actually carries.
    let mut checks = super::preflight::run_all(
        &ctx,
        &mut spec,
        false,
        args.accept_decimals_mismatch,
        Some(&oracle_id),
    )
    .await?;

    checks.push(admin_holds_the_role(&ctx, &spec, &args.governance_admin).await?);

    if let (Some(intended), Some(path)) = (&intended, &args.against) {
        checks.push(matches_intent(&spec, intended, path));
    }

    print_json(&serde_json::json!({
        "market_id": args.market_id,
        "oracle_id": spec.oracle_id()?,
        "checks": checks,
    }))?;

    let failed = crate::spec::check::failures(&checks);
    anyhow::ensure!(
        failed == 0,
        "{failed} check(s) failed for {}",
        args.market_id
    );
    Ok(())
}

/// Is `--governance-admin` actually the admin?
///
/// It cannot be recovered from chain state — the role is granted at init and no
/// view names its holder — so it is supplied. Supplied is not the same as true:
/// the reconstructed spec embeds it, and an unverified assertion would flow
/// into every comparison below as though it were fact. `hasRole` can settle it,
/// so it does.
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
/// Compares the two specs' **on-chain projections**, not the specs themselves.
/// A reconstruction cannot recover what never reached the chain — `symbol`,
/// `reference`, and the freshness bounds an authored spec leaves to defaults —
/// so comparing documents reports drift for a market that is byte-identical to
/// its own spec, on every scheduled run forever. What is deployable is what is
/// comparable.
///
/// `versions` is likewise not compared: a deployment records the version it was
/// created with, while a spec names the version to deploy *next*.
fn matches_intent(deployed: &MarketSpec, intended: &MarketSpec, path: &std::path::Path) -> Check {
    let id = "verify.matches_intent";

    let projected = |spec: &MarketSpec| -> anyhow::Result<serde_json::Value> {
        let (Some(collateral), Some(borrow)) = (spec.collateral.decimals, spec.borrow.decimals)
        else {
            anyhow::bail!("decimals are unresolved, so no configuration can be projected");
        };
        let age = spec.market.price_maximum_age;
        Ok(serde_json::json!({
            "configuration": spec
                .clone()
                .into_market_configuration(i32::from(collateral), i32::from(borrow))?,
            "collateral_proxy": spec.collateral.clone().into_proxy(age),
            "borrow_proxy": spec.borrow.clone().into_proxy(age),
        }))
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
