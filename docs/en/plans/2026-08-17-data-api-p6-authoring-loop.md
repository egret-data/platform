# Data API P6 — Authoring Loop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An author can see the rows their draft SQL returns, without saving, publishing, or leaving the page — and the endpoint editor is split so the query gets a surface of its own.

**Architecture:** One new backend route, `POST /api/ws/{workspace}/data-api/endpoints/{id}/preview`, gated on `data_api_endpoint:manage`, which runs the on-screen draft as the author through the *same* validation and render path publish and serving use. The frontend detail route grows from five sections to six: `Definition` splits into `Query` (database, SQL, parameter declarations, preview) and `Config` (identity + the three existing config tables), and the three run-something buttons are relabelled so each says which version it runs.

**Tech Stack:** Rust — axum 0.8, sqlx 0.9, utoipa, tower `ServiceExt::oneshot`, minijinja (via `crate::render`). Frontend — React 18, TypeScript, react-router-dom, TanStack Query via orval-generated hooks, shadcn/Radix, Tailwind v4 OKLCH tokens, vitest + Testing Library.

**Spec:** `docs/en/specs/2026-08-17-data-api-authoring-loop-design.md` (English canonical; Vietnamese mirror alongside it). Where this plan and the spec disagree, ask — do not pick.

## Global Constraints

