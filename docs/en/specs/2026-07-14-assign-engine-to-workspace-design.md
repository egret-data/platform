# Assign Provisioned Engine → Workspace — Design

- **Date:** 2026-07-14
- **Status:** Approved (design), pending implementation plan
- **Depends on:** control-plane workspace cockpit (PR #246, merged), B3a per-user delegation (PR #200), CP engines (`egret-engine-mgmt`, migrations 0056–0058)

## Problem

egret has **two disconnected "engine" concepts**:

1. **Provisioned engines** — the `engines` table (migration 0056): platform-global Trino/StarRocks/RisingWave clusters that egret itself provisions onto k8s, each wired to a warehouse + `engine_catalogs` (each catalog maps to a warehouse→storage and an optional `secret_ref`→secret) + an optional Ranger `authz_service`. **They have no link to any workspace**, and a workspace cannot see or use them.
2. **Self-service connections** — the per-workspace `databases` table: engine connections a workspace admin adds/edits/deletes at `/explore/:ws/engines` (`DatabasesPage`).

We want a platform admin to **assign a provisioned engine to a workspace** so that:
- the workspace's Engines page **shows the provisioned engine read-only** (cannot edit/delete — it's platform-managed), alongside the workspace's own self-service connections, and
- the engine becomes **usable for queries** in that workspace (SQL Lab), with **per-user authorization** (each user queries as themselves, Ranger-enforced), and
- the read-only card **surfaces what the engine maps to** — its catalogs, and for each catalog the storage (via the warehouse) and secret it references.

## Key finding — auth is already solved

