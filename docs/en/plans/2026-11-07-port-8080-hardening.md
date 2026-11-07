# Port 8080 Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close plain-HTTP identity spoofing on provisioned Trino engines (PR A), and move egret's `/opa/` surface to its own engines-only listener (PR B). Both are prerequisites of SP2b Task 26.

**Architecture:**
- **PR A** changes the Trino provisioner render, so every engine has an authenticator and `allow-insecure-over-http=false`. The engine apply path normalises a spec without auth. A recorded "security revision" tells operators which engines still need a re-apply.
- **PR B** splits the OPA router off `build_router` onto a second listener (`[governance.opa] listen`). It gives engines a URL on that port, records the URL per engine, and re-applies stale engines at start. Helm gets a NetworkPolicy on egret's pod that admits only engines to the new port.

**Tech Stack:** Rust (axum, sqlx, tokio), Helm templates, Docker Compose stacks, bash/python e2e scripts, React frontend (one badge), Trino 483.

**Spec:** `docs/en/specs/2026-11-07-port-8080-hardening-design.md` (read it first; §2 is binding).

## Global Constraints

- Worktree `/Users/1pro-l4r8/Documents/github/data-portal/egret-p8080`.
  - PR A is on branch `feat/port-8080-hardening`.
  - PR B is on branch `feat/opa-listener`, created from PR A's tip, and rebased with `--onto` after PR A squash-merges.
- Build with your own target dir: `CARGO_TARGET_DIR=target-p8080 CARGO_INCREMENTAL=0`.
- DB tests: `env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`. Never run two test suites at once.
- Migrations:
  - PR A uses `migrations/0215_engine_security_rev.sql`; PR B uses `migrations/0216_engine_applied_opa_url.sql`.
  - Every statement is `ADD COLUMN IF NOT EXISTS`.
  - Re-check the numbers against `origin/main` and open PRs before each push.
- `TRINO_SECURITY_REV: i16 = 1`.
- Default OPA listen address: `0.0.0.0:8082`. Helm port name: `opa`.
- Docker:
  - Never run `docker system prune`, any prune, a mass `docker rm`, or `just up/down/reset-testdata`.
  - Never touch containers of `egret-dev-*`, `egret-demo`, `egretdocs-*`, `egret-s4-*` or `example-dwh*`.
  - A live check uses its own compose project.
  - A backend image build can crash Docker Desktop, so check free VM memory first, and prefer stacks that need no egret image.
- Regenerate the frontend client only with `npm run gen:api`, never with a bare orval.
- Commits are Conventional Commits, ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Every user-visible change (config keys, API fields, Helm values, behaviour) gets docs in `egret-data/docs`, written by the docs task of its PR.
- Each behaviour change has a test that fails without it, and a mutant killed by a failing assertion, not by a compile error.

---

## PR A — Trino identity

### Task A1: Live measurement of the flag on Trino 483 (no egret image)

Measure Trino's behaviour before writing code, on a standalone Trino 483 that mirrors egret's render. No egret build is needed.

**Files:**
- Create: `scripts/trino-insecure-http/` with `docker-compose.yml`, `etc/` (config files) and `check.sh`.
- Create: `docs/en/plans/2026-11-07-port-8080-hardening-live-result.md`.

**Interfaces:**
- Produces the measured facts Task A2 relies on:
  - which of the probe (`GET /v1/info`), `preStop` (`GET /v1/info/state` as `X-Trino-User: admin`), the monitor (Basic + `X-Forwarded-Proto: https`) and worker registration still work at `false`;
  - whether a forwarded-https request without a credential is refused.

- [ ] **Step 1:** Write the coordinator `config.properties`. Copy what `trino_config_map` renders for an impersonate engine with TLS: run `cargo test -p egret-provisioner impersonate_engine_keeps_insecure_over_http -- --nocapture` after temporarily printing `cc`, or read the render in `crates/egret-provisioner/src/trino.rs:85-197`.
  - Include `http-server.process-forwarded=true`, `http-server.authentication.type=PASSWORD`, `http-server.https.enabled=true`, `http-server.https.port=8443` with a self-signed PEM keystore, and `internal-communication.shared-secret`.
  - Include `password-authenticator.properties` with a `password.db` holding `egret_delegator` and `egret-monitor` (bcrypt `$2y$`, generated with `htpasswd -B -C 10`).
  - Add one worker with `discovery.uri=http://coordinator:8080`.
  - Use a file-based access control that allows everything, except that `impersonation` allows only `egret_delegator`. This isolates authentication from Ranger and OPA.
