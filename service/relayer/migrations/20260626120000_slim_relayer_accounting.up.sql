-- up
-- Collapse the relayer's transaction bookkeeping into the account row.
--
-- The gateway now owns the full transaction lifecycle and records every
-- operation's status, on-chain hash, and gas cost. The relayer keeps only what
-- the gateway can't: each user's allowance, plus -- while a charge is in flight
-- -- the gateway idempotency key it is waiting on and the deposit to bill if
-- that operation succeeds. Status / gas / hash are read back from the gateway at
-- settlement, so the separate "transaction" table (and its status enum) are
-- gone. There is only ever one in-flight charge per account, so the pending
-- marker and its inner-spend live inline on `account`.
ALTER TABLE
    account DROP CONSTRAINT IF EXISTS fk__account__transaction;

ALTER TABLE
    account DROP COLUMN IF EXISTS pending_transaction_hash,
ADD
    COLUMN IF NOT EXISTS pending_operation_key uuid,
ADD
    COLUMN IF NOT EXISTS pending_inner_spend numeric(39, 0);

-- In-flight pre-gateway charges can't be reconciled against the gateway (they
-- have no gateway operation), and historical rows are audit data the gateway now
-- retains. Drop the table -- and the in-flight locks the dropped column held --
-- wholesale; this is a one-time cutover release of any charge in flight at
-- upgrade.
DROP TABLE IF EXISTS "transaction";

DROP TYPE IF EXISTS transaction_status;
