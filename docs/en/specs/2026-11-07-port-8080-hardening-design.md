# Port 8080 hardening: Trino identity over plain HTTP, and a dedicated OPA listener

**Status:** designed 2026-11-07 and approved section by section by the user. This is the prerequisite for governance SP2b Task 26, which turns `[governance.opa] enabled` on by default. Two code PRs (A, then B), each with a docs PR in `egret-data/docs`.

**Sources:**
- `egret-sp2b/.superpowers/sdd/2026-11-04-gov-sp2b-opa-target/final-review-2b2.md`: §"Task 26 readiness" items 1 (I2) and 6, and `task-21-report.md` finding 2 (both git-ignored).
- `docs/en/specs/2026-07-15-delegated-jwt-tls-mode-design.md`, which closed the spoofing surface for jwt engines only.
- `docs/en/specs/2026-07-04-trino-auth-ingress-design.md`.

## 1. Problem

Two defects share one root: port 8080 is reachable by more peers than it should trust.

### A. Any peer that reaches a Trino coordinator's port 8080 can be any user

- egret renders `http-server.authentication.allow-insecure-over-http=true` for every engine except jwt-mode over TLS (`crates/egret-provisioner/src/trino.rs:147`).
- With that flag, Trino does not authenticate a request that arrives over plain HTTP. The user named in the request (`X-Trino-User`, or the CLI's `--user`/`--session-user`) is taken as is.
- This was **measured** in SP2b Task 21 on a provisioned impersonate-mode Trino 483 with `"auth":{"password":{}}`, on Ranger and on OPA alike. `trino --user ann --session-user bob`, sent with no password, ran as `bob`, and no impersonation check fired.
- The comment at `trino.rs:144-146` ("safe because impersonation is delegator-only") and the same claim in the 2026-07-15 spec are wrong: no credential is involved at all.
- **Engines with no `trinoConfig.auth` block** have no authenticator and no `password.db`, so the same holds for them regardless of the flag.
- **Who reaches 8080** (`deploy/helm/egret/templates/networkpolicy-engines.yaml`):
  - every pod in the engine namespace;
  - every egret pod (`app: egret`), on all ports;
  - `ingressFrom` peers, which includes the ingress controller when an engine is exposed;
  - `extraFrom` peers.
- Under OPA, every decision trusts the user Trino forwards. Advertising OPA as the default path while any of these peers can claim any identity is not acceptable (user decision, 2026-11-07).

### B. The `/opa/` surface shares egret's API port with app and task pods

- The OPA routes (`crates/egret-perm-compile/src/opa/surface.rs:338-353`) are merged into the main router (`crates/egret-server/src/lib.rs:334-336`), which listens on `server.port` (8080).
- **The surface has to read the body before it can authenticate.** The engine credential is in the request body (`input.context.properties`, from the plugin's `opa.context-file`), and the Trino plugin cannot be told to send it elsewhere.
- **Its budget is global.** It has 64 in-flight units of 4 MiB and 16 parse units (`surface.rs:99-116`), shared by every peer.
- **Who reaches egret's 8080.** No NetworkPolicy in the chart selects egret's pod for ingress. `networkpolicy-apps.yaml` explicitly lets app pods reach `egret:8080`, and `networkpolicy-tasks.yaml` lets task pods reach the whole release namespace.
- **The attack.** A peer that opens connections, declares a large `Content-Length` and stalls holds units for up to `body_read_timeout_ms`. 64 such connections starve every engine: they get 503, and Trino fails the query.

A per-peer limit was rejected:
- many pods each have their own address;
- behind an ingress every request carries one address;
- `serve_with_drain` does not provide `ConnectInfo` today.

## 2. Decisions

### 2.1 User decisions, 2026-11-07

- **Q1. Every provisioned Trino engine gets authentication**, including engines saved without an auth block. No engine is left with no authenticator.
- **Q2. `/opa/` moves to its own listener with a NetworkPolicy** that admits only engines. No per-peer limit.
- **Q3. Hard move with automatic re-apply.** 8080 stops serving `/opa/`. egret re-applies provisioned engines on OPA profiles whose applied OPA URL is stale. External engines re-install the plugin files, and the upgrade note says so.
- **Out of scope:** StarRocks. FE port 9030 accepts `root` with no password when `root_password` is unset. This is recorded as a follow-up next to S1.

### 2.2 Design decisions

**Part A: Trino identity**

- **A1. Authentication is always on.** The engine apply path treats a Trino spec without `trinoConfig.auth` as `"auth": {"password": {}}`.
  - The delegator (impersonate mode) and `egret-monitor` are injected as today (`crates/egret-engine-mgmt/src/engines.rs:743-794`).
  - The stored spec is normalised the same way, so the console shows the block. A request that removes it is normalised back, not refused.
  - An existing engine gains it, and the new flag (A2), on its next apply. Migration **0215** adds `engine_mgmt.engines.applied_security_rev smallint NOT NULL DEFAULT 0`; every successful apply of a Trino engine records the current revision (`TRINO_SECURITY_REV = 1`). The engine API carries `security_reapply_needed` (a provisioned Trino engine whose recorded revision is below the current one), the engine list shows it, and the PR A upgrade note tells operators to re-apply those engines.
- **A2. Insecure HTTP is off on every engine.** `allow-insecure-over-http=false` is rendered whenever an authenticator is configured, which after A1 means always. Requests then behave as follows:
  - A plain-HTTP request to 8080 without `X-Forwarded-Proto: https` is refused, except for Trino's public resources.
  - With `process-forwarded=true`, which is unchanged, a request carrying `X-Forwarded-Proto: https` is treated as secure, so it must carry a valid password or JWT. The header alone does not let a peer claim a user.
  - Coordinator↔worker traffic is authenticated by `internal-communication.shared-secret` and is not subject to the flag. jwt engines already run this way.
- **A3. egret's own callers keep working:**
  - the monitor (Basic `egret-monitor` plus `X-Forwarded-Proto: https`, `crates/egret-engine-mgmt/src/monitor.rs:166-199`);
  - the delegated query path (HTTPS on 8443, `assignment.rs:196-216`);
  - ingress clients, whose `X-Forwarded-Proto` the ingress controller adds;
  - the readiness probe (`GET /v1/info`, `trino.rs:594-619`);
  - the `preStop` hook (`trino.rs:923`).

  The probe and `preStop` are verified live (A4). If either is refused, it gets the monitor credential.
- **A4. Live check before code.** On a Trino 483 provisioned by egret on its own compose stack, with one engine on a Ranger profile and one on an OPA profile, before and after the flag:
  - `trino --user ann --session-user bob` over plain HTTP runs as bob before, and is refused after;
  - `curl -H 'X-Forwarded-Proto: https' -H 'X-Trino-User: bob'` with no credential is refused after;
  - the monitor, the probe, `preStop`, worker registration and a delegated query all work after.

  Results go in `docs/en/plans/2026-11-07-port-8080-hardening-live-result.md`.
- **A5.** Fix the `trino.rs:144-146` comment, and add an "as corrected" note to `2026-07-15-delegated-jwt-tls-mode-design.md`.

**Part B: the OPA listener**

- **B1. Own listener.**
  - When `[governance.opa] enabled` is on, egret binds `[governance.opa] listen` (new key, default `0.0.0.0:8082`) and serves only the OPA router there. It uses a second `serve_with_drain`, as the apps listener on 8081 does (`crates/egret-bin/src/main.rs:3635-3655`).
  - The OPA router is no longer merged into `build_router`, so 8080 answers 404 for `/opa/...`.
  - `/ranger/`, the machine-IdP routes and the Iceberg catalog stay on 8080:
    - `/ranger/` authenticates in a layer before any body, and external engines reach it through the ingress;
    - the others carry no pre-auth body work.
  - The listener must not be the same address as `server` or `apps.listen`; config validation refuses that.
- **B2. URL given to engines.**
  - New key `[governance.opa] engine_url`. When unset, it is derived from `engine_egret_url` (`main.rs:2805-2812`) by replacing the port with the listener's port.
  - `opa_plugin_files` (`crates/egret-engine-mgmt-api/src/opa_config.rs:102-122`) builds its base from it.
  - `GET /engines/{id}/opa-plugin-config` and the console's plugin tab show the new URL.
- **B3. Helm.**
  - The Deployment and Service gain port `opa` (8082).
  - A new `networkpolicy-egret.yaml` selects `app: egret` in the release namespace. It has two ingress rules:
    1. TCP 8080 and 8081 from anywhere, which keeps today's reachability;
    2. TCP 8082 from `engineNamespace` and from `opa.extraFrom` (new value, default `[]`, for engines outside the cluster's engine namespace).
  - It is enabled with the engine policy (`engineNetworkPolicy.enabled`).
  - The ingress gains no `/opa` rule.
- **B4. Compose.**
  - `deploy/opa-e2e` and the demo stack set the OPA listener.
  - Engines get `http://egret:8082/opa/...`.
  - Ports are not published to the host.
- **B5. Transition.**
  - Migration **0216** adds `engine_mgmt.engines.applied_opa_url text` (nullable).
  - Every apply that renders OPA plugin files records the base URL it rendered. Other applies clear it.
  - At start, when OPA is enabled, a one-shot pass re-applies every provisioned engine on an OPA profile whose `applied_opa_url` differs from the current one. An engine whose URL is current is not touched, so a restart of egret restarts no engine.
  - A failure is logged and reported by engine, as `reapply_bound_engines` does, and does not stop the server.

## 3. Shape of the change

- **PR A:**
  - provisioner render (`trino.rs`) and engine apply normalisation (`engines.rs`), with unit tests;
  - the live result;
  - the engine list's "re-apply needed" marker;
  - docs: engine security page, upgrade note, a known-issues entry fixed.
- **PR B:**
  - config keys (`egret-config`);
  - the listener (`egret-bin`) and router split (`egret-server`);
  - `opa_plugin_files` and the plugin-config route;
  - migration 0216 and the start pass;
  - Helm templates and values;
  - compose stacks and the `opa-e2e` scripts;
  - docs: Trino on OPA page, configuration keys, Helm values, upgrade note.

## 4. Tests

- **A, unit:**
  - the render has `allow-insecure-over-http=false` for impersonate and for no-auth specs;
  - normalisation adds the auth block;
  - a mutant that restores `true` is killed.
- **A, live:** A4, and an `opa-e2e` scenario that asserts `--session-user bob` is refused over plain HTTP on both engines.
- **B, router:**
  - `build_router` answers 404 for `POST /opa/v1/data/egret/trino/allow`;
  - the OPA router on its own listener answers;
  - config validation refuses a port clash.
- **B, render:**
  - `helm template` has both NetworkPolicy rules and the `opa` port;
  - `opa_plugin_files` uses `engine_url`;
  - the start pass re-applies only stale engines (DB test).
- **B, e2e:** `opa-e2e` runs on the new port, and asserts that `egret:8080/opa/...` is 404 from the engine container.

## 5. Follow-ups recorded

- **StarRocks FE 9030 `root` without a password** (out of scope, §2.1).
- **Task 26** after A and B:
  - the no-tag-service warning in the switch answer and in explain;
  - shared-tag-service latency (M5);
  - the full-router `/batch` > 4 MiB test (M4);
  - the default flip;
  - docs for KI-352/353/354 as decided.