- [ ] **Step 2:** In `check.sh`, run each case twice: with `allow-insecure-over-http=true`, then `false`. Use `docker compose -p trino-insecure-http up -d`, then wait for `"starting":false`.

| # | Request | Expected at `true` | Expected at `false` |
|---|---|---|---|
| 1 | `curl -s http://coord:8080/v1/statement -H 'X-Trino-User: bob' -d 'SELECT current_user'`, following `nextUri` | runs as bob | refused (401/403) |
| 2 | Trino CLI `trino --server http://coord:8080 --user ann --session-user bob --execute 'SELECT current_user'` | `bob` | refused |
| 3 | As 1 plus `-H 'X-Forwarded-Proto: https'`, no credential | ? | refused |
| 4 | As 3 with `-u egret-monitor:<pw>`, `X-Trino-User: egret-monitor` | runs | runs |
| 5 | As 4 with `X-Trino-User: bob` (the monitor claiming bob) | ? | refused by impersonation |
| 6 | `curl -s http://coord:8080/v1/info` | 200 | record the result |
| 7 | `curl -s -H 'X-Trino-User: admin' http://coord:8080/v1/info/state` | 200 | record the result |
| 8 | `SELECT count(*) FROM system.runtime.nodes` over https 8443 as `egret_delegator` with `X-Trino-Original-User: egret_delegator`, `X-Trino-User: ann` | 2 nodes | 2 nodes |
| 9 | The same as 8, `-k`, as `egret_delegator` impersonating `ann` | runs as ann | runs as ann |

- [ ] **Step 3:** Run it. Record every row's real answer in the live-result doc, with the exact HTTP status and error text. Then tear down only the `trino-insecure-http` project.
- [ ] **Step 4:** Decide from the results and write the decision into the doc:
  - If rows 6 or 7 fail at `false`: the probe or `preStop` must authenticate (Task A2 step 4).
  - If row 3 succeeds without a credential: **stop and report BLOCKED**, because the spec's premise fails.
- [ ] **Step 5:** Commit: `test(trino): measure allow-insecure-over-http on Trino 483 (live result)`.

### Task A2: Render `allow-insecure-over-http=false` for every engine

**Files:**
- Modify: `crates/egret-provisioner/src/trino.rs:141-150` (the flag and its comment), and the probe and `preStop` only if A1 says so (`:594-619`, `:923`).
- Test: the same file, `mod tests` (`impersonate_engine_keeps_insecure_over_http` at `:2280` and the assertion at `:2096`).

**Interfaces:**
- Consumes: A1's live result.
- Produces: the render invariant "any rendered `http-server.authentication.type` ⇒ `allow-insecure-over-http=false`".

- [ ] **Step 1: Write the failing test.** Rename `impersonate_engine_keeps_insecure_over_http` to `impersonate_engine_rejects_insecure_over_http`. Assert `allow-insecure-over-http=false` and the absence of `=true`. Add `password_only_engine_rejects_insecure_over_http`, with `password_users` only, no JWT and no TLS. Update the `:2096` assertion to `false`.
- [ ] **Step 2:** Run `cargo test -p egret-provisioner insecure_over_http`. Expect FAIL.
- [ ] **Step 3: Implement.** Replace the block with:

```rust
                // Every authenticated engine refuses authentication over plain
                // HTTP (port-8080 hardening, spec 2026-11-07 A2). With `true`, Trino
                // does not authenticate a plain-HTTP request at all: whoever reaches
                // 8080 runs as the user they name (measured, live result
                // 2026-11-07). A request carrying `X-Forwarded-Proto: https` (the
                // ingress, egret's monitor) is treated as secure, so it must still
                // present a password or JWT. Coordinator<->worker traffic uses the
                // internal shared secret and is not subject to this flag.
                out.push_str(
                    "http-server.authentication.allow-insecure-over-http=false\n",
                );
```

