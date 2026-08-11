# Report Renderer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

> Spec: `docs/en/specs/2026-08-12-report-renderer-design.md`
> Parent: `docs/en/specs/2026-08-11-alerts-and-reports-design.md` §5.3 · P4 of
> `docs/en/plans/2026-08-11-alerts-and-reports-plan.md`

**Goal:** deliver a saved chart or dashboard on a schedule as PNG, PDF, CSV or text.

**Architecture:** the backend mints a single-object, single-use, ~60s token and asks a new
`egret-renderer` service (Playwright + Chromium) to screenshot a chrome-free SPA print route. The
print route fetches its data ONCE, with that token, and sets an explicit readiness flag the
renderer waits on. CSV and text bypass the renderer entirely.

**Tech Stack:** Rust (axum, sqlx) · React/Vite · Playwright + Chromium (Node) · Helm · Postgres.

## Global Constraints

- **Readiness is a flag, never a timer.** `window.__EGRET_PRINT_READY__` is set only after every
  chart reports ECharts' `finished` AND `document.fonts.ready` resolves. A renderer timeout is a
  FAILED render; a partial capture must never be returned.
- **The render token is single-object, single-use, ~60s, bound to the schedule's owner.** The row
  is DELETED inside the transaction that reads it. Only its hash is stored.
- **The print page makes exactly ONE authenticated request**, carrying the token in an
  `Authorization: Bearer` header.
- **Authorization is the owner's, at render time** (§5.2). An owner who lost access → `error`.
- **A dashboard report must be `png` or `pdf`; `csv`/`text` are chart-only.**
- **Every failure path records a run** with its reason and recomputes `next_run_at`.
- Tests: DB-gated tests use `#[sqlx::test(migrations = "../../migrations")]` (a database per test),
  NOT the shared `egret_test`.
- CI gates that must stay green: `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo fmt --all -- --check`, the committed `openapi.json` (regenerate with
  `cargo run -p egret-bin -- openapi > openapi.json`, then `cd frontend && npm run gen:api`).

## File Structure

| File | Responsibility |
|---|---|
| `migrations/0112_render_tokens.sql` | `automation.render_tokens`; the dashboard/format CHECK |
| `crates/egret-automation/src/render/token.rs` | Mint, redeem-once, prune |
| `crates/egret-automation/src/render/payload.rs` | Assemble the one-shot payload as the owner |
| `crates/egret-automation/src/render/client.rs` | HTTP client for `egret-renderer` |
| `crates/egret-automation/src/render/mod.rs` | Re-exports; `RenderConfig` |
| `crates/egret-automation/src/routes.rs` | `GET /api/render/payload` (bearer token) |
| `crates/egret-automation/src/runner.rs` | Report branch of the tick |
| `frontend/src/routes/print/PrintObject.tsx` | Chrome-free print route + readiness flag |
| `renderer/` | New Node service: `server.js`, `Dockerfile`, `package.json` |
| `deploy/helm/egret/templates/renderer-*.yaml` | Deployment + ClusterIP Service |

---

## Task 1: Migration — render tokens and the format constraint

**Files:**
- Create: `migrations/0112_render_tokens.sql`
- Test: probe with `psql` (step 2), then Task 2's Rust tests

**Interfaces:**
- Produces: table `automation.render_tokens (id uuid pk, token_hash text unique, kind text,
  object_id uuid, owner_id uuid, expires_at timestamptz, created_at timestamptz)`; CHECK
  `schedule_report_format_matches_target` on `automation.schedules`.

- [ ] **Step 1: Write the migration**

```sql
-- Per-render credentials for the report renderer (P4). Deliberately NOT a
-- session: one object, one use, ~60s, bound to the schedule's owner.
CREATE TABLE automation.render_tokens (
  id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  -- Only the HASH is stored, like every other bearer secret in this repo: a
  -- database leak must not yield usable credentials.
  token_hash text NOT NULL UNIQUE,
  kind       text NOT NULL,
  object_id  uuid NOT NULL,
  owner_id   uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  expires_at timestamptz NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT render_token_kind_valid CHECK (kind IN ('chart', 'dashboard'))
);
CREATE INDEX render_tokens_expiry_idx ON automation.render_tokens (expires_at);

-- A dashboard has no single table, so a CSV or text rendering of one is either
-- an arbitrary choice of card or a zip of many. Saying so is better than either.
ALTER TABLE automation.schedules
  ADD CONSTRAINT schedule_report_format_matches_target CHECK (
    type <> 'report'
    OR dashboard_id IS NULL
    OR report_format IN ('png', 'pdf')
  );
```

- [ ] **Step 2: Probe the constraints against a clone before writing any Rust**

```bash
psql "$DATABASE_URL" -c "DROP DATABASE IF EXISTS egret_mig4;" -c "CREATE DATABASE egret_mig4 TEMPLATE egret;"
psql "postgres://egret:egret@localhost:5432/egret_mig4" -v ON_ERROR_STOP=1 -f migrations/0112_render_tokens.sql
```

Then, in that database, confirm each of these (the first must succeed, the rest must fail):

