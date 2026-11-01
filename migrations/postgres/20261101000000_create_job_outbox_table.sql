CREATE TABLE job_outbox (
    job_outbox_id UUID PRIMARY KEY,
    kind VARCHAR(64) NOT NULL,
    payload JSONB NOT NULL,
    deduplication_key VARCHAR(191) UNIQUE,
    status VARCHAR(16) NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'running', 'completed', 'failed')),
    attempts BIGINT NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    available_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    locked_until TIMESTAMPTZ,
    lock_token UUID,
    relay_due_at TIMESTAMPTZ DEFAULT clock_timestamp(),
    relay_token UUID,
    last_error VARCHAR(64),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CONSTRAINT job_outbox_running_has_lease CHECK (
        (status = 'running' AND locked_until IS NOT NULL AND lock_token IS NOT NULL)
        OR (status <> 'running' AND locked_until IS NULL AND lock_token IS NULL)
    ),
    CONSTRAINT job_outbox_delivery_state CHECK (
        (status IN ('pending', 'running') AND relay_due_at IS NOT NULL)
        OR (status IN ('completed', 'failed') AND relay_due_at IS NULL AND relay_token IS NULL)
    ),
    CONSTRAINT job_outbox_execution_before_relay CHECK (
        status <> 'running' OR relay_due_at >= locked_until
    )
);

CREATE INDEX job_outbox_relay_idx ON job_outbox (relay_due_at, job_outbox_id)
    WHERE relay_due_at IS NOT NULL;
CREATE INDEX job_outbox_retention_idx ON job_outbox (status, updated_at, job_outbox_id);
