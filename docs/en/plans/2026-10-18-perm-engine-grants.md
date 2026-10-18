# Engine-scoped grants (F11) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Engine-level permissions (impersonate a named user, set a named session property, execute a named
procedure or function on Trino; impersonate, use a global function, manage blacklists on StarRocks) become real,
allow-only grants in egret's model: stored in `perm.engine_grants`, compiled by `egret-perm-compile` into the
engine's Ranger profile as `perm-engine-{uuid}`, listed per engine with the seeded baselines as managed entries,
and explainable.

**Architecture:** Model and store in `egret-perm` (`engine_grants.rs`); the curated catalogue, compile, routes and
explain in `egret-perm-compile` (`engine_capabilities.rs`, `engine_grants.rs`, `routes/engine_grants.rs`); one new
prefix in `egret-policy`; reference readers extended in `egret-perm-compile` and `egret-auth`; former-profile
recompiles in `egret-engine-mgmt`; a differential case set in `egret-ranger-eval`.

**Tech Stack:** Rust (axum, sqlx, utoipa), Postgres, `egret-ranger-eval` + the real Ranger jars (differential).

**Spec:** `docs/en/specs/2026-10-18-perm-engine-grants-design.md` (corrections C1–C12, decisions E1–E21) on the
programme spec `docs/en/specs/2026-10-16-dg-frontend-redesign-design.md` (F11, F12). Read the spec's "Corrections"
table before any task: the brief's `queryid`, `function`, `resource_group` and `operate` premises are false.

## Global Constraints

- Worktree `/Users/1pro-l4r8/Documents/github/data-portal/egret-f11`, branch `feat/perm-engine-grants`, base main
  `4a67321b`. Never read or touch `data-portal/egret` (stale checkout). `rg` is not installed: use `grep -rn`.
