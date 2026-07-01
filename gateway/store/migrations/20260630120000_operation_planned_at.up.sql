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
