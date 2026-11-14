-- 0229_service_account_principals.sql
-- SA1: the service account becomes a first-class principal of record. The
-- platform-level identity (scope + name) moves here from dataapi.service_accounts,
-- which becomes a backward-compatible view so existing Data-API code is unchanged.
CREATE TABLE service_accounts (
  user_id      uuid PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  scope        text NOT NULL CHECK (scope IN ('platform','workspace')),
  workspace_id uuid REFERENCES workspaces(id) ON DELETE CASCADE,
  name         text NOT NULL,
  description  text NOT NULL DEFAULT '',
  created_by   uuid REFERENCES users(id) ON DELETE SET NULL,
  created_at   timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT service_account_scope_ws CHECK (
    (scope = 'platform'  AND workspace_id IS NULL) OR
    (scope = 'workspace' AND workspace_id IS NOT NULL)
  )
);
CREATE UNIQUE INDEX service_accounts_ws_name
  ON service_accounts (workspace_id, name) WHERE scope = 'workspace';
CREATE UNIQUE INDEX service_accounts_platform_name
  ON service_accounts (name) WHERE scope = 'platform';
CREATE INDEX service_accounts_ws_idx ON service_accounts (workspace_id);

-- Backfill: every existing dataapi.service_accounts row is workspace-scoped.
INSERT INTO service_accounts (user_id, scope, workspace_id, name, description, created_by, created_at)
  SELECT user_id, 'workspace', workspace_id, name, description, created_by, created_at
    FROM dataapi.service_accounts;

-- Repoint dataapi.clients' FK from the old table to the new base table.
-- Name is Postgres's auto-name for the inline FK in 0119.
ALTER TABLE dataapi.clients DROP CONSTRAINT clients_service_account_id_fkey;
ALTER TABLE dataapi.clients
  ADD CONSTRAINT clients_service_account_id_fkey
  FOREIGN KEY (service_account_id) REFERENCES service_accounts(user_id) ON DELETE CASCADE;

-- Replace the old table with a view over the new base (workspace scope only),
-- exposing exactly the columns Data-API reads/writes.
DROP TABLE dataapi.service_accounts;
CREATE VIEW dataapi.service_accounts AS
  SELECT user_id, workspace_id, name, description, created_by, created_at
    FROM service_accounts WHERE scope = 'workspace';

-- The Data-API create path does `INSERT INTO dataapi.service_accounts (...)`;
-- route it into the base table as a workspace SA, filling created_at for RETURNING.
CREATE FUNCTION dataapi.service_accounts_insert() RETURNS trigger AS $$
BEGIN
  INSERT INTO service_accounts (user_id, scope, workspace_id, name, description, created_by)
    VALUES (NEW.user_id, 'workspace', NEW.workspace_id, NEW.name,
            COALESCE(NEW.description, ''), NEW.created_by)
    RETURNING created_at INTO NEW.created_at;
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;
CREATE TRIGGER service_accounts_insert_trg
  INSTEAD OF INSERT ON dataapi.service_accounts
  FOR EACH ROW EXECUTE FUNCTION dataapi.service_accounts_insert();
