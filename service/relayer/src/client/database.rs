use near_primitives::hash::CryptoHash;
use near_sdk::{AccountId, AccountIdRef, NearToken};
use sqlx::{postgres::PgPoolOptions, types::Decimal, PgPool};
use tokio::sync::watch;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Database {
    connection: PgPool,
}

#[derive(Debug, sqlx::Type, PartialEq, Eq)]
#[sqlx(type_name = "account_mark", rename_all = "lowercase")]
pub enum AccountMark {
    Default,
    AlwaysApprove,
    AlwaysDeny,
}

#[derive(Debug, sqlx::Type, PartialEq, Eq)]
#[sqlx(type_name = "transaction_status", rename_all = "lowercase")]
pub enum TransactionStatus {
    Pending,
    Succeeded,
    Failed,
}

/// A pending transaction whose on-chain hash is already known, ready for the
/// broom to reconcile against its execution outcome.
#[derive(Debug, Clone)]
pub struct PendingTransaction {
    pub account_id: AccountId,
    pub operation_key: Uuid,
}

pub mod error {
    use near_sdk::{AccountId, NearToken};
    use thiserror::Error;

    #[derive(Debug, Error)]
    #[error("Account \"{account_id}\" does not exist in database")]
    pub struct AccountDoesNotExistError {
        pub account_id: AccountId,
    }

    #[derive(Debug, Error)]
    #[error("Account \"{account_id}\" has insufficient allowance: {actual} < {required}")]
    pub struct InsufficientAllowanceError {
        pub account_id: AccountId,
        pub actual: NearToken,
        pub required: NearToken,
    }

    #[derive(Debug, Error)]
    #[error("Account \"{account_id}\" already has a pending transaction")]
    pub struct PendingTransactionError {
        pub account_id: AccountId,
    }

    #[derive(Debug, Error)]
    #[error("Account \"{account_id}\" does not have a pending transaction")]
    pub struct MissingPendingTransactionError {
        pub account_id: AccountId,
    }

