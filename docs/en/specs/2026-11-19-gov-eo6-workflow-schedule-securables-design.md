# EO6 — Workflow & schedule securables (per-object sharing)

Status: design, 2026-11-19. Follow-on to the egret-objects programme [EO0–EO5](2026-11-18-gov-eo0-egret-objects-decomposition-design.md). EO6 realizes the deferred **EO-D6**: it makes **workflows** and **schedules** (alerts + reports) first-class egret-object securables with **per-object sharing**, exactly as dashboards, charts, datasets, saved queries and apps already are — grant a specific workflow or schedule to a user or group, enforced by the PDP, surfaced in the Access tab.

Notebook, the third name in EO-D6, is **gone**: migration `0169_remove_notebook.sql` removed it, its surface was redirected to marimo **Data Apps**, and the `app` kind is already a securable. So EO6 covers the two remaining object types.

## Why

Today workflows and schedules are governed by **workspace-scoped** RBAC only: `workflow:read`/`workflow:manage`/`workflow:use_secrets` and `alert:read`/`alert:manage`. There is no way to share one workflow with a collaborator who is not a workspace editor, or to let a user see a single alert without `alert:read` over the whole workspace — the per-object grant that dashboards have had since the AC series. EO1–EO5 built the one securable model and routed every object decision through the Cedar PDP; EO6 extends that model to the last two object types the product exposes.

## What the live schema says (grounding)

Verified against the EO6 worktree (recon, 2026-11-19):

