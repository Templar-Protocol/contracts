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

-- One in-flight charge per account: the key and its inner-spend are set and
-- cleared together. Forbid a half-populated row, which would otherwise be picked
-- up by the broom and let settle() underbill via coalesce(pending_inner_spend, 0).
ALTER TABLE
    account
ADD
    CONSTRAINT chk__account__pending_charge_complete CHECK (
        (pending_operation_key IS NULL) = (pending_inner_spend IS NULL)
    );

-- Pre-gateway in-flight charges have no gateway operation, so the new settlement
-- path can't reconcile them. Rather than silently drop (and release) them at
-- cutover, refuse to migrate while any remain: drain them by letting the old
-- relayer settle in-flight work before upgrading. Historical non-pending rows
-- are audit data the gateway now retains, so dropping them is safe.
DO
$$
BEGIN
    IF to_regclass('"transaction"') IS NOT NULL
        AND EXISTS (
            SELECT 1 FROM "transaction" WHERE "status" = 'pending'::transaction_status
        ) THEN
        RAISE EXCEPTION
            'refusing cutover: % in-flight charge(s) remain in "transaction"; drain them before upgrading',
            (SELECT count(*) FROM "transaction" WHERE "status" = 'pending'::transaction_status);
    END IF;
END
$$
;

DROP TABLE IF EXISTS "transaction";

DROP TYPE IF EXISTS transaction_status;
