-- up
-- `updated_at` was NOT NULL but, unlike `created_at`, had no default, so the
-- store's inserts (which don't set it explicitly) violated the constraint. Give
-- it the same default; the store re-inserts an operation on every save, so this
-- tracks the last write time.
ALTER TABLE gateway_operations
ALTER COLUMN updated_at
SET DEFAULT NOW();

ALTER TABLE gateway_operation_steps
ALTER COLUMN updated_at
SET DEFAULT NOW();
