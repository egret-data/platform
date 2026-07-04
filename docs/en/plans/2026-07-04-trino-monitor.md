# Native Trino Monitor Page (Phase 1.6b) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Monitor tab on the engine detail page — query list, per-query detail with a live stage/split visualizer, kill, and cluster/node stats — served by egret calling the coordinator's Trino REST API.

**Architecture:** A three-layer seam mirrors the existing provisioning path. `egret-orchestrator` gains `KubeClient::coordinator_request` (in-cluster: direct Service DNS; dev: kube API-server proxy). `egret-controlplane` stays kube-free: it gets a matching `EngineProvisioner::coordinator_request` trait method, a `monitor` module that maps Trino JSON → egret DTOs, a derived `egret-monitor` credential (injected into `password.db` when auth is on), and the monitor routes. `egret-bin` wires the trait to the KubeClient. The frontend adds a Monitor tab (polled list + cluster, then a live stage visualizer).

**Tech Stack:** Rust (kube-rs `Client` proxy, reqwest, serde_json), axum, React/TS + @tanstack/react-query.

**Spec:** `docs/en/specs/2026-07-04-trino-monitor-design.md`.
**Chart reference:** `/Users/1pro-l4r8/Documents/github/data-portal/trino-charts/charts/trino`.

---

## File Structure

- **Modify** `crates/egret-orchestrator/src/kube_client.rs` — `coordinator_request` on the `KubeClient` trait; `LiveKube` (DNS + API-proxy, needs an `in_cluster: bool`) + `FakeKube` (canned) impls.
- **Create** `crates/egret-controlplane/src/monitor.rs` — DTOs (`QueryDto`, `QueryDetailDto`, `ClusterDto`) + Trino-JSON→DTO pure mappers + the monitor routes.
- **Modify** `crates/egret-controlplane/src/engines.rs` — `EngineProvisioner::coordinator_request` trait method; `monitor_password()` + inject `egret-monitor` into `resolve_auth` when auth is on; mount the monitor routes.
- **Modify** `crates/egret-bin/src/main.rs` — impl `coordinator_request` (delegate to the KubeClient).
- **Modify** `frontend/src/api/controlPlane.ts` + `EngineDetail.tsx`; **Create** `frontend/src/routes/control-plane/EngineMonitor.tsx` (Monitor tab) + `StageTree.tsx` (visualizer).

---

## Task 1: `KubeClient::coordinator_request`

**Files:** Modify `crates/egret-orchestrator/src/kube_client.rs`

- [ ] **Step 1: Failing test** (FakeKube records the call; add a `coordinator_calls` Vec + a `coordinator_response: (u16, Vec<u8>)` to FakeKube):

```rust
#[tokio::test]
async fn fake_kube_coordinator_request_records() {
    let k = FakeKube::default();
    k.set_coordinator_response(200, br#"[{"queryId":"x"}]"#.to_vec());
    let (code, body) = k.coordinator_request("egret-engines", "demo-trino", "GET", "/v1/query", &[("X-Trino-User".into(),"egret-monitor".into())], None).await.unwrap();
    assert_eq!(code, 200);
    assert!(String::from_utf8_lossy(&body).contains("queryId"));
    assert_eq!(k.coordinator_calls.lock().unwrap()[0], ("GET".to_string(), "/v1/query".to_string()));
}
```

- [ ] **Step 2** Run → FAIL (method missing).

- [ ] **Step 3: Trait + FakeKube**

Add to the `KubeClient` trait:

```rust
    async fn coordinator_request(
        &self, namespace: &str, service: &str, method: &str, path: &str,
        headers: &[(String, String)], body: Option<Vec<u8>>,
    ) -> anyhow::Result<(u16, Vec<u8>)>;
```

FakeKube: store `coordinator_calls: std::sync::Mutex<Vec<(String,String)>>` + a settable `coordinator_response`; the impl pushes `(method, path)` and returns the canned response.

- [ ] **Step 4: LiveKube impl** — LiveKube needs to know its environment. Add an `in_cluster: bool` field; set `true` in `in_cluster()`, `false` in `from_kubeconfig()`.