- **`workflow.flows`** — `id` (PK), `workspace_id` (NOT NULL), `folder_id` (→ `analytics.folders`, nullable, mig 0132), `created_by` (owner), `name`. Full securable shape **with folder cascade**. Its folders carry `entity_type = "flow"` (NOT `"workflow"`).
- **`automation.schedules`** — `id` (PK), `workspace_id` (NOT NULL), `folder_id` (→ `analytics.folders`, mig **0113** — so it DOES cascade), `created_by` (owner), `name`, `type ∈ {alert, report, dataapi_alert}`. One table holds all three; its folders carry `entity_type = "alert"`. **`dataapi_alert` rows have an extra gate** (`require_dataapi_endpoint_read` — the alert's visibility depends on the Data API endpoint it reads); an object grant must preserve that gate, not bypass it.
- RBAC keys today: `workflow:read`/`workflow:manage`/`workflow:use_secrets`; `alert:read`/`alert:manage` — one `alert:*` pair already governs alerts, reports AND dataapi_alerts. **These keys already exist** (catalog in `egret-core/src/rbac.rs`); today they `classify` as `TypedEpPrivilege` (workspace capability). `access_grants.object_type` is **free text — no CHECK constraint** (confirmed `0169`), so no schema migration is needed to admit a new kind string.
- Extension points: `egret_perm_ir::egret_object::{ObjectKind, classify, object_grant}`; `egret-bi/src/shares.rs` (`object_table`/`read_action`/`manage_action`/`gate_kind`); `egret-workspace/src/rbac.rs::has_object_grant_in_workspace` (admits object-guests into `WsContext`); `egret-workspace/src/access_views.rs` + `egret-auth/src/effective.rs` (the shared-objects rollups powering the Access tab). **`object_table` callers hard-code the `analytics.` schema prefix** — the two new tables are in `workflow.`/`automation.`, so the kind→table mapping must become schema-qualified.

## Decisions

| # | Decision |
|---|---|
| EO6-D1 | **Two new securable kinds: `workflow` and `alert`.** Added to `ObjectKind` (variants `Workflow`, `Schedule` with `as_str()` = `"workflow"` / `"alert"`), the `ObjectKind::ALL` array (bump 7→9), `shares.rs::object_table` (`workflow → workflow.flows`, `alert → automation.schedules`), and the object-grant checks. They join the existing securables with the same per-object grant mechanics (`access_grants`, `scope_type='object'`, `object_type` = the kind string). |
| EO6-D2 | **The `alert` kind covers every `automation.schedules` row — alert, report AND dataapi_alert.** The three share one table, one shape and one existing permission pair (`alert:*`); one securable kind governs all, with no `type` filter. The kind STRING is `"alert"` (not `"schedule"`) to match the live router segment (`/alerts`), the folder `entity_type` (`"alert"`), and the existing `alert:*` keys — introducing a `"schedule"` string would mismatch all three. "Covers report too" is satisfied because sharing is on the row regardless of `type`. |
| EO6-D3 | **No new vocabulary, no seeding migration.** `workflow:read`/`workflow:manage` and `alert:read`/`alert:manage` already exist in the catalog and are already seeded to the workspace roles. EO6 only **promotes** them in `classify` from `TypedEpPrivilege` to `ObjectGrant` so they also work as per-object grants. `workflow:use_secrets` stays `TypedEpPrivilege` (a secrets capability, not a share). Because `object_type` is free text (no CHECK) and no keys are added, **EO6 ships zero migrations.** |
| EO6-D4 | **Folder cascade for both.** Both `workflow.flows` and `automation.schedules` have `folder_id`, so a grant on an object's folder cascades to it via the existing `grants_for` `object_scope_ids` path. For the cascade to be grantable, `gate_kind` (which resolves a folder to its `entity_type`) must map the workflow folders' `entity_type = "flow"` to the `workflow` kind, and `"alert"` to the `alert` kind (today both return `None` ⇒ "cannot be shared"). |
| EO6-D5 | **Per-object PDP enforcement at the per-id handlers — matching the dashboard/app precedent exactly.** Each flow/schedule per-id endpoint (read, update, delete, run, publish, versions, set-folder, …) resolves the object (`shares::resolve` → `ObjectRef{kind, id, folder_id, workspace_id}`) and calls `require_object(pdp, ctx, READ|MANAGE, obj)`, so a holder of a direct object grant is authorized for that object even without the workspace permission. **List endpoints stay workspace-gated** (dashboards and apps do NOT list directly-shared objects to a non-member; matching that keeps EO6 consistent and avoids unprecedented list-filtering). **`dataapi_alert` rows keep `require_dataapi_endpoint_read`** — an `alert` object grant must not bypass the Data API endpoint's own read gate. `WsContext` already admits object-guests via `has_object_grant_in_workspace`, which EO6 extends (EO6-D6). |
| EO6-D6 | **Admission + rollups + shares API + Access tab.** Add `workflow`/`alert` arms to: `has_object_grant_in_workspace` (admit a share recipient who is not a workspace member), `access_views.rs` and `effective.rs` (the shared-objects rollups behind the Access tab's "Shared objects" list and who-can-access). The generic `shares_router` endpoints (`PUT/GET/DELETE /ws/{ws}/objects/{kind}/{id}/grants`) then work for the new kinds once `object_table`/`read_action`/`manage_action` have arms. Frontend: add `"workflow"` and `"alert"` to `ShareKind` and open `ShareDialog` from the Workflows and Alerts list pages (the app precedent). |
| EO6-D7 | **Additive and compatible; served through Cedar automatically.** Because workflow/alert become IR securables, the `egret-cedar` compile target (EO2) and the PDP (EO3/EO4) pick them up with no bespoke work — grants flow through the same `grants_for`/`link_grants` path, and `ObjectRef.kind` is a free string. Existing workspace-scoped `workflow:*`/`alert:*` grants keep working unchanged; EO6 only adds the object scope. No vocab removed. Note the `catalog_drift` test (`egret-controlplane/tests/catalog_drift.rs`) scans the enforcing files for each key — the new object-grant uses must sit in files it already lists (`shares.rs`, the workflow/automation routes). |

## Architecture

```text
 ObjectKind::{Workflow, Schedule}  (egret_perm_ir::egret_object)
        │  classify: workflow:read/manage, schedule:read/manage (+ alert:* → Schedule)
        ▼
 shares.rs object_table: workflow→workflow.flows, schedule→automation.schedules
        │  (read_action/manage_action arms; folder cascade for workflow only)
        ▼
 access_grants (scope_type='object', scope_id = flow/schedule id)   ← shares API writes here
        │  grants_for → link_grants  (unchanged PDP path; Cedar compile picks up the new kinds)
        ▼
 workflow/schedule read·manage endpoints consult the PDP  ──►  per-object visibility + access
 Access tab lists workflow/schedule object grants
```

## Scope

**In:** the two new `ObjectKind`s (`Workflow`, `Schedule`→`"alert"`) + `ALL` array bump + `classify` promotions (`workflow:read/manage`, `alert:read/manage` → `ObjectGrant`); `shares.rs` `object_table` (schema-qualified: `workflow.flows`, `automation.schedules`) + `read_action`/`manage_action`/`gate_kind` (`flow`→workflow, `alert`→alert) arms, and the `analytics.`-prefix fix in `object_name`/`object_ref`; `egret-workspace/src/rbac.rs::has_object_grant_in_workspace`, `access_views.rs`, and `egret-auth/src/effective.rs` (both its private `object_table` and rollups, schema-qualified) arms; per-object `require_object` conversion of the flow and schedule per-id handlers (preserving the `dataapi_alert` endpoint-read gate); frontend `ShareKind` + `ShareDialog` on the Workflows and Alerts list pages; tests (classify round-trip, object routing, folder cascade both kinds, end-to-end share→read/manage→revoke per kind, a `dataapi_alert`-still-gated test, `catalog_drift`/`share_deny`/`shares_rollup` updates, decision-equivalence that existing workspace grants are unchanged); openapi.json + client regen; the egret-data/docs update.

**Out:** **any migration** (keys exist; `object_type` is free text — EO6 ships none); any change to what a workflow/alert/report *does* (execution, evaluation, delivery); list-inclusion of directly-shared objects for object-guests (lists stay workspace-gated, matching dashboards/apps — a non-member guest reaches a shared object by id, not via the list); `workflow:use_secrets` as an object grant (stays a capability); tagging the new kinds (governance where-used), unless trivial; removing the workspace-scoped `workflow:*`/`alert:*` grants; the EO3/EO4 flip-to-serve or finalize (soak-gated); any other object type.

## Errors

Fail closed, consistent with the existing securables:

- A principal with neither a workspace permission nor an object grant on a workflow/schedule is denied (unchanged default).
- An unknown object kind at a shares endpoint is rejected (`object_table` returns `None` ⇒ `InvalidInput`).
- A `dataapi_alert` row remains gated by the Data API endpoint's own read even when an `alert` object grant exists — fail closed on the endpoint gate.
- No migration ships, so there is no seed to fail; existing `alert:*`/`workflow:*` grants are untouched.

## Verification

1. **Vocab:** `classify("workflow:read") == ObjectGrant{Workflow, Read}`, `classify("workflow:manage") == ObjectGrant{Workflow, Manage}`, `classify("alert:read") == ObjectGrant{Schedule, Read}`, `classify("alert:manage") == ObjectGrant{Schedule, Manage}`; `workflow:use_secrets` is NOT an object grant (still `TypedEpPrivilege`). `ObjectKind::ALL` round-trips all 9.
2. **Object routing:** `shares.rs::object_table("workflow") == "workflow.flows"` and `("alert") == "automation.schedules"` (schema-qualified); `read_action`/`manage_action` resolve the `:read`/`:manage` keys; `gate_kind` maps folder `entity_type` `"flow"`→workflow and `"alert"`→alert.
3. **Folder cascade (both):** a grant on a workflow's folder reaches the workflow, and a grant on a schedule's folder reaches the schedule.
4. **End-to-end share:** for each kind, a user with no workspace permission but a direct object grant can read (and, with manage, manage) exactly that object by id and no other; revoke removes access on the next reload. The object is NOT added to the workspace list (matches dashboards/apps).
5. **`dataapi_alert` gate preserved:** an `alert` object grant on a `dataapi_alert` row does NOT let the holder see it if they fail `require_dataapi_endpoint_read`.
6. **No regression:** existing workspace-scoped `workflow:*`/`alert:*` grants still authorize unchanged; `catalog_drift`, `share_deny`, and `shares_rollup` tests pass; the EO2/EO3/EO4 differentials stay green.
7. **Served through Cedar:** with the policy source at `shadow`/`compiled`, workflow/alert object grants produce the same decision as the legacy path (the EO2 differential covers the new kinds via the generic `ObjectRef`/`grants_for` path).

## After EO6

Every object type egret exposes is a securable with per-object sharing. The egret-objects programme's object surface is then complete. What remains is the soak-gated tail already tracked: the EO3/EO4 flip-to-serve and the combined finalize.
