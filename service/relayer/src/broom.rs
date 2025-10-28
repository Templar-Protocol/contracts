use std::time::Duration;

use tokio::{select, sync::watch};

use crate::client::{database::Database, near::Near};

#[tracing::instrument(skip_all, fields(batch_size = %batch_size, delay = ?delay))]
pub async fn start(
    database: Database,
    near: Near,
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
                if let Ok(pending_transactions) = database
                    .get_pending_transactions(batch_size)
                    .await
                {
                    tracing::debug!(
                        "Broom processing {} pending transactions...",
                        pending_transactions.len(),
                    );

                    for (account_id, transaction_hash) in pending_transactions {
                        let status = match near
                            .fetch_transaction_status(account_id.clone(), transaction_hash)
                            .await
                        {
                            Ok(s) => s,
                            Err(e) => {
                                tracing::warn!("Failed to fetch transaction status for ({account_id}, {transaction_hash}): {e}");
                                continue;
                            }
                        };

                        if let Err(e) = database.record_transaction(&account_id, &status).await {
                            tracing::warn!("Broom error trying to automatically record transaction ({account_id}, {transaction_hash}): {e}");
                        }
                    }
                } else {
                    tracing::warn!("Failed to fetch pending transactions.");
                }
            }
        }
    }
}
