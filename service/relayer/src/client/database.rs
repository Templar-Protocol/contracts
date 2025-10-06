use std::str::FromStr;

use near_primitives::{
    hash::CryptoHash,
    views::{FinalExecutionOutcomeView, FinalExecutionStatus},
};
use near_sdk::{AccountId, AccountIdRef, NearToken};
use sqlx::{postgres::PgPoolOptions, types::Decimal, PgPool};
use tokio::sync::watch;
use tracing::warn;

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

pub mod error {
    use near_primitives::hash::CryptoHash;
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
    #[error("Account \"{account_id}\" already has a pending transaction: {}", pending_transaction_hash.map(|t| t.to_string()).unwrap_or("<???>".to_string()))]
    pub struct PendingTransactionError {
        pub account_id: AccountId,
        pub pending_transaction_hash: Option<CryptoHash>,
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

    /// # Errors
    ///
    /// - Query errors
    pub async fn get_pending_transactions(
        &self,
        limit: i64,
    ) -> Result<Vec<(AccountId, CryptoHash)>, sqlx::Error> {
        let results = sqlx::query!(
            "
SELECT
    account.account_id,
    pending_transaction_hash
FROM
    account
    JOIN transaction ON account.pending_transaction_hash = transaction.transaction_hash
WHERE
    pending_transaction_hash IS NOT NULL
ORDER BY
    transaction.created_at ASC
LIMIT
    $1
",
            limit,
        )
        .fetch_all(&self.connection)
        .await?;

        Ok(results
            .into_iter()
            .filter_map(|r| {
                // Since this is a filter-map, there is technically the
                // possibility that we get (and skip) some invalid records
                // here. The number of invalid records could exceed `limit`,
                // causing us to always return an empty list.
                let account_id: AccountId = r.account_id.parse().ok()?;
                #[allow(clippy::unwrap_used, reason = "Guaranteed not null by query")]
                let hash = r
                    .pending_transaction_hash
                    .and_then(|hash| CryptoHash::from_str(&hash).ok())
                    .unwrap();
                Some((account_id, hash))
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

    /// # Errors
    ///
    /// - Query errors
    /// - Account does not exist
    /// - Pending transaction already exists
    /// - Insufficient allowance
    pub async fn set_pending_transaction(
        &self,
        account_id: &AccountIdRef,
        allowance_lock_gas: NearToken,
        allowance_lock_inner: NearToken,
        transaction_hash: CryptoHash,
    ) -> Result<(), error::SetPendingTransactionError> {
        let mut tx = self.connection.begin().await?;

        let account = sqlx::query!(
            r#"
SELECT
    pending_transaction_hash,
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
                actual: NearToken::from_yoctonear(account.allowance.try_into().unwrap()),
            }
            .into());
        }

        sqlx::query!(
            r#"
WITH inserted AS (
    INSERT INTO
        "transaction" (
            transaction_hash,
            account_id,
            "status",
            allowance_spent_gas,
            allowance_spent_inner
        )
    VALUES
        ($1, $2, 'pending'::transaction_status, $3, $4)
    RETURNING
        transaction_hash
)
UPDATE
    account
SET
    pending_transaction_hash = (
        SELECT
            transaction_hash
        FROM
            inserted
    )
WHERE
    account_id = $2
    AND pending_transaction_hash IS NULL
RETURNING
    pending_transaction_hash
"#,
            transaction_hash.to_string(),
            account_id.as_str(),
            Decimal::from(allowance_lock_gas.as_yoctonear()),
            Decimal::from(allowance_lock_inner.as_yoctonear()),
        )
        .fetch_one(&self.connection)
        .await?;

        Ok(tx.commit().await?)
    }

    /// # Errors
    ///
    /// - Account does not exist
    /// - Pending transaction does not exist
    /// - Query errors
    pub async fn record_transaction(
        &self,
        account_id: &AccountIdRef,
        status: &FinalExecutionOutcomeView,
    ) -> Result<(), error::RecordTransactionError> {
        let allowance_spent_gas =
            NearToken::from_yoctonear(status.transaction_outcome.outcome.tokens_burnt);

        let success = matches!(status.status, FinalExecutionStatus::SuccessValue(_));

        let transaction_hash = status.transaction.hash;

        self.finalize_pending_transaction(
            account_id,
            transaction_hash,
            allowance_spent_gas,
            success,
        )
        .await
    }

    async fn finalize_pending_transaction(
        &self,
        account_id: &AccountIdRef,
        transaction_hash: CryptoHash,
        allowance_spent_gas: NearToken,
        succeeded: bool,
    ) -> Result<(), error::RecordTransactionError> {
        let transaction_record = sqlx::query!(
            r#"
SELECT
    allowance_spent_inner,
    "status" AS "status: TransactionStatus"
FROM
    transaction
WHERE
    account_id = $1
    AND transaction_hash = $2
"#,
            account_id.as_str(),
            transaction_hash.to_string(),
        )
        .fetch_one(&self.connection)
        .await?;

        if transaction_record.status != TransactionStatus::Pending {
            // Final status already inserted; do nothing.
            return Ok(());
        }

        let allowance_spent_inner =
            NearToken::from_yoctonear(transaction_record.allowance_spent_inner.try_into().unwrap());

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
    pending_transaction_hash = NULL
WHERE
    account_id = $2
    AND pending_transaction_hash = $3
",
            Decimal::from(allowance_spent.as_yoctonear()),
            account_id.as_str(),
            transaction_hash.to_string(),
        )
        .execute(&mut *tx)
        .await?;

        if result.rows_affected() == 0 {
            tx.rollback().await?;

            warn!("Failed to unlock allowance for {account_id}");
            let account = sqlx::query!(
                "
SELECT
    pending_transaction_hash
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
            if account.pending_transaction_hash.is_none() {
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
    allowance_spent_inner = $3
WHERE
    transaction_hash = $4
"#,
            status as TransactionStatus,
            Decimal::from(allowance_spent_gas.as_yoctonear()),
            Decimal::from(allowance_spent_inner.as_yoctonear()),
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
