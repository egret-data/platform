# Data API P5a — Backend Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build every server-side capability the Data API management UI needs — Activity statistics, generated OpenAPI, the Try-it proxy, and the audit/authz gaps — so the frontend plan that follows has nothing left to invent.

**Architecture:** All new routes join the existing `management_router()` in `crates/egret-dataapi/src/routes.rs`, gated through `WsContext` + `egret_core::authz::require` exactly as the P2/P3 routes are. One migration adds three columns to `dataapi.access_log`. The single most important structural change is extracting the invoke check into one function that the serving path, the OpenAPI generator and the Access checklist all call — three consumers, one expression.

**Tech Stack:** Rust, axum 0.8, sqlx 0.9 (Postgres), utoipa, tower (`ServiceExt::oneshot`), Cedar via `egret_core::authz::Pdp`.

Implements `docs/en/specs/2026-08-16-data-api-p5-design.md` §4, §5, §6.1, §7 — that is PRs 1 and 2 of the spec's §9 decomposition. Frontend (PRs 3–7) is a separate plan written after this merges.

## Global Constraints

- **sqlx 0.9: `sqlx::query()` takes `&'static str`.** Every statement in this plan is a fixed literal. Optional filters are expressed as `($n::type IS NULL OR col = $n)` predicates, never by string-building. Do not reach for `AssertSqlSafe`.
- **Time ranges are computed in Rust and bound as `timestamptz`.** Never interpolate an interval into SQL.
- **DB-gated tests self-skip when `DATABASE_URL` is unset and report `ok` while asserting nothing.** Run them with `DATABASE_URL` pointed at `egret_test` — never a dev database, which cannot be pointed at a running server without breaking its boot — and check the *duration*, not just the `ok`.
- **`cargo test --workspace` stops at the first failing binary.** When something fails, that is not the whole picture.
- **Adding a `#[utoipa::path]` changes the committed `openapi.json`**, which is asserted byte-identical by `committed_openapi_json_is_current` in `crates/egret-server/src/lib.rs:429` — a *different* crate from the one being edited. Regenerate with `egret openapi > openapi.json` and commit it in the same commit.
- **The workspace always comes from `WsContext`**, never from re-reading the `{workspace}` path segment. Mismatch answers 404, never 403.
- **Permission constants** (`crates/egret-dataapi/src/routes.rs:91-118`): `SA_READ`/`SA_MANAGE` = `service_account:read`/`:manage`; `EP_READ`/`EP_MANAGE` = `data_api_endpoint:read`/`:manage`; `EP_INVOKE` = `data_api_endpoint:invoke`; `OBJECT_KIND` = `data_api_endpoint` (`endpoints.rs:303`).
- **Never put a `client_secret` in an audit detail.** `crates/egret-dataapi/tests/audit.rs` asserts crate-wide that neither the key nor the secret's bytes appear.

## File Structure

| File | Responsibility |
|---|---|
| `migrations/0125_dataapi_access_log_p5.sql` | Create: three columns on `dataapi.access_log` |
| `crates/egret-dataapi/src/serving/access_log.rs` | Modify: three fields on `AccessLogEntry`, three binds in `record` |
| `crates/egret-dataapi/src/serving/router.rs` | Modify: populate the new fields; call the shared invoke check |
| `crates/egret-dataapi/src/invoke.rs` | Create: `can_invoke` — the one invoke expression |
| `crates/egret-dataapi/src/activity.rs` | Create: `ActivityStore` — summary and call-list queries |
| `crates/egret-dataapi/src/openapi_gen.rs` | Create: build an OpenAPI 3.0.3 document from live snapshots |
| `crates/egret-dataapi/src/try_it.rs` | Create: the `/try` handler — mint, drive the real router, audit |
| `crates/egret-dataapi/src/routes.rs` | Modify: mount the new routes, add the four missing audit events |
| `crates/egret-dataapi/src/serving/error.rs` | Modify: classify engine permission failures as `ranger_denied` |
| `crates/egret-bi/src/routes.rs:3375` | Modify: `kind_perms` arm for `data_api_endpoints` |
| `crates/egret-dataapi/tests/activity.rs` | Create: §4 tests |
| `crates/egret-dataapi/tests/openapi_gen.rs` | Create: §5 tests |
| `crates/egret-dataapi/tests/try_it.rs` | Create: §3 tests |

New modules rather than more of `routes.rs`, which is already 71.9K and holds the P2/P3 surface. Each new file owns one capability; `routes.rs` keeps only the thin handlers that mount them.

## Fixture additions (do these first)

Every test in this plan builds on `crates/egret-dataapi/tests/serving_common/mod.rs`, which today exposes `db`, `serving`, `mgmt`, `session`, `ws_slug`, `ws_id`, `principal_a`, `principal_b`, `admin`, `database_id`, `delegated_database_id`, `impersonating_database_id`, `port`, `clock`, plus `call_as_client` and a private `mgmt_send`. The tasks below need more. Add all of it in one commit before Task 1, so no task is blocked on a helper introduced by another:

**New `Fixture` fields**
- `state: DataApiState` — clone it *before* `.with_state(st)` consumes the value.
- `endpoint_id: Uuid` — the endpoint the fixture publishes.
- `author: Uuid` — the human whose session `session` belongs to; already a local in the builder, just carry it out.

**New helpers**
- `pub async fn mgmt_get(&self, path: &str) -> (StatusCode, Value)`, `mgmt_post(&self, path, Value)`, `mgmt_delete(&self, path)` — thin wrappers over the existing private `mgmt_send`, which already sets the session cookie.
- `pub async fn log_row(&self, status: i32, duration_ms: i32, cache_hit: bool, error_code: Option<&str>)` — insert straight into `dataapi.access_log` for `self.ws_id` and `self.endpoint_id`. Shaping a population by direct insert rather than by driving N real requests is deliberate: the statistics under test are about the *query*, and ten real calls would also be ten chances for something unrelated to change the numbers.
- `pub async fn log_row_as_trial(&self, status: i32, duration_ms: i32, actor: Uuid)` — same, with `actor_user_id` set.
- `pub async fn log_row_in(&self, ws: Uuid, status: i32, duration_ms: i32, request_id: Option<&str>)` — for the cross-workspace test.
- `pub async fn make_workspace(&self, slug: &str) -> Uuid`.
- `pub async fn create_draft_endpoint(&self, name: &str, path: &str) -> Uuid` — through the real `POST …/endpoints` route, not a direct insert.
- `pub async fn create_service_account(&self, name: &str) -> Uuid` — likewise through the real route.
- `FakePort::fail_next_with(&self, msg: &str)` — makes the next `execute_as*` return an error carrying `msg` (Task 8).

Because `log_row` writes columns added by Task 1's migration, this fixture commit must land **after** the migration file exists — write the migration first, then the fixture, then Task 1's test.

---

### Task 1: Migration and the three access-log columns

**Files:**
- Create: `migrations/0125_dataapi_access_log_p5.sql`
- Modify: `crates/egret-dataapi/src/serving/access_log.rs:73-122` (struct + `new`), `:200-236` (`record`)
- Modify: `crates/egret-dataapi/src/serving/router.rs:379-400` (`serve`)
- Test: `crates/egret-dataapi/tests/activity.rs` (new file, first test)

**Interfaces:**
- Produces: `AccessLogEntry { actor_user_id: Option<Uuid>, request_method: Option<String>, request_path: Option<String>, .. }` — Task 5 reads all three, Task 7 sets `actor_user_id`.

- [ ] **Step 1: Write the migration**

```sql
-- Data API P5: what the management UI needs the access log to remember.
--
-- Three nullable columns, all nullable for the same reason: every row written
-- before this migration has no answer, and inventing one would make historical
-- rows lie about which caller they came from.
--
-- `actor_user_id` — non-null means "run from Try-it, by this person". It does
-- two jobs at once: statistics filter it OUT (a trial is not consumer traffic
-- and would pollute every percentile), while the call list shows it as the
-- audit trail of an impersonation action.
--
-- No FK, exactly as `endpoint_id` and `service_account_id` in 0123: the log must
-- survive the deletion of what it describes. A dangling uuid here is the normal
-- end state of a deleted user, not corruption.
ALTER TABLE dataapi.access_log ADD COLUMN actor_user_id uuid;

-- `request_method` / `request_path` — what the caller actually asked for.
--
-- WHY THEY ARE NEEDED AT ALL: a request that never resolved an endpoint writes a
-- row with `endpoint_id IS NULL` (see 0123's comment — "a 404 still produces a
-- row"). Without the requested URL, those rows can only be counted, never read:
-- the management UI can say "37 calls matched no endpoint" and nothing more,
-- which is exactly the situation where an operator needs to know WHICH url a
-- client is getting wrong.
--
-- The serving path holds both values at the moment it writes the row, so this
-- records something already in hand rather than reconstructing it.
--
-- NO INDEX ON request_path, deliberately. Nothing looks a row up by it: the
-- call list narrows on (workspace_id, ts) via `access_log_ws_ts_idx` and reads
-- the path out of the rows that query already returned. An index here would
-- only slow down the busiest insert in the schema.
ALTER TABLE dataapi.access_log ADD COLUMN request_method text;
ALTER TABLE dataapi.access_log ADD COLUMN request_path   text;
```

- [ ] **Step 2: Add the three fields to `AccessLogEntry`**

In `crates/egret-dataapi/src/serving/access_log.rs`, after the `error_code` field (line 93):

```rust
    /// Non-null ⇒ this call came from the Try-it console, run by this person on
    /// a service account's behalf (P5 §3). Statistics filter these out; the call
    /// list shows them, labelled.
    ///
    /// **Set by the management `/try` handler, never by the serving path.** A
    /// real consumer call has no actor, and a serving path that could populate
    /// this would make "is this a trial?" a claim rather than a fact.
    pub actor_user_id: Option<Uuid>,
    /// The method and path as requested. Recorded because a row that resolved no
    /// endpoint (`endpoint_id == None`) has nothing else identifying it — see
    /// the column comment in `0125_dataapi_access_log_p5.sql`.
    pub request_method: Option<String>,
    pub request_path: Option<String>,
```

And in `new`, after `error_code: None,`:

```rust
            actor_user_id: None,
            request_method: None,
            request_path: None,
```

- [ ] **Step 3: Bind them in `record`**

