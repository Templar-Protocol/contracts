use std::{
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};

use tracing::{debug, warn};

use crate::client::{database::Database, near::Near};

#[derive(Debug)]
pub struct Broom {
    kill_switch: Arc<AtomicBool>,
}

impl Drop for Broom {
    fn drop(&mut self) {
        self.kill_switch
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Broom {
    pub fn new(database: Database, near: Near, batch_size: u32, delay: Duration) -> Self {
        let kill_switch = Arc::new(AtomicBool::new(false));

        let kill = Self {
            kill_switch: Arc::clone(&kill_switch),
        };

        tokio::spawn(async move {
            while !kill_switch.load(std::sync::atomic::Ordering::Relaxed) {
                #[allow(
                    clippy::unwrap_used,
                    reason = "We should always be connected to the database"
                )]
                let pending_transactions = database
                    .get_pending_transactions(i64::from(batch_size))
                    .await
                    .unwrap();

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

                tokio::time::sleep(delay).await;
            }
        });

        kill
    }
}
