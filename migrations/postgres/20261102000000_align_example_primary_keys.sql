-- Preserve existing entity UUIDs and the immutable earlier migration history.
ALTER TABLE users RENAME COLUMN id TO user_id;
ALTER INDEX users_created_at_id_index RENAME TO users_created_at_user_id_index;

ALTER TABLE jobs RENAME COLUMN id TO job_id;
ALTER TABLE jobs ADD COLUMN claimable_at TIMESTAMPTZ GENERATED ALWAYS AS (
    CASE
        WHEN status = 'pending' THEN available_at
        WHEN status = 'running' THEN locked_until
        ELSE NULL
    END
) STORED;

-- One ordered eligibility range replaces the pending/expired OR predicate.
DROP INDEX jobs_ready_idx;
DROP INDEX jobs_expired_idx;
CREATE INDEX jobs_claimable_idx ON jobs (claimable_at, created_at, job_id)
    WHERE claimable_at IS NOT NULL;

DROP INDEX jobs_retention_idx;
CREATE INDEX jobs_retention_idx ON jobs (status, updated_at, job_id);
