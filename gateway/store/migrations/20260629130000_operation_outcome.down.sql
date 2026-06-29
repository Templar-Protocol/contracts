-- down
ALTER TABLE gateway_operation_steps
DROP COLUMN IF EXISTS outcome;
