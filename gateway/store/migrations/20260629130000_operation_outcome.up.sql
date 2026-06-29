-- up
-- Persist the per-step execution outcome (return value, per-receipt logs,
-- gas/tokens burnt) captured from a transaction's submission result, so a
-- reloaded step is reconstructable without a follow-up tx.get. Present for
-- succeeded and reverted steps; null for steps that never executed.
ALTER TABLE gateway_operation_steps
ADD COLUMN IF NOT EXISTS outcome jsonb;