- [ ] **Step 4:** Only if A1 recorded a failure for rows 6 or 7, make the probe or `preStop` authenticate. Reuse what the monitor sends (`crates/egret-engine-mgmt/src/monitor.rs:166-199`): Basic `egret-monitor` plus `X-Forwarded-Proto: https`. The password reaches the pod through the env Secret, the same way `TRINO_INTERNAL_SECRET` does (`trino.rs:194-196`). Add a render test for it.
- [ ] **Step 5:** Run `cargo test -p egret-provisioner`. Expect PASS.
- [ ] **Step 6: Mutation.** Change the literal to `true` and confirm the new tests fail on assertions. Restore, and `cmp` against a saved copy.
- [ ] **Step 7:** Commit: `fix(provisioner): refuse authentication over plain HTTP on every Trino engine`.

### Task A3: Every provisioned Trino engine has authentication

**Files:**
- Modify: `crates/egret-engine-mgmt/src/engines.rs`:
  - `resolve_auth` (`:2053`);
  - the create and update paths that store `spec` (find them by grepping for `trinoConfig` in `create_engine` and `update_engine`);
  - `inject_monitor_user` and `inject_delegator_user` (`:2165`, `:2184`), whose docs say "no-op when auth is None".
- Test: `engines.rs` `mod tests` (next to `inject_monitor_user_noop_when_no_auth` at `:8290`) and `crates/egret-engine-mgmt/tests/routes.rs`.

**Interfaces:**
- Produces `pub(crate) fn normalise_trino_auth(spec: &mut serde_json::Value)`. It sets `spec["trinoConfig"]["auth"] = {"password": {}}` when `trinoConfig` exists and `auth` is not an object. It is idempotent.

- [ ] **Step 1: Failing tests.**
  - Unit: `normalise_trino_auth` adds `{"password":{}}` to `{"trinoConfig":{}}`, keeps an existing `{"auth":{"passwordUsers":[...]}}` unchanged, and leaves a StarRocks spec untouched.
  - Route test: creating a Trino engine without `auth` stores a spec with `trinoConfig.auth` an object.
  - Route test: an update that drops `auth` stores it back.
  - Apply test: an engine row whose stored spec lacks auth renders `http-server.authentication.type=PASSWORD` with `egret-monitor` in `password.db`.
- [ ] **Step 2:** Run them. Expect FAIL.
- [ ] **Step 3: Implement.**

```rust
/// Every provisioned Trino engine authenticates (port-8080 hardening, spec
/// 2026-11-07 A1): a spec without an auth object is treated as PASSWORD auth
/// with no human users, which still provisions `egret-monitor` and, in
/// impersonate mode, `egret_delegator`.
pub(crate) fn normalise_trino_auth(spec: &mut serde_json::Value) {
    if let Some(tc) = spec.get_mut("trinoConfig").and_then(|v| v.as_object_mut()) {
        if !tc.get("auth").is_some_and(serde_json::Value::is_object) {
            tc.insert("auth".into(), serde_json::json!({ "password": {} }));
        }
    }
}
```

  - Call it in create and update before the spec is stored.
  - Call it at the start of the apply build on a clone of `engine.spec`, before `resolve_auth` (`:649`). An old row then gains auth without a separate write.
  - Correct the two injector docs: they still no-op on `None`, but `None` no longer happens for a Trino engine.
- [ ] **Step 4:** Run `cargo test -p egret-engine-mgmt` (DB env). Expect PASS.
- [ ] **Step 5: Mutation.** Make `normalise_trino_auth` a no-op; the apply and route tests must fail.
- [ ] **Step 6:** Commit: `fix(engines): every provisioned Trino engine authenticates`.

### Task A4: Record the security revision and show "re-apply needed"

**Files:**
- Create: `migrations/0215_engine_security_rev.sql`.
- Modify:
  - `crates/egret-engine-mgmt/src/store.rs`: add `set_applied_security_rev`, next to `set_applied_mechanism` at `:431`.
  - `crates/egret-engine-mgmt/src/engines.rs`: in `apply_built_noting` near `:1010-1027`, and in `EngineDto` at `:299` and its mapping.
  - `openapi.json` and `frontend/src/api/generated*`, through `npm run gen:api`.
  - The engine list in the frontend: find the component rendering the engines table under `frontend/src/routes/control-plane/`.
