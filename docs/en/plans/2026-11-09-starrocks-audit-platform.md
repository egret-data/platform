# StarRocks audit, platform side (package A): implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** every StarRocks engine egret provisions on egret's image records every statement in a
30-day audit table inside the engine, through the AuditLoader plugin. egret shows that table in a
**Query log** tab on the Engine detail page, naming the person behind a delegated query.

**Spec:** `docs/en/specs/2026-11-09-starrocks-audit-platform-design.md` (and its `docs/vi`
mirror). Its four open questions were decided on 2026-11-10, each on the recommended option:
1 (b) `engine:audit`, 2 (a) deny everyone, 3 (a) no Ranger audit, 4 (b) a boot pass, with the
**Set up query log** button kept. The tasks below implement those answers; the "With (x)"
alternatives they mention are not built.

**Branch:** `feat/starrocks-query-log`, cut from `main` after package B merged (platform #847, `e565d7b4b`; A
consumes B's `egret_perm::is_patched_starrocks_fe` and B's `security_upgrade.rs`). Docs go to
`egret-data/docs`, branch `docs/starrocks-query-log`.

**Architecture.**
- **Setup:** a new module `crates/egret-engine-mgmt/src/query_log.rs` holds one idempotent
  `ensure_query_log`. It is called from the post-Ready bootstrap, from a new
  `spawn_query_log_setup` after StarRocks applies that may restart the FE, and from a
  synchronous setup route.
- **Ranger:** two new IR infrastructure kinds, rendered into an `egret-query-log-loader` row and
  `egret-query-log-guard - …` rows on the StarRocks profile, converged by the existing
  `converge_infra_in_tx`.
- **Read:** three `engine:read` routes in the same module read the table as `root` through
  `fe_target`, with egret-side redaction.
- **Frontend:** a `query-log` tab in `EngineDetail.tsx`.

## 0. Corrections to the spec's starting points, checked against `main` 26016f9b1

1. **"The provisioner creates the table" cannot be literal.** `crates/egret-provisioner` only
   renders Kubernetes objects (`starrocks.rs`). Every FE statement egret runs goes through
   `st.starrocks_ddl` with `starrocks_ddl::fe_target`, from `engines.rs`'s
   `spawn_starrocks_catalog_wire` or `catalog_apply.rs`. The setup therefore lives in
   `egret-engine-mgmt`; the spec says so in its section 1.
