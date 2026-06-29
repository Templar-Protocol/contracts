-- down
ALTER TABLE gateway_operation_steps
ALTER COLUMN updated_at
DROP DEFAULT;

ALTER TABLE gateway_operations
ALTER COLUMN updated_at
DROP DEFAULT;
