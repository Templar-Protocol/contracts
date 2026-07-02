-- down
-- Restore the pre-slim shape (an empty "transaction" table + status enum). This
-- only restores the structure, not data.

-- Refuse to roll back while ledger-backed charges are still in flight: the
-- restored (old) schema can't reconcile them, so they would leak. Let them
-- settle first.
DO
$$
BEGIN
    IF to_regclass('pending_gateway_charge') IS NOT NULL
        AND EXISTS (SELECT 1 FROM pending_gateway_charge) THEN
        RAISE EXCEPTION
            'refusing rollback: % in-flight charge(s) remain; let them settle first',
            (SELECT count(*) FROM pending_gateway_charge);
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
    account ADD
    COLUMN IF NOT EXISTS pending_transaction_hash varchar(45);

DROP TABLE IF EXISTS pending_gateway_charge;

ALTER TABLE
    account
ADD
    CONSTRAINT fk__account__transaction FOREIGN KEY (pending_transaction_hash) REFERENCES "transaction" (transaction_hash);
