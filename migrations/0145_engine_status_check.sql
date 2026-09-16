-- 0145_engine_status_check.sql
-- engines.status is the recorded lifecycle phase. Nothing but the column
-- default ever wrote it, so map any stray value to 'provisioning' first, then
-- pin the set: provisioning (created, pods not all ready yet), running (all
-- pods reported ready), failed (a create that could not be rolled back),
-- deleting (delete in progress; retryable).
UPDATE engine_mgmt.engines
   SET status = 'provisioning'
 WHERE status NOT IN ('provisioning', 'running', 'failed', 'deleting');

ALTER TABLE engine_mgmt.engines
    ADD CONSTRAINT engines_status_check
    CHECK (status IN ('provisioning', 'running', 'failed', 'deleting'));
