use near_sdk::{AccountId, AccountIdRef, NearToken};
use sqlx::{postgres::PgPoolOptions, types::Decimal, PgPool};
use templar_gateway_types::{OperationRecord, OperationStatus};
use tokio::sync::watch;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Database {
    connection: PgPool,
}

/// How an accounted gateway operation resolved, from the caller's perspective.
/// A terminal operation has had its charge settled (or released); a non-terminal
/// one is still in flight, its charge left locked for the broom to reconcile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountedStatus {
    /// Terminal success: the recorded cost (gas + inner spend) was charged.
    Succeeded,
    /// Terminal failure: either reverted on chain (gas charged) or rejected
    /// before execution (charge released). Nothing more will be billed.
    Failed,
    /// Not yet terminal: the charge is left locked and settled later by the broom
    /// against the gateway's final record — never billed off a non-final cost.
    Pending,
}

#[derive(Debug, sqlx::Type, PartialEq, Eq)]
#[sqlx(type_name = "account_mark", rename_all = "lowercase")]
pub enum AccountMark {
    Default,
    AlwaysApprove,
    AlwaysDeny,
}

/// An account with a charge in flight, awaiting settlement against its gateway
/// operation. The broom reconciles each by looking the operation up in the
/// gateway store by `operation_key`.
#[derive(Debug, Clone)]
pub struct PendingCharge {
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
    #[error("Account \"{account_id}\" already has a charge in flight")]
    pub struct PendingChargeError {
        pub account_id: AccountId,
    }

