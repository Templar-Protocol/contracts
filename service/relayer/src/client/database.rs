use near_primitives::hash::CryptoHash;
use near_sdk::{AccountIdRef, NearToken};
use sqlx::{postgres::PgPoolOptions, types::Decimal, PgPool};
use tracing::warn;

#[derive(Debug, Clone)]
pub struct Database {
    connection: PgPool,
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
    pub fn new(database_url: &str) -> Result<Self, sqlx::Error> {
        let connection = PgPoolOptions::new()
            .max_connections(4)
            .connect_lazy(database_url)?;

        Ok(Self { connection })
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
            "update account set
                allowance_locked = $1,
                pending_transaction_hash = $2,
                pending_transaction_issued_at = now()
            where account_id = $3 and allowance_locked = 0 and allowance >= $1",
            Decimal::from(allowance_lock_amount.as_yoctonear()),
            &transaction_hash.0,
            account_id.as_str(),
        )
        .execute(&self.connection)
        .await?;

        if affected.rows_affected() == 0 {
            let account = sqlx::query!(
                "select allowance, allowance_locked, pending_transaction_hash from account where account_id = $1",
                account_id.as_str(),
            ).fetch_optional(&self.connection).await?;
            let record = account.ok_or_else(|| error::AccountDoesNotExistError {
                account_id: account_id.to_owned(),
            })?;
            if let Some(pending_transaction_hash) = record.pending_transaction_hash {
                let pending_transaction_hash = Some(CryptoHash(
                    <[u8; 32]>::try_from(pending_transaction_hash).unwrap_or_default(),
                ));
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
        } else {
            Ok(())
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
        transaction_hash: CryptoHash,
        allowance_spent: NearToken,
        succeeded: bool,
    ) -> Result<(), error::RecordTransactionError> {
        let mut tx = self.connection.begin().await?;
        let result = sqlx::query!(
            "update account set
                allowance_locked = 0,
                pending_transaction_hash = null,
                pending_transaction_issued_at = null
            where account_id = $1 and pending_transaction_hash = $2",
            account_id.as_str(),
            &transaction_hash.0,
        )
        .execute(&mut *tx)
        .await?;

        if result.rows_affected() == 0 {
            warn!("Failed to unlock allowance for {account_id}");
            let account = sqlx::query!(
                "select pending_transaction_hash from account where account_id = $1",
                account_id.as_str(),
            )
            .fetch_optional(&mut *tx)
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
            "insert into call (account_id, transaction_hash, allowance_spent, succeeded) values ($1, $2, $3, $4)",
            account_id.as_str(),
            &transaction_hash.0,
            Decimal::from(allowance_spent.as_yoctonear()),
            succeeded,
        ).execute(&mut *tx).await?;

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
            "insert into account (account_id, allowance) values ($1, $2)",
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
            "select allowance, allowance_locked from account where account_id = $1",
            account_id.as_str(),
        )
        .fetch_optional(&self.connection)
        .await?;

        Ok(result
            .and_then(|r| u128::try_from(r.allowance.saturating_sub(r.allowance_locked)).ok())
            .map(NearToken::from_yoctonear))
    }
}
