-- up
-- Mark when an operation finished planning. NULL = a reservation: the row was
-- created to hold the idempotency key before planning ran, and planning has not
-- yet produced (or abandoned) a plan. Once planning completes the column is set,
-- even for a no-op plan -- so a planned no-op (0 steps, terminal) is no longer
-- indistinguishable from a bare reservation (0 steps, in flight).
ALTER TABLE
    gateway_operations
ADD
    COLUMN IF NOT EXISTS planned_at timestamptz;

-- Backfill: every row that predates this column was already planned
-- (reservations are new). Without this, existing rows read back as bare
-- reservations -- reaped by recovery or given the wrong status.
UPDATE
    gateway_operations
SET
    planned_at = created_at
WHERE
    planned_at IS NULL;