The Trino connector (`egret-query/src/execution/connector/trino.rs`) and the query path already support **per-user JWT delegation** (B3a, PR #200, migration 0075):

- `databases.delegated` (boolean). When `true`, `execution/routes.rs` mints the caller's short-lived **engine JWT** via `EngineTokenMinter::mint(user_id)` and sends `Authorization: Bearer <jwt>`; Trino authenticates the request as that user (via `principal-field`), and Ranger enforces per-user. A non-OIDC caller is denied (403) — never falls back to shared creds.
- A non-delegated connection keeps shared-service behavior.

So **making an assigned engine query as the real user requires no new auth work** — the materialized connection just sets `delegated = true`. (Trino impersonation via `X-Trino-User` is also supported by the connector but is **not** used here; it would need a shared service credential, which delegation avoids. Impersonation stays a future fallback only for engines not configured with JWT auth.)

## Design

### Core approach: materialize a read-only `databases` row

On assignment, insert a **read-only `databases` row** in the target workspace that points at the engine's coordinator, reusing the entire existing query path. This means SQL Lab, the engine job, the connector, and delegation all work unchanged — the only new query-path concept is a flag marking the row as provisioned/read-only.

- Coordinator connection is derived from the engine: `host = {engine.name}-coordinator.{engine.namespace}.svc`, `port = 8080`, `engine = engine.type` (`trino`/`starrocks`), `options` seeded from the engine's primary catalog. `delegated = true`, `secret_id = NULL`.
- **Marker + link:** add `databases.engine_id uuid NULL REFERENCES engines(id) ON DELETE CASCADE`. A **non-null** `engine_id` means the row is provisioned (read-only, managed by an assignment) and links back to the source engine for surfacing catalogs/storage/secret. **Null** = ordinary self-service connection (unchanged).

### Data model

```sql
-- Migration NNNN: engine ↔ workspace assignment (many-to-many)
CREATE TABLE engine_workspaces (
    engine_id    uuid NOT NULL REFERENCES engines(id) ON DELETE CASCADE,
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    assigned_by  uuid REFERENCES users(id),
    assigned_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (engine_id, workspace_id)
);

-- Marker/link on the materialized connection:
ALTER TABLE databases
    ADD COLUMN engine_id uuid NULL REFERENCES engines(id) ON DELETE CASCADE;
```

Invariant: for each `engine_workspaces(e, w)` there is exactly one `databases` row with `engine_id = e AND workspace_id = w`. Assign creates both; unassign deletes both (the `databases` row is removed explicitly, not only via cascade, so it happens even when unassigning without dropping the engine).

### Assignment lifecycle (backend, `egret-engine-mgmt`)

- **Assign(engine, ws):** insert `engine_workspaces` row; materialize the `databases` row (name defaulted to the engine name, de-duplicated against existing names in the workspace — append a numeric suffix on `(workspace_id, name)` conflict). Gated `workspace:assign_engine` (new platform permission) / platform_admin.
- **Unassign(engine, ws):** delete the materialized `databases` row for `(engine_id, workspace_id)`, then the `engine_workspaces` row.
- **Assignability guard:** only engines whose spec enables JWT auth (`engine.spec.auth.jwt` present) may be assigned, because delegation requires the coordinator to accept the minted engine JWT. Assigning a non-JWT engine is rejected with a clear error (fix the engine's auth first). *(Scope note: v1 targets `trino`; `starrocks` delegation parity is a follow-up.)*

### Read-only enforcement

The existing workspace database routes (`egret_query::workspace_database_router`, `PUT/DELETE/POST …/databases/{id}` and the Test-connection route) reject the request with 409/403 when the target row has `engine_id IS NOT NULL`. These rows are managed only through the assignment API.

### Backend surfaces

- **`egret-engine-mgmt`** (CP engines):
  - `GET /api/control-plane/engines/{id}/workspaces` → assigned workspaces `[{workspace_id, name, slug, assigned_at}]`.
  - `PUT /api/control-plane/engines/{id}/workspaces` → set the full assignment set (diff → assign/unassign, materializing/removing `databases` rows).
- **Workspace-admin side** (CP Workspace cockpit, `admin_workspace_router`):
  - `GET /api/admin/workspaces/{id}/engines` → engines assigned to this workspace (read model joining `engine_workspaces` + `engines`).
  - `POST /api/admin/workspaces/{id}/engines {engine_id}` / `DELETE …/engines/{engine_id}` → assign / unassign (same logic as above).
- **Read-only engine view for the workspace** (`egret-engine-mgmt`, gated `catalog:read` on the ws):
  - `GET /api/ws/{ws}/engines/{engine_id}` → `{name, type, status, catalogs: [{name, kind, storage_name, secret_name}]}` — catalogs from `engine_catalogs`; `storage_name` resolved via the engine's warehouse → storage connection; `secret_name` resolved from `secret_ref` (name only, never the value). Only returns engines assigned to `{ws}`.

### Frontend surfaces

- **CP → Engine detail** (`egret-engine-mgmt` FE): new **Workspaces** tab — a `FilterableTable`/checkbox list of all workspaces with assign/unassign, calling `PUT …/engines/{id}/workspaces`.
- **CP → Workspace detail** (the cockpit from PR #246): the **Resources** tab gains an **Assigned engines** section — list assigned engines + **Assign engine** dialog (pick from unassigned JWT-capable engines) + remove, calling `/api/admin/workspaces/{id}/engines`.
- **Workspace `/explore/:ws/engines`** (`DatabasesPage`): split into **two groups**, partitioned by `engine_id`:
  - **Provisioned by egret** (read-only): each engine as a card — type · status · catalogs; each catalog shows its `kind`, storage, and secret name. No Add/Edit/Delete/Test.
  - **Your connections** (self-service): unchanged (Add/Edit/Delete/Test).
- **SQL Lab** engine picker: lists both self-service and provisioned connections; provisioned ones carry an "egret" tag. No picker logic change — both are `databases` rows.

### Query flow (unchanged)

```
User runs a query in ws against a provisioned engine
  → picks the materialized `databases` row (engine_id set, delegated=true)
  → execution/routes.rs: db.delegated → EngineTokenMinter.mint(user) → Bearer JWT
  → Trino connector → coordinator ({name}-coordinator.{ns}.svc:8080), Authorization: Bearer
  → Trino authenticates as the user (principal-field); Ranger (authz_service) enforces per-user
```

## Phasing

This is an epic; build in two increments (auth needs no phase of its own — it comes via `delegated = true`).

- **P1 — backend core:** `engine_workspaces` table + `databases.engine_id`; assign/unassign that materializes/removes the read-only `databases` row; read-only guards on the workspace database routes; CP engine-mgmt assignment endpoints + workspace-admin assignment endpoints; assignability (JWT) guard. Delegated queries against an assigned engine work end-to-end (Trino).
- **P2 — surfaces:** CP Engine-detail Workspaces tab; CP Workspace-detail Assigned-engines section; workspace Engines page two-group read-only view + catalog/storage/secret surfacing; SQL Lab "egret" tag.

Each phase gets its own plan → implementation cycle.

## Testing

- **Backend (P1):** assign materializes a `databases` row with `engine_id`, `delegated=true`, coordinator host/port; unassign removes it; the workspace DB routes 409 on edit/delete/test of a provisioned row; name de-dup on conflict; non-JWT engine → assign rejected; assign is idempotent; `PUT …/workspaces` diff assigns/unassigns correctly.
- **Live (P1):** on the dev stack, assign a JWT-configured provisioned Trino to a workspace, then run a SQL Lab query as a workspace user → returns rows, and the query reaches Trino as that user (delegation). This is manual Playwright-MCP driving, not an automated test.
- **Frontend (P2):** Vitest for the Engine-detail Workspaces tab, the Workspace-detail Assigned-engines section, and the two-group Engines page (provisioned cards read-only, no action buttons; self-service unchanged).

## Out of scope / future

- **StarRocks** delegated assignment (v1 is Trino; StarRocks JWT parity is a follow-up).
- **Impersonation** mode (`X-Trino-User`) for non-JWT engines.
- Editing a provisioned engine's catalogs from the workspace (managed only in CP).
- Per-catalog visibility scoping within an assigned engine (all its catalogs are visible/queryable subject to Ranger).
- RisingWave assignment.
