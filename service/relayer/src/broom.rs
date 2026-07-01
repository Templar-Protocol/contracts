use std::time::Duration;

use templar_gateway_client::Client as GatewayClient;
use templar_gateway_core::GatewayError;
use templar_gateway_types::IdempotencyKey;
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
/// Each charge is only touched once it is past `MIN_PENDING_AGE`, so a sweep
/// never races the synchronous execute path. At that point the sweep drives the
/// operation toward terminal — reconciling a submitted step against the chain,
/// or dropping a stale reservation — then settles charges whose operation
/// reached a terminal outcome, releases those whose operation never landed, and
/// defers the rest to a later sweep. (Startup still runs one `resume` pass so
/// recovery does not wait for the first sweep.)
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

    // Drive the operation toward terminal (reconciling a submitted step against
    // the chain) before settling. The charge is already past `MIN_PENDING_AGE`,
    // so this cannot race the live execute path; the driver ages out an unknown
    // transaction on its own. A reservation is left as-is (its request is still
    // planning) and deferred to a later sweep.
    let operation = gateway.reconcile_operation(&key).await?;

    // Resolve the charge through the exact rule the dispatching endpoint uses:
    // a missing operation releases the slot, a terminal one settles, and a still
    // in-flight one is left locked for a later sweep. The broom carries no
    // resolution logic of its own — it only picks up charges the endpoint could
    // not settle synchronously.
    database
        .resolve_charge(&charge.account_id, charge.operation_key, operation.as_ref())
        .await?;
    Ok(())
}
