use std::{collections::VecDeque, str::FromStr};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use near_api::types::transaction::SignedTransaction;
use serde_json::Value;
use sqlx::{postgres::PgPoolOptions, PgPool};
use templar_gateway_types::{
    operation::OperationId, CryptoHash, IdempotencyKey, ManagedAccountId, OperationStatus,
};

use crate::{
    operation::{OperationPlan, PlannedTransaction, StoredOperation, SucceededStep},
    store::{CreateOperationResult, OperationStore},
    GatewayError, GatewayResult,
};

#[derive(Debug, Clone)]
pub struct PostgresStore {
    pool: PgPool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "gateway_operation_status", rename_all = "snake_case")]
enum OperationStatusRow {
    Pending,
    InProgress,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "gateway_operation_step_state", rename_all = "snake_case")]
enum OperationStepStateRow {
    NotStarted,
    Prepared,
    Submitted,
    Succeeded,
    Failed,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct OperationRow {
    id: uuid::Uuid,
    rpc_method: String,
    signer_account_id: String,
    idempotency_key: Option<String>,
    request_fingerprint_hash: Vec<u8>,
    request_payload: Value,
    status: OperationStatusRow,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct OperationStepRow {
    operation_id: uuid::Uuid,
    step_index: i32,
    signer_account_id: String,
    receiver_id: String,
    wait_until: String,
    actions: Value,
    state: OperationStepStateRow,
    tx_hash: Option<String>,
    signed_transaction: Option<Vec<u8>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl PostgresStore {
    #[allow(dead_code)]
    pub fn new(database_url: &str) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect_lazy(database_url)?;
        Ok(Self { pool })
    }

    #[allow(dead_code)]
    pub async fn migrate(&self) -> Result<(), sqlx::migrate::MigrateError> {
        sqlx::migrate!("./migrations").run(&self.pool).await
    }

    #[allow(dead_code)]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

#[async_trait]
impl OperationStore for PostgresStore {
    async fn get_by_id(
        &self,
        operation_id: &OperationId,
    ) -> GatewayResult<Option<StoredOperation>> {
        let operation_uuid = uuid::Uuid::from_str(&operation_id.0)
            .map_err(|error| GatewayError::InvalidStoredOperation(error.to_string()))?;
        let Some(row) = sqlx::query!(
            r#"
            SELECT
                id,
                rpc_method,
                signer_account_id,
                idempotency_key,
                request_fingerprint_hash,
                request_payload,
                status as "status: OperationStatusRow",
                created_at,
                updated_at
            FROM gateway_operations
            WHERE id = $1
            "#,
            operation_uuid,
        )
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        let operation_row = OperationRow {
            id: row.id,
            rpc_method: row.rpc_method,
            signer_account_id: row.signer_account_id,
            idempotency_key: row.idempotency_key,
            request_fingerprint_hash: row.request_fingerprint_hash,
            request_payload: row.request_payload,
            status: row.status,
            created_at: row.created_at,
            updated_at: row.updated_at,
        };

        let step_rows = load_step_rows(&self.pool, operation_row.id).await?;
        rows_to_stored_operation(operation_row, step_rows).map(Some)
    }

    async fn get_by_idempotency_key(
        &self,
        idempotency_key: &IdempotencyKey,
    ) -> GatewayResult<Option<StoredOperation>> {
        let Some(row) = sqlx::query!(
            r#"
            SELECT
                id,
                rpc_method,
                signer_account_id,
                idempotency_key,
                request_fingerprint_hash,
                request_payload,
                status as "status: OperationStatusRow",
                created_at,
                updated_at
            FROM gateway_operations
            WHERE idempotency_key = $1
            "#,
            idempotency_key.0.as_str(),
        )
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        let operation_row = OperationRow {
            id: row.id,
            rpc_method: row.rpc_method,
            signer_account_id: row.signer_account_id,
            idempotency_key: row.idempotency_key,
            request_fingerprint_hash: row.request_fingerprint_hash,
            request_payload: row.request_payload,
            status: row.status,
            created_at: row.created_at,
            updated_at: row.updated_at,
        };

        let step_rows = load_step_rows(&self.pool, operation_row.id).await?;
        rows_to_stored_operation(operation_row, step_rows).map(Some)
    }

    async fn create_or_get_operation(
        &self,
        rpc_method: &str,
        signer_account_id: ManagedAccountId,
        idempotency_key: Option<IdempotencyKey>,
        request_fingerprint_hash: [u8; 32],
        request_payload: Vec<u8>,
        plan: OperationPlan,
    ) -> GatewayResult<CreateOperationResult> {
        let operation = StoredOperation {
            rpc_method: rpc_method.to_owned(),
            request_fingerprint_hash,
            request_payload,
            id: OperationId(uuid::Uuid::new_v4().to_string()),
            signer_account_id,
            succeeded_steps: vec![],
            current_step: None,
            remaining_steps: VecDeque::from(plan.steps),
        };

        match save_operation_tx(&self.pool, &operation, idempotency_key.as_ref(), None).await {
            Ok(()) => Ok(CreateOperationResult::Created(operation)),
            Err(GatewayError::Sql(sqlx::Error::Database(database_error)))
                if database_error.constraint()
                    == Some("gateway_operations_idempotency_key_unique") =>
            {
                let key = idempotency_key.expect("idempotency key should exist on unique conflict");
                let existing = self.get_by_idempotency_key(&key).await?.ok_or_else(|| {
                    GatewayError::InvalidStoredOperation(
                        "idempotency conflict without existing operation".to_owned(),
                    )
                })?;
                if existing.request_fingerprint_hash != operation.request_fingerprint_hash {
                    return Err(GatewayError::IdempotencyConflict);
                }
                Ok(CreateOperationResult::Existing(existing))
            }
            Err(error) => Err(error),
        }
    }

    async fn save_operation(&self, operation: StoredOperation) -> GatewayResult<()> {
        save_operation_tx(&self.pool, &operation, None, Some(&operation.id)).await
    }

    async fn list_incomplete_operations(&self) -> GatewayResult<Vec<StoredOperation>> {
        let rows = sqlx::query!(
            r#"
            SELECT
                id,
                rpc_method,
                signer_account_id,
                idempotency_key,
                request_fingerprint_hash,
                request_payload,
                status as "status: OperationStatusRow",
                created_at,
                updated_at
            FROM gateway_operations
            WHERE status IN ('pending', 'in_progress')
            ORDER BY created_at ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let mut operations = Vec::with_capacity(rows.len());
        for row in rows {
            let operation_row = OperationRow {
                id: row.id,
                rpc_method: row.rpc_method,
                signer_account_id: row.signer_account_id,
                idempotency_key: row.idempotency_key,
                request_fingerprint_hash: row.request_fingerprint_hash,
                request_payload: row.request_payload,
                status: row.status,
                created_at: row.created_at,
                updated_at: row.updated_at,
            };
            let steps = load_step_rows(&self.pool, operation_row.id).await?;
            operations.push(rows_to_stored_operation(operation_row, steps)?);
        }
        Ok(operations)
    }
}

async fn load_step_rows(
    pool: &PgPool,
    operation_id: uuid::Uuid,
) -> GatewayResult<Vec<OperationStepRow>> {
    let rows = sqlx::query!(
        r#"
        SELECT
            operation_id,
            step_index,
            signer_account_id,
            receiver_id,
            wait_until,
            actions,
            state as "state: OperationStepStateRow",
            tx_hash,
            signed_transaction,
            created_at,
            updated_at
        FROM gateway_operation_steps
        WHERE operation_id = $1
        ORDER BY step_index ASC
        "#,
        operation_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| OperationStepRow {
            operation_id: row.operation_id,
            step_index: row.step_index,
            signer_account_id: row.signer_account_id,
            receiver_id: row.receiver_id,
            wait_until: row.wait_until,
            actions: row.actions,
            state: row.state,
            tx_hash: row.tx_hash,
            signed_transaction: row.signed_transaction,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
        .collect())
}

async fn save_operation_tx(
    pool: &PgPool,
    operation: &StoredOperation,
    create_idempotency_key: Option<&IdempotencyKey>,
    existing_operation_id: Option<&OperationId>,
) -> GatewayResult<()> {
    let mut tx = pool.begin().await?;

    let status = operation_status_row(operation.status());
    let operation_uuid = uuid::Uuid::from_str(&operation.id.0)
        .map_err(|error| GatewayError::InvalidStoredOperation(error.to_string()))?;
    let now = Utc::now();
    let step_rows = stored_operation_to_step_rows(operation)?;

    if existing_operation_id.is_none() {
        let payload_json: Value =
            serde_json::from_slice(&operation.request_payload).map_err(|error| {
                GatewayError::InvalidStoredOperation(format!(
                    "invalid stored request payload json: {error}"
                ))
            })?;
        sqlx::query!(
            r#"
            INSERT INTO gateway_operations (
                id,
                rpc_method,
                signer_account_id,
                idempotency_key,
                request_fingerprint_hash,
                request_payload,
                status,
                updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
            operation_uuid,
            operation.rpc_method.as_str(),
            operation.signer_account_id.0.as_str(),
            create_idempotency_key.map(|key| key.0.as_str()),
            operation.request_fingerprint_hash.to_vec(),
            payload_json,
            status as OperationStatusRow,
            now,
        )
        .execute(&mut *tx)
        .await?;
    } else {
        validate_existing_operation_shape(&mut tx, operation_uuid, operation, &step_rows).await?;

        sqlx::query!(
            r#"
            UPDATE gateway_operations
            SET status = $2,
                updated_at = $3
            WHERE id = $1
            "#,
            operation_uuid,
            status as OperationStatusRow,
            now,
        )
        .execute(&mut *tx)
        .await?;
    }

    for (index, step_row) in step_rows.into_iter().enumerate() {
        sqlx::query!(
            r#"
            INSERT INTO gateway_operation_steps (
                operation_id,
                step_index,
                signer_account_id,
                receiver_id,
                wait_until,
                actions,
                state,
                tx_hash,
                signed_transaction,
                updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (operation_id, step_index) DO UPDATE
            SET state = EXCLUDED.state,
                tx_hash = EXCLUDED.tx_hash,
                signed_transaction = EXCLUDED.signed_transaction,
                updated_at = EXCLUDED.updated_at
            "#,
            operation_uuid,
            index as i32,
            step_row.signer_account_id,
            step_row.receiver_id,
            step_row.wait_until,
            step_row.actions,
            step_row.state as OperationStepStateRow,
            step_row.tx_hash,
            step_row.signed_transaction,
            now,
        )
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

#[derive(Debug)]
struct ExistingStepShapeRow {
    step_index: i32,
    signer_account_id: String,
    receiver_id: String,
    wait_until: String,
    actions: Value,
}

async fn validate_existing_operation_shape(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operation_uuid: uuid::Uuid,
    operation: &StoredOperation,
    step_rows: &[PersistedStepRow],
) -> GatewayResult<()> {
    let existing = sqlx::query!(
        r#"
        SELECT
            rpc_method,
            signer_account_id,
            request_fingerprint_hash,
            request_payload
        FROM gateway_operations
        WHERE id = $1
        "#,
        operation_uuid,
    )
    .fetch_one(&mut **tx)
    .await?;

    let payload_json: Value =
        serde_json::from_slice(&operation.request_payload).map_err(|error| {
            GatewayError::InvalidStoredOperation(format!(
                "invalid stored request payload json: {error}"
            ))
        })?;

    if existing.rpc_method != operation.rpc_method
        || existing.signer_account_id != operation.signer_account_id.0.as_str()
        || existing.request_fingerprint_hash != operation.request_fingerprint_hash.to_vec()
        || existing.request_payload != payload_json
    {
        return Err(GatewayError::InvalidStoredOperation(
            "operation immutable metadata changed".to_owned(),
        ));
    }

    let existing_steps = sqlx::query!(
        r#"
        SELECT
            step_index,
            signer_account_id,
            receiver_id,
            wait_until,
            actions
        FROM gateway_operation_steps
        WHERE operation_id = $1
        ORDER BY step_index ASC
        "#,
        operation_uuid,
    )
    .fetch_all(&mut **tx)
    .await?;

    if existing_steps.len() != step_rows.len() {
        return Err(GatewayError::InvalidStoredOperation(
            "operation step count changed".to_owned(),
        ));
    }

    for (index, (existing, current)) in existing_steps.into_iter().zip(step_rows.iter()).enumerate()
    {
        let existing = ExistingStepShapeRow {
            step_index: existing.step_index,
            signer_account_id: existing.signer_account_id,
            receiver_id: existing.receiver_id,
            wait_until: existing.wait_until,
            actions: existing.actions,
        };

        if existing.step_index != index as i32
            || existing.signer_account_id != current.signer_account_id
            || existing.receiver_id != current.receiver_id
            || existing.wait_until != current.wait_until
            || existing.actions != current.actions
        {
            return Err(GatewayError::InvalidStoredOperation(
                "operation step plan changed".to_owned(),
            ));
        }
    }

    Ok(())
}

struct PersistedStepRow {
    signer_account_id: String,
    receiver_id: String,
    wait_until: String,
    actions: Value,
    state: OperationStepStateRow,
    tx_hash: Option<String>,
    signed_transaction: Option<Vec<u8>>,
}

fn stored_operation_to_step_rows(
    operation: &StoredOperation,
) -> GatewayResult<Vec<PersistedStepRow>> {
    let mut rows = Vec::new();

    for step in &operation.succeeded_steps {
        rows.push(persisted_step_row(
            &step.transaction,
            OperationStepStateRow::Succeeded,
            Some(step.tx_hash),
            None,
        )?);
    }

    if let Some(current) = &operation.current_step {
        match current {
            crate::operation::CurrentStep::Prepared {
                transaction,
                signed_transaction,
                tx_hash,
            } => rows.push(persisted_step_row(
                transaction,
                OperationStepStateRow::Prepared,
                Some(*tx_hash),
                Some(serde_json::to_vec(signed_transaction)?),
            )?),
            crate::operation::CurrentStep::Submitted {
                transaction,
                tx_hash,
            } => rows.push(persisted_step_row(
                transaction,
                OperationStepStateRow::Submitted,
                Some(*tx_hash),
                None,
            )?),
            crate::operation::CurrentStep::Failed {
                transaction,
                tx_hash,
            } => rows.push(persisted_step_row(
                transaction,
                OperationStepStateRow::Failed,
                *tx_hash,
                None,
            )?),
        }
    }

    for transaction in &operation.remaining_steps {
        rows.push(persisted_step_row(
            transaction,
            OperationStepStateRow::NotStarted,
            None,
            None,
        )?);
    }

    Ok(rows)
}

fn persisted_step_row(
    transaction: &PlannedTransaction,
    state: OperationStepStateRow,
    tx_hash: Option<CryptoHash>,
    signed_transaction: Option<Vec<u8>>,
) -> GatewayResult<PersistedStepRow> {
    Ok(PersistedStepRow {
        signer_account_id: transaction.signer_account_id.0.to_string(),
        receiver_id: transaction.receiver_id.to_string(),
        wait_until: serde_json::to_string(&transaction.wait_until)?,
        actions: serde_json::to_value(&transaction.actions)?,
        state,
        tx_hash: tx_hash.map(|hash| hash.0.to_string()),
        signed_transaction,
    })
}

fn rows_to_stored_operation(
    operation_row: OperationRow,
    step_rows: Vec<OperationStepRow>,
) -> GatewayResult<StoredOperation> {
    let signer_account_id =
        ManagedAccountId(operation_row.signer_account_id.parse().map_err(|error| {
            GatewayError::InvalidStoredOperation(format!("invalid signer account id: {error}"))
        })?);
    let request_payload = serde_json::to_vec(&operation_row.request_payload)?;
    let request_fingerprint_hash: [u8; 32] = operation_row
        .request_fingerprint_hash
        .try_into()
        .map_err(|_| {
            GatewayError::InvalidStoredOperation("invalid fingerprint length".to_owned())
        })?;

    validate_step_sequence(&step_rows)?;

    let mut succeeded_steps = Vec::new();
    let mut current_step = None;
    let mut remaining_steps = VecDeque::new();

    for row in step_rows {
        let transaction = planned_transaction_from_row(&row)?;
        match row.state {
            OperationStepStateRow::Succeeded => succeeded_steps.push(SucceededStep {
                transaction,
                tx_hash: parse_tx_hash(row.tx_hash.as_deref())?
                    .expect("succeeded row requires tx hash"),
            }),
            OperationStepStateRow::Prepared => {
                let signed_transaction = serde_json::from_slice::<SignedTransaction>(
                    row.signed_transaction.as_deref().ok_or_else(|| {
                        GatewayError::InvalidStoredOperation(
                            "prepared step missing signed tx".to_owned(),
                        )
                    })?,
                )?;
                current_step = Some(crate::operation::CurrentStep::Prepared {
                    transaction,
                    signed_transaction: Box::new(signed_transaction),
                    tx_hash: parse_tx_hash(row.tx_hash.as_deref())?
                        .expect("prepared row requires tx hash"),
                });
            }
            OperationStepStateRow::Submitted => {
                current_step = Some(crate::operation::CurrentStep::Submitted {
                    transaction,
                    tx_hash: parse_tx_hash(row.tx_hash.as_deref())?
                        .expect("submitted row requires tx hash"),
                });
            }
            OperationStepStateRow::Failed => {
                current_step = Some(crate::operation::CurrentStep::Failed {
                    transaction,
                    tx_hash: parse_tx_hash(row.tx_hash.as_deref())?,
                });
            }
            OperationStepStateRow::NotStarted => {
                remaining_steps.push_back(transaction);
            }
        }
    }

    let operation = StoredOperation {
        rpc_method: operation_row.rpc_method,
        request_fingerprint_hash,
        request_payload,
        id: OperationId(operation_row.id.to_string()),
        signer_account_id,
        succeeded_steps,
        current_step,
        remaining_steps,
    };

    let persisted_status = operation_status_from_row(operation_row.status);
    let derived_status = operation.status();
    if persisted_status != derived_status {
        return Err(GatewayError::InvalidStoredOperation(format!(
            "persisted operation status mismatch: persisted={persisted_status:?} derived={derived_status:?}"
        )));
    }

    Ok(operation)
}

fn validate_step_sequence(step_rows: &[OperationStepRow]) -> GatewayResult<()> {
    let mut seen_current = false;
    let mut seen_remaining = false;

    for (expected_index, row) in step_rows.iter().enumerate() {
        if row.step_index != expected_index as i32 {
            return Err(GatewayError::InvalidStoredOperation(
                "step indices must be contiguous".to_owned(),
            ));
        }

        match row.state {
            OperationStepStateRow::Succeeded if seen_current || seen_remaining => {
                return Err(GatewayError::InvalidStoredOperation(
                    "succeeded steps must form a contiguous prefix".to_owned(),
                ));
            }
            OperationStepStateRow::Prepared
            | OperationStepStateRow::Submitted
            | OperationStepStateRow::Failed => {
                if seen_current || seen_remaining {
                    return Err(GatewayError::InvalidStoredOperation(
                        "at most one current step may exist".to_owned(),
                    ));
                }
                seen_current = true;
            }
            OperationStepStateRow::NotStarted => {
                seen_remaining = true;
            }
            OperationStepStateRow::Succeeded => {}
        }
    }

    Ok(())
}

fn planned_transaction_from_row(row: &OperationStepRow) -> GatewayResult<PlannedTransaction> {
    Ok(PlannedTransaction {
        signer_account_id: ManagedAccountId(row.signer_account_id.parse().map_err(|error| {
            GatewayError::InvalidStoredOperation(format!("invalid step signer account id: {error}"))
        })?),
        wait_until: serde_json::from_str(&row.wait_until)?,
        receiver_id: row.receiver_id.parse().map_err(|error| {
            GatewayError::InvalidStoredOperation(format!("invalid receiver account id: {error}"))
        })?,
        actions: serde_json::from_value(row.actions.clone())?,
    })
}

fn parse_tx_hash(tx_hash: Option<&str>) -> GatewayResult<Option<CryptoHash>> {
    tx_hash
        .map(|hash| {
            hash.parse().map(CryptoHash).map_err(|error| {
                GatewayError::InvalidStoredOperation(format!("invalid tx hash: {error}"))
            })
        })
        .transpose()
}

fn operation_status_row(status: OperationStatus) -> OperationStatusRow {
    match status {
        OperationStatus::Pending => OperationStatusRow::Pending,
        OperationStatus::InProgress => OperationStatusRow::InProgress,
        OperationStatus::Succeeded => OperationStatusRow::Succeeded,
        OperationStatus::Failed => OperationStatusRow::Failed,
    }
}

fn operation_status_from_row(status: OperationStatusRow) -> OperationStatus {
    match status {
        OperationStatusRow::Pending => OperationStatus::Pending,
        OperationStatusRow::InProgress => OperationStatus::InProgress,
        OperationStatusRow::Succeeded => OperationStatus::Succeeded,
        OperationStatusRow::Failed => OperationStatus::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_api::types::transaction::actions::{Action, TransferAction};
    use near_api::types::CryptoHash as NearCryptoHash;
    use templar_gateway_types::{common::TxExecutionStatus, NearToken};

    fn sample_transaction(index: u8) -> PlannedTransaction {
        PlannedTransaction::single_action(
            ManagedAccountId(format!("signer-{index}.near").parse().unwrap()),
            TxExecutionStatus::ExecutedOptimistic,
            format!("receiver-{index}.near").parse().unwrap(),
            Action::Transfer(TransferAction {
                deposit: NearToken::from_yoctonear(index as u128 + 1),
            }),
        )
    }

    fn sample_operation_row(status: OperationStatusRow) -> OperationRow {
        OperationRow {
            id: uuid::Uuid::new_v4(),
            rpc_method: "tx.functionCall".to_owned(),
            signer_account_id: "signer-0.near".to_owned(),
            idempotency_key: Some("key".to_owned()),
            request_fingerprint_hash: vec![7; 32],
            request_payload: serde_json::json!({"hello": "world"}),
            status,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn row_roundtrip_preserves_succeeded_submitted_remaining_shape() {
        let operation = StoredOperation {
            rpc_method: "tx.functionCall".to_owned(),
            request_fingerprint_hash: [7; 32],
            request_payload: serde_json::to_vec(&serde_json::json!({"hello": "world"})).unwrap(),
            id: OperationId(uuid::Uuid::new_v4().to_string()),
            signer_account_id: ManagedAccountId("signer-0.near".parse().unwrap()),
            succeeded_steps: vec![SucceededStep {
                transaction: sample_transaction(0),
                tx_hash: NearCryptoHash([1; 32]).into(),
            }],
            current_step: Some(crate::operation::CurrentStep::Submitted {
                transaction: sample_transaction(1),
                tx_hash: NearCryptoHash([2; 32]).into(),
            }),
            remaining_steps: VecDeque::from(vec![sample_transaction(2)]),
        };

        let rows = stored_operation_to_step_rows(&operation).unwrap();
        let restored = rows_to_stored_operation(
            sample_operation_row(OperationStatusRow::InProgress),
            rows.into_iter()
                .enumerate()
                .map(|(i, row)| OperationStepRow {
                    operation_id: uuid::Uuid::nil(),
                    step_index: i as i32,
                    signer_account_id: row.signer_account_id,
                    receiver_id: row.receiver_id,
                    wait_until: row.wait_until,
                    actions: row.actions,
                    state: row.state,
                    tx_hash: row.tx_hash,
                    signed_transaction: row.signed_transaction,
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                })
                .collect(),
        )
        .unwrap();

        assert_eq!(restored.succeeded_steps.len(), 1);
        assert!(matches!(
            restored.current_step,
            Some(crate::operation::CurrentStep::Submitted { .. })
        ));
        assert_eq!(restored.remaining_steps.len(), 1);
    }

    #[test]
    fn invalid_step_sequence_is_rejected() {
        let result = rows_to_stored_operation(
            sample_operation_row(OperationStatusRow::InProgress),
            vec![
                OperationStepRow {
                    operation_id: uuid::Uuid::nil(),
                    step_index: 0,
                    signer_account_id: "signer-0.near".to_owned(),
                    receiver_id: "receiver-0.near".to_owned(),
                    wait_until: serde_json::to_string(&TxExecutionStatus::ExecutedOptimistic)
                        .unwrap(),
                    actions: serde_json::to_value(vec![Action::Transfer(TransferAction {
                        deposit: NearToken::from_yoctonear(1),
                    })])
                    .unwrap(),
                    state: OperationStepStateRow::NotStarted,
                    tx_hash: None,
                    signed_transaction: None,
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                },
                OperationStepRow {
                    operation_id: uuid::Uuid::nil(),
                    step_index: 1,
                    signer_account_id: "signer-1.near".to_owned(),
                    receiver_id: "receiver-1.near".to_owned(),
                    wait_until: serde_json::to_string(&TxExecutionStatus::ExecutedOptimistic)
                        .unwrap(),
                    actions: serde_json::to_value(vec![Action::Transfer(TransferAction {
                        deposit: NearToken::from_yoctonear(2),
                    })])
                    .unwrap(),
                    state: OperationStepStateRow::Succeeded,
                    tx_hash: Some("11111111111111111111111111111111".to_owned()),
                    signed_transaction: None,
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                },
            ],
        );

        assert!(result.is_err());
    }
}
