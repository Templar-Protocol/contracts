-- down
-- Restore the pre-slim shape (an empty "transaction" table + status enum). This
-- only restores the structure, not data.

-- Refuse to roll back while UUID-backed charges are still in flight: the
-- restored (old) schema can't reconcile them, so they would leak. Let them
-- settle first.
DO
$$
BEGIN
    IF EXISTS (SELECT 1 FROM account WHERE pending_operation_key IS NOT NULL) THEN
        RAISE EXCEPTION
            'refusing rollback: % account(s) have an in-flight charge; let them settle first',
            (SELECT count(*) FROM account WHERE pending_operation_key IS NOT NULL);
    END IF;
END
$$
;

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
