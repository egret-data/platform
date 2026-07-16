# StarRocks Delegation / Security Parity — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. Follow superpowers:test-driven-development per task.

**Goal:** Bring StarRocks per-user delegation + transport security to parity with Trino — `jwt` | `impersonate` mode selection, impersonate via `EXECUTE AS <session_user> WITH NO REVERT`, TLS on the `:9030` MySQL connection, and provisioning the FE for JWT + TLS + an impersonation service user.

**Architecture:** The shared delegation decision (`submit_query`) + engine-JWT mint are reused UNCHANGED — they already produce `(session_user, bearer)` per mode engine-agnostically. This slice makes the StarRocks **connector** honor the impersonate path (`EXECUTE AS`) + TLS (`mysql_async` `SslOpts` + FE cert pin), and extends the slice-2 provisioner to configure the FE.

**Tech Stack:** Rust (`egret-query`, `egret-orchestrator`, `egret-engine-mgmt`), `mysql_async` (patched, `third_party/`), StarRocks `authentication_jwt` + `EXECUTE AS` + FE SSL, React/TS frontend.

**Spec:** `docs/en/specs/2026-07-16-starrocks-delegation-design.md`. **Branch:** `feat/starrocks-delegation` (off `feat/starrocks-provisioning` — slice 2).

**Reference:** `crates/egret-query/src/execution/connector/trino.rs` (bearer/impersonate/TLS structure); slice-1 `deploy/starrocks/init/security-integration.sql` (the JWT integration SQL).

---

## Task 1: Connector — impersonate (`EXECUTE AS`) + TLS

**Files:** Modify `crates/egret-query/src/execution/connector/starrocks.rs`.

- [ ] **Step 1: Write failing unit tests** (add a `#[cfg(test)] mod tests` to `starrocks.rs`, or a new `tests/connector_starrocks_opts.rs` that constructs specs — no live server). Add a `#[cfg(test)]` accessor `pub fn execute_as(&self) -> Option<&str>` alongside the existing `opts()`.
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::execution::connector::ConnSpec;
      use egret_query_api::{ConnOptions, Engine};

      fn base_spec() -> ConnSpec {
          ConnSpec { engine: Engine::StarRocks, host: "fe".into(), port: 9030,
              options: ConnOptions::default(), secret: Some("svc-pw".into()),
              session_user: None, bearer_token: None }
      }

      #[test]
      fn impersonate_sets_execute_as_and_service_login() {
          let mut s = base_spec();
          s.options.impersonate = Some(true);
          s.options.user = Some("svc".into());
          s.session_user = Some("alice".into());
          let c = StarRocksConnector::new(&s);
          assert_eq!(c.execute_as(), Some("alice")); // impersonate → EXECUTE AS alice
          // logs in as the SERVICE user (not alice) on the impersonate path
          assert_eq!(c.opts().user(), Some("svc"));
      }

      #[test]
      fn jwt_mode_logs_in_as_user_no_execute_as() {
          let mut s = base_spec();
          s.bearer_token = Some("jwt".into());
          s.session_user = Some("alice".into());
          let c = StarRocksConnector::new(&s);
          assert_eq!(c.execute_as(), None);          // jwt login IS the user; no switch
          assert_eq!(c.opts().user(), Some("alice"));
      }

      #[test]
      fn tls_cert_enables_ssl() {
          let mut s = base_spec();
          s.options.tls_cert = Some("-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n".into());
          let c = StarRocksConnector::new(&s);
          assert!(c.opts().ssl_opts().is_some());
      }

      #[test]
      fn no_tls_cert_no_ssl() {
          assert!(StarRocksConnector::new(&base_spec()).opts().ssl_opts().is_none());
      }
  }
  ```

- [ ] **Step 2: Run → FAIL** (`cargo test -p egret-query --lib starrocks` — `execute_as` undefined, ssl not wired).

- [ ] **Step 3: Add the struct field + compute impersonate + TLS in `new`.** Change the struct + `new`:
  ```rust
  pub struct StarRocksConnector {
      opts: mysql_async::Opts,
      /// Set on the impersonate path (service-user login + run AS this user).
      /// `EXECUTE AS \`<user>\` WITH NO REVERT` is prepended to each connection.
      execute_as: Option<String>,
  }
  ```
  In `new`, after the existing `user`/bearer/secret block, before building `opts`:
  ```rust
      // Impersonate mode (Trino parity): no bearer, connection opts in, end-user resolved.
      // egret logs in as the service user and runs AS the caller via EXECUTE AS.
      let execute_as = if spec.bearer_token.is_none()
          && spec.options.impersonate == Some(true)
      {
          spec.session_user.clone()
      } else {
          None
      };

      // TLS: pin the FE cert (egret-provisioned). mysql_clear_password sends the JWT
      // in cleartext, so this MUST be on for a bearer connection in production.
      if let Some(pem) = &spec.options.tls_cert {
          let ssl = mysql_async::SslOpts::new()
              .with_root_certs(vec![mysql_async::PathOrBuf::from(pem.clone().into_bytes())])
              // egret pins a self-signed FE cert whose CN won't match the service DNS.
              .with_danger_skip_domain_validation(true);
          b = b.ssl_opts(Some(ssl));
      }
      StarRocksConnector { opts: b.into(), execute_as }
  ```
  Add the `#[cfg(test)] pub fn execute_as(&self) -> Option<&str> { self.execute_as.as_deref() }` accessor next to `opts()`.
  (Confirm the `mysql_async` re-export path for `SslOpts`/`PathOrBuf`; import at top if needed.)

