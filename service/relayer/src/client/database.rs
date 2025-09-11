use std::str::FromStr;

use near_primitives::{
    hash::CryptoHash,
    views::{ActionView, FinalExecutionOutcomeView, FinalExecutionStatus},
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
    account_id,
    pending_transaction_hash
FROM
    account
WHERE
    pending_transaction_hash IS NOT NULL
ORDER BY
    pending_transaction_issued_at ASC
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
        allowance_lock_amount: NearToken,
        transaction_hash: CryptoHash,
    ) -> Result<(), error::SetPendingTransactionError> {
        let affected = sqlx::query!(
            r#"
UPDATE
    account
SET
    allowance_locked = $1,
    pending_transaction_hash = $2,
    pending_transaction_issued_at = NOW()
WHERE
    account_id = $3
    AND (
        (
            allowance_locked = 0
            AND allowance >= $1
            AND mark != 'always_deny'
        )
        OR mark = 'always_approve'
    )
"#,
            Decimal::from(allowance_lock_amount.as_yoctonear()),
            transaction_hash.to_string(),
            account_id.as_str(),
        )
        .execute(&self.connection)
        .await?;

        if affected.rows_affected() != 0 {
            return Ok(());
        }

        // Update failed, let's see why
        let account = sqlx::query!(
            "
SELECT
    allowance,
    allowance_locked,
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

        let record = account.ok_or_else(|| error::AccountDoesNotExistError {
            account_id: account_id.to_owned(),
        })?;

        if let Some(pending_transaction_hash) = record.pending_transaction_hash {
            let pending_transaction_hash =
                Some(CryptoHash::from_str(&pending_transaction_hash).unwrap_or_default());
            Err(error::PendingTransactionError {
                account_id: account_id.to_owned(),
                pending_transaction_hash,
            }
            .into())
        } else if !record.allowance_locked.is_zero() {
            Err(error::PendingTransactionError {
                account_id: account_id.to_owned(),
                pending_transaction_hash: None,
            }
            .into())
        } else if Decimal::from(allowance_lock_amount.as_yoctonear()) > record.allowance {
            Err(error::InsufficientAllowanceError {
                account_id: account_id.to_owned(),
                actual: NearToken::from_yoctonear(
                    u128::try_from(record.allowance).unwrap_or(u128::MAX),
                ),
                required: allowance_lock_amount,
            }
            .into())
        } else {
            Err(error::SetPendingTransactionError::UnknownError(
                account_id.to_owned(),
            ))
        }
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

        let transaction_hash = status.transaction.hash;

        self.insert_into_call(account_id, transaction_hash, allowance_spent, success)
            .await
    }

    async fn insert_into_call(
        &self,
        account_id: &AccountIdRef,
        transaction_hash: CryptoHash,
        allowance_spent: NearToken,
        succeeded: bool,
    ) -> Result<(), error::RecordTransactionError> {
        let already_inserted = sqlx::query!(
            "
SELECT
    1 AS inserted
FROM
    call
WHERE
    account_id = $1
    AND transaction_hash = $2
",
            account_id.as_str(),
            transaction_hash.to_string(),
        )
        .fetch_optional(&self.connection)
        .await?;

        if already_inserted.is_some() {
            return Ok(());
        }

        let mut tx = self.connection.begin().await?;
        let result = sqlx::query!(
            "
UPDATE
    account
SET
    allowance = greatest(allowance - $1, 0),
    allowance_locked = 0,
    pending_transaction_hash = NULL,
    pending_transaction_issued_at = NULL
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

        sqlx::query!(
            "
INSERT INTO
    call (
        account_id,
        transaction_hash,
        allowance_spent,
        succeeded
    )
VALUES
    ($1, $2, $3, $4)
",
            account_id.as_str(),
            transaction_hash.to_string(),
            Decimal::from(allowance_spent.as_yoctonear()),
            succeeded,
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
            "
SELECT
    allowance,
    allowance_locked,
    mark AS \"mark: AccountMark\"
FROM
    account
WHERE
    account_id = $1
",
            account_id.as_str(),
        )
        .fetch_optional(&self.connection)
        .await?;

        Ok(result
            .and_then(|r| {
                if r.mark == AccountMark::AlwaysDeny {
                    Some(0)
                } else {
                    u128::try_from(r.allowance.saturating_sub(r.allowance_locked)).ok()
                }
            })
            .map(NearToken::from_yoctonear))
    }
}
