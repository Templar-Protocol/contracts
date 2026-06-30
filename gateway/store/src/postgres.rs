use std::{collections::VecDeque, str::FromStr};

use async_trait::async_trait;
use borsh::{to_vec, BorshDeserialize};
use chrono::{DateTime, Utc};
use near_api::types::transaction::SignedTransaction;
use near_api::types::CryptoHash as NearCryptoHash;
use serde_json::Value;
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    PgPool,
};
use templar_gateway_core::{
    CreateOperationResult, CurrentStep, GatewayError, GatewayResult, OperationPlan, OperationStore,
    PlannedTransaction, StoredOperation, SucceededStep,
};
use templar_gateway_types::{
    operation::{ExecutionOutcome, OperationId, ReceiptOutcome, ReceiptStatus},
    CryptoHash, IdempotencyKey, ManagedAccountId, NearGas, NearToken, OperationStatus,
};

/// Default schema for gateway store tables, types, and migrations.
pub const DEFAULT_SCHEMA: &str = "gateway";

/// Validate the unquoted schema identifier used in `search_path` and DDL.
fn validate_schema_identifier(schema: &str) -> Result<(), sqlx::Error> {
    let valid = (1..=63).contains(&schema.len())
        && schema.chars().enumerate().all(|(index, character)| {
            if index == 0 {
                character.is_ascii_alphabetic() || character == '_'
            } else {
                character.is_ascii_alphanumeric() || character == '_'
            }
        });
    if valid {
        Ok(())
    } else {
        Err(sqlx::Error::Configuration(
            format!("invalid Postgres schema identifier: {schema:?}").into(),
        ))
    }
}

#[derive(Debug, Clone)]
pub struct PostgresStore {
    pool: PgPool,
    /// Schema containing the store's tables, types, and sqlx migrations.
    schema: String,
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

// Row DTOs carry the full table shape (incl. audit columns) for completeness;
// not every field is read by the domain logic.
#[derive(Debug, Clone)]
#[allow(dead_code, reason = "row DTO mirrors the table; some columns unused")]
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

#[derive(Debug, Clone)]
#[allow(dead_code, reason = "row DTO mirrors the table; some columns unused")]
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
    // Execution-outcome scalars (present iff the step executed); the per-receipt
    // detail lives in `gateway_operation_step_receipts`.
    outcome_tokens_burnt: Option<String>,
    outcome_total_gas_burnt: Option<String>,
    outcome_return_value: Option<Vec<u8>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl PostgresStore {
    /// Connect using [`DEFAULT_SCHEMA`].
    pub fn new(database_url: &str) -> Result<Self, sqlx::Error> {
        Self::with_schema(database_url, DEFAULT_SCHEMA)
    }

    /// Connect using a specific Postgres `schema`.
    ///
    /// Pass `"public"` only for legacy databases whose gateway store already
    /// lives in the default schema. The identifier is validated because it is
    /// interpolated into `search_path` and `CREATE SCHEMA`.
    pub fn with_schema(database_url: &str, schema: &str) -> Result<Self, sqlx::Error> {
        validate_schema_identifier(schema)?;
        let options = PgConnectOptions::from_str(database_url)?
            .options([("search_path", format!("{schema},public"))]);
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect_lazy_with(options);
        Ok(Self {
            pool,
            schema: schema.to_owned(),
        })
    }

    pub async fn migrate(&self) -> Result<(), sqlx::migrate::MigrateError> {
        // Create the schema before sqlx creates `_sqlx_migrations` in it.
        sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS \"{}\"", self.schema))
            .execute(&self.pool)
            .await?;
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
        let Some(operation_row) = sqlx::query_as!(
            OperationRow,
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

        let step_rows = load_step_rows(&self.pool, operation_row.id).await?;
        let receipts = load_step_receipts(&self.pool, operation_row.id).await?;
        rows_to_stored_operation(operation_row, step_rows, receipts).map(Some)
    }

