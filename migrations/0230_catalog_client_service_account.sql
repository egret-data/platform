-- SA2: a catalog client becomes a credential of a service account, with the
-- same revoke/expire lifecycle as dataapi.clients. service_account_id is
-- nullable: existing rows are backfilled by the `egret backfill
-- catalog-client-sa` CLI, and the verifier is fail-closed on NULL so an
-- un-backfilled client cannot mint in the meantime.
ALTER TABLE catalog.catalog_clients
  ADD COLUMN service_account_id uuid REFERENCES service_accounts(user_id) ON DELETE CASCADE,
  ADD COLUMN revoked_at  timestamptz,
  ADD COLUMN expires_at  timestamptz;
CREATE INDEX catalog_clients_service_account_idx
  ON catalog.catalog_clients (service_account_id);