Replace the `INSERT` statement and add three binds after `.bind(&e.request_id)`:

```rust
        let res = sqlx::query(
            "INSERT INTO dataapi.access_log \
               (workspace_id, endpoint_id, version, service_account_id, client_id, \
                status_code, row_count, duration_ms, cache_hit, engine_query_id, \
                client_ip, error_code, request_id, actor_user_id, \
                request_method, request_path) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11::text::inet,$12,$13,$14,$15,$16)",
        )
```

```rust
        .bind(e.actor_user_id)
        .bind(e.request_method.as_deref())
        .bind(e.request_path.as_deref())
```

- [ ] **Step 4: Populate method and path in `serve`**

In `crates/egret-dataapi/src/serving/router.rs`, immediately after `let mut entry = AccessLogStore::entry(&request_id, &headers);`:

```rust
    // Recorded BEFORE `run`, because `run` is what may fail to resolve an
    // endpoint — and a row with no endpoint is precisely the row these two
    // columns exist for. Populating them afterwards would mean the failure path
    // had to remember to do it.
    entry.request_method = Some(method.to_string());
    entry.request_path = Some(path.1.clone());
```

- [ ] **Step 5: Write the failing test**

Create `crates/egret-dataapi/tests/activity.rs`:

```rust
//! Data API P5 §4 — Activity statistics and the call list.
//!
//! DB-gated. Self-skips (passes) when `DATABASE_URL` is unset, so check the
//! DURATION, not just the `ok`.

mod serving_common;

use serving_common::fixture;

#[tokio::test]
async fn a_call_that_matches_no_endpoint_still_records_its_url() {
    let Some(f) = fixture().await else { return };

    // A path no endpoint claims: this 404s inside routing, so the row carries
    // no endpoint_id and the two new columns are the only thing identifying it.
    let res = f.call_as_client(f.principal_a, "client-a", "no/such/endpoint").await;
    assert_eq!(res.status, 404);

    let row: (Option<uuid::Uuid>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT endpoint_id, request_method, request_path \
         FROM dataapi.access_log WHERE workspace_id = $1 ORDER BY ts DESC LIMIT 1",
    )
    .bind(f.ws_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("access_log row");

    assert_eq!(row.0, None, "no endpoint should have resolved");
    assert_eq!(row.1.as_deref(), Some("GET"));
    assert_eq!(
        row.2.as_deref(),
        Some("no/such/endpoint"),
        "without this the row is an uncountable 404 with nothing to act on"
    );
}
```

- [ ] **Step 6: Run the test to verify it fails**

Run: `DATABASE_URL=postgres://localhost/egret_test cargo test -p egret-dataapi --test activity -- --nocapture`
Expected: FAIL — `column "request_method" does not exist` (migration not yet applied) or a compile error on the unknown field.

- [ ] **Step 7: Apply the migration and re-run**

Run: `DATABASE_URL=postgres://localhost/egret_test cargo test -p egret-dataapi --test activity -- --nocapture`
Expected: PASS. If it fails with a stale-migration error, touch `crates/egret-metastore/src/lib.rs` to force the embedded migrations to rebuild.

- [ ] **Step 8: Verify the test is not vacuous**

Comment out the two `entry.request_method`/`entry.request_path` lines from Step 4, rebuild, and re-run. **Confirm the build succeeds with 0 errors first** — a mutation that fails to compile looks exactly like a test that did not catch anything.
Expected: FAIL on the `request_path` assertion. Restore the lines.

- [ ] **Step 9: Commit**

```bash
git add migrations/0125_dataapi_access_log_p5.sql \
        crates/egret-dataapi/src/serving/access_log.rs \
        crates/egret-dataapi/src/serving/router.rs \
        crates/egret-dataapi/tests/activity.rs
git commit -m "feat(dataapi): record actor, method and path on the access log"
```

---

### Task 2: One invoke expression, three consumers

**Files:**
- Create: `crates/egret-dataapi/src/invoke.rs`
- Modify: `crates/egret-dataapi/src/serving/router.rs:839-878` (`invoke_allowed`)
- Modify: `crates/egret-dataapi/src/lib.rs` (module declaration)
- Test: `crates/egret-dataapi/tests/openapi_gen.rs` (Task 6 asserts the cascade through this)

**Interfaces:**
- Produces: `pub(crate) async fn can_invoke(st: &DataApiState, user_id: UserId, ws: WorkspaceId, endpoint_id: Uuid) -> Result<bool, egret_core::Error>` — Task 6 and the frontend's Access checklist both call it.

The invoke question is currently asked in exactly one place, and P5 adds two more askers. A grant-table scan would answer differently from the PDP for any principal reachable through the `ws_admin` cascade, so the expression must exist once.

- [ ] **Step 1: Write the failing test**

Append to `crates/egret-dataapi/tests/activity.rs` for now (it moves to `openapi_gen.rs` in Task 6; keeping it here means Task 2 ships with its own gate):

```rust
#[tokio::test]
async fn ws_admin_can_invoke_without_any_object_grant() {
    let Some(f) = fixture().await else { return };

    // `f.admin` is a ws_admin membership and holds NO object-scoped grant on the
    // endpoint. Migration 0124 puts data_api_endpoint:invoke on that role, and
    // the Cedar workspace->object cascade applies it to every endpoint — so a
    // grant-table scan says "no" here and the PDP says "yes". This test is the
    // difference between those two answers.
    let grants: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants \
         WHERE principal_id = $1 AND scope_type = 'object'",
    )
    .bind(f.admin)
    .fetch_one(&f.db.pool)
    .await
    .expect("count grants");
    assert_eq!(grants, 0, "fixture must not give the admin an object grant");

    let allowed = egret_dataapi::can_invoke_for_test(
        &f.state,
        egret_core::ids::UserId(f.admin),
        egret_core::WorkspaceId(f.ws_id),
        f.endpoint_id,
    )
    .await
    .expect("pdp answered");

    assert!(allowed, "ws_admin reaches invoke through the workspace cascade");
}
```

This requires `Fixture` to expose `state: DataApiState` and `endpoint_id: Uuid`. Add both fields in `crates/egret-dataapi/tests/serving_common/mod.rs` — `state: st.clone()` before the existing `.with_state(st)` consumes it, and the id of the endpoint the fixture publishes.

- [ ] **Step 2: Run it to verify it fails**

Run: `DATABASE_URL=postgres://localhost/egret_test cargo test -p egret-dataapi --test activity ws_admin_can_invoke -- --nocapture`
Expected: FAIL to compile — `can_invoke_for_test` does not exist.

- [ ] **Step 3: Write `crates/egret-dataapi/src/invoke.rs`**

```rust
//! The invoke question, asked once.
//!
//! Three callers need to know whether a principal may invoke an endpoint: the
//! serving path (before it runs anything), the OpenAPI generator (before it
//! documents a path for a consumer), and the management UI's Access checklist
//! (before it draws a tick). They must agree, and the only way they can is by
//! being the same expression.
//!
//! # Why this cannot be a query against `access_grants`
//!
//! After migration 0124 the `ws_admin` role carries `data_api_endpoint:invoke`,
//! and the Cedar workspace->object cascade applies a workspace-scoped grant to
//! every object in it. A principal can therefore be fully able to invoke an
//! endpoint while owning no row in `access_grants` that mentions it. A scan
//! answers "no" there, the PDP answers "yes", and the serving path follows the
//! PDP — so a scan-based generator emits a document with paths silently missing,
//! and a scan-based checklist draws a cross next to an endpoint that works.

use crate::routes::DataApiState;
use egret_core::ids::UserId;
use egret_core::WorkspaceId;
use uuid::Uuid;

/// Whether `user_id` may invoke `endpoint_id` in `ws`.
///
/// `folder_id: None` — a folder-scoped grant does NOT cascade to invoke. That is
/// a decision, not an omission: "invoke everything in this folder" widens access
/// and nobody has asked for it. When P5's UI makes endpoint folders reachable,
/// whoever wants that cascade passes the real folder here and accepts what it
/// means.
///
/// An error is an error, never a refusal. A PDP that could not answer is our
/// fault: returning `false` would send an operator to write a grant that already
/// exists.
pub(crate) async fn can_invoke(
    st: &DataApiState,
    user_id: UserId,
    ws: WorkspaceId,
    endpoint_id: Uuid,
) -> Result<bool, egret_core::Error> {
    Ok(st
        .pdp
        .authorize(
            &egret_core::authz::Subject {
                user_id,
                group_ids: Vec::new(),
            },
            crate::routes::EP_INVOKE,
            &egret_core::authz::Resource::Object(egret_core::authz::ObjectRef {
                kind: crate::endpoints::OBJECT_KIND.to_string(),
                id: endpoint_id,
                folder_id: None,
                workspace_id: ws,
            }),
        )
        .await?
        .allow)
}
```

- [ ] **Step 4: Re-point `invoke_allowed` at it**

Replace the body of `invoke_allowed` in `crates/egret-dataapi/src/serving/router.rs` (keep its doc comment, which explains the `folder_id: None` decision, and add a line saying the expression now lives in `crate::invoke`):

```rust
async fn invoke_allowed(
    st: &DataApiState,
    principal: &super::ServingPrincipal,
    ws: egret_core::WorkspaceId,
    endpoint_id: Uuid,
) -> Result<(), ServingError> {
    let allowed = crate::invoke::can_invoke(st, principal.user_id, ws, endpoint_id)
        .await
        // A PDP that could not answer is OUR fault, and it must not read as a
        // refusal: `Internal` (500) sends an operator to egret, `NotGranted`
        // (403) would send them to write a grant that already exists.
        .map_err(ServingError::internal)?;
    if allowed {
        Ok(())
    } else {
        tracing::debug!(
            %endpoint_id,
            service_account_id = %principal.user_id.0,
            client_id = %principal.client_id,
            "data-api call refused: no invoke grant on this endpoint"
        );
        Err(ServingError::NotGranted)
    }
}
```

- [ ] **Step 5: Declare the module and the test shim**

In `crates/egret-dataapi/src/lib.rs`, add `mod invoke;` alongside the other module declarations, and export a test-only shim so integration tests can reach a `pub(crate)` function:

