CREATE TABLE job_outbox (
    job_outbox_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    kind VARCHAR(64) NOT NULL,
    payload JSON NOT NULL,
    deduplication_key VARCHAR(191) CHARACTER SET ascii COLLATE ascii_bin UNIQUE,
    status VARCHAR(16) NOT NULL DEFAULT 'pending',
    attempts BIGINT NOT NULL DEFAULT 0,
    available_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    locked_until DATETIME(6) NULL,
    lock_token CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    relay_due_at DATETIME(6) NULL DEFAULT CURRENT_TIMESTAMP(6),
    relay_token CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    last_error VARCHAR(64) NULL,
    created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    CONSTRAINT job_outbox_status_valid CHECK (status IN ('pending', 'running', 'completed', 'failed')),
    CONSTRAINT job_outbox_attempts_valid CHECK (attempts >= 0),
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
    ),
    INDEX job_outbox_relay_idx (relay_due_at, job_outbox_id),
    INDEX job_outbox_retention_idx (status, updated_at, job_outbox_id)
) ENGINE = InnoDB DEFAULT CHARACTER SET utf8mb4 COLLATE utf8mb4_bin;
