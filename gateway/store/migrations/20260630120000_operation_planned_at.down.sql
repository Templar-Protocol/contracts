-- down
ALTER TABLE
    gateway_operations DROP COLUMN IF EXISTS planned_at;
