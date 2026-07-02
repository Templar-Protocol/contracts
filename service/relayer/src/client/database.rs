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
    operation_key
FROM
    pending_gateway_charge
WHERE
    updated_at < NOW() - make_interval(secs => $2)
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
    /// gateway idempotency key (`operation_key`). Records gas and inner-spend
    /// reservations so concurrent gateway operations cannot overdraw the account;
    /// the actual gas cost is read back from the gateway at settlement.
    ///
    /// # Errors
    ///
    /// - Query errors
    /// - Account does not exist
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
FOR UPDATE
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

        let reserved = sqlx::query_scalar!(
            r#"
SELECT
    coalesce(sum(gas_estimate + inner_spend), 0) AS "reserved!: Decimal"
FROM
    pending_gateway_charge
WHERE
    account_id = $1
"#,
            account_id.as_str(),
        )
        .fetch_one(&mut *tx)
        .await?;
        let available = account.allowance - reserved;

        if account.mark != AccountMark::AlwaysApprove
            && available < Decimal::from(required.as_yoctonear())
        {
            return Err(error::InsufficientAllowanceError {
                account_id: account_id.to_owned(),
                required,
                actual: NearToken::from_yoctonear(u128::try_from(available).unwrap_or(0)),
            }
            .into());
        }

        sqlx::query!(
            r#"
INSERT INTO
    pending_gateway_charge (operation_key, account_id, gas_estimate, inner_spend)
VALUES
    ($1, $2, $3, $4)
"#,
            operation_key,
            account_id.as_str(),
            Decimal::from(gas_estimate.as_yoctonear()),
            Decimal::from(inner_spend.as_yoctonear()),
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Resolve an in-flight charge from the gateway's knowledge of its operation —
    /// the single rule every caller (the dispatching endpoint and the broom) uses,
    /// so a charge is only ever billed off a *terminal* operation's final cost.
    ///
    /// - `None` (no operation persisted — planning failed, or a reservation was
    ///   reaped): nothing reached the chain, so release the reservation uncharged.
    /// - `Succeeded`: settle the recorded cost (gas + inner spend).
    /// - `Failed`: reverted on chain (recorded an outcome) settles the gas;
    ///   rejected before execution (no outcome) releases the reservation uncharged.
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
                // Bill the locked deposit only when the operation actually executed.
                // A planned no-op is terminal `Succeeded` with no outcome and never
                // attached the deposit, so settling it must not charge `inner_spend`.
                let executed = operation.final_outcome().is_some();
                self.settle(
                    account_id,
                    operation_key,
                    operation.tokens_burnt(),
                    executed,
                )
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

    /// Settle a pending charge against its gateway operation's actual cost:
    /// debit `tokens_burnt` (always) plus the locked `inner_spend` (only if the
    /// operation succeeded), and release the reservation.
    ///
    /// Private: the only caller is [`Database::resolve_charge`], which alone
    /// decides whether an operation is terminal enough to settle.
    ///
    /// Idempotent: the ledger delete means a charge already
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
WITH charge AS (
    DELETE FROM pending_gateway_charge
    WHERE
        account_id = $1
        AND operation_key = $2
    RETURNING
        inner_spend
)
UPDATE
    account
SET
    allowance = greatest(
        allowance - (
            $3 + CASE
                WHEN $4 THEN (SELECT inner_spend FROM charge)
                ELSE 0
            END
        ),
        0
    )
WHERE
    account_id = $1
    AND EXISTS (SELECT 1 FROM charge)
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
    /// never reached the chain. Only clears the ledger row; the allowance was never
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
DELETE FROM
    pending_gateway_charge
WHERE
    account_id = $1
    AND operation_key = $2
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
    account.allowance,
    account.mark AS "mark: AccountMark",
    coalesce(sum(pending_gateway_charge.gas_estimate + pending_gateway_charge.inner_spend), 0) AS "reserved!: Decimal"
FROM
    account
LEFT JOIN
    pending_gateway_charge ON pending_gateway_charge.account_id = account.account_id
WHERE
    account.account_id = $1
GROUP BY
    account.account_id
"#,
            account_id.as_str(),
        )
        .fetch_optional(&self.connection)
        .await?;

        result
            .map(|row| {
                if row.mark == AccountMark::AlwaysDeny {
                    return Ok(NearToken::from_yoctonear(0));
                }

                let available = if row.mark == AccountMark::AlwaysApprove {
                    row.allowance
                } else {
                    row.allowance - row.reserved
                };
                Ok(NearToken::from_yoctonear(
                    u128::try_from(available).unwrap_or(0),
                ))
            })
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use near_api::types::CryptoHash as NearCryptoHash;
    use templar_gateway_types::{
        operation::{ExecutionOutcome, StepStatus, TransactionStepRecord},
        CryptoHash, ManagedAccountId, NearGas, OperationId,
    };

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
    /// and `final_outcome` is `None`) — a planned no-op / reservation, enough to
    /// exercise resolve_charge's release/defer branches.
    fn operation(status: OperationStatus) -> OperationRecord {
        OperationRecord {
            id: OperationId("op".to_owned()),
            signer_account_id: ManagedAccountId("signer.near".parse().unwrap()),
            status,
            steps: vec![],
        }
    }

    /// A single-step operation that executed and succeeded, carrying an outcome (so
    /// `final_outcome` is `Some` and the locked deposit is billed) that burnt
    /// `tokens_burnt` in gas.
    fn executed_success(tokens_burnt: NearToken) -> OperationRecord {
        OperationRecord {
            id: OperationId("op".to_owned()),
            signer_account_id: ManagedAccountId("signer.near".parse().unwrap()),
            status: OperationStatus::Succeeded,
            steps: vec![TransactionStepRecord {
                index: 0,
                status: StepStatus::Succeeded {
                    tx_hash: CryptoHash(NearCryptoHash::default()),
                    outcome: ExecutionOutcome {
                        tokens_burnt,
                        total_gas_burnt: NearGas::from_gas(0),
                        receipts: vec![],
                        return_value: None,
                        failure: None,
                    },
                },
            }],
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
        assert_eq!(allowance(&db, &account).await, 85);

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

        db.lock_pending(&account, near(10), near(0), Uuid::new_v4())
            .await
            .unwrap();
    }

    /// Settling twice for the same charge debits once (the ledger delete makes the
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
        // The reservation remains outstanding while the operation is pending.
        assert_eq!(allowance(&db, &account).await, 85);
    }

    /// A succeeded operation that executed settles its recorded cost — here the
    /// locked deposit (5), with zero gas burnt in the outcome.
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
            .resolve_charge(&account, key, Some(&executed_success(near(0))))
            .await
            .unwrap();
        assert_eq!(status, AccountedStatus::Succeeded);
        assert_eq!(allowance(&db, &account).await, 100 - 5);
    }

    /// A planned no-op is terminal `Succeeded` with no outcome: it never attached
    /// the locked deposit, so settling clears the reservation without billing
    /// `inner_spend`. Guards against overcharging a no-op.
    #[sqlx::test]
    async fn resolve_charge_no_op_success_bills_nothing(pool: PgPool) {
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
        // The no-op spent nothing, so the locked deposit is not billed.
        assert_eq!(allowance(&db, &account).await, 100);
        db.lock_pending(&account, near(10), near(0), Uuid::new_v4())
            .await
            .unwrap();
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
        db.lock_pending(&account, near(10), near(0), Uuid::new_v4())
            .await
            .unwrap();
    }

    /// No operation (planning failed, or a reservation was reaped) releases the
    /// charge uncharged — the rule the dispatching endpoint's error path relies on
    /// so a failed plan never strands the account's reservation.
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
        db.lock_pending(&account, near(10), near(0), Uuid::new_v4())
            .await
            .unwrap();
    }

    /// Multiple gateway operations may reserve allowance for the same account.
    #[sqlx::test]
    async fn lock_allows_multiple_pending_charges_for_same_account(pool: PgPool) {
        let db = db(pool);
        let account = acct("a.near");
        db.create_account(&account, near(100)).await.unwrap();

        let first = Uuid::new_v4();
        let second = Uuid::new_v4();

        db.lock_pending(&account, near(10), near(5), first)
            .await
            .unwrap();
        db.lock_pending(&account, near(20), near(7), second)
            .await
            .unwrap();

        let mut pending = db.get_pending_charges(10, Duration::ZERO).await.unwrap();
        pending.sort_by_key(|charge| charge.operation_key);
        let mut keys = [first, second];
        keys.sort();

        assert_eq!(pending.len(), 2);
        assert!(pending.iter().all(|charge| charge.account_id == account));
        assert_eq!(
            pending
                .iter()
                .map(|charge| charge.operation_key)
                .collect::<Vec<_>>(),
            keys
        );
        assert_eq!(allowance(&db, &account).await, 58);
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

    /// Existing reservations reduce what a later gateway operation can reserve.
    #[sqlx::test]
    async fn lock_rejects_when_existing_reservations_exhaust_allowance(pool: PgPool) {
        let db = db(pool);
        let account = acct("a.near");
        db.create_account(&account, near(30)).await.unwrap();

        db.lock_pending(&account, near(10), near(5), Uuid::new_v4())
            .await
            .unwrap();

        let err = db
            .lock_pending(&account, near(12), near(4), Uuid::new_v4())
            .await
            .unwrap_err();
        assert!(matches!(err, error::LockError::InsufficientAllowance(_)));
        assert_eq!(allowance(&db, &account).await, 15);
    }

    /// Available allowance reports the spendable balance after pending reservations.
    #[sqlx::test]
    async fn available_allowance_subtracts_pending_reservations(pool: PgPool) {
        let db = db(pool);
        let account = acct("a.near");
        db.create_account(&account, near(100)).await.unwrap();

        db.lock_pending(&account, near(10), near(5), Uuid::new_v4())
            .await
            .unwrap();
        db.lock_pending(&account, near(20), near(7), Uuid::new_v4())
            .await
            .unwrap();

        assert_eq!(allowance(&db, &account).await, 58);
    }

    /// Settling or releasing one operation key leaves sibling reservations intact.
    #[sqlx::test]
    async fn settle_and_release_one_charge_leave_other_pending(pool: PgPool) {
        let db = db(pool);
        let account = acct("a.near");
        db.create_account(&account, near(100)).await.unwrap();

        let settled = Uuid::new_v4();
        let released = Uuid::new_v4();
        db.lock_pending(&account, near(10), near(5), settled)
            .await
            .unwrap();
        db.lock_pending(&account, near(20), near(7), released)
            .await
            .unwrap();

        db.settle(&account, settled, near(8), true).await.unwrap();

        let pending = db.get_pending_charges(10, Duration::ZERO).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].operation_key, released);
        assert_eq!(allowance(&db, &account).await, 60);

        db.release_pending(&account, released).await.unwrap();
        assert!(db
            .get_pending_charges(10, Duration::ZERO)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(allowance(&db, &account).await, 87);
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

    /// Locking a charge stamps the ledger row's `updated_at`, so a new reservation
    /// is never immediately broom-eligible.
    #[sqlx::test]
    async fn lock_pending_refreshes_charge_age(pool: PgPool) {
        let db = db(pool);
        let account = acct("a.near");
        db.create_account(&account, near(100)).await.unwrap();

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

        let key = Uuid::new_v4();
        db.lock_pending(&account, near(10), near(5), key)
            .await
            .unwrap();

        let pending = db
            .get_pending_charges(10, Duration::from_secs(60))
            .await
            .unwrap();
        assert!(
            pending.is_empty(),
            "a just-locked charge must not be broom-eligible until it ages past min_age"
        );
    }

    /// Broom age is evaluated per ledger row rather than per account.
    #[sqlx::test]
    async fn get_pending_charges_filters_by_ledger_updated_at(pool: PgPool) {
        let db = db(pool);
        let account = acct("a.near");
        db.create_account(&account, near(100)).await.unwrap();

        let stale = Uuid::new_v4();
        let fresh = Uuid::new_v4();
        db.lock_pending(&account, near(10), near(0), stale)
            .await
            .unwrap();
        db.lock_pending(&account, near(10), near(0), fresh)
            .await
            .unwrap();

        sqlx::query("ALTER TABLE pending_gateway_charge DISABLE TRIGGER updated_at_trigger")
            .execute(&db.connection)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE pending_gateway_charge SET updated_at = NOW() - INTERVAL '1 hour' WHERE operation_key = $1",
        )
        .bind(stale)
        .execute(&db.connection)
        .await
        .unwrap();
        sqlx::query("ALTER TABLE pending_gateway_charge ENABLE TRIGGER updated_at_trigger")
            .execute(&db.connection)
            .await
            .unwrap();

        let pending = db
            .get_pending_charges(10, Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].operation_key, stale);
    }
}
