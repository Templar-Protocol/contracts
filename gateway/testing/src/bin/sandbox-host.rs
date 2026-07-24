//! Out-of-band `neard` host for attach-mode tests.
//!
//! Launches one sandbox `neard` via near-sandbox (so version/genesis match the
//! harness), reports its RPC url (stdout and, if an arg is given, that file),
//! and stays alive until terminated — at which point the `Sandbox` drops and
//! `just test-sandbox` starts this in the background through
//! `script/sandbox-up.sh` and exports `NEAR_SANDBOX_RPC_URL`, so many test
//! processes share one node instead of each booting its own.

use std::time::Duration;

use anyhow::{Context, Result};
use near_sandbox::Sandbox;
use templar_gateway_testing::sandbox::sandbox_config;
use tokio::signal::unix::{signal, SignalKind};

/// How often the node is asked whether it is still serving.
const HEALTH_INTERVAL: Duration = Duration::from_secs(2);

/// Consecutive failed probes before the node is declared dead. A loaded node can
/// miss a probe or two under CPU pressure, so only a sustained outage counts.
const HEALTH_FAILURES_BEFORE_DEAD: u32 = 5;

const HEALTH_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> Result<()> {
    let sandbox = Sandbox::start_sandbox_with_config(sandbox_config())
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
    let health_client = reqwest::Client::builder()
        .timeout(HEALTH_TIMEOUT)
        .build()
        .context("failed to build health-probe client")?;
    let outcome = tokio::select! {
        _ = terminate.recv() => Ok(()),
        _ = interrupt.recv() => Ok(()),
        () = supervise(&health_client, &url) => Err(anyhow::anyhow!(
            "sandbox node at {url} stopped responding — the node died or hung. \
             Tests routed to it would otherwise fail with confusing transport errors."
        )),
    };

    drop(sandbox);
    outcome
}

/// Return once the node has stopped serving RPC.
///
/// near-sandbox keeps its child process private, so health is judged from the
/// outside. Without this the host would sit blocked on a signal while its node
/// was dead, leaving the pool advertising a slot that answers nothing — every
/// test landing there fails with a bare transport error that looks like
/// flakiness rather than a downed node.
async fn supervise(client: &reqwest::Client, url: &str) {
    let mut consecutive_failures = 0;

    loop {
        tokio::time::sleep(HEALTH_INTERVAL).await;

        if is_serving(client, url).await {
            consecutive_failures = 0;
            continue;
        }

        consecutive_failures += 1;
        eprintln!(
            "sandbox node at {url} failed health probe \
             {consecutive_failures}/{HEALTH_FAILURES_BEFORE_DEAD}"
        );
        if consecutive_failures >= HEALTH_FAILURES_BEFORE_DEAD {
            return;
        }
    }
}

async fn is_serving(client: &reqwest::Client, url: &str) -> bool {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "health",
        "method": "status",
        "params": [],
    });
    client
        .post(url)
        .json(&request)
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}
