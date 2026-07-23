# Per-workspace feature entitlements — Design

## 1. Goal

Let a platform admin decide **which modules a workspace may use at all**:

- workspace A — everything;
- workspace B — BI only, no Notebooks;
- workspace C — any other combination of the five modules.

This is a **tenant-level entitlement**, deliberately distinct from the two
mechanisms egret already has:

| Mechanism | Question it answers | Where it lives |
| --- | --- | --- |
| RBAC roles (`ws_viewer/editor/admin`) + Cedar PDP | *May this user do this action?* | `access_grants`, `egret-auth/pdp` |
| Ranger / classification policies | *May this user see this data?* | data plane |
| **Feature entitlements (this doc)** | *Does this workspace have this module?* | `workspace_features` |

The two axes are independent and compose by AND: a `ws_editor` in a workspace
with `notebooks` disabled gets no notebooks; a `ws_viewer` in a workspace with
`notebooks` enabled gets read-only notebooks.

## 2. Scope

**Feature = module**, exactly one toggle per module, five keys:

`sql_lab`, `datasets`, `charts`, `dashboards`, `notebooks`

Read/write depth stays with RBAC — a feature has no "read-only" state.

**Only `platform_admin` toggles features**, from Control Plane → Workspaces →
detail → *Features*. `ws_admin` cannot; entitlement is granted by the platform to
the tenant, not chosen by the tenant. This matches the ADR-19 three-layer model.

**Absent row means enabled.** No backfill; every existing workspace keeps its
current behaviour. A row is written only when an admin changes something.

### 2.1 Non-goals

Deliberately excluded from this design:

- plan/tier presets (`Full`, `BI-only`, …) — the toggles are the primitive; a
  preset is UI sugar that can be added later without a schema change;
- `ws_admin` self-service narrowing;
- reclaiming running resources on disable (killing a workspace's kernel pods,
  revoking runtime tokens, stopping scheduled jobs);
- the four placeholder nav items (`workflows`, `alerts`, `data_api`,
  `pipelines`) — they render `<Placeholder>`, so there is nothing to gate. They
  get keys when they become real modules;
- per-user feature overrides.

Data of a disabled module is **preserved untouched** — it simply becomes
unreachable, and re-enabling restores access.

### 2.2 Platform admin does NOT bypass the gate

`platform_admin` bypasses permission checks everywhere else in egret, and that is
correct: permissions describe what a *person* may do. An entitlement describes
what a *workspace* is, so it applies to everyone inside that workspace, admins
included. If notebooks are off for workspace B, a platform admin browsing
workspace B sees no notebooks either.

The escape hatch is to re-enable the feature, which is a two-click, audited
action. Silently exempting admins would make the toggle mean "hidden from most
people", which is not what it claims.

## 3. Data model

Migration `0094_workspace_features.sql`:

```sql
CREATE TABLE workspace_features (
  workspace_id UUID        NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  feature      TEXT        NOT NULL,
  enabled      BOOLEAN     NOT NULL,
  updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_by   UUID        REFERENCES users(id) ON DELETE SET NULL,
  PRIMARY KEY (workspace_id, feature)
);
```

Notes:

- `feature` is `TEXT`, validated against the code catalog rather than a Postgres
  enum, so adding a module later needs no migration.
- `ON DELETE CASCADE` on the workspace; `ON DELETE SET NULL` on `updated_by`,
  consistent with migration 0093's treatment of user attribution columns.
- Rows persist through re-enabling (`enabled = true`) so `updated_by` /
  `updated_at` keep a trace of who turned it back on.

Because a missing row reads as enabled, the whole table is empty on first deploy
and the feature is inert until someone uses it.

## 4. Feature catalog (code)

New module `crates/egret-workspace/src/features.rs` — the single source of truth
for the middleware, the API responses, the UI list, and the route-coverage test.
It mirrors how the permission catalog is code-defined (`#174`).

```rust
/// One gated HTTP surface: a path prefix under `/api/ws/{workspace}/`, plus the
/// methods it applies to (`None` = every method).
pub struct GateRule {
    pub prefix: &'static str,
    pub methods: Option<&'static [Method]>,
}

pub struct Feature {
    pub key: &'static str,          // "notebooks"
    pub label: &'static str,        // "Notebooks"
    pub description: &'static str,  // shown in the Features tab
    pub rules: &'static [GateRule],
}

pub const FEATURES: &[Feature] = &[ /* the five below */ ];

/// Workspace-scoped path prefixes that are never gated. Every prefix reachable
/// under `/api/ws/{workspace}/` must appear either in a `Feature` or here — see
/// the coverage test in §8.1.
pub const UNGATED: &[&str] = &[ /* … */ ];
```

