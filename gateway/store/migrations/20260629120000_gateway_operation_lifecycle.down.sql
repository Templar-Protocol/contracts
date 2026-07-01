-- down
DROP TABLE IF EXISTS gateway_operation_step_receipts;

ALTER TABLE gateway_operation_steps
    DROP CONSTRAINT IF EXISTS gateway_operation_steps_outcome_check,
    DROP COLUMN IF EXISTS outcome_return_value,
    DROP COLUMN IF EXISTS outcome_total_gas_burnt,
    DROP COLUMN IF EXISTS outcome_tokens_burnt,
    DROP COLUMN IF EXISTS submitted_at,
    -- Restore wait_until (all steps waited for Final by the time it was dropped).
    ADD COLUMN IF NOT EXISTS wait_until text NOT NULL DEFAULT '"Final"';

ALTER TABLE gateway_operation_steps
    ALTER COLUMN wait_until DROP DEFAULT,
    ALTER COLUMN updated_at DROP DEFAULT;

ALTER TABLE gateway_operations
    DROP COLUMN IF EXISTS planned_at,
    ALTER COLUMN updated_at DROP DEFAULT;
