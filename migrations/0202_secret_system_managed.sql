-- KI-207: secrets egret mints for itself are marked as such, so the Secrets
-- routes can refuse to rename, rotate or delete them and the pickers can leave
-- them out. From here on the code that mints one sets the flag
-- (`WorkspaceSecretStore::create_system`); it is never inferred from a name.
ALTER TABLE workspace_secrets
    ADD COLUMN system_managed boolean NOT NULL DEFAULT false;

-- Existing rows: every global secret an engine or a catalog attachment points
-- at as one egret minted (the internal-communication and monitor passwords,
-- the coordinator keystore, the Trino delegator and StarRocks root passwords,
-- and the credential of a Catalog Client egret minted). `tls_cert_secret_ref`
-- is a person's certificate and is not one of them.
UPDATE workspace_secrets ws
   SET system_managed = true
 WHERE ws.workspace_id IS NULL
   AND (
        EXISTS (SELECT 1 FROM engine_mgmt.engines e
                 WHERE ws.id IN (e.internal_secret_id, e.monitor_secret_id, e.tls_secret_id,
                                 e.delegator_secret_id, e.root_secret_id))
     OR EXISTS (SELECT 1 FROM registry.engine_catalog_attachments a
                 WHERE a.credential_secret_id = ws.id
                   AND a.minted_client
                   AND a.catalog_client_id IS NOT NULL)
     -- A minted secret no row points at any more (a previous generation that
     -- an apply has not swept yet) still carries the exact name and
     -- description its minting code gave it.
     OR (ws.name ~ '^engine-internal-[0-9a-f-]{36}-[0-9a-f]{32}$'
         AND ws.description = 'Trino internal-communication shared secret')
     OR (ws.name ~ '^engine-monitor-[0-9a-f-]{36}-[0-9a-f]{32}$'
         AND ws.description = 'egret-monitor password for the native Trino monitor')
     OR (ws.name ~ '^engine-tls-[0-9a-f-]{36}$'
         AND ws.description = 'jwt-mode coordinator keystore')
     OR (ws.name ~ '^catalog-client-[0-9a-f-]{36}$'
         AND ws.description = 'Catalog Client credential an engine presents to egret''s catalog')
     OR (ws.name ~ '^egret-delegator-[0-9a-f-]{36}$'
         AND ws.description = 'impersonate-mode Trino delegator password')
     OR (ws.name ~ '^egret-root-[0-9a-f-]{36}$'
         AND ws.description = 'StarRocks FE root password')
   );
