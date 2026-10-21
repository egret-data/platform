-- KI-023: a draft edit must not change the URL an endpoint is served on.
--
-- Until now the serving router matched `dataapi.endpoints (method, path)` — the
-- DRAFT columns — so saving a new path on a live endpoint's draft moved its URL
-- at once: the old URL answered 404 and the new one served the live version's
-- old SQL, before anything was published. The URL is part of the published
-- contract (`EndpointSnapshot::method` / `path`, and `contract.rs` already
-- classifies a path change as breaking), so it must change only at publish.
--
-- `live_method` / `live_path` are the route the LIVE version answers on. Only
-- the two writers of `live_version_id` (publish and rollback) write them, from
-- the snapshot they make live. The serving router, the generated OpenAPI
-- document and Try it read them; the draft's `method` / `path` take effect at
-- the next publish.
--
-- A column pair on the endpoint row rather than a lookup through
-- `endpoint_versions.snapshot`, because the URL needs a UNIQUE constraint and a
-- constraint cannot reach through a join: with routing on the snapshot alone,
-- an endpoint whose draft moved away from /x and a second endpoint created and
-- published at /x would both be live at /x with nothing to break the tie. The
-- partial unique index below makes that a refused publish (409) instead.
ALTER TABLE dataapi.endpoints
  ADD COLUMN live_method text,
  ADD COLUMN live_path text,
  ADD CONSTRAINT dataapi_endpoints_live_route_pair
    CHECK ((live_method IS NULL) = (live_path IS NULL)),
  ADD CONSTRAINT dataapi_endpoints_live_method_check
    CHECK (live_method IS NULL OR live_method IN ('GET', 'POST'));

-- Backfill from the live snapshot, which is what the versions list already
-- calls live. One exception keeps the migration from failing on data the old
-- routing allowed: where another endpoint in the workspace already holds that
-- URL (its draft row sits there, or its own live snapshot claims it too), the
-- row keeps the URL it is served on today (`method` / `path`). Those values are
-- unique by `UNIQUE (workspace_id, method, path)`, and a row only takes its
-- snapshot URL when no other row holds it either way, so the index below is
-- satisfiable by construction.
WITH snap AS (
  SELECT e.id, e.workspace_id,
         COALESCE(v.snapshot->>'method', e.method) AS m,
         COALESCE(v.snapshot->>'path', e.path) AS p
    FROM dataapi.endpoints e
    JOIN dataapi.endpoint_versions v ON v.id = e.live_version_id
), decided AS (
  SELECT a.id, a.m, a.p,
         EXISTS (SELECT 1 FROM dataapi.endpoints o
                  WHERE o.workspace_id = a.workspace_id AND o.id <> a.id
                    AND o.method = a.m AND o.path = a.p)
      OR EXISTS (SELECT 1 FROM snap b
                  WHERE b.workspace_id = a.workspace_id AND b.id <> a.id
                    AND b.m = a.m AND b.p = a.p) AS contested
    FROM snap a
)
UPDATE dataapi.endpoints t
   SET live_method = CASE WHEN d.contested THEN t.method ELSE d.m END,
       live_path   = CASE WHEN d.contested THEN t.path   ELSE d.p END
  FROM decided d
 WHERE d.id = t.id;

-- Not filtered on `state` or `deleted_at`: a disabled or trashed endpoint keeps
-- its claim on the URL, exactly as its draft row keeps `UNIQUE (workspace_id,
-- method, path)`, so re-enabling or restoring it can never meet a stranger
-- serving its URL.
CREATE UNIQUE INDEX dataapi_endpoints_live_route_key
  ON dataapi.endpoints (workspace_id, live_method, live_path)
  WHERE live_path IS NOT NULL;
