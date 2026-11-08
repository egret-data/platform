-- Port-8080 hardening (spec 2026-11-07 B5): the OPA base URL egret rendered
-- into this engine's plugin files at its last successful apply; NULL when the
-- engine was not applied on OPA. A start-up pass re-applies engines on an OPA
-- profile whose value differs from the current URL.
ALTER TABLE engine_mgmt.engines
    ADD COLUMN IF NOT EXISTS applied_opa_url text;
