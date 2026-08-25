mod plan;

pub use plan::{Apply, Plan};

use clap::Subcommand;

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
pub enum PatchNs {
    /// Resolve a patch spec into a reviewable atomic transaction.
    Plan(Plan),
    /// Re-derive and submit a patch plan.
    Apply(Apply),
    /// List the readable types accepted by `borsh` byte expressions.
    Codecs,
    /// Emit the patch spec JSON Schema.
    PrintSchema,
}

pub fn print_schema() -> anyhow::Result<()> {
    crate::context::print_json(&schemars::schema_for!(crate::spec::patch::PatchSpec))
}

pub fn print_codecs() -> anyhow::Result<()> {
    crate::context::print_json(&crate::spec::patch::codec_names().collect::<Vec<_>>())
}
