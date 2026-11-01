-- Preserve existing entity UUIDs and the immutable earlier migration history.
-- MySQL DDL implicitly commits; deploy in a coordinated maintenance window.
ALTER TABLE users
    RENAME COLUMN id TO user_id,
    RENAME INDEX users_created_at_id_index TO users_created_at_user_id_index;

ALTER TABLE jobs
    RENAME COLUMN id TO job_id,
    ADD COLUMN claimable_at DATETIME(6) GENERATED ALWAYS AS (
        -- MySQL can evaluate a generated column before CURRENT_TIMESTAMP
        -- defaults are initialized (verified MySQL bug #116338). Ignore that
        -- temporary zero-year value; MySQL recomputes after applying defaults.
        -- Strict SQL mode still rejects an explicitly supplied zero date.
        CASE
            WHEN status = 'pending' AND YEAR(available_at) > 0 THEN available_at
            WHEN status = 'running' AND YEAR(locked_until) > 0 THEN locked_until
            ELSE NULL
        END
    ) STORED,
    DROP INDEX jobs_ready_idx,
    DROP INDEX jobs_expired_idx,
    DROP INDEX jobs_retention_idx,
    ADD INDEX jobs_claimable_idx (claimable_at, created_at, job_id),
    ADD INDEX jobs_retention_idx (status, updated_at, job_id);
