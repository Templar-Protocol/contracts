use std::{collections::VecDeque, str::FromStr};

use async_trait::async_trait;
use borsh::{to_vec, BorshDeserialize};
use chrono::{DateTime, Utc};
use near_api::types::transaction::SignedTransaction;
use near_api::types::CryptoHash as NearCryptoHash;
use serde_json::Value;
use sqlx::{postgres::PgPoolOptions, PgPool};
use templar_gateway_core::{
    CreateOperationResult, GatewayError, GatewayResult, OperationPlan, OperationStore,
    PlannedTransaction, StoredOperation, SucceededStep,
};
use templar_gateway_types::{
    operation::OperationId, CryptoHash, IdempotencyKey, ManagedAccountId, OperationStatus,
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

#[derive(Debug, Clone)]
struct OperationRow {
    id: uuid::Uuid,
    rpc_method: String,
    signer_account_id: String,
    #[allow(
        dead_code,
        reason = "loaded from the audit table for row-shape completeness"
    )]
    idempotency_key: Option<String>,
    request_fingerprint_hash: Vec<u8>,
    request_payload: Value,
    status: OperationStatusRow,
    #[allow(
        dead_code,
        reason = "operation audit timestamp retained in the row DTO"
    )]
    created_at: DateTime<Utc>,
    #[allow(
        dead_code,
        reason = "operation audit timestamp retained in the row DTO"
    )]
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct OperationStepRow {
    #[allow(
        dead_code,
        reason = "loaded from the audit table for row-shape completeness"
    )]
    operation_id: uuid::Uuid,
    #[allow(dead_code, reason = "step ordering metadata retained in the row DTO")]
    step_index: i32,
    signer_account_id: String,
    receiver_id: String,
    wait_until: String,
    actions: Value,
    state: OperationStepStateRow,
    tx_hash: Option<String>,
    signed_transaction: Option<Vec<u8>>,
    #[allow(dead_code, reason = "step audit timestamp retained in the row DTO")]
    created_at: DateTime<Utc>,
    #[allow(dead_code, reason = "step audit timestamp retained in the row DTO")]
    updated_at: DateTime<Utc>,
}

impl PostgresStore {
    pub fn new(database_url: &str) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect_lazy(database_url)?;
        Ok(Self { pool })
    }

    pub async fn migrate(&self) -> Result<(), sqlx::migrate::MigrateError> {
        sqlx::migrate!("./migrations").run(&self.pool).await
    }

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
    STATUS AS "status: OperationStatusRow",
    created_at,
    updated_at
FROM
    gateway_operations
WHERE
    id = $1
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
    STATUS AS "status: OperationStatusRow",
    created_at,
    updated_at
FROM
    gateway_operations
WHERE
    idempotency_key = $1
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
                let Some(key) = idempotency_key else {
                    return Err(GatewayError::InvalidStoredOperation(
                        "idempotency unique conflict without idempotency key".to_owned(),
                    ));
                };
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
    STATUS AS "status: OperationStatusRow",
    created_at,
    updated_at
FROM
    gateway_operations
WHERE
    STATUS IN ('pending', 'in_progress')
ORDER BY
    created_at ASC
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

