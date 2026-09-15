# SP1 — Engine lifecycle hardening

**Status:** design, awaiting review
**Programme:** `docs/{en,vi}/specs/2026-09-15-control-plane-dg-review-programme.md`
**Delivery:** two PRs — Part A (delete/unassign, the user's bug) then Part B (provisioning hardening)

## 1. Problem

Deleting an engine returns "internal error" with nothing in the logs, and by then the engine's
Kubernetes objects are already gone while its row survives. A review of the delete path and the
rest of provisioning found the cause plus a set of ordering, cleanup and secret defects.

## 2. Findings

Handlers are in `crates/egret-engine-mgmt/src/engines.rs` unless noted. **V** = verified in source
while writing this spec; the rest come from the review and must be re-verified when planning.

### 2.1 Delete path

Current order (`engines.rs:3884-3894`): Kubernetes delete → `DELETE FROM engine_mgmt.engines` →
revoke catalog clients (errors ignored). The row delete cascades to `engine_catalogs`,
`engine_workspaces` and the engine-materialized `query.databases` rows, and from those onward.

| # | Defect | |
|---|---|---|
| D1 | `dataapi.endpoints.database_id … ON DELETE RESTRICT` (`migrations/0121_dataapi_endpoints.sql:30`). Any endpoint on a materialized database — **including trashed ones** (0126 is a soft delete) — makes the DELETE fail. On the kind cluster `demo-trino` has 26. | V |
| D2 | `automation.schedules.database_id … ON DELETE SET NULL` (`0110:24`) collides with `CHECK schedule_alert_needs_query` (`0110:71`, alert requires `database_id`). Any SQL alert blocks the delete. The comment in `0066_query_schema.sql` claiming the delete cannot be blocked is stale. | V |
| D3 | `EngineMgmtError::Db(_) => EngineHttpError::Internal` (`http_error.rs:61,72`) discards the `sqlx::Error` without logging. | V |
| D4 | Kubernetes is deleted before the DB delete is known to succeed → cluster gone, row and dependents left. | V |
| D5 | StarRocks delete removes the CR with `?`; `KubeClient::delete` treats 404 as an error → every retry returns 400 once the CR is gone. | |
| D6 | Trino delete wraps every call in `let _ =` → 403/timeout/unreachable API server are ignored, rows deleted, cluster orphaned. | |
| D7 | No Kubernetes configured → delete skips the cluster and returns 204 (create returns 400). | V |
| D8 | When the delete does succeed it silently cascades datasets → charts → dashboard cards → schedules (and runs). The confirm dialog mentions only Kubernetes resources. | |
| D9 | Nothing cleans up: the auto-created Ranger profile and `<name>_tag` service, secrets (`delegator_secret_id`, `tls_secret_id`, `tls_cert_secret_ref`, `root_secret_id`), and revocation ignores `is_minted_client`. Re-creating an engine with the same name hits `AlreadyExists` in `create_service`, logs a warning and continues **unbound → Trino allow-all while Ranger is enabled**. | |
| D10 | Unassigning an engine from a workspace (`DELETE …/workspaces/{ws}`, `PUT /engines/{id}/workspaces`) deletes the materialized database → same D1/D2 500. `PUT` is non-atomic (can apply assigns, then fail on an unassign). Assigning to an unknown workspace is a 500, not 404. | |

**No purge exists for trashed Data API endpoints** (no production `DELETE FROM dataapi.endpoints`;
only tests). Blocking on trashed endpoints therefore needs a purge action, or an engine with any
past endpoint could never be deleted. See §3.3. **V**

### 2.2 Provisioning

| # | Defect | |
|---|---|---|
| P1 | **Security.** `TRINO_INTERNAL_SECRET = "egret-internal-{engine.id}"` (`engines.rs:659`) and the monitor password `egret-monitor-{id}` (`:1457`) are derivable from the engine UUID, which workspace members can read (`Database.engine_id`, `/api/ws/{ws}/engines/{id}`). Anyone reaching the coordinator can forge internal-communication tokens or log in as `egret-monitor`. | V |
| P2 | A failed create deletes the row and revokes the client, but leaves applied Kubernetes objects, TLS/delegator secret rows and seeded Ranger policies. | |
| P3 | Trino update persists the spec, then returns 400 if Kubernetes fails — DB ahead of cluster, no rollback. A body omitting `delegationMode` silently reads back as `impersonate`. | |
| P4 | `rematerialize_for_engine` runs only for StarRocks; a Trino delegation-mode or TLS change leaves workspace connections stale. | |
| P5 | `remove_catalog` deletes the row and revokes the credential before updating Kubernetes; on failure the cluster keeps a catalog with a revoked credential and a retry has nothing to act on. | |
| P6 | `engines.status` is never updated (`set_engine_status` has no callers); every engine reads `provisioning` whenever live status is unavailable. | |
| P7 | The StarRocks bootstrap background task does not check that the engine still exists. | |
| P8 | `redact_catalog_config` strips credentials only for `egret-warehouse`; inline sensitive properties of other kinds are returned to `engine:read`. | |
| C1 | `engine_catalogs.kind` still has `CHECK (kind IN ('egret-warehouse','postgresql','tpch','tpcds','generic'))` (`0057:10`, never dropped), while v2 "connector-first" `add_catalog` and the UI offer mysql, iceberg, clickhouse, … → 500. | V |

## 3. Design — Part A: delete and unassign

### 3.1 Dependents

A **dependent** is a user-authored asset that the delete would destroy or that blocks it. Computed
over the `query.databases` rows with `engine_id = <engine>` (optionally restricted to one workspace):

| Kind | Source | Blocks |
|---|---|---|
| Data API endpoints (live, draft, disabled) | `dataapi.endpoints` | yes |
| Data API endpoints in trash | `dataapi.endpoints WHERE deleted_at IS NOT NULL` | yes (reported separately, purgeable §3.3) |
| Schedules (alerts, reports) on the database | `automation.schedules.database_id` | yes |
| Datasets (and, through them, charts, dashboard cards, report schedules) | `analytics.datasets` | yes |
| Query history | `query.queries` | no — cascades |
| Notebook engine access | `notebook_engine_access` | no — cascades |
| Saved queries | `saved_queries` | no — SET NULL (existing) |

The planner must enumerate the live FK tree again (`pg_constraint`) and fail a test if a new
RESTRICT or NOT-NULL-vs-SET-NULL child of `query.databases` appears that the preflight does not cover.

### 3.2 `DELETE /api/control-plane/engines/{id}`

1. `authz::require("engine:delete")`; load the engine (404).
2. **Preflight.** Compute dependents. If any blocking kind is non-zero → **409**:
   ```json
   {"code":"engine_has_dependents",
    "dependents":[{"workspaceId":"…","workspaceName":"…","kind":"dataapi_endpoint","count":25,"sample":["orders-by-day", "…"]},
                  {"workspaceId":"…","workspaceName":"…","kind":"dataapi_endpoint_trashed","count":1,"sample":["…"]}]}
   ```
   (`sample` ≤ 5 names.) Nothing is changed.
3. Kubernetes must be configured; otherwise **400** "kubernetes not configured" (same as create).
4. Set `status = 'deleting'`.
5. **Kubernetes delete**, per object. `KubeClient::delete` returns `Ok` on 404. Any other error →
   **502** `engine_cluster_delete_failed` with the cause; the row stays `deleting` and the request
   can be retried. Trino and StarRocks both propagate non-404 errors.
6. **One DB transaction:** re-run the blocking preflight under the transaction (race with a
   concurrent endpoint create → 409, row stays `deleting`, cluster already gone — acceptable, the
   next retry succeeds once dependents are handled); delete the engine row (cascades as today);
   delete the engine's secret rows; delete the Ranger profile and its tag service **when auto-created
   for this engine and no other engine is bound**.
7. After commit: revoke catalog clients where `is_minted_client`; failures are logged, not returned.
8. **204.**

Backstop: `From<EngineMgmtError>` / `From<CpError>` log the `sqlx::Error` at `error` level, and map
SQLSTATE `23503` (FK) and `23514` (CHECK) to **409** `constraint_violation` naming the constraint.

### 3.3 Purging trashed Data API endpoints

New route `DELETE /api/ws/{ws}/data-api/endpoints/{id}/permanent`, gated like the existing trash
routes (`data_api_endpoint:manage`), allowed only when `deleted_at IS NOT NULL` (else 409). Hard-deletes
the endpoint; its children cascade (versions, grants, `dataapi_alert` schedules via
`dataapi_endpoint_id ON DELETE CASCADE`; the planner verifies `access_log` behaviour). Audit
`data_api_endpoint:purge`. The Data API trash view gets a "Delete permanently" action with a
type-the-name confirmation.

### 3.4 Unassign

`DELETE /api/admin/workspaces/{ws}/engines/{id}` and `PUT /api/control-plane/engines/{id}/workspaces`
run the preflight restricted to the affected workspace(s) **before any write**. `PUT` computes the
full diff, preflights every unassign, then applies assigns and unassigns in one transaction.
Unknown workspace → 404.

### 3.5 UI

- `EngineDetail` delete dialog states the scope: the cluster is destroyed; query history and
  notebook access on this engine's workspace connections are removed; saved queries lose their
  connection. Type-the-name confirmation.
- On 409, the dialog shows the dependents table (workspace, kind, count, sample) with a link to each
  workspace's Data API / alerts / datasets page. No delete button until the list is empty.
- On 502, a toast with the cause; `status = deleting` renders a "Deleting — retry" badge with a
  **Retry delete** button.
- The workspace assignment tab shows the same dependents table on 409.

## 4. Design — Part B: provisioning hardening

1. **Random internal secrets (P1).** Two new nullable columns `engines.internal_secret_id`,
   `engines.monitor_secret_id` → global named secrets holding 32 random bytes (base64). Created on
   provision; for an existing engine they are created on the next apply (update, catalog change,
   restart) — the derived value is never used again once a secret row exists. New route
   `POST /api/control-plane/engines/{id}/rotate-secrets` (`engine:update`) regenerates both and
   re-applies (pods roll). EngineDetail → Security & Exposure gets a **Rotate secrets** button with a
   "pods will restart" confirmation. Secrets are deleted with the engine (§3.2 step 6).
2. **Re-create with the same name (D9).** `create_service` `AlreadyExists` → if the existing profile
   has no bound engine, reuse it (and its tag service); if it is bound, fail the create with 409.
   While `config.ranger.enabled`, a Trino engine is never provisioned unbound.
3. **Failed create compensation (P2).** Compensation deletes applied Kubernetes objects (404-tolerant),
   the TLS/delegator/internal/monitor secret rows, and policies seeded for this engine, in addition
   to the row and the client. Each step logs its own failure and continues.
4. **Update (P3).** Snapshot the stored spec; persist the new spec; on Kubernetes failure restore the
   snapshot and return 502 (mirrors `update_catalog`). A missing `delegationMode` keeps the stored value.
5. **Rematerialize for Trino (P4).** Any change to delegation mode or TLS calls
   `rematerialize_for_engine` for Trino as for StarRocks.
6. **`remove_catalog` (P5).** Update Kubernetes with the catalog removed first; on success delete the
   row, then revoke the client if minted.
7. **Status (P6).** `status ∈ {provisioning, running, failed, deleting}` with a CHECK (the migration first maps any other stored value to `provisioning`). `create` sets
   `provisioning`; a failed provision that is not rolled back sets `failed`; `get_engine` writes
   `running` when live status reports all pods ready and the stored value is `provisioning`; delete
   sets `deleting`. The UI shows the live status when available, the stored one otherwise.
8. **StarRocks bootstrap (P7).** The background task re-reads the engine by id and `name` before
   each write and exits if it is gone or recreated.
9. **Redaction (P8).** `redact_catalog_config` redacts, for every kind, `credential`, `clientRowId`
   and any property whose key matches `(?i)(password|secret|token|credential|private[-_.]?key|access[-_.]?key)`.
10. **Catalog kind (C1).** Migration drops the `kind` CHECK on `engine_mgmt.engine_catalogs`;
    validation stays in `add_catalog`.

## 5. Testing

- **Part A, DB route tests** (`crates/egret-engine-mgmt/tests/routes.rs`), one per blocking kind:
  endpoint, trashed endpoint, SQL alert, dataset → 409 with the right `kind`/`count`, the engine row
  and FakeKube untouched. Happy path → 204, secrets and auto profile gone, minted client revoked.
  Unassign with each blocking kind → 409, no assignment row changed; `PUT` mixed assign/unassign
  with one blocked → nothing applied.
- **FK-tree guard test**: query `pg_constraint` for children of `query.databases`; assert the set of
  (table, on_delete) equals the set the preflight classifies.
- **FakeKube**: delete returning 404 → success; returning 500 → 502 and `status = deleting`; retry after
  fixing → 204.
- **Error mapping**: a forced FK violation → 409 `constraint_violation`, and the log contains the SQLSTATE.
- **Purge route** (`crates/egret-dataapi/tests/trash.rs`): purge of a live endpoint → 409; purge of a
  trashed endpoint → gone with children; permission denied for viewer.
- **Part B**: re-create same name binds the orphan profile (regression for allow-all); rotate-secrets
  changes the rendered env secret and the password.db hash; update failure restores the spec;
  `remove_catalog` failure keeps row and client; redaction for a `postgresql` catalog with an inline
  password; adding a `mysql` catalog succeeds after the migration.
- **Frontend**: dependents table rendering from a 409 fixture; retry-delete badge; purge action.
- **Live on kind**: `DELETE` `demo-trino` → 409 listing 26 endpoints and the alerts (not 500). No
  data on the cluster is modified without the user's decision.

## 6. Out of scope

- Moving endpoints/datasets/alerts to another connection ("re-point") — a later convenience.
- Cleaning the 18 orphaned `*-tls` Kubernetes Secrets already in `egret-engines` (manual, user decision).
- Anything in SP2–SP5.
