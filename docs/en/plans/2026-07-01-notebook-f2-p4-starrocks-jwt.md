# Notebook F2-P4 — egret→StarRocks OIDC JWT Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** egret's StarRocks connector presents the user's short-lived OIDC engine JWT over the MySQL protocol (via the `mysql_clear_password` client plugin) so StarRocks — configured with an `authentication_jwt` security integration — authorizes queries per-user.

**Architecture:** The callback (`POST /api/kernel/query`) already mints the user's engine JWT and threads it as `ConnSpec.bearer_token` for ANY engine (F2-P3). P4 makes `StarRocksConnector::new` USE it: when `bearer_token` is set, present the JWT as the cleartext password (`.pass(jwt).enable_cleartext_plugin(true)`), which `mysql_async` 0.34 transmits verbatim; else the existing native-password path. StarRocks security-integration SQL is added as a deploy artifact.

**Tech Stack:** Rust (`mysql_async` 0.34 — `enable_cleartext_plugin`), StarRocks SQL config.

**Scope / verification note:** The connector change is unit-testable locally (assert the `Opts` carry the JWT + cleartext plugin — no live server). **StarRocks' actual JWT enforcement (the security integration + the cleartext auth-switch handshake) is verified only against the live Keycloak+StarRocks stack — F2-P5.** No kernel/handler change is needed (P3 already mints + passes the bearer). Daft-direct `read_sql` remains a separate later phase.

---

## File structure
- `crates/egret-engine/src/connector/starrocks.rs` — branch `new()` on `bearer_token`; add `opts()` test accessor + `#[cfg(test)] mod tests`.
- `deploy/starrocks/init/security-integration.sql` + `deploy/starrocks/README.md` — the `CREATE SECURITY INTEGRATION` SQL (applied manually / at P5).

---

## Task 1: StarRocks connector presents the JWT (TDD)

**Files:** `crates/egret-engine/src/connector/starrocks.rs`.

- [ ] **Step 1: Add a test accessor** to `StarRocksConnector` (for unit-testing the built `Opts`):
```rust
    #[cfg(test)]
    pub fn opts(&self) -> &mysql_async::Opts {
        &self.opts
    }
```

- [ ] **Step 2: Failing unit test** — add a `#[cfg(test)] mod tests` at the bottom of the file (mirror trino.rs's test module; import `super::*` + `ConnSpec`/`ConnOptions`/`Engine` from the crate — check the exact paths trino.rs's tests use):
```rust
#[cfg(test)]
mod tests {
    use super::StarRocksConnector;
    use crate::connector::ConnSpec;
    use crate::model::{ConnOptions, Engine}; // adjust to the real paths (see trino.rs tests)

    fn spec(bearer: Option<String>, secret: Option<String>) -> ConnSpec {
        ConnSpec {
            engine: Engine::StarRocks,
            host: "h".into(),
            port: 9030,
            options: ConnOptions { user: Some("alice".into()), ..Default::default() },
            secret,
            session_user: Some("alice".into()),
            bearer_token: bearer,
        }
    }

    #[test]
    fn bearer_uses_cleartext_plugin_with_jwt_as_password() {
        let c = StarRocksConnector::new(&spec(Some("eyJ.jwt.sig".into()), Some("native-pw".into())));
        assert!(c.opts().enable_cleartext_plugin());
        assert_eq!(c.opts().pass(), Some("eyJ.jwt.sig"));
    }

    #[test]
    fn no_bearer_keeps_native_password_no_cleartext() {
        let c = StarRocksConnector::new(&spec(None, Some("native-pw".into())));
        assert!(!c.opts().enable_cleartext_plugin());
        assert_eq!(c.opts().pass(), Some("native-pw"));
    }
}
```
(VERIFY the `ConnOptions`/`Engine`/`ConnSpec` import paths + that `Engine::StarRocks` is the variant name, by reading the existing `connector_starrocks.rs` test's `spec()` helper and trino.rs's `mod tests`. Set EVERY `ConnSpec` field explicitly. `mysql_async::Opts` getters `enable_cleartext_plugin()` + `pass()` are public in 0.34.2 — `pass()` returns `Option<&str>`.)

- [ ] **Step 3: Run — fails** `cargo test -p egret-engine starrocks` (or the exact test names) → FAIL (bearer ignored / cleartext off).

- [ ] **Step 4: Implement** in `StarRocksConnector::new` — replace the current secret block:
```rust
        if let Some(s) = &spec.secret {
            b = b.pass(Some(s.clone()));
        }
```
with a bearer-first branch:
```rust
        if let Some(jwt) = &spec.bearer_token {
            // StarRocks `authentication_jwt`: present the OIDC JWT as the cleartext
            // "password" via the mysql_clear_password client plugin. mysql_async
            // transmits it verbatim (unhashed) when the server requests that plugin.
            b = b.pass(Some(jwt.clone())).enable_cleartext_plugin(true);
        } else if let Some(s) = &spec.secret {
            b = b.pass(Some(s.clone()));
        }
```
(Keep the `db_name` block after it unchanged.)

