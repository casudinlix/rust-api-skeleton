CREATE TABLE jobs (
    id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    kind VARCHAR(64) NOT NULL,
    payload JSON NOT NULL,
    deduplication_key VARCHAR(191) CHARACTER SET ascii COLLATE ascii_bin UNIQUE,
    status VARCHAR(16) NOT NULL DEFAULT 'pending',
    attempts BIGINT NOT NULL DEFAULT 0,
    available_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    locked_until DATETIME(6) NULL,
    lock_token CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    last_error TEXT NULL,
    created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    CONSTRAINT jobs_status_valid CHECK (status IN ('pending', 'running', 'completed', 'failed')),
    CONSTRAINT jobs_attempts_valid CHECK (attempts >= 0),
    CONSTRAINT jobs_running_has_lease CHECK (
        (status = 'running' AND locked_until IS NOT NULL AND lock_token IS NOT NULL)
        OR (status <> 'running' AND locked_until IS NULL AND lock_token IS NULL)
    ),
    INDEX jobs_ready_idx (status, available_at, created_at, id),
    INDEX jobs_expired_idx (status, locked_until),
    INDEX jobs_retention_idx (status, updated_at)
) ENGINE = InnoDB DEFAULT CHARACTER SET utf8mb4 COLLATE utf8mb4_bin;