```rust
    async fn coordinator_request(&self, namespace: &str, service: &str, method: &str, path: &str, headers: &[(String,String)], body: Option<Vec<u8>>) -> anyhow::Result<(u16, Vec<u8>)> {
        if self.in_cluster {
            // Direct Service DNS (reqwest).
            let url = format!("http://{service}.{namespace}.svc:8080{path}");
            let mut rb = reqwest::Client::new().request(method.parse()?, &url);
            for (k, v) in headers { rb = rb.header(k, v); }
            if let Some(b) = body { rb = rb.body(b); }
            let resp = rb.send().await?;
            Ok((resp.status().as_u16(), resp.bytes().await?.to_vec()))
        } else {
            // kube API-server proxy subresource.
            let uri = format!("/api/v1/namespaces/{namespace}/services/{service}:8080/proxy{path}");
            let mut req = http::Request::builder().method(method).uri(uri);
            for (k, v) in headers { req = req.header(k.as_str(), v.as_str()); }
            let req = req.body(body.unwrap_or_default())?;
            let resp = self.client.send(req).await?;   // self.client: kube::Client
            let status = resp.status().as_u16();
            let bytes = http_body_util::BodyExt::collect(resp.into_body()).await?.to_bytes().to_vec();
            Ok((status, bytes))
        }
    }
```

Add `reqwest` + `http` + `http-body-util` to `crates/egret-orchestrator/Cargo.toml` if absent (check the workspace-resolved versions; `cargo add` each). If `kube::Client::send` signature differs in the pinned kube version, adapt (the goal: send a raw `http::Request<Vec<u8>>` to the proxy subresource and read the status + body).

- [ ] **Step 5** `cargo test -p egret-orchestrator` + clippy clean; `cargo fmt --all`. Commit `feat(orchestrator): KubeClient::coordinator_request (Service DNS + API proxy)`.

---

## Task 2: `EngineProvisioner::coordinator_request` seam + egret-bin impl

**Files:** Modify `crates/egret-controlplane/src/engines.rs`, `crates/egret-bin/src/main.rs`

- [ ] **Step 1** Add to the `EngineProvisioner` trait (in engines.rs) the SAME signature (kube-free — returns `anyhow::Result<(u16, Vec<u8>)>`):

```rust
    async fn coordinator_request(&self, namespace: &str, service: &str, method: &str, path: &str, headers: &[(String,String)], body: Option<Vec<u8>>) -> anyhow::Result<(u16, Vec<u8>)>;
```

- [ ] **Step 2** In egret-bin, the `EngineProvisioner` impl (the one wrapping `TrinoProvisioner`/the KubeClient) delegates: `self.kube.coordinator_request(namespace, service, method, path, headers, body).await` (reach the KubeClient the provisioner already holds).
- [ ] **Step 3** Any test double for `EngineProvisioner` in egret-controlplane tests gets a stub `coordinator_request` returning a canned value (find the existing fake provisioner; add the method).
- [ ] **Step 4** `cargo build -p egret-bin` + `cargo test -p egret-controlplane` compile. `cargo fmt --all`. Commit `feat(controlplane): coordinator_request provisioner seam`.

---

## Task 3: Monitor DTOs + Trino JSON → DTO mapping

**Files:** Create `crates/egret-controlplane/src/monitor.rs`; Modify `lib.rs` (add `pub mod monitor;`)

- [ ] **Step 1: Failing tests** (pure mappers over Trino JSON fixtures — times/sizes pass through as raw Trino strings; `outputStage` passes through as raw `Value`):

```rust
use serde_json::json;
#[test]
fn map_query_list_item() {
    let v = json!({"queryId":"20260704_x","state":"RUNNING","query":"SELECT 1",
        "session":{"user":"bob"},
        "queryStats":{"elapsedTime":"1.23s","totalCpuTime":"0.50s","userMemoryReservation":"12MB","progressPercentage":42.0}});
    let d = query_dto(&v);
    assert_eq!(d.id, "20260704_x");
    assert_eq!(d.user, "bob");
    assert_eq!(d.state, "RUNNING");
    assert_eq!(d.query, "SELECT 1");
    assert_eq!(d.elapsed, "1.23s");
    assert_eq!(d.memory, "12MB");
    assert_eq!(d.progress_percent, 42.0);
}
#[test]
fn map_query_detail_passes_output_stage_and_error() {
    let v = json!({"queryId":"q","state":"FAILED","query":"SELECT 1","session":{"user":"u"},
        "queryStats":{"elapsedTime":"1s","totalCpuTime":"1s","userMemoryReservation":"0B","progressPercentage":100.0},
        "failureInfo":{"message":"boom"},
        "outputStage":{"stageId":"0","state":"FINISHED","subStages":[]}});
    let d = query_detail_dto(&v);
    assert_eq!(d.base.state, "FAILED");
    assert_eq!(d.error_message.as_deref(), Some("boom"));
    assert_eq!(d.output_stage.as_ref().unwrap()["stageId"], "0");
}
#[test]
fn cluster_tally_counts_states() {
    let queries = json!([{"state":"RUNNING"},{"state":"RUNNING"},{"state":"QUEUED"},{"state":"FINISHED"}]);
    let nodes = json!([{"uri":"http://w1:8080","recentFailures":0.0}]);
    let c = cluster_dto(&queries, &nodes);
    assert_eq!(c.running, 2);
    assert_eq!(c.queued, 1);
    assert_eq!(c.nodes.len(), 1);
}
```

