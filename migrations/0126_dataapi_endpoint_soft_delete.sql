ALTER TABLE dataapi.endpoints ADD COLUMN deleted_at timestamptz NULL;
CREATE INDEX dataapi_endpoints_deleted_idx ON dataapi.endpoints (workspace_id, deleted_at);

-- The serving path resolves an endpoint through exactly one query
-- (`serving/router.rs::live_snapshot`), which filters on `state = 'live'`. This
-- constraint is what makes "trashed but still serving" unrepresentable rather
-- than merely unlikely: a predicate can be forgotten by whoever writes the next
-- query, a CHECK cannot. The Alerts programme shipped the forgotten-predicate
-- version of this bug — a soft-deleted schedule kept firing.
ALTER TABLE dataapi.endpoints
  ADD CONSTRAINT dataapi_endpoints_trashed_not_live
  CHECK (deleted_at IS NULL OR state <> 'live');
