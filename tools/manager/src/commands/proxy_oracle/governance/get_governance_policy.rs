use clap::Args;

use crate::resolve::GovernanceTarget;

/// `get-governance-policy` carries only the target selector; the dispatcher resolves it and builds the
/// gateway spec (which needs nothing else).
#[derive(Args, Debug)]
pub struct GetGovernancePolicy {
    #[command(flatten)]
    pub(crate) target: GovernanceTarget,
}