- Migration number **0195** is reserved for this package (#3 Overview uses 0196+). Before the final gate,
  `git fetch && git ls-tree origin/main migrations/ | grep 0195` must be empty; if not, renumber and re-run
  `cargo test -p egret-metastore` (the filename gate, `crates/egret-metastore/src/lib.rs:173`).
- Build and test with a **private** target dir and **private** services, never shared with another package:
  - `export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-f11/target`
  - Postgres: `docker run -d --name f11-pg -e POSTGRES_USER=egret -e POSTGRES_PASSWORD=egret -e POSTGRES_DB=egret_test -p 55495:5432 postgres:16`
    then `export DATABASE_URL=postgres://egret:egret@localhost:55495/egret_test`.
  - Valkey: `docker run -d --name f11-valkey -p 56395:6379 valkey/valkey:8` then
    `export EGRET_TEST_SHARED_STORE_URL=redis://localhost:56395`.
  - `EGRET_ALLOW_DB_SKIP` must be unset (`env -u EGRET_ALLOW_DB_SKIP`): a DB-gated test that skips reports `ok`
    having asserted nothing.
  - If a port is taken pick another free one and say so in the task report. Never start, stop or modify
    `egret-dev-*` containers. Kill only your own PIDs. Keep ≥ 30 GiB free disk (`df -h .`) before a workspace build.
- Per task, before committing: `cargo fmt --all -- --check`, `cargo clippy -p <every crate touched> --all-targets -- -D warnings`,
  and the task's own test commands. One task = one commit.
- Commits: Conventional Commits, identity `hoangquocvietbk2.13@gmail.com`, trailer
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Do not push. Never `git stash`, `git checkout -- <file>`,
  `git clean`. Never commit `.superpowers/` or scratchpad files.
- Never widen an existing route's permission. New reads `policy:read`, writes `policy:update`, through
  `egret_core::authz::require` (`crates/egret-perm-compile/src/routes.rs:1502`).
- Never send groups or roles to `egret-ranger-eval` (`AccessRequest.groups/roles` stay empty, `routes.rs:2264-2271`).
- Lock order for every write: group locks, then the engine row `FOR SHARE`, then profile locks ascending
  (`lock_services_in_tx`, `crates/egret-perm-compile/src/trigger.rs:1833`), then model rows (spec §5.3).
- Red tests first: each task's Step 1 tests must fail for the stated reason before Step 3 code is written; paste the
  failing output summary into the task report.
- DB tests scope every query to their own fixture rows (own service ids, own engine ids, unique names): tests in one
  binary share a database (`crates/egret-perm-compile/tests/managed_by_agreement.rs:13-18`).

## File structure

- Create `migrations/0195_perm_engine_grants.sql`.
- `crates/egret-perm/src/engine_grants.rs` (model + store), `lib.rs` exports; test `crates/egret-perm/tests/engine_grants_store.rs`.
- `crates/egret-core/src/engine_principals.rs`: `DELEGATOR_PRINCIPAL`; `crates/egret-engine-mgmt/src/delegator_policy.rs:15` re-exports it.
- `crates/egret-policy/src/store.rs`, `lib.rs`: `COMPILER_ENGINE_GRANT_PREFIX` and its four sites.
- `crates/egret-perm-compile/src/engine_capabilities.rs` (catalogue), `engine_grants.rs` (compile, managed mapping,
  audit hook), `routes/engine_grants.rs` (routes, explain), edits in `trigger.rs`, `members.rs`, `principal_refs.rs`,
  `attribution.rs`, `routes.rs`, `lib.rs`.
- Tests in `crates/egret-perm-compile/tests/`: `engine_grants_compile.rs`, `engine_grants_references.rs`,
  `engine_grants_routes.rs`, `engine_grants_explain.rs`, `engine_grant_prefix_rename.rs`.
- `crates/egret-auth/src/groups.rs` (refusal text).
- `crates/egret-engine-mgmt/src/engines.rs` (former-profile recompiles); tests in `crates/egret-engine-mgmt/tests/routes.rs`.
- `crates/egret-ranger-eval/tests/differential.rs` (new fixed-case test).
- `openapi.json`, `frontend/src/api/generated/**`, `frontend/src/components/perm/ExplainResult.tsx` (one additive
  branch) + its test.

---

### Task 1: migration 0195 and the store

**Files:** create `migrations/0195_perm_engine_grants.sql`, `crates/egret-perm/src/engine_grants.rs`,
`crates/egret-perm/tests/engine_grants_store.rs`, `crates/egret-perm-compile/tests/engine_grant_prefix_rename.rs`;
modify `crates/egret-perm/src/lib.rs`.

**Interfaces (produces):**

```rust
// crates/egret-perm/src/engine_grants.rs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EngineGrant {
    pub id: Uuid,
    pub engine_id: Uuid,
    pub capability: String,
    pub values: Vec<String>,
    /// `exclude` is always empty (allow-only, include-only, spec E5).
    pub principals: PrincipalSet,
}
pub async fn list_engine_grants_via(conn: &mut PgConnection) -> Result<Vec<EngineGrant>, PermError>;
pub async fn list_engine_grants_for_profile_via(conn: &mut PgConnection, service_id: i64) -> Result<Vec<EngineGrant>, PermError>;
pub async fn list_engine_grants_for_engine_via(conn: &mut PgConnection, engine_id: Uuid) -> Result<Vec<EngineGrant>, PermError>;
pub async fn get_engine_grant_via(conn: &mut PgConnection, id: Uuid) -> Result<Option<EngineGrant>, PermError>;
pub async fn create_engine_grant_in_tx(tx: &mut Transaction<'_, Postgres>, g: &EngineGrant) -> Result<Uuid, PermError>;
pub async fn update_engine_grant_in_tx(tx: &mut Transaction<'_, Postgres>, g: &EngineGrant) -> Result<bool, PermError>;
pub async fn delete_engine_grant_in_tx(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> Result<bool, PermError>;
```

Lists are ordered by `(engine_id, capability, resource_values, id)`; principals by `(kind, ref)`.

- [ ] **Step 1: failing tests.**
  - `engine_grants_store.rs` (DB): create a Trino engine row by SQL (`INSERT INTO engine_mgmt.engines (name, type,
    namespace, spec) … RETURNING id`, as `crates/egret-perm-compile/src/lineage_compile_tests.rs:106` does) and a
    grant `trino.set_system_session_property ["query_max_memory"]` with principals `user:alice`, `group:g1`:
    round-trips through `get_engine_grant_via` equal to what was written; update replaces values and principals;
    delete returns `true` then `false`; deleting the engine row cascades (the grant and its principals are gone).
  - CHECKs, each an insert by SQL that must fail with `23514` (check violation): capability `trino.kill_any`;
    value `*`; value `a?b`; value `{USER}`; `trino.set_catalog_session_property` with 1 value;
    `starrocks.manage_blacklist` with 1 value; a NULL element; principal kind `workspace_members`; principal
    `group:public`; principal ref `''`. A duplicate `(engine_id, capability, resource_values)` fails with `23505`.
  - `engine_grant_prefix_rename.rs` (DB, in `egret-perm-compile` because it seeds a service): the rename statement of
    0195 is extracted with `include_str!` exactly as `managed_by_agreement.rs:28-46` extracts 0180's, scoped to the
    fixture's service ids, and run over rows `perm-engine-x` (`managed_by='operator'`), `perm-engine-y`
    (`managed_by='compiler'`), `perm-grant-z`: afterwards `perm-engine-x` is named
    `renamed by 0195: perm-engine-x` in the column AND in `policy_jsonb->>'name'`, its `version` went up by 1, its
    service's `policy_version` went up by 1; the other two rows are byte-identical; the extractor found exactly one
    `UPDATE policy.authz_policies` statement (guard against measuring nothing).
- [ ] **Step 2:** run, see them fail (no table / no statement):
  `env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm --test engine_grants_store` and
  `env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test engine_grant_prefix_rename`.
- [ ] **Step 3: code.** The SQL of spec §3.1 verbatim; then the rename (spec §3.3) as ONE `UPDATE … RETURNING
  service_id` inside a CTE that also bumps `policy.authz_services.policy_version` for the returned ids, plus a
  `DO $$ … RAISE NOTICE` with the count. Keep every comment on its own line (the extractor strips only whole-line
  `--` comments, `migrations/0180_policy_managed_by.sql:52-56`). The store module mirrors `list_grants_via` /
  `create_grant_in_tx` (`crates/egret-perm/src/store.rs:252`, `:366`); principals are written in a second statement
  per row, as `perm.grant_principals` is.
- [ ] **Step 4:** run Step 2's commands green, plus `cargo test -p egret-metastore` (both migration gates).
- [ ] **Step 5:** commit `feat(perm): engine grant tables and store (migration 0195)`.

---

### Task 2: the `perm-engine-` prefix and attribution

**Files:** `crates/egret-policy/src/store.rs`, `crates/egret-policy/src/lib.rs`,
`crates/egret-perm-compile/src/trigger.rs` (the existing-state query only), `crates/egret-perm-compile/src/attribution.rs`,
`crates/egret-perm-compile/src/routes.rs` (`ExplainAttribution`, `attribution_for`); tests in the same files' test
modules and `crates/egret-policy/tests/generated_profile_guard.rs`.

- [ ] **Step 1: failing tests.**
  - `is_compiler_owned_policy_name("perm-engine-0f…")` is `true`; `"perm-engines"` and `"perm-engine"` (no dash)
    are `false`.
  - `refuse_reserved_name("perm-engine-x")` is `Err(CompilerOwned)` and its message names `"perm-engine-"`.
  - DB (`generated_profile_guard.rs`): a profile holding one `perm-engine-<uuid>` row refuses `delete_service` with
    `CompilerOwned`; after the row is removed it deletes.
  - `attribute_policy_name("perm-engine-<uuid>") == Some(PolicyAttribution::EngineGrant(uuid))`;
    `"perm-engine-<uuid>-x"` is `None` (an engine grant never carries a locator).
  - `attribution_for` maps it to `ExplainAttribution::EngineGrant { engine_grant_id }`, serialised
    `{"kind":"engine_grant","engine_grant_id":"…"}`.
  - A sweep check (DB, `crates/egret-perm-compile/tests/recompile.rs`): a hand-inserted `perm-engine-<random uuid>`
    row with `managed_by='compiler'` in a generated profile is reaped by `recompile_service` (no engine grant
    produces it) — proves the prefix is in `apply_diff_in_tx`'s query.
- [ ] **Step 2:** `cargo test -p egret-policy --lib`, `env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-policy --test generated_profile_guard`,
  `cargo test -p egret-perm-compile --lib attribution`, `env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test recompile` → red.
- [ ] **Step 3: code.** Spec §4.3 items 1–5. Then `grep -rn "COMPILER_CLEARANCE_PREFIX" crates --include='*.rs' | grep -v tests/`
  and confirm every production site lists the new prefix too (paste the grep in the report).
- [ ] **Step 4:** Step 2 green; `env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test managed_by_agreement`
  still green (0180's fixture is unchanged and must stay so).
- [ ] **Step 5:** commit `feat(policy): reserve the perm-engine- prefix for compiled engine grants`.

---

### Task 3: the capability catalogue

**Files:** create `crates/egret-perm-compile/src/engine_capabilities.rs`; modify `crates/egret-perm-compile/src/lib.rs`,
`crates/egret-core/src/engine_principals.rs`, `crates/egret-engine-mgmt/src/delegator_policy.rs`.

**Interfaces (produces):**

```rust
pub enum ValueKind { Username, Identifier, Catalog, FunctionSignature }
pub struct Capability {
    pub id: &'static str,                 // "trino.impersonate_user", …
    pub engine: EngineType,
    pub label: &'static str,
    pub description: &'static str,
    pub resources: &'static [(&'static str, Option<ValueKind>)], // None = fixed "*"
    pub access: &'static str,
}
pub struct NotGrantable { pub engine: EngineType, pub what: &'static str, pub why: &'static str }
pub const CAPABILITIES: &[Capability];      // the 8 rows of spec §2, in that order
pub const NOT_GRANTABLE: &[NotGrantable];   // spec §2 "Not offered"
pub fn capability(id: &str) -> Option<&'static Capability>;
/// Shape rules of spec §2 (no DB): count, non-empty, ≤255, no surrounding
/// whitespace, none of `*?{}`, the per-kind pattern; returns the normalised
/// values (lower-cased except FunctionSignature).
pub fn check_values(cap: &Capability, values: &[String]) -> Result<Vec<String>, String>;
// egret_core::engine_principals
pub const DELEGATOR_PRINCIPAL: &str = "egret_delegator";
```

- [ ] **Step 1: failing tests** (pure, in the module):
  - for each of `CAPABILITIES`: every resource key is a `name` in its engine's service-def
    (`egret_policy::trino_service_def()` / `starrocks_service_def()`), the keys form one parent chain in order
    (each key's `parent` is the previous key, the first has none); `access` is in `accessTypes`; `access` is in
    the leaf resource's `accessTypeRestrictions` for every entry except `trino.execute_function`, and that one is
    asserted to be the ONLY exception and its leaf to be `schemafunction` (spec C2);
  - near-root absence: no entry's keys include `role`, `sysinfo`, `queryid`, `storage_volume`, `resource`,
    `resource_group`, `function` (Trino level 10, never read by the plugin); the only `system` entry has access
    `blacklist`; every `NOT_GRANTABLE.what` names a key or `key access` pair the defs declare;
  - the ids equal, as a set, the list in `migrations/0195_perm_engine_grants.sql`'s capability `CHECK` (parse the
    `include_str!` text between `capability IN (` and `)`);
  - `check_values`: `trino.impersonate_user` refuses `[]`, `["*"]`, `["al*"]`, `["{USER}"]`, `[" bob"]`, `["a","b"]`
    and accepts `["Bob"]` → `["bob"]`; `trino.set_catalog_session_property` refuses `["ice"]`;
    `starrocks.use_global_function` accepts `["my_udf(INT,VARCHAR)"]` unchanged and refuses `["my_udf"]`;
    `starrocks.manage_blacklist` accepts `[]` only;
  - `DELEGATOR_PRINCIPAL == "egret_delegator"` and `delegator_policy::DELEGATOR_USERNAME` is the same constant.
- [ ] **Step 2:** `cargo test -p egret-perm-compile --lib engine_capabilities` and `cargo test -p egret-core --lib` → red.
- [ ] **Step 3: code.** Labels and descriptions are the sentences of spec §2 ("Labels state the consequences").
  `delegator_policy.rs:15` becomes `pub use egret_core::engine_principals::DELEGATOR_PRINCIPAL as DELEGATOR_USERNAME;`.
  Do NOT add it to `RESERVED_ENGINE_USERNAMES` (spec E21).
- [ ] **Step 4:** Step 2 green; `cargo test -p egret-engine-mgmt --lib delegator_policy`.
- [ ] **Step 5:** commit `feat(perm): curated engine capability catalogue`.

---

### Task 4: compile engine grants

**Files:** create `crates/egret-perm-compile/src/engine_grants.rs`, `tests/engine_grants_compile.rs`; modify
`trigger.rs` (`compile_desired`, `SourceKind`, `record_compiled`), `members.rs` (`expand_enumerated_principals`,
`referenced_principals`, `premint_model_usernames`), `lib.rs`.

**Interfaces (produces):**

```rust
pub fn compile_engine_grant(g: &EngineGrant, profile: &str, engine: EngineType)
    -> Result<AccessPolicyInput, CompileError>; // name "perm-engine-{id}"
pub const fn engine_grant_policy_name(id: Uuid) -> String; // or fn; one spelling
/// The checks a target must pass at compile (spec E11); `Err(reason)` = skip.
pub(crate) async fn impersonation_target_ok(conn, service_id: i64, target: &str) -> Result<Result<(), String>, CompileError>;
// members.rs
pub(crate) async fn expand_enumerated_principals(conn, grants: &mut [Grant], rules: &mut [Rule],
    engine_grants: &mut [EngineGrant]) -> Result<(), CompileError>;
// trigger.rs
pub enum SourceKind { Grant, Rule, Clearance, EngineGrant }
```

- [ ] **Step 1: failing tests.**
  - Pure: for each capability, `access_to_ranger_json(compile_engine_grant(..))` has `name = perm-engine-<id>`,
    `resources` exactly the catalogue keys with `{values:[v], isExcludes:false, isRecursive:false}`
    (`system: ["*"]` for blacklist), one `policyItems` entry with the users and `[{type: access, isAllowed: true}]`,
    `delegateAdmin: false`, no deny items, `serviceType` stamped; a StarRocks capability on a Trino profile is
    refused; a `User` ref `a@b` is refused (`UnusableUserRef`); `egret-lineage` refused (`ReservedEnginePrincipal`).
  - DB (`engine_grants_compile.rs`; fixture: Trino profile + its tag service + engine bound to it, StarRocks profile
    + engine, users `ua` (username), group `g1 = {ua, ub(no username)}`, role `r1` held by `ud`):
    1. grant `set_system_session_property ["query_max_memory"]` to `user:ua`; `recompile_service`: exactly one
       `perm-engine-<id>` row in the Trino profile, `managed_by='compiler'`, profile `origin='generated'`,
       `policy_version` bumped once; recompiling again writes nothing (`summary.wrote_anything() == false`);
    2. delete the grant; recompile reaps the row;
    3. group grant: the item's users are `[ua, <minted ub>]`, `ub` now has a username;
    4. role grant: users `[ud]`;
    5. a group with no members → `skipped` with `SourceKind::EngineGrant` and no row;
    6. impersonation grant targeting `uc`; delete user `uc`'s username by SQL; recompile → skipped with reason
       "no user carries" and the row reaped; target named in the profile's `egret-admin - trinouser` policy users
       (seed with `PolicyStore::seed_admin_all_access`) → skipped with reason "engine admin";
    7. ceiling: with `PROFILE_EXPANSION_CEILING` policies already compiled (insert by the existing per-column helper
       in `tests/recompile.rs`, or lower the count by fixture), one more engine grant is refused with
       `ProfileExpansionCeiling`;
    8. no `perm.compiled` row is written for an engine grant;
    9. a grant on an engine bound to profile P does not appear in any other profile.
  - DB: `premint_model_usernames` mints a username for a member of a group named only by an engine grant.
- [ ] **Step 2:** `cargo test -p egret-perm-compile --lib engine_grants`,
  `env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test engine_grants_compile` → red.
- [ ] **Step 3: code.** Spec §4.2. `compile_desired` reads `list_engine_grants_for_profile_via` right after
  `list_rules_via` (`trigger.rs:437-438`) and passes them to the one expansion call (`trigger.rs:445`); the compile
  loop follows the grants loop (`trigger.rs:450-510`) with the same empty-include skip; `record_compiled` returns
  early for `EngineGrant` as it does for `Clearance` (`trigger.rs:1274`). Update every other caller of
  `expand_enumerated_principals` (`grep -rn expand_enumerated_principals crates`) to pass `&mut []`.
- [ ] **Step 4:** Step 2 green; `env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile` (whole crate: the
  expansion signature change touches every compile test).
- [ ] **Step 5:** commit `feat(perm-compile): compile engine grants into the engine's profile`.

---

### Task 5: engine grants count as references

**Files:** `crates/egret-perm-compile/src/trigger.rs` (`group_references_via`, `affected_services_for_group_membership`,
`affected_services_for_role_membership`), `principal_refs.rs` (`unresolved_principals_via`,
`stored_user_refs_naming_via`), `crates/egret-auth/src/groups.rs` (doc + `referenced_conflict`); test
`crates/egret-perm-compile/tests/engine_grants_references.rs`, and one case in `crates/egret-auth` unit tests for the text.

- [ ] **Step 1: failing tests** (DB unless stated):
  - `group_references_via(["g1"])` returns a row `{source:"engine_grant", id:<grant>, effect:"allow",
    position:"include"}` for an engine grant naming `g1`;
  - deleting local group `g1` through `GroupRepo::delete_local` (with the real `PermGroupRecompiler`) is refused
    with a conflict whose text contains `engine grant <id> (allow, include)`; renaming it likewise; after the engine
    grant is deleted, both succeed (the #692 tests in `tests/group_races.rs` show the fixture);
  - pure (`egret-auth`): `referenced_conflict` formats an `engine_grant` reference as above;
  - adding a member to `g1` recompiles the engine's profile (the new member appears in the compiled item) — through
    `affected_services_for_group_membership`; same for granting `r1` to a user (role membership path);
  - `/perm/unresolved-principals` lists an engine grant whose `user` principal was renamed away by SQL, with
    `source:"engine_grant"`, `failsOpen:false`;
  - `POST /perm/people/users/{id}/username` answers 409 when an engine grant's impersonation value equals the name
    the user would be allocated (`source:"engine_grant_target"`), and when an engine-grant `user` principal does.
- [ ] **Step 2:** `env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test engine_grants_references`,
  `cargo test -p egret-auth --lib groups` → red.
- [ ] **Step 3: code.** Spec §4.5 table. In the two affected-set functions, the engine arm is
  `SELECT DISTINCT e.authz_service_id FROM perm.engine_grant_principals p JOIN perm.engine_grants g ON g.id =
  p.engine_grant_id JOIN engine_mgmt.engines e ON e.id = g.engine_id WHERE p.kind = '<group|role>' AND p.ref = …
  AND e.authz_service_id IS NOT NULL`, merged before `normalize_service_order`.
- [ ] **Step 4:** Step 2 green; `env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test group_races --test unresolved_principals --test allocate_username --test group_grants --test role_grants`.
- [ ] **Step 5:** commit `feat(perm): engine grants are group, role and username references`.

---

### Task 6: routes (catalogue, listings, writes)

**Files:** create `crates/egret-perm-compile/src/routes/engine_grants.rs`, `tests/engine_grants_routes.rs`; modify
`routes.rs` (mount, `PermApi` paths and schemas), `engine_grants.rs` (managed mapping, reach, audit hook).

**Interfaces (produces):** the DTOs of spec §5.1–§5.3 (`EngineCapabilitiesResponse`, `CapabilityView`,
`NotGrantableView`, `EngineGrantsResponse`, `EngineRef`, `ProfileRef`, `Reach`, `EngineGrantView`, `ManagedEntry`,
`ManagedKind`, `NewEngineGrant`, `EngineGrantWriteResponse`), and

```rust
pub(crate) async fn write_engine_grant(tx, engine_id: Uuid, id: Option<Uuid>, body: NewEngineGrant,
    p: &egret_core::Principal) -> Result<EngineGrantOutcome, PermHttpError>; // spec §5.3 steps 1–9
pub(crate) fn audit_engine_grant_write(p: &egret_core::Principal, action: &str, id: Uuid, detail: &serde_json::Value);
pub(crate) fn audit_detail(g: &EngineGrant, services: &[i64]) -> serde_json::Value;
```

- [ ] **Step 1: failing tests** (`engine_grants_routes.rs`, real `CedarPdp` as `tests/people.rs:131-172`; sessions
  `admin`, `reader` (`policy:read`), `writer` (`policy:read` + `policy:update`), `nobody`):
  - every route: `nobody` 403; `reader` 200 on the three GETs and 403 on POST/PATCH/DELETE; `writer` 201/200;
  - `GET /perm/engine-capabilities`: two engines, 5 Trino + 3 StarRocks ids in catalogue order; `notGrantable`
    non-empty for both and includes `operate`;
  - POST creates and the response's `compile.services == [profile]`, the profile holds `perm-engine-<id>`;
    `reach.engines` is the one engine and `sharedProfile:false`; with a second engine bound to the same profile by SQL
    (legacy share), `reach.engines` lists both, sorted by name, `sharedProfile:true`, and `GET …/grants` of EITHER
    engine shows the grant's reach as both;
  - each 400 of spec §5.5 (unknown capability, Trino capability on StarRocks engine, `*` value, empty principals,
    `workspace_members` kind, `group:public`, a catalog the profile does not reach, an unknown user ref);
    each 409 (engine without profile, `status='deleting'`, `risingwave` engine, target `egret_delegator`, target
    `egret-lineage`, target named in `egret-admin - …`, duplicate grant); 404 unknown engine / unknown grant id;
  - PATCH replaces values and principals and recompiles (old item gone, new present); DELETE reaps;
  - `GET /perm/engines/{id}/grants` on a delegating Trino profile seeded with `user_baseline_policies`,
    `delegator_impersonate_policy` and `seed_admin_all_access` lists `managed` entries: `egret-user-impersonate-self`
    → `{kind:"user_baseline", capability:"trino.impersonate_user", resources:{trinouser:["{USER}"]}}`,
    `egret-user-execute-query` → `capability:null`, `egret-query-timeout` →
    `trino.set_system_session_property`, `egret-delegator-impersonate` → `{kind:"delegator"}`, every
    `egret-admin - …` row → `admin_all_access` with `capability` set only for chains equal to a catalogue key set;
    an engine with no profile → `profile:null`, empty lists, 200;
  - `GET /perm/engine-grants` lists grants of two engines;
  - concurrency (the `tests/support/pausing.rs` pattern): a POST paused after step 5 (engine row `FOR SHARE`)
    blocks a concurrent `UPDATE engine_mgmt.engines SET authz_service_id = …` until it commits;
  - pure: `audit_detail` has exactly `engineId, capability, values, principals, services`.
- [ ] **Step 2:** `env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test engine_grants_routes` → red.
- [ ] **Step 3: code.** Spec §5. Handlers wrap `write_engine_grant` in one transaction like `create_grant`
  (`routes.rs:1531-1547`); delete mirrors `delete_grant` (`routes.rs:1612-1640`): read row, engine `FOR SHARE`,
  group locks are not needed for a delete (it names no new group) but the profile lock is taken before the DELETE.
  Call `audit_engine_grant_write` after `tx.commit()` in all three. Register every path and schema in `PermApi`.
- [ ] **Step 4:** Step 2 green; `env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test wire_dtos --test routes`.
- [ ] **Step 5:** commit `feat(perm): engine grant routes with reach and managed baselines`.

---

### Task 7: explain a capability

**Files:** `crates/egret-perm-compile/src/routes/engine_grants.rs` (handler), `routes.rs` (mount, schemas; reuse
`served_artifact_via`, `profile_plugin_version`, `ranger_engine`, `attribution_for`); test `tests/engine_grants_explain.rs`.

- [ ] **Step 1: failing tests** (DB + real CedarPdp):
  - Trino profile with a `set_system_session_property ["query_max_memory"]` grant to `ua`: `ua` → `decision.kind ==
    "allowed"`, `attribution == {kind:"engine_grant", engine_grant_id}`; `uc` → `denied` (no matching policy);
  - with `user_baseline_policies` seeded: `trino.impersonate_user ["ub"]` for `ua` is `undecidable` and the cause is
    `ResourceValueExpressionMarker` on `trinouser` (spec C9) — even with an engine grant allowing it;
  - StarRocks `use_global_function ["my_udf(INT)"]` grant to `ua`: allowed for `ua`, denied for `uc`;
  - `trino.execute_procedure` with a tag block holding one access policy → `undecidable`, `tagResolution:"declined"`;
    with an empty tag block → decided;
  - an engine with no profile → `outcome:"no_profile"`, `decision:null`;
  - 403 without `policy:read`; 400 for a bad capability or value (same rules as writes).
- [ ] **Step 2:** `env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test engine_grants_explain` → red.
- [ ] **Step 3: code.** Spec §5.4. Build the request with `RequestResource::new().with(key, value)` per catalogue
  key; `resolved_tags` per the spec's tag rule; no groups, no roles, `owner: None`.
- [ ] **Step 4:** Step 2 green; `env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test explain_wire --test explain_principal`.
- [ ] **Step 5:** commit `feat(perm): explain an engine capability on the engine's served artifact`.

---

### Task 8: former profiles are recompiled

**Files:** `crates/egret-engine-mgmt/src/engines.rs` (`update_engine` after `apply_authz_service_update`;
`delete_engine` step 6; `delete_external_engine`); tests in `crates/egret-engine-mgmt/tests/routes.rs` using
`RealProfileCompiler` (`tests/routes.rs:117`).

- [ ] **Step 1: failing tests** (DB):
  - `PUT /engines/{id}` rebinding a provisioned engine from P1 to P2 with an engine grant on it: afterwards P1 holds
    no `perm-engine-<id>` row and P2 holds one (extend the fixture of
    `update_with_authz_service_id_binds_and_recompiles`, `tests/routes.rs:10211`);
  - deleting an engine whose profile is still bound by a second engine (legacy share by SQL): the profile no longer
    holds the deleted engine's `perm-engine-` row; deleting an engine with an unshared profile does NOT call the
    compiler (`RecordingProfileCompiler`, `tests/routes.rs:61`, records no call);
  - the external delete path, same two cases.
- [ ] **Step 2:** `env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-engine-mgmt --test routes former_profile` → red.
- [ ] **Step 3: code.** Spec §6: a helper `recompile_former_profile(st, previous: Option<i64>, only_if_shared: bool)`
  that logs like `restore_engine_authz_service` (`engines.rs:5212-5226`).
- [ ] **Step 4:** Step 2 green; `env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-engine-mgmt --test routes rebind`
  and `… delete` (the existing rebind and delete tests stay green).
- [ ] **Step 5:** commit `fix(engine-mgmt): recompile an engine's former profile on rebind and delete`.

---

### Task 9: the differential on the real jars

**Files:** `crates/egret-ranger-eval/tests/differential.rs` (one new `#[test]`).

- [ ] **Step 1: write the test** `engine_capability_policies_decide_identically_on_the_real_jars`, gated on
  `EGRET_TEST_RANGER_E2E=1` like `:8413`, built with `lv_case` (`:8245`), `load_engine`, `ask_ranger` (`:4848`), at
  2.5.0, 2.8.0 and 2.9.0. Bodies come from `compile_engine_grant` (never hand-built), plus the seeded baselines from
  `egret_engine_mgmt::delegator_policy` where named. Cases, each with the grantee AND a non-grantee:
  - Trino `trinouser` `impersonate`: target = grantee, another user, case variant (`BOB` vs `bob`);
  - Trino `queryid=*` `execute` from `execute_query_policy` (pins C1's baseline);
  - Trino `systemproperty`, `catalog`+`sessionproperty`, `catalog`+`schema`+`procedure` — each `alter`/`execute`;
  - Trino `catalog`+`schema`+`schemafunction` `execute` (C2: restriction list omits it; the jars decide);
  - StarRocks `user` `impersonate`, `global_function` `usage`, `system` `blacklist`, and `system` `operate` asked of
    a blacklist-only grant (must be denied: the control that the access, not the key, decides);
  - StarRocks `resource_group` `alter` from a hand-built policy (the key evaluates; pins C3's reading).
  Assert the Rust decision equals Java's for every case, and assert the expected Java answer for the grantee rows
  (Allowed) and non-grantee rows (not Allowed), so a both-wrong agreement fails.
- [ ] **Step 2:** without the flag the test prints its skip and passes; with
  `EGRET_TEST_RANGER_E2E=1 cargo test -p egret-ranger-eval --test differential -- --exact engine_capability_policies_decide_identically_on_the_real_jars --nocapture`
  run it. A disagreement is a finding: STOP and report it with the case (do not edit the evaluator in this task).
- [ ] **Step 3:** `cargo test -p egret-ranger-eval --test differential -- --list | grep engine_capability_policies`
  shows the test (the `just test-ranger-gates` line that runs the rest of the file includes it; check it is not in a
  `--skip`, `justfile:178`).
- [ ] **Step 4:** commit `test(ranger-eval): engine capability policies on the real Ranger jars`.

---

### Task 10: OpenAPI and the generated client

**Files:** `openapi.json`, `frontend/src/api/generated/**`, `frontend/src/components/perm/ExplainResult.tsx`,
`frontend/src/components/perm/__tests__/ExplainResult.test.tsx`.

- [ ] **Step 1:** `cargo run -p egret-bin -- openapi > openapi.json`; `cd frontend && npm run gen:api` (never bare
  `orval`: it drops `encodeURIComponent`).
- [ ] **Step 2:** `npx tsc -b`. The new `engine_grant` member of the attribution union breaks the narrowing in
  `ExplainResult.tsx:636-700` (after `materialized_view_deny` the code assumes `clearance`). Write a failing vitest
  first: an access attributed `{kind:"engine_grant", engine_grant_id}` renders "Engine grant <id>" and the Ranger
  policy name, and no clearance text. Then add one additive branch before the clearance block (FE-TEST-3; no other
  frontend change, the Engine permissions tab is #4).
- [ ] **Step 3:** `npm run lint`, `npx tsc -b`, `npx vitest run src/components/perm`, `npm run build`.
- [ ] **Step 4:** commit `chore(api): regenerate the client for engine grants`.

---

### Task 11: full gate (controller)

Rebase on fresh `origin/main` first (re-check 0195, regenerate the client after the rebase), then, in this worktree
with the private services and target dir of Global Constraints:

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `env -u EGRET_ALLOW_DB_SKIP -u EGRET_ALLOW_STORE_SKIP DATABASE_URL=postgres://egret:egret@localhost:55495/egret_test EGRET_TEST_SHARED_STORE_URL=redis://localhost:56395 CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-f11/target cargo test --workspace --no-fail-fast 2>&1 | tee <scratchpad>/f11-test.log`;
  report the summed `test result:` lines from the log, not from a `tail` (a truncated read can only acquit)
- [ ] `EGRET_TEST_RANGER_E2E=1 cargo test -p egret-ranger-eval --test differential --no-fail-fast -- --nocapture`
  and `EGRET_TEST_RANGER_E2E=1 cargo test -p egret-policy --test ranger_engine_gate --no-fail-fast` (needs a JVM and
  Maven; a failure to start is reported as "did not run", never as green)
- [ ] `cargo run -p egret-bin -- openapi > openapi.json && cd frontend && npm run gen:api && npx tsc -b && npm run lint`;
  `git status --short openapi.json frontend/src/api` is clean (the committed client is the generated one)
- [ ] Whole-branch review on the most capable model against the spec's decisions and corrections; one fix dispatch
  and a scoped re-review; then tear down `f11-pg` and `f11-valkey`.