```sql
-- ok: a dashboard report as png
INSERT INTO automation.schedules (workspace_id,type,name,crontab,report_format,dashboard_id)
VALUES ((SELECT id FROM workspaces LIMIT 1),'report','r-png','0 9 * * *','png',
        (SELECT id FROM analytics.dashboards LIMIT 1));
-- ERROR expected: schedule_report_format_matches_target
INSERT INTO automation.schedules (workspace_id,type,name,crontab,report_format,dashboard_id)
VALUES ((SELECT id FROM workspaces LIMIT 1),'report','r-csv','0 9 * * *','csv',
        (SELECT id FROM analytics.dashboards LIMIT 1));
-- ERROR expected: render_token_kind_valid
INSERT INTO automation.render_tokens (token_hash,kind,object_id,owner_id,expires_at)
VALUES ('x','table',gen_random_uuid(),(SELECT id FROM users LIMIT 1), now() + interval '1 minute');
```

Expected: `INSERT 0 1`, then two `ERROR: new row ... violates check constraint`. Drop the database
afterwards. **A constraint written backwards passes inspection and fails here** — P1 shipped one
that rejected exactly the valid case, caught only by this probe.

- [ ] **Step 3: Force sqlx to re-embed the migrations**

Run: `touch crates/egret-metastore/src/lib.rs`
(Embedded migrations are baked at compile time; without this the new file is invisible to tests.)

- [ ] **Step 4: Commit**

```bash
git add migrations/0112_render_tokens.sql
git commit -m "feat(render): render_tokens table and the dashboard/format constraint"
```

---

## Task 2: Render tokens — mint, redeem once, prune

**Files:**
- Create: `crates/egret-automation/src/render/token.rs`, `crates/egret-automation/src/render/mod.rs`
- Modify: `crates/egret-automation/src/lib.rs` (add `pub mod render;`)
- Test: `crates/egret-automation/tests/render_token_db.rs`

**Interfaces:**
- Consumes: Task 1's table; `egret_metastore::Db`; `crate::error::AutomationError`.
- Produces:
  - `pub struct RenderTokenRepo { db: Db }` with `pub fn new(db: Db) -> Self`
  - `pub struct RenderGrant { pub kind: String, pub object_id: Uuid, pub owner_id: Uuid }`
  - `pub async fn mint(&self, kind: &str, object_id: Uuid, owner: Uuid, ttl: Duration) -> Result<String, AutomationError>` — returns the CLEARTEXT token, stores only its hash
  - `pub async fn redeem(&self, token: &str) -> Result<RenderGrant, AutomationError>` —
    `Err(AutomationError::NotFound)` when absent, expired, or already used
  - `pub async fn prune_expired(&self) -> Result<u64, AutomationError>`

- [ ] **Step 1: Write the failing tests**

Create `crates/egret-automation/tests/render_token_db.rs`:

```rust
//! Render-token lifecycle. The security properties live here: single use, short
//! life, and a cleartext token that is never stored.
use egret_automation::render::token::RenderTokenRepo;
use egret_automation::AutomationError;
use egret_metastore::Db;
use std::time::Duration;
use uuid::Uuid;

async fn seed_user(db: &Db) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (email, provider) VALUES ($1,'local') RETURNING id")
        .bind(format!("r_{}@t.local", Uuid::new_v4().simple()))
        .fetch_one(&db.pool).await.unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_minted_token_redeems_once_and_only_once(pool: sqlx::PgPool) {
    // Single-use is the property that keeps a leaked token harmless. A second
    // redemption must find nothing at all, not a row flagged as used.
    let db = Db { pool };
    let repo = RenderTokenRepo::new(db.clone());
    let owner = seed_user(&db).await;
    let object = Uuid::new_v4();

    let token = repo.mint("dashboard", object, owner, Duration::from_secs(60)).await.unwrap();
    let grant = repo.redeem(&token).await.expect("first redemption works");
    assert_eq!(grant.kind, "dashboard");
    assert_eq!(grant.object_id, object);
    assert_eq!(grant.owner_id, owner);

    let second = repo.redeem(&token).await;
    assert!(matches!(second, Err(AutomationError::NotFound(_))), "got {second:?}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_cleartext_token_is_never_stored(pool: sqlx::PgPool) {
    // Asserted by searching the stored row, not by trusting the column name.
    let db = Db { pool };
    let repo = RenderTokenRepo::new(db.clone());
    let owner = seed_user(&db).await;
    let token = repo.mint("chart", Uuid::new_v4(), owner, Duration::from_secs(60)).await.unwrap();

    let rows: Vec<String> = sqlx::query_scalar("SELECT t::text FROM automation.render_tokens t")
        .fetch_all(&db.pool).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].contains(&token), "cleartext token found in the row: {}", rows[0]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_expired_token_is_refused(pool: sqlx::PgPool) {
    let db = Db { pool };
    let repo = RenderTokenRepo::new(db.clone());
    let owner = seed_user(&db).await;
    let token = repo.mint("chart", Uuid::new_v4(), owner, Duration::from_secs(60)).await.unwrap();
    sqlx::query("UPDATE automation.render_tokens SET expires_at = now() - interval '1 second'")
        .execute(&db.pool).await.unwrap();

    assert!(matches!(repo.redeem(&token).await, Err(AutomationError::NotFound(_))));
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_unknown_token_is_refused(pool: sqlx::PgPool) {
    let db = Db { pool };
    let repo = RenderTokenRepo::new(db.clone());
    assert!(matches!(repo.redeem("not-a-real-token").await, Err(AutomationError::NotFound(_))));
}

#[sqlx::test(migrations = "../../migrations")]
async fn two_tokens_for_the_same_object_are_distinct(pool: sqlx::PgPool) {
    // Otherwise the UNIQUE index on the hash would make a second render fail.
    let db = Db { pool };
    let repo = RenderTokenRepo::new(db.clone());
    let owner = seed_user(&db).await;
    let object = Uuid::new_v4();
    let a = repo.mint("chart", object, owner, Duration::from_secs(60)).await.unwrap();
    let b = repo.mint("chart", object, owner, Duration::from_secs(60)).await.unwrap();
    assert_ne!(a, b);
}

#[sqlx::test(migrations = "../../migrations")]
async fn pruning_removes_only_expired_tokens(pool: sqlx::PgPool) {
    let db = Db { pool };
    let repo = RenderTokenRepo::new(db.clone());
    let owner = seed_user(&db).await;
    let live = repo.mint("chart", Uuid::new_v4(), owner, Duration::from_secs(600)).await.unwrap();
    let dead = repo.mint("chart", Uuid::new_v4(), owner, Duration::from_secs(600)).await.unwrap();
    sqlx::query(
        "UPDATE automation.render_tokens SET expires_at = now() - interval '1 hour' \
         WHERE token_hash <> encode(digest($1,'sha256'),'hex')")
        .bind(&live).execute(&db.pool).await.ok();

    assert!(repo.prune_expired().await.unwrap() >= 1);
    assert!(repo.redeem(&live).await.is_ok(), "the live token survived");
    assert!(repo.redeem(&dead).await.is_err());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-automation --test render_token_db`