    /// Failure to lock allowance for a new charge.
    #[derive(Debug, Error)]
    pub enum LockError {
        #[error(transparent)]
        AccountDoesNotExist(#[from] AccountDoesNotExistError),
        #[error(transparent)]
        InsufficientAllowance(#[from] InsufficientAllowanceError),
        #[error(transparent)]
        PendingCharge(#[from] PendingChargeError),
        #[error("SQL error: {0}")]
        Sql(#[from] sqlx::Error),
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

    /// Accounts whose in-flight charge is at least `min_age` old, oldest first.
    ///
    /// The `min_age` floor skips charges young enough to still be mid-submission,
    /// so a charge whose gateway operation isn't persisted yet is never mistaken
    /// for an abandoned one. `updated_at` is set when the lock is taken, so it is
    /// the charge's age.
    ///
    /// # Errors
    ///
    /// - Query errors
    #[tracing::instrument(skip(self), fields(limit = %limit, min_age = ?min_age))]
    pub async fn get_pending_charges(
        &self,
        limit: i64,
        min_age: std::time::Duration,
    ) -> Result<Vec<PendingCharge>, sqlx::Error> {
        tracing::debug!("Fetching pending charges");
        let results = sqlx::query!(
            r#"
SELECT
    account_id,
    pending_operation_key AS "operation_key!"
FROM
    account
WHERE
    pending_operation_key IS NOT NULL
    AND updated_at < NOW() - make_interval(secs => $2)
ORDER BY
    updated_at ASC
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
                Some(PendingCharge {
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

    /// Lock allowance for a charge the relayer is about to submit, keyed by the
    /// gateway idempotency key (`operation_key`). Claims the account's single
    /// in-flight slot and records the deposit (`inner_spend`) to bill if the
    /// operation succeeds; the gas cost is read back from the gateway at
    /// settlement. `gas_estimate` only gates affordability here and is not
    /// stored.
    ///
    /// # Errors
    ///
    /// - Query errors
    /// - Account does not exist
    /// - A charge is already in flight for the account
    /// - Insufficient allowance
    #[tracing::instrument(skip(self), fields(
        account_id = %account_id,
        gas_estimate = %gas_estimate,
        inner_spend = %inner_spend,
        operation_key = %operation_key
    ))]
    pub async fn lock_pending(
        &self,
        account_id: &AccountIdRef,
        gas_estimate: NearToken,
        inner_spend: NearToken,
        operation_key: Uuid,
    ) -> Result<(), error::LockError> {
        tracing::debug!("Locking allowance for charge");
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

        let required = gas_estimate.saturating_add(inner_spend);

        if account.mark != AccountMark::AlwaysApprove
            && account.allowance < Decimal::from(required.as_yoctonear())
        {
            return Err(error::InsufficientAllowanceError {
                account_id: account_id.to_owned(),
                required,
                #[allow(
                    clippy::unwrap_used,
                    reason = "guaranteed to be less than `required`, which fits in u128"
                )]
                actual: NearToken::from_yoctonear(account.allowance.try_into().unwrap()),
            }
            .into());
        }

        // Claim the account's single in-flight slot (the `pending_operation_key
        // IS NULL` guard). If it is already taken, no row matches and nothing is
        // claimed.
        let claimed = sqlx::query!(
            r#"
UPDATE
    account
SET
    pending_operation_key = $2,
    pending_inner_spend = $3
WHERE
    account_id = $1
    AND pending_operation_key IS NULL
RETURNING
    account_id
"#,
            account_id.as_str(),
            operation_key,
            Decimal::from(inner_spend.as_yoctonear()),
        )
        .fetch_optional(&mut *tx)
        .await?;

        if claimed.is_none() {
            return Err(error::PendingChargeError {
                account_id: account_id.to_owned(),
            }
            .into());
        }

        tx.commit().await?;
        Ok(())
    }

    /// Resolve an in-flight charge from the gateway's knowledge of its operation —
    /// the single rule every caller (the dispatching endpoint and the broom) uses,
    /// so a charge is only ever billed off a *terminal* operation's final cost.
    ///
    /// - `None` (no operation persisted — planning failed, or a reservation was
    ///   reaped): nothing reached the chain, so release the slot uncharged.
    /// - `Succeeded`: settle the recorded cost (gas + inner spend).
    /// - `Failed`: reverted on chain (recorded an outcome) settles the gas;
    ///   rejected before execution (no outcome) releases the slot uncharged.
    /// - `Pending`/`InProgress`: not terminal, so the cost isn't final — leave the
    ///   charge locked for the broom to reconcile once the operation settles. This
    ///   is what makes undercharging a still-in-flight operation impossible: there
    ///   is no path that settles a non-terminal record.
    ///
    /// # Errors
    ///
    /// - Query errors
    pub async fn resolve_charge(
        &self,
        account_id: &AccountIdRef,
        operation_key: Uuid,
        operation: Option<&OperationRecord>,
    ) -> Result<AccountedStatus, sqlx::Error> {
        let Some(operation) = operation else {
            self.release_pending(account_id, operation_key).await?;
            return Ok(AccountedStatus::Failed);
        };
        match operation.status {
            OperationStatus::Succeeded => {
                self.settle(account_id, operation_key, operation.tokens_burnt(), true)
                    .await?;
                Ok(AccountedStatus::Succeeded)
            }
            OperationStatus::Failed => {
                if operation.final_outcome().is_some() {
                    self.settle(account_id, operation_key, operation.tokens_burnt(), false)
                        .await?;
                } else {
                    self.release_pending(account_id, operation_key).await?;
                }
                Ok(AccountedStatus::Failed)
            }
            OperationStatus::Pending | OperationStatus::InProgress => Ok(AccountedStatus::Pending),
        }
    }

    /// Settle an in-flight charge against its gateway operation's actual cost:
    /// debit `tokens_burnt` (always) plus the locked `inner_spend` (only if the
    /// operation succeeded), and release the slot.
    ///
    /// Private: the only caller is [`Database::resolve_charge`], which alone
    /// decides whether an operation is terminal enough to settle.
    ///
    /// Idempotent: the `pending_operation_key = $2` guard means a charge already
    /// settled (e.g. by the hot path before the broom got to it) matches no row
    /// and is a no-op.
    #[tracing::instrument(skip(self), fields(
        account_id = %account_id,
        operation_key = %operation_key,
        tokens_burnt = %tokens_burnt,
        succeeded = succeeded,
    ))]
    async fn settle(
        &self,
        account_id: &AccountIdRef,
        operation_key: Uuid,
        tokens_burnt: NearToken,
        succeeded: bool,
    ) -> Result<(), sqlx::Error> {
        tracing::info!("Settling charge");
        sqlx::query!(
            r#"
UPDATE
    account
SET
    allowance = greatest(
        allowance - (
            $3 + CASE
                WHEN $4 THEN coalesce(pending_inner_spend, 0)
                ELSE 0
            END
        ),
        0
    ),
    pending_operation_key = NULL,
    pending_inner_spend = NULL
WHERE
    account_id = $1
    AND pending_operation_key = $2
"#,
            account_id.as_str(),
            operation_key,
            Decimal::from(tokens_burnt.as_yoctonear()),
            succeeded,
        )
        .execute(&self.connection)
        .await?;

        Ok(())
    }

