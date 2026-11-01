CREATE TABLE jobs (
    id UUID PRIMARY KEY,
    kind VARCHAR(64) NOT NULL,
    payload JSONB NOT NULL,
    deduplication_key VARCHAR(191) UNIQUE,
    status VARCHAR(16) NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'running', 'completed', 'failed')),
    attempts BIGINT NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    available_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    locked_until TIMESTAMPTZ,
    lock_token UUID,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CONSTRAINT jobs_running_has_lease CHECK (
        (status = 'running' AND locked_until IS NOT NULL AND lock_token IS NOT NULL)
        OR (status <> 'running' AND locked_until IS NULL AND lock_token IS NULL)
    )
);

CREATE INDEX jobs_ready_idx ON jobs (status, available_at, created_at, id);
CREATE INDEX jobs_expired_idx ON jobs (status, locked_until);
CREATE INDEX jobs_retention_idx ON jobs (status, updated_at);
