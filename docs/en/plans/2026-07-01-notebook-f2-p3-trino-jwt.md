# Notebook F2-P3 — egret→Trino OIDC JWT (Option A) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** The governed query path (kernel `POST /api/kernel/query` → egret `TrinoConnector`) presents the user's short-lived **OIDC engine JWT** (`Authorization: Bearer`) to Trino instead of basic-auth + `X-Trino-User` impersonation, and Trino is configured to authenticate via that JWT (Keycloak JWKS) + authorize by group.

**Architecture (Option A):** Thread an optional `bearer_token` through `ConnSpec` → `ConnectorFactory::build` → `TrinoConnector`. When present, the connector uses `reqwest`'s `bearer_auth(jwt)` at every Trino request (replacing `basic_auth`). The `kernel_query` handler already has the caller's `user_id` and the P1 `engine_minter`; it mints the user's engine JWT and passes it down a widened `NotebookQueryRunner::run`. Impersonation (`ConnOptions.impersonate`, `X-Trino-User`) is retained as the session user but is no longer the auth mechanism for the notebook path. Trino dev config (JWT auth + file-based group ACL) is added as deploy artifacts.

**Tech Stack:** Rust (`reqwest`, axum), Trino server config, existing `EngineTokenMinter` (P1).

**Scope / verification note:** The Rust changes (connector bearer, runner mint) are unit-testable locally. **Trino's actual JWT enforcement + group ACL is verified only against the live Keycloak+Trino stack — that is F2-P5 (real-Keycloak e2e), NOT this plan.** P3 delivers the code path + the Trino config artifacts; P5 proves enforcement. Daft-direct `read_sql` (kernel opens its own Trino connection) is a SEPARATE later phase (needs manifest connection-detail extension) — NOT in P3.

---

## File structure
- `crates/egret-engine/src/connector/mod.rs` — `ConnSpec.bearer_token`; widen `ConnectorFactory::build` + `from_database_as` with a `bearer_token` param.
- `crates/egret-engine/src/connector/trino.rs` — `TrinoConnector.bearer` field; `bearer_auth` at the 4 request sites; `bearer()` accessor; unit test.
- `crates/egret-kernel/src/query.rs` — widen `NotebookQueryRunner::run` with `bearer: Option<String>`; update `FakeQueryRunner`.
- `crates/egret-kernel/src/routes.rs` — `kernel_query` mints the engine JWT and passes it; map mint failure to a clear error.
- `crates/egret-bin/src/main.rs` — `EngineQueryRunner::run` passes `bearer` into `factory.build`.
- `deploy/trino/etc/{config.properties,access-control.properties,rules.json}` + `deploy/docker-compose.dev.yml` — Trino JWT auth + group ACL (validated at P5).

---

## Task 1: Thread a bearer JWT through the connector (TDD)

**Files:** `crates/egret-engine/src/connector/mod.rs`, `crates/egret-engine/src/connector/trino.rs`.

- [ ] **Step 1: `ConnSpec` gains `bearer_token`** (mod.rs, struct ~17-26):
```rust
    /// Per-user OIDC engine JWT presented to the engine as `Authorization: Bearer`.
    /// When set, it takes precedence over `secret` (basic-auth). `None` = legacy path.
    pub bearer_token: Option<String>,
```
Update `ConnSpec::from_database_as` (mod.rs ~28-50) to take a `bearer_token: Option<String>` param and set the field; `from_database` (the 2-arg convenience) passes `None`. Update `ConnectorFactory::build` (trait, mod.rs ~96-106) to add `bearer_token: Option<String>` and thread it into `from_database_as`; update `DefaultConnectorFactory::build` (mod.rs ~109-124) accordingly. Fix ALL callers of `build`/`from_database`/`from_database_as` (grep) — existing non-notebook callers pass `None`.