async fn save_operation_tx(
    pool: &PgPool,
    operation: &StoredOperation,
    idempotency_key: Option<&IdempotencyKey>,
    replace_operation_id: Option<&OperationId>,
) -> GatewayResult<()> {
    let mut tx = pool.begin().await?;

    if let Some(operation_id) = replace_operation_id {
        let operation_uuid = uuid::Uuid::from_str(&operation_id.0)
            .map_err(|error| GatewayError::InvalidStoredOperation(error.to_string()))?;

        sqlx::query!(
            r#"
DELETE FROM
    gateway_operation_steps
WHERE
    operation_id = $1
"#,
            operation_uuid,
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query!(
            r#"
DELETE FROM
    gateway_operations
WHERE
    id = $1
"#,
            operation_uuid,
        )
        .execute(&mut *tx)
        .await?;
    }

    let operation_uuid = uuid::Uuid::from_str(&operation.id.0)
        .map_err(|error| GatewayError::InvalidStoredOperation(error.to_string()))?;
    insert_operation_row(&mut tx, operation_uuid, operation, idempotency_key).await?;
    insert_operation_steps(&mut tx, operation_uuid, operation).await?;

    tx.commit().await?;
    Ok(())
}

async fn insert_operation_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operation_uuid: uuid::Uuid,
    operation: &StoredOperation,
    idempotency_key: Option<&IdempotencyKey>,
) -> GatewayResult<()> {
    let status = operation_status_row(operation.status());
    let request_payload = serde_json::from_slice::<Value>(&operation.request_payload)
        .map_err(GatewayError::JsonSerialization)?;

    sqlx::query!(
        r#"
INSERT INTO
    gateway_operations (
        id,
        rpc_method,
        signer_account_id,
        idempotency_key,
        request_fingerprint_hash,
        request_payload,
        STATUS
    )
VALUES
    ($1, $2, $3, $4, $5, $6, $7)
"#,
        operation_uuid,
        operation.rpc_method,
        operation.signer_account_id.0.to_string(),
        idempotency_key.map(|key| key.0.as_str()),
        operation.request_fingerprint_hash.as_slice(),
        request_payload,
        status as OperationStatusRow,
    )
    .execute(&mut **tx)
    .await?;

    Ok(())
}

async fn insert_operation_steps(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operation_uuid: uuid::Uuid,
    operation: &StoredOperation,
) -> GatewayResult<()> {
    for (index, step) in operation.succeeded_steps.iter().enumerate() {
        insert_step_row(
            tx,
            operation_uuid,
            step_index(index)?,
            &step.transaction,
            OperationStepStateRow::Succeeded,
            Some(step.tx_hash),
            None,
        )
        .await?;
    }

    let mut current_index = step_index(operation.succeeded_steps.len())?;
    if let Some(current_step) = &operation.current_step {
        match current_step {
            templar_gateway_core::CurrentStep::Prepared {
                transaction,
                signed_transaction,
                tx_hash,
            } => {
                insert_step_row(
                    tx,
                    operation_uuid,
                    current_index,
                    transaction,
                    OperationStepStateRow::Prepared,
                    Some(*tx_hash),
                    Some(signed_transaction),
                )
                .await?;
            }
            templar_gateway_core::CurrentStep::Submitted {
                transaction,
                tx_hash,
            } => {
                insert_step_row(
                    tx,
                    operation_uuid,
                    current_index,
                    transaction,
                    OperationStepStateRow::Submitted,
                    Some(*tx_hash),
                    None,
                )
                .await?;
            }
            templar_gateway_core::CurrentStep::Failed {
                transaction,
                tx_hash,
                // The failure reason is surfaced in-process (in the returned
                // operation record); it is not persisted, so a step resumed from
                // the store reports `failure: None`.
                failure: _,
            } => {
                insert_step_row(
                    tx,
                    operation_uuid,
                    current_index,
                    transaction,
                    OperationStepStateRow::Failed,
                    Some(*tx_hash),
                    None,
                )
                .await?;
            }
        }
        current_index += 1;
    }

    for (offset, step) in operation.remaining_steps.iter().enumerate() {
        insert_step_row(
            tx,
            operation_uuid,
            current_index + step_index(offset)?,
            step,
            OperationStepStateRow::NotStarted,
            None,
            None,
        )
        .await?;
    }

    Ok(())
}

fn step_index(index: usize) -> GatewayResult<i32> {
    i32::try_from(index).map_err(|_| {
        GatewayError::InvalidStoredOperation("operation step index exceeds i32 range".to_owned())
    })
}

