# Engine Operations — Logs / Exec / Warehouse Client / Restart / Catalog Test — Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Add pod logs viewing, pod exec (terminal), egret-warehouse client selection, a manual engine restart, and a catalog "test connection".

**Architecture:** New `KubeClient` methods (`pod_logs`; `exec` behind the kube `ws` feature) + control-plane endpoints (REST for logs/restart/test/client; a websocket proxy for exec). Frontend: a logs viewer, an xterm terminal, a client picker in the catalog form, a Restart button, a Test button.

**Branch:** new `feat/engine-ops` (off current `main`). **Spec:** `docs/en/specs/2026-07-05-engine-ops-logs-exec-client-restart-test.md`.

**Sequencing:** logs → client selection → restart → catalog test → **exec last** (largest; new dep + WS). Ship-able incrementally.

---

## Conventions
- Backend from repo root; frontend from `frontend/`. Gates: backend `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p egret-controlplane -p egret-orchestrator`; frontend `npm run typecheck && npm run lint && npm run test -- --run`.
- Conventional Commits, repo identity, one commit per task.
- **Guardrails:** every pod/catalog endpoint verifies the target belongs to the path engine (namespace + `egret/engine` label / `engine_id`), mirroring `remove_catalog`. All are platform-admin. Provisioning behavior for existing engines is unchanged except the new restart/test/client paths.

---

## Task 1: Backend — pod logs
**Files:** `egret-orchestrator/src/kube_client.rs` (trait + real impl + FakeKube), `egret-controlplane/src/engines.rs` (or `monitor.rs`) endpoint + route.

- [ ] **Step 1:** Add to `KubeClient`: `async fn pod_logs(&self, namespace: &str, pod: &str, container: Option<&str>, tail_lines: i64, previous: bool) -> anyhow::Result<String>`. Real impl: `Api::<Pod>::namespaced(client, ns).logs(pod, &LogParams{ container, tail_lines: Some(tail_lines), previous, ..Default::default() })` (confirm the exact kube 0.99 `LogParams` fields + `k8s_openapi` Pod import). FakeKube: return a canned string.
- [ ] **Step 2:** Endpoint `GET /engines/{id}/pods/{pod}/logs` (query `tail` default 1000 capped at e.g. 5000, `previous` bool, `container` optional): platform-admin; load engine (404); verify the pod belongs to the engine (call `list_pods` and check `pod` is in it, else 404); call `prov`/kube `pod_logs` in the engine namespace; return `text/plain` (or `{ logs: String }` JSON). Wire the route.
- [ ] **Step 3:** Test (FakeKube-based unit test of the handler wiring, or a self-skip integration test) + `cargo build/clippy/fmt`. Commit `feat(engine): pod logs endpoint`.

## Task 2: Frontend — logs viewer
**Files:** `controlPlane.ts` (`getPodLogs(id, pod, {tail, previous, container})`), `EngineDetail.tsx` (StatusCard pod rows) or a new `PodLogsDialog.tsx`.
- [ ] Add the API call + a **Logs** button per pod row → a Dialog with a scrollable `<pre>` of the tail, a tail-size Select (200/1000/5000), a "previous container" toggle, and Refresh. Loading/error states. `npm run` gate. Commit `feat(engine-ui): pod logs viewer`.

## Task 3: Backend — egret-warehouse client selection
**Files:** `egret-controlplane/src/engines.rs` (`add_catalog` egret-warehouse branch ~1820-1868).
- [ ] **Step 1:** Read `CatalogClient` (`egret-catalog/src/service/embedded.rs`) — determine whether an existing client's **secret** is retrievable. Pick the path (reuse secret if available; else mint a fresh client under the chosen client's `catalog_role_id`). Document which in the code + report.
- [ ] **Step 2:** In `add_catalog` egret-warehouse: if `b.config["clientId"]` is a valid Uuid belonging to the warehouse, use the chosen-client path (no auto-mint of a NEW role/client, or mint-under-its-role per Step 1); store `clientRowId` + `mintedClient: false`. Else keep today's auto-mint (`mintedClient: true`). In `remove_catalog`, only `revoke_client` when `mintedClient != false` (don't revoke a user-owned client). `redact_catalog_config` already strips `clientRowId`/`credential` — also don't leak `mintedClient` concerns (it's fine to keep, it's not sensitive).
- [ ] **Step 3:** Tests (self-skip): choosing a client stores it and doesn't mint; removing it doesn't revoke. `cargo` gate. Commit `feat(engine-catalog): egret-warehouse client selection`.

## Task 4: Frontend — client picker
**Files:** `CatalogForm.tsx` (egret-warehouse add branch), `controlPlane.ts` (`listWarehouseClients(warehouseId)` if not present — check `warehouses.rs` route `/warehouses/{id}/clients`).
- [ ] After the Warehouse Select (add mode), add a **Client** Select: "Create a dedicated client (default)" (value `""`) + one item per the warehouse's clients (from the clients API, enabled once a warehouse is chosen). Selecting a client id adds `clientId` to the submitted `config`. `npm run` gate. Commit `feat(engine-ui): warehouse client picker`.

