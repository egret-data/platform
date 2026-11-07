-- Port-8080 hardening (spec 2026-11-07 A1): the Trino security render an
-- engine was last applied with. 0 = before the hardening; egret writes
-- TRINO_SECURITY_REV on every successful Trino apply.
ALTER TABLE engine_mgmt.engines
    ADD COLUMN IF NOT EXISTS applied_security_rev smallint NOT NULL DEFAULT 0;