- Test: `crates/egret-engine-mgmt/tests/routes.rs`, and the frontend list test next to that component.

**Interfaces:**
- Produces:
  - `pub const TRINO_SECURITY_REV: i16 = 1;` in `engines.rs`;
  - `EngineDto.security_reapply_needed: bool`, which is true when the engine is provisioned (not external), is Trino, and has `applied_security_rev < TRINO_SECURITY_REV`.

- [ ] **Step 1: The migration.**

```sql
-- Port-8080 hardening (spec 2026-11-07 A1): the Trino security render an
-- engine was last applied with. 0 = before the hardening; egret writes
-- TRINO_SECURITY_REV on every successful Trino apply.
ALTER TABLE engine_mgmt.engines
    ADD COLUMN IF NOT EXISTS applied_security_rev smallint NOT NULL DEFAULT 0;
```

- [ ] **Step 2: Failing tests.**
  - A provisioned Trino engine created without applying lists `security_reapply_needed: true`.
  - After a successful apply, it lists `false`.
  - An external engine and a StarRocks engine list `false`.
- [ ] **Step 3: Implement.**
  - The store method is `UPDATE engine_mgmt.engines SET applied_security_rev = $2 WHERE id = $1`.
  - Call it in `apply_built_noting` beside `set_applied_mechanism`, when `res.is_ok()` and the engine is Trino.
  - If the write fails, log at error and append to the same "not recorded" note. A lost write shows the marker again, which is conservative.
  - Add the column to the row type and to the SELECTs that build `EngineDto`.
- [ ] **Step 4: Frontend.**
  - When `security_reapply_needed` is true, show an amber badge "Re-apply needed" on the engine's row, with the tooltip "This engine still accepts unauthenticated requests on port 8080. Re-apply it to turn on authentication."
  - Add a vitest test that renders the badge for a true row and not for a false row.
- [ ] **Step 5:** `npm run gen:api` in `frontend/`. Run `cargo test -p egret-server committed_openapi_json_is_current generated_client` first if the spec changed: regenerate `openapi.json` with `egret openapi > openapi.json`.
- [ ] **Step 6:** Run the engine-mgmt suite, `egret-server --lib`, and the frontend `npm run lint && npx tsc -b && npx vitest run src/routes/control-plane`. Expect PASS.
- [ ] **Step 7: Mutation.** Skip the `set_applied_security_rev` call; the "after apply lists false" test must fail.
- [ ] **Step 8:** Commit: `feat(engines): record the Trino security revision and flag engines to re-apply`.

### Task A5: Prove it end to end, and correct the old claims

**Files:**
- Modify:
  - `scripts/opa-e2e/live-check.sh`, or add `scripts/opa-e2e/identity-check.sh` called from `layer2.sh`'s setup;
  - the `opa-e2e` job in `.github/workflows/ci.yml`, if a new script needs a step;
  - `docs/en/specs/2026-07-15-delegated-jwt-tls-mode-design.md`, by appending an "As corrected, 2026-11-07" note under its insecure-over-HTTP paragraph (`:105-112`);
  - `docs/en/plans/2026-11-07-port-8080-hardening-live-result.md`, by adding the e2e result.

- [ ] **Step 1:** On both `trino-opa` and `trino-ranger`, from inside the coordinator container:
  - `curl` `/v1/statement` with `X-Trino-User: bob` and no credential must be refused;
  - the same request with `X-Forwarded-Proto: https` and no credential must be refused;
  - the existing layer-2 impersonation through `X-Trino-Original-User` with the delegator credential (`trino-batch.sh`) keeps working.

  Then check that `trino-batch.sh` still reaches the engines. It may currently use plain HTTP without a credential; if so, switch it to the delegator Basic credential plus `X-Forwarded-Proto: https`, since that is the supported path now.
