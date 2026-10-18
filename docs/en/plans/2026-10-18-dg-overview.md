# Governance Overview (sub-project #3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `/governance` opens an Overview: a counts strip, "Needs attention" (existing signals plus plugin drift,
grouped by severity, each linking to where it is fixed) and "Recent changes" (governance events, recorded from now
on in the write's own transaction).

**Architecture:** A transactional governance-event helper in `egret-audit` called by every governance write
(`egret-perm-compile`, `egret-bi`, `egret-policy`); a `perm.compile_runs` row written by the compile funnel; two read
routes in `egret-controlplane` (`/governance/attention`, `/governance/changes`), each source of the first isolated in
a savepoint of one read-only snapshot and filtered by the reader's permissions. Frontend: a lazy
`routes/governance/overview/` area mounted as the Governance index.

**Tech Stack:** Rust (axum, sqlx, utoipa), React 18 + TypeScript, TanStack Query via the orval client, Radix UI,
vitest + Testing Library, Playwright MCP for the live check.

**Spec:** `docs/en/specs/2026-10-18-dg-overview-design.md` (O1–O13; read "Premises in the brief that the code
contradicts" first) on the programme spec `docs/en/specs/2026-10-16-dg-frontend-redesign-design.md` (F1–F12).

## Global Constraints

- Base main `4a67321b`; branch `feat/dg-overview`; worktree `egret-dgov`. Never read or touch
  `data-portal/egret` (stale checkout). Rebase on fresh main before the final gate; regenerate the client after it.
- **Migrations 0196 and 0197 only.** F11 (engine grants) holds 0195 and is in flight in parallel; it also edits
  `crates/egret-perm-compile/src/routes.rs`. Whichever package merges second rebases and resolves. F11 will call
  `egret_audit::governance::record_change` for engine-grant writes if this package merged first, so the helper's
  name, signature and action strings (spec §3.1) do not change without telling the controller.
- **Private services, never shared ones.** One Postgres and one Valkey for this package:
  ```bash
  docker run -d --name dgov-pg -e POSTGRES_USER=egret -e POSTGRES_PASSWORD=egret -e POSTGRES_DB=egret \
    -p 127.0.0.1:55496:5432 postgres:16
  docker exec dgov-pg sh -c 'until pg_isready -U egret; do sleep 1; done; createdb -U egret egret_test'
  docker run -d --name dgov-valkey -p 127.0.0.1:56496:6379 valkey/valkey:8
  export DATABASE_URL=postgres://egret:egret@127.0.0.1:55496/egret_test
  export EGRET_TEST_SHARED_STORE_URL=redis://127.0.0.1:56496
  export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-dgov/target
  ```
  Never DB `egret`, never `egret-dev-*`, `egret-w7*` or another package's container. Never `EGRET_ALLOW_DB_SKIP=1`.
  `cargo test` binaries read `.env` through dotenvy: confirm `DATABASE_URL` is exported in the same shell.
- Every per-task cargo command below assumes those exports. Rust per task: the named `cargo test -p <crate> --test
  <file>` lines, then `cargo test -p <crate>` for each touched crate, `cargo clippy -p <crate> --all-targets -- -D
  warnings`, `cargo fmt --all -- --check`.
- Never widen an existing route's permission. New routes use `egret_core::authz::require` (through
  `require_governance_read`, `crates/egret-controlplane/src/governance_browse.rs:337`).
- An engine with `origin = external` is never dialled and never judged for liveness (spec §4.3).
- Frontend: `docs/en/frontend-rules.md` in full; egret + jsx-a11y lint; never add a file to
  `eslint-rules/legacy.js`; OKLCH tokens as `var(--token)`; regenerate with `cargo build -p egret-bin &&
  target/debug/egret openapi > openapi.json` then `cd frontend && npm run gen:api` only. Shared components change
  additively (FE-TEST-3): `Sidebar` gains an optional `end`; `OverlapReport`, `peopleUrl.ts` and
  `data/objectPath.ts` are used, not changed.
- Frontend per task: `npm run lint`, `npx tsc -b`, `npx vitest run <touched paths>`.
- Never `git stash`, `git checkout -- <file>`, `git clean`. Kill only your own PIDs. ≥ 30 GiB free disk before any
  build (`df -h`); `target/` and `$TMPDIR/egret-ranger-diff-*` fill disks. Never commit `.superpowers/` or scratch
  files. Commits: Conventional Commits, identity hoangquocvietbk2.13@gmail.com, trailer
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Do not push.
- UI copy: English, sentence case, short and factual. A label never claims more than its data shows.

## File structure

Backend:
- `crates/egret-audit/src/governance.rs` (new): action constants, `Family`, `family_of`, `GOVERNANCE_ACTIONS`,
  `Change`, `record_change`, `governance_changes_via` (the feed query). Tests: `crates/egret-audit/tests/governance.rs`.
- `migrations/0196_perm_compile_runs.sql`, `migrations/0197_audit_mutation_index.sql` (new).
- `crates/egret-perm-compile/src/compile_runs.rs` (new): `record_compile_ok_in_tx`, `record_compile_failure`,
  `compile_runs_via`. Modify `trigger.rs` (funnel, `recompile_service`), `routes.rs` (`write_grant`, `write_rule`,
  `delete_grant`, `delete_rule`, `compile`, migrate/undo routes), `lib.rs`. Tests:
  `crates/egret-perm-compile/tests/compile_runs.rs`, `tests/governance_events.rs`.
- `crates/egret-bi/src/tags.rs` (`set_policy`, `clear_policy`), `crates/egret-controlplane/src/tags.rs` (callers).
  Tests: `crates/egret-bi/tests/clearance_events.rs`.
- `crates/egret-policy/src/admin/mod.rs` (`write_policy_checked` and its four callers, `delete_policy_handler`,
  `set_policy_enabled`), `crates/egret-policy/Cargo.toml` (+ `egret-audit`). Tests:
  `crates/egret-policy/tests/policy_events.rs`.
- `crates/egret-perm/src/` (new `plugin_report.rs` or beside `engine_images`): `ranger_plugin_report`; modify
  `crates/egret-engine-mgmt/src/engines.rs` to call it.
- `crates/egret-controlplane/src/governance_overview.rs` (new): both routes, the source list, sentences.
  Modify `lib.rs` (router, OpenAPI, `CpState.audit_retention_days`), every `CpState { .. }` literal (tests and
  `egret-server`, `egret-bin`). Tests: `crates/egret-controlplane/tests/governance_attention.rs`,
  `tests/governance_changes.rs`.
- `crates/egret-config/src/lib.rs`: `AuditConfig::enabled` doc comment.

Frontend (`frontend/src/routes/governance/overview/`): `OverviewPage.tsx`, `CountsStrip.tsx`,
`AttentionSection.tsx`, `AttentionRow.tsx`, `RecentChanges.tsx`, `refUrl.ts`, `__tests__/`. Modify `App.tsx`,
`components/layout/Sidebar.tsx`, `frontend/package.json`, `docs/en/frontend-rules.md` (FE-ROUTE-2, FE-TEST-4 lists);
new `frontend/scripts/assert-governance-overview-lazy.mjs`. Update tests that pin the governance index or sidebar
order (find them: `grep -rln "governance/data\"\|GovernanceIndex\|People & groups" frontend/src --include=*.test.tsx`).

---

### Task 1: live measurement — does the plugin ledger say what the drift rules assume?

No product code. The drift rules (spec §4.3, O9) rest on four premises nobody has measured. Measure them first.

- [ ] Write predictions first: create `docs/en/plans/2026-10-18-dg-overview-live.md` with `## Measurements (Task 1)`
  and `### 1.0 Predictions`, one predicted value per cell, commit, before any container starts.
- [ ] Stack as `docs/en/plans/2026-10-17-dg-people-groups-live.md` §2.2 (own `vg-` containers on `vg-net`, image
  from this branch, a throwaway DB `egret_dgov_live` in the private `dgov-pg`, Trino 483 JWT + StarRocks 4.1.6 with
  Ranger, engine rows inserted as `provisioned`/`running` and recorded as DEV-n, Ranger poll 5000 ms).
- [ ] Cells (read `policy.ranger_plugin_downloads` and `policy.authz_services` with `psql` between steps):
  - L1 rows per engine: how many `(plugin_id, kind)` rows each engine writes; Trino's `tags` rows land under its
    resource service; does StarRocks write `tags` rows at all?
  - L2 `plugin_id` stability: restart each engine container; does the plugin id change (a new row, the old one
    left behind)?
  - L3 lag: after `POST /perm/compile/{id}` that writes something, how many seconds until `last_known_version`
    equals the new `policy_version`; is 90 s enough at the default 30 s poll (rendered config)?
  - L4 `updated_at`: list every statement that sets `policy.authz_services.updated_at` without a version bump
    (grep the tree; then confirm one live) — it can only delay `plugin_behind`.
  - L5 stop StarRocks: rows keep their last `at`; no prune deletes them while nothing polls (spec premise 3).
- [ ] If L1 shows Trino's policies rows under a service other than the bound profile, or L2 shows ids that change
  on every poll: **STOP, report BLOCKED** with the cell — the rule needs redesign before Task 7.
- [ ] Tear down (containers, network, DB, image, scratch secrets), record `### 1.1 Result`, `### 1.2 Stack and
  deviations`, `### 1.3 Every cell`, commit `docs(dg): Overview plugin-ledger measurement (Task 1)`.

---

### Task 2: the governance-event helper and the migrations

**Files:** create `crates/egret-audit/src/governance.rs`, `migrations/0196_perm_compile_runs.sql`,
`migrations/0197_audit_mutation_index.sql`, `crates/egret-audit/tests/governance.rs`; modify
`crates/egret-audit/src/lib.rs` (`pub mod governance;`), `crates/egret-audit/Cargo.toml` if `sqlx`'s `postgres`
types are not yet enabled there, `crates/egret-config/src/lib.rs` (doc comment only).

**Interfaces (produces):** exactly spec §3.1, plus

```rust
pub struct ChangeRow { pub id: i64, pub ts: DateTime<Utc>, pub actor_user_id: Option<Uuid>,
                       pub impersonator: Option<Uuid>, pub action: String, pub resource_type: String,
                       pub resource_id: Option<String>, pub detail: serde_json::Value }
/// Newest first, `kind = 'mutation'`, `(action, resource_type)` in the pairs `families` allow, before `cursor`.
pub async fn governance_changes_via(conn: &mut PgConnection, families: &[Family],
    before: Option<(DateTime<Utc>, i64)>, limit: i64) -> Result<Vec<ChangeRow>, sqlx::Error>;
```

- [ ] **Step 1: failing tests** (`egret_test_db::url()`; each test inserts and reads only rows whose `resource_id`
  it generated):
  - `record_change` inside a transaction that then rolls back leaves no row; one that commits leaves exactly one with
    `kind = 'mutation'`, the action, `resource_type`, `resource_id`, `actor_user_id = principal.user_id`, and
    `detail` equal (as `serde_json::Value`, never via `to_string()`) to the summary;
  - an impersonating principal stamps `impersonator` (build it as `crates/egret-audit/tests/mutation_for.rs` does);
  - a 20 KiB summary is stored as `{"truncated": true, "bytes": <n>}`;
  - `family_of`: every `action::*` constant maps to its family; `("tag:assign","catalog")` → Tag;
    `("tag:assign","object")` → None; `("role:assign","grant")` → Policy; `("dashboard:certify", _)` → None;
    `GOVERNANCE_ACTIONS` holds every constant once (a test lists them by hand, so a constant added without the
    list fails);
  - `governance_changes_via`: a decision row and a BI `object` tag row are never returned; `[Tag]` returns no
    `perm_grant:*`; newest first; cursor `(ts, id)` of the last row of page 1 gives page 2 with no overlap and no gap
    (three rows with the same `ts`, distinct ids);
  - migration 0196: inserting `outcome = 'error'` with `error = NULL` fails the CHECK; deleting the profile cascades;
  - migration 0197: `pg_indexes` has `audit_log_mutation_ts_idx` with predicate `kind = 'mutation'`.
- [ ] **Step 2: run** `cargo test -p egret-audit --test governance` → fails (module missing).
- [ ] **Step 3: implement.** `record_change` is one `INSERT INTO audit.audit_log (kind, actor_user_id,
  impersonator, workspace_id, action, resource_type, resource_id, detail) VALUES ('mutation', …, NULL, …)`. The
  feed query builds its pair list from `family_of`'s table, not a second copy. Migration headers say why (spec §3.3).
  Update `AuditConfig::enabled`'s comment as spec §3.1 states.
- [ ] **Step 4: run** `cargo test -p egret-audit` + clippy + fmt → pass. Run `cargo test -p egret-config` (doc
  change only, must stay green).
- [ ] **Step 5: commit** `feat(audit): record governance changes in their write's transaction`.

---

### Task 3: `perm.compile_runs` — every compile's outcome

**Files:** create `crates/egret-perm-compile/src/compile_runs.rs`, `crates/egret-perm-compile/tests/compile_runs.rs`;
modify `trigger.rs` (`recompile_locked_service_in_tx` :1332, `recompile_service` :1356), `lib.rs`.

**Interfaces (produces):**

```rust
pub(crate) async fn record_compile_ok_in_tx(tx: &mut Transaction<'_, Postgres>, service_id: i64,
    s: &CompileSummary) -> Result<(), CompileError>;
pub async fn record_compile_failure(pool: &PgPool, service_id: i64, error: &str) -> Result<(), sqlx::Error>;
pub struct CompileRun { pub service_id: i64, pub service_name: String, pub outcome: String,
    pub attempted_at: DateTime<Utc>, pub error: Option<String>, pub last_ok_at: Option<DateTime<Utc>>,
    pub created: i32, pub updated: i32, pub deleted: i32, pub unchanged: i32, pub skipped: i32 }
pub async fn failed_compile_runs_via(conn: &mut PgConnection) -> Result<Vec<CompileRun>, sqlx::Error>;
```

- [ ] **Step 1: failing tests** (fixtures as `tests/recompile.rs`):
  - a grant create through `POST /perm/grants` leaves one `ok` row for the bound profile with `created ≥ 1`;
    recompiling unchanged leaves `ok` with `unchanged ≥ 1`, `created = 0` and a later `attempted_at`;
  - `recompile_services_in_tx` on a caller transaction that rolls back leaves no row (and an existing row unchanged);
  - a profile whose type the compiler does not target (`compile_desired` → `None`) gets no row;
  - `recompile_service` that fails (fixture: two stored overlapping masks in the profile, as `tests/overlap.rs`
    builds them, so the recompile is refused) writes `error` with the refusal's text, keeps the earlier
    `last_ok_at` and counts; a subsequent successful compile (overlap removed) writes `ok`, `error = NULL`;
  - a grant write whose recompile fails (the same overlap) answers 409 and writes **no** `error` row (spec §3.3);
  - `record_compile_failure` with an older `attempted_at` than the stored row changes nothing.
- [ ] **Step 2: run** `cargo test -p egret-perm-compile --test compile_runs` → fails.
- [ ] **Step 3: implement.** In the funnel, after `apply_diff_in_tx` returns `Ok`, call `record_compile_ok_in_tx`.
  In `recompile_service`, on `Err(e)`: drop/roll back the transaction, then `record_compile_failure(store.pool(),
  service_id, &e.to_string())` (log a failure of that write; return the original error). Error text capped at
  2 000 chars.
- [ ] **Step 4: run** `cargo test -p egret-perm-compile` (every existing recompile test must stay green: the new
  INSERT is on the compile's transaction and must not change any count those tests read) + clippy + fmt.
- [ ] **Step 5: commit** `feat(perm): record each profile's last compile`.

---

### Task 4: governance events for grants, rules, migrations and the manual compile

**Files:** `crates/egret-perm-compile/src/routes.rs`, `Cargo.toml` (+ `egret-audit` if not direct),
`crates/egret-perm-compile/tests/governance_events.rs`.

- [ ] **Step 1: failing tests** (real `CedarPdp` as `tests/routes.rs` builds it; a steward principal holding
  `policy:update`):
  - `POST /perm/grants` → one `perm_grant:create` event, `resource_id` = the new id, `detail.before = null`,
    `detail.after` = the grant summary (catalog name, levels, effect, actions, `kind:ref` principals);
  - `PATCH` → `perm_grant:update` with both; `DELETE` → `perm_grant:delete` with `after = null`;
  - the same three for rules (`perm_rule:*`);
  - `POST /perm/preview` → no event; a create refused with 409 (overlap fixture) → no event;
  - migrate dry run → no event; unproved (422) → no event; proved → one `policy_migration:apply` naming the
    created grants and rules; undo → `policy_migration:undo`;
  - `POST /perm/compile/{id}` → one `profile:compile` with the counts and a `perm.compile_runs` `ok` row; the refused
    compile fixture → one `profile:compile` with `outcome: "error"` and an `error` row; the response codes and
    bodies of every route equal what they were (compare with the existing route tests, which stay untouched).
- [ ] **Step 2: run** `cargo test -p egret-perm-compile --test governance_events` → fails.
- [ ] **Step 3: implement** spec §3.2's rows. `write_grant`/`write_rule` already take `&p`: read the before-row in
  the transaction, record after the row write. Rewrite `compile` (`routes.rs:3731`): `begin`, `get_service_by_id`
  first (keep the 404), `recompile_service_in_tx`, `record_change`, `commit`; on error roll back, then
  `record_compile_failure` + the error event in one new transaction, then return the original error unchanged.
- [ ] **Step 4: run** `cargo test -p egret-perm-compile` + clippy + fmt.
- [ ] **Step 5: commit** `feat(perm): audit grant, rule, migration and compile changes`.

---

### Task 5: governance events for clearances and hand-written Ranger policies

**Files:** `crates/egret-bi/src/tags.rs`, `crates/egret-controlplane/src/tags.rs`, every caller of `set_policy` /
`clear_policy` (`grep -rn "set_policy(\|clear_policy(" crates`), `crates/egret-policy/src/admin/mod.rs`,
`crates/egret-policy/Cargo.toml`; tests `crates/egret-bi/tests/clearance_events.rs`,
`crates/egret-policy/tests/policy_events.rs`.

- [ ] **Step 1: failing tests:**
  - `set_policy` writes one `tag_clearance:set` with `before` (null for a new gate) and `after` holding role names
    and group **names** (resolved in the transaction); `clear_policy` writes `tag_clearance:clear`; a set refused
    by an overlap (fixture as `tests/catalog_tag_overlap.rs`) writes none;
  - `POST /policies` → `ranger_policy:create` with the summary of spec §3.1 (resource lists capped at 20);
    `PUT` → `ranger_policy:update` with before/after; `DELETE` → `ranger_policy:delete`; `PUT …/enabled` →
    `ranger_policy:enable` or `:disable`; a write refused by the compiler-owned guard writes none;
  - existing `admin_routes.rs`, `classification_policy_db.rs` and the control-plane tag tests stay green.
- [ ] **Step 2: run** `cargo test -p egret-bi --test clearance_events` and
  `cargo test -p egret-policy --test policy_events` → fail.
- [ ] **Step 3: implement.** `set_policy(tag_id, roles, groups, actor: &egret_core::Principal)` replaces `by:
  Option<Uuid>` (store `actor.user_id.0` in `created_by` as before); `clear_policy` gains `actor`. Pass the principal
  into `write_policy_checked`. Record before `tx.commit()` in every case.
- [ ] **Step 4: run** `cargo test -p egret-bi`, `cargo test -p egret-policy`, `cargo test -p egret-controlplane`
  + clippy + fmt.
- [ ] **Step 5: commit** `feat(governance): audit clearance and hand-written policy changes`.

---

### Task 6: `ranger_plugin_report` in `egret-perm`

**Files:** `crates/egret-perm/src/` (new function), `crates/egret-engine-mgmt/src/engines.rs`.

- [ ] **Step 1: failing tests** in `egret-perm`: the five cases `engines.rs:7194-7305` assert (modelled external,
  unmodelled external, external with no version, provisioned with no pinned image, provisioned with a known
  unmodelled image) now called on `ranger_plugin_report` directly, with the same notes.
- [ ] **Step 2: run** `cargo test -p egret-perm` → fails.
- [ ] **Step 3: implement.** Move the body of `plugin_report` (`engines.rs:335-371`); `EngineDto::from` calls it.
  Do not change a sentence.
- [ ] **Step 4: run** `cargo test -p egret-perm`, `cargo test -p egret-engine-mgmt` (the engines tests unchanged and
  green) + clippy.
- [ ] **Step 5: commit** `refactor(perm): move the Ranger plugin report to egret-perm`.

---

### Task 7: `GET /governance/attention`

**Files:** create `crates/egret-controlplane/src/governance_overview.rs`,
`crates/egret-controlplane/tests/governance_attention.rs`; modify `crates/egret-controlplane/src/lib.rs`.

**Interfaces (produces):** spec §4.1's DTOs (`AttentionResponse`, `Finding`, `Severity`, `Sample`, `Ref`,
`NotChecked`, `Counts`, all `camelCase`, `ToSchema`), and

```rust
type SourceFn = for<'c> fn(&'c mut PgConnection) -> BoxFuture<'c, Result<Option<Finding>, SourceError>>;
pub(crate) struct Source { kind: &'static str, requires: Requires, run: SourceFn }
pub(crate) enum Requires { All(&'static [&'static str]), Any(&'static [&'static str]) }
pub(crate) const SOURCES: &[Source];
pub(crate) async fn attention_via(tx: &mut Transaction<'_, Postgres>, held: &HashSet<&str>,
    sources: &[Source]) -> AttentionResponse;
```

- [ ] **Step 1: failing tests** (`egret_test_db::url()`; one `CpState` as `tests/governance_lineage.rs:300-360`
  builds it, with a real `CedarPdp` and roles created per test; each fixture names its own objects with a
  per-test prefix, and assertions filter findings' samples to that prefix so other tests' rows cannot satisfy or
  break them):
  - one positive and one negative fixture per kind of spec §4.3, including: a plugin row with `at` before the
    profile's `updated_at` and an old version is ignored; a plugin row after `updated_at` with the old version and
    `updated_at` 2 min ago → `plugin_behind`; the same 30 s ago → nothing; no row since `updated_at` and
    `updated_at` 2 min ago → `plugin_behind`; last `at` 25 h ago → `plugin_silent` (not `plugin_quiet`); 10 min ago →
    `plugin_quiet`; a profile bound only to an external engine → none of the three; `engine_failed` for a provisioned
    `failed` engine and not for an external one; a `compile_runs` error row → `compile_failed` with the error's first
    line; `policy_warnings.count` equals the sum of `profile_warnings` over two profiles;
  - `count` is the total when there are 7 and `sample.len() == 5`, in the documented order;
  - permission filtering: `policy:read` only → no `engine_failed`, `plugin_*`, `plugin_unmodelled`,
    `catalog_apply_failed`, `tags_without_clearance`; each listed in `notChecked` with its permissions;
    `counts.tags == null`; `tag:read` only → no grant, rule, overlap, plugin or compile finding, `counts.grants ==
    null`; neither → 403; admin → every kind evaluated;
  - isolation: `attention_via` with `[failing, ..SOURCES]` where `failing` runs `SELECT 1/0` → one
    `source_unavailable` naming it, and every other kind still evaluated;
  - the snapshot is read-only: a source that tries an `INSERT` fails as `source_unavailable` (prove the transaction
    mode, not just intent).
- [ ] **Step 2: run** `cargo test -p egret-controlplane --test governance_attention` → fails.
- [ ] **Step 3: implement.** Store functions, not routes: `unresolved_principals_via`, `stored_overlaps_via`,
  `profile_warnings`, `failed_compile_runs_via`, `ranger_plugin_report`, plain SQL for engines, ledger, lineage,
  attachments, tags and catalogs. Sentences exactly as spec §4.3. Mount the route in `control_plane_router()` and
  its path in `openapi()`.
- [ ] **Step 4: run** `cargo test -p egret-controlplane` + clippy + fmt.
- [ ] **Step 5: commit** `feat(governance): what needs attention, per the reader's permissions`.

---

### Task 8: `GET /governance/changes`, OpenAPI and the client

**Files:** `governance_overview.rs`, `crates/egret-controlplane/tests/governance_changes.rs`, `lib.rs`
(`CpState.audit_retention_days`), every `CpState { … }` literal (`grep -rn "CpState {" crates`), `crates/egret-bin/src/main.rs`,
`openapi.json`, `frontend/src/api/generated/**`.

- [ ] **Step 1: failing tests:**
  - a `tag:read`-only reader sees `tag_clearance:*` and catalog `tag:assign` events and no `perm_grant:*`,
    `role:assign` or BI `object` tag event; a `policy:read`-only reader the reverse; 403 with neither;
  - labels: actor name; "A as B" pieces for an impersonated event; a deleted actor → `actor.label = null` with its
    id kept; a role event names the role (and its id when the role is gone); a legacy catalog-tag event names the
    catalog;
  - `targetExists`: false after the grant is deleted;
  - `sentence` per action (one assertion each, literal strings);
  - paging: 30 events, `limit=25` → 25 + cursor; the cursor → 5, no cursor; no duplicate ids;
  - `retentionDays` = 90 by default, `null` when the configured retention is 0.
- [ ] **Step 2: run** `cargo test -p egret-controlplane --test governance_changes` → fails.
- [ ] **Step 3: implement** spec §3.4; reuse `egret_audit::governance::governance_changes_via` and the cursor format of
  `audit.rs:57-60`.
- [ ] **Step 4: run** `cargo test -p egret-controlplane`, `cargo test -p egret-server --lib` + clippy + fmt.
- [ ] **Step 5: regenerate** `cargo build -p egret-bin && target/debug/egret openapi > openapi.json`, then
  `cd frontend && npm run gen:api`; `npx tsc -b` passes. Commit `feat(governance): recent governance changes`.

---

### Task 9: the Overview area, route and sidebar

**Files:** create `overview/OverviewPage.tsx` (shell only in this task), `overview/refUrl.ts`,
`frontend/scripts/assert-governance-overview-lazy.mjs`, tests; modify `App.tsx`, `Sidebar.tsx`, `package.json`,
`docs/en/frontend-rules.md`.

**Interfaces (produces):** `export function refUrl(r: Ref): string | null` (every `Ref` variant of spec §4.1; names
encoded through `encodeSegment`/`pathToUrl` and `principalUrl`; `{type:"object"}` → `pathToUrl(..., "lineage")`).

- [ ] **Step 1: failing tests:**
  - `refUrl` for every variant, including names with `/`, `%`, space and a dot-only name (round-trip through the
    Data route params, as `data/__tests__` does for `pathToUrl`);
  - `/governance` renders the Overview for `policy:read` and for `tag:read`; `/governance/nope` lands on
    `/governance` with `replace`; Data, People and every existing route still render;
  - sidebar: "Governance overview" is the first group with one item "Overview", visible with either permission;
    it is `aria-current="page"` on `/governance` and **not** on `/governance/data`; no heading equals another app's
    heading (assert over all three groups' arrays);
  - the existing tests that pinned `/governance` → `/governance/data` are updated to the new behaviour (list them in
    the report).
- [ ] **Step 2: run** `npx vitest run src/routes/governance/overview src/components/layout src/App` → fail.
- [ ] **Step 3: implement** spec §5. Bundle guard modelled on `scripts/assert-governance-people-lazy.mjs`, one marker
  per overview module; add `test:bundle:governance-overview` and list it in FE-ROUTE-2 and FE-TEST-4.
- [ ] **Step 4: run** tests + lint + tsc; `npm run build && npm run test:bundle:governance-overview
  test:bundle:governance-data test:bundle:governance-people` (each as its own `npm run`).
- [ ] **Step 5: commit** `feat(governance): Overview is the Governance index`.

---

### Task 10: counts strip and Needs attention

**Files:** `overview/{CountsStrip,AttentionSection,AttentionRow}.tsx`, `OverviewPage.tsx`, tests.

- [ ] **Step 1: failing tests** (MSW or the repo's existing fetch mock for orval hooks, as the people tests do):
  - counts: four items with values; a `null` count is absent (not "0"); singulars ("1 catalog", "1 egret grant");
  - grouping: findings rendered under "Critical", "Warning", "Info" headings in that order; an empty group renders
    no heading; each row shows the severity word (not only colour), the server's sentence, ≤ 5 samples each linked
    by `refUrl`, and "Open" linking to the row's `ref`; a sample with `ref: null` is text;
  - `source_unavailable` renders as a warning "Could not check …" with no link;
  - empty state: `findings: []`, `notChecked: []` → "Nothing needs attention."; `findings: []` with `notChecked`
    entries → "Nothing needs attention among the checks you can run." plus the not-checked lines naming the
    permissions; `findings` holding only `source_unavailable` → no "Nothing needs attention" text at all;
  - the fixed "Not checked here" line links to Classifications only with `tag:read`;
  - loading is a skeleton; a 500 shows the server message and a Retry button; the Recent changes section still
    renders when attention fails (and vice versa).
- [ ] **Steps 2–5:** run → implement → run + lint + tsc → commit `feat(governance): Overview counts and what needs attention`.

---

### Task 11: Recent changes

**Files:** `overview/RecentChanges.tsx`, `OverviewPage.tsx`, tests.

- [ ] **Step 1: failing tests:**
  - rows show when (`<time dateTime>` with the ISO value; relative text from `lib/relativeTime.ts`), who ("Ana";
    "Admin as Ana"; "A deleted user"), the sentence, and a link when `targetExists` and `refUrl` gives one; a
    deleted target shows "(deleted)" and no link;
  - "Show older" fetches with the cursor (a generated fetcher called directly is allowed only with a one-line comment
    naming the cursor reason, FE-DATA-2) and appends without duplicates; it is absent when `nextCursor` is null;
  - footnote with `retentionDays` 90 → "pruned after 90 days"; `null` → "never pruned"; the best-effort sentence for
    role and catalog-tag events;
  - empty → the spec §2 sentence; error → message + Retry; loading → skeleton;
  - at 390 px the list renders as cards (assert the card markup is used when `useIsNarrowViewport` is true).
- [ ] **Steps 2–5:** run → implement → run + lint + tsc → commit `feat(governance): Overview recent changes`.

---

### Task 12: live browser check

- [ ] Predictions first in `2026-10-18-dg-overview-live.md` `## 2.` (commit before the stack starts).
- [ ] Stack as Task 1 (rebuilt image from this branch); vite dev server from this worktree on a free port (not 5173,
  5000, 8080), proxying `/api`, `/catalog`, `/oauth` to the egret container. Log in as `admin@egret.local` /
  `Admin12345!` and once as a steward holding `policy:read` + `policy:update` only, once as one holding `tag:read`
  only. Logins are rate-limited (5 per 5 min): space them ≥ 60 s apart.
- [ ] Seed, recording each as DEV-n when it is SQL:
  - an orphan group ref in a deny grant's include (`perm.grant_principals` row naming group `ghost`, SQL: writes refuse
    it) → `unresolved_principals_fail_open`;
  - two overlapping mask rules (one through the API, the second as a hand-written masking policy) →
    `rule_overlaps`;
  - a failed lineage (`view_lineage_state` row `failed`, or a view whose SQL Trino cannot plan) → `lineage_failed`;
  - stop StarRocks (`docker stop vg-sr`): after 5 min → `plugin_quiet`; then compile its profile → `plugin_behind`
    after 90 s; set its rows' `at` to 25 h ago (SQL) → `plugin_silent`;
  - a failed catalog attachment (`apply_status = 'failed'`, SQL) → `catalog_apply_failed`;
  - `POST /perm/compile/{id}` on the overlap profile → `compile_failed`;
  - a tag with no clearance; a catalog with no engine.
- [ ] With Playwright MCP:
  - each finding's row, sentence and every link (click through; the target page shows the thing named);
  - the steward without `engine:read` sees no engine or plugin finding and the not-checked line; the `tag:read`
    steward sees tag findings and no grant change in Recent changes;
  - make a grant, a rule, a clearance, a hand-written policy change, a migration and a manual compile through the UI;
    each appears in Recent changes with the right actor and link; a preview and a refused write do not; impersonate
    a user and change a grant → "Admin as <user>";
  - `/governance` and `/governance/nope`; the sidebar item active state;
  - 390 px (no horizontal page scroll; rows readable) and 1280 px, light and dark; keyboard-only through the page.
    Screenshots to the scratchpad, checked with `sips` (pixel size and bytes; a 16 or 32 px square is an icon).
- [ ] Record every cell with a verdict; defects go to a fix round, not here. Tear down; commit the doc.

---

## Final gate (controller)

Whole-branch review on the most capable model; one fix dispatch + scoped re-review; rebase on fresh main (resolve
F11's `routes.rs` and migration numbering if it merged first, then regenerate `openapi.json` and the client). Record
test counts **before** (on `4a67321b`, same commands) and **after**, from the full output, never a `tail`.

With the private services and exports of Global Constraints:

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret-dgov
./deploy/prune-target.sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
just test-shipping-resolution
env -u EGRET_ALLOW_DB_SKIP -u EGRET_ALLOW_STORE_SKIP EGRET_ALLOW_ENGINE_SKIP=1 EGRET_ALLOW_DOCKER_SKIP=1 \
  cargo test --workspace --no-fail-fast 2>&1 | tee "$SCRATCH/dgov-cargo-test.log"
just test-ranger-gates
cd frontend && npm ci && npm run lint && npx tsc -b && npx vitest run && npm run build \
  && npm run test:bundle && npm run test:bundle:governance-data && npm run test:bundle:governance-people \
  && npm run test:bundle:governance-overview
```

(`$SCRATCH` is the session scratchpad.) Count `test result:` lines' passed/failed/ignored from the log; a DB-gated
suite that reports `ok` in 0.00 s is a skip and fails the gate. Stop `dgov-pg` and `dgov-valkey` only after the gate.