## Task 5: Backend — manual restart
**Files:** `egret-controlplane/src/engines.rs`.
- [ ] `POST /engines/{id}/restart`: platform-admin; load engine; `build_provision_req` + `prov.update` (rolls via checksum). If a no-config-change roll is needed, ALSO set a `restartedAt` timestamp annotation on the pod template so the checksum changes — simplest: the provisioner's `update` already re-applies; if that alone doesn't force a roll without config change, add a restart annotation in the manifest (coordinate with `trino.rs`'s pod-template annotations). Return 200. Route it. Test + gate. Commit `feat(engine): manual restart endpoint`.

## Task 6: Frontend — restart button
**Files:** `EngineDetail.tsx` (Overview / StatusCard), `controlPlane.ts` (`restartEngine(id)`).
- [ ] A **Restart** button (confirm dialog) that calls the endpoint, shows a pending state, invalidates the engine query. `npm run` gate. Commit `feat(engine-ui): restart button`.

## Task 7: Backend — catalog test connection
**Files:** `egret-controlplane/src/engines.rs` (reuse the `coordinator_request` proxy pattern from `monitor.rs`).
- [ ] `POST /engines/{id}/catalogs/{cid}/test`: platform-admin; load engine + catalog (verify ownership); via `prov.coordinator_request`, submit `SHOW SCHEMAS FROM "<cat.name>"` to Trino REST (`POST /v1/statement` with header `X-Trino-User: admin`, then follow `nextUri` GETs until `state` is `FINISHED`/`FAILED`, bounded by a timeout / max iterations). Return `{ ok: bool, message: String }` (ok on FINISHED; message = the `error.message` on FAILED). Read `monitor.rs` for the exact coordinator-proxy call + service name. Test (FakeKube canned coordinator_response) + gate. Commit `feat(engine-catalog): test connection endpoint`.

## Task 8: Frontend — catalog test UI
**Files:** `EngineDetail.tsx` catalog detail pane, `controlPlane.ts` (`testEngineCatalog(id, cid)`).
- [ ] A **Test connection** button on the catalog detail (non-warehouse; maybe all) → shows a spinner then an ok/failed alert with the message. `npm run` gate. Commit `feat(engine-ui): catalog test connection`.

## Task 9: Backend — pod exec (websocket)
**Files:** `Cargo.toml` (kube `ws` feature), `egret-orchestrator/src/kube_client.rs` (`exec`), `egret-controlplane/src/engines.rs` (WS route).
- [ ] **Step 1:** Add `ws` to the kube feature list in the workspace `Cargo.toml`. `KubeClient::exec(namespace, pod, cmd: &[String]) -> ` an attached-process handle exposing stdin sink + stdout/stderr streams + resize (via `Api::<Pod>::exec` + `AttachParams{ tty:true, stdin:true, stdout:true, stderr:false, ..}` + `AttachedProcess`). Confirm the kube 0.99 `AttachedProcess` API (stdin()/stdout()/terminal_size()). FakeKube: a stub that errors "exec unsupported in tests" (exec isn't unit-tested; it's smoke-tested live).
- [ ] **Step 2:** WS endpoint `GET /engines/{id}/pods/{pod}/exec` (axum `WebSocketUpgrade`): platform-admin; verify pod ownership; open the kube exec; bridge browser WS ↔ exec (browser text/binary → stdin; stdout → browser; a small JSON control message `{resize:{cols,rows}}` → `terminal_size`). Close cleanly. Log a session line (engine, pod, user). Route it.
- [ ] **Step 3:** `cargo build/clippy/fmt` (workspace, since Cargo.toml changed). Commit `feat(engine): pod exec websocket`.

## Task 10: Frontend — terminal (xterm)
**Files:** `frontend/package.json` (`@xterm/xterm` + `@xterm/addon-fit`), a new `PodTerminalDialog.tsx`, `EngineDetail.tsx` pod rows.
- [ ] Add the deps (pin versions; inline CSS import). An **Exec** button per pod row → a confirm ("Open an interactive shell into <pod>?") → a Dialog hosting an xterm `Terminal` wired to a `WebSocket` at the exec endpoint (send keystrokes as data; write incoming data to the terminal; send resize on fit). Handle open/close/error. `npm run typecheck && npm run lint && npm run build`. Commit `feat(engine-ui): pod exec terminal`.

## Task 11: Verify + live smoke
- [ ] Backend workspace fmt/clippy/test; rebuild `egret-bin`; restart serve. Frontend full gate. Live smoke on `demo2`: view a pod's logs; Restart the engine (pods roll); test a catalog (ok + a deliberately-broken one → failed); egret-warehouse add with a chosen client; open an exec terminal into a pod and run `ls`. 0 console errors.

---

## Self-review
- **Coverage:** logs (T1-2), client (T3-4), restart (T5-6), test (T7-8), exec (T9-10), verify (T11).
- **Guardrails:** every endpoint verifies pod/catalog↔engine ownership + platform-admin; user-owned clients not revoked; exec confirm-gated.
- **Sequencing:** exec (new dep + WS) is last and independently shippable.
