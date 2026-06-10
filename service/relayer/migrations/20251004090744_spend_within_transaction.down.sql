-- down
ALTER TABLE
    account DROP CONSTRAINT IF EXISTS fk__account__transaction,
ADD
    COLUMN IF NOT EXISTS allowance_locked numeric(39, 0) NOT NULL DEFAULT 0,
ADD
    COLUMN IF NOT EXISTS pending_transaction_issued_at timestamptz DEFAULT NULL;

DROP INDEX IF EXISTS uq__max_one_pending_tx_per_account;

ALTER TABLE
    "transaction"
ADD
    COLUMN IF NOT EXISTS succeeded bool;

UPDATE
    "transaction"
SET
    succeeded = CASE
        WHEN STATUS = 'succeeded' THEN TRUE
        ELSE false
    END,
    allowance_spent_gas = allowance_spent_gas + allowance_spent_inner;

ALTER TABLE
    "transaction" RENAME COLUMN allowance_spent_gas TO allowance_spent;

ALTER TABLE
    "transaction"
ALTER COLUMN
    succeeded
SET
    NOT NULL,
    DROP COLUMN IF EXISTS allowance_spent_inner,
ADD
    COLUMN IF NOT EXISTS id uuid NOT NULL DEFAULT gen_random_uuid(),
    DROP CONSTRAINT pk__transaction,
ADD
    CONSTRAINT pk__call PRIMARY KEY (id),
    DROP COLUMN "status";

DROP TYPE IF EXISTS transaction_status;

ALTER TABLE
    "transaction" RENAME TO call;
