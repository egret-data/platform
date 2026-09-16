-- 0146_engine_internal_secrets.sql
-- Random per-engine secrets that replace values derived from the engine UUID
-- (which workspace members can read): Trino's internal-communication shared
-- secret and the `egret-monitor` password. Each column references the GLOBAL
-- named secret (workspace_secrets.id) that the LAST SUCCESSFUL apply rendered;
-- NULL means the running pods still carry the legacy derived values. No FK,
-- matching delegator_secret_id / tls_secret_id / root_secret_id.
ALTER TABLE engine_mgmt.engines
    ADD COLUMN internal_secret_id uuid,
    ADD COLUMN monitor_secret_id uuid;
