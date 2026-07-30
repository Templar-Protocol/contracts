//! Local operations on a deployment spec. Arguments only — the preflight that
//! fulfils `check` lives in [`crate::dispatch::preflight`], since it reads the
//! chain.

use std::path::PathBuf;

use clap::{Args, Subcommand};

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum SpecNs {
    /// Resolve a spec's `extends` chain and run its preflight checks.
    Check(Check),
    /// Emit the spec's JSON Schema, for editor completion and validation.
    PrintSchema,
}

#[derive(Args, Debug)]
pub struct Check {
    /// Path to the market spec.
    pub(crate) path: PathBuf,

    /// Skip every check that reads the chain. The remaining checks need no
    /// network, so this is the form to run in CI.
    #[arg(long)]
    pub(crate) offline: bool,

    /// Accept a `decimals` override that disagrees with the token's metadata.
    /// Only correct when the spec is right and the token is lying.
    #[arg(long)]
    pub(crate) accept_decimals_mismatch: bool,
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