    async fn get_by_idempotency_key(
        &self,
        idempotency_key: &IdempotencyKey,
    ) -> GatewayResult<Option<StoredOperation>> {
        let Some(operation_row) = sqlx::query_as!(
            OperationRow,
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

        let step_rows = load_step_rows(&self.pool, operation_row.id).await?;
        let receipts = load_step_receipts(&self.pool, operation_row.id).await?;
        rows_to_stored_operation(operation_row, step_rows, receipts).map(Some)
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

        match insert_operation_tx(&self.pool, &operation, idempotency_key.as_ref()).await {
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
        update_operation_tx(&self.pool, &operation).await
    }

    async fn delete_operation(&self, operation_id: &OperationId) -> GatewayResult<()> {
        let operation_uuid = uuid::Uuid::from_str(&operation_id.0)
            .map_err(|error| GatewayError::InvalidStoredOperation(error.to_string()))?;
        // Steps (and their receipts) cascade via the FK ON DELETE CASCADE.
        sqlx::query!(
            "DELETE FROM gateway_operations WHERE id = $1",
            operation_uuid
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_incomplete_operations(&self) -> GatewayResult<Vec<StoredOperation>> {
        let operation_rows = sqlx::query_as!(
            OperationRow,
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

        let mut operations = Vec::with_capacity(operation_rows.len());
        for operation_row in operation_rows {
            let steps = load_step_rows(&self.pool, operation_row.id).await?;
            let receipts = load_step_receipts(&self.pool, operation_row.id).await?;
            operations.push(rows_to_stored_operation(operation_row, steps, receipts)?);
        }
        Ok(operations)
    }
}

/// Insert a brand-new operation row and its planned steps. The identity columns
/// (`id`, `idempotency_key`, signer, fingerprint, payload) are written once,
/// here, and never rewritten — see [`update_operation_tx`].
async fn insert_operation_tx(
    pool: &PgPool,
    operation: &StoredOperation,
    idempotency_key: Option<&IdempotencyKey>,
) -> GatewayResult<()> {
    let mut tx = pool.begin().await?;
    let operation_uuid = uuid::Uuid::from_str(&operation.id.0)
        .map_err(|error| GatewayError::InvalidStoredOperation(error.to_string()))?;
    insert_operation_row(&mut tx, operation_uuid, operation, idempotency_key).await?;
    insert_operation_steps(&mut tx, operation_uuid, operation).await?;
    tx.commit().await?;
    Ok(())
}

/// Persist progress on an existing operation. Only `status` and the step rows
/// change as an operation runs; its identity columns are immutable, so this
/// updates the status in place and rewrites the steps without touching the
/// operation row's other columns — crucially leaving `idempotency_key` intact
/// (an earlier version re-inserted the whole row with a null key, silently
/// dropping idempotency and breaking crash recovery).
async fn update_operation_tx(pool: &PgPool, operation: &StoredOperation) -> GatewayResult<()> {
    let mut tx = pool.begin().await?;
    let operation_uuid = uuid::Uuid::from_str(&operation.id.0)
        .map_err(|error| GatewayError::InvalidStoredOperation(error.to_string()))?;

    let status = operation_status_row(operation.status());
    sqlx::query!(
        r#"
UPDATE
    gateway_operations
SET
    STATUS = $2
WHERE
    id = $1
"#,
        operation_uuid,
        status as OperationStatusRow,
    )
    .execute(&mut *tx)
    .await?;

    // Steps genuinely change shape between saves; replace them wholesale
    // (receipts cascade-delete with their step rows).
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
            Some(&step.outcome),
        )
        .await?;
    }

    let mut current_index = step_index(operation.succeeded_steps.len())?;
    if let Some(current_step) = &operation.current_step {
        // Reverted and Rejected share the `failed` row state; the presence of an
        // outcome distinguishes them on load (reverted executed and carries one;
        // rejected never executed).
        let (transaction, state, tx_hash, signed, outcome) = match current_step {
            CurrentStep::Prepared {
                transaction,
                signed_transaction,
                tx_hash,
            } => (
                transaction,
                OperationStepStateRow::Prepared,
                *tx_hash,
                Some(signed_transaction.as_ref()),
                None,
            ),
            CurrentStep::Submitted {
                transaction,
                tx_hash,
            } => (
                transaction,
                OperationStepStateRow::Submitted,
                *tx_hash,
                None,
                None,
            ),
            CurrentStep::Reverted {
                transaction,
                tx_hash,
                outcome,
            } => (
                transaction,
                OperationStepStateRow::Failed,
                *tx_hash,
                None,
                Some(outcome),
            ),
            CurrentStep::Rejected {
                transaction,
                tx_hash,
            } => (
                transaction,
                OperationStepStateRow::Failed,
                *tx_hash,
                None,
                None,
            ),
        };
        insert_step_row(
            tx,
            operation_uuid,
            current_index,
            transaction,
            state,
            Some(tx_hash),
            signed,
            outcome,
        )
        .await?;
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

#[allow(
    clippy::too_many_arguments,
    reason = "a step row is a flat record; grouping its columns would obscure the 1:1 INSERT"
)]
async fn insert_step_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operation_id: uuid::Uuid,
    step_index: i32,
    transaction: &PlannedTransaction,
    state: OperationStepStateRow,
    tx_hash: Option<CryptoHash>,
    signed_transaction: Option<&SignedTransaction>,
    outcome: Option<&ExecutionOutcome>,
) -> GatewayResult<()> {
    let actions = serde_json::to_value(&transaction.actions)?;
    let signed_transaction = signed_transaction
        .map(to_vec)
        .transpose()
        .map_err(|error| GatewayError::InvalidStoredOperation(error.to_string()))?;
    // u128/u64 amounts as lossless decimal text; return value as raw bytes.
    let tokens_burnt = outcome.map(|o| o.tokens_burnt.as_yoctonear().to_string());
    let total_gas_burnt = outcome.map(|o| o.total_gas_burnt.as_gas().to_string());
    let return_value = outcome.and_then(|o| o.return_value.as_ref().map(|b| b.0.clone()));

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
        signed_transaction,
        outcome_tokens_burnt,
        outcome_total_gas_burnt,
        outcome_return_value
    )
VALUES
    (
        $1,
        $2,
        $3,
        $4,
        $5,
        $6,
        $7,
        $8,
        $9,
        $10,
        $11,
        $12
    )
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
        tokens_burnt,
        total_gas_burnt,
        return_value,
    )
    .execute(&mut **tx)
    .await?;

