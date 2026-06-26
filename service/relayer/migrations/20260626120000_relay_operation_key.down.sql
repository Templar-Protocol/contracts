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

-- Rows without a transaction hash cannot exist under the old schema.
DELETE FROM "transaction"
WHERE transaction_hash IS NULL;

ALTER TABLE "transaction"
DROP CONSTRAINT IF EXISTS pk__transaction;

ALTER TABLE "transaction"
ALTER COLUMN transaction_hash SET NOT NULL;

ALTER TABLE "transaction"
ADD CONSTRAINT pk__transaction PRIMARY KEY (transaction_hash);

ALTER TABLE "transaction"
DROP COLUMN IF EXISTS operation_key;

ALTER TABLE account
ADD CONSTRAINT fk__account__transaction FOREIGN KEY (pending_transaction_hash) REFERENCES "transaction" (transaction_hash);
