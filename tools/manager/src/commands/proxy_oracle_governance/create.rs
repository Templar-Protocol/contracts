use std::path::PathBuf;

use anyhow::Context as _;
use clap::Args;
use near_account_id::AccountId;
use templar_common::Nanoseconds;
use templar_gateway_methods_spec::registry as registry_spec;
use templar_gateway_types::{Base64Bytes, NearToken};
use templar_proxy_oracle_near_governance_common::TtlConfig;

use super::{load_json_file, uniform_ttls};

/// Create (deploy-from-registry) a governance contract, building its
/// `new(proxy_oracle_id, admin_id, ttls)` init args from typed flags.
///
/// A governance contract administers exactly one proxy oracle and must be made
/// that oracle's owner after creation (propose-owner to it, then have it
/// execute an `admin-function-call own_accept_owner` proposal).
#[derive(Args, Debug)]
pub struct GovernanceCreate {
    #[arg(long, value_name = "ACCOUNT_ID")]
    registry_id: AccountId,
    #[arg(long, value_name = "NAME")]
    name: String,
    #[arg(long, value_name = "KEY")]
    version_key: String,
    /// The proxy oracle account this governance contract will administer
    #[arg(long, value_name = "ACCOUNT_ID")]
    proxy_oracle_id: AccountId,
    /// The account granted the Admin role
    #[arg(long, value_name = "ACCOUNT_ID")]
    admin_id: AccountId,
    /// Default proposal TTL (nanoseconds) applied to every operation kind
    #[arg(long, value_name = "NANOSECONDS", default_value = "0")]
    ttl_default: u64,
    /// Full TtlConfig JSON, overriding --ttl-default with per-operation TTLs
    #[arg(long, value_name = "PATH")]
    ttls_file: Option<PathBuf>,
    #[arg(long, value_name = "AMOUNT")]
    deposit: NearToken,
}

/// Init args for the governance contract's `new(proxy_oracle_id, admin_id, ttls)`.
#[derive(serde::Serialize)]
struct GovernanceInit {
    proxy_oracle_id: AccountId,
    admin_id: AccountId,
    ttls: TtlConfig,
}

impl GovernanceCreate {
    pub fn parse(self) -> anyhow::Result<registry_spec::Deploy> {
        let ttls = match self.ttls_file {
            Some(path) => load_json_file(&path).context("parse TtlConfig")?,
            None => uniform_ttls(Nanoseconds::from_ns(self.ttl_default)),
        };

        let init = GovernanceInit {
            proxy_oracle_id: self.proxy_oracle_id,
            admin_id: self.admin_id,
            ttls,
        };
        let init_args = serde_json::to_vec(&init).context("encode governance init args")?;

        Ok(registry_spec::Deploy {
            registry_id: self.registry_id,
            name: self.name,
            version_key: self.version_key,
            init_args: Base64Bytes(init_args),
            full_access_keys: None,
            deposit: self.deposit,
        })
    }
}
