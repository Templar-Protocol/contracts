#![allow(clippy::unwrap_used)]
#![allow(dead_code)]

use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use near_workspaces::{network::Sandbox, Account, Worker};
use serde::Serialize;
use templar_manager::{commands::SignerArgs, CliContext};

/// Create a [`CliContext`] pointing at the sandbox RPC.
///
/// `workspace_path` is set to `CARGO_WORKSPACE_DIR` so that
/// [`FixedContractWasm::no_build()`] can load pre-built WASMs from
/// `target/near/`.
pub fn setup_ctx(worker: &Worker<Sandbox>) -> CliContext {
    CliContext {
        workspace_path: PathBuf::from(env!("CARGO_WORKSPACE_DIR")),
        transaction_url_prefix: String::new(),
        near: near_fetch::Client::new(&worker.rpc_addr()),
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