- **Preview is gated on `data_api_endpoint:manage`, never `:read`.** egret's permissions are FLAT: `:manage` implies no `:read` and `:read` implies no `:manage`. Only an explicit `egret_core::authz::require(&*st.pdp, &ctx, EP_MANAGE)` stands between a reader and this route.
- **Preview must reuse serving's render path, not a second one.** It renders through `check_render_rules` (which calls the behavioural gate `crate::render::check_publish`) and binds through `crate::render::render`. Do not write a second renderer, a second parameter coercion, or a second placeholder check.
- **Preview must enforce single-SELECT.** `data_api_endpoint:manage` is not `sqllab:write`. A preview that ran arbitrary statements would hand a role holding only `:manage` write access to the engine. `check_single_select` is load-bearing authorization here, not tidiness.
- **Preview caps at exactly 100 rows**, fixed, not configurable, and the endpoint's own `limits.max_rows` does not apply. The response says whether it truncated.
- **Preview does none of the four consumer-facing things:** no `access_log` row, no rate-limit consumption, no cache read, no cache write.
- **Preview returns the engine's own column names** — `output.naming` is NOT applied — **and the engine's own error text verbatim.** Serving's `data_access_denied` sanitising is deliberately inverted here. This is why the `:manage` gate is load-bearing.
- **Preview emits no audit event.** `validate` does not either; both run a read-only query as the caller themselves.
- **Single-statement SQL stays enforced, parameters stay query-only.** No `location: body`, no `location` control, no script engine, no multi-statement SQL. Anything in the spec's §7 is out of scope.
- **A body-supplied `database_id` must be workspace-scoped.** `EndpointStore::connection_per_principal_pair` answers `NotFound("database")` for a database outside the workspace — call it, and do not bypass it.
- **DB-gated backend tests silently pass when `DATABASE_URL` is unset.** Run them against `egret_test` and read the *duration*, not just the `ok`. Never point them at a dev DB.
- **Every assertion is mutation-checked before being believed**, and a mutation that fails to compile proves nothing: build first, confirm zero compile errors, *then* read the test result.
- **Frontend: no msw.** Tests `vi.mock` the API module and use `vi.mocked(hook).mockReturnValue(...)`.
- **Frontend: CSRF is free through the generated client.** Never hand-roll a `fetch`; `api/mutator.ts#customInstance` attaches `X-Egret-CSRF`.
- **`ConfigTableRow.details` is a plain string with no markdown rendering.** Backticks in it render as literal backtick characters. Do not widen the prop to `ReactNode`.
- **`Badge variant="success"` is a hardcoded `bg-green-600`, not the OKLCH token.** Use `bg-success/15 text-success`.
- **`eslint.config.js` enables `react-hooks/set-state-in-effect`.** The frontend baseline is **0 errors / 178 warnings** — hold it; do not add warnings.
- **`openapi.json` at the repo root is a committed artifact with a byte-identity gate** in a *different* crate (`egret-server`'s `committed_openapi_json_is_current`). Any utoipa change must regenerate it or that gate fails.

---

## File Structure

**Backend**

| File | Responsibility after this plan |
|---|---|
| `crates/egret-dataapi/src/lifecycle.rs` | Gains `check_definition_scans` (the shared scan bundle) and `validate_preview`. `validate_definition` keeps its exact current check ORDER. |
| `crates/egret-dataapi/src/serving/params.rs` | `resolve` splits into `decode` + a new `resolve_supplied`, which preview calls with an already-decoded map so nothing is re-encoded into a fake query string. |
| `crates/egret-dataapi/src/preview.rs` | **New.** The preview expression: validate → resolve → render → execute. Deliberately its own file rather than more mass in `routes.rs` (85 KB) or `lifecycle.rs` (122 KB). |
| `crates/egret-dataapi/src/routes.rs` | The `preview_endpoint` handler, its `#[utoipa::path]`, the `.route(...)` line, and the `paths(...)` registration. |
| `crates/egret-dataapi/src/lib.rs` | `mod preview;` |
| `crates/egret-dataapi/tests/preview.rs` | **New.** The six backend tests of spec §5, on `serving_common`'s fixture. |
| `openapi.json` | Regenerated. |

**Frontend**

| File | Responsibility after this plan |
|---|---|
| `frontend/src/routes/explore/dataapi/detail/Query.tsx` | **New.** Database picker, SQL editor, `ParamEditor`, and the preview block. The detail route's index. |
| `frontend/src/routes/explore/dataapi/detail/PreviewBlock.tsx` | **New.** Test-value inputs, the Preview button, and the result table. Split from `Query.tsx` so the section file stays about layout and this file owns one interaction. |
| `frontend/src/routes/explore/dataapi/detail/Config.tsx` | **New.** An `Endpoint` group (name, description, method, path) plus the Cache / Limits / Rate limit `ConfigTable`s moved verbatim out of `Definition.tsx`. Owns Validate. |
| `frontend/src/routes/explore/dataapi/detail/Definition.tsx` | **Deleted** at the end of Task 4. |
| `frontend/src/routes/explore/dataapi/EndpointDetail.tsx` | `SECTIONS` gains `Query`/`Config`; the Save button's label is unchanged. |
| `frontend/src/App.tsx` | `index` → `<Query />`, new `config` child route. |
| `frontend/src/api/dataApi.ts` | Untouched. Preview is not a mutation whose success invalidates a list — see Task 2. |
| `frontend/src/routes/explore/dataapi/__tests__/EndpointDetail.test.tsx` | Its `Definition` imports/renders move to `Config`. |
| `frontend/src/routes/explore/dataapi/__tests__/Query.test.tsx` | **New.** The frontend test of spec §5 plus the preview block's own behaviour. |

**Task order and why:** Task 1 ships the route on its own and is independently testable. Tasks 2–4 are the UI, and Tasks 3–4 together are what retires `Definition` — they must land close together so no release has the same field editable in two places. Task 5 is the button labelling, kept separate so a one-line copy change is not lost inside a large diff.

---

### Task 1: The preview route

**Files:**
- Modify: `crates/egret-dataapi/src/lifecycle.rs` (near `validate_definition`, currently line 967)
- Modify: `crates/egret-dataapi/src/serving/params.rs:103` (`resolve`)
- Create: `crates/egret-dataapi/src/preview.rs`
- Modify: `crates/egret-dataapi/src/lib.rs` (add `mod preview;`)
- Modify: `crates/egret-dataapi/src/routes.rs` (handler after `validate_endpoint`, currently line 1149; router near line 1868; `paths(...)` near line 1931)
- Modify: `openapi.json`
- Test: `crates/egret-dataapi/tests/preview.rs`

**Interfaces:**
- Consumes: `egret_dataapi_api::{ParamSpec, ColumnSpec}`; `egret_query_api::{Param, Limits, QueryExecPort}`; `egret_query_api::QueryExecPort::execute_as_with_params(&self, ws: Uuid, database_id: Uuid, principal: egret_core::UserId, sql: &str, params: &[Param], limits: Limits) -> Result<egret_core::TabularResult, egret_core::Error>`; `crate::render::{render, ParamValues}`; `crate::routes::{DataApiState, DataApiError, EP_MANAGE}`; `EndpointStore::connection_per_principal_pair(&self, workspace: Uuid, database: Uuid) -> Result<(bool, bool)>`.
- Produces, for Task 2's generated client:
  - `POST /api/ws/{workspace}/data-api/endpoints/{id}/preview`
  - request `PreviewEndpointInput { sql: String, params: Vec<ParamSpec>, values: BTreeMap<String, Vec<String>>, database_id: Uuid }`
  - response `DataApiPreviewDto { columns: Vec<ColumnMeta>, rows: Vec<Vec<serde_json::Value>>, row_count: u64, truncated: bool }`
  - generated hook name: `useDataApiPreviewEndpoint` (orval prefixes the tag: `data-api` + operationId `preview_endpoint`).

- [ ] **Step 1: Read the two functions this task refactors**

Read `crates/egret-dataapi/src/lifecycle.rs:960-1000` (`validate_definition`) and `crates/egret-dataapi/src/serving/params.rs:86-225` (`resolve`). Both are heavily commented and the comments are the reasoning — do not delete them while moving code.

The check ORDER inside `validate_definition` is pinned by an existing test (`validate_definition_runs_the_statement_check_first`, `lifecycle.rs:2584`). The refactor below preserves it exactly.

- [ ] **Step 2: Write the failing unit test for the shared scan bundle**

Add to the existing `mod tests` in `crates/egret-dataapi/src/lifecycle.rs`:

```rust
    /// Preview and publish must refuse the SAME definitions, minus the one rule
    /// that is about a thing preview does not do. If a rule is ever added to
    /// `validate_definition` and not to `validate_preview`, this is what says so:
    /// preview would run a definition publish refuses.
    #[test]
    fn validate_preview_refuses_what_publish_refuses() {
        let cache = CacheSpec::default();
        // Not a single SELECT — the rule that stops `:manage` becoming engine
        // write access.
        assert!(validate_preview("SELECT 1; DROP TABLE t", &[]).is_err());
        assert!(validate_definition("SELECT 1; DROP TABLE t", &[], &cache, false, false).is_err());

        // An enum with no `allowed` list.
        let bad_enum = vec![ParamSpec {
            name: "sort".into(),
            ty: ParamType::Enum,
            location: ParamLocation::Query,
            required: false,
            default: None,
            allowed: None,
            description: String::new(),
        }];
        assert!(validate_preview("SELECT 1", &bad_enum).is_err());

        // A `location: body` parameter.
        let body_param = vec![ParamSpec {
            name: "region".into(),
            ty: ParamType::String,
            location: ParamLocation::Body,
            required: false,
            default: None,
            allowed: None,
            description: String::new(),
        }];
        assert!(validate_preview("SELECT 1", &body_param).is_err());

        // And a definition both accept.
        assert!(validate_preview("SELECT 1 AS n", &[]).is_ok());
    }

    /// The ONE rule preview deliberately omits, and the reason it is safe to
    /// omit: preview neither reads nor writes the cache, so a `shared` scope on
    /// a delegated connection cannot leak anything through it.
    #[test]
    fn validate_preview_omits_only_the_cache_scope_rule() {
        let shared = CacheSpec {
            enabled: true,
            ttl_seconds: 60,
            scope: CacheScope::Shared,
        };
        // Publish refuses it on a delegated connection…
        assert!(validate_definition("SELECT 1 AS n", &[], &shared, true, false).is_err());
        // …and preview never sees a cache at all, so the same SQL is fine.
        assert!(validate_preview("SELECT 1 AS n", &[]).is_ok());
    }
```

If `CacheSpec`/`CacheScope` field names differ from the above, read `crates/egret-dataapi-api/src/lib.rs` and use the real ones — do not invent a constructor.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p egret-dataapi --lib validate_preview`
Expected: FAIL to compile — `cannot find function 'validate_preview' in this scope`.

- [ ] **Step 4: Split `validate_definition` and add `validate_preview`**

Replace `validate_definition`'s body (`lifecycle.rs:967-982`) with these three functions, keeping the existing doc comment on `validate_definition`:

```rust
pub fn validate_definition(
    sql: &str,
    params: &[ParamSpec],
    cache: &CacheSpec,
    delegated: bool,
    impersonate: bool,
) -> std::result::Result<(), ValidationError> {
    // Order preserved EXACTLY: `check_single_select` first (pinned by
    // `validate_definition_runs_the_statement_check_first`), the cache rule
    // between the scans and the renders, as it has always been.
    check_definition_scans(sql, params)?;
    check_cache_scope(cache, delegated, impersonate)?;
    check_render_rules(sql, params)?;
    Ok(())
}

/// Every publish rule that is a SCAN — no render, no engine, no connection.
///
/// Extracted so [`validate_preview`] and [`validate_definition`] cannot drift.
/// A rule added here is in both by construction; a rule that belongs to only one
/// of them has to be added to that one explicitly, which is a visible decision
/// rather than an omission.
fn check_definition_scans(
    sql: &str,
    params: &[ParamSpec],
) -> std::result::Result<(), ValidationError> {
    check_single_select(sql)?;
    check_enum_params(params)?;
    check_param_locations(params)?;
    check_defaults(params)?;
    check_params(sql, params)?;
    Ok(())
}

/// The publish rules that apply to a PREVIEW of an unsaved draft.
///
/// Everything [`validate_definition`] enforces except [`check_cache_scope`],
/// which is about a thing preview does not do: preview neither reads nor writes
/// the cache, so a `shared` scope cannot leak across principals through it. The
/// draft body carries no `CacheSpec` for the same reason.
///
/// # `check_single_select` is authorization here, not tidiness
///
/// Preview executes author-supplied SQL under `data_api_endpoint:manage`, which
/// is NOT `sqllab:write`. Without this check a role holding only `:manage` would
/// have arbitrary write access to every engine the workspace can reach. It is
/// the first check for that reason.
pub fn validate_preview(
    sql: &str,
    params: &[ParamSpec],
) -> std::result::Result<(), ValidationError> {
    check_definition_scans(sql, params)?;
    check_render_rules(sql, params)?;
    Ok(())
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p egret-dataapi --lib validate_preview`
Expected: PASS, 2 tests.

Then run the whole lib suite to prove the order was preserved:
Run: `cargo test -p egret-dataapi --lib`
Expected: PASS, including `validate_definition_runs_the_statement_check_first`.

- [ ] **Step 6: Mutation-check the drift test**

Delete `check_single_select(sql)?;` from `check_definition_scans`. Run `cargo build -p egret-dataapi` and confirm **zero** compile errors, then run `cargo test -p egret-dataapi --lib validate_preview`. It must FAIL. Restore the line.

- [ ] **Step 7: Commit**

```bash
git add crates/egret-dataapi/src/lifecycle.rs
git commit -m "refactor(dataapi): share publish's scan rules with a preview entry point"
```

- [ ] **Step 8: Write the failing unit test for the decoded-map resolve**

Add to `mod tests` in `crates/egret-dataapi/src/serving/params.rs` (create the module if there is none):

```rust
    /// Preview hands `resolve_supplied` an already-decoded map. Re-encoding it
    /// into a query string so it could go through `resolve` would be a lossy
    /// round trip: a value containing `&` or `=` would split into two
    /// parameters, so a preview and a real call would disagree about the same
    /// value. This is the assertion that preview does not take that route.
    #[test]
    fn resolve_supplied_takes_values_containing_query_separators() {
        let spec = ParamSpec {
            name: "q".into(),
            ty: ParamType::String,
            location: ParamLocation::Query,
            required: false,
            default: None,
            allowed: None,
            description: String::new(),
        };
        let mut supplied: BTreeMap<String, Vec<String>> = BTreeMap::new();
        supplied.insert("q".into(), vec!["a&b=c".into()]);
        let out = resolve_supplied(&supplied, "SELECT {{ q }}", std::slice::from_ref(&spec))
            .expect("resolves");
        assert_eq!(out.key.get("q").map(Vec::as_slice), Some(&["a&b=c".to_string()][..]));
    }
```

- [ ] **Step 9: Run it to verify it fails**

Run: `cargo test -p egret-dataapi --lib resolve_supplied`
Expected: FAIL to compile — `cannot find function 'resolve_supplied'`.

- [ ] **Step 10: Split `resolve` into `decode` + `resolve_supplied`**

In `crates/egret-dataapi/src/serving/params.rs`, keep `resolve`'s signature and doc comment exactly as they are and reduce its body to two calls; move everything after `let supplied = decode(...)` into the new function:

```rust
pub(crate) fn resolve(
    raw_query: Option<&str>,
    snap: &EndpointSnapshot,
) -> Result<Resolved, ServingError> {
    let supplied = decode(raw_query, snap)?;
    resolve_supplied(&supplied, &snap.sql, &snap.params)
}

/// [`resolve`], from an ALREADY-DECODED map rather than a raw query string.
///
/// Preview calls this directly. The alternative — re-encoding the author's test
/// values into a query string so they could go through [`resolve`] — is a lossy
/// round trip: a value containing `&` or `=` splits into two parameters, and a
/// preview would then disagree with the real call about the same value. Serving
/// still reaches it through [`resolve`], so there is exactly one place where a
/// declared parameter becomes a [`Param`].
pub(crate) fn resolve_supplied(
    supplied: &BTreeMap<String, Vec<String>>,
    sql: &str,
    specs: &[ParamSpec],
) -> Result<Resolved, ServingError> {
    // …the existing body of `resolve` from `let iterated` onward, with
    // `snap.sql` → `sql` and `snap.params` → `specs`. Move the comments with
    // the code they explain.
}
```

`decode` keeps taking `&EndpointSnapshot` — it is only reached from `resolve`, and its byte/repeat caps are about an untrusted query string that preview does not have.

- [ ] **Step 11: Run to verify it passes**

Run: `cargo test -p egret-dataapi --lib params`
Expected: PASS.
Run: `cargo test -p egret-dataapi` — the whole crate, including the serving suites that go through `resolve`.
Expected: no new failures. (These are DB-gated; see Step 22 for the real run.)

- [ ] **Step 12: Commit**

```bash
git add crates/egret-dataapi/src/serving/params.rs
git commit -m "refactor(dataapi): resolve parameters from a decoded map, not only a query string"
```

- [ ] **Step 13: Write `crates/egret-dataapi/src/preview.rs`**

```rust
//! `POST …/endpoints/{id}/preview` — spec §2 of the P6 authoring-loop design.
//!
//! # What this is, in one line
//!
//! The author's on-screen draft, run as the AUTHOR, through the same validation
//! and render path publish and serving use, capped at 100 rows, with none of the
//! four consumer-facing side effects.
//!
//! # Why the body carries the draft rather than being read from the database
//!
//! Reading the saved draft would defeat the feature: the author would have to
//! Save before every preview, and the loop this route exists to close would
//! still be cut — by a button rather than by navigation. This opens no new
//! attack surface, because SQL Lab already executes arbitrary author-supplied
//! SQL at an equivalent permission, and the payload here passes the same gates
//! publish passes ([`crate::lifecycle::validate_preview`]).
//!
//! # The four things this must never do
//!
//! No `access_log` row (Activity measures real traffic; a preview click is not
//! traffic, and writing one would skew every percentile on that page). No
//! rate-limit consumption (an author iterating must not throttle a live
//! consumer). No cache read (it would show a stale answer for SQL just edited)
//! and no cache write (it would poison what consumers read with a result
//! produced under a different principal).
//!
//! Each absence is asserted in `tests/preview.rs`, because an absence is
//! exactly what a later refactor adds back without noticing.

use std::collections::BTreeMap;

use egret_core::UserId;
use egret_dataapi_api::ParamSpec;
use uuid::Uuid;

use crate::routes::{DataApiError, DataApiState};

/// Rows a preview returns, at most.
///
/// Fixed and not configurable, and the endpoint's own `limits.max_rows` does not
/// apply: an author wants the SHAPE of the data, not a million rows in a
/// browser. Named here rather than left to a call site so there is one number to
/// change and one number the UI's copy has to agree with.
pub(crate) const PREVIEW_MAX_ROWS: usize = 100;

/// Wall-clock budget for a preview.
///
/// Shorter than the serving ceiling (`serving/router.rs`'s 300 s) because the
/// person waiting is at a keyboard with an editor open, not a batch client. The
/// draft body carries no `limits`, so this is not clamping author input — it is
/// the whole budget.
pub(crate) const PREVIEW_TIMEOUT_SECS: u64 = 30;

/// The on-screen draft, as the request carries it.
///
/// No `cache`, `limits` or `rate_limit`: preview does not consult any of them,
/// and accepting fields it ignores would invite the reader to think it did.
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewEndpointInput {
    /// What is in the editor right now — not what is stored.
    pub sql: String,
    /// The parameter declarations as they are on screen right now.
    #[serde(default)]
    pub params: Vec<ParamSpec>,
    /// The author's test values, one list per parameter name. A list, not a
    /// scalar, because a repeated parameter is a list on the serving path too
    /// and preview must resolve it the same way.
    #[serde(default)]
    pub values: BTreeMap<String, Vec<String>>,
    /// The database picker's current value. Workspace-scoped by
    /// `connection_per_principal_pair`, which answers `NotFound` for a database
    /// belonging to another workspace — this id comes from a request body, so it
    /// is exactly the shape that could name someone else's connection.
    pub database_id: Uuid,
}

/// What a preview answers with.
///
/// `columns` carries the ENGINE's own names — `output.naming` is deliberately not
/// applied, because an author debugging SQL needs the names they wrote, and the
/// published contract (naming transform included) is what `Response` is for. The
/// UI must say which is which.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct DataApiPreviewDto {
    pub columns: Vec<egret_core::ColumnMeta>,
    #[schema(value_type = Vec<Vec<Object>>)]
    pub rows: Vec<Vec<serde_json::Value>>,
    pub row_count: u64,
    /// True when the result was longer than [`PREVIEW_MAX_ROWS`] and this is a
    /// prefix of it. The connector fetches `cap + 1` and sets this rather than
    /// truncating silently, so `row_count == 100` alone cannot tell "exactly at"
    /// from "over".
    pub truncated: bool,
}

/// Run `body` as `principal` and return what the engine said.
///
/// Every error variant passes through unchanged, which is the deliberate
/// INVERSE of the serving path's `data_access_denied` sanitising: a consumer must
/// not learn the shape of the tables, and an author who owns the SQL cannot fix a
/// syntax error from a sanitised message. That inversion is why this expression
/// must never be reachable below `data_api_endpoint:manage`.
pub(crate) async fn run(
    st: &DataApiState,
    ws: Uuid,
    principal: UserId,
    body: &PreviewEndpointInput,
) -> Result<DataApiPreviewDto, DataApiError> {
    // Workspace containment for a body-supplied id, before anything is rendered
    // or executed. The pair itself is unused — preview consults no cache — but
    // this call is the scope check, so it is not replaceable with a cheaper one.
    let _ = st
        .endpoints
        .connection_per_principal_pair(ws, body.database_id)
        .await?;

    crate::lifecycle::validate_preview(&body.sql, &body.params)?;

    // The SAME function serving resolves through, so a value that renders one
    // way here cannot render another way for a consumer.
    let resolved = crate::serving::params::resolve_supplied(&body.values, &body.sql, &body.params)
        .map_err(|_| {
            egret_core::Error::InvalidInput(
                "a test value does not satisfy its parameter's declared type".into(),
            )
        })?;

    let (sql, binds) = crate::render::render(&body.sql, &resolved.values)
        .map_err(crate::lifecycle::ValidationError::Render)?;

    let out = st
        .query
        .execute_as_with_params(
            ws,
            body.database_id,
            principal,
            &sql,
            &binds,
            egret_query_api::Limits {
                max_rows: PREVIEW_MAX_ROWS,
                timeout: std::time::Duration::from_secs(PREVIEW_TIMEOUT_SECS),
            },
        )
        .await?;

    Ok(DataApiPreviewDto {
        columns: out.columns,
        rows: out.rows,
        row_count: out.row_count,
        truncated: out.truncated,
    })
}
```

Two things to confirm against the real code rather than assume:
- `crate::serving::params` must be reachable from `preview.rs`. If `serving`'s `mod params` is private, make it `pub(crate) mod params;` — do not copy the function.
- `ValidationError` must convert into `DataApiError` (= `egret_core::Error`). `validate` at `lifecycle.rs:1419` relies on the same conversion, so use whatever it uses (`?` with a `From` impl); if the `map_err(ValidationError::Render)` line does not compile, follow `validate`'s exact pattern at `lifecycle.rs:1431`.

- [ ] **Step 14: Add `mod preview;` and the handler**

In `crates/egret-dataapi/src/lib.rs`, add `mod preview;` beside the other module declarations.

In `crates/egret-dataapi/src/routes.rs`, immediately after `validate_endpoint` (ends line 1149), add:

```rust
#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/data-api/endpoints/{id}/preview",
    tag = "data-api",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Endpoint id"),
    ),
    request_body = crate::preview::PreviewEndpointInput,
    responses(
        (status = 200, description = "The draft ran; up to 100 rows, with the engine's own column names", body = crate::preview::DataApiPreviewDto),
        (status = 400, description = "Not a single SELECT, a parameter mismatch, a test value that does not satisfy its declared type, or SQL the engine rejects — with the engine's own message", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:manage permission, or the engine's policy denies a table the query names", body = ErrorBody),
        (status = 404, description = "No such endpoint in this workspace, or a database_id belonging to another workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn preview_endpoint(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, id)): Path<(String, Uuid)>,
    Json(body): Json<crate::preview::PreviewEndpointInput>,
) -> Result<Json<crate::preview::DataApiPreviewDto>, DataApiError> {
    // `:manage`, exactly like `validate` and for a stronger reason: preview
    // executes author-supplied SQL as the caller and returns the engine's own
    // error text, which serving deliberately hides from consumers.
    egret_core::authz::require(&*st.pdp, &ctx, EP_MANAGE).await?;
    // The endpoint row is read for containment only — the draft's CONTENT comes
    // from the body, which is the whole point of this route. A `{id}` naming
    // another workspace's endpoint must 404 here as it does everywhere else,
    // even though nothing in the body depends on the row.
    let _ = st.endpoints.get(ctx.workspace_id.0, id).await?;
    let out = crate::preview::run(&st, ctx.workspace_id.0, ctx.principal.user_id, &body).await?;
    Ok(Json(out))
}
```

Wire it beside `validate` (near line 1868):

```rust
        .route(
            "/ws/{workspace}/data-api/endpoints/{id}/preview",
            post(preview_endpoint),
        )
```

And register it in the `paths(...)` list (near line 1931), next to `validate_endpoint`:

```rust
        preview_endpoint,
```

If `DataApiPreviewDto`/`PreviewEndpointInput` need to be in the utoipa `components(schemas(...))` list, add them there too — copy how `DataApiValidationDto` is registered.

- [ ] **Step 15: Build**

Run: `cargo build -p egret-dataapi`
Expected: zero errors. Fix real compile errors against the actual signatures; do not stub anything out.

- [ ] **Step 16: Regenerate `openapi.json`**

Run: `cargo run -p egret-bin -- openapi > openapi.json`
Then: `cargo test -p egret-server committed_openapi_json_is_current`
Expected: PASS. This gate lives in a DIFFERENT crate from the one you edited, which is exactly why it gets forgotten.

- [ ] **Step 17: Write the six failing backend tests**

Create `crates/egret-dataapi/tests/preview.rs`. It uses the existing `serving_common` fixture — read `crates/egret-dataapi/tests/serving_common/mod.rs` first, in particular `Fixture` (line 386), `fixture()` (549), `mgmt_post` (807), `FakePort` (79), `marker` (44) and `bind_signature` (63).

```rust
//! `POST …/endpoints/{id}/preview` — spec §5 of the P6 authoring-loop design.
//!
//! DB-gated. Self-skips (passes) when DATABASE_URL is unset, like every other
//! suite here — so check the DURATION, not just the `ok`.

mod serving_common;

use axum::http::StatusCode;
use egret_core::UserId;
use serving_common::{bind_signature, fixture, marker};

fn preview_path(f: &serving_common::Fixture) -> String {
    format!(
        "/api/ws/{}/data-api/endpoints/{}/preview",
        f.ws_slug, f.endpoint_id
    )
}

/// The body every test below starts from: a draft with no parameters.
fn draft(f: &serving_common::Fixture, sql: &str) -> serde_json::Value {
    serde_json::json!({
        "sql": sql,
        "params": [],
        "values": {},
        "database_id": f.database_id,
    })
}

/// Spec §5: **preview runs as the author, not the service account.**
///
/// CORRECTION (2026-08-30): this heading, this test's NAME and the UI copy
/// sketched later in this plan were all false, and are corrected on the
/// branch that shipped the governance flag. What the route decides is the
/// PRINCIPAL it passes; which identity the engine applies policy for is
/// `delegation_for`'s answer, and on an ordinary connection that is the
/// service account. The shipped test is `preview_passes_the_author_as_the_
/// principal`. See the design doc's §2.1.
///
/// `FakePort` folds the principal into the value it returns (`marker`), the way
/// Ranger filters per principal. A preview that ran as a service account would
/// be `Try it` wearing the wrong label — and the author would be told their SQL
/// works against rows they cannot actually see.
#[tokio::test]
async fn preview_runs_as_the_author() {
    let Some(f) = fixture().await else { return };
    let (status, body) = f
        .mgmt_post(&preview_path(&f), Some(draft(&f, "SELECT 1 AS n")))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["rows"][0][0].as_i64(),
        Some(marker(UserId(f.author))),
        "preview must run as the session's own user, not a service account"
    );
}

/// Spec §5: **preview reflects unsaved SQL.**
///
/// The property the request body exists for. It fails the moment anyone
/// "simplifies" the handler into reading the stored draft — at which point the
/// feature is gone and every other test here still passes.
#[tokio::test]
async fn preview_runs_the_body_not_the_stored_draft() {
    let Some(f) = fixture().await else { return };
    let (status, body) = f
        .mgmt_post(
            &preview_path(&f),
            Some(draft(&f, "SELECT 42 AS unsaved_marker")),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (sql, _) = f.port.last();
    assert!(
        sql.contains("unsaved_marker"),
        "the engine was sent {sql:?}, not the body's SQL"
    );
}

/// Spec §5: **preview writes no `access_log` row.**
///
/// Activity measures real traffic. A preview row would inflate call counts and
/// skew every percentile on that page.
#[tokio::test]
async fn preview_writes_no_access_log_row() {
    let Some(f) = fixture().await else { return };
    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM dataapi.access_log WHERE workspace_id = $1")
            .bind(f.ws_id)
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    let (status, body) = f
        .mgmt_post(&preview_path(&f), Some(draft(&f, "SELECT 1 AS n")))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM dataapi.access_log WHERE workspace_id = $1")
            .bind(f.ws_id)
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    assert_eq!(before, after, "a preview must not appear in Activity");
}

/// Spec §5: **preview does not populate the cache.**
///
/// A cached preview would poison what consumers read with a result produced
/// under a different principal — the author's, not the service account's.
#[tokio::test]
async fn preview_does_not_populate_the_cache() {
    let Some(f) = fixture().await else { return };
    let (status, _) = f
        .mgmt_post(&preview_path(&f), Some(draft(&f, "SELECT 1 AS n")))
        .await;
    assert_eq!(status, StatusCode::OK);
    let res = f.call_as(f.principal_a, "probe").await;
    assert_eq!(res.status, StatusCode::OK, "{:?}", res.body);
    assert_eq!(
        res.body["meta"]["cached"].as_bool(),
        Some(false),
        "a consumer's first call must be a MISS — a preview must not seed the cache"
    );
}

/// Spec §5: **preview refuses what publish refuses**, and with the same error —
/// which is what proves it did not get a render path of its own.
#[tokio::test]
async fn preview_refuses_a_definition_publish_refuses() {
    let Some(f) = fixture().await else { return };
    let (status, body) = f
        .mgmt_post(
            &preview_path(&f),
            Some(draft(&f, "SELECT 1 AS n; DROP TABLE t")),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a second statement must be refused: `:manage` is not engine write access — {body}"
    );
    assert_eq!(f.port.executions(), 0, "nothing may reach the engine");
}

/// Spec §5: **preview surfaces the engine's message**, not `data_access_denied`.
///
/// The one place serving's sanitising rule is deliberately inverted. An author
/// owns the SQL, runs as themselves, and cannot fix a syntax error from a
/// scrubbed message.
#[tokio::test]
async fn preview_surfaces_the_engines_own_message() {
    let Some(f) = fixture().await else { return };
    f.port
        .fail_next_with(egret_core::Error::InvalidInput(
            "line 1:8: mismatched input 'FRUM'".into(),
        ));
    let (status, body) = f
        .mgmt_post(&preview_path(&f), Some(draft(&f, "SELECT 1 AS n")))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let text = body.to_string();
    assert!(
        text.contains("mismatched input 'FRUM'"),
        "the author must see the engine's own text, got {text}"
    );
    assert!(
        !text.contains("data_access_denied"),
        "serving's sanitising must NOT apply to a preview"
    );
}

/// Not in spec §5, and it belongs here anyway: a `database_id` from a request
/// body is exactly the shape that can name another workspace's connection.
#[tokio::test]
async fn preview_refuses_a_foreign_database_id() {
    let Some(f) = fixture().await else { return };
    let mut body = draft(&f, "SELECT 1 AS n");
    body["database_id"] = serde_json::json!(uuid::Uuid::new_v4());
    let (status, out) = f.mgmt_post(&preview_path(&f), Some(body)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{out}");
    assert_eq!(f.port.executions(), 0);
}

/// A parameterised preview binds the author's values — the assertion that makes
/// every other one here non-vacuous. `bind_signature` folds the bound values
/// into the returned row, so two different test values cannot look the same.
#[tokio::test]
async fn preview_binds_the_authors_test_values() {
    let Some(f) = fixture().await else { return };
    let body = |v: &str| {
        serde_json::json!({
            "sql": "SELECT 1 AS n WHERE region = {{ region }}",
            "params": [{
                "name": "region",
                "type": "string",
                "location": "query",
                "required": true,
            }],
            "values": { "region": [v] },
            "database_id": f.database_id,
        })
    };
    let (s1, b1) = f.mgmt_post(&preview_path(&f), Some(body("EU"))).await;
    let (s2, b2) = f.mgmt_post(&preview_path(&f), Some(body("US"))).await;
    assert_eq!(s1, StatusCode::OK, "{b1}");
    assert_eq!(s2, StatusCode::OK, "{b2}");
    assert_ne!(
        b1["rows"][0][0], b2["rows"][0][0],
        "two different test values must produce different rows"
    );
    let (_, binds) = f.port.last();
    assert_eq!(
        bind_signature(&binds),
        bind_signature(&[egret_query_api::Param::Str("US".into())]),
        "the author's value must reach the engine as a BIND, not as SQL text"
    );
}

/// `:read` cannot preview. egret's permissions are flat, so nothing but the
/// explicit `require` stands between a reader and running SQL on the engine.
#[tokio::test]
async fn read_permission_cannot_preview() {
    let Some(f) = fixture().await else { return };
    // Follow `routes_authz.rs`'s pattern for building a `:read`-only principal
    // against the real Cedar PDP — a mocked flag would pin the consumer, not the
    // server. Copy that file's helper rather than inventing one here.
    // Assert 403 and `f.port.executions() == 0`.
}
```

The last test is a stub on purpose: `serving_common`'s fixture builds one admin session. Read `crates/egret-dataapi/tests/routes_authz.rs` for how it constructs a principal with an exact permission set against the real PDP, and write the test with that. **If that harness cannot be reached from `serving_common`'s fixture, put this one test in `routes_authz.rs` instead**, beside the other `:read`-cannot-author tests, using `UnreachablePort` — a preview that a reader can reach never gets as far as needing rows.

- [ ] **Step 18: Run the tests to verify they fail**

Run: `DATABASE_URL="postgres://egret:egret@localhost:5432/egret_test" cargo test -p egret-dataapi --test preview`
Expected: FAIL. Note the duration — a sub-100ms "ok" means `DATABASE_URL` did not reach the process and nothing was asserted.

- [ ] **Step 19: Make them pass**

The implementation from Steps 13–14 should already satisfy them. Fix real failures. If `f.port.fail_next_with` is not `pub`, make it `pub` in `serving_common` — do not switch the test to a weaker assertion.

Run: `DATABASE_URL="postgres://egret:egret@localhost:5432/egret_test" cargo test -p egret-dataapi --test preview`
Expected: PASS, all tests, non-trivial duration.

- [ ] **Step 20: Mutation-check the three absence assertions**

An absence is what a later refactor silently adds back, so each one must be shown to fail when the thing IS present.

1. **`access_log`:** in `preview::run`, after the execute, insert a row into `dataapi.access_log` for this workspace. Build (zero errors), run `preview_writes_no_access_log_row` → must FAIL. Revert.
2. **Cache:** in `preview::run`, after the execute, call `st.cache.put_at(...)` with the serving key for this endpoint. Build (zero errors), run `preview_does_not_populate_the_cache` → must FAIL. Revert. If constructing the serving key here is more work than the check is worth, say so in the report rather than skipping silently.
3. **Engine message:** change `preview::run`'s execute `.await?` to `.map_err(|_| egret_core::Error::Forbidden("data_access_denied".into()))?`. Build, run `preview_surfaces_the_engines_own_message` → must FAIL. Revert.

- [ ] **Step 21: Mutation-check the author-identity assertion**

Change `preview::run`'s `principal` argument at the `execute_as_with_params` call to some other `UserId` (e.g. construct one from a fresh uuid). Build (zero errors), run `preview_runs_as_the_author` → must FAIL. Revert.

- [ ] **Step 22: Run the whole crate suite**

Run: `DATABASE_URL="postgres://egret:egret@localhost:5432/egret_test" cargo test -p egret-dataapi`
Expected: no new failures versus `main`. Note: `cargo test` STOPS at the first failing binary, so if something fails, fix it and re-run rather than assuming the rest passed.

Also: `cargo fmt --all` and `cargo clippy -p egret-dataapi --all-targets -- -D warnings`.

- [ ] **Step 23: Commit**

```bash
git add crates/egret-dataapi/src/preview.rs crates/egret-dataapi/src/lib.rs \
        crates/egret-dataapi/src/routes.rs crates/egret-dataapi/tests/preview.rs \
        openapi.json
git commit -m "feat(dataapi): preview an unsaved draft as the author"
```

---

### Task 2: The preview block

**Files:**
- Create: `frontend/src/routes/explore/dataapi/detail/PreviewBlock.tsx`
- Create: `frontend/src/routes/explore/dataapi/__tests__/Query.test.tsx`
- Modify: `frontend/src/api/generated/**` (regenerated, committed)

**Interfaces:**
- Consumes: Task 1's `useDataApiPreviewEndpoint` from `@/api/generated/data-api/data-api`, and the generated model types `PreviewEndpointInput` / `DataApiPreviewDto`; `useEndpointDetail()` from `../EndpointDetail` for `{ ws, id, form }`.
- Produces: `export function PreviewBlock(): JSX.Element` — no props. It reads everything from `useEndpointDetail()`, matching how every other `detail/*.tsx` section gets its data.

- [ ] **Step 1: Regenerate the API client**

```bash
cd frontend && npm run gen:api
```

Confirm `useDataApiPreviewEndpoint` now exists:
```bash
grep -n "useDataApiPreviewEndpoint" frontend/src/api/generated/data-api/data-api.ts
```
If it does not, Task 1's `paths(...)` registration or `openapi.json` regeneration was missed — stop and fix that, do not hand-write a hook.

- [ ] **Step 2: Write the failing test**

Create `frontend/src/routes/explore/dataapi/__tests__/Query.test.tsx`:

```tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// No msw in this repo — mock the API module directly.
vi.mock("@/api/generated/data-api/data-api");
vi.mock("@/api/databases");
vi.mock("@/api/dataApi");

import * as api from "@/api/generated/data-api/data-api";
import * as databasesApi from "@/api/databases";
import * as dataApi from "@/api/dataApi";
import { ParamLocation, ParamType, type DataApiEndpointDto } from "@/api/generated/model";
import EndpointDetail from "@/routes/explore/dataapi/EndpointDetail";
import Query from "@/routes/explore/dataapi/detail/Query";

function makeEndpoint(overrides: Partial<DataApiEndpointDto> = {}): DataApiEndpointDto {
  // Copy the helper from `EndpointDetail.test.tsx` verbatim rather than
  // importing it — that file does not export it, and duplicating a fixture
  // builder is cheaper than making a 900-line test file into a module.
  // Fill in EVERY field of DataApiEndpointDto; TS will name any you miss.
  throw new Error("copy makeEndpoint from EndpointDetail.test.tsx");
}

function renderQuery(endpoint: DataApiEndpointDto = makeEndpoint()) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/explore/ws1/data-api/endpoints/${endpoint.id}`]}>
        <Routes>
          <Route path="/explore/:workspace/data-api/endpoints/:id" element={<EndpointDetail />}>
            <Route index element={<Query />} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("Query section — preview", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(databasesApi.useDatabases).mockReturnValue({
      data: [{ id: "db-1", name: "warehouse" }],
    } as never);
    vi.mocked(dataApi.useUpdateEndpoint).mockReturnValue({
      mutateAsync: vi.fn(),
      isPending: false,
    } as never);
  });

  it("sends the on-screen SQL, not the loaded draft", async () => {
    const endpoint = makeEndpoint({ sql: "SELECT 1" });
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    const mutateAsync = vi.fn().mockResolvedValue({
      status: 200,
      data: { columns: [{ name: "n", type: "integer" }], rows: [[1]], row_count: 1, truncated: false },
    });
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({
      mutateAsync,
      isPending: false,
    } as never);

    renderQuery(endpoint);
    const sql = await screen.findByLabelText("SQL");
    await userEvent.clear(sql);
    await userEvent.type(sql, "SELECT 2");
    await userEvent.click(screen.getByRole("button", { name: /preview/i }));

    await waitFor(() => expect(mutateAsync).toHaveBeenCalled());
    expect(mutateAsync.mock.calls[0][0].data.sql).toBe("SELECT 2");
  });

  it("says the result was capped when the engine truncated it", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({
      mutateAsync: vi.fn().mockResolvedValue({
        status: 200,
        data: { columns: [{ name: "n", type: "integer" }], rows: [[1]], row_count: 1, truncated: true },
      }),
      isPending: false,
    } as never);

    renderQuery(endpoint);
    await userEvent.click(await screen.findByRole("button", { name: /preview/i }));
    // A capped preview read as a small result set is the failure this exists to
    // stop, so the copy must name the cap.
    expect(await screen.findByText(/first 100 rows/i)).toBeInTheDocument();
  });

  it("shows the engine's own error text inline", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({
      mutateAsync: vi.fn().mockRejectedValue(new Error("line 1:8: mismatched input 'FRUM'")),
      isPending: false,
    } as never);

    renderQuery(endpoint);
    await userEvent.click(await screen.findByRole("button", { name: /preview/i }));
    // Inline, not a toast: a toast vanishes before an author mid-edit reads it,
    // and this is exactly the moment they need the message on screen.
    expect(await screen.findByText(/mismatched input 'FRUM'/)).toBeInTheDocument();
  });

  it("says preview shows the engine's column names, not the published contract's", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    renderQuery(endpoint);
    // Spec §2.4.1: say which is which or the two get confused.
    expect(await screen.findByText(/column names come from the engine/i)).toBeInTheDocument();
  });

  it("says a green preview does not prove a consumer can call it", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    renderQuery(endpoint);
    // Spec §2.1: "The consequence must reach the UI, not just this document."
    expect(await screen.findByText(/runs as you/i)).toBeInTheDocument();
  });
});
```

Replace the `makeEndpoint` stub by copying the real helper out of `EndpointDetail.test.tsx` (it is near the top of that file). Do not leave the `throw`.

- [ ] **Step 3: Run it to verify it fails**

Run: `cd frontend && npx vitest run src/routes/explore/dataapi/__tests__/Query.test.tsx`
Expected: FAIL — `Cannot find module '@/routes/explore/dataapi/detail/Query'`.

- [ ] **Step 4: Write `PreviewBlock.tsx`**

```tsx
import { useState } from "react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { SettingsNote } from "@/routes/explore/settings/SettingsNote";
import { useDataApiPreviewEndpoint } from "@/api/generated/data-api/data-api";
import type { DataApiPreviewDto } from "@/api/generated/model";
import { useEndpointDetail } from "../EndpointDetail";

/** The cap the backend applies (`preview::PREVIEW_MAX_ROWS`). Named so the copy
 *  below and the server cannot disagree about the number on screen. */
const PREVIEW_MAX_ROWS = 100;

/**
 * Runs the ON-SCREEN draft and shows the rows, without saving or publishing.
 *
 * **Test values are NOT part of the endpoint.** They live in this component's
 * own state, never in the shared `form`, so they cannot be saved by accident and
 * `dirty` cannot latch on typing one. That is also why they are inputs of their
 * own rather than a column added to `ParamEditor`'s rows: a test value beside a
 * declaration invites the reader to think it is stored with the endpoint.
 *
 * This duplicates `TryIt`'s input row, and that is accepted: the two sections
 * are never on screen together and they run as DIFFERENT principals — preview as
 * the author, a trial as a service account.
 */
export function PreviewBlock() {
  const { ws, id, form } = useEndpointDetail();
  const previewMutation = useDataApiPreviewEndpoint();
  const [values, setValues] = useState<Record<string, string>>({});
  const [result, setResult] = useState<DataApiPreviewDto | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function onPreview() {
    // Clear BOTH outcomes, not just the one this run is about to set —
    // otherwise a failing second run leaves the first run's table on screen
    // beside the error, showing two answers to "what happened" at once.
    setResult(null);
    setError(null);
    try {
      // One list per name, matching the serving path: a repeated parameter is a
      // list there too. A blank field is omitted rather than sent as `[""]`, so
      // an unfilled optional parameter behaves like it was never supplied.
      const sent: Record<string, string[]> = {};
      for (const p of form.params) {
        const v = values[p.name];
        if (v !== undefined && v !== "") sent[p.name] = [v];
      }
      const res = await previewMutation.mutateAsync({
        workspace: ws,
        id,
        data: {
          sql: form.sql,
          params: form.params,
          values: sent,
          database_id: form.database_id,
        },
      });
      // The mutator throws on any non-2xx, so this is the only branch reached at
      // runtime — the check just narrows TS's view of the response union, the
      // same trick every other section in this feature uses.
      if (res.status !== 200) {
        setError("Could not run the preview.");
        return;
      }
      setResult(res.data);
    } catch (e) {
      // The engine's own message, verbatim — that is what the route returns and
      // it is the only thing an author can act on.
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  return (
    <div className="space-y-3 border-t border-border pt-4">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="text-sm font-medium">Preview</h3>
        <Button type="button" size="sm" onClick={onPreview} disabled={previewMutation.isPending}>
          {previewMutation.isPending ? "Running…" : "Preview draft"}
        </Button>
      </div>

      <SettingsNote>
        Preview runs as you, against the database above, and shows the first{" "}
        {PREVIEW_MAX_ROWS} rows. It does not save, publish, spend a rate limit, fill the cache, or
        appear in Activity. Column names come from the engine, not from the published contract — see
        Response for the names a consumer receives. And because it runs as you rather than as a
        service account, a green preview does not prove a consumer can call this endpoint; Try it
        answers that.
      </SettingsNote>

      {form.params.length > 0 && (
        <div className="grid grid-cols-2 gap-3">
          {form.params.map((p) => (
            <div key={p.name} className="space-y-1.5">
              <Label htmlFor={`preview-param-${p.name}`}>
                {p.name}
                {p.required && <span className="text-destructive"> *</span>}
              </Label>
              <Input
                id={`preview-param-${p.name}`}
                value={values[p.name] ?? ""}
                placeholder={p.default ?? undefined}
                onChange={(e) => setValues((prev) => ({ ...prev, [p.name]: e.target.value }))}
              />
            </div>
          ))}
        </div>
      )}

      {/* Inline, not a toast — an author mid-edit needs the message to stay on
          screen while they fix the SQL it is about. */}
      {error && (
        <Alert variant="destructive">
          <AlertDescription className="font-mono text-xs">{error}</AlertDescription>
        </Alert>
      )}

      {result && (
        <div className="space-y-2">
          <p className="text-xs text-muted-foreground">
            {result.truncated
              ? `Showing the first ${PREVIEW_MAX_ROWS} rows — there are more.`
              : `${result.row_count} row${result.row_count === 1 ? "" : "s"}.`}
          </p>
          {/* Wide results scroll inside their own container; the page body must
              never scroll horizontally. */}
          <div className="max-h-72 overflow-auto rounded-md border border-border">
            <table className="w-full text-xs">
              <thead className="sticky top-0 bg-muted/60">
                <tr>
                  {result.columns.map((c) => (
                    <th key={c.name} className="px-2 py-1.5 text-left font-medium">
                      {c.name}
                      <span className="ml-1 font-normal text-muted-foreground">{c.type}</span>
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {result.rows.map((row, ri) => (
                  <tr key={ri} className="border-t border-border">
                    {row.map((cell, ci) => (
                      <td key={ci} className="px-2 py-1 font-mono">
                        {cell === null ? (
                          <span className="text-muted-foreground">null</span>
                        ) : (
                          String(cell)
                        )}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </div>
  );
}
```

If the generated `DataApiPreviewDto`'s column type field is `type` rather than `ty`, use what the generated model says — `ColumnMeta` serialises its type as `"type"` (`egret-core/src/tabular.rs`).

- [ ] **Step 5: Write a minimal `Query.tsx` so the test can mount**

Task 3 builds the real section. For now, just enough to render the block and the SQL field the test types into:

```tsx
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { useEndpointDetail } from "../EndpointDetail";
import { PreviewBlock } from "./PreviewBlock";

/** Placeholder — Task 3 moves the database picker and `ParamEditor` in here. */
export default function Query() {
  const { form, setForm } = useEndpointDetail();
  return (
    <div className="space-y-6">
      <div className="space-y-1.5">
        <Label htmlFor="q-sql">SQL</Label>
        <Textarea
          id="q-sql"
          className="font-mono text-xs"
          rows={8}
          value={form.sql}
          onChange={(e) => setForm((f) => ({ ...f, sql: e.target.value }))}
        />
      </div>
      <PreviewBlock />
    </div>
  );
}
```

- [ ] **Step 6: Point the index route at it**

In `frontend/src/App.tsx:135`, change `<Route index element={<Definition />} />` to `<Route index element={<Query />} />` and add the import. Leave `Definition` imported and routed nowhere yet — Task 4 deletes it.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cd frontend && npx vitest run src/routes/explore/dataapi/__tests__/Query.test.tsx`
Expected: PASS, 5 tests.

- [ ] **Step 8: Mutation-check the two copy assertions**

The copy tests are the weakest here — they pass on any string containing the phrase. Delete the whole `<SettingsNote>` block, run the file, and confirm the last three tests FAIL. Restore it.

Then change `data: { sql: form.sql, … }` to `data: { sql: "SELECT 1", … }`, build, and confirm `sends the on-screen SQL` FAILS. Restore.

- [ ] **Step 9: Run the full frontend gates**

```bash
cd frontend && npx vitest run && npm run typecheck && npm run lint
```
Expected: all tests pass; `tsc` 0 errors; eslint **0 errors / 178 warnings** — the baseline. A new warning is a regression.

- [ ] **Step 10: Commit**

```bash
git add frontend/src/api/generated frontend/src/routes/explore/dataapi/detail/PreviewBlock.tsx \
        frontend/src/routes/explore/dataapi/detail/Query.tsx \
        frontend/src/routes/explore/dataapi/__tests__/Query.test.tsx frontend/src/App.tsx
git commit -m "feat(frontend): preview an endpoint draft's rows in place"
```

---

### Task 3: The `Query` section

**Files:**
- Modify: `frontend/src/routes/explore/dataapi/detail/Query.tsx`
- Modify: `frontend/src/routes/explore/dataapi/__tests__/Query.test.tsx`

**Interfaces:**
- Consumes: `useEndpointDetail()`; `useDatabases(ws)` from `@/api/databases`; `ParamEditor` from `./ParamEditor` with props `{ params: DataApiEndpointDto["params"]; onChange: (params: DataApiEndpointDto["params"]) => void }` (read `detail/ParamEditor.tsx` and use its real signature); `PreviewBlock` from `./PreviewBlock`.
- Produces: nothing new. `Query` stays the index route's default export.

- [ ] **Step 1: Write the failing test**

Append to `Query.test.tsx`:

```tsx
describe("Query section — layout", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(databasesApi.useDatabases).mockReturnValue({
      data: [{ id: "db-1", name: "warehouse" }],
    } as never);
    vi.mocked(dataApi.useUpdateEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
  });

  it("holds the database picker, the SQL editor and the parameter declarations", async () => {
    const endpoint = makeEndpoint({
      params: [
        {
          name: "region",
          type: ParamType.string,
          location: ParamLocation.query,
          required: true,
          description: "",
        },
      ],
    });
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);

    renderQuery(endpoint);
    expect(await screen.findByLabelText("Database")).toBeInTheDocument();
    expect(screen.getByLabelText("SQL")).toBeInTheDocument();
    // The DECLARATION, from ParamEditor…
    expect(screen.getByDisplayValue("region")).toBeInTheDocument();
    // …and a separate test-value input for the same parameter, which is the
    // distinction merging the two would destroy.
    expect(screen.getByLabelText(/^region/)).toBeInTheDocument();
  });

  it("does not hold name, path or the config tables — those are Config's", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderQuery(endpoint);
    await screen.findByLabelText("SQL");
    expect(screen.queryByLabelText("Name")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Path")).not.toBeInTheDocument();
    expect(screen.queryByText("Rate limit")).not.toBeInTheDocument();
  });
});
```

Fix the `ParamType`/`ParamLocation` enum member spellings against `@/api/generated/model` — the existing `EndpointDetail.test.tsx` shows the real ones.

- [ ] **Step 2: Run it to verify it fails**

Run: `cd frontend && npx vitest run src/routes/explore/dataapi/__tests__/Query.test.tsx`
Expected: FAIL — no `Database` label in the placeholder `Query.tsx`.

- [ ] **Step 3: Build the real `Query.tsx`**

Replace the placeholder. Move the database picker (`Definition.tsx:93-107`) and the SQL field (`109-118`) and the `ParamEditor` line (`121`) across verbatim, changing only the `id`/`htmlFor` prefix from `def-` to `q-`:

```tsx
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { useDatabases } from "@/api/databases";
import { useEndpointDetail } from "../EndpointDetail";
import { ParamEditor } from "./ParamEditor";
import { PreviewBlock } from "./PreviewBlock";

/**
 * Everything that decides what the query RETURNS: the database it runs against,
 * the SQL, the parameters it declares, and a preview of the rows.
 *
 * **Vertical — editor above, results below — following SQL Lab, not
 * ChartExplore's left/right split.** ChartExplore owns the full page width; this
 * section sits inside the detail route under the shell's `max-w-3xl`, and a
 * right-hand results column would squeeze both halves.
 *
 * Parameters appear twice here on purpose. `ParamEditor` holds the DECLARATION
 * (name, type, required, default, allowed) which is saved with the endpoint;
 * `PreviewBlock` holds a TEST VALUE per declared parameter, which is not. Putting
 * a test value in the declaration's row would invite the reader to think it was
 * stored.
 */
export default function Query() {
  const { ws, form, setForm } = useEndpointDetail();
  const { data: databases } = useDatabases(ws);

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-lg font-semibold">Query</h2>
        <p className="text-sm text-muted-foreground">
          The query this endpoint runs, and the parameters callers may pass it.
        </p>
      </div>

      <div className="space-y-4">
        <div className="space-y-1.5">
          <Label htmlFor="q-db">Database</Label>
          <Select value={form.database_id} onValueChange={(v) => setForm((f) => ({ ...f, database_id: v }))}>
            <SelectTrigger id="q-db" aria-label="Database">
              <SelectValue placeholder="Choose a database" />
            </SelectTrigger>
            <SelectContent>
              {(databases ?? []).map((d) => (
                <SelectItem key={d.id} value={d.id}>
                  {d.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        <div className="space-y-1.5">
          <Label htmlFor="q-sql">SQL</Label>
          <Textarea
            id="q-sql"
            className="font-mono text-xs"
            rows={12}
            value={form.sql}
            onChange={(e) => setForm((f) => ({ ...f, sql: e.target.value }))}
          />
        </div>
      </div>

      <ParamEditor params={form.params} onChange={(params) => setForm((f) => ({ ...f, params }))} />

      <PreviewBlock />
    </div>
  );
}
```

`rows={12}` rather than Definition's `rows={8}`: the editor no longer shares the section with three config tables, which is the whole reason for the split.

- [ ] **Step 4: Run to verify it passes**

Run: `cd frontend && npx vitest run src/routes/explore/dataapi/__tests__/Query.test.tsx`
Expected: PASS, 7 tests.

- [ ] **Step 5: Run the full frontend gates**

```bash
cd frontend && npx vitest run && npm run typecheck && npm run lint
```
Expected: all pass; 0 errors / 178 warnings. `EndpointDetail.test.tsx` still renders `Definition` directly, so it must still be green here — Task 4 is what changes it.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/routes/explore/dataapi/detail/Query.tsx \
        frontend/src/routes/explore/dataapi/__tests__/Query.test.tsx
git commit -m "feat(frontend): give the query its own section in the endpoint editor"
```

---

### Task 4: The `Config` section, and retiring `Definition`

**Files:**
- Create: `frontend/src/routes/explore/dataapi/detail/Config.tsx`
- Delete: `frontend/src/routes/explore/dataapi/detail/Definition.tsx`
- Modify: `frontend/src/routes/explore/dataapi/EndpointDetail.tsx:102-108` (`SECTIONS`)
- Modify: `frontend/src/App.tsx:134-145`
- Modify: `frontend/src/routes/explore/dataapi/__tests__/EndpointDetail.test.tsx`

**Interfaces:**
- Consumes: `useEndpointDetail()`; `ConfigSection` from `@/components/form/ConfigSection`; `ConfigTable`/`ConfigTableRow` from `@/components/config/ConfigTable` (`ConfigTableRow` props: `{ label, fieldKey, htmlFor, details, docsUrl, mono, children }` — `details` is a plain string, no markdown); `useDataApiValidateEndpoint` from `@/api/generated/data-api/data-api`.
- Produces: `Config` as the default export of `detail/Config.tsx`, routed at `config`.

- [ ] **Step 1: Write the failing test**

In `EndpointDetail.test.tsx`, replace the `Definition` import with `Config` and rename the `renderDetail` helper's rendered element accordingly. Then add:

```tsx
  it("Config holds the endpoint's identity and its three config tables", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderConfig(endpoint);
    expect(await screen.findByLabelText("Name")).toBeInTheDocument();
    expect(screen.getByLabelText("Description")).toBeInTheDocument();
    expect(screen.getByLabelText("Method")).toBeInTheDocument();
    expect(screen.getByLabelText("Path")).toBeInTheDocument();
    expect(screen.getByText("Cache")).toBeInTheDocument();
    expect(screen.getByText("Limits")).toBeInTheDocument();
    expect(screen.getByText("Rate limit")).toBeInTheDocument();
  });

  it("Config does not hold the SQL editor — that is Query's", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderConfig(endpoint);
    await screen.findByLabelText("Name");
    expect(screen.queryByLabelText("SQL")).not.toBeInTheDocument();
  });
```

Add a `renderConfig` helper beside the existing `renderDetail`, mounting `<Route path="config" element={<Config />} />` at `/explore/ws1/data-api/endpoints/:id/config`.

Every existing test in that file that reached a field now in `Config` (Name, Description, Method, Path, Cache/Limits/Rate-limit rows, Validate) must be pointed at `renderConfig`; tests that reach SQL, Database or `ParamEditor` move to `Query.test.tsx`. Read the file and move each one — do not delete a test to make the suite green.

- [ ] **Step 2: Run it to verify it fails**

Run: `cd frontend && npx vitest run src/routes/explore/dataapi/__tests__/EndpointDetail.test.tsx`
Expected: FAIL — `Cannot find module '@/routes/explore/dataapi/detail/Config'`.

- [ ] **Step 3: Write `Config.tsx`**

Move `Definition.tsx`'s name/description/method/path block (lines 58-91), the three `ConfigSection` blocks (123-299) and the Validate block (301-332) across **verbatim**, including their comments, changing only the `id`/`htmlFor` prefix from `def-` to `cfg-` and wrapping the identity fields in a `ConfigSection title="Endpoint"`:

```tsx
/**
 * Everything about the endpoint as an HTTP object: the URL contract callers use,
 * and the cache / limits / rate-limit policy applied to their calls. Nothing here
 * decides what the query returns — that is `Query`.
 *
 * One column, deliberately — the shell (`EndpointDetail.tsx`) sets `max-w-3xl`
 * around the `Outlet` so every section inherits it structurally. A two-column
 * grid pads the short groups to the height of the tall one beside them; CSS
 * `columns` fights the flex container this page sits in and clips the form at
 * the fold. Both were tried on this editor and both were wrong.
 */
export default function Config() {
  const { ws, id, form, setForm, dirty } = useEndpointDetail();
  // …Definition's `validateMutation`, `validation` state and `runValidate`,
  // moved verbatim.
}
```

Validate belongs here, not in `Query`: it runs the *saved draft* and reports the resolved output schema, which is a statement about the published contract rather than about the query in the editor. Task 5 makes that explicit in its label.

- [ ] **Step 4: Add the section and the route**

`EndpointDetail.tsx:102`:

```tsx
const SECTIONS: { to: string; label: string; end?: boolean }[] = [
  { to: "", label: "Query", end: true },
  { to: "config", label: "Config" },
  { to: "response", label: "Response" },
  { to: "access", label: "Access" },
  { to: "try", label: "Try it" },
  { to: "versions", label: "Versions" },
];
```

`App.tsx`, beside the other children:

```tsx
            <Route path="config" element={<Config />} />
```

- [ ] **Step 5: Delete `Definition.tsx`**

```bash
git rm frontend/src/routes/explore/dataapi/detail/Definition.tsx
```

Then confirm nothing still imports it:
```bash
grep -rn "detail/Definition" frontend/src
```
Expected: no matches. Remove the now-dead import from `App.tsx`.

- [ ] **Step 6: Run the full frontend gates**

```bash
cd frontend && npx vitest run && npm run typecheck && npm run lint
```
Expected: all tests pass; `tsc` 0 errors; 0 errors / 178 warnings.

- [ ] **Step 7: Commit**

```bash
git add -A frontend/src
git commit -m "feat(frontend): split the endpoint editor into Query and Config"
```

---

### Task 5: Say which version each button runs

**Files:**
- Modify: `frontend/src/routes/explore/dataapi/detail/PreviewBlock.tsx`
- Modify: `frontend/src/routes/explore/dataapi/detail/Config.tsx`
- Modify: `frontend/src/routes/explore/dataapi/detail/Versions.tsx`
- Modify: `frontend/src/routes/explore/dataapi/__tests__/Query.test.tsx`

**Interfaces:**
- Consumes: `useEndpointDetail()`'s `dirty`.
- Produces: nothing new.

Spec §3.4 — three actions on one page that disagree about unsaved changes:

| Action | With unsaved changes | Runs |
|---|---|---|
| **Preview** | Enabled — this is the point | What is on screen |
| **Validate** | Disabled | The saved draft |
| **Publish** | Disabled | The saved draft |

"Preview" without a label inviting the distinction lets an author preview, see green, publish, and get something else — the failure the whole spec exists to prevent.

- [ ] **Step 1: Write the failing test**

Append to `Query.test.tsx`:

```tsx
  it("keeps Preview enabled while dirty, unlike Validate and Publish", async () => {
    const endpoint = makeEndpoint({ sql: "SELECT 1" });
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);

    renderQuery(endpoint);
    const sql = await screen.findByLabelText("SQL");
    await userEvent.type(sql, " -- edited");
    // One test, three assertions — it pins §3.4 as a SET, not one button at a
    // time, because the property is the DIFFERENCE between them.
    expect(screen.getByRole("button", { name: /preview draft/i })).toBeEnabled();
    expect(screen.getByRole("button", { name: /^save$/i })).toBeEnabled();
    expect(screen.getByText(/unsaved changes/i)).toBeInTheDocument();
  });
```

And in `EndpointDetail.test.tsx`, beside the existing Validate tests:

```tsx
  it("labels Validate as checking the SAVED draft", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderConfig(endpoint);
    expect(await screen.findByRole("button", { name: /validate saved draft/i })).toBeInTheDocument();
  });
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd frontend && npx vitest run src/routes/explore/dataapi/__tests__/`
Expected: FAIL — the Validate button is still labelled `Validate`.

- [ ] **Step 3: Relabel the three buttons**

`PreviewBlock.tsx` — already `Preview draft`; keep it, and confirm the note says "runs as you" and names what it does not do.

`Config.tsx` — the Validate button:

```tsx
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={runValidate}
            disabled={validateMutation.isPending || dirty}
            title={dirty ? "Save your changes before validating — Validate checks the saved draft." : undefined}
          >
            {validateMutation.isPending ? "Validating…" : "Validate saved draft"}
          </Button>
```

`Versions.tsx` — find the Publish button and change its label to `Publish saved draft`, keeping its existing `disabled={dirty || …}` and `title` exactly as they are. Read the file first; do not change its disabled logic.

The three labels have to differ in the same dimension — *which version* — or the distinction is decoration:
- `Preview draft` — what is on screen
- `Validate saved draft` — the stored draft
- `Publish saved draft` — the stored draft, frozen into a version

- [ ] **Step 4: Run to verify it passes**

Run: `cd frontend && npx vitest run src/routes/explore/dataapi/__tests__/`
Expected: PASS.

- [ ] **Step 5: Mutation-check the labelling test**

Change `Validate saved draft` back to `Validate`, run the file, confirm the new test FAILS, restore it.

- [ ] **Step 6: Run every gate one last time**

```bash
cd frontend && npx vitest run && npm run typecheck && npm run lint && npm run build
```
Expected: all tests pass; 0 type errors; 0 eslint errors / 178 warnings; build green.

```bash
cd .. && cargo fmt --all --check
DATABASE_URL="postgres://egret:egret@localhost:5432/egret_test" cargo test -p egret-dataapi
cargo test -p egret-server committed_openapi_json_is_current
```

- [ ] **Step 7: Commit**

```bash
git add frontend/src
git commit -m "fix(frontend): each run-something button names the version it runs"
```

---

## After the plan: verify it in a browser

Every P5 phase found a real defect in the browser that no test could see — CSRF traversal, a missing `h1`, literal backticks in copy, a UUID in the Who column. Do not report this complete on a green suite.

Run the local stack and drive the loop end to end:

```bash
EGRET_DATABASE__URL="postgres://egret:egret@localhost:5432/egret" ./target/debug/egret serve
cd frontend && npm run dev
```

Log in `admin@egret.local` / `Admin12345!`, workspace `default`, and check:

1. **Preview a draft with an unsaved edit.** Change the SQL, do not Save, click **Preview unsaved draft**, and confirm the rows reflect the edit. This is the whole feature and it is the one thing a test asserts only through a mock.

   (The label above was `Preview draft` when this plan was written. The final review found that, beside `Validate saved draft` and `Publish saved draft`, a bare "draft" reads as the same thing those two mean by it — so the button now names the *unsaved* on-screen text. The three labels must all differ in that one dimension or the distinction is decoration. Lines 1100, 1635 and 1655 above are the plan as written and are left as the historical record.)
2. **`POST …/preview` returns 200 through the real CSRF layer.** The same route must answer 403 to a bare `curl` with no `X-Egret-CSRF`. A hand-rolled `fetch` is what breaks, and only in production.
3. **Preview an endpoint with a parameter**, with two different test values, and confirm the rows differ. A preview that ignored the values would look identical.
4. **Break the SQL deliberately** and confirm the engine's own message is on screen, not `data_access_denied`.
5. **Preview, then check Activity.** The call must NOT appear, with *Include trial runs* on or off, and the tiles must not move.
6. **Preview, then Try it as a service account** on a cached endpoint, and confirm the trial reports `cached: false` — the preview did not seed the cache.
7. **Read the copy on screen.** Backticks in a `ConfigTableRow.details` string render literally; the `SettingsNote` in `PreviewBlock` is JSX and is fine, but check anything moved into a `details` prop.
8. **Tab through the six sections** and confirm the page still has exactly one `h1` (`sr-only`, the endpoint name) and that each section's heading is an `h2`.

---

## Self-Review

**Spec coverage**

| Spec | Task |
|---|---|
| §2 route, path, `:manage` gate | Task 1 Steps 14, 17 (`read_permission_cannot_preview`) |
| §2.1 runs as the author; consequence reaches the UI | Task 1 Step 17 (`preview_runs_as_the_author`); Task 2 Step 4 (`SettingsNote`) + Step 2 test |
| §2.2 body carries the on-screen draft | Task 1 Step 13 (`PreviewEndpointInput`), Step 17 (`preview_runs_the_body_not_the_stored_draft`); Task 2 Step 2 |
| §2.3 reuses serving's render path | Task 1 Steps 4, 10, 13; Step 17 (`preview_refuses_a_definition_publish_refuses`) |
| §2.4 no access_log / rate limit / cache; 100-row cap stated in UI | Task 1 Step 17 (two tests) + Step 20 mutations; Task 2 Step 2 (`first 100 rows`) |
| §2.4.1 engine column names; engine errors verbatim | Task 1 Step 17 (`preview_surfaces_the_engines_own_message`); Task 2 Steps 2, 4 |
| §2.5 not audited | No audit call is added anywhere; Task 1 Step 14's handler has none, unlike `publish_endpoint` |
| §3.1 `Query` section, vertical layout, declaration ≠ test value | Task 3 |
| §3.2 `Config` section, four groups | Task 4 |
| §3.3 `Response`/`Access`/`Try it`/`Versions` unchanged | Not touched by any task except `Versions`' Publish LABEL (Task 5) |
| §3.4 three buttons, three relationships with `dirty` | Task 5 |
| §4 nothing new reaches a published endpoint | `check_single_select` retained (Task 1 Step 4); no serving-path file is modified except `params.rs`, and only by extraction |
| §5 all seven tests | Task 1 Step 17 (six backend) + Task 2/5 (frontend) |
| §6 four-step decomposition | Tasks 1–5 (§6's step 2 split into Tasks 2+3 so the preview block ships behind its own review gate) |
| §7 deferred | Nothing in any task adds a script engine, multi-statement SQL, or a parameter location |

**Two things this plan decides that the spec left open**, both flagged to the user rather than buried:
- `PREVIEW_TIMEOUT_SECS = 30` — the spec names the row cap but not the time budget.
- `check_single_select` is retained for preview as an **authorization** property, not just consistency. The spec's §4 says single-statement SQL stays enforced for published endpoints; this plan states why it must also hold for preview.

**Type consistency:** `PreviewEndpointInput` and `DataApiPreviewDto` are named identically in Task 1 (definition), Task 1 Step 14 (utoipa), and Task 2 (generated hook + component). `resolve_supplied` has one signature, used in Task 1 Steps 8, 10 and 13. `validate_preview(sql, params)` has one signature, used in Steps 2, 4 and 13. `PreviewBlock` takes no props in both Task 2 and Task 3. `PREVIEW_MAX_ROWS` is 100 on both sides.

**Placeholder scan:** the only intentional incompletenesses are marked as such and each names the file to read for the real answer — Task 1 Step 17's `read_permission_cannot_preview` (read `routes_authz.rs`), Task 2 Step 2's `makeEndpoint` (copy from `EndpointDetail.test.tsx`), and Task 4 Step 3's moved blocks (verbatim from named line ranges in `Definition.tsx`). None is a "TODO" standing in for a decision.