- [ ] **Step 2:** Run the stack locally only if a fresh `egret-opa-e2e-backend:dev` image exists or can be built safely (Global Constraints). Otherwise push and let the CI `opa-e2e` job run it, and say so in the report.
- [ ] **Step 3:** Commit: `test(e2e): plain-HTTP identity is refused on both mechanisms`, plus `docs: correct the insecure-over-HTTP claim`.

### Task A6: Docs for PR A (`egret-data/docs`)

- [ ] **Step 1:** In a new docs worktree from `origin/main`, branch `docs/trino-plain-http-identity`:
  - the engine security or auth page under `platform/` (find it with `git grep -l -i "password users\|delegation mode"`): every Trino engine now authenticates; plain HTTP on 8080 refuses unauthenticated requests; clients use the ingress (HTTPS) or a password; the "Re-apply needed" badge;
  - `platform/deploy-operate/upgrade.md`: re-apply every Trino engine after the upgrade; the badge lists them; direct `http://…:8080` clients without a password stop working;
  - `platform/known-issues.md`: add a fixed row for the plain-HTTP identity issue, in the style of the other fixed rows.
- [ ] **Step 2:** Run `npm run build`, with `node_modules` symlinked from `../docs/node_modules` and removed afterwards. Expect PASS. Commit; do not push.

---

## PR B — the OPA listener

### Task B1: Config keys `listen` and `engine_url`

**Files:**
- Modify: `crates/egret-config/src/lib.rs` (`OpaSettings` at `:620-707`, `Config::validate`).
- Test: the same file's tests.

**Interfaces:**
- Produces:
  - `OpaSettings.listen: String`, default `"0.0.0.0:8082"`;
  - `OpaSettings.engine_url: Option<String>`;
  - `pub fn opa_engine_url(engine_egret_url: &str, settings: &OpaSettings) -> Result<String, String>`. It returns `engine_url` when set. Otherwise it parses `engine_egret_url` with the `url` crate (already a workspace dependency; check `Cargo.toml`), sets the port to `listen`'s port, and returns it without a trailing slash.

- [ ] **Step 1: Failing tests.**
  - The defaults.
  - `opa_engine_url("http://egret:8080", default)` returns `"http://egret:8082"`.
  - `"http://egret.ns.svc:8080/"` gives `"http://egret.ns.svc:8082"`.
  - An explicit `engine_url` wins.
  - `validate` refuses `listen` equal to `server.host:server.port`, and refuses `listen` equal to `apps.listen` when apps is configured. The message names both keys.
  - `listen` without a port is refused.
- [ ] **Step 2:** Run them. Expect FAIL.
- [ ] **Step 3: Implement.** Add the fields with serde defaults and doc comments in the style of `batch_body_limit_bytes`: key, env `EGRET_GOVERNANCE__OPA__LISTEN` / `…__ENGINE_URL`, why it exists (spec B1), and the NetworkPolicy relationship. Add the checks to `Config::validate`, where `OpaSettings::validate` is called.
- [ ] **Step 4:** Run `cargo test -p egret-config`. Expect PASS.
- [ ] **Step 5:** Commit: `feat(config): [governance.opa] listen and engine_url`.

### Task B2: Serve `/opa/` only on its own listener

**Files:**
- Modify:
  - `crates/egret-server/src/lib.rs:331-336`: remove the merge, and add `pub fn opa_router(state: &AppState) -> Option<Router>`;
  - `crates/egret-bin/src/main.rs:3629-3657`: bind the listener, modelled on the apps listener at `:3635-3655`.
- Test: `crates/egret-server/tests/opa_surface_mounted.rs`.

**Interfaces:**
- Consumes: `OpaSettings.listen` (B1).
- Produces: `egret_server::opa_router(&AppState) -> Option<axum::Router>`, which returns `Some` when `state.opa` is `Some`.

- [ ] **Step 1: Failing tests** in `opa_surface_mounted.rs`:
  - `build_router(state)` answers **404** to `POST {OPA_SURFACE_PREFIX}/allow` with a valid engine body;
  - `opa_router(&state).unwrap()` answers it as the old test did (200 with a decision);
  - `opa_router` is `None` when OPA is off.
- [ ] **Step 2:** Run `cargo test -p egret-server --test opa_surface_mounted`. Expect FAIL.
- [ ] **Step 3: Implement.**