- [ ] **Step 2: Failing unit test** in trino.rs `mod tests` (mirror the existing `xtrino_user_is_session_user_only_when_impersonating` constructor test):
```rust
#[test]
fn bearer_token_takes_precedence_over_secret() {
    let s = ConnSpec {
        engine: Engine::Trino, host: "h".into(), port: 8080,
        options: ConnOptions { user: Some("svc".into()), ..Default::default() },
        secret: Some("basic-pw".into()),
        session_user: Some("alice".into()),
        bearer_token: Some("eyJ.jwt.sig".into()),
    };
    let c = TrinoConnector::new(&s);
    assert_eq!(c.bearer(), Some("eyJ.jwt.sig"));
    // still carries the session user for X-Trino-User
    assert_eq!(c.x_trino_user(), "alice");
}

#[test]
fn no_bearer_uses_basic_auth_path() {
    let s = ConnSpec {
        engine: Engine::Trino, host: "h".into(), port: 8080,
        options: ConnOptions::default(), secret: Some("pw".into()),
        session_user: None, bearer_token: None,
    };
    assert_eq!(TrinoConnector::new(&s).bearer(), None);
}
```

- [ ] **Step 3: Run — fails** `cargo test -p egret-engine bearer` → FAIL (field/accessor missing).

- [ ] **Step 4: Implement** in trino.rs:
  - Add field `bearer: Option<String>` to the struct; set it in `new` from `spec.bearer_token.clone()`.
  - Add accessor `pub fn bearer(&self) -> Option<&str> { self.bearer.as_deref() }`.
  - In `execute`, replace each `if let Some(s) = &self.secret { req = req.basic_auth(self.auth_user(), Some(s)); }` (the POST site ~92-102 and the GET site ~161-167) with:
```rust
    if let Some(jwt) = &self.bearer {
        req = req.bearer_auth(jwt);
    } else if let Some(s) = &self.secret {
        req = req.basic_auth(self.auth_user(), Some(s));
    }
```
  Keep the `X-Trino-User` header on all four sites unchanged (the session user is still sent; the JWT is the auth credential). The two DELETE sites (~140-145, ~152-157) set only `X-Trino-User` today and issue no auth — leave them, OR if they need auth too, add the same bearer/basic block (match whatever the POST does; if the cancel DELETE worked without auth before, keep it — do NOT change behavior beyond the auth swap).

- [ ] **Step 5: Run — passes** `cargo test -p egret-engine bearer` + `cargo build -p egret-engine`.

- [ ] **Step 6: Commit**
```bash
git add crates/egret-engine/src/connector/mod.rs crates/egret-engine/src/connector/trino.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(engine): TrinoConnector presents a bearer JWT (precedence over basic-auth)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Callback mints the user's engine JWT + passes it (TDD)

**Files:** `crates/egret-kernel/src/query.rs`, `crates/egret-kernel/src/routes.rs`, `crates/egret-bin/src/main.rs`.

- [ ] **Step 1: Widen the runner trait** in `crates/egret-kernel/src/query.rs`. `NotebookQueryRunner::run` gains `bearer: Option<String>`:
```rust
    async fn run(
        &self,
        database_id: uuid::Uuid,
        session_user: String,
        bearer: Option<String>,
        sql: String,
        max_rows: usize,
    ) -> Result<QueryRows, QueryRunError>;
```
Update `FakeQueryRunner::run` (query.rs ~50-73) to the new signature (record `bearer` if the fake records calls; otherwise just accept it). Keep it behind its `#[cfg(any(test, feature = "test-util"))]` gate.

