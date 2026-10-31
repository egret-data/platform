# Catalog guard: namespace create and listings, body deadline, storage overlap (P3)

Status: implemented, 2026-10-31. Base: `main` `99270626`. Branch `fix/catalog-guard-namespace-body-timeout`.
Rows: KI-312, KI-313, KI-316 (docs `platform/known-issues.md`). Precedents: the rename-authz spec
(`2026-10-27-catalog-rename-authorizes-source-design.md`: `GuardedBody`, buffer after authN, names compared under
lakekeeper's collation) and the vended-credentials spec (`2026-10-27-vended-credentials-follow-grants-design.md`,
§10 "Cross-warehouse register").

## Premise on `main`

| Row | Result | Evidence |
|---|---|---|
| KI-312 create | reproduces | `POST /{prefix}/namespaces` has no namespace in its path, so the guard treated it as a catalog-root write: any `write`/`manage` grant anywhere created any namespace (red test `creating_a_namespace_needs_write_on_its_parent`). |
| KI-312 list | reproduces | `GET /{prefix}/namespaces` is a catalog-root read (any grant); a table grant reads `GET …/namespaces/{ns}/tables` and `/views` unfiltered (red tests `listing_namespaces_shows_only_what_the_role_can_see`, `listing_tables_shows_only_the_granted_ones`). |
| KI-313 | reproduces | the mount's `to_bytes` on a guarded body has no deadline; lakekeeper, the view hold and the table hook read the other bodies with none either (red test `a_body_that_never_arrives_is_a_408`: no answer). |
| KI-316 | partly | lakekeeper parses the endpoint as a URL and strips its path, so `http://rustfs:9000` vs `http://rustfs:9000/`, host case and an explicit default port are already one store to it. What slips past: a different **region** string (compared although an endpoint is set), `http` vs `https` on one host and port, and two host names for one store. Red test `an_overlapping_warehouse_is_refused_however_its_store_is_written`. |

## Design and decisions

1. **Namespace create (KI-312).** `GuardedBody::NamespaceCreate` (`POST /{prefix}/namespaces`, 1 MiB cap, read after
   authN like a rename). The guard reads `namespace` (a non-empty list of non-empty strings, else 400) and requires
   what a table create in the **parent** requires: a `write`/`manage` grant covering the parent namespace (a
   `namespace` grant on exactly the parent, or `catalog`). A top-level namespace needs a catalog-level write grant.
   *Decision D1:* a namespace grant on the namespace itself does not create it, as a table grant does not create its
   table (rename-authz E5); the parent is what is written.
2. **Listings (KI-312).** The request is still allowed as before (any grant), and the mount hands the 2xx answer of
   `GET /{prefix}/namespaces`, `…/namespaces/{ns}/tables` and `…/views` to the guard (`CatalogGuard::filter_listing`),
   which cuts it. A catalog grant sees everything (answer passed unchanged). Otherwise:
   - a namespace is shown when a grant is **on, above or under** it: a `namespace` grant sees its namespace, its
     ancestors and its descendants; a `table` grant sees its table's namespace and its ancestors (not the namespaces
     beside its table) — `grant_match::grant_sees_namespace`;
   - a table or view is shown when a load of it would be allowed (`catalog_grant_covers_by`, read).
   Names compare under lakekeeper's collation (`CatalogNameEq`), as every other decision. Every top-level list of the
   same length as the names (`namespace-uuids`, `table-uuids`, `protection-status`) is cut in step.
   *Decision D2:* filtering a page can leave it short or empty while `next-page-token` still points on; Iceberg
   clients page until the token is null, so this is valid. An answer the guard cannot parse is a 502, never passed on.
3. **Body deadline (KI-313).** `BODY_DEADLINE` = 30 s from the request's arrival, a constant (the catalog has no
   config pattern for it). The mount wraps every request body that passes the guard; a read still waiting at the
   deadline fails, and the mount answers **408** in the Iceberg error model, whoever was reading (the guard's buffer,
   the view hold, the table hook or lakekeeper). *Decision D3:* a total deadline, not an idle one, so a client that
   trickles a byte at a time is cut too; 16 MiB (the largest body accepted) needs about 4 s at 32 Mbit/s.
   Behind the demo edge nginx buffers the body first (its own `client_body_timeout`), so the live check talks to egret
   directly.
4. **Storage overlap (KI-316).** `create_lakekeeper_warehouse` refuses (409) before asking lakekeeper when the new
   S3 storage overlaps any S3 warehouse lakekeeper holds (`lakekeeper.warehouse.storage_profile`, so the stored
   profile, not an edited storage connection): same bucket, same store, and key prefixes that contain one another
   (or either is the whole bucket). The store is `host:port/path` (`store_identity`): host lower-cased without a
   trailing dot, the scheme's default port, path without `/` at either end and without a trailing bucket name (as
   lakekeeper strips it). *Decision D4:* the scheme and the region are not part of the store. An endpoint that is not
   an `http(s)` URL with a host is compared as written (lower case), so a path is never read as a host (tested with
   `http://proxy/rustfs:9000`, `/rustfs:9000`, `rustfs:9000`). **Remaining limit:** two host names (or a host name and
   an IP) for one store cannot be told apart without DNS, which egret does not do; no endpoint (AWS by region) and an
   explicit AWS endpoint are also compared as different. The check and lakekeeper's own are not serialized: two
   concurrent creates can both pass.

## Live check (own demo stack, image from this branch, 2026-10-31)

Catalog clients minted through `/oauth/token`, requests to `/catalog/v1/{prefix}/…` on the demo-warehouse:

- KI-312: a client with only a table `write` grant got 403 creating `rogue` and `sales.sub`; a `namespace` `write`
  grant on `stage` got 403 for `rogue2` and `corp.x` and 200 for `stage.child`. The table-grant client listed
  namespaces as `[["sales"]]` and the tables of `sales` as `open` only (the engine client sees `secret` too); a
  `corp.fin` reader saw `[["corp"]]` at the top and `[["corp","fin"]]` under `corp`.
- KI-313: from inside the egret container to `127.0.0.1:8080` (no nginx in front), five requests declaring
  `Content-Length: 4096` and sending nothing (rename, multi-table commit, table commit, namespace create, table
  create) were each answered `408 Request Timeout` after 30 s.
- KI-316: a warehouse on a storage connection `http://RUSTFS:9000/`, region `eu-west-1`, same bucket, got 409 naming
  `demo-warehouse`; so did `https://rustfs:9000` with key prefix `sub`. One on `http://<rustfs container IP>:9000`
  was created (201): the host-alias limit above.