```rust
/// The OPA decision surface (governance SP2b), served on its own listener
/// (`[governance.opa] listen`, port-8080 hardening spec B1) and never on the
/// API port: it reads a request body before it can authenticate it, so only
/// engines may reach it (Helm `networkpolicy-egret.yaml`).
pub fn opa_router(state: &AppState) -> Option<Router> {
    state
        .opa
        .clone()
        .map(|opa| egret_perm_compile::opa_surface(opa).with_state(()))
}
```

  In `main.rs`, after binding the main listener and before `serve_with_drain(listener, app, …)`:

```rust
    if let Some(router) = egret_server::opa_router(&app_state_for_opa) {
        let opa_addr = config.governance.opa.listen.clone();
        let opa_listener = tokio::net::TcpListener::bind(&opa_addr)
            .await
            .with_context(|| format!("binding OPA listener {opa_addr}"))?;
        tracing::info!(addr = %opa_addr, "OPA surface listening");
        tokio::spawn(async move {
            if let Err(e) = egret_server::serve_with_drain(
                opa_listener,
                router,
                egret_server::shutdown_signal(),
                egret_server::DRAIN_DEADLINE,
            )
            .await
            {
                tracing::error!(error = %e, "OPA listener stopped");
            }
        });
    }
```

  `app_state_for_opa` is a clone of the `AppState` taken before it moves into `build_router`. Find where `app` is built in `main.rs`, and clone the state there. A bind failure is fatal (`?`), like the main listener.
- [ ] **Step 4:** Run the test, then `cargo test -p egret-server`. Expect PASS.
- [ ] **Step 5: Mutation.** Re-add the merge into `build_router`; the 404 test must fail.
- [ ] **Step 6:** Commit: `feat(opa): serve the OPA surface on its own listener only`.

### Task B3: Give engines the OPA listener's URL

**Files:**
- Modify:
  - `crates/egret-engine-mgmt/src/state.rs` (`CpState`, beside `engine_egret_url` at `:77`): add `engine_opa_url: String`;
  - `crates/egret-policy/src/admin/mod.rs:93`: add the same field;
  - `crates/egret-bin/src/main.rs:2805-2812`: compute it once with `egret_config::opa_engine_url`, and pass it at `:2862` and `:3104` and wherever `engine_egret_url` is passed into these states;
  - `crates/egret-engine-mgmt/src/engines.rs:1238`: `egret_url` is the OPA URL when `mechanism` is OPA;
  - `crates/egret-engine-mgmt/src/external.rs:995-1010` and `crates/egret-policy/src/admin/mod.rs:1187-1197`: use `engine_opa_url` for OPA files;
  - every test constructor of these states: grep for `engine_egret_url: "http://egret:8080".into()` and add `engine_opa_url: "http://egret:8082".into()`.
- Test: `crates/egret-engine-mgmt/tests/routes.rs:16928` (the `opa-plugin-config` route test) and an apply-render test.

- [ ] **Step 1: Failing tests.**
  - `GET /engines/{id}/opa-plugin-config` returns files whose `opa.policy.uri` starts with `http://egret:8082/opa/`.
  - Applying an engine on an OPA profile renders the same.
  - An engine on a Ranger profile still renders the Ranger URL on 8080.
- [ ] **Step 2:** Run them. Expect FAIL.
- [ ] **Step 3: Implement.** In `ranger_binding`:

```rust
        egret_url: match mechanism {
            egret_engine_mgmt_api::opa_config::AccessMechanism::Opa => st.engine_opa_url.clone(),
            _ => st.engine_egret_url.clone(),
        },
```

  `main.rs` fails start when `opa_engine_url` returns `Err`, naming `governance.opa.engine_url`.
- [ ] **Step 4:** Run the engine-mgmt and policy suites. Expect PASS.
- [ ] **Step 5: Mutation.** Use `engine_egret_url` for OPA again; the tests must fail.
- [ ] **Step 6:** Commit: `feat(opa): engines reach the OPA surface on its own port`.

### Task B4: Record the applied OPA URL; re-apply stale engines at start