- [ ] **Step 2** Run → FAIL.

- [ ] **Step 3: Implement** the DTOs + mappers (all `#[derive(Serialize)]`, camelCase via `#[serde(rename_all="camelCase")]`):

```rust
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct QueryDto {
    pub id: String, pub user: String, pub state: String, pub query: String,
    pub elapsed: String, pub cpu: String, pub memory: String, pub progress_percent: f64,
}
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct QueryDetailDto {
    #[serde(flatten)] pub base: QueryDto,
    pub error_message: Option<String>,
    pub output_stage: Option<Value>,
}
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NodeDto { pub uri: String, pub recent_failures: f64 }
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ClusterDto { pub running: u32, pub queued: u32, pub blocked: u32, pub nodes: Vec<NodeDto> }

fn s(v: &Value, p: &str) -> String { v.get(p).and_then(|x| x.as_str()).unwrap_or("").to_string() }

pub fn query_dto(v: &Value) -> QueryDto {
    let qs = &v["queryStats"];
    QueryDto {
        id: s(v, "queryId"),
        user: v["session"]["user"].as_str().unwrap_or("").to_string(),
        state: s(v, "state"),
        query: s(v, "query"),
        elapsed: qs["elapsedTime"].as_str().unwrap_or("").to_string(),
        cpu: qs["totalCpuTime"].as_str().unwrap_or("").to_string(),
        memory: qs["userMemoryReservation"].as_str().unwrap_or("").to_string(),
        progress_percent: qs["progressPercentage"].as_f64().unwrap_or(0.0),
    }
}
pub fn query_detail_dto(v: &Value) -> QueryDetailDto {
    QueryDetailDto {
        base: query_dto(v),
        error_message: v["failureInfo"]["message"].as_str().map(|s| s.to_string()),
        output_stage: v.get("outputStage").filter(|s| !s.is_null()).cloned(),
    }
}
pub fn cluster_dto(queries: &Value, nodes: &Value) -> ClusterDto {
    let count = |st: &str| queries.as_array().map(|a| a.iter().filter(|q| q["state"] == st).count() as u32).unwrap_or(0);
    ClusterDto {
        running: count("RUNNING"), queued: count("QUEUED"), blocked: count("BLOCKED"),
        nodes: nodes.as_array().map(|a| a.iter().map(|n| NodeDto {
            uri: n["uri"].as_str().unwrap_or("").to_string(),
            recent_failures: n["recentFailures"].as_f64().unwrap_or(0.0),
        }).collect()).unwrap_or_default(),
    }
}
```

- [ ] **Step 4** `cargo test -p egret-controlplane monitor::` passes + clippy + fmt. Commit `feat(controlplane): Trino monitor DTOs + JSON mapping`.

---

## Task 4: Monitor credential — derive + inject `egret-monitor` when auth on

**Files:** Modify `crates/egret-controlplane/src/engines.rs`

- [ ] **Step 1: Failing test**

```rust
#[test]
fn monitor_password_is_derived_from_id() {
    let id = uuid::Uuid::nil();
    assert_eq!(monitor_password(id), "egret-monitor-00000000-0000-0000-0000-000000000000");
}
```

- [ ] **Step 2** Run → FAIL.

