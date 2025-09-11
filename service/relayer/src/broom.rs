use std::time::Duration;

use tokio::{select, sync::watch};
use tracing::{debug, warn};

use crate::client::{database::Database, near::Near};

pub async fn start(
    database: Database,
    near: Near,
    batch_size: u32,
    delay: Duration,
    kill: watch::Sender<()>,
) {
    let batch_size = i64::from(batch_size);

    let mut interval = tokio::time::interval(delay);
    let mut on_kill = kill.subscribe();

    loop {
        select! {
            _ = on_kill.changed() => {
                debug!("Received kill notification.");
                break;
            }
            _ = interval.tick() => {
                if let Ok(pending_transactions) = database
                    .get_pending_transactions(batch_size)
                    .await
                {
                    debug!(
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
                                warn!("Failed to fetch transaction status for ({account_id}, {transaction_hash}): {e}");
                                continue;
                            }
                        };

                        if let Err(e) = database.record_transaction(&account_id, &status).await {
                            warn!("Broom error trying to automatically record transaction ({account_id}, {transaction_hash}): {e}");
                        }
                    }
                } else {
                    warn!("Failed to fetch pending transactions.");
                }
            }
        }
    }
}