    #[derive(Debug, Error)]
    pub enum SetPendingTransactionError {
        #[error(transparent)]
        AccountDoesNotExist(#[from] AccountDoesNotExistError),
        #[error(transparent)]
        InsufficientAllowance(#[from] InsufficientAllowanceError),
        #[error(transparent)]
        PendingTransaction(#[from] PendingTransactionError),
        #[error("SQL error: {0}")]
        Sql(#[from] sqlx::Error),
        #[error("Unknown error: {0}")]
        UnknownError(AccountId),
    }

    #[derive(Debug, Error)]
    pub enum RecordTransactionError {
        #[error(transparent)]
        AccountDoesNotExist(#[from] AccountDoesNotExistError),
        #[error(transparent)]
        MissingPendingTransaction(#[from] MissingPendingTransactionError),
        #[error("SQL error: {0}")]
        Sql(#[from] sqlx::Error),
        #[error("Unknown error: {0}")]
        UnknownError(AccountId),
    }
}

impl Database {
    /// # Errors
    ///
    /// - Database connection errors
    pub fn new(database_url: &str, kill: watch::Sender<()>) -> Result<Self, sqlx::Error> {
        let connection = PgPoolOptions::new()
            .max_connections(4)
            .connect_lazy(database_url)?;

        tokio::spawn({
            let connection = connection.clone();
            async move {
                let mut on_kill = kill.subscribe();
                #[allow(clippy::unwrap_used)]
                on_kill.changed().await.unwrap();
                tracing::info!("Closing database connection...");
                connection.close().await;
                tracing::info!("Database connection closed.");
            }
        });

        Ok(Self { connection })
    }

    /// Migrate the database schema.
    ///
    /// # Errors
    ///
    /// - Database migration errors
    pub async fn migrate(&self) -> Result<(), sqlx::migrate::MigrateError> {
        sqlx::migrate!("./migrations").run(&self.connection).await
    }

    /// Pending transactions older than `min_age`, oldest first.
    ///
    /// The broom reconciles each against the gateway operation store (keyed by
    /// `operation_key`). The `min_age` floor skips rows young enough to still be
    /// mid-submission, so a row whose gateway operation isn't persisted yet is
    /// never mistaken for an abandoned one.
    ///
    /// # Errors
    ///
    /// - Query errors
    #[tracing::instrument(skip(self), fields(limit = %limit, min_age = ?min_age))]
    pub async fn get_pending_transactions(
        &self,
        limit: i64,
        min_age: std::time::Duration,
    ) -> Result<Vec<PendingTransaction>, sqlx::Error> {
        tracing::debug!("Fetching pending transactions");
        let results = sqlx::query!(
            r#"
SELECT
    account_id,
    operation_key
FROM
    "transaction"
WHERE
    "status" = 'pending'::transaction_status
    AND created_at < NOW() - make_interval(secs => $2)
ORDER BY
    created_at ASC
LIMIT
    $1
"#,
            limit,
            min_age.as_secs_f64(),
        )
        .fetch_all(&self.connection)
        .await?;

        Ok(results
            .into_iter()
            .filter_map(|r| {
                // A malformed account id skips the row rather than aborting the
                // batch; such records should never exist.
                Some(PendingTransaction {
                    account_id: r.account_id.parse().ok()?,
                    operation_key: r.operation_key,
                })
            })
            .collect())
    }

    /// # Errors
    ///
    /// - Query errors
    pub async fn get_available_allowance_or_create(
        &self,
        account_id: &AccountIdRef,
        default_allowance: NearToken,
    ) -> Result<NearToken, sqlx::Error> {
        let available = self.get_available_allowance(account_id).await?;
        if let Some(available) = available {
            Ok(available)
        } else {
            self.create_account(account_id, default_allowance).await?;
            Ok(default_allowance)
        }
    }

    /// Lock allowance for a transaction the relayer is about to submit, keyed by
    /// the gateway idempotency key. The on-chain transaction hash is unknown at
    /// this point and is attached later via [`Database::attach_transaction_hash`].
    ///
    /// # Errors
    ///
    /// - Query errors
    /// - Account does not exist
    /// - Pending transaction already exists
    /// - Insufficient allowance
    #[tracing::instrument(skip(self), fields(
        account_id = %account_id,
        allowance_lock_gas = %allowance_lock_gas,
        allowance_lock_inner = %allowance_lock_inner,
        operation_key = %operation_key
    ))]
    pub async fn set_pending_transaction(
        &self,
        account_id: &AccountIdRef,
        allowance_lock_gas: NearToken,
        allowance_lock_inner: NearToken,
        operation_key: Uuid,
    ) -> Result<(), error::SetPendingTransactionError> {
        tracing::debug!("Setting pending transaction");
        let mut tx = self.connection.begin().await?;

        let account = sqlx::query!(
            r#"
SELECT
    allowance,
    mark AS "mark: AccountMark"
FROM
    account
WHERE
    account_id = $1
    AND mark <> 'always_deny'
"#,
            account_id.to_string(),
        )
        .fetch_optional(&mut *tx)
        .await?;

        let Some(account) = account else {
            return Err(error::AccountDoesNotExistError {
                account_id: account_id.to_owned(),
            }
            .into());
        };

        let allowance_lock_total = allowance_lock_gas.saturating_add(allowance_lock_inner);

        if account.mark != AccountMark::AlwaysApprove
            && account.allowance < Decimal::from(allowance_lock_total.as_yoctonear())
        {
            return Err(error::InsufficientAllowanceError {
                account_id: account_id.to_owned(),
                required: allowance_lock_total,
                #[allow(
                    clippy::unwrap_used,
                    reason = "guaranteed to be less than `allowance_lock_total`, which fits in u128"
                )]
                actual: NearToken::from_yoctonear(account.allowance.try_into().unwrap()),
            }
            .into());
        }

        // Claim the account's pending slot *first* (the `pending_operation_key
        // IS NULL` guard), then insert the transaction row only if the claim
        // succeeded. Doing it in this order means a contended account never
        // leaves an orphaned `pending` transaction row behind (which would keep
        // its allowance reserved forever): if the slot is already taken, the
        // claim matches no row, so nothing is inserted and none is returned.
        let claimed = sqlx::query!(
            r#"
WITH claimed AS (
    UPDATE
        account
    SET
        pending_operation_key = $1
    WHERE
        account_id = $2
        AND pending_operation_key IS NULL
    RETURNING
        account_id
),
inserted AS (
    INSERT INTO
        "transaction" (
            operation_key,
            account_id,
            "status",
            allowance_spent_gas,
            allowance_spent_inner
        )
    SELECT
        $1,
        $2,
        'pending'::transaction_status,
        $3,
        $4
    FROM
        claimed
    RETURNING
        operation_key
)
SELECT
    operation_key
FROM
    inserted
"#,
            operation_key,
            account_id.as_str(),
            Decimal::from(allowance_lock_gas.as_yoctonear()),
            Decimal::from(allowance_lock_inner.as_yoctonear()),
        )
        .fetch_optional(&mut *tx)
        .await?;

        if claimed.is_none() {
            return Err(error::PendingTransactionError {
                account_id: account_id.to_owned(),
            }
            .into());
        }

        Ok(tx.commit().await?)
    }

    /// # Errors
    ///
    /// - Query errors
    pub async fn remove_pending_transaction(
        &self,
        account_id: &AccountIdRef,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.connection.begin().await?;

        let result = sqlx::query!(
            r#"
SELECT
    operation_key,
    allowance_spent_gas,
    allowance_spent_inner
FROM
    "transaction"
WHERE
    account_id = $1
    AND "status" = 'pending'::transaction_status
"#,
            account_id.to_string(),
        )
        .fetch_optional(&mut *tx)
        .await?;

        let Some(result) = result else {
            // Pending tx does not exist
            return Ok(());
        };

        let allowance_lock_total = result
            .allowance_spent_gas
            .saturating_add(result.allowance_spent_inner);

        let update_account = sqlx::query!(
            r#"
UPDATE
    account
SET
    pending_operation_key = NULL,
    allowance = allowance + $1
WHERE
    account_id = $2
    AND pending_operation_key = $3
"#,
            allowance_lock_total,
            account_id.as_str(),
            result.operation_key,
        )
        .execute(&mut *tx)
        .await?;

        if update_account.rows_affected() != 0 {
            sqlx::query!(
                r#"
DELETE FROM
    "transaction"
WHERE
    operation_key = $1
    AND account_id = $2
"#,
                result.operation_key,
                account_id.as_str(),
            )
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await
    }

    /// Finalize a pending transaction, deducting its actual cost from the
    /// account's allowance and releasing the pending slot.
    ///
    /// `tokens_burnt` is the true gas cost (summed across the transaction and
    /// all its receipts); `allowance_spent_inner` (the locked in-transaction
    /// spend, e.g. a storage deposit) is charged only when the transaction
    /// succeeded.
    ///
    /// # Errors
    ///
    /// - Account does not exist
    /// - Pending transaction does not exist
    /// - Query errors
    #[allow(clippy::too_many_lines)]
    #[tracing::instrument(skip(self), fields(
        account_id = %account_id,
        operation_key = %operation_key,
        transaction_hash = %transaction_hash,
        tokens_burnt = %tokens_burnt,
        succeeded = succeeded,
    ))]
    pub async fn finalize_pending_transaction(
        &self,
        account_id: &AccountIdRef,
        operation_key: Uuid,
        transaction_hash: CryptoHash,
        tokens_burnt: NearToken,
        succeeded: bool,
    ) -> Result<(), error::RecordTransactionError> {
        tracing::info!("Finalizing pending transaction");
        let allowance_spent_gas = tokens_burnt;

        let transaction_record = sqlx::query!(
            r#"
SELECT
    allowance_spent_inner,
    "status" AS "status: TransactionStatus"
FROM
    "transaction"
WHERE
    operation_key = $1
"#,
            operation_key,
        )
        .fetch_one(&self.connection)
        .await?;

        if transaction_record.status != TransactionStatus::Pending {
            // Final status already inserted; do nothing.
            return Ok(());
        }

        let allowance_spent_inner = NearToken::from_yoctonear(
            transaction_record
                .allowance_spent_inner
                .try_into()
                .unwrap_or(u128::MAX),
        );

        let allowance_spent = if succeeded {
            allowance_spent_gas.saturating_add(allowance_spent_inner)
        } else {
            allowance_spent_gas
        };

        let mut tx = self.connection.begin().await?;
        let result = sqlx::query!(
            "
UPDATE
    account
SET
    allowance = greatest(allowance - $1, 0),
    pending_operation_key = NULL
WHERE
    account_id = $2
    AND pending_operation_key = $3
",
            Decimal::from(allowance_spent.as_yoctonear()),
            account_id.as_str(),
            operation_key,
        )
        .execute(&mut *tx)
        .await?;

        if result.rows_affected() == 0 {
            tx.rollback().await?;

            tracing::warn!("Failed to unlock allowance for {account_id}");
            let account = sqlx::query!(
                "
SELECT
    pending_operation_key
FROM
    account
WHERE
    account_id = $1
",
                account_id.as_str(),
            )
            .fetch_optional(&self.connection)
            .await?;
            let account = account.ok_or_else(|| error::AccountDoesNotExistError {
                account_id: account_id.to_owned(),
            })?;
            if account.pending_operation_key.is_none() {
                return Err(error::MissingPendingTransactionError {
                    account_id: account_id.to_owned(),
                }
                .into());
            }
            return Err(error::RecordTransactionError::UnknownError(
                account_id.to_owned(),
            ));
        }

        let (status, allowance_spent_inner) = if succeeded {
            (TransactionStatus::Succeeded, allowance_spent_inner)
        } else {
            (TransactionStatus::Failed, NearToken::from_near(0))
        };

        sqlx::query!(
            r#"
UPDATE
    "transaction"
SET
    "status" = $1,
    allowance_spent_gas = $2,
    allowance_spent_inner = $3,
    transaction_hash = $5
WHERE
    operation_key = $4
"#,
            status as TransactionStatus,
            Decimal::from(allowance_spent_gas.as_yoctonear()),
            Decimal::from(allowance_spent_inner.as_yoctonear()),
            operation_key,
            transaction_hash.to_string(),
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(())
    }

    /// # Errors
    ///
    /// - Query errors
    pub async fn create_account(
        &self,
        account_id: &AccountIdRef,
        allowance: NearToken,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            "
INSERT INTO
    account (account_id, allowance)
VALUES
    ($1, $2)
",
            account_id.as_str(),
            Decimal::from(allowance.as_yoctonear()),
        )
        .execute(&self.connection)
        .await?;

        Ok(())
    }

    /// # Errors
    ///
    /// - Query errors
    pub async fn get_available_allowance(
        &self,
        account_id: &AccountIdRef,
    ) -> Result<Option<NearToken>, sqlx::Error> {
        let result = sqlx::query!(
            r#"
SELECT
    allowance,
    mark AS "mark: AccountMark"
FROM
    account
WHERE
    account_id = $1
"#,
            account_id.as_str(),
        )
        .fetch_optional(&self.connection)
        .await?;

        Ok(result
            .and_then(|r| {
                if r.mark == AccountMark::AlwaysDeny {
                    Some(0)
                } else {
                    u128::try_from(r.allowance).ok()
                }
            })
            .map(NearToken::from_yoctonear))
    }
}