async fn insert_step_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operation_id: uuid::Uuid,
    step_index: i32,
    transaction: &PlannedTransaction,
    state: OperationStepStateRow,
    tx_hash: Option<CryptoHash>,
    signed_transaction: Option<&SignedTransaction>,
) -> GatewayResult<()> {
    let actions = serde_json::to_value(&transaction.actions)?;
    let signed_transaction = signed_transaction
        .map(to_vec)
        .transpose()
        .map_err(|error| GatewayError::InvalidStoredOperation(error.to_string()))?;

    sqlx::query!(
        r#"
INSERT INTO
    gateway_operation_steps (
        operation_id,
        step_index,
        signer_account_id,
        receiver_id,
        wait_until,
        actions,
        state,
        tx_hash,
        signed_transaction
    )
VALUES
    ($1, $2, $3, $4, $5, $6, $7, $8, $9)
"#,
        operation_id,
        step_index,
        transaction.signer_account_id.0.to_string(),
        transaction.receiver_id.to_string(),
        serde_json::to_string(&transaction.wait_until).map_err(GatewayError::JsonSerialization)?,
        actions,
        state as OperationStepStateRow,
        tx_hash.map(|hash| hash.0.to_string()),
        signed_transaction,
    )
    .execute(&mut **tx)
    .await?;

    Ok(())
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
    state AS "state: OperationStepStateRow",
    tx_hash,
    signed_transaction,
    created_at,
    updated_at
FROM
    gateway_operation_steps
WHERE
    operation_id = $1
ORDER BY
    step_index ASC
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

fn rows_to_stored_operation(
    operation_row: OperationRow,
    step_rows: Vec<OperationStepRow>,
) -> GatewayResult<StoredOperation> {
    let mut succeeded_steps = Vec::new();
    let mut current_step = None;
    let mut remaining_steps = VecDeque::new();

    for row in step_rows {
        apply_step_row(
            row,
            &mut succeeded_steps,
            &mut current_step,
            &mut remaining_steps,
        )?;
    }

    let id = OperationId(operation_row.id.to_string());
    let signer_account_id = ManagedAccountId(
        operation_row
            .signer_account_id
            .parse::<near_account_id::AccountId>()
            .map_err(|error| GatewayError::InvalidStoredOperation(error.to_string()))?,
    );
    let request_payload = serde_json::to_vec(&operation_row.request_payload)?;

    let mut request_fingerprint_hash = [0_u8; 32];
    if operation_row.request_fingerprint_hash.len() != request_fingerprint_hash.len() {
        return Err(GatewayError::InvalidStoredOperation(
            "request fingerprint hash must be 32 bytes".to_owned(),
        ));
    }
    request_fingerprint_hash.copy_from_slice(&operation_row.request_fingerprint_hash);

    let operation = StoredOperation {
        rpc_method: operation_row.rpc_method,
        request_fingerprint_hash,
        request_payload,
        id,
        signer_account_id,
        succeeded_steps,
        current_step,
        remaining_steps,
    };

    let expected_status = operation_status_row(operation.status());
    if operation_row.status != expected_status {
        return Err(GatewayError::InvalidStoredOperation(format!(
            "operation status mismatch: row={:?} computed={:?}",
            operation_row.status, expected_status
        )));
    }

    Ok(operation)
}

fn apply_step_row(
    row: OperationStepRow,
    succeeded_steps: &mut Vec<SucceededStep>,
    current_step: &mut Option<templar_gateway_core::CurrentStep>,
    remaining_steps: &mut VecDeque<PlannedTransaction>,
) -> GatewayResult<()> {
    let transaction = step_row_transaction(&row)?;
    match row.state {
        OperationStepStateRow::Succeeded => {
            let tx_hash = parse_required_crypto_hash(row.tx_hash.as_deref(), "succeeded")?;
            succeeded_steps.push(SucceededStep {
                transaction,
                tx_hash,
            });
        }
        OperationStepStateRow::Prepared => {
            let tx_hash = parse_required_crypto_hash(row.tx_hash.as_deref(), "prepared")?;
            let signed_transaction = parse_signed_transaction(row.signed_transaction)?;
            *current_step = Some(templar_gateway_core::CurrentStep::Prepared {
                transaction,
                signed_transaction: Box::new(signed_transaction),
                tx_hash,
            });
        }
        OperationStepStateRow::Submitted => {
            let tx_hash = parse_required_crypto_hash(row.tx_hash.as_deref(), "submitted")?;
            *current_step = Some(templar_gateway_core::CurrentStep::Submitted {
                transaction,
                tx_hash,
            });
        }
        OperationStepStateRow::Failed => {
            let tx_hash = parse_required_crypto_hash(row.tx_hash.as_deref(), "failed")?;
            *current_step = Some(templar_gateway_core::CurrentStep::Failed {
                transaction,
                tx_hash,
                failure: None,
            });
        }
        OperationStepStateRow::NotStarted => remaining_steps.push_back(transaction),
    }
    Ok(())
}

fn step_row_transaction(row: &OperationStepRow) -> GatewayResult<PlannedTransaction> {
    Ok(PlannedTransaction {
        signer_account_id: ManagedAccountId(
            row.signer_account_id
                .parse::<near_account_id::AccountId>()
                .map_err(|error| GatewayError::InvalidStoredOperation(error.to_string()))?,
        ),
        wait_until: serde_json::from_str(&row.wait_until)
            .map_err(|error| GatewayError::InvalidStoredOperation(error.to_string()))?,
        receiver_id: row
            .receiver_id
            .parse::<near_account_id::AccountId>()
            .map_err(|error| GatewayError::InvalidStoredOperation(error.to_string()))?,
        actions: serde_json::from_value(row.actions.clone())?,
    })
}

fn parse_required_crypto_hash(value: Option<&str>, state: &str) -> GatewayResult<CryptoHash> {
    parse_crypto_hash(value)?.ok_or_else(|| {
        GatewayError::InvalidStoredOperation(format!("{state} step missing transaction hash"))
    })
}

fn parse_signed_transaction(value: Option<Vec<u8>>) -> GatewayResult<SignedTransaction> {
    value
        .ok_or_else(|| {
            GatewayError::InvalidStoredOperation(
                "prepared step missing signed transaction".to_owned(),
            )
        })
        .and_then(|bytes| {
            SignedTransaction::try_from_slice(&bytes)
                .map_err(|error| GatewayError::InvalidStoredOperation(error.to_string()))
        })
}

fn parse_crypto_hash(value: Option<&str>) -> GatewayResult<Option<CryptoHash>> {
    value
        .map(|value| {
            NearCryptoHash::from_str(value)
                .map(CryptoHash::from)
                .map_err(|error| GatewayError::InvalidStoredOperation(error.to_string()))
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

#[cfg(test)]
mod tests {
    use near_api::types::transaction::actions::{Action, TransferAction};
    use near_api::types::CryptoHash as NearCryptoHash;
    use templar_gateway_core::CurrentStep;
    use templar_gateway_types::{common::TxExecutionStatus, NearToken};

    use super::*;

    fn sample_transaction() -> PlannedTransaction {
        PlannedTransaction::single_action(
            ManagedAccountId("signer.near".parse().unwrap()),
            TxExecutionStatus::Final,
            "receiver.near".parse().unwrap(),
            Action::Transfer(TransferAction {
                deposit: NearToken::from_yoctonear(7),
            }),
        )
    }

    fn sample_operation(status: OperationStatus) -> StoredOperation {
        let transaction = sample_transaction();
        match status {
            OperationStatus::Pending => StoredOperation {
                rpc_method: "tx.transfer".to_owned(),
                request_fingerprint_hash: [1; 32],
                request_payload: serde_json::to_vec(&serde_json::json!({ "amount": "7" })).unwrap(),
                id: OperationId(uuid::Uuid::new_v4().to_string()),
                signer_account_id: ManagedAccountId("signer.near".parse().unwrap()),
                succeeded_steps: vec![],
                current_step: None,
                remaining_steps: VecDeque::from([transaction]),
            },
            OperationStatus::InProgress => StoredOperation {
                rpc_method: "tx.transfer".to_owned(),
                request_fingerprint_hash: [2; 32],
                request_payload: serde_json::to_vec(&serde_json::json!({ "amount": "8" })).unwrap(),
                id: OperationId(uuid::Uuid::new_v4().to_string()),
                signer_account_id: ManagedAccountId("signer.near".parse().unwrap()),
                succeeded_steps: vec![],
                current_step: Some(CurrentStep::Failed {
                    transaction,
                    tx_hash: CryptoHash(NearCryptoHash::default()),
                    failure: None,
                }),
                remaining_steps: VecDeque::new(),
            },
            OperationStatus::Succeeded => StoredOperation {
                rpc_method: "tx.transfer".to_owned(),
                request_fingerprint_hash: [3; 32],
                request_payload: serde_json::to_vec(&serde_json::json!({ "amount": "9" })).unwrap(),
                id: OperationId(uuid::Uuid::new_v4().to_string()),
                signer_account_id: ManagedAccountId("signer.near".parse().unwrap()),
                succeeded_steps: vec![SucceededStep {
                    transaction,
                    tx_hash: CryptoHash(NearCryptoHash::default()),
                }],
                current_step: None,
                remaining_steps: VecDeque::new(),
            },
            OperationStatus::Failed => StoredOperation {
                rpc_method: "tx.transfer".to_owned(),
                request_fingerprint_hash: [4; 32],
                request_payload: serde_json::to_vec(&serde_json::json!({ "amount": "10" }))
                    .unwrap(),
                id: OperationId(uuid::Uuid::new_v4().to_string()),
                signer_account_id: ManagedAccountId("signer.near".parse().unwrap()),
                succeeded_steps: vec![],
                current_step: Some(CurrentStep::Failed {
                    transaction,
                    tx_hash: CryptoHash(NearCryptoHash::default()),
                    failure: None,
                }),
                remaining_steps: VecDeque::new(),
            },
        }
    }

    #[test]
    fn operation_status_row_matches_status() {
        assert_eq!(
            operation_status_row(OperationStatus::Pending),
            OperationStatusRow::Pending
        );
        assert_eq!(
            operation_status_row(OperationStatus::InProgress),
            OperationStatusRow::InProgress
        );
        assert_eq!(
            operation_status_row(OperationStatus::Succeeded),
            OperationStatusRow::Succeeded
        );
        assert_eq!(
            operation_status_row(OperationStatus::Failed),
            OperationStatusRow::Failed
        );
    }

    #[test]
    fn rows_round_trip_preserves_succeeded_operation() {
        let operation = sample_operation(OperationStatus::Succeeded);
        let operation_row = OperationRow {
            id: uuid::Uuid::from_str(&operation.id.0).unwrap(),
            rpc_method: operation.rpc_method.clone(),
            signer_account_id: operation.signer_account_id.0.to_string(),
            idempotency_key: None,
            request_fingerprint_hash: operation.request_fingerprint_hash.to_vec(),
            request_payload: serde_json::from_slice(&operation.request_payload).unwrap(),
            status: OperationStatusRow::Succeeded,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let step_rows = vec![OperationStepRow {
            operation_id: operation_row.id,
            step_index: 0,
            signer_account_id: operation.signer_account_id.0.to_string(),
            receiver_id: operation
                .succeeded_steps
                .first()
                .unwrap()
                .transaction
                .receiver_id
                .to_string(),
            wait_until: serde_json::to_string(
                &operation
                    .succeeded_steps
                    .first()
                    .unwrap()
                    .transaction
                    .wait_until,
            )
            .unwrap(),
            actions: serde_json::to_value(
                &operation
                    .succeeded_steps
                    .first()
                    .unwrap()
                    .transaction
                    .actions,
            )
            .unwrap(),
            state: OperationStepStateRow::Succeeded,
            tx_hash: Some(
                operation
                    .succeeded_steps
                    .first()
                    .unwrap()
                    .tx_hash
                    .0
                    .to_string(),
            ),
            signed_transaction: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }];

        let restored = rows_to_stored_operation(operation_row, step_rows).unwrap();
        assert_eq!(restored.status(), OperationStatus::Succeeded);
        assert_eq!(restored.succeeded_steps.len(), 1);
    }
}
