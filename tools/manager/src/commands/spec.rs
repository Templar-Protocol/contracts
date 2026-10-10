//! Local operations on a deployment spec. Arguments only — the preflight that
//! fulfills `check` lives in [`crate::dispatch::preflight`], since it reads the
//! chain.

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};
use templar_gateway_oracle_updates_dispatch::{LazerSourceArgs, RedStoneSourceArgs};

#[allow(
    clippy::large_enum_variant,
    reason = "inline command arguments avoid an extra heap allocation on every spec check"
)]
#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum SpecNs {
    /// Resolve a spec's `extends` chain and run its preflight checks.
    Check(Check),
    /// Emit the spec's JSON Schema, for editor completion and validation.
    PrintSchema,
}

/// Input used to evaluate oracle prices during online preflight.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum PricesFrom {
    /// Read verified Lazer/RedStone provider prices; other sources stay on chain.
    #[default]
    Provider,
    /// Read stored adapter prices from chain.
    Chain,
}

/// Invocation-local provider configuration, never persisted in a spec or plan.
#[derive(Args, Debug)]
pub struct PreflightPriceArgs {
    /// Select verified provider prices or stored chain prices for preflight.
    /// Provider failures never fall back to stored prices.
    #[arg(long, value_enum, default_value_t = PricesFrom::Provider)]
    pub(crate) prices_from: PricesFrom,

    #[command(flatten)]
    pub(crate) lazer: LazerSourceArgs,

    #[command(flatten)]
    pub(crate) redstone: RedStoneSourceArgs,
}

#[derive(Args, Debug)]
pub struct Check {
    /// Path to the market spec.
    pub(crate) path: PathBuf,

    #[command(flatten)]
    pub(crate) prices: PreflightPriceArgs,

    /// Skip every check that reads the chain. The remaining checks need no
    /// network, so this is the form to run in CI.
    #[arg(long)]
    pub(crate) offline: bool,

    /// Accept a `decimals` override that disagrees with the token's metadata.
    /// Only correct when the spec is right and the token is lying.
    #[arg(long)]
    pub(crate) accept_decimals_mismatch: bool,

    /// Ignore a named check. Every other check still runs — this suppresses one
    /// verdict, not the preflight, and the report records what it suppressed.
    #[arg(long = "skip-check", value_name = "CHECK_ID")]
    #[allow(
        clippy::struct_field_names,
        reason = "the flag is `--skip-check` on every command that has it; \
                  renaming it here to please the lint would rename the flag"
    )]
    pub(crate) skip_check: Vec<String>,
}

/// Print the spec's JSON Schema.
///
/// The embedded on-chain types (`InterestRateStrategy`, `YieldWeights`) do not
/// implement `JsonSchema`, so they appear as unconstrained JSON. Everything the
/// spec itself owns — structure, unknown-key rejection, asset strings, source
/// kinds, durations, amounts — is described precisely.
pub fn print_schema() -> anyhow::Result<()> {
    // The *file* shape, which is what an author writes. `MarketSpec` is the
    // parsed form and states the same fields under a different arrangement.
    let schema = schemars::schema_for!(crate::spec::RawMarketSpec);
    crate::context::print_json(&schema)
}
