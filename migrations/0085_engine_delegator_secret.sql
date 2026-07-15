-- 0085_engine_delegator_secret.sql
-- Per-engine secret holding the generated `egret_delegator` service-user password
-- (impersonate-mode delegation). NULL until the engine is (re)provisioned in
-- impersonate mode. The password is stored in `secrets`; this column references it.
ALTER TABLE engine_mgmt.engines
    ADD COLUMN delegator_secret_id uuid;
