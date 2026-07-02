DO
$$
BEGIN
CREATE TYPE gateway_outcome_status AS ENUM (
    'succeeded',
    'failed'
);

EXCEPTION
WHEN duplicate_object THEN NULL;

END
$$
;

-- This migration is intentionally greenfield for gateway operation state. If a
-- database has operation rows from an earlier operation-store shape, they cannot
-- be reconstructed into structural plans/executions/results safely. Refuse
-- loudly instead of treating legacy rows as unplanned reservations.
DO
$$
BEGIN
    IF EXISTS (SELECT 1 FROM gateway_operations) THEN
        RAISE EXCEPTION 'gateway_operation_lifecycle migration requires an empty gateway_operations table: legacy rows predate structural plan/execution/result tables and cannot be migrated. Drain and clear gateway operation state before upgrading.';
    END IF;
END
$$
;

CREATE TABLE IF NOT EXISTS gateway_operation_plans (
    operation_id uuid PRIMARY KEY REFERENCES gateway_operations(id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS gateway_plan_steps (
    operation_id uuid NOT NULL,
    step_index integer NOT NULL,
    signer_account_id text NOT NULL,
    receiver_id text NOT NULL,
    actions jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT NOW(),
    PRIMARY KEY (operation_id, step_index),
    FOREIGN KEY (operation_id)
        REFERENCES gateway_operation_plans (operation_id) ON DELETE CASCADE,
    CONSTRAINT gateway_plan_steps_step_index_check CHECK (step_index >= 0),
    CONSTRAINT gateway_plan_steps_actions_array_check CHECK (jsonb_typeof(actions) = 'array')
);

CREATE TABLE IF NOT EXISTS gateway_step_executions (
    operation_id uuid NOT NULL,
    step_index integer NOT NULL,
    tx_hash text NOT NULL,
    signed_transaction bytea NOT NULL,
    prepared_at timestamptz NOT NULL DEFAULT NOW(),
    submitted_at timestamptz,
    updated_at timestamptz NOT NULL DEFAULT NOW(),
    PRIMARY KEY (operation_id, step_index),
    FOREIGN KEY (operation_id, step_index)
        REFERENCES gateway_plan_steps (operation_id, step_index) ON DELETE CASCADE,
    CONSTRAINT gateway_step_executions_submitted_after_prepared_check
        CHECK (submitted_at IS NULL OR submitted_at >= prepared_at)
);

CREATE TABLE IF NOT EXISTS gateway_step_results (
    operation_id uuid NOT NULL,
    step_index integer NOT NULL,
    tx_hash text NOT NULL,
    completed_at timestamptz NOT NULL DEFAULT NOW(),
    PRIMARY KEY (operation_id, step_index),
    FOREIGN KEY (operation_id, step_index)
        REFERENCES gateway_step_executions (operation_id, step_index) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS gateway_step_outcomes (
    operation_id uuid NOT NULL,
    step_index integer NOT NULL,
    status gateway_outcome_status NOT NULL,
    tokens_burnt text NOT NULL,
    total_gas_burnt text NOT NULL,
    return_value bytea,
    PRIMARY KEY (operation_id, step_index),
    FOREIGN KEY (operation_id, step_index)
        REFERENCES gateway_step_results (operation_id, step_index) ON DELETE CASCADE,
    CONSTRAINT gateway_step_outcomes_tokens_burnt_unsigned_decimal_check
        CHECK (tokens_burnt ~ '^[0-9]+$'),
    CONSTRAINT gateway_step_outcomes_total_gas_burnt_unsigned_decimal_check
        CHECK (total_gas_burnt ~ '^[0-9]+$')
);

CREATE TABLE IF NOT EXISTS gateway_step_receipts (
    operation_id uuid NOT NULL,
    step_index integer NOT NULL,
    receipt_index integer NOT NULL,
    contract_id text NOT NULL,
    status gateway_outcome_status NOT NULL,
    logs text[] NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT NOW(),
    PRIMARY KEY (operation_id, step_index, receipt_index),
    FOREIGN KEY (operation_id, step_index)
        REFERENCES gateway_step_outcomes (operation_id, step_index) ON DELETE CASCADE,
    CONSTRAINT gateway_step_receipts_receipt_index_check CHECK (receipt_index >= 0)
);
