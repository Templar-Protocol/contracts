-- down
ALTER TABLE
    account DROP CONSTRAINT fk__account__transaction,
ADD
    COLUMN allowance_locked numeric(39, 0) NOT NULL DEFAULT 0,
ADD
    COLUMN pending_transaction_issued_at timestamptz DEFAULT NULL;

ALTER TABLE
    "transaction"
ADD
    COLUMN succeeded bool;

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
    DROP COLUMN allowance_spent_inner,
ADD
    COLUMN id uuid NOT NULL DEFAULT gen_random_uuid(),
    DROP CONSTRAINT pk__transaction,
ADD
    CONSTRAINT pk__call PRIMARY KEY (id),
    DROP COLUMN STATUS;

DROP TYPE transaction_status;

ALTER TABLE
    "transaction" RENAME TO call;