**Files:**
- Create: `migrations/0216_engine_applied_opa_url.sql`.
- Modify:
  - `crates/egret-engine-mgmt/src/store.rs`: add `set_applied_opa_url(id, Option<&str>)` and `list_engines_with_stale_opa_url(current: &str) -> Vec<EngineRow>`;
  - `crates/egret-engine-mgmt/src/engines.rs`: in `apply_built_noting`, and add `pub async fn reapply_stale_opa_engines(st: &CpState) -> egret_policy_api::ReconfigureReport`, exported from `lib.rs` next to `reapply_bound_engines`;
  - `crates/egret-bin/src/main.rs`: when OPA is enabled, spawn the pass after the engine provisioner is ready.
- Test: `crates/egret-engine-mgmt/tests/routes.rs` (DB).

```sql
-- Port-8080 hardening (spec 2026-11-07 B5): the OPA base URL egret rendered
-- into this engine's plugin files at its last successful apply; NULL when the
-- engine was not applied on OPA. A start-up pass re-applies engines on an OPA
-- profile whose value differs from the current URL.
ALTER TABLE engine_mgmt.engines
    ADD COLUMN IF NOT EXISTS applied_opa_url text;
```

- [ ] **Step 1: Failing tests.**
  - After an OPA apply, `applied_opa_url` equals `engine_opa_url`; after a Ranger apply it is NULL.
  - `reapply_stale_opa_engines` re-applies an OPA-profile engine whose stored URL is `http://egret:8080` and lists it in `applied`.
  - It does not touch an engine whose URL is current: assert the provisioner fake saw no apply.
  - It skips external, deleting and Ranger-profile engines.
- [ ] **Step 2:** Run them. Expect FAIL.
- [ ] **Step 3: Implement.**
  - The stale query: engines joined to their authz service with `mechanism = 'opa'`, not external, status not deleting, and `applied_opa_url IS DISTINCT FROM $1`. Reuse the filters of `reapply_bound_engines` (`engines.rs:1132-1134`).
  - The pass calls `reapply_one` for each engine and logs each failure at warn.
  - In `main.rs`, spawn it with `tokio::spawn` so a slow provisioner never delays serving.
- [ ] **Step 4:** Run the suite. Expect PASS.
- [ ] **Step 5: Mutation.** Drop `IS DISTINCT FROM` (re-apply all); the "current URL untouched" test must fail.
- [ ] **Step 6:** Commit: `feat(opa): re-apply engines whose OPA URL is stale at start`.

### Task B5: Helm — the `opa` port and an egret NetworkPolicy

**Files:**
- Modify:
  - `deploy/helm/egret/values.yaml`: add an `opa:` block next to `engineNetworkPolicy`;
  - `templates/configmap-env.yaml`;
  - `templates/backend-deployment.yaml:28-31`;
  - `templates/backend-service.yaml`.
- Create: `deploy/helm/egret/templates/networkpolicy-egret.yaml`.
- Test: `crates/egret-server/tests/deployment_probes.rs` (same text-reading pattern) or a new `crates/egret-server/tests/helm_opa_listener.rs`.

```yaml
# egret answers Trino's OPA plugin (governance SP2b) on its own port, reachable only
# from engines (templates/networkpolicy-egret.yaml).
opa:
  enabled: false
  port: 8082
  # NetworkPolicy peers outside engineNamespace that may reach the OPA port, for
  # example the namespace of an external Trino on OPA.
  extraFrom: []
```

```yaml
{{- /*
Who may connect to egret's pod. The API (8080) and the apps host (8081) stay open
to every peer, as before this policy existed. The OPA port only admits engines:
the OPA surface reads a request body before it can authenticate it, so a peer that
holds stalled requests could exhaust its budget (port-8080 hardening spec B3).
*/ -}}
{{- if .Values.engineNetworkPolicy.enabled }}
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata: { name: egret-ingress, namespace: {{ .Release.Namespace }} }
spec:
  podSelector: { matchLabels: { app: egret } }
  policyTypes: [Ingress]
  ingress:
    - ports:
        - { protocol: TCP, port: 8080 }
        - { protocol: TCP, port: 8081 }
    - from:
        - namespaceSelector: { matchLabels: { kubernetes.io/metadata.name: {{ .Values.engineNamespace }} } }
        {{- with .Values.opa.extraFrom }}
        {{- toYaml . | nindent 8 }}
        {{- end }}
      ports:
        - { protocol: TCP, port: {{ .Values.opa.port }} }
{{- end }}
```