- [ ] **Step 4: Prepend `EXECUTE AS` in `execute`.** After `Conn::new(...)` succeeds and before `conn.query_iter(sql)`:
  ```rust
      if let Some(u) = &self.execute_as {
          // Escape backticks in the identifier; switch the session identity
          // irreversibly for this connection so Ranger authorizes the query as `u`.
          let esc = u.replace('`', "``");
          conn.query_drop(format!("EXECUTE AS `{esc}` WITH NO REVERT"))
              .await
              .map_err(|e| EngineError::Engine(e.to_string()))?;
      }
  ```
  (`query_drop` is on `Queryable`, already imported.)

- [ ] **Step 5: Run → PASS** (`cargo test -p egret-query --lib starrocks`). `cargo fmt -p egret-query`; `cargo clippy -p egret-query --all-targets -- -D warnings` clean. (The live `tests/connector_starrocks.rs` are env-gated — unaffected.)

- [ ] **Step 6: Commit.** `feat(query): StarRocks impersonate (EXECUTE AS) + TLS delegation`.

---

## Task 2: Provisioning — FE JWT integration + TLS + impersonation user

**Files:** Modify `crates/egret-engine-mgmt-api/src/lib.rs` (a delegation block on `StarRocksConfig`); `crates/egret-orchestrator/src/starrocks.rs` (FE `ssl_*` config); `crates/egret-engine-mgmt/src/engines.rs` (runtime JWT-integration + impersonation-user bootstrap; store `impersonate`/`tls_cert` on the connection).

- [ ] **Step 1: Add a delegation block to `StarRocksConfig`.** In `egret-engine-mgmt-api`:
  ```rust
  /// Delegation/security wiring for a provisioned StarRocks. `None` = no JWT/TLS.
  #[derive(Clone, Debug)]
  pub struct StarRocksDelegation {
      /// Keycloak JWKS URL reachable from the FE pod.
      pub jwks_url: String,
      pub issuer: String,
      pub audience: String,
      /// PEM of the FE TLS cert egret generated (also pinned on the connection).
      pub tls_cert_pem: String,
      /// Enable FE SSL on :9030.
      pub tls_enabled: bool,
  }
  ```
  Add `pub delegation: Option<StarRocksDelegation>` to `StarRocksConfig` (update the manual `Debug` impl to redact nothing new; update the `sample_sr_spec`/constructors — grep `StarRocksConfig {`).

- [ ] **Step 2: Template FE SSL in `starrocks_fe_configmap`** (unit-test first). Add a test asserting `ssl_*` keys appear only when `delegation.tls_enabled`. Then, in `starrocks_fe_configmap`, when `sr.delegation` has `tls_enabled`, append the FE SSL config (exact keys are LIVE-ONLY — Task 3 smoke tunes them; start with the documented set):
  ```rust
  if let Some(d) = sr.delegation.as_ref().filter(|d| d.tls_enabled) {
      let _ = d;
      conf.push_str("ssl_keystore_location = /etc/starrocks/ssl/keystore\n");
      conf.push_str("ssl_keystore_password = changeit\n");
      conf.push_str("ssl_key_password = changeit\n");
      conf.push_str("ssl_force_secure_transport = false\n"); // allow both in dev; true in prod
  }
  ```
  Mount the keystore/cert ConfigMap on the FE spec (a `<name>-ssl-cm`, like the ranger-cm mount in slice 2) when TLS is enabled.

- [ ] **Step 3: Runtime JWT-integration + impersonation-user bootstrap.** Mirror the slice-2 external-catalog auto-wire (`spawn_starrocks_catalog_wire`): in the same post-Ready best-effort task, when the engine has a delegation config, run against the FE (over TLS if enabled):
  - `CREATE SECURITY INTEGRATION egret_oidc PROPERTIES("type"="authentication_jwt","jwks_url"="<jwks>","principal_field"="preferred_username","required_issuer"="<issuer>","required_audience"="<aud>")`
  - `ADMIN SET FRONTEND CONFIG ("authentication_chain"="native,egret_oidc")`
  - For impersonate mode: `CREATE USER IF NOT EXISTS '<svc>'@'%' IDENTIFIED BY '<pw>'` + `GRANT IMPERSONATE ON ALL USERS TO '<svc>'@'%'` (the connection's service user).
  All best-effort + logged; the SQL builders are pure + unit-tested (assert the integration SQL + grant strings; escape identifiers).

- [ ] **Step 4: Store the connection's mode + cert.** Where `create_starrocks_engine` records the engine's connection (`databases` row / the primary catalog wiring), set `options.impersonate` + `options.tls_cert` from the delegation config so the connector (Task 1) picks the mode automatically. (If the engine's query connection is created elsewhere, thread the delegation values there.)

- [ ] **Step 5: Build + tests.** `cargo test -p egret-orchestrator starrocks`; `cargo test -p egret-engine-mgmt`; `cargo build -p egret-bin`. fmt + clippy clean.

- [ ] **Step 6: Commit.** `feat(engine): provision StarRocks JWT integration + FE TLS + impersonation user`.

---

## Task 3: Frontend — surface delegation mode + TLS for StarRocks

**Files:** Modify `frontend/src/routes/control-plane/` (engine/connection config) + any connection-edit surface.

- [ ] **Step 1: De-Trino-gate the delegation-mode + TLS controls.** Audit the connection/engine config for `engine === "trino"` (or `kind`) gates around the impersonate/delegation-mode selector (#257) + TLS indicator; open them to `starrocks` where the semantics match (impersonate + jwt both apply; the `catalog`/`http_scheme` Trino-only fields stay Trino-gated). Grep the FE for `impersonate`, `delegat`, `tls`, `http_scheme` and adjust the gates.

- [ ] **Step 2: Test.** Add/extend a test asserting the delegation-mode selector renders for a StarRocks connection/engine. `cd frontend && npm run typecheck && npx vitest run src/routes/control-plane` (+ the touched components). Only the pre-existing `useEngineConfigForm.dirty.test.tsx` typecheck error is allowed.

- [ ] **Step 3: Commit.** `feat(fe): surface delegation mode + TLS for StarRocks connections`.

---

## Task 4: Gate + LIVE SMOKE (controller-driven)

- [ ] **Step 1: Full CI locally.** `cargo fmt --all`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace` (DATABASE_URL=egret_test; the 2 pre-existing `egret-engine-mgmt/assignment.rs` failures are unrelated); frontend `lint` + `typecheck` + `vitest` + `build` (build blocked only by the pre-existing error).

- [ ] **Step 2: Provision a delegation-enabled StarRocks** (operator installed; egret in-cluster-reachable). Confirm FE comes up with the JWT security integration + TLS on `:9030`, and the impersonation service user + `IMPERSONATE` grant exist (`SHOW SECURITY INTEGRATIONS`, `ADMIN SHOW FRONTEND CONFIG LIKE 'authentication_chain'`).

- [ ] **Step 3: jwt-mode delegated query over TLS** — a delegated connection (jwt), two OIDC principals, run a query; the FE authenticates each via the JWT integration, TLS on the wire; Ranger enforces per identity (masked/denied differs). Record outputs.

- [ ] **Step 4: impersonate-mode delegated query over TLS** — flip the connection to impersonate; egret logs in as the service user + `EXECUTE AS <user>`; confirm `SELECT current_user()` / Ranger enforcement reflects the impersonated identity, not the service user.

- [ ] **Step 5: Report** exactly what ran (live queries). Fold any FE `ssl_*` key / `EXECUTE AS` grant / integration-activation fixes back into the code + spec.

**Fallback:** if FE TLS won't start, confirm the exact `ssl_*` keys + keystore format for the `fe-ubuntu` image (live-only). If `EXECUTE AS` is denied, check the service user's `IMPERSONATE` grant. If JWT login fails, verify the integration + `authentication_chain` activation + the JWKS URL reachability (slice-1 gotchas).

---

## Self-Review

- **Spec coverage:** connector impersonate+TLS → T1; provisioning FE JWT+TLS+impersonation-user + connection mode/cert → T2; FE surfacing → T3; jwt+impersonate live proof → T4. Shared decision/mint untouched (spec non-goal). Covered.
- **Type consistency:** `StarRocksConnector { opts, execute_as }`; `execute_as` computed from `bearer.is_none() && impersonate` in T1 and consumed in `execute`; `StarRocksDelegation`/`StarRocksConfig.delegation` (T2) drive both the FE `ssl_*` template and the runtime integration SQL; `options.impersonate`/`options.tls_cert` (existing `ConnOptions`) are the seam between T2 (writes) and T1 (reads).
- **No placeholders:** the connector code, `SslOpts` API, `EXECUTE AS` escaping, the integration/grant SQL, and the config seam are concrete. LIVE-ONLY values (FE `ssl_*` keys, keystore format) are explicitly deferred to the T4 smoke with fallbacks — mirroring slices 1–2.
- **Ambiguity:** impersonate is `EXECUTE AS … WITH NO REVERT` (not a proxy scheme); TLS is `mysql_async` root-cert pinning; jwt mode is unchanged (login-as-user, no EXECUTE AS).

---

## Live Verification (2026-07-16, dev cluster)

Provisioned a delegation-enabled StarRocks (`starrocks/fe-ubuntu:3.3-latest`, kube-starrocks operator, FE+CN shared-data, `delegationMode=jwt`, `tlsEnabled=true`) and drove the smoke. What was proven vs. what is blocked by the dev image:

**Proven on real StarRocks 3.3:**
- **Delegation-enabled provisioning** — the `StarRocksCluster` CR applies; the FE ConfigMap carries the `ssl_*` keys + the built keystore; the stored engine connection records `starrocksConfig.delegation = {mode, tlsEnabled}`. The delegation bootstrap fires (JWT-integration + impersonation SQL).
- **Keystore init-container (T2c fix)** — the FE pod schedules and the `build-keystore` init-container runs to completion, producing `/etc/starrocks/ssl/keystore` (a valid PKCS12, 1 `PrivateKeyEntry`). This validated the `secretKeyRef`-ENV fix for the operator's volume-name hashing (`getVolumeName`); the prior `secrets`-volume form failed to schedule (`volumeMounts[].name: Not found`).
- **Impersonate primitive (`EXECUTE AS`)** — `GRANT IMPERSONATE ON ALL USERS` + `EXECUTE AS <user> WITH NO REVERT` switch the session identity end-to-end: connected as `egret_svc`, `current_user()` flipped `'egret_svc'@'%'` → `'alice'@'%'`. This is exactly the connector's impersonate path (T1) + the provisioner's impersonation-grant SQL (T2), proven on a real FE.

**Blocked by the community image — FE MySQL-port TLS is enterprise-only:**
- With a valid keystore + `ssl_keystore_location`/`ssl_keystore_password`/`ssl_key_password` present, the FE still refuses TLS (`SSL is required but the server doesn't support it`). The FE boot log shows the root cause:
  `ERROR [SSLChannelImpClassLoader.loadSSLChannelImpClazz] can not found jar file at /opt/starrocks/fe/lib/starrocks-ssl-1.0.jar`.
  The community `fe-ubuntu` image does **not** bundle `starrocks-ssl-1.0.jar` (only netty/ranger/wildfly SSL libs), so the MySQL-protocol SSL channel is unavailable regardless of keystore format (PKCS12 and JKS both tested). `ssl_force_secure_transport` is not even a recognized FE config key on this build. The FE degrades gracefully (logs the error, boots without SSL) — the `ssl_*` block and keystore init-container do not break provisioning.
- **Consequence for `tlsEnabled`:** the connector requires TLS when `tls_cert` is set (`mysql_async` `SslOpts`), so `tlsEnabled=true` against a community FE yields a failed handshake. **On community images use `tlsEnabled=false`**; the TLS parity path (keystore + cert-pin) is correct and unit-tested but can only be exercised end-to-end against a TLS-capable (enterprise) StarRocks. jwt mode over a non-TLS FE sends the JWT via `mysql_clear_password` in cleartext — the connector already logs a loud warning for this; prefer impersonate mode on community images.

**Other dev blockers (unchanged from slice 2):** egret runs on the host and cannot resolve in-cluster FE DNS (`<name>-fe-service.<ns>.svc`) without a port-forward/in-cluster deploy; a full jwt-over-the-wire proof additionally needs Keycloak realm users with the `starrocks` audience + Ranger policies + pod-reachable JWKS.

**Disposition:** the connector, provisioning, de-Trino-gate, and FE surfacing are complete and unit-tested; the impersonate delegation primitive and delegation-enabled provisioning are live-proven. FE-TLS end-to-end and the cross-mode over-the-wire query proof are deferred to a TLS-capable StarRocks + an in-cluster egret — documented follow-ups, not code gaps.
