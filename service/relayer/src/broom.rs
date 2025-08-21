use std::{
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};

use near_primitives::views::{ActionView, FinalExecutionStatus};
use near_sdk::NearToken;
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
                        Ok(Some(s)) => s,
                        Ok(None) => continue,
                        Err(e) => {
                            warn!("Failed to fetch transaction status for ({account_id}, {transaction_hash}): {e}");
                            continue;
                        }
                    };

                    let allowance_spent_gas = NearToken::from_yoctonear(status.tokens_burnt());

                    let success = matches!(status.status, FinalExecutionStatus::SuccessValue(_));

                    let allowance_spent = if success {
                        let allowance_spent_storage_deposit = NearToken::from_yoctonear(
                            status
                                .transaction
                                .actions
                                .iter()
                                .filter_map(|a| match a {
                                    ActionView::FunctionCall {
                                        method_name,
                                        deposit,
                                        ..
                                    } if method_name == "storage_deposit" => Some(*deposit),
                                    _ => None,
                                })
                                .sum(),
                        );

                        allowance_spent_gas.saturating_add(allowance_spent_storage_deposit)
                    } else {
                        allowance_spent_gas
                    };

                    if let Err(e) = database
                        .record_transaction(&account_id, transaction_hash, allowance_spent, success)
                        .await
                    {
                        warn!("Broom error trying to automatically record transaction ({account_id}, {transaction_hash}): {e}");
                    }
                }

                tokio::time::sleep(delay).await;
            }
        });

        kill
    }
}
