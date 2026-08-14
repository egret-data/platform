-- Data API P2: service accounts and their OAuth2 clients.
--
-- A service account is a REAL row in `users` with provider = 'service'. That is
-- deliberate: `access_grants`, Cedar, `ensure_username` (Ranger/Trino
-- delegation) and audit all key off a user id, so reusing `users` means a
-- machine principal needs no parallel identity plumbing anywhere. The price is
-- that human-facing user LISTS must exclude it — see `UserRepo::list`.
ALTER TABLE users DROP CONSTRAINT users_provider_check;
ALTER TABLE users ADD CONSTRAINT users_provider_check
  CHECK (provider IN ('local','oidc','service'));

-- A service account must never be able to log in interactively. The login
-- handler already requires password_hash IS NOT NULL, but that is one code path
-- away from being wrong forever. Enforce it where it cannot be forgotten.
ALTER TABLE users ADD CONSTRAINT users_service_has_no_password
  CHECK (provider <> 'service' OR password_hash IS NULL);

CREATE SCHEMA IF NOT EXISTS dataapi;

-- The workspace binding lives HERE, not on `users`: `users` is platform-level
-- and every other consumer of it would have to learn to ignore a workspace
-- column that means nothing for humans.
CREATE TABLE dataapi.service_accounts (
  user_id      uuid PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  name         text NOT NULL,
  description  text NOT NULL DEFAULT '',
  created_by   uuid REFERENCES users(id) ON DELETE SET NULL,
  created_at   timestamptz NOT NULL DEFAULT now(),
  UNIQUE (workspace_id, name)
);
CREATE INDEX service_accounts_ws_idx ON dataapi.service_accounts (workspace_id);

-- OAuth2 client_credentials clients, deliberately shaped like
-- `catalog.catalog_clients` (0053) — same idea, same machine-IdP, second
-- audience. N clients per service account, so rotation is mint-new-then-revoke-old.
CREATE TABLE dataapi.clients (
  id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  service_account_id uuid NOT NULL REFERENCES dataapi.service_accounts(user_id) ON DELETE CASCADE,
  name               text NOT NULL,
  client_id          text NOT NULL UNIQUE,
  -- Argon2, matching CatalogStore::verify_client. Affordable because it is
  -- checked once per TOKEN, not once per API request.
  client_secret_hash text NOT NULL,
  expires_at         timestamptz,
  revoked_at         timestamptz,
  last_used_at       timestamptz,
  created_by         uuid REFERENCES users(id) ON DELETE SET NULL,
  created_at         timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX dataapi_clients_sa_idx ON dataapi.clients (service_account_id);
-- No index on `client_id`: the UNIQUE constraint above already creates one, and
-- that is the index `ClientStore::verify`'s lookup uses.