```rust
/// Test-only access to the shared invoke expression (P5 §5.2). Integration
/// tests live outside the crate and cannot see `pub(crate)`; exposing it through
/// a named shim keeps the real function crate-private, so no production caller
/// can drift onto a second copy of this decision.
#[doc(hidden)]
pub async fn can_invoke_for_test(
    st: &crate::routes::DataApiState,
    user_id: egret_core::ids::UserId,
    ws: egret_core::WorkspaceId,
    endpoint_id: uuid::Uuid,
) -> Result<bool, egret_core::Error> {
    crate::invoke::can_invoke(st, user_id, ws, endpoint_id).await
}
```

- [ ] **Step 6: Run the tests**

Run: `DATABASE_URL=postgres://localhost/egret_test cargo test -p egret-dataapi --test activity --test serving_authz -- --nocapture`
Expected: PASS, including the pre-existing `serving_authz` suite — the serving behaviour must be unchanged by the extraction.

- [ ] **Step 7: Commit**

```bash
git add crates/egret-dataapi/src/invoke.rs crates/egret-dataapi/src/lib.rs \
        crates/egret-dataapi/src/serving/router.rs \
        crates/egret-dataapi/tests/serving_common/mod.rs \
        crates/egret-dataapi/tests/activity.rs
git commit -m "refactor(dataapi): extract the invoke check into one shared expression"
```

---

### Task 3: The four missing audit events

**Files:**
- Modify: `crates/egret-dataapi/src/routes.rs` — `create_service_account`, `delete_service_account`, `create_endpoint`, `update_endpoint`
- Test: `crates/egret-dataapi/tests/audit.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: four new `action` strings — `service_account:create`, `service_account:delete`, `data_api_endpoint:create`, `data_api_endpoint:update`.

- [ ] **Step 1: Write the failing test**

Append to `crates/egret-dataapi/tests/audit.rs`:

```rust
#[tokio::test]
async fn deleting_a_service_account_records_what_it_destroyed() {
    let Some(f) = fixture().await else { return };

    let (status, created) = f
        .mgmt_post(
            &format!("/api/ws/{}/data-api/service-accounts", f.ws_slug),
            serde_json::json!({ "name": "doomed", "description": "" }),
        )
        .await;
    assert_eq!(status, 201);
    let sa_id = created["user_id"].as_str().expect("user_id").to_string();

    let (status, _) = f
        .mgmt_delete(&format!(
            "/api/ws/{}/data-api/service-accounts/{sa_id}",
            f.ws_slug
        ))
        .await;
    assert_eq!(status, 200);

    let detail: serde_json::Value = sqlx::query_scalar(
        "SELECT detail FROM audit.audit_log \
         WHERE action = 'service_account:delete' ORDER BY ts DESC LIMIT 1",
    )
    .fetch_one(&f.db.pool)
    .await
    .expect("audit row");

    // The row is gone and its grants and clients cascaded with it, so this event
    // is the ONLY surviving record that the principal existed. A bare uuid would
    // say something was deleted without ever saying what.
    assert_eq!(detail["name"], "doomed");
    assert!(detail["email"].is_string(), "the synthetic address identifies it");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `DATABASE_URL=postgres://localhost/egret_test cargo test -p egret-dataapi --test audit deleting_a_service_account -- --nocapture`
Expected: FAIL — `audit row` panics, because no such event is emitted.

- [ ] **Step 3: Emit the event**

In `delete_service_account`, after the delete succeeds and using the columns the delete returns (add a `RETURNING name, email` to the store's statement if it does not already return them — the event must not cost a second query, which could race the delete):

```rust
    // The hardest of the four to add later and the most important to have: this
    // hard-deletes a machine principal along with its `access_grants` and every
    // client it owns. Nothing else survives to say it existed.
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "service_account:delete",
        "service_account",
        Some(id.to_string()),
        serde_json::json!({ "name": gone.name, "email": gone.email }),
    ));
```

- [ ] **Step 4: Emit the other three**

`create_service_account`, after the account is created:

```rust
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "service_account:create",
        "service_account",
        Some(created.user_id.to_string()),
        serde_json::json!({ "name": created.name, "email": created.email }),
    ));
```

`create_endpoint`, after the draft is written:

```rust
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "data_api_endpoint:create",
        "data_api_endpoint",
        Some(created.id.to_string()),
        serde_json::json!({
            "name": created.name,
            "path": created.path,
            "method": created.method,
            "database_id": created.database_id,
        }),
    ));
```

`update_endpoint`, after the update. **The detail records the URL as it now stands, not a field-by-field diff** — `path` and `method` are editable on a live row, so "which URL does this endpoint claim" is the question an auditor asks, and a diff of unchanged fields buries it:

```rust
    let updated = st.endpoints.update(ctx.workspace_id.0, id, &body).await?;
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "data_api_endpoint:update",
        "data_api_endpoint",
        Some(id.to_string()),
        serde_json::json!({
            "name": updated.name,
            "path": updated.path,
            "method": updated.method,
            "database_id": updated.database_id,
        }),
    ));
    Ok(Json(updated))
```

- [ ] **Step 5: Run the whole audit suite**

Run: `DATABASE_URL=postgres://localhost/egret_test cargo test -p egret-dataapi --test audit -- --nocapture`
Expected: PASS, including the crate-wide assertion that no `client_secret` key or secret bytes appear in any event.

- [ ] **Step 6: Update the spec's audit table**

The P4 spec's §5.3 table lists seven audited routes and states the other four emit nothing. That statement is now false. Edit `docs/en/specs/2026-08-14-data-api-design.md`: add the four rows and replace the "the rest is NOT audited yet" paragraph with a sentence saying P5 closed it. Mirror in `docs/vi/`.

- [ ] **Step 7: Commit**

```bash
git add crates/egret-dataapi/src/routes.rs crates/egret-dataapi/tests/audit.rs \
        docs/en/specs/2026-08-14-data-api-design.md \
        docs/vi/specs/2026-08-14-data-api-design.md
git commit -m "feat(dataapi): audit service-account and endpoint draft mutations"
```

---

### Task 4: Activity summary

**Files:**
- Create: `crates/egret-dataapi/src/activity.rs`
- Modify: `crates/egret-dataapi/src/routes.rs` (mount + handler + utoipa)
- Test: `crates/egret-dataapi/tests/activity.rs`

**Interfaces:**
- Consumes: `AccessLogEntry`'s new columns (Task 1).
- Produces: `ActivityRange` (`OneHour`/`OneDay`/`SevenDays`/`ThirtyDays`, `serde` as `1h`/`24h`/`7d`/`30d`), `ActivitySummaryDto { total: ActivityRow, endpoints: Vec<ActivityRow> }`, `ActivityRow { endpoint_id: Option<Uuid>, endpoint_name: Option<String>, calls: i64, errors: i64, p50_ms: Option<f64>, p95_ms: Option<f64>, cache_hits: i64, cacheable_calls: i64 }`.

The frontend computes rates from the four raw counts rather than receiving pre-divided percentages — a rate whose denominator is not visible cannot be checked, and §4.3's whole point is that the denominators are not obvious.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn latency_percentiles_ignore_error_responses() {
    let Some(f) = fixture().await else { return };

    // Three successes at 100ms and seven failures at 1ms. Including the failures
    // drags p50 to 1ms and makes latency look BETTER the more the system fails.
    for _ in 0..3 {
        f.log_row(200, 100, false, None).await;
    }
    for _ in 0..7 {
        f.log_row(401, 1, false, Some("unauthorized")).await;
    }

    let (status, body) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/activity/summary?range=24h",
            f.ws_slug
        ))
        .await;
    assert_eq!(status, 200);

    assert_eq!(body["total"]["calls"], 10);
    assert_eq!(body["total"]["errors"], 7);
    assert_eq!(
        body["total"]["p50_ms"].as_f64().unwrap().round(),
        100.0,
        "p50 must come from the 2xx population only"
    );
}

#[tokio::test]
async fn trial_runs_are_excluded_from_statistics() {
    let Some(f) = fixture().await else { return };

    f.log_row(200, 10, false, None).await;
    f.log_row_as_trial(200, 10, f.admin).await;

    let (_, body) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/activity/summary?range=24h",
            f.ws_slug
        ))
        .await;

    assert_eq!(body["total"]["calls"], 1, "the trial must not be counted");
}
```

Add `log_row`, `log_row_as_trial` and `mgmt_get` helpers to `serving_common/mod.rs`; `log_row` inserts directly into `dataapi.access_log` so a test can shape a population without driving ten real requests.

- [ ] **Step 2: Run to verify failure**

Run: `DATABASE_URL=postgres://localhost/egret_test cargo test -p egret-dataapi --test activity latency_percentiles -- --nocapture`
Expected: FAIL with 404 — the route does not exist.

- [ ] **Step 3: Write `activity.rs`**

```rust
//! Data API P5 §4 — what `dataapi.access_log` can tell an operator.

use chrono::{DateTime, Duration, Utc};
use egret_metastore::Db;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The four ranges the UI offers, and the only ones it may ask for.
///
/// A fixed set rather than a date picker: every query here narrows on
/// `access_log_ws_ts_idx (workspace_id, ts DESC)`, and the presets are what keep
/// it inside that index's reach. An input that accepts "one year" on the
/// fastest-growing table in the schema has nothing stopping the person who types
/// it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, utoipa::ToSchema)]
pub enum ActivityRange {
    #[serde(rename = "1h")]
    OneHour,
    #[serde(rename = "24h")]
    OneDay,
    #[serde(rename = "7d")]
    SevenDays,
    #[serde(rename = "30d")]
    ThirtyDays,
}

impl ActivityRange {
    /// The cutoff, computed in Rust and bound as a `timestamptz`.
    ///
    /// Never interpolated into SQL: sqlx 0.9's `query()` takes a `&'static str`,
    /// so a range that reached the statement text would have to be built, and a
    /// built statement is the shape this codebase spent P1 removing.
    pub fn since(self, now: DateTime<Utc>) -> DateTime<Utc> {
        now - match self {
            Self::OneHour => Duration::hours(1),
            Self::OneDay => Duration::hours(24),
            Self::SevenDays => Duration::days(7),
            Self::ThirtyDays => Duration::days(30),
        }
    }
}

