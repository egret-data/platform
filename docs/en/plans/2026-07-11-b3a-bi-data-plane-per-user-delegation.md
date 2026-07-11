# B3a: BI data-plane per-user delegation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the BI `run_query` path present the caller's per-user engine JWT (minted via F2's `EngineTokenMinter`) as the connector bearer for connections marked `delegated`, so BI queries run under the engine's per-user authz.

**Architecture:** A per-connection `delegated` flag on `databases`. `run_query` (and the `JobManager` submit path) gain a `bearer_token` parameter. A `resolve_delegation` helper (egret-bi) and the SQL Lab submit handler (egret-query) mint the caller's JWT when the target connection is delegated and pass `(session_user = username, bearer = jwt)`; a non-OIDC user on a delegated connection is denied (403). Non-delegated connections keep the current shared-service behavior (`bearer = None`). Reuses F2's connector bearer support (`TrinoConnector`/`StarRocksConnector` already use `bearer_token`).

**Tech Stack:** Rust (axum, sqlx), `EngineTokenMinter` (egret-analytics-api), React 18 + orval + Vitest.

**Spec:** `docs/en/specs/2026-07-11-b3a-bi-data-plane-per-user-delegation-design.md`.

**Conventions (read before starting):**
- Run cargo from repo root `/Users/1pro-l4r8/Documents/github/data-portal/egret`. DB tests need `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret`.
- After adding a migration, `touch crates/egret-metastore/src/lib.rs` (sqlx re-embed) before running tests that migrate.
- RTK proxy filter SILENTLY DROPS passing test binaries + rewrites `cargo test`. For an accurate full count, run `cargo test ...` via a shell-script file (not `rtk proxy cargo test`) and read the file; use `rtk proxy` for single-test/grep.
- Git identity configured — do NOT override author. End every commit body with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Never run two `cargo test --workspace` at once.

**Key facts (verified in the code):**
- `run_query(databases, factory, database_id, sql, limit, session_user: Option<String>, limits, cancel)` at `crates/egret-query/src/execution/job.rs:172` calls `factory.build(&db, secret, session_user, None)`. The 4th `build` arg is `bearer_token: Option<String>` (already on the trait, `execution/connector/mod.rs:106`); connectors already use it.
- The three egret-bi object-scoped data endpoints call `run_query` at `crates/egret-bi/src/routes.rs` **1039** (dataset `ds_data`), **1808** (chart `ch_data`-object / `POST /charts/{id}/data`), **2093** (dashboard card `POST /dashboards/{id}/cards/{card_id}/data`). All pass `session_user = Some(ctx.principal.email.clone())` today.
- SQL Lab submit: `EngineState::submit` (`crates/egret-query/src/execution/routes.rs:106`) → `JobManager::submit` (`job.rs:56`) → `run` (`87`) → `execute` (`116`) → `run_query`. `EngineState` already has `users: UserRepo` + `databases` + `sessions`; it does NOT yet have a minter, and egret-query does NOT dep egret-analytics-api.
- `UserRepo::ensure_username(user_id) -> Result<String>` at `crates/egret-auth/src/users.rs:145` — the Keycloak `preferred_username`.
- The F2 minter Arc is built in `crates/egret-bin/src/main.rs` (~line 1079) AFTER `BiState` (~1060); it is injected into `NotebookState`/`KernelState`.
- `EngineToken { access_token, expires_at }` + `EngineTokenMinter::mint(user_id) -> Result<EngineToken, BrokerError>` + `BrokerError::NoRefreshToken` in `crates/egret-analytics-api/src/engine_token.rs`.

---

## File structure

- `migrations/00NN_database_delegated.sql` (create) — the `delegated` column.
- `crates/egret-query-api/src/lib.rs` (modify) — `Database.delegated`.
- `crates/egret-query/src/registry/model.rs` (modify) — `DatabaseInput`/`DatabaseDetailDto`/`DatabaseListDto` `delegated`.
- `crates/egret-query/src/registry/repo.rs` (modify) — SELECT/INSERT/UPDATE the column.
- `crates/egret-query/src/execution/job.rs` (modify) — `run_query` + `JobManager` `bearer_token`.
- `crates/egret-bi/src/routes.rs` (modify) — `resolve_delegation` helper + the 3 call sites; `BiState` += `engine_minter` + `users`.
- `crates/egret-query/src/execution/routes.rs` (modify) — `EngineState` += `engine_minter`; submit resolves delegation.
- `crates/egret-query/Cargo.toml` (modify) — dep egret-analytics-api.
- `crates/egret-bin/src/main.rs` (modify) — reorder minter above BiState; inject into BiState + EngineState.
- `frontend/src/routes/.../DatabasesPage` (modify) — delegated toggle; regen client.

