//! The CLI execution context: a gateway [`Client`] plus the resolved signer and
//! presentation settings, and the small set of helpers every dispatch path uses
//! to read, write, report, and print. Keeping this in one place means the
//! context's whole surface lives together rather than scattered across dispatch.

use anyhow::Context as _;
use near_api::{NetworkConfig, SecretKey};
use serde::Serialize;
use std::io::Write as _;
use templar_gateway_client::{Client, NetworkConfigBuilder};
use templar_gateway_core::{DispatchRead, GatewayContext, PlanWrite};
use templar_gateway_methods_dispatch::Dispatch;
use templar_gateway_types::{
    common::WriteOperationResult, operation::ReceiptStatus, ManagedAccountId, MethodSpec,
    OperationStatus,
};

use crate::cli::Cli;
use crate::commands::signer::SignerArgs;

pub(crate) struct CliContext {
    /// An unsigned client for reads. Writes build a per-operation signing client
    /// from the command's own credentials (see [`CliContext::signing_client`]).
    pub(crate) client: Client,
    network: NetworkConfig,
    transaction_url_prefix: String,
}

impl CliContext {
    /// Build a single-signer client for `account_id` from `secret_key`. Each
    /// write signs with credentials carried by its own command, and teardown
    /// flows (e.g. `registry clear-deployments`) sign many discovered accounts
    /// with one authorized key.
    pub(crate) fn signing_client(
        &self,
        account_id: impl Into<ManagedAccountId>,
        secret_key: SecretKey,
    ) -> anyhow::Result<Client> {
        Client::builder(self.network.clone())
            .secret_key(account_id, secret_key)?
            .build()
            .context("build signing client")
    }

    /// Dispatch a read and print its JSON result.
    pub(crate) async fn read<S>(&self, request: S) -> anyhow::Result<()>
    where
        S: MethodSpec,
        Dispatch: DispatchRead<S, GatewayContext>,
    {
        let output = self.client.read(request).await?;
        print_json(&output)
    }

    /// Execute a write signed by `signer`, report the tx link, print the JSON
    /// result, then fail unless the operation succeeded on chain.
    pub(crate) async fn write<S>(&self, signer: SignerArgs, body: S) -> anyhow::Result<()>
    where
        S: MethodSpec<Output = WriteOperationResult>,
        Dispatch: PlanWrite<S, GatewayContext>,
    {
        let (account_id, secret_key) = signer.resolve()?;
        let client = self.signing_client(account_id.clone(), secret_key)?;
        let output = client.execute_as(account_id, body).await?;
        self.report_tx(&output);
        // Print the machine-readable result before checking status, so a reverted
        // operation still emits its JSON on stdout.
        print_json(&output)?;
        check_operation_status(&output)
    }

    /// Log the explorer link for a completed write to stderr (the JSON result,
    /// carrying every step's hash, still goes to stdout).
    pub(crate) fn report_tx(&self, result: &WriteOperationResult) {
        if let Some(tx_hash) = result.operation.latest_tx_hash() {
            tracing::info!("tx: {}{}", self.transaction_url_prefix, tx_hash);
        }
    }

    /// Report an intermediate write's tx link, then fail if it reverted — the
    /// multi-step counterpart to [`write`](CliContext::write) (which also prints
    /// the JSON), so a reverted step in a teardown/proposal flow aborts instead of
    /// being treated as success.
    pub(crate) fn report_checked(&self, result: &WriteOperationResult) -> anyhow::Result<()> {
        self.report_tx(result);
        check_operation_status(result)
    }
}