/// One line of the statistics table, or — with `endpoint_id: None` — either the
/// grand total or the unmatched-request bucket, depending on which grouping set
/// produced it.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ActivityRow {
    pub endpoint_id: Option<Uuid>,
    pub endpoint_name: Option<String>,
    pub calls: i64,
    pub errors: i64,
    /// `None` when the population is empty — no successful call in range. Zero
    /// would be a measurement; this is the absence of one.
    pub p50_ms: Option<f64>,
    pub p95_ms: Option<f64>,
    pub cache_hits: i64,
    /// The DENOMINATOR for cache-hit rate: successful calls on a version whose
    /// snapshot had caching enabled. Sent raw so the caller can render `—`
    /// rather than `0%` when it is zero — zero of zero is not zero.
    pub cacheable_calls: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ActivitySummaryDto {
    pub total: ActivityRow,
    pub endpoints: Vec<ActivityRow>,
    /// `DataApiConfig::access_log_retention_days`, passed through so the UI can
    /// disable ranges that reach past it (spec §4.7).
    ///
    /// **The UI cannot compute this and must not assume 30 days exist.**
    /// Retention may be shorter, and `0` means keep nothing — the same reading
    /// `AuditConfig::retention_days` has. Without this field someone selects
    /// `30d`, watches the data thin out at the old end, and concludes traffic
    /// fell; the screen would be actively supporting a wrong conclusion.
    ///
    /// `None` ⇒ no prune is scheduled (`scheduled_retention` returned `None`),
    /// so nothing is being deleted and every range is honest.
    pub retention_days: Option<i64>,
}

#[derive(Clone)]
pub struct ActivityStore {
    db: Db,
    /// Held rather than re-read per request: it comes from config, which does
    /// not change while the process runs.
    retention_days: Option<i64>,
}

impl ActivityStore {
    /// `retention_days` is `crate::serving::scheduled_retention(cfg.dataapi
    /// .access_log_retention_days)` — the SAME guard the prune scheduler uses,
    /// so the UI is told what is actually happening rather than what was
    /// configured. Passing the raw config value here would report a retention
    /// of 0 as "keeps nothing" even where no prune is scheduled at all.
    pub fn new(db: Db, retention_days: Option<i64>) -> Self {
        Self { db, retention_days }
    }

    /// Tiles and table, from ONE statement.
    ///
    /// `GROUPING SETS ((), (endpoint_id))` rather than two queries: the table is
    /// written continuously, and two statements moments apart produce a total
    /// that does not equal the sum of its own rows. A reader who notices that
    /// concludes they misread the screen.
    ///
    /// # The three denominators
    ///
    /// * `calls` / `errors` — every row.
    /// * `p50`/`p95` — `FILTER (WHERE status_code BETWEEN 200 AND 299)`. A burst
    ///   of 401s returning in 2ms drags p50 down and makes latency look better
    ///   at exactly the moment the system is failing.
    /// * `cacheable_calls` — successful calls whose SERVING VERSION had caching
    ///   on, read from that version's own snapshot. Joining the version the row
    ///   records (rather than the endpoint's current config) is what makes the
    ///   ratio describe the calls it counts: an endpoint whose caching was turned
    ///   on yesterday must not retroactively make last week's calls look like
    ///   misses.
    ///
    /// Trials (`actor_user_id IS NOT NULL`) are excluded here and shown only in
    /// the call list.
    pub async fn summary(
        &self,
        ws: Uuid,
        since: DateTime<Utc>,
    ) -> Result<ActivitySummaryDto, sqlx::Error> {
        let rows: Vec<(Option<Uuid>, Option<String>, i64, i64, Option<f64>, Option<f64>, i64, i64, i32)> =
            sqlx::query_as(
                "SELECT al.endpoint_id, \
                        max(e.name) AS endpoint_name, \
                        count(*) AS calls, \
                        count(*) FILTER (WHERE al.status_code >= 400) AS errors, \
                        percentile_cont(0.5) WITHIN GROUP (ORDER BY al.duration_ms) \
                          FILTER (WHERE al.status_code BETWEEN 200 AND 299) AS p50_ms, \
                        percentile_cont(0.95) WITHIN GROUP (ORDER BY al.duration_ms) \
                          FILTER (WHERE al.status_code BETWEEN 200 AND 299) AS p95_ms, \
                        count(*) FILTER (WHERE al.cache_hit) AS cache_hits, \
                        count(*) FILTER ( \
                          WHERE al.status_code BETWEEN 200 AND 299 \
                            AND (ev.snapshot -> 'cache' ->> 'enabled')::boolean IS TRUE \
                        ) AS cacheable_calls, \
                        grouping(al.endpoint_id) AS is_total \
                 FROM dataapi.access_log al \
                 LEFT JOIN dataapi.endpoints e ON e.id = al.endpoint_id \
                 LEFT JOIN dataapi.endpoint_versions ev \
                        ON ev.endpoint_id = al.endpoint_id AND ev.version = al.version \
                 WHERE al.workspace_id = $1 AND al.ts >= $2 AND al.actor_user_id IS NULL \
                 GROUP BY GROUPING SETS ((), (al.endpoint_id))",
            )
            .bind(ws)
            .bind(since)
            .fetch_all(&self.db.pool)
            .await?;

        let mut total = ActivityRow {
            endpoint_id: None,
            endpoint_name: None,
            calls: 0,
            errors: 0,
            p50_ms: None,
            p95_ms: None,
            cache_hits: 0,
            cacheable_calls: 0,
        };
        let mut endpoints = Vec::new();
        for r in rows {
            let row = ActivityRow {
                endpoint_id: r.0,
                endpoint_name: r.1,
                calls: r.2,
                errors: r.3,
                p50_ms: r.4,
                p95_ms: r.5,
                cache_hits: r.6,
                cacheable_calls: r.7,
            };
            // `grouping()` is 1 for the empty grouping set — the grand total —
            // and 0 for the per-endpoint rows. It is the only way to tell the
            // total apart from the unmatched bucket: BOTH carry a NULL
            // endpoint_id, and reading NULL as "total" would double-count every
            // 404 into the headline number.
            if r.8 == 1 {
                total = row;
            } else {
                endpoints.push(row);
            }
        }
        endpoints.sort_by(|a, b| b.calls.cmp(&a.calls));
        Ok(ActivitySummaryDto {
            total,
            endpoints,
            retention_days: self.retention_days,
        })
    }
}
```

- [ ] **Step 4: Mount the route**

In `routes.rs`, add `pub activity: ActivityStore` to `DataApiState` (and construct it in `crates/egret-bin/src/main.rs:1889` and in `serving_common/mod.rs:460`), then:

```rust
#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ActivityQuery {
    pub range: ActivityRange,
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/data-api/activity/summary",
    tag = "data-api",
    params(WorkspacePath, ActivityQuery),
    responses(
        (status = 200, description = "Call statistics for the range", body = ActivitySummaryDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn activity_summary(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Query(q): Query<ActivityQuery>,
) -> Result<Json<ActivitySummaryDto>, DataApiError> {
    // Workspace-scoped `:read` for the whole tab. Activity is deliberately a
    // workspace-level view and is NOT sliced per endpoint: a per-row filter
    // would make the totals disagree with their own rows, which is the one thing
    // §4.2 spends a GROUPING SETS query to prevent.
    egret_core::authz::require(&*st.pdp, &ctx, EP_READ).await?;
    Ok(Json(
        st.activity
            .summary(ctx.workspace_id.0, q.range.since(Utc::now()))
            .await
            .map_err(db_err)?,
    ))
}
```

Register the route in `management_router()` and add `ActivitySummaryDto`, `ActivityRow`, `ActivityRange` to `DataApiAdminApi`'s `components(schemas(...))` and `activity_summary` to its `paths(...)`.

- [ ] **Step 5: Run the tests**

Run: `DATABASE_URL=postgres://localhost/egret_test cargo test -p egret-dataapi --test activity -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Verify the percentile test is not vacuous**

Change the `FILTER (WHERE al.status_code BETWEEN 200 AND 299)` on `p50_ms` to no filter, rebuild (confirm 0 errors), re-run.
Expected: FAIL — p50 becomes 1.0. Restore.

- [ ] **Step 7: Regenerate the OpenAPI document**

Run: `cargo run -p egret-bin -- openapi > openapi.json`
Then: `cargo test -p egret-server committed_openapi_json_is_current`
Expected: PASS. This gate lives in a different crate from the one edited, and it is the one that catches a forgotten regeneration.

- [ ] **Step 8: Commit**

```bash
git add crates/egret-dataapi/src/activity.rs crates/egret-dataapi/src/routes.rs \
        crates/egret-dataapi/src/lib.rs crates/egret-bin/src/main.rs \
        crates/egret-dataapi/tests/serving_common/mod.rs \
        crates/egret-dataapi/tests/activity.rs openapi.json
git commit -m "feat(dataapi): activity summary statistics from the access log"
```

---

### Task 5: Activity call list

**Files:**
- Modify: `crates/egret-dataapi/src/activity.rs`, `crates/egret-dataapi/src/routes.rs`
- Test: `crates/egret-dataapi/tests/activity.rs`

**Interfaces:**
- Produces: `ActivityCallDto`, and `ActivityStore::calls(ws, since, filters, cursor, limit) -> Vec<ActivityCallDto>`.

- [ ] **Step 1: Write the failing test — the security one first**

```rust
#[tokio::test]
async fn a_request_id_lookup_cannot_read_another_workspace() {
    let Some(f) = fixture().await else { return };

    // A row in ANOTHER workspace, with a request_id the caller knows.
    let other_ws = f.make_workspace("other").await;
    let rid = "01JOTHERWORKSPACEREQUESTID";
    f.log_row_in(other_ws, 500, 5, Some(rid)).await;

    let (status, body) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/activity/calls?range=24h&request_id={rid}",
            f.ws_slug
        ))
        .await;

    assert_eq!(status, 200);
    assert_eq!(
        body.as_array().map(|a| a.len()),
        Some(0),
        "access_log_request_id_idx is global on purpose; the QUERY is what scopes it"
    );
}

#[tokio::test]
async fn trial_runs_appear_only_when_asked_for() {
    let Some(f) = fixture().await else { return };
    f.log_row(200, 10, false, None).await;
    f.log_row_as_trial(200, 10, f.admin).await;

    let (_, without) = f
        .mgmt_get(&format!("/api/ws/{}/data-api/activity/calls?range=24h", f.ws_slug))
        .await;
    assert_eq!(without.as_array().unwrap().len(), 1);

    let (_, with) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/activity/calls?range=24h&include_trials=true",
            f.ws_slug
        ))
        .await;
    assert_eq!(with.as_array().unwrap().len(), 2);
    let trial = with
        .as_array()
        .unwrap()
        .iter()
        .find(|r| !r["actor_user_id"].is_null())
        .expect("the trial row is present and identifies its actor");
    assert_eq!(trial["actor_user_id"], f.admin.to_string());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `DATABASE_URL=postgres://localhost/egret_test cargo test -p egret-dataapi --test activity a_request_id_lookup -- --nocapture`
Expected: FAIL with 404 — no such route.

- [ ] **Step 3: Add the DTO and the query**

```rust
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ActivityCallDto {
    pub id: Uuid,
    pub ts: DateTime<Utc>,
    pub endpoint_id: Option<Uuid>,
    pub endpoint_name: Option<String>,
    /// What the caller asked for. The ONLY identification a row carries when
    /// `endpoint_id` is null — see migration 0125.
    pub request_method: Option<String>,
    pub request_path: Option<String>,
    pub status_code: i32,
    pub error_code: Option<String>,
    pub duration_ms: i32,
    pub cache_hit: bool,
    pub client_id: Option<String>,
    pub request_id: String,
    /// Non-null ⇒ a Try-it run by this person, not consumer traffic.
    pub actor_user_id: Option<Uuid>,
}
```

```rust
    /// The call list. One fixed statement; every filter is an optional
    /// predicate, because sqlx 0.9's `query_as` takes a `&'static str` and a
    /// built statement is what P1 spent its whole scope removing.
    ///
    /// **`workspace_id = $1` is not negotiable, including on the `request_id`
    /// lookup.** `access_log_request_id_idx` is deliberately global so support
    /// can find a row without knowing the workspace; a handler that leans on the
    /// index and drops the predicate lets a guessable id read across a tenancy
    /// boundary. The index's shape is not the query's authorization.
    ///
    /// Keyset pagination on `ts` — `OFFSET` on the fastest-growing table in the
    /// schema gets slower every page and can skip rows written mid-scroll.
    #[allow(clippy::too_many_arguments)]
    pub async fn calls(
        &self,
        ws: Uuid,
        since: DateTime<Utc>,
        endpoint_id: Option<Uuid>,
        status_code: Option<i32>,
        error_code: Option<&str>,
        request_id: Option<&str>,
        include_trials: bool,
        before: Option<DateTime<Utc>>,
        limit: i64,
    ) -> Result<Vec<ActivityCallDto>, sqlx::Error> {
        sqlx::query_as(
            "SELECT al.id, al.ts, al.endpoint_id, e.name AS endpoint_name, \
                    al.request_method, al.request_path, al.status_code, al.error_code, \
                    al.duration_ms, al.cache_hit, al.client_id, al.request_id, \
                    al.actor_user_id \
             FROM dataapi.access_log al \
             LEFT JOIN dataapi.endpoints e ON e.id = al.endpoint_id \
             WHERE al.workspace_id = $1 \
               AND al.ts >= $2 \
               AND ($3::uuid IS NULL OR al.endpoint_id = $3) \
               AND ($4::int IS NULL OR al.status_code = $4) \
               AND ($5::text IS NULL OR al.error_code = $5) \
               AND ($6::text IS NULL OR al.request_id = $6) \
               AND ($7::boolean OR al.actor_user_id IS NULL) \
               AND ($8::timestamptz IS NULL OR al.ts < $8) \
             ORDER BY al.ts DESC \
             LIMIT $9",
        )
        .bind(ws)
        .bind(since)
        .bind(endpoint_id)
        .bind(status_code)
        .bind(error_code)
        .bind(request_id)
        .bind(include_trials)
        .bind(before)
        .bind(limit.clamp(1, 200))
        .fetch_all(&self.db.pool)
        .await
    }
```

- [ ] **Step 4: Add the handler**

```rust
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ActivityCallsQuery {
    pub range: ActivityRange,
    #[serde(default)]
    pub endpoint_id: Option<Uuid>,
    #[serde(default)]
    pub status_code: Option<i32>,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub request_id: Option<String>,
    /// Off by default. A trial is not consumer traffic, so it must be asked for.
    #[serde(default)]
    pub include_trials: bool,
    /// Keyset cursor: return rows strictly older than this.
    #[serde(default)]
    pub before: Option<DateTime<Utc>>,
    #[serde(default = "default_call_limit")]
    pub limit: i64,
}

fn default_call_limit() -> i64 {
    50
}
```

The handler gates on `EP_READ` exactly as `activity_summary` does and forwards to `st.activity.calls(...)`.

- [ ] **Step 5: Run the tests**

Run: `DATABASE_URL=postgres://localhost/egret_test cargo test -p egret-dataapi --test activity -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Verify the tenancy test is not vacuous**

Remove `AND al.workspace_id = $1` (bind `$1` in a dummy comparison so the statement still compiles with the same parameter count), rebuild, confirm 0 errors, re-run.
Expected: FAIL — the other workspace's row is returned. Restore.

- [ ] **Step 7: Regenerate OpenAPI and commit**

```bash
cargo run -p egret-bin -- openapi > openapi.json
cargo test -p egret-server committed_openapi_json_is_current
git add crates/egret-dataapi/src/activity.rs crates/egret-dataapi/src/routes.rs \
        crates/egret-dataapi/tests/activity.rs crates/egret-dataapi/tests/serving_common/mod.rs \
        openapi.json
git commit -m "feat(dataapi): activity call list with request_id lookup"
```

---

### Task 6: Generated OpenAPI

**Files:**
- Create: `crates/egret-dataapi/src/openapi_gen.rs`
- Modify: `crates/egret-dataapi/src/routes.rs`
- Test: `crates/egret-dataapi/tests/openapi_gen.rs`

**Interfaces:**
- Consumes: `can_invoke` (Task 2), `DataApiState.issuer`.
- Produces: `pub async fn build(st: &DataApiState, ws: WorkspaceId, service_account: UserId, only: Option<Uuid>) -> Result<serde_json::Value, OpenApiGenError>` with `OpenApiGenError::NamingCollision { endpoint: String }`.

The document is assembled as `serde_json::Value`, not through utoipa: utoipa describes *this server's* compiled routes, while these paths exist only in a user's data.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn a_ws_admin_gets_cascade_reachable_paths() {
    let Some(f) = fixture().await else { return };
    // The admin holds no object grant (asserted in Task 2's test); its invoke
    // path is the ws_admin membership and the Cedar cascade. A generator that
    // scanned access_grants would emit an EMPTY paths object here and pass every
    // other test in this file.
    let (status, doc) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/openapi?service_account_id={}",
            f.ws_slug, f.admin
        ))
        .await;
    assert_eq!(status, 200);
    assert!(
        doc["paths"].as_object().is_some_and(|p| !p.is_empty()),
        "cascade-reachable endpoints must be documented"
    );
}