---

## Task 1: The `delegated` column + Database model

**Files:**
- Create: `migrations/00NN_database_delegated.sql` (use the next free number; repo is past 0074)
- Modify: `crates/egret-query-api/src/lib.rs`, `crates/egret-query/src/registry/model.rs`, `crates/egret-query/src/registry/repo.rs`, `crates/egret-metastore/src/lib.rs` (touch)
- Test: `crates/egret-query/tests/` (extend a registry/db test — find one that creates a database via `DatabaseRepo`)

- [ ] **Step 1: Write the migration**

Create `migrations/00NN_database_delegated.sql` (replace `NN` with the next number after 0074):

```sql
-- B3a: per-connection per-user delegation. When true, BI queries against this
-- connection run as the caller's per-user engine JWT (delegation) instead of the
-- shared service credentials. Default false = current shared-service behavior.
ALTER TABLE databases ADD COLUMN delegated boolean NOT NULL DEFAULT false;
```

- [ ] **Step 2: Touch the metastore lib**

Run: `touch crates/egret-metastore/src/lib.rs`

- [ ] **Step 3: Add `delegated` to the `Database` domain type**

In `crates/egret-query-api/src/lib.rs`, add to `struct Database` (after `status`):

```rust
    pub status: String,
    /// B3a: run BI queries as the caller's per-user engine JWT (delegation).
    #[serde(default)]
    pub delegated: bool,
```

- [ ] **Step 4: Add `delegated` to the DTOs + input**

