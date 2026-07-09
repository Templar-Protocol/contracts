-- A step may be marked `continue_on_failure`: an on-chain revert of it is
-- recorded and tolerated (the operation advances to the next step) rather than
-- aborting the operation. Additive and backward-compatible — pre-existing steps
-- default to `false` (all-or-nothing), which is their historical behavior.
ALTER TABLE gateway_plan_steps
    ADD COLUMN continue_on_failure boolean NOT NULL DEFAULT false;
