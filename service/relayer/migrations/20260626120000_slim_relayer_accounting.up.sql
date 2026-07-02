-- up
-- Collapse the relayer's transaction bookkeeping into account allowance plus a
-- pending gateway-charge ledger.
--
-- The gateway now owns the full transaction lifecycle and records every
-- operation's status, on-chain hash, and gas cost. The relayer keeps only what
-- the gateway can't: each user's allowance and the per-operation reservations
-- that prevent concurrent gateway submissions from overspending it. Status /
-- gas / hash are read back from the gateway at settlement, so the separate
-- "transaction" table (and its status enum) are gone.
ALTER TABLE
    account DROP CONSTRAINT IF EXISTS fk__account__transaction;

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

CREATE TABLE IF NOT EXISTS pending_gateway_charge (
    operation_key uuid NOT NULL,
    account_id varchar(64) NOT NULL,
    gas_estimate numeric(39, 0) NOT NULL,
    inner_spend numeric(39, 0) NOT NULL,
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT pk__pending_gateway_charge PRIMARY KEY (operation_key),
    CONSTRAINT fk__pending_gateway_charge__account FOREIGN KEY (account_id) REFERENCES account (account_id),
    CONSTRAINT chk__pending_gateway_charge__gas_estimate_nonnegative CHECK (gas_estimate >= 0),
    CONSTRAINT chk__pending_gateway_charge__inner_spend_nonnegative CHECK (inner_spend >= 0)
);

CREATE INDEX IF NOT EXISTS idx__pending_gateway_charge__account_id ON pending_gateway_charge (account_id);

CREATE INDEX IF NOT EXISTS idx__pending_gateway_charge__updated_at ON pending_gateway_charge (updated_at);

CREATE
OR REPLACE TRIGGER updated_at_trigger BEFORE
UPDATE
    ON pending_gateway_charge FOR EACH ROW EXECUTE PROCEDURE updated_at();

ALTER TABLE
    account DROP CONSTRAINT IF EXISTS chk__account__pending_charge_complete;

ALTER TABLE
    account DROP COLUMN IF EXISTS pending_transaction_hash,
    DROP COLUMN IF EXISTS pending_operation_key,
    DROP COLUMN IF EXISTS pending_inner_spend;

DROP TABLE IF EXISTS "transaction";

DROP TYPE IF EXISTS transaction_status;
