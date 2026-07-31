use std::path::PathBuf;

use anyhow::Context as _;
use clap::Args;
use near_account_id::AccountId;
use templar_common::Nanoseconds;
use templar_gateway_methods_spec::registry as registry_spec;
use templar_proxy_oracle_near_governance_common::TtlConfig;

use super::{load_json_file, uniform_ttls};
use crate::commands::deploy_common::DeployCommonArgs;
use crate::commands::duration::parse_duration;
use crate::commands::signer::SignerArgs;

/// Create (deploy-from-registry) a governance contract, building its
/// `new(proxy_oracle_id, admin_id, ttls)` init args from typed flags.
///
/// A governance contract administers exactly one proxy oracle and must be that
/// oracle's owner. Prefer naming it as the oracle's `owner_id` at deploy time;
/// an oracle that is already owned by someone else has to hand ownership over
/// instead (propose-owner to it, then have it execute an `admin-function-call
/// own_accept_owner` proposal).
#[derive(Args, Debug)]
pub struct GovernanceCreate {
    #[command(flatten)]
    common: DeployCommonArgs,
    /// The proxy oracle account this governance contract will administer
    #[arg(long, value_name = "ACCOUNT_ID")]
    proxy_oracle_id: AccountId,
    /// The account granted the Admin role
    #[arg(long, value_name = "ACCOUNT_ID")]
    admin_id: AccountId,
    /// Default proposal TTL applied to every operation kind (e.g. `10s`, `100ns`).
    #[arg(long, value_name = "DURATION", default_value = "0ns", value_parser = parse_duration, conflicts_with = "ttls_file")]
    ttl_default: Nanoseconds,
    /// Full TtlConfig JSON, overriding --ttl-default with per-operation TTLs
    #[arg(long, value_name = "PATH")]
    ttls_file: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

/// Init args for the governance contract's `new(proxy_oracle_id, admin_id, ttls)`.
#[derive(serde::Serialize)]
pub struct GovernanceInit {
    pub proxy_oracle_id: AccountId,
    pub admin_id: AccountId,
    pub ttls: TtlConfig,
}

impl GovernanceCreate {
    pub fn try_into_spec(self) -> anyhow::Result<registry_spec::Deploy> {
        let ttls = match self.ttls_file {
            Some(path) => load_json_file(&path).context("parse TtlConfig")?,
            None => uniform_ttls(self.ttl_default),
        };

        let init = GovernanceInit {
            proxy_oracle_id: self.proxy_oracle_id,
            admin_id: self.admin_id,
            ttls,
        };
        let init_args = serde_json::to_vec(&init).context("encode governance init args")?;

        let signer = self.signer;
        self.common.into_deploy(|| signer.public_key(), init_args)
    }
}