2. **The bootstrap does not run after every apply.** `update_starrocks_engine` spawns
   `spawn_starrocks_catalog_wire` only when `deleg_changed`, and B's `security_upgrade`
   (`crates/egret-engine-mgmt/src/security_upgrade.rs`, on `main` since #847) applies and returns. An image
   change restarts the FE without any bootstrap. Hence call site 2 (`spawn_query_log_setup`).
3. **`build_default_policies` is private** (`crates/egret-policy/src/default_policies.rs`), so
   the guard rows need a new public builder beside `admin_all_access_bodies`, not a call to it.
4. **The fork e2e grants the loader nothing natively.** Its `run.sh` creates `egret_audit` with
   no `GRANT`, because Ranger's `audit-loader` policy allows it. An egret engine bound to no
   profile runs native access control, so egret must `GRANT INSERT` there (spec section 1,
   step 4).
5. **`lower_infra` returns early when no bound engine is delegating**
   (`crates/egret-perm-ir/src/lower/infra.rs`), and a StarRocks engine without
   `starrocksConfig.delegation` does not qualify (`egret_core::engine_delegation::qualifies_for_infra`).
   The query-log statements must be emitted on their own flag, before that return.
6. **Migration numbers:** `main` has 0217 (B's `0217_engine_security_upgrade_pending.sql`).
   A takes **0218**. Re-check above `main` and every open PR before committing.

Task 1 adds a short "Corrections" note to both spec files saying 1, 2 and 4.

## Global Constraints

- **Build:** use your own target directory, `CARGO_TARGET_DIR=$PWD/target-sra`.
- **DB tests:** run them with
  `env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`.
  A DB-gated test that skipped is a failure of the report, not a pass.
- **Never run two test suites at once.**
- **Docker and stacks:**
  - never prune Docker;
  - never touch containers you did not start;
  - never run `just up`, `just down` or `just reset-testdata`.
- **StarRocks images:** only egret's fork images (`ghcr.io/egret-data/egret-starrocks-{fe,allin1}`),
  except the one deliberate `pluginNotShipped` run against the official
  `starrocks/allin1-ubuntu:4.1.7`.
- **Frontend client:** regenerate it only with `npm run gen:api` in `frontend/`.
- **Frontend rules:** `docs/en/frontend-rules.md`.
- **Tests:** every behaviour change has a test that fails without it and a mutant of the change
  that the test kills, recorded in the task report.
- **SQL:** every value from a request or a secret reaches SQL only through
  `egret_query_api::quote_string` (StarRocks dialect: it escapes `'` and `\`), or as an integer
  or timestamp egret formatted itself.
- **FE errors:** reach a log or a response only through `FeError::scrubbed(&fe, &secrets)`, with
  the loader password in `secrets`.
- **Commits:** Conventional Commits, ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **Before finishing a task:** `cargo fmt --all -- --check` and `cargo clippy -D warnings` on the
  touched crates are clean, and so are `npm run lint` and `npm run typecheck` when the frontend
  changed.

---

### Task 1: The loader principal, its secret, migration 0218 (and the spec corrections)

**Files:**
- `crates/egret-core/src/engine_principals.rs`
- `migrations/0218_engine_query_log.sql`
- `crates/egret-engine-mgmt/src/store.rs` (`Engine`, every `SELECT … applied_security_rev` column
  list, `set_root_secret`'s neighbour)
- `crates/egret-engine-mgmt/src/engines.rs` (`ensure_root_password_secret`'s neighbour)
- `crates/egret-auth/tests/` (the ledger test beside `username_ledger.rs`)
- both spec files (section 0's note)

**Produces:**

```rust
// egret_core::engine_principals
/// The StarRocks user the AuditLoader plugin stream-loads audit events as.
/// Reserved by migration 0218's `username_ledger` row, like `DELEGATOR_PRINCIPAL`.
pub const QUERY_LOG_LOADER_PRINCIPAL: &str = "egret_query_log";

// egret_engine_mgmt::store
pub struct Engine { /* … */ pub query_log_secret_id: Option<Uuid> }
impl EngineMgmtStore {
    pub async fn set_query_log_secret(&self, engine_id: Uuid, secret_id: Uuid)
        -> Result<(), EngineMgmtError>;
}

// egret_engine_mgmt::engines
pub(crate) async fn ensure_query_log_secret(st: &CpState, engine: &Engine)
    -> Result<(Uuid, String), CpError>;
```

**Requirements:**
- **Migration 0218:**
  - `ALTER TABLE engine_mgmt.engines ADD COLUMN query_log_secret_id uuid`;
  - `INSERT INTO username_ledger (username, source, retired_at) VALUES ('egret_query_log', 'reserved', now()) ON CONFLICT DO NOTHING`.
- **`ensure_query_log_secret`** is `ensure_root_password_secret` with:
  - the name `egret-query-log-<engine id>`;
  - the description "StarRocks query log loader password";
  - `set_query_log_secret`.

  The same recover-by-name branch. Factor the shared body into one private helper if it removes
  the third copy; do not change the other two's behaviour.
- **Every `Engine` row read** selects the new column. Grep `applied_security_rev` in `store.rs`
  for every list.
- **Tests:**
  - DB: the first call mints and stores; a second returns the same id and value; with the column
    cleared, it recovers by name;
  - the ledger holds `egret_query_log` after migrations, and minting a human named
    `egret_query_log` takes a suffixed name (follow the existing `egret_delegator` ledger test);
  - unit: `QUERY_LOG_LOADER_PRINCIPAL == "egret_query_log"`.
- **Mutants:**
  - drop the recover-by-name branch: the cleared-column test fails;
  - drop the ledger insert: the mint test fails.

### Task 2: Ranger, the loader allow and the guard as IR infrastructure

**Open question 2: decided (a).** That is what is written here; the alternatives below are not built. With (b), the guard rows
get a deny exception for the admin set on `select`. With (c), the audience excludes the admins.
Each is a change to `query_log_guard_bodies` and its test only.

**Files:**
- `crates/egret-perm-ir/src/statement.rs` (`InfraKind`, `InfraObject`)
- `crates/egret-perm-ir/src/lower/infra.rs` (`InfraInputs`, `lower_infra`, tests)
- `crates/egret-perm-compile/src/ir_load.rs` (`load_infra_inputs`)
- `crates/egret-perm-compile/src/ranger/infra.rs` (`render`, tests)
- `crates/egret-policy-api/src/infra_names.rs` (names, `is_ir_infra_name`,
  `is_egret_infrastructure_policy`, tests)
- `crates/egret-policy-api/src/infra_bodies.rs` (`query_log_loader_policy`)
- `crates/egret-policy/src/default_policies.rs` (`query_log_guard_bodies`)
- every other `InfraInputs { … }` literal: `crates/egret-perm-eval/benches/eval.rs` and the
  tests in `ranger/infra.rs` and `lower/infra.rs`

**Produces:**

```rust
// statement.rs
pub enum InfraKind { /* … */ QueryLogLoader, QueryLogGuard }
pub enum InfraObject { /* … */ QueryLogTable, QueryLogDatabase }

// lower/infra.rs
pub struct InfraInputs { /* … */ pub query_log: bool }

// infra_names.rs
pub const QUERY_LOG_LOADER_POLICY_NAME: &str = "egret-query-log-loader";
pub const QUERY_LOG_GUARD_NAME_PREFIX: &str = "egret-query-log-guard";
pub const QUERY_LOG_DATABASE: &str = "starrocks_audit_db__";
pub const QUERY_LOG_TABLE: &str = "starrocks_audit_tbl__";

// infra_bodies.rs
pub fn query_log_loader_policy(service: &str) -> Value;

// default_policies.rs
pub fn query_log_guard_bodies(service_name: &str)
    -> Result<Vec<(String, Value)>, PolicyStoreError>;
```

**Requirements:**
- **`load_infra_inputs`:** `query_log` is true when any bound engine has type `starrocks` and
  origin not external. It is computed from the same `engines` rows it already reads.
- **`lower_infra`:**
  - after `AdminAllAccess` and before the `!i.delegating` return, if
    `i.engine == EngineType::StarRocks && i.query_log`, push:
    - `QueryLogLoader`: audience user `QUERY_LOG_LOADER_PRINCIPAL`, `EnginePrivilege { capability: "insert" }`,
      target `QueryLogTable`;
    - `QueryLogGuard`: audience `Everyone` except user `QUERY_LOG_LOADER_PRINCIPAL`,
      `Effect::Deny { privileges: vec![Privilege::Read, Privilege::Write] }`, target
      `QueryLogDatabase`;
  - restructure the early return so `AdminAllAccess` keeps its own condition. Trino output is
    byte-for-byte unchanged; pin this with the existing `trino_impersonate_emits_all_five_in_order`
    test.
- **Bodies:**
  - `query_log_loader_policy`: resources `catalog=default_catalog`, `database=QUERY_LOG_DATABASE`,
    `table=QUERY_LOG_TABLE`, `column=*`; one allow item, user `egret_query_log`, access `insert`.
    Copy the item shape of the fork's `tests/e2e/policies.py` policy `audit-loader`;
  - `query_log_guard_bodies`: one row per resource hierarchy of `starrocks_service_def()` whose
    chain contains `database`. The name is `"egret-query-log-guard - <chain joined by ', '>"`,
    the separator `build_default_policies` uses. Each row has:
    - resources pinned to `default_catalog` / `QUERY_LOG_DATABASE`, and `*` below;
    - one deny item for group `public` with every access type the def allows on that leaf
      (read the def's access-type restrictions; fall back to all access types when it has none);
    - on the table hierarchy, a deny exception for user `egret_query_log`, access `insert`.
- **`render`:** the two new arms, `QueryLogLoader` (one row) and `QueryLogGuard` (the builder's
  rows). The match stays exhaustive, with no `_` arm.
- **Names:**
  - `is_ir_infra_name` accepts the loader name and the guard prefix plus separator;
  - `is_egret_infrastructure_policy` accepts them on `service_type == "starrocks"` only;
  - extend `the_ir_owns_egret_admin_and_the_four_fixed_names_only`, and rename it if its name
    becomes false.
- **The Trino evaluator** (`crates/egret-perm-eval/src/decide.rs`) matches `InfraObject` with
  `matches!`/`_ => false`. The new objects never reach a Trino snapshot. Add one test that
  `lower_infra` for Trino with `query_log = true` emits neither kind.
- **Tests:**
  - lowering: StarRocks with `query_log` → both kinds, in order; without it → neither; Trino →
    neither; StarRocks not delegating but `query_log` → both (correction 5);
  - render: the loader body equals a literal JSON; the guard rows' names, resources, the
    exception on the table row only, and `public` in every deny item;
  - DB (`crates/egret-perm-compile/tests/`, next to the existing infra convergence tests):
    binding a provisioned StarRocks engine adds the rows with `managed_by = 'infra'`, unbinding
    removes them, and an external StarRocks engine adds none;
  - a human `POST` of a policy named `egret-query-log-loader` on a StarRocks profile gets 409
    (the existing reserved-name path).
- **Mutants:**
  - emit the statements only when delegating: the not-delegating test fails;
  - drop the deny exception: the exception test fails;
  - drop the external filter: the external-engine DB test fails.

### Task 3: `ensure_query_log` and its three call sites

**Files:**
- create `crates/egret-engine-mgmt/src/query_log.rs` and register it in `lib.rs`
- `crates/egret-engine-mgmt/src/engines.rs`:
  - `spawn_starrocks_catalog_wire`: call site 1, plus the factored wait helper;
  - `update_starrocks_engine`: call site 2;
- `crates/egret-engine-mgmt/src/security_upgrade.rs` (B's file): call site 2, after its
  `apply_built` succeeds
- `crates/egret-engine-mgmt-api/src/lib.rs` (`SR_PROTECTED_FE_KEYS`)

**Consumes:** Task 1 (`ensure_query_log_secret`, `QUERY_LOG_LOADER_PRINCIPAL`); B's
`egret_perm::is_patched_starrocks_fe` and `egret_perm::stored_engine_image`.

**Produces:**

```rust
pub(crate) const QUERY_LOG_TABLE_DDL: &str; // the fork's audit-table.sql, verbatim
pub(crate) enum QueryLogSetup { Ready, PluginNotShipped, Failed(String) }
pub(crate) async fn ensure_query_log(st: &CpState, engine: &Engine, fe: &FeTarget,
    wait_for_events: bool) -> QueryLogSetup;
/// Pure: the statements of steps 2-4, in order (the password inline).
pub(crate) fn query_log_setup_sql(password: &str, bound_to_profile: bool) -> Vec<String>;
pub(crate) fn install_plugin_sql(password: &str) -> String;
/// Pure: from `SHOW PLUGINS` rows and the stored FE image.
pub(crate) fn plugin_step(show_plugins: &[Vec<Option<String>>], fe_image: Option<&str>) -> PluginStep;
pub(crate) enum PluginStep { AlreadyInstalled, Install, NotShipped }
pub(crate) fn spawn_query_log_setup(st: CpState, engine_id: Uuid, name: String, namespace: String)
    -> tokio::task::JoinHandle<()>;

// engines.rs: the two wait loops, factored out of spawn_starrocks_catalog_wire unchanged
pub(crate) async fn wait_for_fe_root_login(st: &CpState, engine_id: Uuid, name: &str,
    namespace: &str, consecutive_read_errors: &mut u32) -> Option<(Engine, FeTarget)>;
```

**Requirements:**
- **The steps** are exactly the spec's section 1 steps 1 to 6, in order:
  - step 1 asks a new `EngineMgmtStore::username_taken(&self, name: &str) -> Result<bool, EngineMgmtError>`
    (`SELECT EXISTS (SELECT 1 FROM users WHERE username = $1)`); a hit is
    `Failed("a user named egret_query_log exists; …")`;
  - `QUERY_LOG_TABLE_DDL` carries a comment naming the fork file and commit `d77446e`;
  - the `INSTALL PLUGIN` properties are `user`, `password`, `database`, `table`,
    `frontend_host_port = "127.0.0.1:8030"` and `max_batch_interval_sec = "60"`;
  - step 6's probe is `SELECT 'egret-query-log-probe-<uuid>'`, polled every 5 s for up to 180 s
    (shrunk under `#[cfg(test)]` like `MAX_POLLS`).
- **`plugin_step`:**
  - `AlreadyInstalled` when a row's `Name` column is `AuditLoader`. Find the column by the
    header the fake and the real FE return, not by a guessed index; the live test in Task 5
    pins it;
  - `NotShipped` when there is no such row and `fe_image.map_or(true, |i| !is_patched_starrocks_fe(i))`;
  - `Install` otherwise.
- **Call site 1:** in `spawn_starrocks_catalog_wire`, after `converge_starrocks_catalogs` and
  before `if let Some(d) = delegation`, behind an `engine_still_current` checkpoint. The function's
  doc comment lists seven checkpoints now; update its count and list.
- **Factoring:** `wait_for_fe_root_login` is the existing FE-ready and root-login loops, moved
  without behaviour change. The existing bootstrap tests (`bootstrap_stops_…`, the D8 tests
  around line 12000) pass unchanged.
- **Call site 2:**
  - `update_starrocks_engine`: after its apply succeeds, spawn `spawn_query_log_setup` when
    `deleg_changed` is false (when it is true, call site 1 covers it);
  - `security_upgrade`: spawn it after `apply_built` succeeds;
  - neither waits on the handle, and neither fails on a setup failure.
- **Open question 4: decided (b).** Add a boot pass
  `pub async fn setup_query_logs(st: &CpState)` in `query_log.rs`, exported from `lib.rs` and
  called from `crates/egret-bin/src/main.rs` beside `reapply_stale_opa_engines` (about line
  3672). It runs `spawn_query_log_setup` for every provisioned StarRocks engine that is not
  `deleting` or `failed`, and logs a count. With (a) there is nothing to add; with (c), Task 6
  adds the list badge from a `queryLogReady` field instead.
- **Protected keys:** add `plugin_enable` and `plugin_dir` to `SR_PROTECTED_FE_KEYS`, and extend
  the existing guard test (`validate_sr_fe_config`'s, and the provisioner's merge test near
  `starrocks.rs:2175`).
- **Unit tests:**
  - `query_log_setup_sql`, unbound: four statements, ending with the `GRANT INSERT`. Bound:
    three, no `GRANT`;
  - a password `p'\x` appears only as `quote_string`'s output;
  - `plugin_step`: listed → `AlreadyInstalled` even on an unpatched image; not listed + egret.4
    → `Install`; not listed + official 4.1.6 → `NotShipped`; not listed + no image →
    `NotShipped`.
- **Tests with `FakeStarRocksDdl`:**
  - a full run executes the statements in order, then `SHOW PLUGINS`, then `INSTALL`;
  - a second run with the fake answering `AuditLoader` executes no `INSTALL`;
  - `NotShipped` executes no `INSTALL` but still the table and user;
  - an FE error comes back `Failed` with the password scrubbed (use `fail_matching`);
  - the bootstrap with a deleted engine stops before the query-log step.
- **Mutants:**
  - always install: the `NotShipped` test fails;
  - grant when bound: the bound test fails;
  - skip the checkpoint: the deleted-engine test fails;
  - remove `plugin_enable` from the keys: the guard test fails.

### Task 4: The routes, the read SQL and the redaction

**Open question 1: decided (b).** That is written here: a new permission
`engine:audit`. With (a), `statement_visible` returns `true`, and the permission row and its
migration are dropped. With (c), it returns `command != "MySQL.COM_STMT_EXECUTE"`.

**Files:**
- `crates/egret-engine-mgmt/src/query_log.rs` (routes, DTOs, SQL builders, redactor)
- `crates/egret-engine-mgmt/src/engines.rs` (`.merge(crate::query_log::routes())` beside
  `crate::monitor::routes()`)
- `crates/egret-engine-mgmt/src/lib.rs` (`openapi()`: `doc.merge(query_log::QueryLogApi::openapi())`)
- `crates/egret-core/src/rbac.rs` (`PERMISSIONS`: `engine:audit`, group "Engines", "Read
  statement text in engine query logs", `Scope::Platform`). Migration 0218 grants it to every
  platform role that holds `engine:update`; follow how the last permission added by migration
  was granted (grep `migrations/` for an `INSERT INTO` on the role-permission table).
- `openapi.json` (regenerated the way the repo does it)

**Produces:**

```rust
#[derive(Serialize, ToSchema)] #[serde(rename_all = "camelCase")]
pub struct QueryLogStatusDto { pub state: QueryLogState, pub message: Option<String>,
    pub last_event_at: Option<String>, pub retention_days: u32 }
#[derive(Serialize, ToSchema)] #[serde(rename_all = "camelCase")]
pub enum QueryLogState { Ready, NotSetUp, PluginNotShipped, Unreachable }
#[derive(Serialize, ToSchema)] #[serde(rename_all = "camelCase")]
pub struct QueryLogEntryDto { /* the spec's section 5 fields */ }
#[derive(Serialize, ToSchema)] #[serde(rename_all = "camelCase")]
pub struct QueryLogPageDto { pub entries: Vec<QueryLogEntryDto>, pub next_cursor: Option<String> }

pub(crate) struct QueryLogFilter { from: DateTime<Utc>, to: DateTime<Utc>, user: Option<String>,
    state: StateFilter, limit: u32, before: Option<(NaiveDateTime, Uuid)> }
pub(crate) fn parse_filter(q: &QueryLogParams, now: DateTime<Utc>) -> Result<QueryLogFilter, CpError>;
pub(crate) fn list_sql(f: &QueryLogFilter) -> String;
pub(crate) fn authorized_name(raw: &str) -> Option<String>;   // "'bob'@'%'" -> "bob"
pub(crate) fn redact_statement(text: &str, exact: &[&str]) -> String;
pub(crate) fn encode_cursor(ts: &NaiveDateTime, id: &Uuid) -> String;
pub(crate) fn decode_cursor(c: &str) -> Result<(NaiveDateTime, Uuid), CpError>;
pub(crate) async fn statement_visible(st: &CpState, p: &Principal) -> bool;

pub fn routes() -> Router<CpState>; // the four routes below
pub struct QueryLogApi;             // utoipa fragment
```

**Routes:**
- `GET  /engines/{id}/query-log/status` (`engine:read`)
- `GET  /engines/{id}/query-log` (`engine:read`)
- `GET  /engines/{id}/query-log/{queryId}` (`engine:read`)
- `POST /engines/{id}/query-log/setup` (`engine:update`); calls `ensure_query_log(…, false)` and
  answers `QueryLogStatusDto`

**Requirements:**
- **Loading the engine:** like `monitor::load_engine`, plus refusals in this order: 404,
  `refuse_if_external` (409 `engine_is_external`), 409 for a non-StarRocks engine, then (setup
  only) 409 while `DELETING`.
- **Setup answers:** `Ready` and `PluginNotShipped` are a 200 with the status; `Failed(msg)` is
  a 502 with the scrubbed message.
- **List and detail without the table:** a 409 with code `query_log_not_ready` and the state;
  other FE failures are a 502 with the scrubbed message.
- **Status:** `SHOW PLUGINS`, `SHOW TABLES FROM starrocks_audit_db__` (an unknown-database
  error means no table), and `SELECT max(timestamp)` over the last day. The state follows the
  spec's section 5. A connect or read failure is `Unreachable` with the scrubbed message, a 200
  (the tab renders it), not a 502.
- **List SQL:**
  - it starts with `/* egret-query-log-read */ SELECT /*+ SET_VAR(query_timeout = 10) */`;
  - it filters on `timestamp >= '<from>' AND timestamp < '<to>'`, formatted by egret in the FE's
    zone as measured in Task 5 (UTC until then);
  - `user`: `` (lower(`user`) = <q> OR lower(regexp_extract(authorizedUser, '^''([^'']*)''@', 1)) = <q>) ``,
    with `<q>` = `quote_string(lowercased value)`;
  - the state filter;
  - the cursor: `(timestamp, queryId) < (<ts>, <id>)` written as the expanded `OR` form;
  - `NOT (`user` = 'root' AND stmt LIKE '/* egret-query-log-read */%')`;
  - `ORDER BY timestamp DESC, queryId DESC LIMIT <limit + 1>`; the extra row decides
    `nextCursor`;
  - it selects `left(stmt, 2000)` and `length(stmt) > 2000`. The detail route selects `stmt`
    whole.
- **Validation:** `from < to`; `to - from <= 31 days`; `limit` within 1 to 200; `state` within
  `ok`, `error`, `all`; `user` at most 128 characters; `queryId` parses as a UUID. Each failure is
  a 400 naming the parameter.
- **Statement fields:** when `!statement_visible`, `statement` is absent and `statementTruncated`
  is false; every other field is unchanged.
- **Redaction:** `redact_statement` runs on `statement` and `errorMessage` with
  `exact = [root password, loader password]` (resolved once per request), then the shape rules of
  the spec's section 5. Never return unredacted text, including from the detail route.
- **utoipa:** every status, and the fact that `Unreachable` is a 200.
- **Unit tests:**
  - `parse_filter`: defaults; each 400;
  - `list_sql` for a user with `'` and `\` (escaped), each state, with and without a cursor, and
    the self-exclusion clause present;
  - `authorized_name` for `'bob'@'%'`, `'o''brien'@'10.%'`, an empty string, and garbage;
  - the cursor round-trip and a tampered cursor (400);
  - `redact_statement`: `IDENTIFIED BY 'x y'`, `"password" = "a b"`, `'secret_key'='k'`, the
    exact root password inside an unrelated string, and a statement with nothing to redact
    (unchanged).
- **DB tests (routes, `FakeStarRocksDdl` answering via `answer_query`):**
  - 403 without `engine:read`; 403 on setup without `engine:update`;
  - external → 409 `engine_is_external`; Trino → 409; unknown → 404;
  - a page of two rows with `limit=1` gives one entry and a cursor;
  - without `engine:audit`, `statement` is absent;
  - the fake's FE error comes back scrubbed (`Unreachable` on status, 502 on list).
- **Mutants:**
  - drop the self-exclusion clause: the SQL test fails;
  - skip `quote_string` for `user`: the escaping test fails;
  - return the statement regardless of `statement_visible`: the permission test fails;
  - drop the exact-value redaction: the root password test fails.

### Task 5: Against a real StarRocks, and the four measurements (controller)

Start your own `ghcr.io/egret-data/egret-starrocks-allin1:4.1.7-egret.4` container, in your own
compose project or a `docker run` with a name you choose, and later the official
`starrocks/allin1-ubuntu:4.1.7`. Stop only what you started.

1. **Add `crates/egret-engine-mgmt/tests/query_log_live.rs`,** gated on
   `EGRET_TEST_STARROCKS_URL` like `starrocks_ddl_live.rs` and using `MysqlDdlRunner`:
   - `ensure_query_log(.., wait_for_events = true)` gives `Ready`; a second call executes no
     `INSTALL`;
   - a statement by a second user, then `EXECUTE AS` that user through a third, appears through
     `list_sql` with the right `authorizedUser`;
   - egret's own list read does not appear;
   - against the official image: `PluginNotShipped`, and the table and user exist.

   Run it with the variable set, and report its duration (a skipped run finishes in
   milliseconds).
2. **Measure and record** in the scratchpad and the PR:
   - (fact 2) the stored `stmt` of the `INSTALL PLUGIN` and of `CREATE USER … IDENTIFIED BY`:
     is the password in it? If it is, it is a finding: tell the user. The exact-value redaction
     already hides it from the API;
   - (fact 3) the FE's error for `INSTALL PLUGIN FROM "/nonexistent.zip"`, which gets pinned in
     Task 3's `Failed` test;
   - (fact 4) a probe's `timestamp` against UTC now; set the list SQL's zone to match;
   - the `SHOW PLUGINS` header, which gets pinned in `plugin_step`'s test.
3. **Fork e2e for the guard:** render `query_log_loader_policy` and `query_log_guard_bodies`
   for service `e2e` in a scratch program, add them in place of `audit-loader` in a scratch copy
   of the fork's `tests/e2e/policies.py`, and run the fork's `run.sh` against egret.4. Do not
   commit to the fork. Expected:
   - `eve` and a user with an all-access allow get `SELECT` and `DROP TABLE` on
     `starrocks_audit_tbl__` denied;
   - the loader still loads (the marker check passes).

### Task 6: Frontend, the Query log tab

**Files:**
- `frontend/src/api/generated/**` (`npm run gen:api` after Task 4's `openapi.json`)
- `frontend/src/routes/control-plane/EngineDetail.tsx` (`KNOWN_TABS`, `EngineTabs`)
- create `frontend/src/routes/control-plane/QueryLogTab.tsx`
- `frontend/src/routes/control-plane/queryKeys.ts` if a hand-written key is needed
- `frontend/src/routes/control-plane/__tests__/QueryLogTab.test.tsx`

**Requirements:**
- **The tab:** `query-log` in `KNOWN_TABS`, and the trigger "Query log" after Monitor, rendered
  only when `engine.type === "starrocks"`. A `?tab=query-log` on a Trino engine falls back to
  `overview`.
- **Status line and callouts:** the spec's section 6 texts. `notSetUp` uses `WarningCallout`
  (`externalEngine.tsx`) with **Set up query log**, enabled by `useCan("engine:update")`. When
  disabled, it carries the reason as text (FE-ACTION-4). On success: invalidate status and list,
  then toast (FE-ACTION-2). On failure: the server's message (FE-ACTION-3).
- **`pluginNotShipped`:** the spec's text. When B's `securityUpgradeNeeded` is set, it names the
  **Upgrade security** action on the same page.
- **Filters:** range, User and State, kept in `?from=&to=&user=&state=` (FE-PAGE-2). Inputs have
  tied `Label`s (FE-FORM-2), and State is a picker (FE-FORM-3).
- **Table:** Time (local, with the UTC instant in `title`), User (`authorizedUser`, falling back
  to `user`), Connected as (shown only when it differs), State (`StatusBadge`), Command,
  Duration, Rows returned, and the statement's first line in monospace, cut with an ellipsis.
  When `statement` is absent, the cell reads "Hidden: needs engine:audit", per open question 1.
- **Side panel:** the detail route's full statement, the error, the counters, `querySource`,
  `clientIp`.
- **Paging:** **Load more** follows `nextCursor`. This is an exception call under FE-DATA-2,
  because it needs a cursor across pages; add the one-line reason comment.
- **States:** `ListStates` (error first), a skeleton while loading, and `EmptyState`. A filter
  miss offers to clear the filters.
- **Vitest:**
  - the tab is present for StarRocks and absent for Trino;
  - each of the four states;
  - the Set up button: enabled with `engine:update`, disabled with its reason without it, and it
    calls the route;
  - the filters write the URL and reach the request;
  - Load more appends;
  - "Connected as" is shown only when it differs;
  - the hidden-statement text.
- **Mutant:** render the tab for every engine type. The Trino test fails.

### Task 7: Live check on kind (controller)

Run this on a kind cluster that egret provisions into and that you created; follow memory
`egret-deploy-kind`.
1. Create a StarRocks engine (default fork image, Ranger on, delegation impersonate). Within
   about three minutes of Ready, the Query log tab is `ready`.
2. Run a query through egret as a normal user. It shows that user as User and the delegator as
   Connected as.
3. Run a query the user's profile denies. It shows Failed, with the "Access denied" text.
4. As an engine admin, connect with a MySQL client and `SELECT` from
   `starrocks_audit_db__.starrocks_audit_tbl__`. It is denied (per open question 2).
5. **Fact 1:** delete the FE pod. Once it is back, check status:
   - still `ready`: record it;
   - `notSetUp`: press **Set up query log**, record that the plugin is lost on restart, and
     stop to tell the user before building the `plugin_dir` move (spec section 7).
6. Create a legacy engine (official 4.1.6 images): the tab is `pluginNotShipped`. Press B's
   **Upgrade security**; within a few minutes after the FE is back, the tab is `ready`.
7. Take the docs screenshots: the tab with rows, the side panel, `notSetUp`, and
   `pluginNotShipped`.

Record the commands and outputs in the scratchpad, and their summary in the PR.

### Task 8: Docs (in `egret-data/docs`)

Branch `docs/starrocks-query-log`. Each item goes on its existing page; find them with grep.
- **Engine detail:** the Query log tab, its four states, the columns, "User" and "Connected as",
  the 30-day retention, **Set up query log** (`engine:update`).
- **Security:**
  - the audit database, the `egret_query_log` loader user and the Ranger guard rows;
  - that statements can show parameter values and mask expressions, who may read them (per open
    question 1), and what egret redacts;
  - the `plugin_enable` and `plugin_dir` keys are now refused in `feConfig`.
- **Permissions:** `engine:audit` (open question 1, decided (b)).
- **Upgrade notes:** per open question 4. Engines on images before egret.4 need **Upgrade
  security** first.
- **REST API:** the four routes, their parameters, answers and statuses.
- **Screenshots:** from Task 7.
- Every code fence has a language, and `npm run build` in the docs repo passes.