#![allow(clippy::unwrap_used)]
#![allow(dead_code)]

use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use near_workspaces::{network::Sandbox, Account, Worker};
use serde::Serialize;
use templar_manager::{
    util::{ContractLoader, SignerArgs},
    CliContext,
};

/// Create a [`CliContext`] pointing at the sandbox RPC.
///
pub fn setup_ctx(worker: &Worker<Sandbox>) -> CliContext {
    CliContext {
        transaction_url_prefix: String::new(),
        near: near_fetch::Client::new(&worker.rpc_addr()),
    }
}

pub fn workspace_path() -> PathBuf {
    PathBuf::from(env!("CARGO_WORKSPACE_DIR"))
}

pub fn no_build_loader() -> ContractLoader {
    ContractLoader {
        no_build: true,
        workspace_path: workspace_path(),
    }
}

/// Build [`SignerArgs`] from a sandbox [`Account`].
pub fn signer_args(account: &Account) -> SignerArgs {
    SignerArgs::new(
        account.id().clone(),
        account.secret_key().to_string().parse().unwrap(),
    )
}

pub fn write_json_file<T: Serialize>(prefix: &str, value: &T) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("templar-manager-{prefix}-{unique}.json"));
    std::fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
    path
}