    // One row per receipt (the outcome's per-receipt detail).
    if let Some(outcome) = outcome {
        for (receipt_index, receipt) in outcome.receipts.iter().enumerate() {
            sqlx::query!(
                r#"
INSERT INTO
    gateway_operation_step_receipts (
        operation_id,
        step_index,
        receipt_index,
        contract_id,
        STATUS,
        LOGS
    )
VALUES
    ($1, $2, $3, $4, $5, $6)
"#,
                operation_id,
                step_index,
                i32::try_from(receipt_index).map_err(|_| {
                    GatewayError::InvalidStoredOperation(
                        "receipt index exceeds i32 range".to_owned(),
                    )
                })?,
                receipt.contract_id.to_string(),
                receipt_status_str(receipt.status),
                &receipt.logs,
            )
            .execute(&mut **tx)
            .await?;
        }
    }

    Ok(())
}

/// The `status` text stored for a receipt (DB CHECK enforces this set).
fn receipt_status_str(status: ReceiptStatus) -> &'static str {
    match status {
        ReceiptStatus::Succeeded => "succeeded",
        ReceiptStatus::Failed => "failed",
    }
}

fn parse_receipt_status(value: &str) -> GatewayResult<ReceiptStatus> {
    match value {
        "succeeded" => Ok(ReceiptStatus::Succeeded),
        "failed" => Ok(ReceiptStatus::Failed),
        other => Err(GatewayError::InvalidStoredOperation(format!(
            "invalid receipt status {other:?}"
        ))),
    }
}