- [ ] **Step 1: Failing test.** Read the templates and assert:
  - the policy file selects `app: egret`;
  - it has an 8080/8081 rule with no `from`;
  - its OPA rule names `engineNamespace`;
  - the Deployment and Service carry the `opa` port under `opa.enabled`;
  - the env map sets `EGRET_GOVERNANCE__OPA__ENABLED` and `EGRET_GOVERNANCE__OPA__LISTEN: "0.0.0.0:{{ .Values.opa.port }}"` under `opa.enabled`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Verify.**
  - `helm template t deploy/helm/egret --set opa.enabled=true | grep -A30 'name: egret-ingress'` shows both rules. Paste the output in the report.
  - `helm template … --set opa.enabled=false` still renders the policy, with an unused OPA rule.
  - `helm lint deploy/helm/egret` passes.
- [ ] **Step 4:** Commit: `feat(helm): egret's OPA port, reachable only from engines`.

### Task B6: Compose stacks and `opa-e2e` on the new port

**Files:**
- Modify:
  - `deploy/opa-e2e/docker-compose.yml`: no host publish for 8082; set `EGRET_GOVERNANCE__OPA__LISTEN: 0.0.0.0:8082` explicitly;
  - `deploy/docker-compose.demo.yml`, only if it enables OPA (grep shows it does not today, so leave it);
  - `scripts/opa-e2e/live-check.sh`;
  - any `opa-e2e` script that names `:8080/opa`.

- [ ] **Step 1:** In `live-check.sh`, add:
  - a check that the engine's rendered `access-control.properties` names `:8082/opa/`;
  - from the `trino-opa` container, `curl -s -o /dev/null -w '%{http_code}' -X POST http://egret:8080/opa/v1/data/egret/trino/allow` prints `404`.
- [ ] **Step 2:** Push. The CI `opa-e2e` job is the run of record. A local run follows the Docker rules in Global Constraints.
- [ ] **Step 3:** Commit: `test(e2e): the OPA surface is on its own port`.

### Task B7: Docs for PR B (`egret-data/docs`)

- [ ] **Step 1:** In a docs worktree, branch `docs/opa-listener`:
  - `platform/data-governance/engines-and-policy/trino-on-opa.md`: the OPA port, the engine URL, external engines re-installing plugin files, and the NetworkPolicy;
  - the configuration-keys page: `governance.opa.listen` and `governance.opa.engine_url`;
  - the Helm values page: `opa.enabled`, `opa.port`, `opa.extraFrom`, and the new `egret-ingress` policy;
  - `platform/deploy-operate/upgrade.md`: provisioned engines on OPA are re-applied at start; external ones re-install their files; firewalls must allow 8082 from engines.

  Escape every `{…}` outside code spans: MDX reads it as an expression and breaks the build (docs #66).
- [ ] **Step 2:** Run `npm run build`. Expect PASS. Commit; do not push.

---

## Self-review notes

- **Spec coverage:**

| Spec item | Tasks |
|---|---|
| A1 | A3, A4 |
| A2 | A2 |
| A3 | A1, A2 step 4 |
| A4 | A1, A5 |
| A5 | A2 (comment), A5 (the 2026-07-15 note) |
| B1 | B1, B2 |
| B2 | B1, B3 |
| B3 | B5 |
| B4 | B6 |
| B5 | B4 |
| §4 tests | spread across the tasks |
| §5 follow-ups | out of scope |

- **Names used across tasks:**
  - `normalise_trino_auth`, `TRINO_SECURITY_REV`, `security_reapply_needed`, `set_applied_security_rev`;
  - `OpaSettings.listen` and `OpaSettings.engine_url`, `opa_engine_url`, `opa_router`;
  - `engine_opa_url`, `set_applied_opa_url`, `list_engines_with_stale_opa_url`, `reapply_stale_opa_engines`.
