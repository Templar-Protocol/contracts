CREATE TABLE IF NOT EXISTS gateway_operations (
    id uuid PRIMARY KEY,
    rpc_method text NOT NULL,
    signer_account_id text NOT NULL,
    idempotency_key text,
    request_fingerprint_hash bytea NOT NULL,
    request_payload jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT NOW(),
    updated_at timestamptz NOT NULL DEFAULT NOW(),
    completed_at timestamptz,
    CONSTRAINT gateway_operations_request_fingerprint_hash_length_check
        CHECK (octet_length(request_fingerprint_hash) = 32),
    CONSTRAINT gateway_operations_request_payload_object_check
        CHECK (jsonb_typeof(request_payload) = 'object')
);

CREATE UNIQUE INDEX IF NOT EXISTS gateway_operations_idempotency_key_unique ON gateway_operations (idempotency_key)
WHERE
    idempotency_key IS NOT NULL;

CREATE INDEX IF NOT EXISTS gateway_operations_signer_account_id_created_at_idx ON gateway_operations (signer_account_id, created_at DESC);

CREATE INDEX IF NOT EXISTS gateway_operations_incomplete_created_at_idx ON gateway_operations (created_at ASC)
WHERE
    completed_at IS NULL;