async fn load_step_rows(
    pool: &PgPool,
    operation_id: uuid::Uuid,
) -> GatewayResult<Vec<OperationStepRow>> {
    sqlx::query_as!(
        OperationStepRow,
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
    outcome_tokens_burnt,
    outcome_total_gas_burnt,
    outcome_return_value,
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
    .await
    .map_err(Into::into)
}

/// Load every step's receipts for an operation, grouped by `step_index` and
/// ordered by `receipt_index`.
async fn load_step_receipts(
    pool: &PgPool,
    operation_id: uuid::Uuid,
) -> GatewayResult<std::collections::HashMap<i32, Vec<ReceiptOutcome>>> {
    let rows = sqlx::query!(
        r#"
SELECT
    step_index,
    contract_id,
    STATUS,
    LOGS
FROM
    gateway_operation_step_receipts
WHERE
    operation_id = $1
ORDER BY
    step_index ASC,
    receipt_index ASC
"#,
        operation_id,
    )
    .fetch_all(pool)
    .await?;

    let mut by_step: std::collections::HashMap<i32, Vec<ReceiptOutcome>> =
        std::collections::HashMap::new();
    for row in rows {
        let contract_id = row
            .contract_id
            .parse::<near_account_id::AccountId>()
            .map_err(|error| GatewayError::InvalidStoredOperation(error.to_string()))?;
        by_step
            .entry(row.step_index)
            .or_default()
            .push(ReceiptOutcome {
                contract_id,
                status: parse_receipt_status(&row.status)?,
                logs: row.logs,
            });
    }
    Ok(by_step)
}

fn rows_to_stored_operation(
    operation_row: OperationRow,
    step_rows: Vec<OperationStepRow>,
    mut receipts_by_step: std::collections::HashMap<i32, Vec<ReceiptOutcome>>,
) -> GatewayResult<StoredOperation> {
    let mut succeeded_steps = Vec::new();
    let mut current_step = None;
    let mut remaining_steps = VecDeque::new();

    for row in step_rows {
        let receipts = receipts_by_step.remove(&row.step_index).unwrap_or_default();
        apply_step_row(
            row,
            receipts,
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
    receipts: Vec<ReceiptOutcome>,
    succeeded_steps: &mut Vec<SucceededStep>,
    current_step: &mut Option<templar_gateway_core::CurrentStep>,
    remaining_steps: &mut VecDeque<PlannedTransaction>,
) -> GatewayResult<()> {
    let transaction = step_row_transaction(&row)?;
    let outcome = build_outcome(&row, receipts)?;
    match row.state {
        OperationStepStateRow::Succeeded => {
            let tx_hash = parse_required_crypto_hash(row.tx_hash.as_deref(), "succeeded")?;
            let outcome = outcome.ok_or_else(|| {
                GatewayError::InvalidStoredOperation(
                    "succeeded step is missing its execution outcome".to_owned(),
                )
            })?;
            succeeded_steps.push(SucceededStep {
                transaction,
                tx_hash,
                outcome,
            });
        }
        OperationStepStateRow::Prepared => {
            let tx_hash = parse_required_crypto_hash(row.tx_hash.as_deref(), "prepared")?;
            let signed_transaction = parse_signed_transaction(row.signed_transaction)?;
            *current_step = Some(CurrentStep::Prepared {
                transaction,
                signed_transaction: Box::new(signed_transaction),
                tx_hash,
            });
        }
        OperationStepStateRow::Submitted => {
            let tx_hash = parse_required_crypto_hash(row.tx_hash.as_deref(), "submitted")?;
            *current_step = Some(CurrentStep::Submitted {
                transaction,
                tx_hash,
            });
        }
        // A `failed` row is `Reverted` if it executed (carries an outcome) and
        // `Rejected` otherwise.
        OperationStepStateRow::Failed => {
            let tx_hash = parse_required_crypto_hash(row.tx_hash.as_deref(), "failed")?;
            *current_step = Some(match outcome {
                Some(outcome) => CurrentStep::Reverted {
                    transaction,
                    tx_hash,
                    outcome,
                },
                None => CurrentStep::Rejected {
                    transaction,
                    tx_hash,
                },
            });
        }
        OperationStepStateRow::NotStarted => remaining_steps.push_back(transaction),
    }
    Ok(())
}

/// Reconstruct a step's [`ExecutionOutcome`] from its scalar columns and loaded
/// receipts. Returns `None` when the step never executed (no scalars stored).
fn build_outcome(
    row: &OperationStepRow,
    receipts: Vec<ReceiptOutcome>,
) -> GatewayResult<Option<ExecutionOutcome>> {
    // The two scalars are written together (DB CHECK), so either both are
    // present (the step executed) or neither is.
    let (Some(tokens), Some(gas)) = (
        row.outcome_tokens_burnt.as_deref(),
        row.outcome_total_gas_burnt.as_deref(),
    ) else {
        return Ok(None);
    };
    let tokens_burnt = NearToken::from_yoctonear(tokens.parse().map_err(|_| {
        GatewayError::InvalidStoredOperation(format!("invalid tokens_burnt {tokens:?}"))
    })?);
    let total_gas_burnt = NearGas::from_gas(gas.parse().map_err(|_| {
        GatewayError::InvalidStoredOperation(format!("invalid total_gas_burnt {gas:?}"))
    })?);
    Ok(Some(ExecutionOutcome {
        tokens_burnt,
        total_gas_burnt,
        receipts,
        return_value: row.outcome_return_value.clone().map(Into::into),
    }))
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
    use templar_gateway_types::{common::TxExecutionStatus, NearGas, NearToken};

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

    fn sample_outcome() -> ExecutionOutcome {
        ExecutionOutcome {
            tokens_burnt: NearToken::from_yoctonear(42),
            total_gas_burnt: NearGas::from_gas(1_000),
            receipts: vec![
                ReceiptOutcome {
                    contract_id: "receiver.near".parse().unwrap(),
                    status: ReceiptStatus::Succeeded,
                    logs: vec!["hello".to_owned()],
                },
                ReceiptOutcome {
                    contract_id: "callback.near".parse().unwrap(),
                    status: ReceiptStatus::Failed,
                    logs: vec![],
                },
            ],
            return_value: None,
        }
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
                current_step: Some(CurrentStep::Submitted {
                    transaction,
                    tx_hash: CryptoHash(NearCryptoHash::default()),
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
                    outcome: sample_outcome(),
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
                current_step: Some(CurrentStep::Reverted {
                    transaction,
                    tx_hash: CryptoHash(NearCryptoHash::default()),
                    outcome: sample_outcome(),
                }),
                remaining_steps: VecDeque::new(),
            },
        }
    }

    /// A progress save must not drop the operation's idempotency key — the
    /// relayer's crash recovery (and gateway idempotent retries) look operations
    /// up by it. A previous version re-inserted the row with a null key on every
    /// save.
    #[sqlx::test(migrations = "./migrations")]
    async fn save_operation_preserves_idempotency_key(pool: PgPool) {
        let store = PostgresStore {
            pool,
            schema: "public".to_owned(),
        };
        let key = IdempotencyKey("op-key-1".to_owned());

        let created = store
            .create_or_get_operation(
                "tx.transfer",
                ManagedAccountId("signer.near".parse().unwrap()),
                Some(key.clone()),
                [1; 32],
                serde_json::to_vec(&serde_json::json!({ "amount": "7" })).unwrap(),
                OperationPlan {
                    steps: vec![sample_transaction()],
                },
            )
            .await
            .unwrap();
        let CreateOperationResult::Created(operation) = created else {
            panic!("expected a freshly created operation");
        };

        // Drive it to a terminal state and persist progress (a step transition).
        let mut progressed = sample_operation(OperationStatus::Succeeded);
        progressed.id = operation.id.clone();
        store.save_operation(progressed).await.unwrap();

        let found = store
            .get_by_idempotency_key(&key)
            .await
            .unwrap()
            .expect("operation still resolvable by idempotency key after a save");
        assert_eq!(found.id, operation.id);
        assert_eq!(found.status(), OperationStatus::Succeeded);
    }

    /// A reservation (no steps) is non-terminal, and deleting it removes the row
    /// and its steps and clears the idempotency mapping — the path the driver
    /// uses for a no-op plan or an abandoned reservation.
    #[sqlx::test(migrations = "./migrations")]
    async fn delete_operation_removes_reservation(pool: PgPool) {
        let store = PostgresStore {
            pool,
            schema: "public".to_owned(),
        };
        let key = IdempotencyKey("del-key".to_owned());

        let CreateOperationResult::Created(reserved) = store
            .create_or_get_operation(
                "tx.transfer",
                ManagedAccountId("signer.near".parse().unwrap()),
                Some(key.clone()),
                [5; 32],
                serde_json::to_vec(&serde_json::json!({ "amount": "1" })).unwrap(),
                OperationPlan { steps: vec![] },
            )
            .await
            .unwrap()
        else {
            panic!("expected a freshly created reservation");
        };
        assert_eq!(reserved.status(), OperationStatus::Pending);
        assert!(store.get_by_idempotency_key(&key).await.unwrap().is_some());

        store.delete_operation(&reserved.id).await.unwrap();

        assert!(store.get_by_id(&reserved.id).await.unwrap().is_none());
        assert!(store.get_by_idempotency_key(&key).await.unwrap().is_none());
    }

    #[test]
    fn schema_identifier_validation() {
        for ok in ["gateway", "public", "_private", "s9_x"] {
            assert!(
                validate_schema_identifier(ok).is_ok(),
                "{ok} should be valid"
            );
        }
        for bad in [
            "",
            "has space",
            "a-b",
            "1abc",
            "\"; DROP SCHEMA x --",
            "a;b",
        ] {
            assert!(
                validate_schema_identifier(bad).is_err(),
                "{bad:?} should be rejected"
            );
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
            outcome_tokens_burnt: Some(sample_outcome().tokens_burnt.as_yoctonear().to_string()),
            outcome_total_gas_burnt: Some(sample_outcome().total_gas_burnt.as_gas().to_string()),
            outcome_return_value: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }];
        let receipts =
            std::collections::HashMap::from([(0_i32, sample_outcome().receipts.clone())]);

        let restored = rows_to_stored_operation(operation_row, step_rows, receipts).unwrap();
        assert_eq!(restored.status(), OperationStatus::Succeeded);
        assert_eq!(restored.succeeded_steps.len(), 1);
        assert_eq!(
            restored.succeeded_steps.first().unwrap().outcome,
            sample_outcome()
        );
    }
}
