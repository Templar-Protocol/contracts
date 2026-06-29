use std::time::Duration;

use templar_gateway_client::Client as GatewayClient;
use templar_gateway_core::GatewayError;
use templar_gateway_methods_spec::tx;
use templar_gateway_types::{common::TxExecutionStatus, IdempotencyKey};
use tokio::{select, sync::watch};

use crate::{
    app::from_gateway_hash,
    client::database::{error::RecordTransactionError, Database, PendingTransaction},
};

/// A pending row is left alone until it is at least this old, so the broom never
/// acts on one still mid-submission (whose gateway operation may not be
/// persisted yet) and mistakes it for abandoned.
const MIN_PENDING_AGE: Duration = Duration::from_secs(60);

/// Periodically settle pending transactions whose on-chain outcome the relayer
/// never recorded (e.g. a relay interrupted between submission and
/// finalization), reconciling each account's allowance against the real cost.
#[tracing::instrument(skip_all, fields(batch_size = %batch_size, delay = ?delay))]
pub async fn start(
    database: Database,
    gateway: GatewayClient,
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
                let Ok(pending_transactions) =
                    database.get_pending_transactions(batch_size, MIN_PENDING_AGE).await
                else {
                    tracing::warn!("Failed to fetch pending transactions.");
                    continue;
                };

                tracing::debug!(
                    "Broom processing {} pending transactions...",
                    pending_transactions.len(),
                );

                for pending in pending_transactions {
                    if let Err(e) = reconcile_pending(&database, &gateway, &pending).await {
                        tracing::warn!(
                            account_id = %pending.account_id,
                            operation_key = %pending.operation_key,
                            "Broom failed to reconcile pending transaction: {e}",
                        );
                    }
                }
            }
        }
    }
}

/// How a pending row should be settled, once resolved against the gateway.
enum Resolution {
    /// The operation reached a final on-chain outcome; charge the actual cost.
    Settle {
        transaction_hash: near_primitives::hash::CryptoHash,
        tokens_burnt: near_sdk::NearToken,
        succeeded: bool,
    },
    /// The operation never reached the gateway; release the pending slot.
    Release,
    /// Not yet resolvable (still in flight); defer it to a later sweep.
    Defer,
}

#[derive(Debug, thiserror::Error)]
enum ReconcileError {
    #[error("gateway error: {0}")]
    Gateway(#[from] GatewayError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("finalize error: {0}")]
    Finalize(#[from] RecordTransactionError),
}

async fn reconcile_pending(
    database: &Database,
    gateway: &GatewayClient,
    pending: &PendingTransaction,
) -> Result<(), ReconcileError> {
    match classify(gateway, pending).await? {
        Resolution::Settle {
            transaction_hash,
            tokens_burnt,
            succeeded,
        } => {
            database
                .finalize_pending_transaction(
                    &pending.account_id,
                    pending.operation_key,
                    transaction_hash,
                    tokens_burnt,
                    succeeded,
                )
                .await?;
        }
        Resolution::Release => {
            database
                .remove_pending_transaction(&pending.account_id)
                .await?;
        }
        Resolution::Defer => {}
    }
    Ok(())
}

/// Classify a pending row against the gateway operation store — the source of
/// truth for an interrupted operation's outcome (it records the signer, the
/// on-chain hash, and survives relayer restarts) — into how it should be settled.
async fn classify(
    gateway: &GatewayClient,
    pending: &PendingTransaction,
) -> Result<Resolution, ReconcileError> {
    let key = IdempotencyKey(pending.operation_key.to_string());

    let Some(operation) = gateway.operation_by_idempotency_key(&key).await? else {
        // The operation never reached the gateway; the lock can be released.
        return Ok(Resolution::Release);
    };

    let Some(tx_hash) = operation.latest_tx_hash() else {
        // Submitted but no on-chain hash yet; reconcile on a later sweep.
        return Ok(Resolution::Defer);
    };

    let status = gateway
        .read(tx::Get {
            tx_hash,
            sender_account_id: operation.signer_account_id.0,
            wait_until: Some(TxExecutionStatus::Executed),
            encoding: tx::ValueEncoding::Json,
        })
        .await?;

    if status.status == tx::Status::Pending {
        return Ok(Resolution::Defer);
    }

    Ok(Resolution::Settle {
        transaction_hash: from_gateway_hash(&tx_hash),
        tokens_burnt: status.tokens_burnt,
        succeeded: status.status == tx::Status::Succeeded,
    })
}
