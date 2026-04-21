DO $$
BEGIN
    CREATE TYPE gateway_operation_status AS ENUM (
        'pending',
        'in_progress',
        'succeeded',
        'failed'
    );
EXCEPTION
    WHEN duplicate_object THEN NULL;
END $$;

DO $$
BEGIN
    CREATE TYPE gateway_operation_step_state AS ENUM (
        'not_started',
        'prepared',
        'submitted',
        'succeeded',
        'failed'
    );
EXCEPTION
    WHEN duplicate_object THEN NULL;
END $$;

CREATE TABLE IF NOT EXISTS gateway_operations (
    id uuid PRIMARY KEY,
    rpc_method text NOT NULL,
    signer_account_id text NOT NULL,
    idempotency_key text,
    request_fingerprint_hash bytea NOT NULL,
    request_payload jsonb NOT NULL,
    status gateway_operation_status NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS gateway_operations_idempotency_key_unique
    ON gateway_operations (idempotency_key)
    WHERE idempotency_key IS NOT NULL;

CREATE INDEX IF NOT EXISTS gateway_operations_status_updated_at_idx
    ON gateway_operations (status, updated_at);

CREATE INDEX IF NOT EXISTS gateway_operations_signer_account_id_created_at_idx
    ON gateway_operations (signer_account_id, created_at DESC);

ALTER TABLE gateway_operations
    DROP CONSTRAINT IF EXISTS gateway_operations_request_fingerprint_hash_length_check;

ALTER TABLE gateway_operations
    ADD CONSTRAINT gateway_operations_request_fingerprint_hash_length_check
    CHECK (octet_length(request_fingerprint_hash) = 32);

CREATE TABLE IF NOT EXISTS gateway_operation_steps (
    operation_id uuid NOT NULL REFERENCES gateway_operations(id) ON DELETE CASCADE,
    step_index integer NOT NULL,
    signer_account_id text NOT NULL,
    receiver_id text NOT NULL,
    wait_until text NOT NULL,
    actions jsonb NOT NULL,
    state gateway_operation_step_state NOT NULL,
    tx_hash text,
    signed_transaction bytea,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL,
    PRIMARY KEY (operation_id, step_index)
);

ALTER TABLE gateway_operation_steps
    DROP CONSTRAINT IF EXISTS gateway_operation_steps_state_payload_check;

ALTER TABLE gateway_operation_steps
    ADD CONSTRAINT gateway_operation_steps_state_payload_check
    CHECK (
        (state = 'not_started' AND tx_hash IS NULL AND signed_transaction IS NULL)
        OR (state = 'prepared' AND tx_hash IS NOT NULL AND signed_transaction IS NOT NULL)
        OR (state = 'submitted' AND tx_hash IS NOT NULL AND signed_transaction IS NULL)
        OR (state = 'succeeded' AND tx_hash IS NOT NULL AND signed_transaction IS NULL)
        OR (state = 'failed' AND signed_transaction IS NULL)
    );

CREATE INDEX IF NOT EXISTS gateway_operation_steps_operation_id_step_index_idx
    ON gateway_operation_steps (operation_id, step_index);