- [ ] **Step 3: Implement** `pub fn monitor_password(id: Uuid) -> String { format!("egret-monitor-{id}") }`. Then in `resolve_auth`: when auth IS enabled (the block that builds `AuthReq` for an engine with password/jwt), append a `PasswordEntryReq { username: "egret-monitor", bcrypt_hash: <bcrypt($2y$) of monitor_password(engine_id)> }` to `password_users` (so `password.db` carries it). `resolve_auth` currently may not have the engine id — thread it in (pass `engine.id` to `resolve_auth`, or move the injection to `build_provision_req` right after `resolve_auth`, where `engine.id` is in scope). Use the same `$2b$`→`$2y$` relabel as the human users.
- [ ] **Step 4** Extend an existing auth test (or add one) asserting that when an engine has a password user + auth, the provision req's `password_users` also contains `egret-monitor` with a `$2y$` hash that `bcrypt::verify(monitor_password(id), ..)` accepts. Run → pass.
- [ ] **Step 5** clippy + fmt. Commit `feat(controlplane): derived egret-monitor credential in password.db`.

---

## Task 5: Monitor routes

**Files:** Modify `crates/egret-controlplane/src/monitor.rs` (routes) + `engines.rs` (mount) ; Test `tests/` (route test if a fake provisioner exists)

- [ ] **Step 1: Implement the routes** — a `pub fn monitor_routes() -> Router<CpState>` (or fold into the engines router) with:
  - `GET /engines/:id/queries` → resolve the engine (namespace + `{name}-trino`) + `monitor_headers(&engine)`; `st.provisioner.coordinator_request(ns, svc, "GET", "/v1/query", &headers, None)`; on 2xx parse the JSON array → `Vec<QueryDto>` via `query_dto`; non-2xx → `CpError::BadGateway`/`502` "engine not reachable".
  - `GET /engines/:id/queries/:qid` → `/v1/query/{qid}` → `query_detail_dto`.
  - `DELETE /engines/:id/queries/:qid` → `coordinator_request(.., "DELETE", "/v1/query/{qid}", ..)` → return 200 on 2xx.
  - `GET /engines/:id/cluster` → two calls (`/v1/node` + `/v1/query`) → `cluster_dto`.
  - `monitor_headers(engine)`: if the engine spec has auth enabled → `[("Authorization", format!("Basic {}", base64("egret-monitor:{monitor_password(id)}"))), ("X-Forwarded-Proto","https")]`; else → `[("X-Trino-User","egret-monitor")]`.
  All routes `require_platform_admin` (mirror the other engine routes).
