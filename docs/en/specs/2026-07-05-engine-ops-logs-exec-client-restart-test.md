# Engine Operations — Pod Logs / Exec / Warehouse Client / Restart / Catalog Test — Design

- **Status:** Approved (design)
- **Date:** 2026-07-05
- **Area:** `egret-orchestrator` (kube), `egret-controlplane` (endpoints), control-plane frontend

## 1. Motivation

Operator asks after shipping the config/catalog rework:
1. **Read pod logs** and **exec into a pod** from the engine UI.
2. Egret-warehouse catalogs only let you pick a **warehouse**, never a **client** — add the ability to run a catalog under an existing (governed) client instead of always auto-minting.
3. Audit surfaced two quality-of-life gaps: a manual **Restart/roll** of an engine, and a best-effort **"test catalog connection"**.

## 2. Grounding

- `KubeClient` (`egret-orchestrator/src/kube_client.rs`) has `apply`/`delete`/`list_pods`/`coordinator_request` — **no logs, no exec**. kube crate features: `["client","runtime","derive"]` (**no `ws`** → exec needs it).
- The Monitor already proxies the Trino REST API via `coordinator_request` (`monitor.rs`) — reuse for catalog test (submit a statement).
- Engine catalogs of kind `egret-warehouse` **auto-mint** a per-catalog client (`create_client` + `ensure_engine_role`), stored credential redacted, revoked on remove. Clients are listable (`GET /warehouses/{id}/clients`).
- Frontend has **no terminal lib** — exec needs `@xterm/xterm`.
- Re-provision (`build_provision_req` + `prov.update`) already rolls pods via a config-checksum annotation.

## 3. Design

### 3.1 Pod logs
- **Backend:** `KubeClient::pod_logs(namespace, pod, opts{ container?, tail_lines?, previous? }) -> String` using kube `Api::<Pod>::logs` + `LogParams`. New endpoint `GET /engines/{id}/pods/{pod}/logs?tail=…&previous=…` (platform-admin; verify the pod belongs to the engine by label/namespace). Returns text (a bounded tail, e.g. default 1000 lines; not a live stream in v1).
- **Frontend:** a **Logs** action on each pod row (Overview status table + Monitor) → a dialog/panel showing the tail in a scrollable `<pre>`, a tail-size selector, a "previous container" toggle, and a Refresh button. Read-only.

### 3.2 Pod exec (terminal)
- **Backend:** add `ws` to the kube crate features. `KubeClient::exec` via `Api::<Pod>::exec` + `AttachParams` (tty, stdin/stdout). Expose a **websocket** endpoint `GET /engines/{id}/pods/{pod}/exec` (upgrade) that bridges the browser WS ↔ the kube exec streams (stdin→pod, pod stdout/stderr→browser; forward resize). Platform-admin only; log the session. Default command `/bin/sh`.
- **Frontend:** add `@xterm/xterm` (+ `@xterm/addon-fit`). An **Exec/Terminal** action on a pod row opens a terminal wired to the WS. Confirm-before-open ("interactive shell into <pod>"). This is the security-sensitive piece — gate on platform-admin (same as everything here) and keep it clearly labelled.
- **Scope note:** exec is the largest sub-feature (new dep + WS proxy). It is sequenced last and can ship separately if needed.

### 3.3 Egret-warehouse client selection
- **Backend `add_catalog` (egret-warehouse):** accept an optional `config.clientId` (an existing `CatalogClient` id for that warehouse). If present, validate it belongs to the warehouse and **use its credential** instead of minting; do NOT mint or auto-revoke it (it's user-owned). Store `clientRowId` = chosen id + a flag `mintedClient: false` so remove does NOT revoke a user-owned client. If absent, keep today's auto-mint (`mintedClient: true`, revoke on remove). The credential resolution (`build_catalog_req` egret-warehouse arm) is unchanged — it reads `config.credential`; when using an existing client we still need its `client_secret`. **Constraint:** an existing client's secret may not be retrievable (only shown at creation). If `list_clients`/the client row does not expose a usable secret, then "use existing client" cannot supply a credential → in that case the selection instead **re-mints under the chosen client's role** (uses the chosen client's `catalog_role_id` for a freshly minted client) rather than reusing its secret. The implementer must check what `CatalogClient` exposes and pick the workable path; document which.
- **Frontend (egret-warehouse catalog form):** after the Warehouse select, add a **Client** control: "Create a dedicated client (default)" vs a picker of the warehouse's existing clients (from `GET /warehouses/{id}/clients`). Selecting an existing client sends `config.clientId`.

### 3.4 Manual restart
- **Backend:** `POST /engines/{id}/restart` → re-provision (`build_provision_req` + `prov.update`) so the pods roll (or bump a `restartedAt` annotation to force a roll even with no config change). Platform-admin.
- **Frontend:** a **Restart** button on the engine Overview (with a confirm), showing a rolling state.

### 3.5 Catalog test connection
- **Backend:** `POST /engines/{id}/catalogs/{cid}/test` → via `coordinator_request`, submit `SHOW SCHEMAS FROM "<catalog-name>"` to the Trino REST API (`POST /v1/statement`, follow `nextUri` until done), return `{ ok, message }` (ok if the statement finished without error; message = the Trino error on failure). Platform-admin. Best-effort — some connectors are lazy, but a bad connector config surfaces here.
- **Frontend:** a **Test** button on the catalog detail (and optionally after add) → shows ok/failed + the message.

## 4. Non-goals
- Log **streaming/follow** (v1 is a bounded tail + refresh).
- Exec session recording/audit beyond a log line; no multi-user session sharing.
- Reusing an existing client's *secret* if the platform never stores it (fall back to mint-under-role — see 3.3).
- No new engine types.

## 5. Risks
- **Exec security:** a shell into an engine pod is powerful; gate on platform-admin (already required), confirm-before-open, label clearly. The WS proxy must scope to the engine's namespace/pod (no arbitrary pod exec).
- **Pod/engine ownership:** logs/exec/test endpoints must verify the pod/catalog belongs to the path engine (namespace + label), like `remove_catalog`'s guard — no cross-engine access by id.
- **Client selection secret availability:** confirm whether an existing client's secret is retrievable; if not, mint-under-chosen-role (documented fallback) so the feature still delivers governed access.
- **Catalog test latency:** bound the statement polling (timeout) so a hung coordinator doesn't block the request.

## 6. Acceptance
- Logs: a pod's tail is viewable + refreshable; wrong pod/engine → 404.
- Exec: a terminal opens a shell into a pod; input/output flow; resize works; platform-admin-gated + confirmed.
- Client: an egret-warehouse catalog can run under a chosen existing client (or auto-mint); removing a user-owned-client catalog does NOT revoke that client.
- Restart: the button rolls the pods; status reflects it.
- Catalog test: a good catalog → ok; a broken connector config → failed + Trino error.
- Backend fmt/clippy/test + frontend lint/typecheck/test/build green; live smoke.
