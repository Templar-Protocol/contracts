-- down
-- Restore the pre-slim shape (an empty "transaction" table + status enum). The
-- rows dropped by the up migration are not recoverable; this only restores the
-- structure.
DO
$$
BEGIN
CREATE TYPE transaction_status AS enum ('pending', 'succeeded', 'failed');

EXCEPTION
WHEN duplicate_object THEN NULL;

END
$$
;

CREATE TABLE IF NOT EXISTS "transaction" (
    account_id varchar(64) NOT NULL,
    transaction_hash varchar(45) NOT NULL,
    allowance_spent_gas numeric(39, 0) NOT NULL,
    "status" transaction_status NOT NULL,
    allowance_spent_inner numeric(39, 0) NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT pk__transaction PRIMARY KEY (transaction_hash),
    FOREIGN KEY (account_id) REFERENCES account (account_id)
);

CREATE UNIQUE INDEX IF NOT EXISTS uq__max_one_pending_tx_per_account ON "transaction" (account_id)
WHERE
    "status" = 'pending'::transaction_status;

ALTER TABLE
    account DROP COLUMN IF EXISTS pending_inner_spend,
    DROP COLUMN IF EXISTS pending_operation_key,
ADD
    COLUMN IF NOT EXISTS pending_transaction_hash varchar(45);

ALTER TABLE
    account
ADD
    CONSTRAINT fk__account__transaction FOREIGN KEY (pending_transaction_hash) REFERENCES "transaction" (transaction_hash);