/// Succeed only when a submitted write reached a `Succeeded` terminal status.
/// A `Failed` operation (RPC round-trip fine, but reverted on chain) errors with
/// a concise stderr diagnostic naming the failed receipt(s); a non-terminal
/// `Pending`/`InProgress` operation also errors, because the CLI's transient
/// in-memory store has no later resume, so an unconfirmed outcome must not read
/// as success. Either way the process exits non-zero — letting driver scripts
/// stop instead of continuing past a failed or unknown step. Callers print the
/// machine-readable JSON first.
pub(crate) fn check_operation_status(result: &WriteOperationResult) -> anyhow::Result<()> {
    let operation = &result.operation;
    match operation.status {
        OperationStatus::Succeeded => Ok(()),
        OperationStatus::Failed => {
            let failed_contracts: Vec<&str> = operation
                .final_outcome()
                .map(|outcome| {
                    outcome
                        .receipts
                        .iter()
                        .filter(|receipt| receipt.status == ReceiptStatus::Failed)
                        .map(|receipt| receipt.contract_id.as_str())
                        .collect()
                })
                .unwrap_or_default();

            if failed_contracts.is_empty() {
                anyhow::bail!("operation {} failed on chain", operation.id.0);
            }
            anyhow::bail!(
                "operation {} failed on chain; reverted receipt(s) on: {}",
                operation.id.0,
                failed_contracts.join(", ")
            )
        }
        status @ (OperationStatus::Pending | OperationStatus::InProgress) => anyhow::bail!(
            "operation {} did not reach a terminal state (status: {status:?}); \
             its on-chain outcome is unknown",
            operation.id.0,
        ),
    }
}

/// Serialize `output` as a single line of JSON to stdout — the machine-readable
/// result channel (diagnostics go to stderr via tracing).
pub(crate) fn print_json(output: &impl Serialize) -> anyhow::Result<()> {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    serde_json::to_writer(&mut lock, output)?;
    writeln!(lock)?;
    Ok(())
}

/// Build the execution context from parsed CLI args: the network and an unsigned
/// gateway client (backed by a transient in-memory operation store) for reads.
/// Writes sign per-operation with credentials carried by their own command.
pub(crate) fn build_context(cli: &Cli) -> anyhow::Result<CliContext> {
    let network = NetworkConfigBuilder::new(cli.network)
        .rpc_url(cli.rpc_url.as_deref())
        .context("invalid RPC URL")?
        .api_key(cli.rpc_api_key.clone())
        .build();

    Ok(CliContext {
        client: Client::read_only(network.clone())?,
        network,
        transaction_url_prefix: cli.transaction_url_prefix(),
    })
}

#[cfg(test)]
mod tests {
    use super::check_operation_status;
    use serde_json::json;
    use templar_gateway_types::common::WriteOperationResult;

    /// A single-step write result with the given operation status and a reverted
    /// step whose first receipt has `receipt_status`.
    fn result(status: &str, receipt_status: &str) -> WriteOperationResult {
        serde_json::from_value(json!({
            "operation": {
                "id": "op-1",
                "signer_account_id": "signer.testnet",
                "status": status,
                "steps": [{
                    "index": 0,
                    "status": { "Reverted": {
                        "tx_hash": "3DeTHGEZzdG5Vpj5b972u45DSRKTBsV87a1eGLCcFQY2",
                        "outcome": {
                            "tokens_burnt": "120081144700600000000",
                            "total_gas_burnt": "1647176572006",
                            "receipts": [
                                {"contract_id": "po-market.signer.testnet", "status": receipt_status, "logs": []},
                                {"contract_id": "signer.testnet", "status": "Succeeded", "logs": []}
                            ],
                            "return_value": null
                        }
                    }}
                }]
            }
        }))
        .expect("valid WriteOperationResult json")
    }

    #[test]
    fn failed_operation_errors_and_names_reverted_receipt() {
        let error = check_operation_status(&result("Failed", "Failed"))
            .expect_err("a failed operation must map to an error (non-zero exit)");
        let message = error.to_string();
        assert!(message.contains("op-1"), "message: {message}");
        assert!(
            message.contains("po-market.signer.testnet"),
            "message should name the reverted receipt: {message}"
        );
    }

    #[test]
    fn succeeded_operation_is_ok() {
        check_operation_status(&result("Succeeded", "Succeeded"))
            .expect("a succeeded operation must not error");
    }

    #[test]
    fn non_terminal_operation_errors() {
        // A non-terminal status is an unknown outcome, not a success: with the
        // CLI's transient store there is no later resume to confirm it.
        for status in ["Pending", "InProgress"] {
            let error = check_operation_status(&result(status, "Succeeded"))
                .expect_err("a non-terminal operation must error");
            assert!(
                error.to_string().contains("did not reach a terminal state"),
                "status {status}: {error}"
            );
        }
    }
}