    /// Release an in-flight charge without billing it — for an operation that
    /// never reached the chain. Only clears the slot; the allowance was never
    /// debited at lock time, so nothing is refunded.
    ///
    /// Idempotent, like [`Database::settle`].
    ///
    /// # Errors
    ///
    /// - Query errors
    #[tracing::instrument(skip(self), fields(account_id = %account_id, operation_key = %operation_key))]
    pub async fn release_pending(
        &self,
        account_id: &AccountIdRef,
        operation_key: Uuid,
    ) -> Result<(), sqlx::Error> {
        tracing::info!("Releasing charge");
        sqlx::query!(
            r#"
UPDATE
    account
SET
    pending_operation_key = NULL,
    pending_inner_spend = NULL
WHERE
    account_id = $1
    AND pending_operation_key = $2
"#,
            account_id.as_str(),
            operation_key,
        )
        .execute(&self.connection)
        .await?;

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
        // Idempotent: a concurrent or retried create leaves the existing row
        // (and its allowance) untouched.
        sqlx::query!(
            "
INSERT INTO
    account (account_id, allowance)
VALUES
    ($1, $2) ON CONFLICT (account_id) DO NOTHING
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use templar_gateway_types::{ManagedAccountId, OperationId};

    use super::*;

    fn db(pool: PgPool) -> Database {
        Database { connection: pool }
    }

    fn acct(id: &str) -> AccountId {
        id.parse().unwrap()
    }

    fn near(yocto: u128) -> NearToken {
        NearToken::from_yoctonear(yocto)
    }

    /// A step-less operation record with the given status (so `tokens_burnt` is 0
    /// and `final_outcome` is `None`) — enough to exercise resolve_charge's
    /// settle/release/defer branches.
    fn operation(status: OperationStatus) -> OperationRecord {
        OperationRecord {
            id: OperationId("op".to_owned()),
            signer_account_id: ManagedAccountId("signer.near".parse().unwrap()),
            status,
            steps: vec![],
        }
    }

    async fn allowance(db: &Database, account: &AccountId) -> u128 {
        db.get_available_allowance(account)
            .await
            .unwrap()
            .unwrap()
            .as_yoctonear()
    }

    /// A successful settle debits gas (`tokens_burnt`) and the locked deposit.
    #[sqlx::test]
    async fn settle_success_debits_gas_and_inner(pool: PgPool) {
        let db = db(pool);
        let account = acct("a.near");
        db.create_account(&account, near(100)).await.unwrap();

        let key = Uuid::new_v4();
        db.lock_pending(&account, near(10), near(5), key)
            .await
            .unwrap();
        // Locking does not touch the allowance.
        assert_eq!(allowance(&db, &account).await, 100);

        db.settle(&account, key, near(8), true).await.unwrap();
        assert_eq!(allowance(&db, &account).await, 100 - 8 - 5);
    }

    /// A reverted settle debits only gas — the deposit is not billed.
    #[sqlx::test]
    async fn settle_revert_debits_gas_only(pool: PgPool) {
        let db = db(pool);
        let account = acct("a.near");
        db.create_account(&account, near(100)).await.unwrap();

        let key = Uuid::new_v4();
        db.lock_pending(&account, near(10), near(5), key)
            .await
            .unwrap();
        db.settle(&account, key, near(8), false).await.unwrap();
        assert_eq!(allowance(&db, &account).await, 100 - 8);
    }

    /// Releasing an unsubmitted charge leaves the allowance untouched (nothing
    /// was debited at lock time — releasing must not inflate it).
    #[sqlx::test]
    async fn release_does_not_change_allowance(pool: PgPool) {
        let db = db(pool);
        let account = acct("a.near");
        db.create_account(&account, near(100)).await.unwrap();

        let key = Uuid::new_v4();
        db.lock_pending(&account, near(10), near(5), key)
            .await
            .unwrap();
        db.release_pending(&account, key).await.unwrap();
        assert_eq!(allowance(&db, &account).await, 100);

        // The slot is free again.
        db.lock_pending(&account, near(10), near(0), Uuid::new_v4())
            .await
            .unwrap();
    }

    /// Settling twice for the same charge debits once (the slot guard makes the
    /// second a no-op), so a hot-path/broom race can't double-charge.
    #[sqlx::test]
    async fn settle_is_idempotent(pool: PgPool) {
        let db = db(pool);
        let account = acct("a.near");
        db.create_account(&account, near(100)).await.unwrap();

        let key = Uuid::new_v4();
        db.lock_pending(&account, near(10), near(5), key)
            .await
            .unwrap();
        db.settle(&account, key, near(8), true).await.unwrap();
        db.settle(&account, key, near(8), true).await.unwrap();
        assert_eq!(allowance(&db, &account).await, 100 - 8 - 5);
    }

    /// A non-terminal operation is never settled on the spot: resolve_charge
    /// defers it, leaving the charge locked so the broom later bills the final
    /// cost rather than the (currently zero) recorded one.
    #[sqlx::test]
    async fn resolve_charge_defers_non_terminal(pool: PgPool) {
        let db = db(pool);
        let account = acct("a.near");
        db.create_account(&account, near(100)).await.unwrap();

        let key = Uuid::new_v4();
        db.lock_pending(&account, near(10), near(5), key)
            .await
            .unwrap();

        let status = db
            .resolve_charge(&account, key, Some(&operation(OperationStatus::InProgress)))
            .await
            .unwrap();
        assert_eq!(status, AccountedStatus::Pending);
        // Allowance untouched, and the slot is still locked (a second lock fails).
        assert_eq!(allowance(&db, &account).await, 100);
        let err = db
            .lock_pending(&account, near(10), near(0), Uuid::new_v4())
            .await
            .unwrap_err();
        assert!(matches!(err, error::LockError::PendingCharge(_)));
    }

    /// A succeeded operation settles its recorded cost (here just the locked
    /// deposit, with zero-step gas).
    #[sqlx::test]
    async fn resolve_charge_settles_succeeded(pool: PgPool) {
        let db = db(pool);
        let account = acct("a.near");
        db.create_account(&account, near(100)).await.unwrap();

        let key = Uuid::new_v4();
        db.lock_pending(&account, near(10), near(5), key)
            .await
            .unwrap();

        let status = db
            .resolve_charge(&account, key, Some(&operation(OperationStatus::Succeeded)))
            .await
            .unwrap();
        assert_eq!(status, AccountedStatus::Succeeded);
        assert_eq!(allowance(&db, &account).await, 100 - 5);
    }

    /// A failed operation with no execution outcome (rejected before running)
    /// releases its charge uncharged.
    #[sqlx::test]
    async fn resolve_charge_releases_rejected(pool: PgPool) {
        let db = db(pool);
        let account = acct("a.near");
        db.create_account(&account, near(100)).await.unwrap();

        let key = Uuid::new_v4();
        db.lock_pending(&account, near(10), near(5), key)
            .await
            .unwrap();

        let status = db
            .resolve_charge(&account, key, Some(&operation(OperationStatus::Failed)))
            .await
            .unwrap();
        assert_eq!(status, AccountedStatus::Failed);
        assert_eq!(allowance(&db, &account).await, 100);
        // The slot is free again.
        db.lock_pending(&account, near(10), near(0), Uuid::new_v4())
            .await
            .unwrap();
    }

    /// No operation (planning failed, or a reservation was reaped) releases the
    /// charge uncharged — the rule the dispatching endpoint's error path relies on
    /// so a failed plan never strands the account's single slot.
    #[sqlx::test]
    async fn resolve_charge_releases_missing_operation(pool: PgPool) {
        let db = db(pool);
        let account = acct("a.near");
        db.create_account(&account, near(100)).await.unwrap();

        let key = Uuid::new_v4();
        db.lock_pending(&account, near(10), near(5), key)
            .await
            .unwrap();

        let status = db.resolve_charge(&account, key, None).await.unwrap();
        assert_eq!(status, AccountedStatus::Failed);
        assert_eq!(allowance(&db, &account).await, 100);
        // The slot is free again.
        db.lock_pending(&account, near(10), near(0), Uuid::new_v4())
            .await
            .unwrap();
    }

    /// Only one charge may be in flight per account.
    #[sqlx::test]
    async fn lock_rejects_second_charge(pool: PgPool) {
        let db = db(pool);
        let account = acct("a.near");
        db.create_account(&account, near(100)).await.unwrap();

        db.lock_pending(&account, near(10), near(0), Uuid::new_v4())
            .await
            .unwrap();
        let err = db
            .lock_pending(&account, near(10), near(0), Uuid::new_v4())
            .await
            .unwrap_err();
        assert!(matches!(err, error::LockError::PendingCharge(_)));
    }

    /// A lock whose gas + deposit exceeds the allowance is rejected.
    #[sqlx::test]
    async fn lock_rejects_insufficient_allowance(pool: PgPool) {
        let db = db(pool);
        let account = acct("a.near");
        db.create_account(&account, near(10)).await.unwrap();

        let err = db
            .lock_pending(&account, near(8), near(5), Uuid::new_v4())
            .await
            .unwrap_err();
        assert!(matches!(err, error::LockError::InsufficientAllowance(_)));
    }

    /// A locked charge is reported as pending (with its key) for the broom.
    #[sqlx::test]
    async fn get_pending_charges_lists_locked_account(pool: PgPool) {
        let db = db(pool);
        let account = acct("a.near");
        db.create_account(&account, near(100)).await.unwrap();

        let key = Uuid::new_v4();
        db.lock_pending(&account, near(10), near(0), key)
            .await
            .unwrap();

        let pending = db.get_pending_charges(10, Duration::ZERO).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].account_id, account);
        assert_eq!(pending[0].operation_key, key);

        // Once settled, it is no longer pending.
        db.settle(&account, key, near(1), true).await.unwrap();
        assert!(db
            .get_pending_charges(10, Duration::ZERO)
            .await
            .unwrap()
            .is_empty());
    }

