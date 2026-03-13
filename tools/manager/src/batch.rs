use near_fetch::ops::Function;
use near_primitives::views::FinalExecutionStatus;
use near_sdk::AccountId;

/// A NEAR batch transaction bound to a specific context.
///
/// Obtain one via [`crate::CliContext::batch`]. Chain actions with [`call`](Self::call),
/// [`deploy`](Self::deploy), or [`delete_account`](Self::delete_account), then call
/// [`transact`](Self::transact) to execute, log the hash, and propagate failures.
pub struct BoundBatch<'a> {
    transaction_url_prefix: String,
    tx: near_fetch::ops::Transaction<'a>,
}

impl<'a> BoundBatch<'a> {
    pub(crate) fn new(
        transaction_url_prefix: String,
        tx: near_fetch::ops::Transaction<'a>,
    ) -> Self {
        Self {
            transaction_url_prefix,
            tx,
        }
    }

    #[must_use]
    pub fn call(self, function: Function) -> Self {
        Self {
            tx: self.tx.call(function),
            transaction_url_prefix: self.transaction_url_prefix,
        }
    }

    #[must_use]
    pub fn deploy(self, code: &[u8]) -> Self {
        Self {
            tx: self.tx.deploy(code),
            transaction_url_prefix: self.transaction_url_prefix,
        }
    }

    #[must_use]
    pub fn delete_account(self, beneficiary_id: &AccountId) -> Self {
        Self {
            tx: self.tx.delete_account(beneficiary_id),
            transaction_url_prefix: self.transaction_url_prefix,
        }
    }

    /// Execute the transaction, log its hash and explorer URL, and return an error
    /// if execution failed.
    pub async fn transact(self) -> anyhow::Result<()> {
        let result = self.tx.transact().await?;
        let hash = &result.transaction.hash;
        tracing::info!(
            transaction_hash = %hash,
            url = %format!("{}{}", self.transaction_url_prefix, hash),
            "Transaction submitted"
        );
        match result.status {
            FinalExecutionStatus::SuccessValue(_) => Ok(()),
            FinalExecutionStatus::Failure(e) => anyhow::bail!("Transaction failed: {e:?}"),
            FinalExecutionStatus::NotStarted | FinalExecutionStatus::Started => {
                anyhow::bail!("Unexpected transaction status: {:?}", result.status)
            }
        }
    }
}
