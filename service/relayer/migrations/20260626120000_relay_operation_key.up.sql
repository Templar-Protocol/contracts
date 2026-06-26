-- up
-- Re-key the pending-transaction allowance lock off the relayer-generated
-- gateway idempotency key.
--
-- The gateway submits and signs transactions itself, surfacing an on-chain
-- transaction hash only AFTER submission -- too late to lock allowance before
-- sending. The idempotency key, by contrast, is generated up front, so it
-- becomes the transaction record's identity; the transaction hash is filled in
-- once the gateway returns it.

ALTER TABLE "transaction"
ADD COLUMN IF NOT EXISTS operation_key uuid;

-- Legacy rows predate idempotency keys; give them synthetic ones so the column
-- can become the primary key.
UPDATE "transaction"
SET operation_key = gen_random_uuid()
WHERE operation_key IS NULL;

ALTER TABLE "transaction"
ALTER COLUMN operation_key SET NOT NULL;

-- Re-point the account's pending marker at the operation key before relaxing
-- the transaction hash it currently references.
ALTER TABLE account
ADD COLUMN IF NOT EXISTS pending_operation_key uuid;

UPDATE account a
SET pending_operation_key = t.operation_key
FROM "transaction" t
WHERE a.pending_transaction_hash = t.transaction_hash;

ALTER TABLE account
DROP CONSTRAINT IF EXISTS fk__account__transaction,
DROP COLUMN IF EXISTS pending_transaction_hash;

-- Swap the transaction primary key to the operation key so the transaction hash
-- can be null until the gateway returns it.
ALTER TABLE "transaction"
DROP CONSTRAINT IF EXISTS pk__transaction;

ALTER TABLE "transaction"
ADD CONSTRAINT pk__transaction PRIMARY KEY (operation_key);

ALTER TABLE "transaction"
ALTER COLUMN transaction_hash DROP NOT NULL;

ALTER TABLE account
ADD CONSTRAINT fk__account__transaction FOREIGN KEY (pending_operation_key) REFERENCES "transaction" (operation_key);
