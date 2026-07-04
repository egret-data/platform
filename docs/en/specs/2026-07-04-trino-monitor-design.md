# Native Trino monitor page (Phase 1.6b, 2026-07-04)

VI reference: `docs/vi/specs/2026-07-04-trino-monitor-design.md`.
Builds on Phase 1.6a: `docs/en/specs/2026-07-04-trino-auth-ingress-design.md` (shipped, #145).

**Goal:** A native, egret-styled Trino monitoring UI — a **Monitor** tab on the
engine detail page showing the query list, per-query detail with a live
stage/split visualizer, a kill action, and cluster/node stats — served by
egret's control plane calling the coordinator's Trino REST API. egret is the
single pane ("1 cổng") for watching engines; no proxying of Trino's own Web UI.

**Non-goals:** proxying the Trino Web UI; autoscaling (1.5.4); observability
export (1.5.5); pod/scheduling (1.5.3); historical query storage (this reads the
coordinator's in-memory/recent queries only).

## A. Reachability — `KubeClient::coordinator_request`

The coordinator is a ClusterIP Service, reachable differently by environment, so
add one method to the `KubeClient` trait:

```rust
async fn coordinator_request(
    &self, namespace: &str, service: &str, method: &str, path: &str,
    headers: &[(String, String)], body: Option<Vec<u8>>,
) -> anyhow::Result<(u16, Vec<u8>)>;
```

- **In-cluster** (prod): a plain HTTP request to `http://{service}.{namespace}.svc:8080{path}`.
- **Dev / kubeconfig** (egret on the host): via the **kube API-server proxy** —
  `GET/DELETE /api/v1/namespaces/{namespace}/services/{service}:8080/proxy{path}`
  through the kube `Client` (works because egret already has cluster credentials).
- `LiveKube` chooses the path (it knows whether it was built `in_cluster()` vs
  `from_kubeconfig()`); `FakeKube` returns canned responses keyed by path for tests.

## B. Monitor credential

The monitor calls the coordinator's management API, which honours Trino's auth:
- **Engine WITHOUT client auth** (default): call with `X-Trino-User: egret-monitor`.
  With no system access control, that user sees + kills all queries.
- **Engine WITH auth** (Phase 1.6a): the control plane auto-adds a system password
  user `egret-monitor` whose password is **derived deterministically** from the
  engine id (`format!("egret-monitor-{}", engine.id)`) — no storage needed, both
  provisioning (bcrypt → password.db) and the monitor backend re-derive the same
  value. The monitor calls with `Authorization: Basic egret-monitor:<derived>` +
  `X-Forwarded-Proto: https` (Trino refuses password auth on requests it judges
  insecure). Adding this user does NOT enable auth on a no-auth engine — it is
  only injected when the engine already has auth.

## C. Backend — monitor routes + Trino JSON → DTO

Control-plane routes (platform-admin gated), each resolving the engine's namespace
+ `{name}-trino` Service + the monitor credential, then `coordinator_request`:

- `GET /engines/{id}/queries` → Trino `GET /v1/query` (array). Map each to
  `QueryDto { id, user, state, query (SQL), createTime, elapsedMs, cpuMs,
  memoryBytes, progressPercent }` (from `session.user`, `state`, `query`,
  `queryStats.{elapsedTime,totalCpuTime,userMemoryReservation,progressPercentage}`).
- `GET /engines/{id}/queries/{qid}` → Trino `GET /v1/query/{qid}` → a richer
  `QueryDetailDto { ...QueryDto, errorMessage?, outputStage? }` where `outputStage`
  is the raw stage tree (`{stageId, state, stageStats, subStages[], tasks[]}`)
  passed through for the frontend visualizer.
- `DELETE /engines/{id}/queries/{qid}` → Trino `DELETE /v1/query/{qid}` (kill);
  return 200/204.
- `GET /engines/{id}/cluster` → Trino `GET /v1/node` (worker nodes) + a query-state
  tally from `/v1/query` → `ClusterDto { nodes: [{uri, recentFailures}], running,
  queued, blocked, failed }`.

`coordinator_request` non-2xx (or the engine having no running coordinator) → a
`CpError::BadRequest`/`502` the UI surfaces ("engine not reachable").

## D. Frontend — Monitor tab (EngineDetail)

A new **Monitor** tab (alongside Roles/Browse/Connect/Settings-style tabs), styled
after the Trino Web UI:
- **Query list**: a table polled every ~3s (`useQuery(refetchInterval)`) — id, user,
  state (colored badge), SQL preview (truncated), elapsed, memory, CPU,
  progress bar. Filter by state + a text search over user/SQL. Row click → detail.
- **Query detail** (a drawer/section): full SQL, state, error, key stats, a **Kill**
  button (`deleteQuery`, confirm), and a **live stage/split visualizer** — render
  `outputStage` as a stage tree (each stage: state, rows/bytes, its tasks with
  split counts + per-task stats), polled while the query is RUNNING.
- **Cluster panel**: running / queued / blocked counts + a node list (uri, failures).

## E. Test

- **Unit**: the DTO mapping from representative Trino `/v1/query` + `/v1/query/{id}`
  JSON fixtures (a captured RUNNING query with an `outputStage`); `coordinator_request`
  picks the in-cluster URL vs the API-proxy path (via `FakeKube` recording the call);
  the monitor-credential derivation (`egret-monitor-{id}`) + that it's bcrypted into
  password.db when auth is on.
- **Live (kind)**: start a slow-ish query on `demo2` (`SELECT count(*) FROM tpch.sf1.orders`),
  then `GET /engines/{id}/queries` returns it RUNNING, `GET .../queries/{qid}` has an
  `outputStage`, `DELETE .../queries/{qid}` kills it (subsequent state FAILED/CANCELED),
  and `GET .../cluster` lists the worker node.

## Compat
- No DB migration; the monitor credential is derived, not stored. When an engine has
  auth, provisioning adds the `egret-monitor` password entry (idempotent, derived).
- Reads the coordinator's live/recent queries only (Trino keeps a bounded history);
  no egret-side query persistence.
