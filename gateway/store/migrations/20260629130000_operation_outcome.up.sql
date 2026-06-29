-- up
-- Persist a step's execution outcome relationally (DB-enforced shape) rather
-- than as an opaque blob: scalar fields on the step row, and one row per receipt
-- in a child table. The outcome is present for succeeded and reverted steps and
-- absent for steps that never executed (NotStarted/Prepared/Submitted/Rejected).
--
-- u128/u64 amounts are stored as decimal text (lossless, no numeric/bigint
-- juggling); the return value is the raw success bytes.
ALTER TABLE gateway_operation_steps
    ADD COLUMN IF NOT EXISTS outcome_tokens_burnt text,
    ADD COLUMN IF NOT EXISTS outcome_total_gas_burnt text,
    ADD COLUMN IF NOT EXISTS outcome_return_value bytea,
    ADD CONSTRAINT gateway_operation_steps_outcome_check CHECK (
        -- The two required scalars are all-or-nothing: a step either has an
        -- execution outcome or it doesn't.
        (outcome_tokens_burnt IS NULL) = (outcome_total_gas_burnt IS NULL)
    );

CREATE TABLE IF NOT EXISTS gateway_operation_step_receipts (
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