    /// Locking a charge refreshes the account's `updated_at` (via the
    /// `updated_at_trigger`), so a just-locked slot is never immediately
    /// broom-eligible — even on a long-idle account whose previous `updated_at`
    /// predates `min_age`. Guards the broom-vs-live-path race: the broom must not
    /// reclaim a slot before the gateway has persisted its operation.
    #[sqlx::test]
    async fn lock_pending_refreshes_charge_age(pool: PgPool) {
        let db = db(pool);
        let account = acct("a.near");
        db.create_account(&account, near(100)).await.unwrap();

        // Backdate the account well past `min_age` to simulate a long-idle
        // account. The trigger overrides `updated_at` on any UPDATE, so disable it
        // just for this setup step.
        sqlx::query("ALTER TABLE account DISABLE TRIGGER updated_at_trigger")
            .execute(&db.connection)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE account SET updated_at = NOW() - INTERVAL '1 hour' WHERE account_id = $1",
        )
        .bind(account.to_string())
        .execute(&db.connection)
        .await
        .unwrap();
        sqlx::query("ALTER TABLE account ENABLE TRIGGER updated_at_trigger")
            .execute(&db.connection)
            .await
            .unwrap();

        // Locking fires the trigger, stamping `updated_at = NOW()`.
        let key = Uuid::new_v4();
        db.lock_pending(&account, near(10), near(5), key)
            .await
            .unwrap();

        // Despite the stale account, the fresh lock is not yet broom-eligible.
        let pending = db
            .get_pending_charges(10, Duration::from_secs(60))
            .await
            .unwrap();
        assert!(
            pending.is_empty(),
            "a just-locked charge must not be broom-eligible until it ages past min_age"
        );
    }
}
