# Workflow package 7f — bulk execution operations (design)

**Status:** design, awaiting review (brainstormed 2026-11-23).
**Package:** #7 execution operations, sub-package **7f** (final sub-package, after 7a #860, 7b #866, 7c #868, 7d #872, 7e #904).
**Branch:** `feat/workflow-bulk-ops` (worktree `egret-7f`, off origin/main `93f76bd4`).
**Migration:** none — bulk ops reuse the single-op data paths, which added their own migrations already.

## Goal

Port Kestra's `by-ids` / `by-query` **bulk** execution operations into egret: apply an operation to many executions in one request, either by an explicit list of ids or by the same filter the execution list uses.

## Scope

**Eight operations, each with a `by-ids` and a `by-query` variant = 16 endpoints.** These are exactly the operations egret already has a single-execution route for (7a–7e):

| Operation | Single-op (exists) | Permission |
|-----------|--------------------|------------|
| delete | `DELETE …/{id}` | `workflow:manage` |
| restart | `POST …/{id}/restart` | `workflow:manage` |
| change-status | `POST …/{id}/status` | `workflow:manage` |
| kill | `POST …/{id}/kill` | `workflow:manage` |
| resume | `POST …/{id}/resume` | `workflow:manage` |
| pause | `POST …/{id}/pause` | `workflow:manage` |
| replay | `POST …/{id}/replay` | `workflow:manage` |
| labels | `POST …/{id}/labels` | `workflow:manage` |

**Deferred:** Kestra's `unqueue` and `force-run` bulk ops. They act on QUEUED executions, which belong to the concurrency/queue subsystem (**package #8**, not yet built); egret has no single-op for them yet. They join 7f-style bulk coverage when #8 lands.

Endpoints (egret paths — no Kestra `/actions/` segment; all `POST` except delete/kill which are `DELETE`, matching Kestra):

```
{DELETE|POST} /api/ws/{ws}/workflows/executions/{op}/by-ids
{DELETE|POST} /api/ws/{ws}/workflows/executions/{op}/by-query
```
where `{op}` ∈ {delete, restart, change-status, kill, resume, pause, replay, labels}. (`delete`'s own path is the bare `.../executions/by-ids` and `.../executions/by-query`, matching Kestra's `DELETE /by-ids`.)

## Architecture

All handlers live in `crates/egret-workflow/src/routes.rs` beside the single-op routes. Each bulk op:

1. Gates `workflow:manage` (every op is a write).
2. Resolves its target id set:
   - **by-ids:** the request body is a JSON `["uuid", …]`.
   - **by-query:** reuse the existing `ExecutionFilter` / `execution_filter` helper (same query params as `list_executions`) → resolve to the matching ids within the workspace. **by-query delegates to the by-ids path once resolved** (Kestra's pattern).
3. Enforces a **cap** (`MAX_BULK`, default 1000) on the resolved id count; over the cap → 400 `too_many` (bounds the synchronous work).
4. **Validates every id up front (all-or-nothing)** against the op's precondition — reusing each single-op's existing validation and per-object PDP check (existence + ownership + state precondition, e.g. restart requires terminal/paused, kill requires running). A foreign-workspace or non-existent id is "not found" (404-class, never a 403 disclosure). If **any** id is invalid, respond **400 `BulkErrorResponse { message, invalids: [{ id, reason }] }` and apply NOTHING**.
5. When all are valid, apply the op to each (the same code path the single-op uses), in one pass, and respond **200 `BulkResponse { count }`**.

**Synchronous, not async (divergence):** Kestra emits each action to an `executionCommandQueue` and returns `202 Accepted` with an async-operation id. egret has no such command queue — its single-ops mutate state inline and the worker picks the work up — so egret's bulk ops run **synchronously inline** and return **200** with the count. The cap keeps the request bounded (each op is a quick state mutation / row write, not a blocking task run — the worker still does the actual execution asynchronously).

### Shared machinery

A small `bulk_apply` helper takes the resolved ids, a per-id validator closure, and a per-id apply closure; it runs the validate-all / then apply-all / all-or-nothing protocol once, so all eight ops share identical semantics and only supply their precondition + action. Each op's validator and action call the SAME functions the single-op route already uses (no reimplementation of preconditions).

## Security

- Every endpoint gates `workflow:manage`.
- by-query is workspace-scoped through `ExecutionFilter` (same as `list_executions`); it can never reach another workspace's executions.
- Per-id per-object PDP: every execution is a securable (EO programme). A caller who holds `workflow:manage` on the workspace but is denied a specific execution by a per-object policy fails that id as a 404-class invalid (no 403 disclosure), which — under all-or-nothing — fails the whole batch. This matches the single-op 404-not-403 rule.
- A bulk delete/kill is as destructive as N single ops; the cap and the all-or-nothing validation are the guardrails.

## Frontend

On the executions list (`ExecutionsList`):
- **Row selection checkboxes** + a **bulk-action toolbar** that appears when ≥1 row is selected: Restart, Kill, Delete, Change status, Replay, Resume, Pause, Edit labels (each enabled only when the selection is eligible, mirroring the single-op buttons).
- **"Select all matching the current filter"** affordance → the `by-query` variant (so a user can act on every execution a filter matches, not just the page).
- A confirmation dialog for the destructive ones (delete/kill), showing the count. On a 400 `BulkErrorResponse`, surface which ids were rejected and why.

## Tests

`crates/egret-workflow/tests/executions_routes.rs` (no migration):
- For a representative subset across the op kinds (at least delete, restart, kill, labels, change-status): by-ids applies to all when all valid (200, count); a single invalid id (not found / wrong state / foreign workspace) → 400 `BulkErrorResponse` listing it and **nothing applied** (assert the valid ones were NOT mutated); by-query resolves the filter and applies to exactly the matching set; the cap is enforced (over `MAX_BULK` → 400); `workflow:read`-only caller → 403; empty id list / empty match → 200 count 0 (or a clear no-op).
- One test proving all-or-nothing atomicity: a batch of 3 where #2 is invalid leaves #1 and #3 unchanged.

## Kestra divergences (to record)

1. Paths omit `/actions/`.
2. **Synchronous (200 `BulkResponse`), not async (202 + command queue)** — egret has no `executionCommandQueue`.
3. A **cap** (`MAX_BULK`, default 1000) bounds the synchronous batch; Kestra has no explicit cap (it is async).
4. `unqueue` and `force-run` bulk ops are **deferred** to package #8 (queue subsystem).
5. by-query resolves to ids then runs by-ids (same as Kestra), all-or-nothing (same as Kestra).

## Open decisions (defaults chosen; confirm in review)

- **Q1 scope:** the 8 ops egret already has single-ops for; defer unqueue/force-run. ✅ (user)
- **Q2 failure semantics:** all-or-nothing (Kestra parity) — validate all, 400 with invalids if any, else apply all. ✅ (user)
- **Q3 cap:** `MAX_BULK = 1000` for by-ids length and by-query match count. *(default — confirm the number)*
- **Q4 response shape:** 200 `BulkResponse { count }`; 400 `BulkErrorResponse { message, invalids: [{id, reason}] }` (Kestra names). *(default)*
- **Q5 FE scope:** selection checkboxes + bulk toolbar + select-all-matching-filter + destructive confirmation. *(default — confirm)*
