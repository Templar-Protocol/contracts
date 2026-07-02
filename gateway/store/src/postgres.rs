use std::collections::{HashMap, VecDeque};
use std::str::FromStr;

use async_trait::async_trait;
use borsh::{to_vec, BorshDeserialize};
use chrono::{DateTime, Utc};
use near_api::types::transaction::SignedTransaction;
use near_api::types::CryptoHash as NearCryptoHash;
use serde_json::Value;
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    FromRow, PgPool,
};
use templar_gateway_core::{
    CreateOperationResult, CurrentStep, GatewayError, GatewayResult, OperationPlan, OperationStore,
    PlannedTransaction, StoredOperation, SucceededStep,
};
use templar_gateway_types::{
    operation::{ExecutionOutcome, OperationId, ReceiptOutcome, ReceiptStatus},
    CryptoHash, IdempotencyKey, ManagedAccountId, NearGas, NearToken,
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
#[sqlx(type_name = "gateway_outcome_status", rename_all = "snake_case")]
enum OutcomeStatusRow {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, FromRow)]
#[allow(
    dead_code,
    reason = "row DTO mirrors the query shape; audit columns unused"
)]
struct OperationRow {
    id: uuid::Uuid,
    rpc_method: String,
    signer_account_id: String,
    idempotency_key: Option<String>,
    request_fingerprint_hash: Vec<u8>,
    request_payload: Value,
    plan_created_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
#[allow(
    dead_code,
    reason = "row DTO mirrors the query shape; audit columns unused"
)]
struct StepLifecycleRow {
    operation_id: uuid::Uuid,
    step_index: i32,
    signer_account_id: String,
    receiver_id: String,
    actions: Value,
    execution_tx_hash: Option<String>,
    signed_transaction: Option<Vec<u8>>,
    submitted_at: Option<DateTime<Utc>>,
    result_tx_hash: Option<String>,
    outcome_status: Option<OutcomeStatusRow>,
    tokens_burnt: Option<String>,
    total_gas_burnt: Option<String>,
    return_value: Option<Vec<u8>>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
struct ReceiptRow {
    step_index: i32,
    contract_id: String,
    status: OutcomeStatusRow,
    logs: Vec<String>,
}

#[derive(Debug, Clone, FromRow)]
struct ExistingExecutionRow {
    step_index: i32,
    signed_transaction: Vec<u8>,
    prepared_at: DateTime<Utc>,
    submitted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
struct ExistingExecution {
    signed_transaction: Vec<u8>,
    prepared_at: DateTime<Utc>,
    submitted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
struct StepExecution<'a> {
    tx_hash: CryptoHash,
    signed_transaction: &'a [u8],
    prepared_at: DateTime<Utc>,
    submitted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy)]
struct StepResult {
    tx_hash: CryptoHash,
}

#[derive(Debug, Clone, Copy)]
struct StepOutcome<'a> {
    status: OutcomeStatusRow,
    outcome: &'a ExecutionOutcome,
}

/// Receipt outcomes grouped by step index.
type ReceiptMap = HashMap<i32, Vec<ReceiptOutcome>>;

