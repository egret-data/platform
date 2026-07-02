-- Control-plane Polaris: the single registered connection + workspace↔catalog assignments.

-- Single registered Polaris connection + admin-token (Keycloak) config.
CREATE TABLE polaris_connection (
    id            boolean PRIMARY KEY DEFAULT true CHECK (id),   -- singleton row
    base_url      text NOT NULL,
    kc_token_url  text NOT NULL,
    kc_client_id  text NOT NULL,
    kc_secret_ref uuid NOT NULL REFERENCES workspace_secrets(id),  -- admin client_secret (global)
    kc_scope      text,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now()
);

-- Which Polaris catalogs a workspace may use + its provisioned service credential.
CREATE TABLE workspace_catalog_assignments (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id   uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    catalog_name   text NOT NULL,
    principal_name text NOT NULL,
    credential_ref uuid NOT NULL REFERENCES workspace_secrets(id),  -- global secret {clientId,clientSecret}
    created_at     timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, catalog_name)
);
CREATE INDEX wca_workspace_id_idx ON workspace_catalog_assignments(workspace_id);