### 4.1 The map

| Feature key | Gated surface under `/api/ws/{workspace}/` |
| --- | --- |
| `sql_lab` | `sqllab/**` (all methods) and `POST /queries` (free-form SQL submit) |
| `datasets` | `datasets/**` |
| `charts` | `charts/**` |
| `dashboards` | `dashboards/**` |
| `notebooks` | `notebooks/**`, `kernel/**`, `notebook-runtime/**`, `notebook-settings/**` |

Never gated: `members`, `roles`, `shares`, `objects`, `folders`, `engines`,
`databases`, `access-requests`, and the non-submit part of `/queries` —
`GET /queries`, `GET /queries/{id}`, `GET /queries/{id}/results`,
`GET /queries/{id}/results.csv`, `POST /queries/{id}/cancel`.

The rationale for splitting `/queries` by method: the privileged act is
**submitting** SQL, not reading back or cancelling a query you already own.
Chart authoring needs the read-back path (§6), so gating it would break charts
for a reason unrelated to SQL Lab.

`folders` and `objects` stay ungated because they are content-type-neutral
(a folder holds charts *and* notebooks; `objects` is the sharing surface). Their
per-object authorization already runs through the PDP.

## 5. Enforcement — inside `WsContext`

The check goes into `WsContext::from_request_parts`
(`crates/egret-workspace/src/context.rs`), as a new step between "load the active
workspace" (step 3) and "resolve membership" (step 4):

```text
3.  Load the active workspace by slug.
3.5 Resolve the module this request belongs to from `parts.uri.path()`.
    If that module is disabled for this workspace → Error::FeatureDisabled(key).
4.  Membership / role resolution (unchanged).
```

Ordering matters: the feature check runs **before** membership so a member of a
workspace with notebooks off and a non-member both get a clean 403, and neither
learns anything extra.

### 5.1 Why here rather than a `.layer()`

Every workspace-scoped route already uses `WsContext` — verified across
`egret-bi` (58 uses), `egret-query` (execution + registry routes), `egret-notebook`
(including `runtime.rs`), `egret-kernel`, and `egret-workspace`. They *must*:
it is the only way to resolve the `{workspace}` slug into a workspace id.

That gives two properties a separate layer cannot:

1. **Coverage by construction.** A new workspace-scoped route cannot exist
   without `WsContext`, so it cannot exist outside the gate. With a layer, the
   guarantee would depend on remembering to mount it — and `egret-bi` mounts one
   router carrying charts, dashboards, datasets and folders together, so a
   per-router layer would need path matching anyway.
2. **No duplicated lookup.** `WsContext` has already resolved the workspace; a
   layer would have to resolve the slug again.

The cost is that `WsContext` now does two things. That is acceptable: both are
"can this request touch this workspace at all", and the alternative is worse.

### 5.2 Matching

Take the path suffix after `/api/ws/{slug}/`, match its **longest** prefix
against the catalog (so `sqllab/queries` resolves before `sqllab`), and check the
method set. Unmatched prefixes are looked up in `UNGATED`; anything in neither
list is a bug caught at test time (§8.1), not at runtime — at runtime an
unmatched prefix is allowed, because failing closed on an unmapped route would
turn a catalog omission into a production outage.

### 5.3 Error shape

New variant `Error::FeatureDisabled(&'static str)` in `egret-core/src/error.rs`,
mapping to **403**.

`ErrorBody` (`egret-core/src/http.rs`) gains an optional machine-readable code so
the frontend can distinguish this from an ordinary authorization failure:

```rust
pub struct ErrorBody {
    pub error: String,                    // "feature disabled: notebooks"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,             // "feature_disabled"
}
```

The field is additive and omitted for every existing error, so no current
response changes shape.

### 5.4 Cost

One extra indexed query (`SELECT feature FROM workspace_features WHERE
workspace_id = $1 AND enabled = false`) per workspace-scoped request. `WsContext`
already runs two queries, so this is the same order of magnitude. **No cache in
v1** — add a small TTL cache with invalidation on write only if measurement shows
it matters.

## 6. Separating chart execution from free-form SQL

`ChartExplore.tsx` currently builds SQL from the chart's `query_spec` and submits
it through the generic `POST /api/ws/{workspace}/queries` endpoint. So gating
that endpoint on `sql_lab` would also kill the chart builder — an unrelated
module.

Fix, in the `charts` module:

- **New** `POST /api/ws/{workspace}/charts/run`, body `{dataset_id, query_spec,
  limit}`. It performs the existing build step **server-side** and submits the
  resulting SQL to the executor, returning the same `{id, status}` ack. Raw SQL
  never crosses the wire when authoring a chart.