Expected: FAIL to compile — `unresolved import egret_automation::render`.

- [ ] **Step 3: Implement the repo**

Create `crates/egret-automation/src/render/mod.rs`:

```rust
//! Report rendering (P4): per-render credentials, the one-shot payload, and the
//! client for the `egret-renderer` service.
pub mod token;
```

Create `crates/egret-automation/src/render/token.rs`:

```rust
//! Per-render credentials.
//!
//! A render token is NOT a session. It authorises reading exactly one object,
//! exactly once, for about a minute, as exactly one user. That envelope is what
//! separates a screenshot service from a data-exfiltration service: compromise
//! the renderer pod and what leaks is one object, briefly, that its owner could
//! already read.

use std::time::Duration;

use egret_metastore::Db;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::AutomationError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderGrant {
    pub kind: String,
    pub object_id: Uuid,
    pub owner_id: Uuid,
}

#[derive(Clone)]
pub struct RenderTokenRepo {
    db: Db,
}

fn hash(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex::encode(h.finalize())
}

impl RenderTokenRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Mint a token and return it in CLEARTEXT — the only moment it exists in
    /// readable form. Only its hash reaches the database.
    pub async fn mint(
        &self,
        kind: &str,
        object_id: Uuid,
        owner: Uuid,
        ttl: Duration,
    ) -> Result<String, AutomationError> {
        let token = format!("rt_{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let expires_at = chrono::Utc::now()
            + chrono::Duration::from_std(ttl).unwrap_or(chrono::Duration::seconds(60));
        sqlx::query(
            "INSERT INTO automation.render_tokens (token_hash, kind, object_id, owner_id, expires_at) \
             VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(hash(&token))
        .bind(kind)
        .bind(object_id)
        .bind(owner)
        .bind(expires_at)
        .execute(&self.db.pool)
        .await
        .map_err(AutomationError::from_db)?;
        Ok(token)
    }

    /// Redeem once. The row is DELETED by the same statement that reads it, so
    /// two concurrent redemptions cannot both succeed — a `used_at` flag would
    /// leave exactly that race open.
    pub async fn redeem(&self, token: &str) -> Result<RenderGrant, AutomationError> {
        let row: Option<(String, Uuid, Uuid)> = sqlx::query_as(
            "DELETE FROM automation.render_tokens \
             WHERE token_hash = $1 AND expires_at > now() \
             RETURNING kind, object_id, owner_id",
        )
        .bind(hash(token))
        .fetch_optional(&self.db.pool)
        .await
        .map_err(AutomationError::from_db)?;

        row.map(|(kind, object_id, owner_id)| RenderGrant { kind, object_id, owner_id })
            .ok_or_else(|| AutomationError::NotFound("render token".into()))
    }

    pub async fn prune_expired(&self) -> Result<u64, AutomationError> {
        let done = sqlx::query("DELETE FROM automation.render_tokens WHERE expires_at <= now()")
            .execute(&self.db.pool)
            .await
            .map_err(AutomationError::from_db)?;
        Ok(done.rows_affected())
    }
}
```

Add to `crates/egret-automation/src/lib.rs`, after `pub mod model;`:

```rust
pub mod render;
```