- [ ] **Step 5: Run — passes** `cargo test -p egret-engine starrocks` + `cargo build -p egret-engine` + `cargo clippy -p egret-engine --all-targets -- -D warnings`.

- [ ] **Step 6: Commit**
```bash
git add crates/egret-engine/src/connector/starrocks.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(engine): StarRocksConnector presents a JWT via mysql_clear_password

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: StarRocks JWT security integration config (deploy artifact; validated at P5)

**Files:** `deploy/starrocks/init/security-integration.sql`, `deploy/starrocks/README.md`.

> Config artifact. StarRocks `allin1-ubuntu` has no init-SQL mount in the dev compose, so this is applied manually (documented) and validated in F2-P5. Do NOT try to apply it against a live StarRocks here.

- [ ] **Step 1: `deploy/starrocks/init/security-integration.sql`**:
```sql
-- F2-P4: authenticate notebook users to StarRocks with the OIDC engine JWT.
-- Apply manually against the FE (port 9030) as root, or via an init step (see README).
-- The JWKS + issuer + audience must match Keycloak realm `egret` and the `starrocks`
-- audience mapper added in F2-P1.
CREATE SECURITY INTEGRATION egret_oidc PROPERTIES (
    "type" = "authentication_jwt",
    "jwks_url" = "http://keycloak:8080/realms/egret/protocol/openid-connect/certs",
    "principal_field" = "sub",
    "required_issuer" = "http://keycloak:8080/realms/egret",
    "required_audience" = "starrocks"
);
-- Make the integration active (group provider / auth chain). Exact GROUP_PROVIDER +
-- authentication_chain syntax is StarRocks-version-specific — finalized at P5.
-- SET PROPERTY / ALTER SYSTEM ... as required by the image's StarRocks version.
```

- [ ] **Step 2: `deploy/starrocks/README.md`** — document: how to apply the SQL (`mysql -h127.0.0.1 -P9030 -uroot < security-integration.sql` once the FE is up), that P5 finalizes (a) the exact `jwks_url` host (compose DNS `keycloak:8080` vs `localhost:8081`), (b) the `required_issuer`/`required_audience` matching the realm, (c) the group→role mapping (native RBAC `GRANT`/`CREATE ROLE` keyed on the JWT groups or a GROUP PROVIDER), and (d) that the client presents the JWT via cleartext-over-plain-TCP (dev only; wants TLS for real). Note the connector side (Task 1) is done + unit-tested; this is the server side.

- [ ] **Step 3: Commit**
```bash
git add deploy/starrocks/init/security-integration.sql deploy/starrocks/README.md
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(deploy): StarRocks authentication_jwt security integration SQL (validated in P5)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Full local CI + P5 handoff note

- [ ] **Step 1:** `cargo fmt --all --check` → clean (fmt + commit if needed).
- [ ] **Step 2:** `cargo clippy --workspace --all-targets -- -D warnings` → clean.
- [ ] **Step 3:** `cargo test --workspace` (postgres + minio up, `warehouse` bucket, `DATABASE_URL`) → green except the known infra-flaky `egret-catalog-engine` MinIO tests, pre-existing `egret-engine` open_secret NULL-ws tests, the env-gated live-Trino/StarRocks tests (self-skip), and `#[ignore]`d kernel tests. Any NON-infra failure in `egret-engine` = BLOCKER. Confirm the 2 new StarRocks connector unit tests pass.
- [ ] **Step 4:** Document (PR body) that StarRocks JWT enforcement (the security integration + cleartext auth-switch handshake + group→role mapping) is validated end-to-end in **F2-P5** against the live Keycloak+StarRocks stack; the connector + unit tests are complete here.

---

## Self-review
- **Spec coverage (F2 §4.5 — StarRocks JWT):** connector presents the JWT via cleartext plugin (T1), security-integration SQL (T2). Enforcement e2e deferred to P5; no handler change needed (P3 already mints+passes the bearer). ✔
- **Placeholder scan:** T2 SQL values (jwks_url host, group mapping) are documented dev defaults refined at P5 — config artifacts, not logic placeholders. T1 logic + the exact `mysql_async` API (`enable_cleartext_plugin`, `pass`) fully specified.
- **Type consistency:** branch on `ConnSpec.bearer_token` (added in P3) → `.pass(jwt).enable_cleartext_plugin(true)`; `opts()` test accessor returns `&mysql_async::Opts`; getters `enable_cleartext_plugin()`/`pass()`.
- **Security:** the JWT is the user's own short-lived engine token; sent as cleartext password (dev over plain TCP — TLS noted for real). No token logged. Native-password (no-bearer) path unchanged. The precedence (bearer over secret) matches the `ConnSpec.bearer_token` contract.
