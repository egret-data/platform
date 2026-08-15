-- Data API P4: the serving surface's access log (spec §8).
--
-- One row per call that got far enough to name a workspace. This is the table
-- the management UI's per-endpoint statistics are computed from — call count,
-- error rate, p50/p95, cache hit rate — and the table support reads when a
-- caller quotes a `request_id`.
--
-- WHY endpoint_id AND service_account_id CARRY NO FOREIGN KEY
--
-- Deliberate, and deliberately the OPPOSITE of the choice 0121 made for
-- `endpoints.database_id`. There the FK is `ON DELETE RESTRICT` because a
-- published endpoint whose database vanished is a broken endpoint, and the
-- constraint exists to force the deletion through the endpoint's own lifecycle.
--
-- An access log is not a live object; it is a record of what already happened.
-- The question it answers most often is "what was this endpoint doing before
-- someone deleted it?", so a CASCADE would erase precisely the history an
-- incident needs and a RESTRICT would make an endpoint undeletable because it
-- had once been called. Both are wrong for the same reason: the log must
-- survive the deletion of what it describes. The columns are therefore plain
-- uuids — a dangling one is not corruption here, it is the normal end state of
-- a deleted endpoint, and readers must treat them as historical labels rather
-- than as joins that are guaranteed to resolve.
--
-- `workspace_id` is the one exception and DOES cascade: it is the tenancy
-- boundary, not a label. A deleted workspace's rows must not outlive it, and
-- every query into this table is scoped by it — which is also why it leads the
-- index.
CREATE TABLE dataapi.access_log (
  id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  ts                 timestamptz NOT NULL DEFAULT now(),
  workspace_id       uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  -- No FK: see above. Null until routing resolves an endpoint, which is why a
  -- 404 still produces a row.
  endpoint_id        uuid,
  version            integer,
  -- No FK: see above. The `users` row backing a service account, i.e. the
  -- identity the engine ran the query as.
  service_account_id uuid,
  -- The OAuth2 client id from the token's `sub`. Text, because a revoked
  -- client's id is still the right label for the calls it made.
  client_id          text,
  status_code        integer NOT NULL,
  row_count          bigint,
  duration_ms        integer NOT NULL,
  cache_hit          boolean NOT NULL DEFAULT false,
  -- Spec §8 wants the engine's own query id here so a slow API call traces back
  -- to the exact query on Trino/StarRocks. `QueryExecPort` does not return one
  -- today (`TabularResult` has no such field), so P4 always writes NULL. The
  -- column exists now rather than in a later migration because adding it later
  -- would leave every historical row unable to carry it either way.
  engine_query_id    text,
  -- Best-effort, from the first hop of `X-Forwarded-For`. NULL when the header
  -- is absent or unparseable — see `access_log.rs` for why it is not trusted
  -- beyond "what the proxy in front of us said".
  client_ip          inet,
  -- NULL on success; otherwise spec §7's fixed code, the SAME string the caller
  -- was given. That is the point: a caller quoting `request_id` and `code` can
  -- be matched to this row without asking them to re-run anything.
  error_code         text,
  request_id         text NOT NULL
);

-- The workspace-scoped reads: the statistics panels and any "what did this
-- workspace do recently" lookup. Leading on `workspace_id` because those queries
-- all name one.
CREATE INDEX access_log_ws_ts_idx ON dataapi.access_log (workspace_id, ts DESC);

-- The retention prune, which is the one read that does NOT narrow by workspace.
--
-- An earlier version of the comment above claimed the prune "narrows the same
-- way" and gave it no index of its own. **It does not narrow the same way and it
-- never could**: `AccessLogStore::prune` is deliberately global — retention is a
-- system-wide policy, not a per-workspace one, exactly as `AuditStore::prune`
-- is — so its `WHERE ts < now() - $1` cannot use an index that leads on
-- `workspace_id`. Left as it was, the hourly job sequentially scanned the
-- fastest-growing table in the schema, forever, and got slower every hour by
-- construction.
--
-- The sibling this table was modelled on has exactly this index
-- (`0076_audit_log.sql:19`, `audit_log_ts_idx`); it was the one line of the
-- pattern that did not get copied.
--
-- `ts` ascending rather than `DESC`: the prune scans the OLD end. Postgres can
-- read either direction, so this is about which end the planner starts from, and
-- a btree on `ts` also serves the `ts DESC` reads.
CREATE INDEX access_log_ts_idx ON dataapi.access_log (ts);

-- Support's entry point is a `request_id` a caller quotes out of an error body,
-- and they do not know which workspace it belonged to. Without this the lookup
-- is a full scan of the fastest-growing table in the system.
CREATE INDEX access_log_request_id_idx ON dataapi.access_log (request_id);