#[tokio::test]
async fn servers_and_token_url_share_one_origin() {
    let Some(f) = fixture().await else { return };
    let (_, doc) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/openapi?service_account_id={}",
            f.ws_slug, f.admin
        ))
        .await;

    let server = doc["servers"][0]["url"].as_str().expect("servers url");
    let token_url = doc["components"]["securitySchemes"]["egret_oauth2"]["flows"]
        ["clientCredentials"]["tokenUrl"]
        .as_str()
        .expect("tokenUrl");

    // Fixture issuer is https://egret.test. Both halves must come from it, or a
    // consumer fetches a token from one origin and calls another.
    // CORRECTED AFTER MERGE REVIEW: the slug belongs here, and asserting the
    // base ALONE is why its absence shipped. See
    // `the_documented_url_is_the_url_try_it_calls`, which composes
    // servers[0].url with a paths key and compares against try_it's echo.
    assert_eq!(server, format!("https://egret.test/data-api/v1/{}", f.ws_slug));
    assert!(token_url.starts_with("https://egret.test/"));
}

#[tokio::test]
async fn a_draft_endpoint_is_not_documented() {
    let Some(f) = fixture().await else { return };
    let draft = f.create_draft_endpoint("unpublished", "/unpublished").await;
    let (_, doc) = f
        .mgmt_get(&format!(
            "/api/ws/{}/data-api/openapi?service_account_id={}",
            f.ws_slug, f.admin
        ))
        .await;
    assert!(
        !doc["paths"].as_object().unwrap().contains_key("/unpublished"),
        "a draft has no published contract to describe"
    );
    let _ = draft;
}
```

- [ ] **Step 2: Run to verify failure**

Run: `DATABASE_URL=postgres://localhost/egret_test cargo test -p egret-dataapi --test openapi_gen -- --nocapture`
Expected: FAIL with 404.

- [ ] **Step 3: Write the generator**

