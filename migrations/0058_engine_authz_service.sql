-- Link a provisioned engine to its Ranger policy profile (authz_services row).
-- Many engines MAY share one profile (no UNIQUE); deleting the service detaches
-- the engine (SET NULL) rather than deleting it. Nullable: pre-existing engines
-- and the standalone seeded `trino` service stay valid.
ALTER TABLE engines
    ADD COLUMN authz_service_id BIGINT NULL
        REFERENCES authz_services(id) ON DELETE SET NULL;