- [ ] **Step 2** Add a `CpError::BadGateway` variant (or reuse an existing 502/BadRequest) mapping to `StatusCode::BAD_GATEWAY` with the message.
- [ ] **Step 3** Mount the routes onto the engines router in `engines.rs`.
- [ ] **Step 4** If the control-plane tests have a fake `EngineProvisioner`, add a route test: stub `coordinator_request` to return a `/v1/query` fixture → `GET /engines/{id}/queries` returns the mapped list (200); a 503 from the stub → the route returns 502. (If no fake provisioner exists in tests, note it and rely on Task 8's live test.)
- [ ] **Step 5** `cargo test -p egret-controlplane` + clippy + fmt. Commit `feat(controlplane): Trino monitor routes (queries/detail/kill/cluster)`.

---

## Task 6: egret-bin wiring check + Frontend api + Monitor tab (list + cluster + kill)

**Files:** Modify `frontend/src/api/controlPlane.ts`, `EngineDetail.tsx`; Create `frontend/src/routes/control-plane/EngineMonitor.tsx`

- [ ] **Step 1** Confirm the whole workspace builds (`cargo build --workspace 2>&1 | grep ^error` empty) — Task 2's egret-bin impl + Task 5's routes are now all present.
- [ ] **Step 2** TS api: types `MonitorQuery { id; user; state; query; elapsed; cpu; memory; progressPercent }`, `MonitorQueryDetail = MonitorQuery & { errorMessage?; outputStage?: unknown }`, `ClusterInfo { running; queued; blocked; nodes: {uri; recentFailures}[] }`. Functions `listQueries(engineId)`, `getQuery(engineId, qid)`, `killQuery(engineId, qid)`, `getCluster(engineId)`. `tsc`.
- [ ] **Step 3** `EngineMonitor.tsx`: a **Monitor** section — a `useQuery(['queries', engineId], , {refetchInterval: 3000})` table (id, user, state colored `<Badge>`, SQL preview truncated, elapsed, memory, cpu, a progress bar from `progressPercent`), a state filter + a text search; a cluster summary strip (`getCluster`, running/queued/blocked + node count). Row click sets a selected `qid` (detail comes in Task 7). A **Kill** button per running row (`killQuery` mutation + confirm + invalidate).
- [ ] **Step 4** Add a **Monitor** tab to `EngineDetail.tsx` rendering `<EngineMonitor engineId={id} />`.
- [ ] **Step 5** `tsc` / `eslint` / `vite build` clean. Commit `feat(ui): Trino Monitor tab — query list, cluster, kill`.

---

## Task 7: Frontend live stage/split visualizer

**Files:** Create `frontend/src/routes/control-plane/StageTree.tsx`; Modify `EngineMonitor.tsx`

- [ ] **Step 1** Query detail: when a `qid` is selected, `useQuery(['query', engineId, qid], () => getQuery(...), {refetchInterval: isRunning ? 2000 : false})`. Render full SQL, state, `errorMessage`, and key stats.
- [ ] **Step 2** `StageTree.tsx`: a recursive component over `outputStage` — each stage shows `stageId`, `state` (badge), the stage's row/byte stats (from `stageStats`), and its `tasks[]` (count + a compact per-task split/stats line); recurse into `subStages[]`. Guard all fields (Trino stage JSON is deep + partially optional). Render as an indented tree.
- [ ] **Step 3** Wire `StageTree` into the detail panel (only when `outputStage` is present). A **Kill** button in the detail too.
- [ ] **Step 4** `tsc` / `eslint` / `vite build` clean; keep control-plane vitest green. Commit `feat(ui): live Trino stage/split visualizer`.

---

## Task 8: Full gate + live test + PR

- [ ] **Step 1** `cargo fmt --all --check`, `cargo clippy --workspace --all-targets`, `cargo test --workspace`.
- [ ] **Step 2** Frontend `tsc` / `eslint` / `vitest run` / `vite build`.
- [ ] **Step 3** Live on kind (egret on host → dev API-proxy path): rebuild egret-bin, restart serve. On `demo2` start a slow query: `POST /v1/statement` (through the coordinator) `SELECT count(*) FROM tpch.sf1.orders` (sf1 ⇒ runs a few seconds). Then hit egret: `GET /api/control-plane/engines/{id}/queries` shows it `RUNNING`; `GET .../queries/{qid}` has a non-null `outputStage`; `GET .../cluster` lists the worker node + running≥1; `DELETE .../queries/{qid}` → the query's next state is `FAILED`/`CANCELED`. Drive the Monitor tab in the browser (Playwright): the query appears, the stage tree renders, Kill works.
- [ ] **Step 4** Commit; open the PR; run the full local gate before an admin squash-merge.

---

## Self-Review notes

- **Spec coverage:** reachability dual-path (Task 1) ✓; provisioner seam (Task 2) ✓; DTO mapping incl. outputStage pass-through (Task 3) ✓; derived monitor credential + password.db inject (Task 4) ✓; routes list/detail/kill/cluster + 502 + credential selection (Task 5) ✓; Monitor tab list/cluster/kill (Task 6) ✓; live stage visualizer (Task 7) ✓; live verify RUNNING/outputStage/kill/cluster (Task 8) ✓.
- **Type consistency:** `QueryDto`/`QueryDetailDto`/`ClusterDto`/`NodeDto` (rust, camelCase) ↔ TS `MonitorQuery`/`MonitorQueryDetail`/`ClusterInfo`. `coordinator_request` identical signature across `KubeClient` (orchestrator) + `EngineProvisioner` (control-plane) + egret-bin impl. `monitor_password(id)` used in both Task 4 (bcrypt into password.db) and Task 5 (Basic auth header) — same format.
- **Times/sizes** pass through as raw Trino strings ("1.23s", "12MB") — no Duration/DataSize parsing; the frontend displays them verbatim. `outputStage` passes through as raw JSON to the visualizer.
- **Credential:** no storage — derived from the engine id both when hashing into `password.db` and when building the monitor Basic header. Only injected when the engine already has auth; no-auth engines use `X-Trino-User`.
- **Deferred:** autoscaling (1.5.4); observability export (1.5.5); pod/scheduling (1.5.3); query persistence.
