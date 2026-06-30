use std::time::Duration;

use near_sdk::NearToken;
use templar_gateway_client::Client as GatewayClient;
use templar_gateway_core::GatewayError;
use templar_gateway_types::{IdempotencyKey, OperationRecord, OperationStatus};
use tokio::{select, sync::watch};

use crate::client::database::{Database, PendingCharge};

/// A charge is left alone until it is at least this old, so the broom never acts
/// on one still mid-submission (whose gateway operation may not be persisted
/// yet) and mistakes it for abandoned.
const MIN_PENDING_AGE: Duration = Duration::from_secs(60);

/// Periodically settle charges the relayer locked but never settled itself
/// (e.g. a request interrupted between submission and settlement), reconciling
/// each account's allowance against the gateway's recorded cost.
///
/// A sweep only reads the gateway's record — never the chain. Driving an
/// interrupted operation to a terminal outcome (crash recovery) happens once at
/// startup instead (`App::new` calls `resume_incomplete_operations`); doing it
/// here would race the synchronous execute path on the same operation. So a
/// sweep settles charges whose operation already reached a terminal outcome,
/// releases those whose operation never landed, and defers the rest to a later
/// sweep (or the next startup's resume).
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
                if let Err(error) = recover(&database, &gateway, batch_size).await {
                    tracing::warn!(%error, "Broom recovery sweep failed");
                }
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum RecoverError {
    #[error("gateway error: {0}")]
    Gateway(#[from] GatewayError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

async fn recover(
    database: &Database,
    gateway: &GatewayClient,
    batch_size: i64,
) -> Result<(), RecoverError> {
    let pending = database
        .get_pending_charges(batch_size, MIN_PENDING_AGE)
        .await?;
    tracing::debug!("Broom reconciling {} pending charges", pending.len());

    for charge in pending {
        if let Err(error) = reconcile(database, gateway, &charge).await {
            tracing::warn!(
                account_id = %charge.account_id,
                operation_key = %charge.operation_key,
                "Broom failed to reconcile charge: {error}",
            );
        }
    }
    Ok(())
}

async fn reconcile(
    database: &Database,
    gateway: &GatewayClient,
    charge: &PendingCharge,
) -> Result<(), RecoverError> {
    let key = IdempotencyKey(charge.operation_key.to_string());

    let Some(operation) = gateway.operation_by_idempotency_key(&key).await? else {
        // The operation never reached the gateway; release the slot.
        database
            .release_pending(&charge.account_id, charge.operation_key)
            .await?;
        return Ok(());
    };

    match settlement(&operation) {
        Settlement::Settle {
            tokens_burnt,
            succeeded,
        } => {
            database
                .settle(
                    &charge.account_id,
                    charge.operation_key,
                    tokens_burnt,
                    succeeded,
                )
                .await?;
        }
        Settlement::Release => {
            database
                .release_pending(&charge.account_id, charge.operation_key)
                .await?;
        }
        Settlement::Defer => {}
    }
    Ok(())
}

/// How a charge should be settled, decided from the gateway operation's recorded
/// outcome — no chain query needed (the gateway captured the cost at execution).
enum Settlement {
    /// The operation reached a terminal outcome; charge the recorded cost.
    Settle {
        tokens_burnt: NearToken,
        succeeded: bool,
    },
    /// The operation failed before executing on chain; release the slot uncharged.
    Release,
    /// Still in flight after `resume`; reconcile on a later sweep.
    Defer,
}

fn settlement(operation: &OperationRecord) -> Settlement {
    match operation.status {
        OperationStatus::Succeeded => Settlement::Settle {
            tokens_burnt: operation.tokens_burnt(),
            succeeded: true,
        },
        // A failed operation that recorded an execution outcome reverted on chain
        // (gas was burnt — charge it); one without an outcome was rejected before
        // execution (nothing landed — release).
        OperationStatus::Failed => {
            if operation.final_outcome().is_some() {
                Settlement::Settle {
                    tokens_burnt: operation.tokens_burnt(),
                    succeeded: false,
                }
            } else {
                Settlement::Release
            }
        }
        OperationStatus::Pending | OperationStatus::InProgress => Settlement::Defer,
    }
}