type ExistingExecutionsByStep = HashMap<i32, ExistingExecution>;

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
        let operation_uuid = parse_operation_uuid(operation_id)?;
        let Some(operation_row) = load_operation_by_id(&self.pool, operation_uuid).await? else {
            return Ok(None);
        };

        load_stored_operation(&self.pool, operation_row)
            .await
            .map(Some)
    }

    async fn get_by_idempotency_key(
        &self,
        idempotency_key: &IdempotencyKey,
    ) -> GatewayResult<Option<StoredOperation>> {
        let Some(operation_row) =
            load_operation_by_idempotency_key(&self.pool, idempotency_key).await?
        else {
            return Ok(None);
        };

        load_stored_operation(&self.pool, operation_row)
            .await
            .map(Some)
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
            // A step-bearing plan is already planned; an empty plan here is a
            // reservation (the no-op case is promoted from a reservation later,
            // never created directly).
            planned: !plan.steps.is_empty(),
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

    async fn delete_reservation(&self, operation_id: &OperationId) -> GatewayResult<()> {
        let operation_uuid = parse_operation_uuid(operation_id)?;
        // Only delete a reservation: an operation row with no structural plan.
        // Planned no-ops have a plan row and are intentionally retained.
        sqlx::query(
            r"
DELETE FROM
    gateway_operations AS operation
WHERE
    operation.id = $1
    AND NOT EXISTS (
        SELECT
            1
        FROM
            gateway_operation_plans AS plan
        WHERE
            plan.operation_id = operation.id
    )
",
        )
        .bind(operation_uuid)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_incomplete_operations(&self) -> GatewayResult<Vec<StoredOperation>> {
        let operation_rows = load_operations_ordered(&self.pool).await?;
        let mut operations = Vec::with_capacity(operation_rows.len());
        for operation_row in operation_rows {
            let operation = load_stored_operation(&self.pool, operation_row).await?;
            if matches!(
                operation.status(),
                templar_gateway_types::OperationStatus::Pending
                    | templar_gateway_types::OperationStatus::InProgress
            ) {
                operations.push(operation);
            }
        }
        Ok(operations)
    }
}

async fn load_operation_by_id(
    pool: &PgPool,
    operation_id: uuid::Uuid,
) -> GatewayResult<Option<OperationRow>> {
    sqlx::query_as::<_, OperationRow>(OPERATION_SELECT_BY_ID)
        .bind(operation_id)
        .fetch_optional(pool)
        .await
        .map_err(Into::into)
}

async fn load_operation_by_idempotency_key(
    pool: &PgPool,
    idempotency_key: &IdempotencyKey,
) -> GatewayResult<Option<OperationRow>> {
    sqlx::query_as::<_, OperationRow>(OPERATION_SELECT_BY_IDEMPOTENCY_KEY)
        .bind(idempotency_key.0.as_str())
        .fetch_optional(pool)
        .await
        .map_err(Into::into)
}

async fn load_operations_ordered(pool: &PgPool) -> GatewayResult<Vec<OperationRow>> {
    sqlx::query_as::<_, OperationRow>(OPERATION_SELECT_ORDERED)
        .fetch_all(pool)
        .await
        .map_err(Into::into)
}

const OPERATION_SELECT_BY_ID: &str = r"
SELECT
    operation.id,
    operation.rpc_method,
    operation.signer_account_id,
    operation.idempotency_key,
    operation.request_fingerprint_hash,
    operation.request_payload,
    plan.created_at AS plan_created_at,
    operation.created_at,
    operation.updated_at
FROM
    gateway_operations AS operation
    LEFT JOIN gateway_operation_plans AS plan ON plan.operation_id = operation.id
WHERE
    operation.id = $1
";

const OPERATION_SELECT_BY_IDEMPOTENCY_KEY: &str = r"
SELECT
    operation.id,
    operation.rpc_method,
    operation.signer_account_id,
    operation.idempotency_key,
    operation.request_fingerprint_hash,
    operation.request_payload,
    plan.created_at AS plan_created_at,
    operation.created_at,
    operation.updated_at
FROM
    gateway_operations AS operation
    LEFT JOIN gateway_operation_plans AS plan ON plan.operation_id = operation.id
WHERE
    operation.idempotency_key = $1
";

const OPERATION_SELECT_ORDERED: &str = r"
SELECT
    operation.id,
    operation.rpc_method,
    operation.signer_account_id,
    operation.idempotency_key,
    operation.request_fingerprint_hash,
    operation.request_payload,
    plan.created_at AS plan_created_at,
    operation.created_at,
    operation.updated_at
FROM
    gateway_operations AS operation
    LEFT JOIN gateway_operation_plans AS plan ON plan.operation_id = operation.id
ORDER BY
    operation.created_at ASC
";

async fn load_stored_operation(
    pool: &PgPool,
    operation_row: OperationRow,
) -> GatewayResult<StoredOperation> {
    let step_rows = load_step_rows(pool, operation_row.id).await?;
    let receipts = load_step_receipts(pool, operation_row.id).await?;
    rows_to_stored_operation(operation_row, step_rows, receipts)
}

/// Insert a brand-new operation row and its initial structural plan, if any. The
/// identity columns (`id`, `idempotency_key`, signer, fingerprint, payload) are
/// written once here and never rewritten.
async fn insert_operation_tx(
    pool: &PgPool,
    operation: &StoredOperation,
    idempotency_key: Option<&IdempotencyKey>,
) -> GatewayResult<()> {
    let mut tx = pool.begin().await?;
    let operation_uuid = parse_operation_uuid(&operation.id)?;
    insert_operation_row(&mut tx, operation_uuid, operation, idempotency_key).await?;
    if operation.planned {
        insert_plan(&mut tx, operation_uuid).await?;
        insert_operation_steps(&mut tx, operation_uuid, operation, &HashMap::new()).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Persist progress on an existing operation. The operation row remains identity
/// only; lifecycle is rewritten structurally under the plan row. Existing signed
/// transactions are read before replacing plan steps so transitions from
/// Prepared into Submitted/terminal states retain the original bytes.
async fn update_operation_tx(pool: &PgPool, operation: &StoredOperation) -> GatewayResult<()> {
    let mut tx = pool.begin().await?;
    let operation_uuid = parse_operation_uuid(&operation.id)?;

    sqlx::query(
        r"
UPDATE
    gateway_operations
SET
    updated_at = NOW()
WHERE
    id = $1
",
    )
    .bind(operation_uuid)
    .execute(&mut *tx)
    .await?;

    if !operation.planned {
        sqlx::query("DELETE FROM gateway_operation_plans WHERE operation_id = $1")
            .bind(operation_uuid)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok(());
    }

    let existing_executions = load_existing_executions(&mut tx, operation_uuid).await?;
    insert_plan(&mut tx, operation_uuid).await?;
    sqlx::query("DELETE FROM gateway_plan_steps WHERE operation_id = $1")
        .bind(operation_uuid)
        .execute(&mut *tx)
        .await?;
    insert_operation_steps(&mut tx, operation_uuid, operation, &existing_executions).await?;

    tx.commit().await?;
    Ok(())
}

async fn load_existing_executions(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operation_id: uuid::Uuid,
) -> GatewayResult<ExistingExecutionsByStep> {
    let rows = sqlx::query_as::<_, ExistingExecutionRow>(
        r"
SELECT
    step_index,
    signed_transaction,
    prepared_at,
    submitted_at
FROM
    gateway_step_executions
WHERE
    operation_id = $1
",
    )
    .bind(operation_id)
    .fetch_all(&mut **tx)
    .await?;

    let mut executions = HashMap::with_capacity(rows.len());
    for row in rows {
        executions.insert(
            row.step_index,
            ExistingExecution {
                signed_transaction: row.signed_transaction,
                prepared_at: row.prepared_at,
                submitted_at: row.submitted_at,
            },
        );
    }
    Ok(executions)
}

async fn insert_operation_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operation_uuid: uuid::Uuid,
    operation: &StoredOperation,
    idempotency_key: Option<&IdempotencyKey>,
) -> GatewayResult<()> {
    let request_payload = serde_json::from_slice::<Value>(&operation.request_payload)
        .map_err(GatewayError::JsonSerialization)?;

    sqlx::query(
        r"
INSERT INTO
    gateway_operations (
        id,
        rpc_method,
        signer_account_id,
        idempotency_key,
        request_fingerprint_hash,
        request_payload
    )
VALUES
    ($1, $2, $3, $4, $5, $6)
",
    )
    .bind(operation_uuid)
    .bind(&operation.rpc_method)
    .bind(operation.signer_account_id.0.to_string())
    .bind(idempotency_key.map(|key| key.0.as_str()))
    .bind(operation.request_fingerprint_hash.as_slice())
    .bind(request_payload)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

async fn insert_plan(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operation_uuid: uuid::Uuid,
) -> GatewayResult<()> {
    sqlx::query(
        r"
INSERT INTO
    gateway_operation_plans (operation_id)
VALUES
    ($1)
ON CONFLICT (operation_id) DO NOTHING
",
    )
    .bind(operation_uuid)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_operation_steps(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operation_uuid: uuid::Uuid,
    operation: &StoredOperation,
    existing_executions: &ExistingExecutionsByStep,
) -> GatewayResult<()> {
    let current_index =
        insert_succeeded_steps(tx, operation_uuid, operation, existing_executions).await?;
    let remaining_start = insert_current_step(
        tx,
        operation_uuid,
        operation.current_step.as_ref(),
        current_index,
        existing_executions,
    )
    .await?;
    insert_remaining_steps(
        tx,
        operation_uuid,
        remaining_start,
        &operation.remaining_steps,
    )
    .await?;
    Ok(())
}

async fn insert_succeeded_steps(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operation_uuid: uuid::Uuid,
    operation: &StoredOperation,
    existing_executions: &ExistingExecutionsByStep,
) -> GatewayResult<i32> {
    for (index, step) in operation.succeeded_steps.iter().enumerate() {
        let step_index = step_index(index)?;
        let existing_execution = existing_execution(existing_executions, step_index)?;
        insert_step_lifecycle(
            tx,
            operation_uuid,
            step_index,
            &step.transaction,
            Some(StepExecution {
                tx_hash: step.tx_hash,
                signed_transaction: &existing_execution.signed_transaction,
                prepared_at: existing_execution.prepared_at,
                submitted_at: existing_execution.submitted_at,
            }),
            Some(StepResult {
                tx_hash: step.tx_hash,
            }),
            Some(StepOutcome {
                status: OutcomeStatusRow::Succeeded,
                outcome: &step.outcome,
            }),
        )
        .await?;
    }
    step_index(operation.succeeded_steps.len())
}

async fn insert_current_step(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operation_uuid: uuid::Uuid,
    current_step: Option<&CurrentStep>,
    current_index: i32,
    existing_executions: &ExistingExecutionsByStep,
) -> GatewayResult<i32> {
    let Some(current_step) = current_step else {
        return Ok(current_index);
    };
    match current_step {
        CurrentStep::Prepared {
            transaction,
            signed_transaction,
            tx_hash,
        } => {
            let signed_transaction = to_vec(signed_transaction.as_ref())
                .map_err(|error| GatewayError::InvalidStoredOperation(error.to_string()))?;
            insert_step_lifecycle(
                tx,
                operation_uuid,
                current_index,
                transaction,
                Some(StepExecution {
                    tx_hash: *tx_hash,
                    signed_transaction: &signed_transaction,
                    prepared_at: Utc::now(),
                    submitted_at: None,
                }),
                None,
                None,
            )
            .await?;
        }
        CurrentStep::Submitted {
            transaction,
            tx_hash,
            submitted_at,
        } => {
            let existing_execution = existing_execution(existing_executions, current_index)?;
            insert_step_lifecycle(
                tx,
                operation_uuid,
                current_index,
                transaction,
                Some(StepExecution {
                    tx_hash: *tx_hash,
                    signed_transaction: &existing_execution.signed_transaction,
                    prepared_at: existing_execution.prepared_at,
                    submitted_at: Some(*submitted_at),
                }),
                None,
                None,
            )
            .await?;
        }
        CurrentStep::Reverted {
            transaction,
            tx_hash,
            outcome,
        } => {
            let existing_execution = existing_execution(existing_executions, current_index)?;
            insert_step_lifecycle(
                tx,
                operation_uuid,
                current_index,
                transaction,
                Some(StepExecution {
                    tx_hash: *tx_hash,
                    signed_transaction: &existing_execution.signed_transaction,
                    prepared_at: existing_execution.prepared_at,
                    submitted_at: existing_execution.submitted_at,
                }),
                Some(StepResult { tx_hash: *tx_hash }),
                Some(StepOutcome {
                    status: OutcomeStatusRow::Failed,
                    outcome,
                }),
            )
            .await?;
        }
        CurrentStep::Rejected {
            transaction,
            tx_hash,
        } => {
            let existing_execution = existing_execution(existing_executions, current_index)?;
            insert_step_lifecycle(
                tx,
                operation_uuid,
                current_index,
                transaction,
                Some(StepExecution {
                    tx_hash: *tx_hash,
                    signed_transaction: &existing_execution.signed_transaction,
                    prepared_at: existing_execution.prepared_at,
                    submitted_at: existing_execution.submitted_at,
                }),
                Some(StepResult { tx_hash: *tx_hash }),
                None,
            )
            .await?;
        }
    }
    Ok(current_index + 1)
}

async fn insert_remaining_steps(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operation_uuid: uuid::Uuid,
    start_index: i32,
    remaining_steps: &VecDeque<PlannedTransaction>,
) -> GatewayResult<()> {
    for (offset, step) in remaining_steps.iter().enumerate() {
        insert_step_lifecycle(
            tx,
            operation_uuid,
            start_index + step_index(offset)?,
            step,
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

fn existing_execution(
    existing_executions: &ExistingExecutionsByStep,
    step_index: i32,
) -> GatewayResult<&ExistingExecution> {
    existing_executions.get(&step_index).ok_or_else(|| {
        GatewayError::InvalidStoredOperation(format!(
            "step {step_index} cannot be persisted without its prepared signed transaction"
        ))
    })
}

async fn insert_step_lifecycle(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operation_id: uuid::Uuid,
    step_index: i32,
    transaction: &PlannedTransaction,
    execution: Option<StepExecution<'_>>,
    result: Option<StepResult>,
    outcome: Option<StepOutcome<'_>>,
) -> GatewayResult<()> {
    insert_plan_step(tx, operation_id, step_index, transaction).await?;
    if let Some(execution) = execution {
        insert_step_execution(tx, operation_id, step_index, execution).await?;
    }
    if let Some(result) = result {
        insert_step_result(tx, operation_id, step_index, result).await?;
    }
    if let Some(outcome) = outcome {
        insert_step_outcome(tx, operation_id, step_index, outcome).await?;
    }
    Ok(())
}

async fn insert_plan_step(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operation_id: uuid::Uuid,
    step_index: i32,
    transaction: &PlannedTransaction,
) -> GatewayResult<()> {
    let actions = serde_json::to_value(&transaction.actions)?;
    sqlx::query(
        r"
INSERT INTO
    gateway_plan_steps (
        operation_id,
        step_index,
        signer_account_id,
        receiver_id,
        actions
    )
VALUES
    ($1, $2, $3, $4, $5)
",
    )
    .bind(operation_id)
    .bind(step_index)
    .bind(transaction.signer_account_id.0.to_string())
    .bind(transaction.receiver_id.to_string())
    .bind(actions)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_step_execution(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operation_id: uuid::Uuid,
    step_index: i32,
    execution: StepExecution<'_>,
) -> GatewayResult<()> {
    sqlx::query(
        r"
INSERT INTO
    gateway_step_executions (
        operation_id,
        step_index,
        tx_hash,
        signed_transaction,
        prepared_at,
        submitted_at
    )
VALUES
    ($1, $2, $3, $4, $5, $6)
",
    )
    .bind(operation_id)
    .bind(step_index)
    .bind(execution.tx_hash.0.to_string())
    .bind(execution.signed_transaction)
    .bind(execution.prepared_at)
    .bind(execution.submitted_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_step_result(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operation_id: uuid::Uuid,
    step_index: i32,
    result: StepResult,
) -> GatewayResult<()> {
    sqlx::query(
        r"
INSERT INTO
    gateway_step_results (operation_id, step_index, tx_hash)
VALUES
    ($1, $2, $3)
",
    )
    .bind(operation_id)
    .bind(step_index)
    .bind(result.tx_hash.0.to_string())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_step_outcome(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operation_id: uuid::Uuid,
    step_index: i32,
    outcome: StepOutcome<'_>,
) -> GatewayResult<()> {
    sqlx::query(
        r"
INSERT INTO
    gateway_step_outcomes (
        operation_id,
        step_index,
        status,
        tokens_burnt,
        total_gas_burnt,
        return_value
    )
VALUES
    ($1, $2, $3, $4, $5, $6)
",
    )
    .bind(operation_id)
    .bind(step_index)
    .bind(outcome.status)
    .bind(outcome.outcome.tokens_burnt.as_yoctonear().to_string())
    .bind(outcome.outcome.total_gas_burnt.as_gas().to_string())
    .bind(
        outcome
            .outcome
            .return_value
            .as_ref()
            .map(|bytes| bytes.0.clone()),
    )
    .execute(&mut **tx)
    .await?;

    for (receipt_index, receipt) in outcome.outcome.receipts.iter().enumerate() {
        insert_step_receipt(tx, operation_id, step_index, receipt_index, receipt).await?;
    }
    Ok(())
}

async fn insert_step_receipt(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operation_id: uuid::Uuid,
    step_index: i32,
    receipt_index: usize,
    receipt: &ReceiptOutcome,
) -> GatewayResult<()> {
    sqlx::query(
        r"
INSERT INTO
    gateway_step_receipts (
        operation_id,
        step_index,
        receipt_index,
        contract_id,
        status,
        logs
    )
VALUES
    ($1, $2, $3, $4, $5, $6)
",
    )
    .bind(operation_id)
    .bind(step_index)
    .bind(i32::try_from(receipt_index).map_err(|_| {
        GatewayError::InvalidStoredOperation("receipt index exceeds i32 range".to_owned())
    })?)
    .bind(receipt.contract_id.to_string())
    .bind(outcome_status_row(receipt.status))
    .bind(&receipt.logs)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn parse_operation_uuid(operation_id: &OperationId) -> GatewayResult<uuid::Uuid> {
    uuid::Uuid::from_str(&operation_id.0)
        .map_err(|error| GatewayError::InvalidStoredOperation(error.to_string()))
}

fn parse_account_id(value: &str) -> GatewayResult<near_account_id::AccountId> {
    value
        .parse::<near_account_id::AccountId>()
        .map_err(|error| GatewayError::InvalidStoredOperation(error.to_string()))
}

fn outcome_status_row(status: ReceiptStatus) -> OutcomeStatusRow {
    match status {
        ReceiptStatus::Succeeded => OutcomeStatusRow::Succeeded,
        ReceiptStatus::Failed => OutcomeStatusRow::Failed,
    }
}

fn parse_receipt_status(status: OutcomeStatusRow) -> ReceiptStatus {
    match status {
        OutcomeStatusRow::Succeeded => ReceiptStatus::Succeeded,
        OutcomeStatusRow::Failed => ReceiptStatus::Failed,
    }
}

async fn load_step_rows(
    pool: &PgPool,
    operation_id: uuid::Uuid,
) -> GatewayResult<Vec<StepLifecycleRow>> {
    sqlx::query_as::<_, StepLifecycleRow>(
        r"
SELECT
    step.operation_id,
    step.step_index,
    step.signer_account_id,
    step.receiver_id,
    step.actions,
    execution.tx_hash AS execution_tx_hash,
    execution.signed_transaction,
    execution.submitted_at,
    result.tx_hash AS result_tx_hash,
    outcome.status AS outcome_status,
    outcome.tokens_burnt,
    outcome.total_gas_burnt,
    outcome.return_value,
    step.created_at
FROM
    gateway_plan_steps AS step
    LEFT JOIN gateway_step_executions AS execution ON execution.operation_id = step.operation_id
        AND execution.step_index = step.step_index
    LEFT JOIN gateway_step_results AS result ON result.operation_id = step.operation_id
        AND result.step_index = step.step_index
    LEFT JOIN gateway_step_outcomes AS outcome ON outcome.operation_id = step.operation_id
        AND outcome.step_index = step.step_index
WHERE
    step.operation_id = $1
ORDER BY
    step.step_index ASC
",
    )
    .bind(operation_id)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

/// Load every step's receipts for an operation, grouped by `step_index` and
/// ordered by `receipt_index`.
async fn load_step_receipts(pool: &PgPool, operation_id: uuid::Uuid) -> GatewayResult<ReceiptMap> {
    let rows = sqlx::query_as::<_, ReceiptRow>(
        r"
SELECT
    step_index,
    contract_id,
    status,
    logs
FROM
    gateway_step_receipts
WHERE
    operation_id = $1
ORDER BY
    step_index ASC,
    receipt_index ASC
",
    )
    .bind(operation_id)
    .fetch_all(pool)
    .await?;

    let mut by_step = ReceiptMap::new();
    for row in rows {
        let contract_id = parse_account_id(&row.contract_id)?;
        by_step
            .entry(row.step_index)
            .or_default()
            .push(ReceiptOutcome {
                contract_id,
                status: parse_receipt_status(row.status),
                logs: row.logs,
            });
    }
    Ok(by_step)
}

fn rows_to_stored_operation(
    operation_row: OperationRow,
    step_rows: Vec<StepLifecycleRow>,
    mut receipts_by_step: ReceiptMap,
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
    let signer_account_id = ManagedAccountId(parse_account_id(&operation_row.signer_account_id)?);
    let request_payload = serde_json::to_vec(&operation_row.request_payload)?;

    let mut request_fingerprint_hash = [0_u8; 32];
    if operation_row.request_fingerprint_hash.len() != request_fingerprint_hash.len() {
        return Err(GatewayError::InvalidStoredOperation(
            "request fingerprint hash must be 32 bytes".to_owned(),
        ));
    }
    request_fingerprint_hash.copy_from_slice(&operation_row.request_fingerprint_hash);

    Ok(StoredOperation {
        rpc_method: operation_row.rpc_method,
        request_fingerprint_hash,
        request_payload,
        id,
        signer_account_id,
        planned: operation_row.plan_created_at.is_some(),
        succeeded_steps,
        current_step,
        remaining_steps,
    })
}

fn apply_step_row(
    row: StepLifecycleRow,
    receipts: Vec<ReceiptOutcome>,
    succeeded_steps: &mut Vec<SucceededStep>,
    current_step: &mut Option<CurrentStep>,
    remaining_steps: &mut VecDeque<PlannedTransaction>,
) -> GatewayResult<()> {
    let transaction = step_row_transaction(&row)?;

    match row_lifecycle(&row)? {
        RowLifecycle::Remaining => remaining_steps.push_back(transaction),
        RowLifecycle::Prepared { tx_hash } => {
            let signed_transaction = parse_signed_transaction(row.signed_transaction)?;
            *current_step = Some(CurrentStep::Prepared {
                transaction,
                signed_transaction: Box::new(signed_transaction),
                tx_hash,
            });
        }
        RowLifecycle::Submitted {
            tx_hash,
            submitted_at,
        } => {
            *current_step = Some(CurrentStep::Submitted {
                transaction,
                tx_hash,
                submitted_at,
            });
        }
        RowLifecycle::Rejected { tx_hash } => {
            *current_step = Some(CurrentStep::Rejected {
                transaction,
                tx_hash,
            });
        }
        RowLifecycle::Reverted { tx_hash } => {
            *current_step = Some(CurrentStep::Reverted {
                transaction,
                tx_hash,
                outcome: build_outcome(&row, receipts)?,
            });
        }
        RowLifecycle::Succeeded { tx_hash } => {
            succeeded_steps.push(SucceededStep {
                transaction,
                tx_hash,
                outcome: build_outcome(&row, receipts)?,
            });
        }
    }
    Ok(())
}

enum RowLifecycle {
    Remaining,
    Prepared {
        tx_hash: CryptoHash,
    },
    Submitted {
        tx_hash: CryptoHash,
        submitted_at: DateTime<Utc>,
    },
    Rejected {
        tx_hash: CryptoHash,
    },
    Reverted {
        tx_hash: CryptoHash,
    },
    Succeeded {
        tx_hash: CryptoHash,
    },
}

fn row_lifecycle(row: &StepLifecycleRow) -> GatewayResult<RowLifecycle> {
    let Some(execution_tx_hash) = parse_crypto_hash(row.execution_tx_hash.as_deref())? else {
        return Ok(RowLifecycle::Remaining);
    };

    let Some(result_tx_hash) = parse_crypto_hash(row.result_tx_hash.as_deref())? else {
        return Ok(match row.submitted_at {
            Some(submitted_at) => RowLifecycle::Submitted {
                tx_hash: execution_tx_hash,
                submitted_at,
            },
            None => RowLifecycle::Prepared {
                tx_hash: execution_tx_hash,
            },
        });
    };

    if result_tx_hash != execution_tx_hash {
        return Err(GatewayError::InvalidStoredOperation(format!(
            "step {} result tx_hash does not match execution tx_hash",
            row.step_index
        )));
    }

    Ok(match row.outcome_status {
        None => RowLifecycle::Rejected {
            tx_hash: result_tx_hash,
        },
        Some(OutcomeStatusRow::Failed) => RowLifecycle::Reverted {
            tx_hash: result_tx_hash,
        },
        Some(OutcomeStatusRow::Succeeded) => RowLifecycle::Succeeded {
            tx_hash: result_tx_hash,
        },
    })
}

fn build_outcome(
    row: &StepLifecycleRow,
    receipts: Vec<ReceiptOutcome>,
) -> GatewayResult<ExecutionOutcome> {
    let tokens = row.tokens_burnt.as_deref().ok_or_else(|| {
        GatewayError::InvalidStoredOperation("executed step missing tokens_burnt".to_owned())
    })?;
    let gas = row.total_gas_burnt.as_deref().ok_or_else(|| {
        GatewayError::InvalidStoredOperation("executed step missing total_gas_burnt".to_owned())
    })?;
    let tokens_burnt = NearToken::from_yoctonear(tokens.parse().map_err(|_| {
        GatewayError::InvalidStoredOperation(format!("invalid tokens_burnt {tokens:?}"))
    })?);
    let total_gas_burnt = NearGas::from_gas(gas.parse().map_err(|_| {
        GatewayError::InvalidStoredOperation(format!("invalid total_gas_burnt {gas:?}"))
    })?);
    Ok(ExecutionOutcome {
        tokens_burnt,
        total_gas_burnt,
        receipts,
        return_value: row.return_value.clone().map(Into::into),
    })
}

fn step_row_transaction(row: &StepLifecycleRow) -> GatewayResult<PlannedTransaction> {
    Ok(PlannedTransaction {
        signer_account_id: ManagedAccountId(parse_account_id(&row.signer_account_id)?),
        receiver_id: parse_account_id(&row.receiver_id)?,
        actions: serde_json::from_value(row.actions.clone())?,
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

#[cfg(test)]
mod tests {
    use near_api::types::transaction::{
        actions::{Action, TransferAction},
        SignedTransaction, Transaction, TransactionV0,
    };
    use near_api::types::CryptoHash as NearCryptoHash;
    use templar_gateway_types::{NearGas, NearToken, OperationStatus};

    use super::*;

    fn sample_transaction() -> PlannedTransaction {
        PlannedTransaction::single_action(
            ManagedAccountId("signer.near".parse().unwrap()),
            "receiver.near".parse().unwrap(),
            Action::Transfer(TransferAction {
                deposit: NearToken::from_yoctonear(7),
            }),
        )
    }

    fn dummy_signed_transaction() -> SignedTransaction {
        let transaction = Transaction::V0(TransactionV0 {
            signer_id: "signer.near".parse().unwrap(),
            public_key: "ed25519:11111111111111111111111111111111".parse().unwrap(),
            nonce: 1,
            receiver_id: "receiver.near".parse().unwrap(),
            block_hash: NearCryptoHash::default(),
            actions: vec![],
        });
        let signature = "ed25519:1111111111111111111111111111111111111111111111111111111111111111"
            .parse()
            .unwrap();
        SignedTransaction::new(signature, transaction)
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
                planned: true,
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
                planned: true,
                succeeded_steps: vec![],
                current_step: Some(CurrentStep::Submitted {
                    transaction,
                    tx_hash: CryptoHash(NearCryptoHash::default()),
                    submitted_at: Utc::now(),
                }),
                remaining_steps: VecDeque::new(),
            },
            OperationStatus::Succeeded => StoredOperation {
                rpc_method: "tx.transfer".to_owned(),
                request_fingerprint_hash: [3; 32],
                request_payload: serde_json::to_vec(&serde_json::json!({ "amount": "9" })).unwrap(),
                id: OperationId(uuid::Uuid::new_v4().to_string()),
                signer_account_id: ManagedAccountId("signer.near".parse().unwrap()),
                planned: true,
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
                planned: true,
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

    async fn prepare_first_step(store: &PostgresStore, operation: &mut StoredOperation) {
        let transaction = operation
            .remaining_steps
            .pop_front()
            .expect("sample operation has a first step");
        operation.current_step = Some(CurrentStep::Prepared {
            transaction,
            signed_transaction: Box::new(dummy_signed_transaction()),
            tx_hash: CryptoHash(NearCryptoHash::default()),
        });
        store.save_operation(operation.clone()).await.unwrap();
    }

    /// A progress save must not drop the operation's idempotency key — the
    /// relayer's crash recovery (and gateway idempotent retries) look operations
    /// up by it.
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
        let CreateOperationResult::Created(mut operation) = created else {
            panic!("expected a freshly created operation");
        };

        prepare_first_step(&store, &mut operation).await;
        let prepared = operation.current_step.take().unwrap();
        let CurrentStep::Prepared {
            transaction,
            tx_hash,
            ..
        } = prepared
        else {
            panic!("expected prepared step");
        };
        operation.succeeded_steps.push(SucceededStep {
            transaction,
            tx_hash,
            outcome: sample_outcome(),
        });
        store.save_operation(operation.clone()).await.unwrap();

        let found = store
            .get_by_idempotency_key(&key)
            .await
            .unwrap()
            .expect("operation still resolvable by idempotency key after a save");
        assert_eq!(found.id, operation.id);
        assert_eq!(found.status(), OperationStatus::Succeeded);
    }

    /// `delete_reservation` removes a bare reservation (clearing its idempotency
    /// mapping) but refuses to touch an operation that has a structural plan.
    #[sqlx::test(migrations = "./migrations")]
    async fn delete_reservation_only_removes_reservations(pool: PgPool) {
        let store = PostgresStore {
            pool,
            schema: "public".to_owned(),
        };

        // A reservation: created with an empty plan, no plan row.
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

        let executed = persist_failed_step(
            &store,
            [6; 32],
            CurrentStep::Reverted {
                transaction: sample_transaction(),
                tx_hash: CryptoHash(NearCryptoHash::default()),
                outcome: sample_outcome(),
            },
        )
        .await;

        store.delete_reservation(&reserved.id).await.unwrap();
        assert!(store.get_by_id(&reserved.id).await.unwrap().is_none());
        assert!(store.get_by_idempotency_key(&key).await.unwrap().is_none());

        store.delete_reservation(&executed.id).await.unwrap();
        assert!(store.get_by_id(&executed.id).await.unwrap().is_some());
    }

    /// Reconstructing a failed operation must preserve the billing-relevant
    /// distinction the broom keys on: `Reverted` carries an outcome (settle the
    /// gas burnt), `Rejected` carries none (release the charge).
    #[sqlx::test(migrations = "./migrations")]
    async fn reconstructs_rejected_distinct_from_reverted(pool: PgPool) {
        let store = PostgresStore {
            pool,
            schema: "public".to_owned(),
        };

        let reverted = persist_failed_step(
            &store,
            [10; 32],
            CurrentStep::Reverted {
                transaction: sample_transaction(),
                tx_hash: CryptoHash(NearCryptoHash::default()),
                outcome: sample_outcome(),
            },
        )
        .await;
        assert!(matches!(
            reverted.current_step,
            Some(CurrentStep::Reverted { .. })
        ));
        assert_eq!(reverted.status(), OperationStatus::Failed);
        assert!(
            reverted.record().final_outcome().is_some(),
            "a reverted step must settle the gas burnt"
        );

        let rejected = persist_failed_step(
            &store,
            [11; 32],
            CurrentStep::Rejected {
                transaction: sample_transaction(),
                tx_hash: CryptoHash(NearCryptoHash::default()),
            },
        )
        .await;
        assert!(matches!(
            rejected.current_step,
            Some(CurrentStep::Rejected { .. })
        ));
        assert_eq!(rejected.status(), OperationStatus::Failed);
        assert!(
            rejected.record().final_outcome().is_none(),
            "a rejected step must release the charge"
        );
    }

    /// A planned no-op (empty plan) persists as a terminal `Succeeded` operation
    /// with no steps, keeps resolving by idempotency key (so a retry dedups
    /// instead of re-planning), and is not removed by `delete_reservation`.
    #[sqlx::test(migrations = "./migrations")]
    async fn planned_noop_persists_and_dedups(pool: PgPool) {
        let store = PostgresStore {
            pool,
            schema: "public".to_owned(),
        };
        let key = IdempotencyKey("noop-key".to_owned());

        let CreateOperationResult::Created(mut operation) = store
            .create_or_get_operation(
                "storage.ensureDeposit",
                ManagedAccountId("signer.near".parse().unwrap()),
                Some(key.clone()),
                [7; 32],
                serde_json::to_vec(&serde_json::json!({})).unwrap(),
                OperationPlan { steps: vec![] },
            )
            .await
            .unwrap()
        else {
            panic!("expected a freshly created reservation");
        };
        assert_eq!(operation.status(), OperationStatus::Pending);
        operation.planned = true;
        store.save_operation(operation.clone()).await.unwrap();

        let reloaded = store
            .get_by_idempotency_key(&key)
            .await
            .unwrap()
            .expect("no-op persisted");
        assert_eq!(reloaded.status(), OperationStatus::Succeeded);
        assert!(reloaded.record().steps.is_empty());

        store.delete_reservation(&operation.id).await.unwrap();
        assert!(store.get_by_id(&operation.id).await.unwrap().is_some());
    }

    /// Persist a failed operation with the given terminal `current_step` and read
    /// it back, exercising the store's structural step reconstruction.
    async fn persist_failed_step(
        store: &PostgresStore,
        fingerprint: [u8; 32],
        current_step: CurrentStep,
    ) -> StoredOperation {
        let CreateOperationResult::Created(mut operation) = store
            .create_or_get_operation(
                "tx.transfer",
                ManagedAccountId("signer.near".parse().unwrap()),
                None,
                fingerprint,
                serde_json::to_vec(&serde_json::json!({})).unwrap(),
                OperationPlan {
                    steps: vec![sample_transaction()],
                },
            )
            .await
            .unwrap()
        else {
            panic!("expected a freshly created operation");
        };
        prepare_first_step(store, &mut operation).await;
        operation.current_step = Some(current_step);
        store.save_operation(operation.clone()).await.unwrap();
        store
            .get_by_id(&operation.id)
            .await
            .unwrap()
            .expect("operation persisted")
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
    fn outcome_status_row_matches_receipt_status() {
        assert_eq!(
            outcome_status_row(ReceiptStatus::Succeeded),
            OutcomeStatusRow::Succeeded
        );
        assert_eq!(
            outcome_status_row(ReceiptStatus::Failed),
            OutcomeStatusRow::Failed
        );
    }

    #[test]
    fn rows_round_trip_preserves_succeeded_operation() {
        let operation = sample_operation(OperationStatus::Succeeded);
        let succeeded_step = operation.succeeded_steps.first().unwrap();
        let operation_row = OperationRow {
            id: uuid::Uuid::from_str(&operation.id.0).unwrap(),
            rpc_method: operation.rpc_method.clone(),
            signer_account_id: operation.signer_account_id.0.to_string(),
            idempotency_key: None,
            request_fingerprint_hash: operation.request_fingerprint_hash.to_vec(),
            request_payload: serde_json::from_slice(&operation.request_payload).unwrap(),
            plan_created_at: Some(Utc::now()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let step_rows = vec![StepLifecycleRow {
            operation_id: operation_row.id,
            step_index: 0,
            signer_account_id: operation.signer_account_id.0.to_string(),
            receiver_id: succeeded_step.transaction.receiver_id.to_string(),
            actions: serde_json::to_value(&succeeded_step.transaction.actions).unwrap(),
            execution_tx_hash: Some(succeeded_step.tx_hash.0.to_string()),
            signed_transaction: Some(to_vec(&dummy_signed_transaction()).unwrap()),
            submitted_at: Some(Utc::now()),
            result_tx_hash: Some(succeeded_step.tx_hash.0.to_string()),
            outcome_status: Some(OutcomeStatusRow::Succeeded),
            tokens_burnt: Some(sample_outcome().tokens_burnt.as_yoctonear().to_string()),
            total_gas_burnt: Some(sample_outcome().total_gas_burnt.as_gas().to_string()),
            return_value: None,
            created_at: Utc::now(),
        }];
        let receipts = ReceiptMap::from([(0_i32, sample_outcome().receipts)]);

        let restored = rows_to_stored_operation(operation_row, step_rows, receipts).unwrap();
        assert_eq!(restored.status(), OperationStatus::Succeeded);
        assert_eq!(restored.succeeded_steps.len(), 1);
        assert_eq!(
            restored.succeeded_steps.first().unwrap().outcome,
            sample_outcome()
        );
    }
}
