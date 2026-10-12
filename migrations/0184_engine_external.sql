-- 0184_engine_external.sql
--
-- SP4d-4 Part C: an engine egret did not provision and never touches, held so
-- a profile can be bound to it and catalogs declared attached to it. Before
-- this, SP4d-3's live check had to INSERT such a row and its attachment by
-- hand. The CHECK makes the external shape unrepresentable any other way: an
-- external row carries an address and a declared Ranger plugin version and
-- NONE of the provisioning state (namespace, spec, secrets, certs) egret holds
-- for clusters it runs; a provisioned row carries none of the external fields.
-- Every external column is tested with an explicit IS NOT NULL: a CHECK passes
-- on NULL, so `external_port BETWEEN 1 AND 65535` alone admits a missing port.
-- An external engine is a Trino or a StarRocks: those are the two engines the
-- Ranger evaluator models, and the only ones a plugin can be installed on.
-- `status` is deliberately not widened: every reader shows `origin` instead.
-- `'declared'` attachments carry no client and no credential. That a declared
-- attachment belongs to an external engine is a cross-table fact a CHECK
-- cannot express; `egret_engine_mgmt::external` is its one writer and a test
-- asserts it across the whole table.
ALTER TABLE engine_mgmt.engines
    ADD COLUMN origin text NOT NULL DEFAULT 'provisioned',
    ADD COLUMN external_host text,
    ADD COLUMN external_port integer,
    ADD COLUMN ranger_plugin_version text,
    ADD CONSTRAINT engines_origin_check CHECK (origin IN ('provisioned', 'external')),
    ADD CONSTRAINT engines_external_shape CHECK (
        CASE origin
        WHEN 'external' THEN
            type IN ('trino', 'starrocks')
            AND external_host IS NOT NULL AND external_host <> ''
            AND external_port IS NOT NULL AND external_port BETWEEN 1 AND 65535
            AND ranger_plugin_version IS NOT NULL
            AND ranger_plugin_version ~ '^[0-9]+\.[0-9]+\.[0-9]+$'
            AND namespace = '' AND spec = '{}'::jsonb
            AND delegator_secret_id IS NULL AND tls_secret_id IS NULL
            AND coordinator_cert IS NULL AND root_secret_id IS NULL
            AND tls_cert_secret_ref IS NULL AND internal_secret_id IS NULL
            AND monitor_secret_id IS NULL
        ELSE
            external_host IS NULL AND external_port IS NULL
            AND ranger_plugin_version IS NULL
        END
    );

ALTER TABLE registry.engine_catalog_attachments
    DROP CONSTRAINT engine_catalog_attachments_apply_status_check,
    ADD CONSTRAINT engine_catalog_attachments_apply_status_check
        CHECK (apply_status IN ('applied', 'failed', 'pending', 'declared')),
    ADD CONSTRAINT attachments_declared_holds_nothing CHECK (
        apply_status <> 'declared'
        OR (catalog_client_id IS NULL AND credential_secret_id IS NULL
            AND NOT minted_client AND NOT is_primary)
    );
