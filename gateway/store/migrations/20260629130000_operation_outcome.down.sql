-- down
DROP TABLE IF EXISTS gateway_operation_step_receipts;

ALTER TABLE gateway_operation_steps
    DROP CONSTRAINT IF EXISTS gateway_operation_steps_outcome_check,
    DROP COLUMN IF EXISTS outcome_tokens_burnt,
    DROP COLUMN IF EXISTS outcome_total_gas_burnt,
    DROP COLUMN IF EXISTS outcome_return_value;