The `pruning_removes_only_expired_tokens` test uses `digest()`, which needs pgcrypto. If it is not
enabled, replace that test's `UPDATE` with one that expires by `object_id` instead — mint the dead
token with a known object id and match on it.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-automation --test render_token_db`
Expected: PASS, 6 tests.

- [ ] **Step 5: Mutation-check the single-use property**

Temporarily change `redeem`'s `DELETE ... RETURNING` to
`SELECT kind, object_id, owner_id FROM automation.render_tokens WHERE token_hash = $1 AND expires_at > now()`.
Re-run. Expected: `a_minted_token_redeems_once_and_only_once` FAILS. Restore.
**A green test that survives this mutation is not testing single use.**

- [ ] **Step 6: Commit**

```bash
git add crates/egret-automation/src/render crates/egret-automation/src/lib.rs \
        crates/egret-automation/tests/render_token_db.rs
git commit -m "feat(render): single-use, single-object render tokens"
```

---

## Task 3: The one-shot payload endpoint

**Files:**
- Create: `crates/egret-automation/src/render/payload.rs`
- Modify: `crates/egret-automation/src/render/mod.rs`, `crates/egret-automation/src/routes.rs`
- Test: `crates/egret-automation/tests/render_payload_routes.rs`

**Interfaces:**
- Consumes: `RenderTokenRepo::redeem`, `egret_bi::{ChartRepo, DashboardRepo}`,
  `egret_query::execution::run_query`, `egret_automation::evaluator::owner_identity`.
- Produces:
  - `pub struct RenderPayload { pub kind: String, pub id: String, pub title: String,
    pub cards: Vec<RenderCard> }`
  - `pub struct RenderCard { pub chart_id: String, pub title: String, pub viz_type: String,
    pub params: serde_json::Value, pub columns: Vec<String>,
    pub rows: Vec<Vec<serde_json::Value>> }`
  - route `GET /api/render/payload` (no `{workspace}` segment — it is authenticated by the token,
    not by a session)

- [ ] **Step 1: Write the failing route tests**

Create `crates/egret-automation/tests/render_payload_routes.rs`. Model the fixture on
`crates/egret-automation/tests/smtp_routes.rs` (same `Db`, `AutomationState`, `oneshot` helper),
adding a chart + dashboard via `egret_bi`'s repos. The four assertions:

```rust
#[sqlx::test(migrations = "../../migrations")]
async fn the_payload_is_returned_once_for_a_valid_token(pool: sqlx::PgPool) {
    // The print page's only request. It must arrive complete — spec AND data —
    // because the page makes no second call.
    let fx = setup(pool).await;
    let token = fx.mint_for_dashboard().await;
    let (s, body) = get_payload(&fx, &token).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["kind"], "dashboard");
    assert!(v["cards"].as_array().unwrap().len() >= 1, "cards carry their own data: {body}");
    assert!(v["cards"][0]["rows"].is_array());
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_replayed_token_is_refused(pool: sqlx::PgPool) {
    // Single use, end to end: the SECOND fetch of the same page fails.
    let fx = setup(pool).await;
    let token = fx.mint_for_dashboard().await;
    assert_eq!(get_payload(&fx, &token).await.0, StatusCode::OK);
    assert_eq!(get_payload(&fx, &token).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn no_token_is_refused(pool: sqlx::PgPool) {
    let fx = setup(pool).await;
    assert_eq!(get_payload_without_token(&fx).await.0, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_session_cookie_cannot_substitute_for_a_token(pool: sqlx::PgPool) {
    // The endpoint must NOT accept an ordinary logged-in user: it exists to be
    // reachable by a browser that has no session, and widening it to sessions
    // would make it a second, unaudited read path for any object.
    let fx = setup(pool).await;
    assert_eq!(get_payload_with_cookie(&fx, &fx.admin_tok).await.0, StatusCode::UNAUTHORIZED);
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `DATABASE_URL=... cargo test -p egret-automation --test render_payload_routes`
Expected: FAIL to compile (`render::payload` missing).

- [ ] **Step 3: Implement `payload.rs`**

```rust
//! Everything the print page needs, in ONE response.
//!
//! The page makes exactly one authenticated call, so single-use is literal
//! rather than aspirational. That forces this module to assemble server-side
//! what the interactive UI fetches piecemeal: the object's spec, and each
//! card's already-executed rows.
//!
//! Every query runs as the token's OWNER, through the same `run_query` path the
//! interactive endpoints use (§5.2) — a report must never render data its owner
//! could not open themselves.

use serde::Serialize;
use uuid::Uuid;

use crate::error::AutomationError;

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct RenderCard {
    pub chart_id: String,
    pub title: String,
    pub viz_type: String,
    pub params: serde_json::Value,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct RenderPayload {
    pub kind: String,
    pub id: String,
    pub title: String,
    pub cards: Vec<RenderCard>,
}
```

Then a `pub async fn build(...)` that, given a `RenderGrant` and the BI repos:
1. loads the chart (kind `chart`) or the dashboard and its cards (kind `dashboard`);
2. for each chart, compiles its saved spec to SQL exactly as `charts/{id}/data` does
   (`compile_chart_sql`), then runs it via `egret_query::execution::run_query` with the identity
   from `owner_identity(grant.owner_id, database_id, …)`;
3. returns `RenderPayload`.

Reuse rather than reimplement: if `compile_chart_sql` is private to `egret-bi`, make it `pub(crate)`
→ `pub` in the smallest possible change and note it in the commit. **Two code paths that compile a
chart differently is how a report stops matching the dashboard it claims to show.**

- [ ] **Step 4: Wire the route**

In `crates/egret-automation/src/routes.rs`, add to `automation_router()`:

```rust
.route("/render/payload", get(render_payload))
```

and the handler, which reads the bearer token itself (there is no `WsContext` here — the caller has
no session, by design):

```rust
#[utoipa::path(
    get, path = "/api/render/payload", tag = "alerts",
    responses(
        (status = 200, description = "Everything the print page needs", body = RenderPayload),
        (status = 401, description = "No render token"),
        (status = 404, description = "Token unknown, expired, or already used"),
    ),
)]
async fn render_payload(
    State(st): State<AutomationState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<RenderPayload>, AutomationApiError> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(AutomationApiError::from(egret_core::Error::Unauthorized))?;
    let grant = st.render_tokens.redeem(token).await.map_err(AutomationApiError::from)?;
    let payload = crate::render::payload::build(&st, &grant).await.map_err(AutomationApiError::from)?;
    Ok(Json(payload))
}
```

Add `pub render_tokens: RenderTokenRepo` to `AutomationState` and construct it in
`crates/egret-bin/src/main.rs` beside `smtp`, and in every test fixture that builds an
`AutomationState` (`schedule_routes.rs`, `smtp_routes.rs`, `tick_end_to_end.rs`).

- [ ] **Step 5: Run the tests to verify they pass**

Run: `DATABASE_URL=... cargo test -p egret-automation --test render_payload_routes`
Expected: PASS, 4 tests.

- [ ] **Step 6: Regenerate the API contract**

```bash
cargo run -p egret-bin -- openapi > openapi.json
cd frontend && npm run gen:api && cd ..
cargo test -p egret-server --lib committed_openapi_json_is_current
```

Also run `cargo test -p egret-server --test feature_route_coverage`. `render` is NOT
workspace-scoped, so it needs no catalog entry — but if that test fails, read its message rather
than guessing. **P3 shipped an unclassified route that all eight of its own tests missed.**

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "feat(render): one-shot payload endpoint, authenticated by render token"
```

---

## Task 4: The print route and its readiness flag

**Files:**
- Create: `frontend/src/routes/print/PrintObject.tsx`, `frontend/src/api/render.ts`
- Modify: `frontend/src/App.tsx` (route, OUTSIDE `RequireAuth`)
- Test: `frontend/src/routes/print/__tests__/PrintObject.test.tsx`

**Interfaces:**
- Consumes: `GET /api/render/payload` with `Authorization: Bearer <token>`; token read from
  `?t=` in the URL.
- Produces: `window.__EGRET_PRINT_READY__ === true`, and `window.__EGRET_PRINT_FAILED__ === true`
  on any error.

- [ ] **Step 1: Write the failing tests**

```tsx
// The flag IS the contract with the renderer. A flag that is set
// unconditionally is the bug this file exists to prevent, so each precondition
// is withheld in turn.
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import PrintObject from "@/routes/print/PrintObject";

const payloadMock = vi.fn();
vi.mock("@/api/render", () => ({ fetchRenderPayload: (...a: unknown[]) => payloadMock(...a) }));

// One chart that never reports `finished`, to prove the flag waits for it.
let finishHandlers: Array<() => void> = [];
vi.mock("@/components/chart/ChartRender", () => ({
  ChartRender: ({ onFinished }: { onFinished: () => void }) => {
    finishHandlers.push(onFinished);
    return <div data-testid="chart" />;
  },
}));

function renderPrint() {
  return render(
    <MemoryRouter initialEntries={["/embed/print/dashboard/d1?t=tok"]}>
      <Routes><Route path="/embed/print/:kind/:id" element={<PrintObject />} /></Routes>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  finishHandlers = [];
  (window as never as Record<string, unknown>).__EGRET_PRINT_READY__ = undefined;
  Object.defineProperty(document, "fonts", { value: { ready: Promise.resolve() }, configurable: true });
  payloadMock.mockResolvedValue({
    kind: "dashboard", id: "d1", title: "Ops",
    cards: [{ chart_id: "c1", title: "A", viz_type: "bar", params: {}, columns: ["n"], rows: [[1]] }],
  });
});

it("does not signal ready while a chart has not finished", async () => {
  renderPrint();
  await waitFor(() => expect(payloadMock).toHaveBeenCalled());
  await new Promise((r) => setTimeout(r, 50));
  expect((window as never as Record<string, unknown>).__EGRET_PRINT_READY__).toBeUndefined();
});

it("signals ready once every chart finishes and fonts resolve", async () => {
  renderPrint();
  await waitFor(() => expect(finishHandlers.length).toBe(1));
  finishHandlers.forEach((f) => f());
  await waitFor(() =>
    expect((window as never as Record<string, unknown>).__EGRET_PRINT_READY__).toBe(true));
});

it("does not signal ready when fonts never resolve", async () => {
  Object.defineProperty(document, "fonts", { value: { ready: new Promise(() => {}) }, configurable: true });
  renderPrint();
  await waitFor(() => expect(finishHandlers.length).toBe(1));
  finishHandlers.forEach((f) => f());
  await new Promise((r) => setTimeout(r, 50));
  expect((window as never as Record<string, unknown>).__EGRET_PRINT_READY__).toBeUndefined();
});

it("marks failure rather than readiness when the payload fetch fails", async () => {
  // A failed page must never be captured. The renderer times out, and the run
  // is recorded `error`.
  payloadMock.mockRejectedValue(new Error("404"));
  renderPrint();
  await waitFor(() =>
    expect((window as never as Record<string, unknown>).__EGRET_PRINT_FAILED__).toBe(true));
  expect((window as never as Record<string, unknown>).__EGRET_PRINT_READY__).toBeUndefined();
});
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd frontend && npx vitest run src/routes/print/__tests__/PrintObject.test.tsx`
Expected: FAIL — cannot resolve `@/routes/print/PrintObject`.

- [ ] **Step 3: Implement the route**

`frontend/src/api/render.ts`:

```ts
import type { RenderPayload } from "./generated/model";

/** The print page's ONLY authenticated call. The token rides in a header, not
 *  the path: the navigation already had to put one in a URL, and this request
 *  is ours to control. */
export async function fetchRenderPayload(token: string): Promise<RenderPayload> {
  const res = await fetch("/api/render/payload", {
    headers: { Authorization: `Bearer ${token}` },
  });
  if (!res.ok) throw new Error(`render payload: ${res.status}`);
  return (await res.json()) as RenderPayload;
}
```

`frontend/src/routes/print/PrintObject.tsx` renders the cards on a white ground with no chrome, and
owns the readiness protocol:

```tsx
// Readiness is a FLAG, never a timer. ECharts animates on mount and web fonts
// resolve after first paint, so a renderer that waits a fixed two seconds gets
// a finished chart most of the time and a half-drawn one occasionally — a
// defect no test sees and that reproduces perhaps one run in twenty.
const [finished, setFinished] = useState(0);
const [fontsReady, setFontsReady] = useState(false);
useEffect(() => { document.fonts.ready.then(() => setFontsReady(true)); }, []);
useEffect(() => {
  if (payload && fontsReady && finished >= payload.cards.length) {
    (window as unknown as Record<string, unknown>).__EGRET_PRINT_READY__ = true;
  }
}, [payload, fontsReady, finished]);
```

Set `__EGRET_PRINT_FAILED__ = true` in the fetch's catch and in any chart error boundary. Apply
`prefers-reduced-motion` in this route's container so animations are short — belt and braces with
the flag, not a substitute for it.

- [ ] **Step 4: Add the route OUTSIDE the auth guard**

In `frontend/src/App.tsx`, beside the `/login` route (NOT inside `RequireAuth`/`RequireWorkspace`):

```tsx
<Route path="/embed/print/:kind/:id" element={<PrintObject />} />
```

The renderer's browser has no session — that is the entire point of the token. A print route behind
`RequireAuth` would redirect it to the login page and screenshot that.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cd frontend && npx vitest run src/routes/print/__tests__/PrintObject.test.tsx`
Expected: PASS, 4 tests.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/routes/print frontend/src/api/render.ts frontend/src/App.tsx
git commit -m "feat(render): chrome-free print route with an explicit readiness flag"
```

---

## Task 5: The `egret-renderer` service

**Files:**
- Create: `renderer/package.json`, `renderer/server.js`, `renderer/Dockerfile`,
  `renderer/test/render.test.js`
- Modify: `Justfile` (`images-build`, `deploy-kind`)

**Interfaces:**
- Produces: `POST /render {url, width, height, format, scale}` → image bytes, or non-2xx with
  `{error}`. `GET /healthz` → 200.

- [ ] **Step 1: Write the service**

`renderer/server.js` — one browser, a page per request, a concurrency cap, and the readiness wait:

```js
// Waits for the page's OWN readiness flag. A timeout is a FAILED render: a
// half-drawn chart in someone's inbox is worse than a missing one, because
// nobody checks a report that arrived.
const READY_TIMEOUT_MS = Number(process.env.RENDER_READY_TIMEOUT_MS || 20000);
const MAX_CONCURRENT = Number(process.env.RENDER_MAX_CONCURRENT || 2);

await page.goto(url, { waitUntil: "domcontentloaded" });
await page.waitForFunction(
  () => window.__EGRET_PRINT_READY__ === true || window.__EGRET_PRINT_FAILED__ === true,
  { timeout: READY_TIMEOUT_MS },
);
const failed = await page.evaluate(() => window.__EGRET_PRINT_FAILED__ === true);
if (failed) throw new Error("the print page reported a failure");
```

then `page.screenshot({ type: "png" })` or `page.pdf({ printBackground: true })`. Chromium memory,
not CPU, is the binding constraint — hold the concurrency cap.

- [ ] **Step 2: Write the tests**

`renderer/test/render.test.js`, served from a local fixture page:

```js
// 1. a page that sets __EGRET_PRINT_READY__ → PNG bytes, non-trivial size
// 2. a page that never sets it → 5xx with "timeout", and NO image
// 3. a page that sets __EGRET_PRINT_FAILED__ → 5xx, and NO image
// 4. format: "pdf" returns a %PDF- header
```

Assert on **byte length and the PNG magic number**, not just a 200: the repo's PNG-export
experience is that a 16×16 or 32×32 result means something grabbed a UI icon rather than the chart.

- [ ] **Step 3: Run the tests**

Run: `cd renderer && npm install && npm test`
Expected: PASS, 4 tests.

- [ ] **Step 4: Dockerfile and Justfile**

Base on `mcr.microsoft.com/playwright:v1.x-jammy` (it already carries Chromium **and the fonts** —
a slim base silently renders boxes instead of text, which no test catches). Then:

```makefile
images-build:
    ...
    docker build -t egret-renderer:dev renderer

deploy-kind: images-build
    kind load docker-image egret-backend:dev egret-frontend:dev egret-kernel:dev egret-renderer:dev --name egret
```

- [ ] **Step 5: Commit**

```bash
git add renderer Justfile
git commit -m "feat(render): egret-renderer service, waiting on the page's readiness flag"
```

---

## Task 6: Render client + the report branch of the tick

**Files:**
- Create: `crates/egret-automation/src/render/client.rs`
- Modify: `crates/egret-automation/src/render/mod.rs`, `crates/egret-automation/src/runner.rs`,
  `crates/egret-automation/src/model.rs`, `crates/egret-config/src/lib.rs`
- Test: `crates/egret-automation/tests/report_tick.rs`

**Interfaces:**
- Consumes: `RenderTokenRepo::mint`, `AutomationRunner`'s existing delivery path.
- Produces:
  - `pub trait Renderer: Send + Sync { async fn render(&self, req: RenderRequest) -> Result<Vec<u8>, String>; }`
  - `pub struct HttpRenderer { base_url: String }` implementing it
  - `pub struct RenderRequest { pub url: String, pub width: u32, pub height: u32,
    pub format: String, pub scale: u32 }`
  - `AutomationConfig` gains `renderer_url: String`, `public_url: String`.

- [ ] **Step 1: Write the failing tests**

`crates/egret-automation/tests/report_tick.rs`, modelled on `tick_end_to_end.rs` with a fake
`Renderer`:

```rust
#[sqlx::test(migrations = "../../migrations")]
async fn a_png_report_renders_and_emails_the_image(pool: sqlx::PgPool) {
    // The whole point of P4, asserted on the RUN LOG and the fake renderer's
    // record — never on the absence of an error.
    let fx = setup(pool, RenderOutcome::Png(vec![0x89, b'P', b'N', b'G'])).await;
    assert_eq!(fx.runner.tick_once().await, 1);
    let log = run_log(&fx).await;
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].0, "success");
    let seen = fx.renderer.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "the renderer was asked exactly once");
    assert!(seen[0].url.contains("/embed/print/dashboard/"), "{}", seen[0].url);
    assert!(seen[0].url.contains("t="), "the URL carries a render token");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_renderer_failure_is_an_error_run_that_stays_scheduled(pool: sqlx::PgPool) {
    let fx = setup(pool, RenderOutcome::Fail("timeout".into())).await;
    fx.runner.tick_once().await;
    let log = run_log(&fx).await;
    assert_eq!(log[0].0, "error");
    assert!(log[0].1.as_deref().unwrap_or_default().contains("timeout"));
    let (_, next) = schedule_state(&fx).await;
    assert!(next.unwrap() > chrono::Utc::now(), "a failing report keeps trying");
}

#[sqlx::test(migrations = "../../migrations")]
async fn each_render_gets_a_fresh_token(pool: sqlx::PgPool) {
    // Single-use means a second run cannot reuse the first run's token.
    let fx = setup(pool, RenderOutcome::Png(vec![0x89, b'P', b'N', b'G'])).await;
    fx.runner.tick_once().await;
    make_due_again(&fx).await;
    fx.runner.tick_once().await;
    let seen = fx.renderer.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert_ne!(token_of(&seen[0].url), token_of(&seen[1].url));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_csv_report_never_calls_the_renderer(pool: sqlx::PgPool) {
    // csv and text are data the backend already has.
    let fx = setup_csv_chart_report(pool).await;
    fx.runner.tick_once().await;
    assert_eq!(run_log(&fx).await[0].0, "success");
    assert!(fx.renderer.seen.lock().unwrap().is_empty());
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `DATABASE_URL=... cargo test -p egret-automation --test report_tick`
Expected: FAIL to compile (`render::client` missing).

- [ ] **Step 3: Implement the client and the report branch**

`client.rs` posts to `{renderer_url}/render` with a timeout below the renderer's own. In
`runner.rs`, `evaluate_and_deliver` gains a report branch **before** the alert branch:

```rust
if s.r#type == "report" {
    return self.render_and_deliver(s).await;
}
```

`render_and_deliver`:
1. resolve the owner (same helper the alert path uses) — no owner is an `EvalError::NoOwner`;
2. for `csv`/`text`, build from the chart's data with no renderer;
3. otherwise mint a token for `(kind, object_id, owner)`, build
   `{public_url}/embed/print/{kind}/{id}?t={token}`, call the renderer, attach the bytes;
4. deliver through the existing email/webhook path;
5. return `Outcome::Fired` — reports have no condition, so they always "fire".

`AutomationConfig` gains `renderer_url` (default `http://egret-renderer:8080`) and `public_url`
(the base URL the RENDERER can reach — not necessarily the user-facing one).

Also prune expired tokens on the tick, beside the run-log prune already there (spec §5). A token
row outlives its usefulness by definition after ~60s, and nothing else deletes one that was minted
for a render that never happened:

```rust
if let Err(e) = self.render_tokens.prune_expired().await {
    tracing::warn!(error = %e, "automation: pruning render tokens failed");
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `DATABASE_URL=... cargo test -p egret-automation --test report_tick`
Expected: PASS, 4 tests.

- [ ] **Step 5: Full backend gate**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
psql "$PGURL" -c "CREATE DATABASE egret_p4;"
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_p4 cargo test --workspace
```

Check the **count** of `test result: ok` lines, not just the exit code: `cargo test` stops at the
first failing binary, so an aborted run looks like a small green one.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat(render): render reports on the tick, as the schedule's owner"
```

---

## Task 7: Helm deployment

**Files:**
- Create: `deploy/helm/egret/templates/renderer-deployment.yaml`,
  `deploy/helm/egret/templates/renderer-service.yaml`
- Modify: `deploy/helm/egret/values.yaml`, `deploy/helm/egret/templates/configmap-env.yaml`

- [ ] **Step 1: Write the Deployment and Service**

Model them on `backend-deployment.yaml`/`backend-service.yaml`. ClusterIP only — **no Ingress**: a
service that screenshots authenticated pages must not be reachable from outside the cluster.
Memory request/limit sized for Chromium (start at 512Mi/1Gi); a too-small request is an OOMKill
under load, which users experience as "reports work sometimes".

- [ ] **Step 2: Point the backend at it**

Add to `configmap-env.yaml`:

```yaml
EGRET_AUTOMATION__RENDERER_URL: "http://{{ include "egret.fullname" . }}-renderer:8080"
EGRET_AUTOMATION__PUBLIC_URL: "http://{{ include "egret.fullname" . }}-frontend"
```

`PUBLIC_URL` is the URL **the renderer** resolves, which is a Service name — not the user's
external hostname, which the pod may not resolve at all.

- [ ] **Step 3: Verify the chart renders**

Run: `helm template deploy/helm/egret | grep -A5 renderer`
Expected: a Deployment and a Service, no Ingress rule.

- [ ] **Step 4: Commit**

```bash
git add deploy/helm && git commit -m "feat(render): always install the renderer alongside egret"
```

---

## Task 8: Live check on kind — the only thing that proves this works

**Files:** none (verification)

No test can assert "the PNG contains the chart". This task is where P4 is actually confirmed.

- [ ] **Step 1: Deploy**

Run: `just deploy-kind`
Expected: four images loaded; `kubectl get pods` shows a running `-renderer` pod.

- [ ] **Step 2: Schedule a real report**

Through the UI: create a dashboard with at least one chart, then a report schedule on it,
`png`, `* * * * *`, with an email recipient. Configure SMTP against mailpit
(`docker run -d -p 1025:1025 -p 8025:8025 axllent/mailpit`).

- [ ] **Step 3: Wait for it to fire, then OPEN the attachment**

```bash
curl -s http://localhost:8025/api/v1/messages | python3 -m json.tool | head -20
```

Check, in order:
1. the run log says `success`;
2. the message has one attachment;
3. **its pixel dimensions match the requested size** and its byte count is in the tens or hundreds
   of KB. A 16×16 or 32×32 image means something captured a UI icon rather than the chart;
4. **look at the picture.** Is the chart fully drawn — bars at full height, labels in the real
   font, no login page? This is the assertion no automated check can make, and the one defect
   class this whole task exists to catch.

- [ ] **Step 4: Prove the readiness flag is load-bearing**

Temporarily raise the renderer's `RENDER_READY_TIMEOUT_MS` to `1` and re-run. Expected: the run is
recorded `error` with a timeout, and **no image is delivered**. Restore.
If a picture still arrives, the renderer is capturing without waiting and the flag is decorative.

- [ ] **Step 5: Record what was observed**

Update P4's section in `docs/en/plans/2026-08-11-alerts-and-reports-plan.md` with what the live
check actually showed, including anything it found. Commit.

---

## Self-review notes

- Spec §4 (readiness) → Tasks 4, 5, 8·4. §5 (token) → Tasks 1, 2, 3. §6 (service) → Task 5.
  §7 (formats/constraint) → Tasks 1, 6. §8 (failure) → Task 6. §9 (deployment) → Task 7.
  §10 (verification) → every task's tests plus Task 8.
- `RenderPayload`/`RenderCard` are defined in Task 3 and consumed by name in Task 4;
  `Renderer`/`RenderRequest` are defined in Task 6 and used by Tasks 6 and 8.
- Task 3 Step 4 changes `AutomationState`, which breaks three existing test fixtures. That is
  called out in the step rather than left to be discovered at compile time.
- Gap found and fixed during this review: `prune_expired` was produced by Task 2 and called by
  nobody. It is now wired into the tick in Task 6 Step 3.
