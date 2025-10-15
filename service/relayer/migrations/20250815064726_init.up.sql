DO
$$
BEGIN
CREATE TYPE account_mark AS enum ('default', 'always_deny', 'always_approve');

EXCEPTION
WHEN duplicate_object THEN NULL;

END
$$
;

CREATE TABLE IF NOT EXISTS account (
    account_id varchar(64) NOT NULL,
    allowance numeric(39, 0) NOT NULL,
    allowance_locked numeric(39, 0) NOT NULL DEFAULT 0,
    pending_transaction_hash varchar(45) DEFAULT NULL,
    pending_transaction_issued_at timestamptz DEFAULT NULL,
    mark account_mark NOT NULL DEFAULT 'default',
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (account_id)
);

CREATE
OR REPLACE FUNCTION updated_at() RETURNS TRIGGER AS
$$
BEGIN
NEW.updated_at = NOW();

RETURN NEW;

END;

$$
language 'plpgsql';

CREATE
OR REPLACE TRIGGER updated_at_trigger BEFORE
UPDATE
    ON account FOR EACH ROW EXECUTE PROCEDURE updated_at();

CREATE TABLE IF NOT EXISTS call (
    id uuid NOT NULL DEFAULT gen_random_uuid(),
    account_id varchar(64) NOT NULL,
    transaction_hash varchar(45) UNIQUE NOT NULL,
    allowance_spent numeric(39, 0) NOT NULL,
    succeeded bool NOT NULL,
    created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (id),
    FOREIGN KEY (account_id) REFERENCES account (account_id)
);

CREATE INDEX IF NOT EXISTS idx__call__account_id ON call (account_id);
