-- up

ALTER TABLE
    call RENAME TO "transaction";

CREATE TYPE transaction_status AS enum ('pending', 'succeeded', 'failed');

ALTER TABLE
    "transaction"
ADD
    COLUMN "status" transaction_status;

UPDATE
    "transaction"
SET
    "status" = CASE
        WHEN succeeded = TRUE THEN 'succeeded'::transaction_status
        ELSE 'failed'
    END;

ALTER TABLE
    "transaction" RENAME COLUMN allowance_spent TO allowance_spent_gas;

ALTER TABLE
    "transaction"
ALTER COLUMN
    "status"
SET
    NOT NULL,
    DROP COLUMN id,
ADD
    CONSTRAINT pk__transaction PRIMARY KEY (transaction_hash),
    DROP COLUMN succeeded,
ADD
    COLUMN allowance_spent_inner numeric(39, 0) NOT NULL DEFAULT 0;

CREATE UNIQUE INDEX uq__max_one_pending_tx_per_account ON "transaction" (account_id) WHERE "status" = 'pending'::transaction_status;

ALTER TABLE
    account DROP COLUMN allowance_locked,
    DROP COLUMN pending_transaction_issued_at,
ADD
    CONSTRAINT fk__account__transaction FOREIGN KEY (pending_transaction_hash) REFERENCES "transaction" (transaction_hash);