```rust
//! Data API P5 §5 — the OpenAPI document a consumer is handed.
//!
//! Assembled as `serde_json::Value` rather than through utoipa, because utoipa
//! describes THIS server's compiled routes and these paths exist only in a
//! user's data.
//!
//! OpenAPI **3.0.3**, not 3.1: this artifact's entire purpose is being consumed
//! by someone else's tooling, and Postman and openapi-generator support 3.0 far
//! more widely.

use crate::routes::DataApiState;
use egret_core::ids::UserId;
use egret_core::WorkspaceId;
use egret_dataapi_api::{EndpointSnapshot, ParamLocation, ParamType};
use serde_json::{json, Map, Value};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum OpenApiGenError {
    /// The endpoint's `output.naming` maps two columns onto one key. Serving
    /// answers `output_naming_collision` (503) for such a version, so a document
    /// describing it would describe something that never returns 200 — and a
    /// document quietly missing a column is worse than no document.
    #[error("endpoint {endpoint} has an output naming collision; re-publish it")]
    NamingCollision { endpoint: String },
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Core(#[from] egret_core::Error),
}

/// Map a raw engine type string onto a JSON Schema type, conservatively.
///
/// `ColumnSpec.ty` is whatever the engine called it, and Trino and StarRocks do
/// not agree. Anything unrecognised becomes `string` and keeps its original
/// spelling in `x-egret-engine-type`: a confident wrong mapping sends a client
/// to parse a number that is not one, while `string` is merely imprecise.
fn json_type(engine_ty: &str) -> &'static str {
    let t = engine_ty.trim().to_ascii_lowercase();
    let head = t.split(['(', '<']).next().unwrap_or("").trim();
    match head {
        "boolean" | "bool" => "boolean",
        "tinyint" | "smallint" | "int" | "integer" | "bigint" | "largeint" => "integer",
        "real" | "float" | "double" | "decimal" | "numeric" => "number",
        _ => "string",
    }
}

fn param_schema(p: &egret_dataapi_api::ParamSpec) -> Value {
    let mut schema = match p.ty {
        ParamType::Number => json!({ "type": "number" }),
        ParamType::Boolean => json!({ "type": "boolean" }),
        ParamType::Date => json!({ "type": "string", "format": "date" }),
        ParamType::Timestamp => json!({ "type": "string", "format": "date-time" }),
        ParamType::String | ParamType::Enum => json!({ "type": "string" }),
    };
    if matches!(p.ty, ParamType::Enum) {
        if let Some(allowed) = &p.allowed {
            schema["enum"] = json!(allowed);
        }
    }
    schema
}

/// The two behaviours a caller cannot see in the schema and will otherwise meet
/// as wrong data rather than as an error.
///
/// A repeated query parameter is a LIST, one bind per value — but `ParamSpec`
/// carries no multiplicity, so the generator genuinely cannot tell which
/// parameters accept repetition. Declaring `oneOf [scalar, array]` to cover it
/// would break most generators; saying it in prose does not.
///
/// And `+` decodes to a SPACE (measured 2026-08-16), so `?token=a+b` binds
/// `"a b"` and returns 200 with rows for a value nobody typed.
fn param_description(p: &egret_dataapi_api::ParamSpec) -> String {
    let mut d = p.description.clone();
    if !d.is_empty() {
        d.push_str("\n\n");
    }
    d.push_str(
        "Repeating this parameter supplies a list (`?x=a&x=b`), one bound value per \
         occurrence. `+` in a value decodes to a space — send a literal `+` as `%2B`.",
    );
    d
}
```

Then `build` itself. Note that it calls `crate::serving::router::apply_naming` — change that function from private to `pub(crate)` in `crates/egret-dataapi/src/serving/router.rs:1145`. **Do not copy it.** It already has unit tests pinning `camel`/`snake` behaviour, and a second copy would drift from what serving actually returns, which is the one thing this document must not misdescribe.

```rust
/// The response schema for one published version.
///
/// `output.shape` decides the container: `objects` gives one property per
/// column, `arrays` gives a positional tuple. Property names go through
/// `apply_naming` — the SAME function serving calls — because the document must
/// name the keys a consumer will actually receive.
fn data_schema(snap: &EndpointSnapshot, endpoint: &str) -> Result<Value, OpenApiGenError> {
    if snap.output.shape == "arrays" {
        // A positional row: OpenAPI 3.0 has no tuple type, so this is an array
        // of mixed scalars and the column order is documented in the
        // description rather than pretended into the schema.
        return Ok(json!({
            "type": "array",
            "description": format!(
                "Positional row. Column order: {}",
                snap.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")
            ),
            "items": {}
        }));
    }
    let mut props = Map::new();
    for c in &snap.columns {
        let key = crate::serving::router::apply_naming(&c.name, &snap.output.naming);
        // Serving answers `output_naming_collision` (503) for this version, so
        // there is no document to write: emitting the surviving column would
        // describe a response that never comes, and quietly dropping one is
        // worse than refusing.
        if props.contains_key(&key) {
            return Err(OpenApiGenError::NamingCollision {
                endpoint: endpoint.to_string(),
            });
        }
        props.insert(
            key,
            json!({
                "type": json_type(&c.ty),
                "nullable": true,
                "x-egret-engine-type": c.ty,
            }),
        );
    }
    Ok(json!({ "type": "object", "properties": props }))
}

/// The error responses, identical for every path, referenced not inlined.
fn error_responses() -> Value {
    let err = json!({
        "description": "Error",
        "content": { "application/json": {
            "schema": { "$ref": "#/components/schemas/ServingErrorBody" }
        }}
    });
    json!({
        "400": err, "401": err, "403": err,
        "404": err, "429": err, "500": err, "503": err,
    })
}

/// Build the document for one service account.
///
/// `only` narrows to a single endpoint — the per-endpoint download is this same
/// builder, not a second mechanism that could describe an endpoint differently
/// from the aggregate.
pub async fn build(
    st: &DataApiState,
    ws: WorkspaceId,
    service_account: UserId,
    only: Option<Uuid>,
) -> Result<Value, OpenApiGenError> {
    // `state = 'live'` AND a live version: a draft has no published contract,
    // and a disabled endpoint answers 404, so documenting either describes
    // something that does not happen.
    let rows: Vec<(Uuid, String, String, String, String, Value, i32)> = sqlx::query_as(
        "SELECT e.id, e.name, e.description, e.path, e.method, ev.snapshot, ev.version \
         FROM dataapi.endpoints e \
         JOIN dataapi.endpoint_versions ev ON ev.id = e.live_version_id \
         WHERE e.workspace_id = $1 AND e.state = 'live' \
           AND ($2::uuid IS NULL OR e.id = $2) \
         ORDER BY e.path",
    )
    .bind(ws.0)
    .bind(only)
    .fetch_all(&st.db.pool)
    .await?;

    let base = st.issuer.trim_end_matches('/');
    let mut paths = Map::new();

    for (id, name, description, path, method, snapshot, version) in rows {
        // THE line this whole module turns on. A scan of `access_grants` would
        // answer "no" for any principal reaching invoke through the ws_admin
        // cascade and silently omit those paths.
        if !crate::invoke::can_invoke(st, service_account, ws, id).await? {
            continue;
        }
        let snap: EndpointSnapshot = serde_json::from_value(snapshot)
            .map_err(|e| OpenApiGenError::Db(sqlx::Error::Decode(Box::new(e))))?;

        let params: Vec<Value> = snap
            .params
            .iter()
            // `location: body` is refused at publish and `serve_post` has no
            // body extractor, so nothing can supply one. Skipped rather than
            // trusted, for snapshots frozen before that refusal existed.
            .filter(|p| matches!(p.location, ParamLocation::Query))
            .map(|p| {
                json!({
                    "name": p.name,
                    "in": "query",
                    "required": p.required,
                    "description": param_description(p),
                    "schema": param_schema(p),
                })
            })
            .collect();

        let mut responses = Map::new();
        responses.insert(
            "200".into(),
            json!({
                "description": "Rows",
                "content": { "application/json": { "schema": {
                    "type": "object",
                    "properties": {
                        "data": { "type": "array", "items": data_schema(&snap, &name)? },
                        "meta": { "$ref": "#/components/schemas/ServingMeta" }
                    }
                }}}
            }),
        );
        if let Value::Object(errs) = error_responses() {
            responses.extend(errs);
        }

        let op = json!({
            "operationId": format!("dataApi_{}", id.simple()),
            "summary": name,
            "description": description,
            "parameters": params,
            "responses": responses,
            // Which frozen contract this description belongs to. Without it a
            // consumer holding a stale file cannot tell that it is stale.
            "x-egret-version": version,
        });

        let verb = method.to_ascii_lowercase();
        paths
            .entry(path)
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("path item is an object")
            .insert(verb, op);
    }

    Ok(json!({
        // 3.0.3, not 3.1 — see the module comment.
        "openapi": "3.0.3",
        "info": { "title": "Data API", "version": "1.0.0" },
        // Both halves of the origin come from `issuer`. See spec §5.3: if these
        // could disagree, one file would send a consumer to fetch a token from
        // one host and call another.
        // CORRECTED AFTER MERGE REVIEW — this line as planned was WRONG.
        // The serving MOUNT is /data-api/v1; the serving ROUTE is
        // /data-api/v1/{ws}/{*path}. Without the workspace slug here, a
        // consumer joining servers[0].url to a paths key binds `ws` to the
        // first path segment and EVERY documented operation answers 404.
        // Shipped as: format!("{base}/data-api/v1/{slug}"), with `slug`
        // resolved from the authorized WorkspaceId exactly as try_it does.
        // See spec §5.3.
        "servers": [ { "url": format!("{base}/data-api/v1/{slug}") } ],
        "security": [ { "egret_oauth2": [] } ],
        "components": {
            "securitySchemes": {
                "egret_oauth2": {
                    "type": "oauth2",
                    "description": "Machine-IdP client_credentials. The client id and \
                                    secret are the ones issued with the service account.",
                    "flows": { "clientCredentials": {
                        "tokenUrl": format!("{base}/oauth2/token"),
                        "scopes": {}
                    }}
                }
            },
            "schemas": {
                "ServingErrorBody": {
                    "type": "object",
                    "properties": {
                        "code": { "type": "string" },
                        "message": { "type": "string" },
                        "request_id": { "type": "string" }
                    }
                },
                "ServingMeta": {
                    "type": "object",
                    "properties": {
                        "columns": { "type": "array", "items": { "type": "object" } },
                        "row_count": { "type": "integer" },
                        "version": { "type": "integer" },
                        "cached": { "type": "boolean" },
                        "request_id": { "type": "string" }
                    }
                }
            }
        },
        "paths": paths,
    }))
}
```

Confirm the machine-IdP token path before relying on `"{base}/oauth2/token"`: read what `issue_client` returns as `token_endpoint` in `crates/egret-dataapi/src/routes.rs` and use **that** derivation verbatim. The two must agree — a consumer already holds the value that route gave them.

- [ ] **Step 4: Mount the route**

Gate `EP_READ`; query params `service_account_id: Uuid` (required) and `endpoint_id: Option<Uuid>`. `NamingCollision` maps to 409 with a message naming the endpoint and telling the author to re-publish.

- [ ] **Step 5: Run the tests**

Run: `DATABASE_URL=postgres://localhost/egret_test cargo test -p egret-dataapi --test openapi_gen -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Verify the cascade test is not vacuous**

Replace the `can_invoke` filter with a scan of `access_grants` for an object-scoped row (the implementation §5.2 warns against), rebuild, confirm 0 errors, re-run.
Expected: `a_ws_admin_gets_cascade_reachable_paths` FAILS with an empty `paths` object while every other test still passes. That asymmetry is the point of the test. Restore.

- [ ] **Step 7: Regenerate OpenAPI and commit**

```bash
cargo run -p egret-bin -- openapi > openapi.json
cargo test -p egret-server committed_openapi_json_is_current
git add crates/egret-dataapi/src/openapi_gen.rs crates/egret-dataapi/src/routes.rs \
        crates/egret-dataapi/src/lib.rs crates/egret-dataapi/tests/openapi_gen.rs openapi.json
