#![allow(clippy::unwrap_used)]
#![allow(dead_code)]

use std::path::PathBuf;

use near_workspaces::{network::Sandbox, Account, Worker};
use templar_manager::{commands::SignerArgs, CliContext};

/// Create a [`CliContext`] pointing at the sandbox RPC.
///
/// `workspace_path` is set to `CARGO_WORKSPACE_DIR` so that
/// [`FixedContractWasm::no_build()`] can load pre-built WASMs from
/// `target/near/`.
pub fn setup_ctx(worker: &Worker<Sandbox>) -> CliContext {
    CliContext::new(
        &worker.rpc_addr(),
        PathBuf::from(env!("CARGO_WORKSPACE_DIR")),
    )
}

/// Build [`SignerArgs`] from a sandbox [`Account`].
pub fn signer_args(account: &Account) -> SignerArgs {
    SignerArgs::new(
        account.id().clone(),
        account.secret_key().to_string().parse().unwrap(),
    )
}
