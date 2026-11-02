ALTER TABLE jobs
    DROP INDEX jobs_claimable_idx,
    DROP INDEX jobs_retention_idx,
    DROP COLUMN claimable_at,
    RENAME COLUMN job_id TO id;

ALTER TABLE jobs
    ADD INDEX jobs_ready_idx (status, available_at, created_at, id),
    ADD INDEX jobs_expired_idx (status, locked_until),
    ADD INDEX jobs_retention_idx (status, updated_at);

ALTER TABLE users
    RENAME COLUMN user_id TO id,
    RENAME INDEX users_created_at_user_id_index TO users_created_at_id_index;