git commit -m "feat(dataapi): generate a consumer-facing OpenAPI document"
```

---

### Task 7: Try-it proxy

**Files:**
- Create: `crates/egret-dataapi/src/try_it.rs`
- Modify: `crates/egret-dataapi/src/routes.rs`, `crates/egret-dataapi/src/serving/access_log.rs` (nothing — Task 1 already added the field)
- Test: `crates/egret-dataapi/tests/try_it.rs`

**Interfaces:**
- Consumes: `AccessLogEntry.actor_user_id` (Task 1), `serving_router()`.
- Produces: `TryRequest { service_account_id: Uuid, params: BTreeMap<String, Vec<String>> }`, `TryResponse { status: u16, body: Value, request: TryRequestEcho }`.

- [ ] **Step 1: Write the failing tests — the router one first**

```rust
#[tokio::test]
async fn a_trial_without_a_grant_gets_the_real_403() {
    let Some(f) = fixture().await else { return };
    // principal_b holds no invoke grant on this endpoint.
    let (status, body) = f
        .mgmt_post(
            &format!("/api/ws/{}/data-api/endpoints/{}/try", f.ws_slug, f.endpoint_id),
            serde_json::json!({ "service_account_id": f.principal_b.0, "params": {} }),
        )
        .await;

    assert_eq!(status, 200, "the trial itself succeeded; the CALL did not");
    assert_eq!(body["status"], 403);
    assert_eq!(
        body["body"]["code"], "not_granted",
        "Try-it must not bypass a single gate — this is the most common first failure"
    );
}

#[tokio::test]
async fn a_trial_is_recorded_as_a_trial() {
    let Some(f) = fixture().await else { return };
    f.mgmt_post(
        &format!("/api/ws/{}/data-api/endpoints/{}/try", f.ws_slug, f.endpoint_id),
        serde_json::json!({ "service_account_id": f.principal_a.0, "params": {} }),
    )
    .await;

    let actor: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT actor_user_id FROM dataapi.access_log \
         WHERE workspace_id = $1 ORDER BY ts DESC LIMIT 1",
    )
    .bind(f.ws_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("access_log row");

    assert_eq!(
        actor,
        Some(f.author),
        "a trial writes to the access log and must be identifiable as one"
    );
}

#[tokio::test]
async fn the_token_never_reaches_the_caller() {
    let Some(f) = fixture().await else { return };
    let (_, body) = f
        .mgmt_post(
            &format!("/api/ws/{}/data-api/endpoints/{}/try", f.ws_slug, f.endpoint_id),
            serde_json::json!({ "service_account_id": f.principal_a.0, "params": {} }),
        )
        .await;
    let text = body.to_string();
    assert!(text.contains("Bearer \\u2022") || text.contains("Bearer •"));
    assert!(
        !text.contains("eyJ"),
        "no JWT may appear anywhere in the response body"
    );
}
```

- [ ] **Step 2: Run to verify failure**

Run: `DATABASE_URL=postgres://localhost/egret_test cargo test -p egret-dataapi --test try_it -- --nocapture`
Expected: FAIL with 404.

- [ ] **Step 3: Write the handler**

Key points the implementation must hold:

```rust
//! Data API P5 §3 — running an endpoint as a service account, from the console.
//!
//! # It drives the REAL serving router
//!
//! `serving_router()` is nested into a throwaway `Router` and called with
//! `tower::ServiceExt::oneshot`, exactly as the P4 test suites drive it. Same
//! router, same layers, same handler a consumer traverses.
//!
//! **Not a re-implemented equivalent, and not an HTTP loopback.** A loopback
//! would need the server's own URL and TLS. A re-implementation is worse than
//! either: P4.5 repeatedly showed a simulated path staying green while the real
//! one failed, and a Try-it that says OK while consumers get 403 converts the one
//! tool that could have found the problem into the reason nobody looked.
//!
//! # It bypasses nothing
//!
//! Rate limit, cache, Ranger, the invoke grant and the access log all apply for
//! real. Three consequences the UI must state rather than let someone discover:
//! a trial spends the service account's rate-limit budget; a trial writes to
//! `access_log` (hence `actor_user_id`); and on a `cache.scope: shared` endpoint
//! a trial can populate the cache real consumers read.
```

```rust
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tower::ServiceExt;
use uuid::Uuid;

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TryRequest {
    pub service_account_id: Uuid,
    /// Values exactly as they would arrive on the query string. A `Vec` per key
    /// because a repeated parameter is a LIST (P4.5), and a trial that could
    /// only send one value could not exercise the case most likely to be wrong.
    #[serde(default)]
    pub params: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TryResponse {
    /// The status the SERVING path returned. The trial itself is a 200 whenever
    /// it ran — a 403 from the endpoint is a result, not a failure of the trial.
    pub status: u16,
    pub body: serde_json::Value,
    pub request: TryRequestEcho,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TryRequestEcho {
    pub method: String,
    pub url: String,
    /// Always the literal `Bearer ••••`. The token is never serialised.
    pub authorization: String,
    /// A copyable reproduction using a `$TOKEN` placeholder.
    pub curl: String,
}

/// Marker inserted into the request the management handler builds, carrying the
/// human who ran the trial.
///
/// **An extension, not a header and not a field on the token.** A header could
/// be sent by any consumer, and the serving path would then be trusting the
/// caller's word about whether their own call was a trial. Only an in-process
/// handler can insert an extension, so `actor_user_id` cannot be forged from
/// outside — which is what makes filtering trials out of statistics a fact
/// rather than a convention.
#[derive(Clone, Copy)]
pub struct TrialActor(pub egret_core::ids::UserId);
```

The handler:

```rust
async fn try_endpoint(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, id)): Path<(String, Uuid)>,
    Json(body): Json<TryRequest>,
) -> Result<Json<TryResponse>, DataApiError> {
    // `:manage`, not `:read` — this runs a query as somebody else.
    egret_core::authz::require(&*st.pdp, &ctx, EP_MANAGE).await?;
    // 404s an endpoint outside this workspace before anything is minted.
    let endpoint = st.endpoints.get(ctx.workspace_id.0, id).await?;
    // Same containment for the account named in the BODY: an id from another
    // workspace must not become a token.
    let account = scoped_account(&st, ctx.workspace_id.0, body.service_account_id).await?;

    // 60 seconds: long enough for one in-process call, short enough that a leak
    // through a log or a crash dump is spent before it is read.
    let token = mint_trial_token(&st, ctx.workspace_id.0, &account)?;

    let mut qs = form_urlencoded::Serializer::new(String::new());
    for (k, values) in &body.params {
        for v in values {
            qs.append_pair(k, v);
        }
    }
    let query = qs.finish();
    let uri = if query.is_empty() {
        format!("/data-api/v1/{}/{}", ctx.workspace_slug, endpoint.path)
    } else {
        format!("/data-api/v1/{}/{}?{query}", ctx.workspace_slug, endpoint.path)
    };

    // THE REAL ROUTER. Same nest, same layers, same handler a consumer reaches.
    let router = axum::Router::new()
        .nest("/data-api/v1", crate::serving_router())
        .with_state(st.clone());
    let req = Request::builder()
        .method(endpoint.method.as_str())
        .uri(&uri)
        .header("authorization", format!("Bearer {token}"))
        .extension(TrialActor(ctx.principal.user_id))
        .body(Body::empty())
        .map_err(|e| DataApiError::Internal(e.to_string()))?;

    let res = router
        .oneshot(req)
        .await
        .map_err(|e| DataApiError::Internal(e.to_string()))?;
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 4 * 1024 * 1024)
        .await
        .map_err(|e| DataApiError::Internal(e.to_string()))?;
    let parsed: serde_json::Value =
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);

    // An impersonation action, audited like one. Precedent: user impersonation
    // (#332). The token is not in the detail and never will be.
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "data_api_endpoint:try",
        "data_api_endpoint",
        Some(id.to_string()),
        serde_json::json!({
            "service_account_id": body.service_account_id,
            "status": status.as_u16(),
        }),
    ));

    Ok(Json(TryResponse {
        status: status.as_u16(),
        body: parsed,
        request: TryRequestEcho {
            method: endpoint.method.clone(),
            url: format!("{}{uri}", st.issuer.trim_end_matches('/')),
            // Never the token. Not truncated, not prefixed — absent.
            authorization: "Bearer ••••".into(),
            curl: format!(
                "curl -H 'Authorization: Bearer $TOKEN' '{}{uri}'",
                st.issuer.trim_end_matches('/')
            ),
        },
    }))
}
```

And in `serving/router.rs`'s `serve`, read the extension the handler inserted — this is the only place `actor_user_id` is ever set:

```rust
    // Present only when an in-process management handler built this request; a
    // real consumer cannot insert an extension, so this cannot be forged.
    if let Some(TrialActor(actor)) = req_extensions.get::<TrialActor>().copied() {
        entry.actor_user_id = Some(actor.0);
    }
```

`serve`'s signature gains an `Extension`-capable extractor for this. Prefer `axum::extract::Extension<Option<TrialActor>>` over reading raw parts, so the absence case is typed rather than a lookup that silently returns `None` if the extractor were ever reordered.

`mint_trial_token` signs the same claim shape `verify_serving_token` checks — `iss` from `st.issuer`, `aud` `data-api`, `ws`, `egret_user_id`, `exp` at +60s, and `sub` set to a client id belonging to the account. **Read `verify_serving_token` before writing this** and match it field for field; a claim shape guessed from the test fixture's `mint` helper is a guess about a test, not about the verifier.

- [ ] **Step 4: Run the tests**

Run: `DATABASE_URL=postgres://localhost/egret_test cargo test -p egret-dataapi --test try_it -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Verify the router test is not vacuous — this is the important one**

Temporarily change the serving handler `serve` to return `StatusCode::IM_A_TEAPOT` before doing anything. Rebuild, **confirm 0 compile errors**, re-run.
Expected: `a_trial_without_a_grant_gets_the_real_403` FAILS with 418. If it still passes, the handler is not driving the real router and §3.1's entire guarantee is decoration — stop and fix that before continuing. Restore.

- [ ] **Step 6: Regenerate OpenAPI and commit**

```bash
cargo run -p egret-bin -- openapi > openapi.json
cargo test -p egret-server committed_openapi_json_is_current
git add crates/egret-dataapi/src/try_it.rs crates/egret-dataapi/src/routes.rs \
        crates/egret-dataapi/src/lib.rs crates/egret-dataapi/tests/try_it.rs openapi.json
