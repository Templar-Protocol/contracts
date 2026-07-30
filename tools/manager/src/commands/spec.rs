//! Local operations on a deployment spec. Nothing here touches the network —
//! that is the property the offline checks exist to preserve.

use std::path::PathBuf;

use clap::{Args, Subcommand};

use crate::spec::{check, extends};

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum SpecNs {
    /// Resolve a spec's `extends` chain and run every offline check.
    Check(Check),
    /// Emit the spec's JSON Schema, for editor completion and validation.
    PrintSchema,
}

#[derive(Args, Debug)]
pub struct Check {
    /// Path to the market spec.
    pub path: PathBuf,
}

impl Check {
    /// Load, check, and report. Fails when any check failed, so this is usable
    /// as a gate in CI or a pre-deploy script.
    pub fn run(&self) -> anyhow::Result<()> {
        let spec = extends::load(&self.path)?;
        let checks = check::run_offline(&spec);

        // The derived proxies are reported, not just the ids: their freshness
        // bounds are defaulted here, so an operator should be able to see the
        // resolved result before anything is deployed.
        let price_maximum_age = spec.market.price_maximum_age;
        crate::context::print_json(&serde_json::json!({
            "market_id": spec.market_id()?,
            "oracle_id": spec.oracle_id()?,
            "governance_id": spec.governance_id()?,
            "network": spec.network()?.to_string(),
            "collateral_proxy": spec.collateral.clone().into_proxy(price_maximum_age),
            "borrow_proxy": spec.borrow.clone().into_proxy(price_maximum_age),
            "checks": checks,
        }))?;

        let failed = checks
            .iter()
            .filter(|check| check.status.is_failure())
            .count();
        anyhow::ensure!(failed == 0, "{failed} check(s) failed");
        Ok(())
    }
}

/// Print the spec's JSON Schema.
///
/// The embedded on-chain types (`InterestRateStrategy`, `Fee`, `TimeBasedFee`,
/// `YieldWeights`) do not implement `JsonSchema`, so they appear as unconstrained
/// JSON. Everything the spec itself owns — structure, unknown-key rejection,
/// asset strings, source kinds, durations — is described precisely.
pub fn print_schema() -> anyhow::Result<()> {
    let schema = schemars::schema_for!(crate::spec::MarketSpec);
    crate::context::print_json(&schema)
}
