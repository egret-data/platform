-- Per-engine Ranger plugin credential (spec 2026-10-20-ranger-surface-auth, §4.3, A10).
-- `ranger_secret_id` is the password the engine's plugin presents now;
-- `ranger_secret_prev_id` is the one it presented before the last rotation, kept
-- valid until the rollout is observed complete (never delete a secret the
-- cluster may still hold). Both name rows in the secret store
-- (`workspace_secrets`, global scope, named `engine-ranger-{engine_id}-{generation}`);
-- neither is a foreign key because the secret store is a separate seam.
--
-- `engines_external_shape` (0184) does not name these columns, so an external
-- engine may carry a credential too.
--
-- The CHECK keeps the window well-formed: a previous credential only exists
-- next to a current one, and is never the same row as it (retiring the
-- previous one deletes that row by id, which must never be the current one).
ALTER TABLE engine_mgmt.engines
    ADD COLUMN ranger_secret_id      uuid,
    ADD COLUMN ranger_secret_prev_id uuid,
    ADD CONSTRAINT engines_ranger_secret_window CHECK (
        ranger_secret_prev_id IS NULL
        OR (ranger_secret_id IS NOT NULL AND ranger_secret_prev_id <> ranger_secret_id)
    );