- Status and results keep using `GET /queries/{id}` and
  `GET /queries/{id}/results`, which are ungated (§4.1).
- `POST /queries` therefore becomes free-form SQL and nothing else, and gating it
  on `sql_lab` is honest.

This also closes a pre-existing hole: today anyone who can author a chart can
submit arbitrary SQL through the same endpoint.

## 7. API and frontend

### 7.1 Control-plane API

Added to `admin_workspace_router` (`crates/egret-workspace/src/routes.rs`), reusing
the platform-scoped permissions the sibling routes already use — `workspace:read`
for the read and `workspace:update` for the write, both already in the code
catalog (`crates/egret-core/src/rbac.rs`). No new permission key: toggling a
module is the same class of act as renaming or archiving a workspace, and today
only `platform_admin` holds these at platform scope.

- `GET /api/admin/workspaces/{id}/features` →
  `[{key, label, description, enabled}]` — all five, computed as
  *catalog minus disabled rows*.
- `PUT /api/admin/workspaces/{id}/features`, body
  `{"features": {"notebooks": false, "sql_lab": true, …}}` — full replacement of
  the five keys. Unknown keys → `400`. Upsert in one transaction, then emit an
  `AuditEvent` (`action = "workspace.features.update"`, `resource_type =
  "workspace"`, `detail` = before/after map) through the existing `AuditSink`.

### 7.2 Member-facing API

`GET /api/me/workspaces` (`list_my_workspaces`) — `WorkspaceDto` gains
`features: Vec<String>`, the enabled keys for that workspace. One batched query
for the whole list, not one per workspace.

### 7.3 Frontend

- **`components/layout/Sidebar.tsx`** — `NavItem` gains `feature?: string`;
  `EXPLORE_GROUPS` is filtered against the current workspace's features, and
  groups left empty are dropped heading and all. This reuses the filtering shape
  already written for the control-plane groups.
- **`App.tsx` / `ExploreRoutes`** — gated routes are wrapped in a
  `<RequireFeature name="…">` guard that renders the existing `Forbidden` route.
  The `<Route index>` redirect must target the **first enabled module** rather
  than the hard-coded `sql-lab`, otherwise a workspace with SQL Lab disabled
  lands on a blocked page.
- **`ChartExplore.tsx`** — `run()` calls `/charts/run` instead of
  `useSubmitQuery`; status/results polling is unchanged.
- **Control Plane → Workspaces → detail → Features tab** — five `Switch` rows
  with label + description, a Save button, and a toast, following the config-table
  pattern established in `#328`.
- **API client** — a `403` carrying `code: "feature_disabled"` renders the
  Forbidden page rather than a generic error toast. This covers the race where an
  admin disables a module while a user has the tab open.

## 8. Testing

### 8.1 Route-coverage test (the important one)

A Rust test reads the generated `openapi.json`, collects every distinct path
prefix under `/api/ws/{workspace}/`, and asserts each one is either claimed by a
`Feature` or listed in `UNGATED`. Adding a new module route without declaring it
fails CI, which is the guarantee that per-handler checks would otherwise have to
provide by hand.

### 8.2 Integration tests (sqlx, against the test database)

- Disable `notebooks` → `GET /ws/{s}/notebooks` is `403` with
  `code = "feature_disabled"`, while `GET /ws/{s}/charts` is still `200`.
  Re-enable → `200`.
- Disable `sql_lab` → `POST /queries` is `403`, but `POST /charts/run` is `200`
  and `GET /queries/{id}/results` for that run is `200`.
- A `platform_admin` gets the same `403` as everyone else (§2.2).
- `PUT …/features` with an unknown key → `400`, and nothing is written.
- Disabling a module leaves its rows intact: count notebooks before disable,
  re-enable, count again.

### 8.3 Frontend (vitest)

- Sidebar omits items for disabled features and drops emptied groups.
- `RequireFeature` renders Forbidden for a disabled module.
- The index redirect picks the first enabled module.

### 8.4 End-to-end (Playwright)

One spec: platform admin disables Notebooks for a workspace → a member of that
workspace sees no Notebooks nav item, and navigating directly to the notebooks
URL is blocked.

## 9. Rollout

Additive and reversible. New table starts empty, absent means enabled, so
deploying the migration changes nothing observable. The only behavioural change
that ships unconditionally is the chart-run split (§6), which is covered by the
existing chart tests plus §8.2.

Order of work: catalog + migration + repo → `WsContext` gate + error shape →
`/charts/run` + `ChartExplore` switch → control-plane API → Features tab →
sidebar/route guards → tests.
