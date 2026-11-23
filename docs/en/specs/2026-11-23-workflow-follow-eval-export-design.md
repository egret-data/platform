# Workflow package 7e — follow, eval, stats & export (design)

**Status:** design, awaiting review (brainstormed 2026-11-23).
**Package:** #7 execution operations, sub-package **7e** (after 7a #860, 7b #866, 7c #868, 7d #872).
**Branch:** `feat/workflow-follow-eval-export` (worktree `egret-execops`, synced to origin/main `7fd2e5e59`).
**Migration:** none — every endpoint is read-only.

## Goal

Port the remaining read-side surface of Kestra's `ExecutionController` and
`LogController` into egret: live **follow** of an execution and its logs over
Server-Sent Events, ad-hoc **eval** of a Pebble expression in an execution's
(or a task run's) render context, and the small **stats/listing** endpoints the
UI leans on — distinct namespaces/flows, a flow's average execution duration,
the latest execution per flow, and a CSV export of the execution list.

One Kestra endpoint in this area, `GET /{id}/follow-dependencies`, is **deferred**
(it is `x-skipped` in Kestra's own SDK and needs a subflow-tree walk plus a third
stream); it is recorded as a follow-up, not shipped here.

## Scope

Shipped (egret paths — egret does not use Kestra's `/actions/` segment):

| # | egret path | Method | Kestra origin |
|---|------------|--------|---------------|
| 1a | `POST /api/ws/{ws}/workflows/executions/{id}/eval` | POST `text/plain` | `POST /{id}/actions/eval` |
| 1b | `POST /api/ws/{ws}/workflows/executions/{id}/task-runs/{taskRunId}/eval` | POST `text/plain` | `POST /{id}/actions/eval/{taskRunId}` |
| 2 | `GET /api/ws/{ws}/workflows/executions/{id}/follow` | GET SSE | `GET /{id}/follow` |
| 3 | `GET /api/ws/{ws}/workflows/executions/{id}/logs/follow` | GET SSE | `GET /logs/{id}/follow` |
| 5a | `GET /api/ws/{ws}/workflows/executions/namespaces` | GET | `GET /namespaces` |
| 5b | `GET /api/ws/{ws}/workflows/executions/namespaces/{ns}/flows` | GET | `GET /namespaces/{ns}/flows` |
| 6 | `GET /api/ws/{ws}/workflows/executions/namespaces/{ns}/flows/{flowId}/average-duration` | GET | `GET /namespaces/{ns}/flows/{flowId}/average-duration` |
| 7 | `POST /api/ws/{ws}/workflows/executions/latest` | POST | `POST /latest` |
| 8 | `GET /api/ws/{ws}/workflows/executions/export.csv` | GET `text/csv` | `GET /export/by-query/csv` |

Deferred (follow-up): `#4` follow-dependencies.

## Architecture

All handlers live in `crates/egret-workflow/src/routes.rs` next to the existing
execution routes, registered in the same router block, and documented with
`utoipa::path` exactly like their neighbours. New support:

- **SSE producer helper** (new, small): an `axum::response::sse::Sse` response
  built from a `tokio_stream` that drives a poll loop. There is no existing
  server-side SSE *producer* in egret — `http/sse.rs` is a client-side *parser*
  from package #4 and is unrelated. The engine-pod log follow uses WebSocket and
  is a separate subsystem; we do not reuse it.
- **Eval** reuses the existing render machinery (`RenderContext` / the
  `VariableRenderer` analog used by outputs/SLA). The HTTP layer only loads the
  execution (and task run), builds the render context, renders one string, and
  maps a render error to the `error` field.
- **Repo methods** (`crates/egret-workflow/src/repo.rs`) for distinct, latest,
  average-duration and the export cursor — all plain `SELECT`s.

### Poll→SSE (decision Q1)

egret has no execution event bus (the frontend currently polls
`GET …/executions/{id}/logs?after=` via `useLogFollow`). Rather than build
Postgres `LISTEN/NOTIFY` push into the executor and log writer, the follow
handlers **poll the database on a fixed interval and emit an event only when the
observed state changes**, keeping the Kestra endpoint *shape* (`text/event-stream`,
consumable by the browser `EventSource`) without new infrastructure.

Mechanics (both follow streams):
- On connect, resolve the execution under `workflow:read`; a missing execution or
  one in another workspace is **404 before the stream opens** (not an error event).
- Emit an initial event with the current snapshot, then poll every **1s** (named
  const; tunable). Emit a new event only when the snapshot differs from the last
  emitted one.
- **Execution follow (#2):** the snapshot is the execution DTO (same shape as
  `GET /executions/{id}`). When the execution reaches a terminal state, emit one
  final event and **close the stream**. Kestra's `followExecution` closes on the
  terminal state the same way.
- **Logs follow (#3):** reuse the server-side watermark that backs the `?after=`
  tail. Each poll fetches lines after the last emitted offset and emits them;
  the stream closes once the execution is terminal **and** the log tail is drained
  (same "settle watermark" rule 7d used for log download).
- Send SSE keep-alive comments (axum `KeepAlive`) so idle connections survive
  proxies.
- The poll query is one indexed `SELECT` per interval per stream; cost is linear
  in open streams, acceptable for the UI's one-viewer-per-execution pattern.

### Eval (#1, decisions Q2)

- Body is the raw Pebble expression (`text/plain`), matching Kestra.
- Response: `{ "result": string | null, "error": string | null }` (Kestra's
  `EvalResult`). A successful render fills `result`; an
  `IllegalVariableEvaluationException` analog fills `error` and still returns
  **200** (Kestra returns the error in-band, not as an HTTP error).
- **Secrets are sealed on BOTH paths** — execution-level and task-run — which is
  stricter than Kestra (Kestra seals only the task-run path). This prevents a
  `workflow:read` caller exfiltrating `{{ secret('X') }}`. Recorded as a
  deliberate divergence.
- Gate: `workflow:read` (eval inspects, it does not run tasks). egret's `egret.sql`
  creator gate does not apply (no task runs).
- `1b` resolves the task run within the execution; an unknown `taskRunId` is
  **404**.

### Stats & export

- **distinct (#5):** `SELECT DISTINCT namespace …` and `SELECT DISTINCT flow_id
  WHERE namespace = $1 …`, scoped to the workspace, ordered, for the filter
  dropdowns.
- **average-duration (#6):** direct SQL over executions that reached a terminal
  state with both a start and end timestamp in the last **30 days** (Kestra's
  `AVERAGE_DURATION_LOOKBACK = Duration.ofDays(30)`):
  `SELECT AVG(<end> - <start>), COUNT(*)` (exact timestamp columns confirmed
  against the schema in the plan). Response
  `{ "avgDurationMs": number | null, "count": number }`; `avgDurationMs` is
  `null` when `count = 0`. This replaces Kestra's daily-statistics subsystem
  (egret has none); a flat `AVG` is the count-weighted mean Kestra computes by
  summing buckets, so the number matches.
- **latest (#7):** body is `[{ "namespace": string, "id": string }]` (Kestra's
  `FlowFilter`, where `id` is the flow id). Response is a list of
  `{ id, flowId, namespace, startDate, status }` (Kestra's `LastExecutionResponse`):
  the newest execution per requested flow via `DISTINCT ON (flow_id) … ORDER BY
  flow_id, created_at DESC`. Flows with no execution are omitted.
- **export (#8):** `GET …/executions/export.csv` reusing `ExecutionFilter` (the
  same query parameters as `list_executions`), streamed as `text/csv` with
  `Content-Disposition: attachment; filename=executions.csv`. One header row plus
  one row per execution — flat columns mirroring the `ExecutionListItemDto`
  fields (id, namespace, flow_id, flow_key, state, created_at, started_at,
  ended_at, duration_ms, labels; exact set pinned in the plan). Streamed via a
  cursor so a large export does not buffer in memory.

## Security

- Every endpoint gates `workflow:read`; none runs a task, so no `egret.sql`
  creator gate and no `workflow:manage`.
- Eval seals secrets on both paths (above).
- Export applies the identical workspace scoping and filter authz as
  `list_executions`.
- SSE resolves and authorizes the execution **before** opening the stream;
  cross-workspace or missing → 404.

## Frontend

1. **Eval panel** — an expression input in the execution view and in the
   task-run view; posts to `1a`/`1b`, shows `result` or `error`.
2. **CSV export button** on the execution list, carrying the current filter into
   `export.csv`.
3. **Average-duration progress** — a progress estimate for a running execution
   from `#6`.
4. **distinct-backed filters** — namespace/flow dropdowns fed by `#5`.
5. **follow** — execution view and log view switch to `EventSource` against
   `#2`/`#3`; the existing `useLogFollow` polling stays as the fallback if the
   SSE connection errors.

## Tests

`crates/egret-workflow/tests/executions_routes.rs` (no migration, no goldens):

- **eval:** a literal expression returns `result`; a broken expression returns
  `error` with 200; `{{ secret('X') }}` is **sealed** on both the execution and
  task-run paths; unknown `taskRunId` → 404; unknown execution → 404.
- **follow execution:** emits an initial snapshot; emits again only on change;
  emits a final event and closes when terminal; unknown/cross-workspace
  execution → 404 before the stream opens.
- **follow logs:** emits appended lines after the watermark; closes when terminal
  and the tail is drained.
- **distinct:** namespaces and per-namespace flows are distinct, workspace-scoped,
  ordered.
- **average-duration:** `null`/`count=0` when no terminated execution in the
  window; count-weighted value when there are; respects the 30-day window.
- **latest:** newest execution per flow; flows with none omitted; empty body →
  empty list.
- **export:** CSV header + one row per matching execution; `Content-Disposition`
  attachment; the filter narrows the rows.

## Kestra divergences (to record in code + §6 of the merged notes)

1. Paths omit `/actions/` (egret convention).
2. Eval seals secrets on **both** paths (Kestra seals only the task-run path).
3. average-duration is a direct `AVG` over a 30-day window, not the
   daily-statistics subsystem (egret has none); the number is equivalent.
4. follow is **poll→SSE** (1s interval), not a reactive event bus; the endpoint
   shape is identical.
5. `#4` follow-dependencies is deferred.

## Open decisions (defaults chosen; confirm in review)

- **Q1 SSE transport:** poll→SSE. ✅ (user)
- **Q2 eval security:** seal both paths, gate `workflow:read`. ✅ (user)
- **Q3 scope:** defer `#4`; ship `#1,2,3,5,6,7,8`. ✅ (user)
- **Q4 poll interval:** 1s for both follow streams (named const). *(default)*
- **Q5 export columns:** flat execution-list columns (above). *(default)*
- **Q6 FE scope:** all five items. ✅ (user, design review)