In `crates/egret-query/src/registry/model.rs`: add `#[serde(default)] pub delegated: bool` to `DatabaseInput` (create/update body), and `pub delegated: bool` to `DatabaseDetailDto` and `DatabaseListDto`. Match the existing field style (camelCase serde if the DTOs use it — check the struct's `#[serde(rename_all)]`).

- [ ] **Step 5: Thread `delegated` through the repo**

In `crates/egret-query/src/registry/repo.rs`: add `delegated` to every `SELECT ... FROM databases` column list that builds a `Database`/`DatabaseDetailDto`/`DatabaseListDto`; add it to the INSERT (create) and UPDATE (edit) column lists + bind the input value. Grep the file for `INSERT INTO databases` / `SELECT` / `UPDATE databases` and update each. Map the sqlx `bool` column to the struct field.

- [ ] **Step 6: Write the test**

Find an existing `DatabaseRepo` test (grep `DatabaseRepo::new` / `.create(` under `crates/egret-query/tests/`). Add a test that creates a database with `delegated: true` in the input and asserts `get(id).delegated == true`, and that the default (omitted) is `false`. Mirror the file's harness (DATABASE_URL self-skip).

- [ ] **Step 7: Run + build + commit**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-query --test <the-test-file>`
Expected: PASS.
Run: `rtk proxy cargo build --workspace` clean; `rtk proxy cargo clippy -p egret-query -p egret-query-api --all-targets -- -D warnings` clean; `cargo fmt` touched crates.

```bash
git add migrations/00NN_database_delegated.sql crates/egret-query-api/src/lib.rs crates/egret-query/src/registry/ crates/egret-metastore/src/lib.rs crates/egret-query/tests/
git commit -m "feat(query): add per-connection delegated flag to databases"
```

---

## Task 2: Thread `bearer_token` through `run_query` + `JobManager`

Pure plumbing — every caller passes `None`, so behavior is byte-identical. This lets Tasks 3 and 4 pass a real bearer.

**Files:**
- Modify: `crates/egret-query/src/execution/job.rs`
- Test: `crates/egret-query/tests/execution_job.rs` (extend)

- [ ] **Step 1: Add `bearer_token` to `run_query`**

In `crates/egret-query/src/execution/job.rs`, change `run_query`'s signature to add `bearer_token: Option<String>` right after `session_user`, and pass it to `factory.build`:

```rust
pub async fn run_query(
    databases: &DatabaseRepo,
    factory: &dyn ConnectorFactory,
    database_id: Uuid,
    sql: &str,
    limit: Option<usize>,
    session_user: Option<String>,
    bearer_token: Option<String>,
    limits: &Limits,
    cancel: CancellationToken,
) -> Result<TabularResult, EngineError> {
    // ... unchanged up to the build call ...
    let connector = factory.build(&db, secret, session_user, bearer_token).await?;
    // ... unchanged ...
}
```

- [ ] **Step 2: Thread `bearer_token` through `JobManager` submit → run → execute**

In the same file, add `bearer_token: Option<String>` to `JobManager::submit`, `run`, and `execute` (after `session_user`), and pass it down. `submit` moves it into the spawned task (String is `Send + 'static`):

```rust
    pub async fn submit(
        &self,
        ws: Uuid,
        database_id: Uuid,
        user: Option<Uuid>,
        session_user: Option<String>,
        bearer_token: Option<String>,
        sql: &str,
        limit: Option<usize>,
    ) -> Result<Uuid, EngineError> {
        let id = self.store.insert(ws, database_id, user, sql).await?;
        let token = CancellationToken::new();
        self.cancels.lock().unwrap().insert(id, token.clone());
        let this = self.clone();
        let run_sql = sql.to_string();
        tokio::spawn(async move {
            this.run(id, database_id, run_sql, limit, session_user, bearer_token, token)
                .await;
            this.cancels.lock().unwrap().remove(&id);
        });
        Ok(id)
    }
```

Add `bearer_token: Option<String>` to `run` (pass to `execute`) and to `execute` (pass to `run_query` in the new slot). The internal `run_query` call in `execute` becomes:

```rust
        let result = run_query(
            &self.databases,
            &*self.factory,
            database_id,
            sql,
            limit,
            session_user,
            bearer_token,
            &self.limits,
            token.clone(),
        )
        .await?;
```

- [ ] **Step 3: Fix all `run_query` / `JobManager::submit` call sites to pass `None`**

Run: `rtk proxy grep -rn "run_query(\|\.submit(" crates/ | grep -v "fn run_query\|fn submit"`
Every existing caller of `run_query` (egret-bi routes.rs 1039/1808/2093 — Task 3 changes these, but pass `None` NOW to compile) and `JobManager::submit` (egret-query routes.rs submit handler — Task 4 changes it, pass `None` now) gets `None` inserted in the new `bearer_token` slot. Update test call sites too (`execution_job.rs`, `execution_limit.rs`, etc.).

- [ ] **Step 4: Extend the test**

In `crates/egret-query/tests/execution_job.rs`, add a test using the file's fake `ConnectorFactory` that captures its args: assert `run_query(..., session_user=Some("u"), bearer_token=Some("tok"), ...)` results in `factory.build` receiving `bearer_token == Some("tok")`. If the fake factory doesn't capture the bearer, extend it to record the last `bearer_token` it was built with.

- [ ] **Step 5: Run + build + commit**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-query`
Expected: PASS (all existing tests unchanged behavior + the new bearer test).
Run: `rtk proxy cargo build --workspace` clean; clippy/fmt.

```bash
git add crates/egret-query/src/execution/job.rs crates/egret-query/tests/ crates/egret-bi/src/routes.rs
git commit -m "refactor(query): thread bearer_token through run_query and JobManager"
```
(egret-bi routes.rs is included only for the `None` args added in Step 3.)

---

## Task 3: `resolve_delegation` + wire the 3 egret-bi endpoints

**Files:**
- Modify: `crates/egret-bi/src/routes.rs` (BiState + helper + 3 call sites)
- Modify: `crates/egret-bin/src/main.rs` (reorder minter above BiState; inject)
- Test: `crates/egret-bi/tests/` (new `delegation.rs`)

- [ ] **Step 1: Add `engine_minter` + `users` to `BiState`**

In `crates/egret-bi/src/routes.rs`, add to `struct BiState`:

```rust
    pub users: egret_auth::users::UserRepo,
    pub engine_minter: std::sync::Arc<dyn egret_analytics_api::engine_token::EngineTokenMinter>,
```

(Confirm the exact path of the `EngineTokenMinter` trait — it is re-exported from `egret_notebook::engine_token` too; use whichever egret-bi can resolve. Add the crate dep to `crates/egret-bi/Cargo.toml` if missing: `egret-analytics-api` and/or `egret-auth` — egret-bi already deps egret-auth for `SessionService`.)

- [ ] **Step 2: Write the `resolve_delegation` helper**

Add to `crates/egret-bi/src/routes.rs`:

```rust
/// For a target database, resolve the `(session_user, bearer_token)` to run a BI
/// query with. Non-delegated connections keep the current behavior (caller email
/// as session_user, no bearer). Delegated connections mint the caller's per-user
/// engine JWT and use the caller's username (= the JWT `preferred_username`, which
/// the engine matches against the bearer principal); a non-OIDC caller is denied.
async fn resolve_delegation(
    st: &BiState,
    ctx: &egret_core::WorkspaceContext,
    database_id: Uuid,
) -> Result<(Option<String>, Option<String>), BiApiError> {
    let db = st
        .databases
        .get(database_id)
        .await
        .map_err(|e| BiApiError::from(crate::BiError::from(e)))?;
    if !db.delegated {
        return Ok((Some(ctx.principal.email.clone()), None));
    }
    let token = st
        .engine_minter
        .mint(ctx.principal.user_id.0)
        .await
        .map_err(|e| match e {
            egret_analytics_api::engine_token::BrokerError::NoRefreshToken => BiApiError::from(
                egret_core::Error::Forbidden,
            ),
            other => BiApiError::from(egret_core::Error::Internal(anyhow::anyhow!(other.to_string()))),
        })?;
    let username = st
        .users
        .ensure_username(ctx.principal.user_id)
        .await
        .map_err(BiApiError::from)?;
    Ok((Some(username), Some(token.access_token)))
}
```

(`egret_core::Error::Forbidden` is a unit variant → 403; `BiApiError: From<egret_core::Error>` exists, same mapping the authz `require` path uses.)

- [ ] **Step 3: Wire the 3 call sites**

At each of the three `run_query` calls (routes.rs 1039, 1808, 2093), replace the `Some(ctx.principal.email.clone())` argument + insert the bearer. Before each `run_query`, add:

```rust
    let (session_user, bearer) = resolve_delegation(&st, &ctx, database_id).await?;
```

(use `ds.database_id` at the dataset site, `database_id` from `compile_chart_sql` at the chart/card sites) and change the `run_query` call to pass `session_user, bearer` in the `session_user` + new `bearer_token` slots:

```rust
    let result = egret_query::execution::run_query(
        &st.databases,
        &egret_query::execution::connector::DefaultConnectorFactory,
        database_id,
        &sql,
        Some(limit as usize),
        session_user,
        bearer,
        &chart_run_limits(),
        tokio_util::sync::CancellationToken::new(),
    )
```

- [ ] **Step 4: Inject the minter + users into BiState (egret-bin)**

In `crates/egret-bin/src/main.rs`: move the `engine_minter` construction (`EngineTokenBroker`/`KeycloakRefresher`, ~line 1079) to ABOVE the `BiState { ... }` construction (~line 1060) — mirror how B1b-3 relocated the pdp/bus block. Add `users: <the UserRepo already built>, engine_minter: engine_minter.clone(),` to the `BiState { ... }` literal. (A `UserRepo` is already built in main.rs for other states — reuse it; grep `UserRepo::new`.)

- [ ] **Step 5: Write the test**

Create `crates/egret-bi/tests/delegation.rs`. Mirror an existing egret-bi object-scope data test (e.g. `chart_data_object_scope.rs` or `dataset_preview.rs`) but build `BiState` with a **fake `EngineTokenMinter`**:

```rust
struct FakeMinter { token: Option<String> } // Some → mint ok; None → NoRefreshToken
#[async_trait::async_trait]
impl egret_analytics_api::engine_token::EngineTokenMinter for FakeMinter {
    async fn mint(&self, _user: uuid::Uuid)
        -> Result<egret_analytics_api::engine_token::EngineToken, egret_analytics_api::engine_token::BrokerError> {
        match &self.token {
            Some(t) => Ok(egret_analytics_api::engine_token::EngineToken {
                access_token: t.clone(),
                expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
            }),
            None => Err(egret_analytics_api::engine_token::BrokerError::NoRefreshToken),
        }
    }
}
```

Assert, against a seeded **delegated** database (set `delegated=true` on the DB row) with a fake connector factory that records its `bearer_token`:
1. A `POST /datasets/{id}/data` (or chart/dashboard) as an OIDC user → the connector is built with `bearer == Some(<the fake token>)` and `session_user == <the seeded username>`.
2. With `FakeMinter { token: None }` → the endpoint returns **403**.
3. Against a **non-delegated** database → the connector is built with `bearer == None` and `session_user == <caller email>`.

(If wiring a fake connector factory into the object-scoped endpoints is hard because they hard-code `DefaultConnectorFactory`, instead assert at the `resolve_delegation` level: call it directly with a delegated vs non-delegated DB and the fake minter, asserting the returned tuple. Prefer the endpoint-level test if the harness allows; otherwise the helper-level test is acceptable and must cover all 3 branches.)

- [ ] **Step 6: Regen OpenAPI + run + commit**

Run: `rtk proxy cargo run -p egret-bin -- openapi > openapi.json` (the `delegated` field appears on the database DTOs).
Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-bi`
Expected: PASS.
Run: `rtk proxy cargo build --workspace` clean; clippy/fmt.

```bash
git add crates/egret-bi/ crates/egret-bin/src/main.rs openapi.json
git commit -m "feat(bi): per-user delegation on object-scoped data endpoints"
```

---

## Task 4: SQL Lab submit delegation (egret-query)

**Files:**
- Modify: `crates/egret-query/Cargo.toml` (dep egret-analytics-api)
- Modify: `crates/egret-query/src/execution/routes.rs` (EngineState + submit)
- Modify: `crates/egret-bin/src/main.rs` (inject minter into EngineState)
- Test: `crates/egret-query/tests/execution_http.rs` (extend)

- [ ] **Step 1: Add the dep + minter field**

In `crates/egret-query/Cargo.toml`, add `egret-analytics-api = { path = "../egret-analytics-api" }` (check the exact path/version style of sibling deps).
In `crates/egret-query/src/execution/routes.rs`, add to `EngineState`:

```rust
    pub engine_minter: std::sync::Arc<dyn egret_analytics_api::engine_token::EngineTokenMinter>,
```

- [ ] **Step 2: Resolve delegation in the submit handler**

In the `submit` handler (`routes.rs:106`), after the `authorize(...)` check and before `st.jobs.submit(...)`, resolve delegation for the target database (mirror egret-bi's helper, adapted to `EngineState` — it has `databases`, `users`, `sessions`):

```rust
    let db = st.databases.get(database_id).await.map_err(EngineApiError::from)?;
    let (session_user, bearer) = if db.delegated {
        let token = st.engine_minter.mint(ctx.principal.user_id.0).await.map_err(|e| match e {
            egret_analytics_api::engine_token::BrokerError::NoRefreshToken => EngineApiError::from(egret_core::Error::Forbidden),
            other => EngineApiError::from(egret_core::Error::Internal(anyhow::anyhow!(other.to_string()))),
        })?;
        let username = st.users.ensure_username(ctx.principal.user_id).await.map_err(EngineApiError::from)?;
        (Some(username), Some(token.access_token))
    } else {
        (Some(ctx.principal.email.clone()), None)
    };
```

Then pass `session_user, bearer` into `st.jobs.submit(ws, database_id, user, session_user, bearer, sql, limit)`. (Confirm the current `submit` call's exact args — the handler builds `session_user` from the principal today; replace that with the resolved tuple.)

- [ ] **Step 3: Inject the minter into EngineState (egret-bin)**

In `crates/egret-bin/src/main.rs`, add `engine_minter: engine_minter.clone(),` to the `EngineState { ... }` construction (the minter is now built above BiState from Task 3, so it is in scope; if EngineState is built before it, move it up as well — one shared Arc for BiState/EngineState/NotebookState/KernelState).

- [ ] **Step 4: Write the test**

In `crates/egret-query/tests/execution_http.rs`, using the same `FakeMinter` pattern (Task 3 Step 5), add: a `POST /ws/{ws}/queries` against a **delegated** database with an OIDC user → the submitted job's connector is built with the bearer (or, if asserting the async job result is hard, assert the submit succeeds and — via a fake factory capturing the bearer — the bearer is `Some`); with `FakeMinter { token: None }` → **403**; against a non-delegated database → no bearer. Reuse the file's app-builder; inject the fake minter into `EngineState`.

- [ ] **Step 5: Run + build + commit**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-query`
Expected: PASS.
Run: `rtk proxy cargo build --workspace` clean; clippy/fmt.

```bash
git add crates/egret-query/Cargo.toml crates/egret-query/src/execution/routes.rs crates/egret-bin/src/main.rs crates/egret-query/tests/execution_http.rs
git commit -m "feat(query): per-user delegation on SQL Lab submit"
```

---

## Task 5: Frontend — delegated toggle + client regen

**Files:**
- Modify: the DatabasesPage connection form (locate via search)
- Modify: generated orval client (regen)
- Test: the DatabasesPage test

- [ ] **Step 1: Regen the api client**

Run: `cd frontend && rtk proxy npm run gen:api`. Confirm the generated database DTOs/input carry `delegated`.

- [ ] **Step 2: Add the toggle (TDD)**

Locate the DatabasesPage connection create/edit form: `grep -rn "engine\|DatabaseInput\|useDatabases\|/databases" frontend/src/routes frontend/src --include=*.tsx | grep -i database`. Add a test to its `*.test.tsx` asserting the form renders a "Per-user delegation" switch bound to `delegated` and includes it in the submit payload. Run it, confirm it fails.

- [ ] **Step 3: Implement the toggle**

Add a labeled switch/checkbox "Per-user delegation (run queries as the signed-in user)" bound to the form's `delegated` field (default `false`), mirroring the form's existing boolean fields (e.g. any ssl/impersonate toggle). Include `delegated` in the create/update mutation payload. Use shadcn `Switch`/`Checkbox` + OKLCH tokens per the existing form.

- [ ] **Step 4: FE gate + commit**

Run (from `frontend`): `rtk proxy npm run lint && rtk proxy npm run typecheck && rtk proxy npm run test && rtk proxy npm run build` — all pass.

```bash
git add frontend/
git commit -m "feat(fe): per-user delegation toggle on database connections"
```

---

## Task 6: Verify — full CI gate

**Files:** none (verification; fix forward if a regression appears).

- [ ] **Step 1: Backend fmt + clippy**

Run: `rtk proxy cargo fmt --all --check` (if it fails, `cargo fmt --all` + note).
Run: `rtk proxy cargo clippy --workspace --all-targets -- -D warnings` — clean (ignore the pre-existing third-party `mysql_async` warning).

- [ ] **Step 2: Full workspace test (accurate count)**

Write `/tmp/b3a.sh`:
```sh
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret
cargo test --workspace --no-fail-fast 2>&1 | tail -80
```
Run: `zsh /tmp/b3a.sh`
Expected: ALL pass. Known-benign: the 2 engine `open_secret` NULL-ws tests may fail on a truly fresh `egret_test` (pre-existing, unrelated). Any OTHER failure is a real regression — fix forward.

- [ ] **Step 3: Behavior-preservation check (non-delegated path)**

Confirm the non-delegated path is byte-identical: `rtk proxy cargo test -p egret-query -p egret-bi` pass, and the existing connector tests (`connector_trino`, `connector_starrocks`) are green (bearer only attached when passed).

- [ ] **Step 4: OpenAPI currency**

Run: `rtk proxy cargo run -p egret-bin -- openapi > /tmp/openapi_b3a.json && git diff --no-index openapi.json /tmp/openapi_b3a.json`
Expected: NO diff (Task 3 committed it). If a diff exists, overwrite `openapi.json`, `cd frontend && npm run gen:api`, commit both.

- [ ] **Step 5: Frontend gate**

Run (from `frontend`): `rtk proxy npm run lint && rtk proxy npm run typecheck && rtk proxy npm run test && rtk proxy npm run build` — all pass.

- [ ] **Step 6: Commit any fix-forward**

If Steps 1–5 needed a fix: `git add -A && git commit -m "chore(authz): B3a CI gate fixes"`. Otherwise report GREEN.

---

## Notes for the security review (after Task 6)

Focus: delegation-not-impersonation (the bearer IS the caller — egret holds no privileged act-as cred on the BI path); no bypass (non-OIDC on a delegated connection → 403, not a silent service-cred fallback); `session_user` == the JWT `preferred_username` via `ensure_username` (the engine rejects a mismatch); a shared-object viewer runs as themselves (the object-share never widens data-plane access — the engine gates the data); the JWT is short-lived, minted per-request, never persisted by the BI/query layer, redacted in logs; non-delegated connections are unchanged (no new surface). Known limitation to note (not a blocker): a viewer denied by the engine's data authz surfaces the connector's `EngineError` (mapping it to a friendly 403 is best-effort — the concrete 403 is the mint-failure path).