- [ ] **Step 2: Failing test** — extend the kernel http test (`crates/egret-kernel/tests/http.rs`, the `/kernel/query` test that uses `FakeQueryRunner`). Assert that the handler mints a JWT and passes it: make `FakeQueryRunner` capture the `bearer` arg (add an `Arc<Mutex<Option<Option<String>>>>` last-call field), inject a minter that returns a known token (the harness already builds `KernelState` with an `engine_minter` — use a stub returning `access_token:"stub.jwt"`), POST `/api/kernel/query` with a valid kernel token, and assert the fake received `bearer == Some("stub.jwt")`. (If the existing harness's minter stub differs, adapt.)

- [ ] **Step 3: Run — fails** (handler doesn't mint yet / signature mismatch).

- [ ] **Step 4: Implement the handler** `kernel_query` in `crates/egret-kernel/src/routes.rs`. Before calling the runner, mint the user's engine JWT:
```rust
    let bearer = match st.engine_minter.mint(ident.user_id).await {
        Ok(tok) => Some(tok.access_token),
        Err(egret_notebook::engine_token::BrokerError::NoRefreshToken) => {
            // No stored offline token (e.g. a non-OIDC login) → cannot act as the
            // user against a JWT-authenticated engine.
            return Err(KernelApiError(egret_core::Error::Unauthorized));
        }
        Err(e) => return Err(KernelApiError(egret_core::Error::Internal(anyhow::anyhow!(e)))),
    };
    let rows = st.query_runner
        .run(eng.id, username, bearer, req.sql, max_rows)
        .await ...;
```
(Match the handler's existing error-construction style; confirm `KernelApiError` maps `Unauthorized`→401 and `Internal`→500. Keep `username` for `X-Trino-User`/session.)

- [ ] **Step 5: Implement `EngineQueryRunner::run`** in `crates/egret-bin/src/main.rs` (~282-329): accept `bearer: Option<String>` and pass it into the factory:
```rust
    let connector = self.factory.build(&db, secret, Some(session_user), bearer).await ...;
```
(The `factory.build` gained the `bearer` param in Task 1.)

- [ ] **Step 6: Run — passes** `cargo test -p egret-kernel --test http` (postgres up + `DATABASE_URL`) + `cargo build --workspace`.

- [ ] **Step 7: Commit**
```bash
git add crates/egret-kernel/src/query.rs crates/egret-kernel/src/routes.rs crates/egret-kernel/tests/http.rs crates/egret-bin/src/main.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel): kernel_query mints the user engine JWT + passes it to the connector

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Trino dev config — JWT auth + group ACL (deploy artifacts; enforcement verified at P5)

**Files:** `deploy/trino/etc/config.properties`, `deploy/trino/etc/access-control.properties`, `deploy/trino/etc/rules.json`, `deploy/docker-compose.dev.yml`.

> These are dev/config artifacts. They are NOT unit-tested; correctness is confirmed when the live stack is brought up in F2-P5. Provide sensible, documented defaults.

- [ ] **Step 1: `deploy/trino/etc/config.properties`** — a complete coordinator config for the single-node dev image, plus JWT auth against Keycloak:
```properties
coordinator=true
node-scheduler.include-coordinator=true
http-server.http.port=8080
discovery.uri=http://localhost:8080
# OIDC JWT authentication (Keycloak). JWKS discovered from the issuer.
http-server.authentication.type=JWT
http-server.authentication.jwt.key-file=http://keycloak:8080/realms/egret/protocol/openid-connect/certs
http-server.authentication.jwt.required-audience=trino
http-server.authentication.jwt.principal-field=sub
# Map the JWT principal to a Trino user name.
http-server.process-forwarded=true
```
(Note in a comment: the `key-file` URL is the in-compose-network Keycloak address; verify the exact host/port + realm at P5. Trino requires HTTPS for JWT auth by default unless `http-server.authentication.allow-insecure-over-http=true` — add that for the HTTP dev stack.)
Add: `http-server.authentication.allow-insecure-over-http=true` and (if Trino demands it) `internal-communication.shared-secret=<dev-secret>`.

- [ ] **Step 2: `deploy/trino/etc/access-control.properties`**:
```properties
access-control.name=file
security.config-file=/etc/trino/rules.json
```

- [ ] **Step 3: `deploy/trino/etc/rules.json`** — a starter file-based ACL keyed on groups (documented as a template refined at P5):
```json
{
  "catalogs": [
    { "group": "data-team", "allow": "all" },
    { "user": ".*", "allow": "read-only" }
  ]
}
```
(Comment: Trino derives groups from a configured group provider or the JWT `groups` claim; wiring groups from the JWT is finalized at P5. This starter grants `data-team` full access, everyone else read-only.)

- [ ] **Step 4: Mount the config in compose.** In `deploy/docker-compose.dev.yml` trino service, add the etc mount alongside the existing catalog mount. Because bind-mounting over `/etc/trino` hides the image's `node.properties`/`jvm.config`, mount the individual files instead:
```yaml
    volumes:
      - ./trino/catalog:/etc/trino/catalog
      - ./trino/etc/config.properties:/etc/trino/config.properties
      - ./trino/etc/access-control.properties:/etc/trino/access-control.properties
      - ./trino/etc/rules.json:/etc/trino/rules.json
```
(Comment in the compose: enforcement + exact property set is validated in F2-P5; if Trino fails to boot with the overridden `config.properties`, P5 reconciles it against the image defaults.)

- [ ] **Step 5: Sanity** — `docker compose -f deploy/docker-compose.dev.yml config >/dev/null` (compose file parses). Do NOT block on Trino booting green here — that is P5.

- [ ] **Step 6: Commit**
```bash
git add deploy/trino/etc/config.properties deploy/trino/etc/access-control.properties deploy/trino/etc/rules.json deploy/docker-compose.dev.yml
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(deploy): Trino JWT auth + file group ACL config (enforcement verified in P5)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Full local CI + P5 handoff note

- [ ] **Step 1:** `cargo fmt --all --check` → clean (run `cargo fmt --all` + commit if needed).
- [ ] **Step 2:** `cargo clippy --workspace --all-targets -- -D warnings` → clean.
- [ ] **Step 3:** `cargo test --workspace` (postgres + minio up, `warehouse` bucket, `DATABASE_URL`) → green except the known infra-flaky `egret-catalog-engine` MinIO tests + pre-existing `egret-engine` open_secret NULL-ws tests + the `#[ignore]`d live-Trino tests. Any failure in `egret-engine` (connector bearer), `egret-kernel`, or `egret-bin` (non-infra) = BLOCKER.
- [ ] **Step 4:** Confirm the new connector bearer unit tests + the kernel_query mint test pass (paste the lines).
- [ ] **Step 5:** Document (in the PR body) that Trino's JWT enforcement + group ACL + the `X-Trino-User`/JWT-principal interplay + the exact `config.properties` property set are validated end-to-end in **F2-P5** against the live Keycloak+Trino stack; and that Daft-direct `read_sql` remains a separate future phase.

---

## Self-review
- **Spec coverage (F2 §4.4 — Trino JWT auth + group ACL, Option A):** connector bearer (T1), callback mints + presents the user's JWT dropping reliance on impersonation for auth (T2), Trino JWT + file ACL config (T3). Enforcement e2e explicitly deferred to P5; Daft-direct explicitly deferred. ✔
- **Placeholder scan:** T3 config values (Keycloak host/port, rules.json groups) are documented dev defaults refined at P5 — these are config artifacts, not logic placeholders. All Rust logic + signatures fully specified.
- **Type consistency:** `ConnSpec.bearer_token: Option<String>` → `ConnectorFactory::build(db, secret, session_user, bearer_token)` → `TrinoConnector.bearer` + `bearer()` accessor; `NotebookQueryRunner::run(database_id, session_user, bearer, sql, max_rows)`; mint via `EngineTokenMinter::mint(user_id) -> EngineToken.access_token`; `BrokerError::NoRefreshToken → 401`. Used consistently across T1/T2.
- **Security:** the connector prefers the bearer JWT over basic-auth; the JWT is the user's own short-lived engine token (P1); a user with no engine token (non-OIDC login) is refused (401) rather than silently querying as a service principal. `X-Trino-User` retained for session identity only. No token logged (do not add logging of `bearer`/`access_token`).
