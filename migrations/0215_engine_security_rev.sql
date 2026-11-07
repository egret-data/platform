-- Port-8080 hardening (spec 2026-11-07 A1): the Trino security render an
-- engine was last applied with. 0 = before the hardening; egret writes
-- TRINO_SECURITY_REV on every successful Trino apply.
ALTER TABLE engine_mgmt.engines
    ADD COLUMN IF NOT EXISTS applied_security_rev smallint NOT NULL DEFAULT 0;

-- A1, "the stored spec is normalised the same way": a provisioned Trino row
-- stored before the hardening may lack `trinoConfig.auth`. Every apply already
-- renders it as PASSWORD auth (`engines::normalise_trino_auth`); write the same
-- block into the stored spec so every reader of the row (the native monitor's
-- credential choice, the UI's config editor) agrees with what was rendered.
-- Never adds a `jwt` block (that would make the engine delegation-capable).
-- Idempotent: a row that already has an auth object is not matched.
UPDATE engine_mgmt.engines
   SET spec = jsonb_set(spec, '{trinoConfig,auth}', '{"password":{}}'::jsonb)
 WHERE type = 'trino'
   AND origin = 'provisioned'
   AND jsonb_typeof(spec->'trinoConfig') = 'object'
   AND jsonb_typeof(spec #> '{trinoConfig,auth}') IS DISTINCT FROM 'object';
