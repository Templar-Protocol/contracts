//! Wall-clock benchmarks for the sandbox harness primitives.
//!
//! Run through `just bench-sandbox` (which drives `bin/sandbox-bench.rs`). This
//! is a binary, not a test, so no test-gate filter picks it up.
//!
//! It answers where a node-backed test's wall time goes — the block-latency
//! floor, per-transaction and per-patch costs, and the fixture setup built from
//! them — so a harness or node-config change can be measured, not guessed.
//!
//! Every measurement runs on a dedicated node ([`SandboxHarness::start_owned`]),
//! never a pooled one, so a concurrent test cannot skew it.

use std::{
    future::Future,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;
use near_api::{AccountId, Contract, NetworkConfig, Signer};
use near_token::NearToken;

use crate::{
    sandbox::{deploy_contract, test_signer},
    SandboxHarness, TEST_FINALITY_POLICY,
};

/// Rounds each measurement averages over, overridable with `BENCH_ROUNDS`.
fn rounds() -> u32 {
    std::env::var("BENCH_ROUNDS")
        .ok()
        .and_then(|rounds| rounds.parse().ok())
        .unwrap_or(5)
}

/// Accounts minted per batched-patch measurement.
const BATCH_SIZE: usize = 8;

/// Blocks jumped when measuring `fast_forward`'s simulated chain-time advance.
const FAST_FORWARD_BLOCKS: u64 = 100;

pub async fn run() -> Result<()> {
    let mut report = Report::new(rounds());

    let start = Instant::now();
    let harness = SandboxHarness::start_owned().await?;
    report.push(
        "harness start (node boot + accounts + FT deploy)",
        start.elapsed(),
    );

    bench_account_creation(&harness, &mut report).await?;
    bench_transactions(&harness, &mut report).await?;
    bench_fixtures(&harness, &mut report).await?;

    report.print();
    report_fast_forward(&harness).await
}

/// The three ways the harness can mint an account, including the batched patch.
async fn bench_account_creation(harness: &SandboxHarness, report: &mut Report) -> Result<()> {
    report
        .measure("create_account (patch)", || async {
            harness
                .create_account("bench", NearToken::from_near(100))
                .await?;
            Ok(())
        })
        .await?;
    report
        .measure("create_account_via_tx", || async {
            harness
                .create_account_via_tx("bench-tx", NearToken::from_near(10))
                .await?;
            Ok(())
        })
        .await?;
    let batch_labels = vec![("bench-batch", NearToken::from_near(10)); BATCH_SIZE];
    report
        .measure(
            &format!("create {BATCH_SIZE} accounts (one patch)"),
            || async {
                harness.create_accounts(&batch_labels).await?;
                Ok(())
            },
        )
        .await
}

/// The transaction floor, and what the WASM payload adds on top of it.
async fn bench_transactions(harness: &SandboxHarness, report: &mut Report) -> Result<()> {
    // The floor every node-backed interaction pays: one transaction, minimal
    // payload, waiting only for optimistic execution.
    let (ft_id, _) = harness
        .create_account("bench-ft", NearToken::from_near(100))
        .await?;
    deploy_contract(
        &harness.network,
        ft_id.clone(),
        test_signer(),
        crate::wasm::ft().await.to_vec(),
        "new",
        serde_json::json!({ "name": "Bench FT", "symbol": "BFT" }),
    )
    .await?;
    report
        .measure("minimal fn-call tx", || async {
            harness
                .call_contract(
                    &ft_id,
                    "increment",
                    serde_json::json!({}),
                    &ft_id,
                    100,
                    NearToken::from_yoctonear(0),
                )
                .await
        })
        .await?;

    for (name, code) in [
        ("mock_ft", crate::wasm::ft().await.to_vec()),
        ("market", crate::wasm::market().await.to_vec()),
        ("vault", crate::wasm::vault().await.to_vec()),
    ] {
        let label = format!("create + deploy: {name} ({} KB)", code.len() / 1024);
        report
            .measure(&label, || async {
                let (id, signer) = harness
                    .create_account("bench-deploy", NearToken::from_near(100))
                    .await?;
                deploy_code(&harness.network, id, signer, code.clone()).await
            })
            .await?;
    }
    Ok(())
}

/// Setup as tests actually pay for it: several deploys, then the real fixtures.
async fn bench_fixtures(harness: &SandboxHarness, report: &mut Report) -> Result<()> {
    let codes = [
        crate::wasm::ft().await.to_vec(),
        crate::wasm::ft().await.to_vec(),
        crate::wasm::mock_oracle().await.to_vec(),
        crate::wasm::market().await.to_vec(),
    ];
    report
        .measure("4 x (create + deploy), serial", || async {
            for code in &codes {
                let (id, signer) = harness
                    .create_account("bench-seq", NearToken::from_near(100))
                    .await?;
                deploy_code(&harness.network, id, signer, code.clone()).await?;
            }
            Ok(())
        })
        .await?;
    report
        .measure("batched create + 4 deploys concurrent", || async {
            let accounts = harness
                .create_accounts(&[("bench-par", NearToken::from_near(100)); 4])
                .await?;
            let deploys = accounts
                .into_iter()
                .zip(codes.iter())
                .map(|((id, signer), code)| {
                    deploy_code(&harness.network, id, signer, code.clone())
                });
            for result in futures::future::join_all(deploys).await {
                result?;
            }
            Ok(())
        })
        .await?;

    report
        .measure("deploy_market fixture (2 FT + oracle + market)", || async {
            harness.deploy_market().await?;
            Ok(())
        })
        .await?;
    report
        .measure("deploy_vault_with_market fixture", || async {
            harness.deploy_vault_with_market().await?;
            Ok(())
        })
        .await
}

/// `sandbox_fast_forward` advances the chain clock by
/// `blocks × avg(min_block_production_delay, max_block_production_delay)` (see
/// nearcore's `Client::sandbox_delta_time`), so a change to either delay changes
/// how much simulated time a `fast_forward` buys — which is what time-sensitive
/// tests (snapshots, interest, TTLs) actually assert on. Print it so any config
/// change can be checked against the ~310ms/block the min/max average is
/// deliberately held at (unchanged whatever the real cadence).
async fn report_fast_forward(harness: &SandboxHarness) -> Result<()> {
    let before = harness.chain_timestamp().await?;
    harness.fast_forward(FAST_FORWARD_BLOCKS).await?;
    let after = harness.chain_timestamp().await?;
    let advance = Duration::from_nanos(after.as_ns().saturating_sub(before.as_ns()));
    let per_block = advance / u32::try_from(FAST_FORWARD_BLOCKS).unwrap_or(u32::MAX);
    println!();
    println!(
        "fast_forward({FAST_FORWARD_BLOCKS}) advanced chain time by {advance:?} ({per_block:?}/block)"
    );
    Ok(())
}

async fn deploy_code(
    network: &NetworkConfig,
    account_id: AccountId,
    signer: Arc<Signer>,
    code: Vec<u8>,
) -> Result<()> {
    Contract::deploy(account_id)
        .use_code(code)
        .without_init_call()
        .with_signer(signer)
        .wait_until(TEST_FINALITY_POLICY.transaction_status())
        .send_to(network)
        .await?
        .assert_success();
    Ok(())
}

struct Report {
    rows: Vec<(String, Duration)>,
    rounds: u32,
}

impl Report {
    fn new(rounds: u32) -> Self {
        Self {
            rows: Vec::new(),
            rounds: rounds.max(1),
        }
    }

    /// Record an already-measured one-shot duration (e.g. harness start).
    fn push(&mut self, label: &str, elapsed: Duration) {
        println!("  measured {label}: {elapsed:?}");
        self.rows.push((label.to_owned(), elapsed));
    }

    /// Run `op` [`rounds`](Self::rounds) times and record its mean wall time.
    async fn measure<F, Fut>(&mut self, label: &str, mut op: F) -> Result<()>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let mut total = Duration::ZERO;
        for _ in 0..self.rounds {
            let start = Instant::now();
            op().await?;
            total += start.elapsed();
        }
        self.push(label, total / self.rounds);
        Ok(())
    }

    fn print(&self) {
        let width = self
            .rows
            .iter()
            .map(|(label, _)| label.len())
            .max()
            .unwrap_or(0);
        println!();
        println!("{:width$}   mean", "operation");
        println!("{}", "-".repeat(width + 12));
        for (label, elapsed) in &self.rows {
            println!(
                "{label:width$}   {:>7.1} ms",
                elapsed.as_secs_f64() * 1000.0
            );
        }
    }
}
