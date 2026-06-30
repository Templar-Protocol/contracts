-- down
ALTER TABLE account
DROP CONSTRAINT IF EXISTS fk__account__transaction;

ALTER TABLE account
ADD COLUMN IF NOT EXISTS pending_transaction_hash varchar(45);

UPDATE account a
SET pending_transaction_hash = t.transaction_hash
FROM "transaction" t
WHERE a.pending_operation_key = t.operation_key;

ALTER TABLE account
DROP COLUMN IF EXISTS pending_operation_key;

-- The old schema requires every transaction to have a hash. Rather than
-- silently dropping in-flight (hashless) rows — losing pending accounting state
-- — fail the downgrade so an operator drains/resolves them first.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM "transaction" WHERE transaction_hash IS NULL) THEN
        RAISE EXCEPTION
            'cannot downgrade: % transaction row(s) have no transaction_hash; resolve pending operations before rolling back',
            (SELECT count(*) FROM "transaction" WHERE transaction_hash IS NULL);
    END IF;
END $$;

ALTER TABLE "transaction"
DROP CONSTRAINT IF EXISTS pk__transaction;

ALTER TABLE "transaction"
ALTER COLUMN transaction_hash SET NOT NULL;

ALTER TABLE "transaction"
ADD CONSTRAINT pk__transaction PRIMARY KEY (transaction_hash);

ALTER TABLE "transaction"
DROP COLUMN IF EXISTS operation_key;

-- NOT VALID + separate VALIDATE, to avoid the full-table scan/lock up front.
ALTER TABLE account
ADD CONSTRAINT fk__account__transaction FOREIGN KEY (pending_transaction_hash) REFERENCES "transaction" (transaction_hash) NOT VALID;

ALTER TABLE account
VALIDATE CONSTRAINT fk__account__transaction;