git commit -m "feat(dataapi): try-it proxy through the real serving router"
```

---

### Task 8: Map engine permission errors to the missing step

**Files:**
- Modify: `crates/egret-dataapi/src/serving/error.rs`
- Test: `crates/egret-dataapi/tests/serving_errors.rs`

**Interfaces:**
- Produces: a `ranger_denied` error code distinct from `not_granted`.

Spec §6.3 point 3, and the base spec's §5.3 has required it since P0: publishing does not grant data access, so the common failure is an endpoint that publishes fine and then fails inside the engine because the service account has no Ranger permission. Today that surfaces as a generic engine failure, which sends the author to debug their SQL — the one thing that is not wrong.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn an_engine_permission_error_names_the_missing_step() {
    let Some(f) = fixture().await else { return };
    // The fake port answers as a Ranger-enforcing engine does when the principal
    // may not read the table.
    f.port.fail_next_with("Access Denied: Cannot select from table sales");

    let res = f.call_as_client(f.principal_a, "client-a", "sales").await;

    assert_eq!(res.status, 403);
    assert_eq!(res.body["code"], "ranger_denied");
    // The message must point at the grant, not at the SQL. An author reading a
    // raw engine error debugs their query — which is correct.
    let msg = res.body["message"].as_str().unwrap();
    assert!(msg.contains("service account"), "{msg}");
    assert!(
        !msg.contains("Cannot select from table"),
        "the raw engine error must not be echoed: {msg}"
    );
}
```

`fail_next_with` is a new `FakePort` helper in `serving_common/mod.rs` that makes the next `execute_as*` return `Err(egret_core::Error::…)` carrying that message.

- [ ] **Step 2: Run to verify failure**

Run: `DATABASE_URL=postgres://localhost/egret_test cargo test -p egret-dataapi --test serving_errors an_engine_permission_error -- --nocapture`
Expected: FAIL — the current code returns a generic engine error, not `ranger_denied`.

- [ ] **Step 3: Classify the error**

```rust
/// Does this engine failure mean "the principal may not read this data"?
///
/// Matched on the message because neither connector surfaces a typed
/// authorization error: Trino answers `Access Denied: …` and StarRocks answers
/// with its own permission wording. **A substring match is a heuristic and is
/// documented as one** — a false negative merely returns today's generic error,
/// which is why the match is deliberately narrow rather than clever.
///
/// Verify these strings against a real engine before trusting them; this
/// codebase has been wrong three times about engine behaviour asserted from the
/// wrong instrument.
fn is_permission_denied(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("access denied")
        || m.contains("permission denied")
        || m.contains("not authorized")
}
```

And where an engine error becomes a `ServingError`, route it:

```rust
    if is_permission_denied(&msg) {
        // 403 with a code of its own: `not_granted` means "no invoke grant in
        // egret", and conflating the two would send an admin to write a grant
        // that already exists while the actual gap is in Ranger.
        return ServingError::RangerDenied;
    }
```

with the body message:

```text
The service account running this endpoint is not permitted to read the
underlying data. Publishing an endpoint does not grant data access — grant the
service account in Ranger as well.
```

- [ ] **Step 4: Run the test**

Run: `DATABASE_URL=postgres://localhost/egret_test cargo test -p egret-dataapi --test serving_errors -- --nocapture`
Expected: PASS, and the rest of the error suite unchanged.

- [ ] **Step 5: Verify against a real engine**

Run the manual Trino integration test and confirm a genuinely unauthorized query produces a message `is_permission_denied` matches. A string heuristic asserted only against a fake port is a claim about the fake.

- [ ] **Step 6: Document the new code**

Add `ranger_denied` to the base spec's §7 error-code table (`docs/en/specs/2026-08-14-data-api-design.md` + vi mirror), and to the `403` description on both `#[utoipa::path]` blocks in `crates/egret-dataapi/src/serving/router.rs:304,334`.

- [ ] **Step 7: Regenerate OpenAPI and commit**

```bash
cargo run -p egret-bin -- openapi > openapi.json
cargo test -p egret-server committed_openapi_json_is_current
git add crates/egret-dataapi/src/serving/ crates/egret-dataapi/tests/ \
        docs/en/specs/2026-08-14-data-api-design.md \
        docs/vi/specs/2026-08-14-data-api-design.md openapi.json
git commit -m "feat(dataapi): map engine permission failures to a ranger_denied error"
```

---

### Task 9: Access-console arm and the member-picker over-grant

**Files:**
- Modify: `crates/egret-bi/src/routes.rs:3375` (`kind_perms`)
- Modify: the workspace member picker's principal search (`crates/egret-workspace/`) — label or exclude `users.provider = 'service'`
- Test: `crates/egret-bi/tests/` (folder routes), `crates/egret-workspace/tests/`

**Interfaces:**
- Consumes: nothing.
- Produces: `data_api_endpoints` as a valid `{kind}` segment on the generic folder router.

- [ ] **Step 1: Write the failing test for `kind_perms`**

```rust
#[tokio::test]
async fn data_api_endpoint_folders_are_a_known_kind() {
    let Some(f) = fixture().await else { return };
    let (status, _) = f
        .get(&format!("/api/ws/{}/folders/data_api_endpoints", f.ws_slug))
        .await;
    assert_eq!(
        status, 200,
        "endpoints must appear in the Access Control console like dashboards and datasets"
    );
}
```

- [ ] **Step 2: Run to verify failure**

Run: `DATABASE_URL=postgres://localhost/egret_test cargo test -p egret-bi data_api_endpoint_folders -- --nocapture`
Expected: FAIL with 404 — `kind_perms` returns `None`.

- [ ] **Step 3: Add the arm**

```rust
        // Data API endpoints live in egret-dataapi and share this router the way
        // notebooks and alerts do; both permissions exist from migration 0122.
        // Literal strings, again to avoid a cross-crate dependency.
        "data_api_endpoints" => Some((
            "data_api_endpoint",
            "data_api_endpoint:read",
            "data_api_endpoint:manage",
        )),
```

- [ ] **Step 4: Write the failing test for the member picker**

```rust
#[tokio::test]
async fn the_member_picker_marks_service_accounts() {
    let Some(f) = fixture().await else { return };
    let sa = f.create_service_account("reporting").await;

    let (status, body) = f.get(&format!("/api/ws/{}/members/search?q=reporting", f.ws_slug)).await;
    assert_eq!(status, 200);

    let hit = body
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == sa.to_string())
        .expect("the account is findable");

    // Inviting a service account as ws_admin gives it data_api_endpoint:invoke
    // (migration 0124), which the Cedar cascade applies to EVERY endpoint in the
    // workspace — a full-catalog grant issued through a member picker. Whatever
    // the UI does with it, the API must not hand back a row that looks like a
    // person.
    assert_eq!(hit["provider"], "service");
}
```

- [ ] **Step 5: Run to verify failure, then surface `provider`**

Run: `DATABASE_URL=postgres://localhost/egret_test cargo test -p egret-workspace the_member_picker_marks -- --nocapture`
Expected: FAIL — no `provider` field. Add it to the search DTO and its `SELECT`.

- [ ] **Step 6: Assert `data_api_invoker` is never an offerable membership role**

```rust
#[tokio::test]
async fn data_api_invoker_is_not_a_membership_role() {
    let Some(f) = fixture().await else { return };
    let (_, roles) = f.get(&format!("/api/ws/{}/roles", f.ws_slug)).await;
    assert!(
        !roles.to_string().contains("data_api_invoker"),
        "it carries invoke on every endpoint through the workspace cascade"
    );
}
```

If this fails, filter the role out of the list the picker is served.

- [ ] **Step 7: Run both suites and commit**

```bash
DATABASE_URL=postgres://localhost/egret_test cargo test -p egret-bi -p egret-workspace
git add crates/egret-bi/src/routes.rs crates/egret-workspace/
git commit -m "feat(access): expose data_api_endpoints to the access console; label service accounts"
```

---

## Final verification

- [ ] **Run the full backend suite.** `DATABASE_URL=postgres://localhost/egret_test cargo test --workspace`. Note that it **stops at the first failing binary**, so a clean tail is not proof the head passed — re-run any binary that did not report.
- [ ] **Run `cargo fmt --all --check` and `cargo clippy --workspace --all-targets`.** CI runs fmt; a formatting-only failure after a long test run is a wasted cycle.
- [ ] **Confirm `openapi.json` is current.** `cargo test -p egret-server committed_openapi_json_is_current`.
- [ ] **Confirm route classification.** `cargo test -p egret-server --test feature_route_coverage` — new routes sit under the already-classified `data-api` segment, so this should pass; if it does not, the catalog needs the new segment.
- [ ] **Live check, not just green tests.** Start the local stack, publish an endpoint, and drive Try-it, the OpenAPI download and Activity in a browser. Every chart defect that mattered in this codebase was found there and not by a passing suite — and P4's worst defect lived in the seam between two tasks that each passed their own.

## Notes for the frontend plan that follows

- `ActivityRow` sends raw counts, not rates. The UI divides, renders `—` when `cacheable_calls` is 0, and labels p50/p95 "across successful calls".
- `ActivitySummaryDto.retention_days` carries the configured retention already run through `scheduled_retention` (spec §4.7). Disable presets longer than it; `null` means no prune is scheduled, so every range is honest.
- `ranger_denied` (403) is the code the Access section's "publishing does not grant data access" message keys off — it is distinct from `not_granted`, which means the invoke grant is missing. The two failures have different fixes and must not share a screen state.
- `GET …/openapi` returns 409 `NamingCollision` — the UI shows the endpoint name and "re-publish it", not a generic failure.
- The Access checklist needs a per-service-account invoke status that distinguishes *granted on this endpoint* from *inherited from the workspace*. `can_invoke` answers the combined question; the existing `/grants` list answers the object-scoped half. The UI needs **both**, and the difference between them is exactly what tells the two states apart.
