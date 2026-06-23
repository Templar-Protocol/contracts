//! Out-of-band `neard` host for attach-mode tests.
//!
//! Launches one sandbox `neard` via near-sandbox (so version/genesis match the
//! harness), reports its RPC url (stdout and, if an arg is given, that file),
//! and stays alive until terminated — at which point the `Sandbox` drops and
//! `neard` is killed. A nextest setup script (or `script/sandbox-up.sh`) runs
//! this in the background and exports `NEAR_SANDBOX_RPC_URL`, so many test
//! processes share the one node instead of each booting their own.

use anyhow::{Context, Result};
use near_sandbox::Sandbox;
use tokio::signal::unix::{signal, SignalKind};

#[tokio::main]
async fn main() -> Result<()> {
    let sandbox = Sandbox::start_sandbox()
        .await
        .context("failed to start out-of-band sandbox")?;
    let url = sandbox.rpc_addr.clone();

    if let Some(path) = std::env::args().nth(1) {
        std::fs::write(&path, &url)
            .with_context(|| format!("failed to write rpc url to {path}"))?;
    }
    println!("{url}");

    // Keep the node alive until asked to stop, then let `sandbox` drop (which
    // kills the child `neard`).
    let mut terminate = signal(SignalKind::terminate()).context("failed to hook SIGTERM")?;
    let mut interrupt = signal(SignalKind::interrupt()).context("failed to hook SIGINT")?;
    tokio::select! {
        _ = terminate.recv() => {}
        _ = interrupt.recv() => {}
    }

    drop(sandbox);
    Ok(())
}
