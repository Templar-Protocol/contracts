use std::time::Duration;

use near_sdk::AccountId;
use templar_gateway_client::Client as GatewayClient;
use templar_gateway_methods_spec::tx;
use templar_gateway_types::common::TxExecutionStatus;
use tokio::{select, sync::watch};

use crate::{
    app::to_gateway_hash,
    client::database::{Database, PendingTransaction},
};

/// Periodically settle any pending transactions whose on-chain outcome the
/// relayer never recorded (e.g. a relay interrupted between submission and
/// finalization), reconciling each account's allowance against the real cost.
#[tracing::instrument(skip_all, fields(batch_size = %batch_size, delay = ?delay))]
pub async fn start(
    database: Database,
    gateway: GatewayClient,
    signer_account_ids: Vec<AccountId>,
    batch_size: u32,
    delay: Duration,
    kill: watch::Sender<()>,
) {
    tracing::info!("Starting broom service");
    let batch_size = i64::from(batch_size);

    let mut interval = tokio::time::interval(delay);
    let mut on_kill = kill.subscribe();

    loop {
        select! {
            _ = on_kill.changed() => {
                tracing::debug!("Received kill notification.");
                break;
            }
            _ = interval.tick() => {
                let Ok(pending_transactions) = database.get_pending_transactions(batch_size).await else {
                    tracing::warn!("Failed to fetch pending transactions.");
                    continue;
                };

                tracing::debug!(
                    "Broom processing {} pending transactions...",
                    pending_transactions.len(),
                );

                for pending in pending_transactions {
                    if let Err(e) = reconcile(&database, &gateway, &signer_account_ids, &pending).await {
                        tracing::warn!(
                            account_id = %pending.account_id,
                            transaction_hash = %pending.transaction_hash,
                            "Broom failed to reconcile pending transaction: {e}",
                        );
                    }
                }
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum ReconcileError {
    #[error("transaction hash is not valid base58")]
    InvalidHash,
    #[error("could not fetch transaction status from any candidate signer")]
    StatusUnavailable,
    #[error("finalize error: {0}")]
    Finalize(#[from] crate::client::database::error::RecordTransactionError),
}

/// Look up a pending transaction's outcome through the gateway and settle the
/// account's allowance against the true gas cost.
async fn reconcile(
    database: &Database,
    gateway: &GatewayClient,
    signer_account_ids: &[AccountId],
    pending: &PendingTransaction,
) -> Result<(), ReconcileError> {
    let Some(tx_hash) = to_gateway_hash(&pending.transaction_hash) else {
        return Err(ReconcileError::InvalidHash);
    };

    // The row doesn't record which relayer-controlled account signed, so try
    // each candidate (relay, UA) until one resolves the transaction.
    let mut result = None;
    for signer_account_id in signer_account_ids {
        if let Ok(status) = gateway
            .read(tx::Get {
                tx_hash,
                sender_account_id: signer_account_id.clone(),
                wait_until: Some(TxExecutionStatus::Executed),
                encoding: tx::ValueEncoding::Json,
            })
            .await
        {
            result = Some(status);
            break;
        }
    }

    let Some(status) = result else {
        return Err(ReconcileError::StatusUnavailable);
    };

    // A still-pending transaction hasn't reached a final outcome yet; leave it
    // for a later sweep.
    if status.status == tx::Status::Pending {
        return Ok(());
    }

    let succeeded = status.status == tx::Status::Succeeded;
    database
        .finalize_pending_transaction(
            &pending.account_id,
            pending.operation_key,
            status.tokens_burnt,
            succeeded,
        )
        .await?;

    Ok(())
}
