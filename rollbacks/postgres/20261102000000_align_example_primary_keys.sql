DROP INDEX jobs_claimable_idx;
DROP INDEX jobs_retention_idx;
ALTER TABLE jobs DROP COLUMN claimable_at;
ALTER TABLE jobs RENAME COLUMN job_id TO id;
CREATE INDEX jobs_ready_idx ON jobs (status, available_at, created_at, id);
CREATE INDEX jobs_expired_idx ON jobs (status, locked_until);
CREATE INDEX jobs_retention_idx ON jobs (status, updated_at);

ALTER TABLE users RENAME COLUMN user_id TO id;
ALTER INDEX users_created_at_user_id_index RENAME TO users_created_at_id_index;
