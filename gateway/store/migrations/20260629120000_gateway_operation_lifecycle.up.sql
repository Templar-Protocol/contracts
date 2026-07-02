-- up
-- Operation-lifecycle columns layered on the base operation-store schema:
-- updated_at defaults, the planned-vs-reservation marker, the relational step
-- execution outcome, and the submitted_at age marker (which replaces the removed
-- per-step wait_until, since every step now waits for final execution).

-- This migration is NOT upgrade-safe for pre-existing operations: it makes
-- `planned_at IS NULL` the reservation sentinel and requires `submitted_at` /
-- outcome columns the base schema never captured, so legacy rows cannot be
-- reconstructed by the loader. The gateway operation store has no released
-- consumer, so a real deployment applies this to an empty table. Refuse loudly if
-- that assumption is violated rather than silently corrupting recovery (a legacy
-- row would otherwise be read as a reservation and deleted, or fail to load).
DO
$$
BEGIN
    IF EXISTS (SELECT 1 FROM gateway_operations) THEN
        RAISE EXCEPTION 'gateway_operation_lifecycle migration requires an empty gateway_operations table: legacy rows predate the planned_at/submitted_at/outcome columns and cannot be migrated. Drain and clear gateway operation state before upgrading.';
    END IF;
END
$$;

-- `updated_at` was NOT NULL but, unlike `created_at`, had no default, so the
-- store's inserts (which don't set it) violated the constraint. The store
-- re-inserts on every save, so this tracks the last write time.
ALTER TABLE gateway_operations
    ALTER COLUMN updated_at SET DEFAULT NOW();

ALTER TABLE gateway_operation_steps
    ALTER COLUMN updated_at SET DEFAULT NOW();

-- `planned_at`: NULL marks a reservation (the row was created to hold the
-- idempotency key before planning ran). It is set once planning completes — even
-- for a no-op plan — so a planned no-op (0 steps, terminal) is distinguishable
-- from a bare reservation (0 steps, in flight).
ALTER TABLE gateway_operations
    ADD COLUMN planned_at timestamptz;

-- A step's execution outcome, persisted relationally (DB-enforced shape) rather
-- than as an opaque blob: scalar fields on the step row, and one row per receipt
-- in a child table. Present for succeeded and reverted steps, absent for steps
-- that never executed. u128/u64 amounts are lossless decimal text; the return
-- value is the raw success bytes.
--
-- `submitted_at` records when a step entered the 'submitted' state, so
-- reconciliation can age a transaction the chain never records into a rejection.
-- NULL for every non-submitted state. `wait_until` is dropped: all steps now wait
-- for final execution.
ALTER TABLE gateway_operation_steps
    DROP COLUMN wait_until,
    ADD COLUMN submitted_at timestamptz,
    ADD COLUMN outcome_tokens_burnt text,
    ADD COLUMN outcome_total_gas_burnt text,
    ADD COLUMN outcome_return_value bytea,
    ADD CONSTRAINT gateway_operation_steps_outcome_check CHECK (
        -- All-or-nothing: a step either has an execution outcome or it doesn't.
        (outcome_tokens_burnt IS NULL) = (outcome_total_gas_burnt IS NULL)
    );

CREATE TABLE gateway_operation_step_receipts (
    operation_id uuid NOT NULL,
    step_index integer NOT NULL,
    receipt_index integer NOT NULL,
    contract_id text NOT NULL,
    status text NOT NULL,
    logs text[] NOT NULL,
    created_at timestamptz NOT NULL DEFAULT NOW(),
    PRIMARY KEY (operation_id, step_index, receipt_index),
    FOREIGN KEY (operation_id, step_index)
        REFERENCES gateway_operation_steps (operation_id, step_index) ON DELETE CASCADE,
    CONSTRAINT gateway_operation_step_receipts_status_check
        CHECK (status IN ('succeeded', 'failed'))
);
