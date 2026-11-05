-- 0213_profile_mechanism.sql
--
-- Governance SP2b: how a profile's engines are asked. 'ranger' (every profile
-- today) or 'opa' (egret answers Trino's OPA plugin from the IR; spec
-- docs/en/specs/2026-11-04-gov-sp2-trino-opa.md). OPA exists for Trino only
-- (StarRocks #77386 is unmerged), so the type condition is in the schema, not
-- in a predicate a writer has to remember. Every row keeps 'ranger'.
ALTER TABLE policy.authz_services
    ADD COLUMN mechanism text NOT NULL DEFAULT 'ranger';

ALTER TABLE policy.authz_services
    ADD CONSTRAINT authz_services_mechanism_is_known
    CHECK (mechanism IN ('ranger', 'opa'));

ALTER TABLE policy.authz_services
    ADD CONSTRAINT authz_services_opa_is_trino_only
    CHECK (mechanism = 'ranger' OR type = 'trino');

-- What each provisioned engine was last applied with. A switch is not atomic
-- across engines (each coordinator restarts on its own apply), so egret keeps
-- answering OPA for an engine still running the OPA plugin after its profile
-- went back to Ranger, until that engine is re-applied. NULL: external, or not
-- applied since this migration (treated as 'ranger').
ALTER TABLE engine_mgmt.engines
    ADD COLUMN applied_mechanism text NULL
    CHECK (applied_mechanism IN ('ranger', 'opa'));
