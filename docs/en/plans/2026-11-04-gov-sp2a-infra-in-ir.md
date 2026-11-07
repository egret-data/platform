# Governance sub-project 2a — infrastructure policies in the IR — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** egret's infrastructure policies (`egret-admin - …` on Trino and StarRocks, plus `egret-delegator-impersonate`, `egret-user-impersonate-self`, `egret-user-execute-query` and `egret-query-timeout` on Trino) are derived from the store through a neutral IR lowering. Today five ad-hoc writers upsert them. After this plan, every profile serves the same Ranger JSON as before, minus stale rows that should not exist.

**Architecture:**
- **IR.** A pure lowering `egret_perm_ir::lower::infra::lower_infra(&InfraInputs) -> Vec<Statement>`, with a new `Source::Infra { kind }` and a new `Target::Infra(InfraObject)`.
- **Ranger target.** `egret-perm-compile/src/ranger/infra.rs` renders each statement into exactly today's JSON body, using the builders moved into `egret-policy-api::infra_bodies`.
- **Convergence.** A new `egret-perm-compile/src/infra.rs::converge_infra_in_tx` reconciles a profile's IR-owned infra rows by name: it updates in place (keeping the id), inserts what is missing, and deletes what is extra. It bumps `policy_version` only when something changed.
  - It replaces `sync_admin_policy_in_tx` at the same chokepoint, before every compile, on the compile's transaction.
  - It also runs on its own: the admin kill-switch, the boot pass and engine apply call it without a full compile.
- **What is removed.** The engine-apply seeders, the boot baseline seeder and the egret-admin writer.

**Tech Stack:** Rust workspace crates, sqlx/Postgres integration tests, and the golden compile harness (`crates/egret-perm-compile/tests/golden_compile.rs`).

**Spec:** `docs/en/specs/2026-11-04-gov-sp2-trino-opa.md`, section "2a". Task 7 amends it as decided on 2026-11-04.

## Global Constraints

- **Golden files.** Every existing golden file under `crates/egret-perm-compile/tests/golden/*.json` stays byte-identical in every task. Only Task 6 may add new golden files, and only new ones.
- **The `all - …` family stays out of the IR.** Its route (`POST …/seed-defaults`), `seed_default_trino_policies` and `upsert_infra_policy` are unchanged. Convergence never reads, writes or deletes a row named `all - …`.
- **The IR-owned infra names are exactly two shapes:** the `egret-admin - …` names (`is_admin_all_access_policy_name`), and the four fixed names `egret-delegator-impersonate`, `egret-user-impersonate-self`, `egret-user-execute-query` and `egret-query-timeout`.
- **Convergence deletes every IR-owned infra row that is not desired** (decision 2026-11-04). This covers:
  - a stale delegator grant after impersonate → jwt;
  - rows on a profile no qualifying engine is bound to;
  - `egret-admin - …` rows the service-def no longer builds;
  - `egret-admin` rows when there are no admins.
- **Neither the kill switch nor engine apply depends on a full compile.** Disabling a platform admin removes them from `egret-admin` even when the profile's compile fails (`tests/admin_policy_sync.rs::disabling_an_admin_does_not_depend_on_a_profile_compiling` stays green, unchanged in intent).
- **No schema change and no migration.** No API change: `openapi.json` and `frontend/src/api/generated/**` stay byte-identical. No UI change.
- **`egret-perm-ir` stays pure.** No sqlx, no I/O, no dependency on `egret-policy`, `egret-policy-api` or `egret-ranger-eval`, and no Ranger word in a public type name (`crates/egret-perm-ir/tests/boundary.rs`).
- **Code is moved, not copied.** A body builder exists in exactly one place after the task that moves it. An old writer is deleted no later than Task 5.
- **Lock sites.** No new lock site, beyond what `crates/egret-perm-compile/tests/profile_lock_sites.rs` already lists. Convergence runs under locks its callers already take: the profile row lock plus the mint lock, which is the order `lock_for_admin_sync_in_tx` uses today.
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings` pass on every commit.
- **DB tests.**
  - Use `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` with `EGRET_ALLOW_DB_SKIP` unset (`env -u EGRET_ALLOW_DB_SKIP …`).
  - Run one test suite at a time.
  - Use your own `CARGO_TARGET_DIR` (for example `$PWD/target-sp2a`). Never share one across checkouts.
- Conventional Commits. Each commit ends with the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

---

## Design

Line numbers are against `origin/main` `06331547`.

### D1. Today's writers, and what replaces each

| Today | Where | Replaced by |
|---|---|---|
| `seed_admin_all_access` → `write_admin_all_access_in_tx`, at Trino apply and StarRocks apply | `egret-policy/src/default_policies.rs:173,257`; `egret-engine-mgmt/src/engines.rs:761-778,3857-3871` | `ProfileCompilerPort::converge_infra(service_id)` at apply (Task 5) |
| `sync_admin_policy_in_tx`, the chokepoint before every compile | `egret-perm-compile/src/trigger.rs:1194,1223-1247` | `crate::infra::converge_infra_in_tx` at the same line (Task 4) |
| `set_user_status_and_recompile_in_tx`, the kill switch | `egret-perm-compile/src/role_membership.rs:405-422` | the same function; calls `converge_infra_in_tx` per infra service (Task 4) |
| `sync_admin_policies`, the boot pass and `egret init` | `role_membership.rs:447-480`; `egret-bin/src/main.rs:155,2912` | renamed `converge_infra_policies`; same per-service transaction (Task 4) |
| `seed_delegator_policies` / `seed_user_baseline` at apply | `engines.rs:751-756,781,2562-2612` | `converge_infra` at apply (Task 5) |
| `seed_user_baselines_for_existing_engines` at boot | `engines.rs:2651-2705`; `main.rs:3093` | deleted. `converge_infra_policies` covers it (Task 5) |
| `admin_policy_services_via` ("services holding an `egret-admin` row") | `default_policies.rs:206`; callers `role_membership.rs:410,451`, `members.rs:435`, `trigger.rs:1227,1256,2188` | `crate::infra::infra_services_via`, which returns services holding an IR-owned infra row **or** bound to a qualifying engine (Task 4) |

### D2. Which engines produce which rows

These are exactly today's predicates, moved into one pure place (Task 1).

- **Qualifying engine for a profile `P`:** a row of `engine_mgmt.engines` with `authz_service_id = P` and `origin <> 'external'` whose spec says it delegates (`supports_delegation`). For Trino that means: `trinoConfig.delegationMode` is `impersonate` (the default when absent), or `trinoConfig.auth.jwt` is an object. For StarRocks: `starrocksConfig.delegation.mode` is `impersonate` or `jwt`.
- **Delegator-impersonating engine:** a qualifying **Trino** engine in `impersonate` mode whose provision config has auth. Today that is `build_trino_config(&engine.spec).auth.is_some()` (`engines.rs:751`). Task 1 turns it into a pure spec predicate and proves the equivalence in a test.

| Family | Emitted when |
|---|---|
| `egret-admin - …` (one row per service-def hierarchy, Trino or StarRocks def) | the profile has ≥ 1 qualifying engine **and** ≥ 1 engine admin |
| `egret-user-impersonate-self`, `egret-user-execute-query`, `egret-query-timeout` | Trino profile with ≥ 1 qualifying engine |
| `egret-delegator-impersonate` | Trino profile with ≥ 1 delegator-impersonating engine |

Engine admins come from `members::engine_admin_usernames_via` (`members.rs:464`): active platform admins, direct or through a group, minted, sorted and deduped. These are the names the chokepoint uses today.

### D3. The IR

```rust
// crates/egret-perm-ir/src/statement.rs (additions)
pub enum Source {
    // ... existing variants ...
    Infra { kind: InfraKind },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum InfraKind {
    AdminAllAccess,
    DelegatorImpersonate,
    SelfImpersonate,
    ExecuteQuery,
    QueryTimeout,
}

pub enum Target {
    // ... existing variants ...
    /// An object of the EP itself, for infrastructure statements.
    Infra(InfraObject),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InfraObject {
    /// Every object the EP has.
    WholeEp,
    /// Any user, as the object of impersonation.
    AnyUser,
    /// The session user themself, as the object of impersonation.
    SessionUser,
    /// Any query.
    AnyQuery,
    /// One system session property.
    SystemProperty(String),
}
```

- Effects reuse `Effect::EnginePrivilege { capability }`. The capability is `"all"` for `WholeEp`, `"impersonate"` for `AnyUser` and `SessionUser`, `"execute"` for `AnyQuery`, and `"alter"` for `SystemProperty`.
- Audiences:
  - admins and the delegator are `MemberName::User(name)` with `Via::Direct`;
  - the per-user baseline is `MemberName::Everyone`, meaning every user acting as themself.
- `Derivation::AsDeclared`.

### D4. Convergence

`converge_infra_in_tx(tx, service_id) -> Result<InfraSync, CompileError>`, in a new `crates/egret-perm-compile/src/infra.rs`:

1. Load: `ir_load::load_infra_inputs(conn, service_id) -> Option<LoadedInfra>`. It returns `None` for a service that is not Trino or StarRocks, and in that case convergence returns `InfraSync::default()`.
2. Lower: `egret_perm_ir::lower::infra::lower_infra(&loaded.inputs)`.
3. Render: `ranger::infra::render(&statement, &loaded.service_name, engine) -> Vec<(String, Value)>`. All-access renders one `(name, body)` per hierarchy; every other kind renders one.
4. Reconcile against `SELECT id, name, policy_jsonb, managed_by FROM policy.authz_policies WHERE service_id = $1`, filtered in Rust to `is_ir_infra_name(name)`.
   - **Desired and present:**
     - if the body differs, `UPDATE … SET policy_jsonb, version = version + 1, updated_at = now() WHERE id`;
     - if `managed_by <> 'infra'`, also set `managed_by = 'infra'`.
   - **Desired and absent:** `INSERT … managed_by 'infra'`. Before inserting, call `egret_policy::store::refuse_reserved_name` and `refuse_non_infrastructure_name(type, name)`, as the old writers did.
   - **Present and not desired:** `DELETE … WHERE id`.
   - Inserts happen in **render order**: admin rows in hierarchy order, then delegator, self, execute, timeout. Today's apply wrote them in this order.
5. If anything was written, `UPDATE policy.authz_services SET policy_version = policy_version + 1, updated_at = now() WHERE id = $1`, exactly once.
6. Return `InfraSync { changed, added: Vec<String>, removed: Vec<String> }` and log it as `sync_admin_policy_in_tx` does today.

### D5. Known accepted differences

The demo stack capture in Task 7 classifies every difference into one of these, and nothing else is allowed:

- **(a)** `version`, `policyVersion` and `policyUpdateTime`.
- **(b)** A stale `egret-delegator-impersonate` row on a profile that has no delegator-impersonating engine, deleted.
- **(c)** IR-owned infra rows on a profile with no qualifying engine, deleted.
- **(d)** `egret-admin - …` rows whose hierarchy the current def no longer builds, deleted.
- **(e)** `egret-admin - …` rows with `users: []`, deleted.

---

### Task 1: Delegation facts as one pure function

**Files:**
- Create: `crates/egret-core/src/engine_delegation.rs`
- Modify: `crates/egret-core/src/lib.rs` (add `pub mod engine_delegation;`)
- Modify: `crates/egret-engine-mgmt/src/store.rs:150-218`. `Engine::{supports_jwt_delegation, delegation_mode, is_impersonate_delegation, is_jwt_delegation, supports_delegation}` delegate to the new functions.
- Test: `crates/egret-engine-mgmt/src/engines.rs` (inline `#[cfg(test)]` conformance test)

**Interfaces:**
- Produces:
  - `egret_core::engine_delegation::delegation_mode(engine_type: &str, spec: &serde_json::Value) -> &str`
  - `supports_delegation(engine_type: &str, spec: &Value) -> bool`
  - `delegator_impersonates(engine_type: &str, spec: &Value) -> bool`
  - `qualifies_for_infra(engine_type: &str, origin: &str, spec: &Value) -> bool`, which is `origin != "external" && supports_delegation(..)`.

- [ ] **Step 1: Find the auth predicate.** Read `build_trino_config` (`engines.rs:1514`) and `trino_config_from_json`. Write down the exact spec condition under which `config.auth.is_some()`. Expected shape: `trinoConfig.auth` is an object on the structured spec (`trinoConfig.coordinator` is an object), and `None` on the old flat spec. Confirm it from the code, not from this line.

- [ ] **Step 2: Write the failing conformance test** in `engines.rs`'s test module. It must cover every spec shape the code distinguishes:

```rust
#[test]
fn delegation_facts_match_the_apply_path() {
    use egret_core::engine_delegation as ed;
    let specs = [
        json!({}),
        json!({"trinoConfig": {}}),
        json!({"trinoConfig": {"delegationMode": "jwt", "auth": {"jwt": {}}}}),
        json!({"trinoConfig": {"delegationMode": "open"}}),
        json!({"trinoConfig": {"coordinator": {}, "auth": {"password": {}}}}),
        json!({"trinoConfig": {"coordinator": {}, "delegationMode": "impersonate", "auth": {"jwt": {}}}}),
        json!({"trinoConfig": {"coordinator": {}, "delegationMode": "jwt", "auth": {"jwt": {}}}}),
        json!({"trinoConfig": {"coordinator": {}}}),
    ];
    for spec in specs {
        let e = engine_with("trino", spec.clone());
        let apply = build_trino_config(&e.spec).auth.is_some() && e.is_impersonate_delegation();
        assert_eq!(ed::delegator_impersonates("trino", &spec), apply, "{spec}");
        assert_eq!(ed::supports_delegation("trino", &spec), e.supports_delegation(), "{spec}");
    }
    for spec in [json!({}), json!({"starrocksConfig": {"delegation": {"mode": "impersonate"}}}),
                 json!({"starrocksConfig": {"delegation": {"mode": "jwt"}}})] {
        assert!(!ed::delegator_impersonates("starrocks", &spec));
    }
}
```

  `engine_with(type, spec)` builds an `Engine`, as the existing `store.rs` tests do at `store.rs:695`. Add it next to the test if no helper exists. The test fails because `egret_core::engine_delegation` does not exist.

- [ ] **Step 3: Implement `engine_delegation.rs`.** Move the bodies of `Engine::delegation_mode` and `Engine::supports_jwt_delegation` here verbatim, as functions of `(engine_type, spec)`, and make the `Engine` methods call them. `delegator_impersonates` is `engine_type == "trino" && delegation_mode == "impersonate" && <the Step 1 auth condition>`.

- [ ] **Step 4: Run.**

```bash
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2a cargo test -p egret-engine-mgmt delegation
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2a cargo test -p egret-core engine_delegation
```

  Expected: PASS, and the existing `store.rs` delegation tests stay green.

- [ ] **Step 5: Commit** `refactor(engine-mgmt): delegation facts as one pure function in egret-core`.

---

### Task 2: The infra lowering in the IR

**Files:**
- Modify: `crates/egret-perm-ir/src/statement.rs` (D3 additions)
- Create: `crates/egret-perm-ir/src/lower/infra.rs`
- Modify: `crates/egret-perm-ir/src/lower/mod.rs` (`pub mod infra;`)
- Modify: every exhaustive `match` on `Source` or `Target` that stops compiling. Known sites are `crates/egret-perm-compile/src/ranger/mod.rs:49` (`skipped`) and `:78` (`emit`). Find the others with `cargo check --workspace`.

**Interfaces:**
- Produces:

```rust
pub struct InfraInputs {
    pub engine: egret_perm::EngineType,
    /// Engine admins, sorted and deduped.
    pub admins: Vec<String>,
    /// At least one qualifying engine is bound (D2).
    pub delegating: bool,
    /// At least one delegator-impersonating Trino engine is bound (D2).
    pub delegator_impersonates: bool,
}
pub fn lower_infra(i: &InfraInputs) -> Vec<Statement>;
```

  The statements come in this order: `AdminAllAccess`, `DelegatorImpersonate`, `SelfImpersonate`, `ExecuteQuery`, `QueryTimeout`, each present or absent as D2 says.
- `ranger::skipped` and `ranger::emit` get an arm for `Source::Infra`. `emit` returns `Err(CompileError::Db("infrastructure statements are converged by infra::converge_infra_in_tx, never emitted".into()))`. `skipped` maps to a new `SourceKind::Infra` with `Uuid::nil()`; `lower_infra` produces no lowerings, so this arm is never reached in practice. Add `SourceKind::Infra` to `trigger.rs` and to every match on `SourceKind`. In `follows_classifications` it is `false`. In `record_compiled` it is skipped, like `Clearance`.

- [ ] **Step 1: Write failing unit tests** in `lower/infra.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::audience::MemberName;
    use crate::statement::{Effect, InfraKind, InfraObject, Source, Target};
    use egret_perm::EngineType;

    fn kinds(v: &[Statement]) -> Vec<InfraKind> {
        v.iter().map(|s| match s.source { Source::Infra { kind } => kind, _ => panic!() }).collect()
    }
    fn inputs(engine: EngineType, admins: &[&str], delegating: bool, imp: bool) -> InfraInputs {
        InfraInputs { engine, admins: admins.iter().map(|s| s.to_string()).collect(),
                      delegating, delegator_impersonates: imp }
    }

    #[test]
    fn nothing_without_a_qualifying_engine() {
        assert!(lower_infra(&inputs(EngineType::Trino, &["a"], false, false)).is_empty());
    }

    #[test]
    fn trino_impersonate_emits_all_five_in_order() {
        let s = lower_infra(&inputs(EngineType::Trino, &["a", "b"], true, true));
        assert_eq!(kinds(&s), vec![InfraKind::AdminAllAccess, InfraKind::DelegatorImpersonate,
            InfraKind::SelfImpersonate, InfraKind::ExecuteQuery, InfraKind::QueryTimeout]);
        assert_eq!(s[0].target, Target::Infra(InfraObject::WholeEp));
        assert_eq!(s[0].effect, Effect::EnginePrivilege { capability: "all".into() });
        let admins: Vec<_> = s[0].audience.include.iter().map(|m| m.name.clone()).collect();
        assert_eq!(admins, vec![MemberName::User("a".into()), MemberName::User("b".into())]);
        assert_eq!(s[1].target, Target::Infra(InfraObject::AnyUser));
        assert_eq!(s[2].target, Target::Infra(InfraObject::SessionUser));
        assert_eq!(s[2].audience.include[0].name, MemberName::Everyone);
        assert_eq!(s[3].target, Target::Infra(InfraObject::AnyQuery));
        assert_eq!(s[4].target,
                   Target::Infra(InfraObject::SystemProperty("query_max_run_time".into())));
    }

    #[test]
    fn jwt_trino_has_no_delegator_grant() {
        let s = lower_infra(&inputs(EngineType::Trino, &["a"], true, false));
        assert!(!kinds(&s).contains(&InfraKind::DelegatorImpersonate));
        assert_eq!(s.len(), 4);
    }

    #[test]
    fn no_admins_means_no_admin_statement() {
        let s = lower_infra(&inputs(EngineType::Trino, &[], true, false));
        assert!(!kinds(&s).contains(&InfraKind::AdminAllAccess));
    }

    #[test]
    fn starrocks_gets_the_admin_set_only() {
        let s = lower_infra(&inputs(EngineType::StarRocks, &["a"], true, true));
        assert_eq!(kinds(&s), vec![InfraKind::AdminAllAccess]);
    }
}
```

  The delegator's name is `egret_core::engine_principals::DELEGATOR_PRINCIPAL`. Check that `egret-perm-ir` may depend on `egret-core`. If it may not, add `delegator: String` to `InfraInputs` and have the loader fill it.

- [ ] **Step 2: Run** `cargo test -p egret-perm-ir infra`. Expected: FAIL, because the items are not defined.

- [ ] **Step 3: Implement** D3 and `lower_infra` exactly as the tests state. `query_max_run_time` is a `const QUERY_TIMEOUT_PROPERTY: &str` in `lower/infra.rs`; the Ranger builder keeps its own copy in Task 3, and a Task 3 test asserts the two are equal.

- [ ] **Step 4: Run** `cargo test -p egret-perm-ir` (the boundary test included) and `cargo check --workspace --all-targets`. Expected: PASS.

- [ ] **Step 5: Mutation check.** For each D2 rule in turn, flip the condition in `lower_infra` (for example, emit the delegator grant on `delegating` instead of `delegator_impersonates`). Run the tests, confirm at least one fails, and restore the code. Record each mutation and its failing test in the task report.

- [ ] **Step 6: Commit** `feat(perm-ir): infrastructure statements and their lowering`.

---

### Task 3: The infra bodies in one place, and the Ranger renderer

**Files:**
- Create: `crates/egret-policy-api/src/infra_bodies.rs`. It receives `policy`, `delegator_impersonate_policy`, `self_impersonate_policy`, `execute_query_policy`, `query_timeout_policy`, `user_baseline_policies`, `USER_MACRO` and `QUERY_TIMEOUT_PROPERTY`, moved verbatim from `crates/egret-engine-mgmt/src/delegator_policy.rs`, together with their unit tests.
- Modify: `crates/egret-engine-mgmt/src/delegator_policy.rs`. Keep only the re-exports its callers use: `DELEGATOR_USERNAME` and the names. Callers import the builders from `egret_policy_api::infra_bodies`.
- Modify: `crates/egret-policy/src/default_policies.rs`. Add `pub fn admin_all_access_bodies(service_type: &str, service_name: &str, users: &[String]) -> Result<Vec<(String, Value)>, PolicyStoreError>`. It picks the def as `write_admin_all_access_in_tx` does (`starrocks_service_def` for `"starrocks"`, else `trino_service_def`) and calls `build_default_policies(&def, service_name, service_type, ADMIN_ALL_ACCESS_NAME_PREFIX, users, &[])`. `write_admin_all_access_in_tx` now calls it.
- Create: `crates/egret-perm-compile/src/ranger/infra.rs`
- Modify: `crates/egret-perm-compile/src/ranger/mod.rs` (`pub(crate) mod infra;`)

**Interfaces:**
- Consumes: `Statement`, `Source::Infra`, `InfraKind` and `InfraObject` from Task 2.
- Produces: `pub(crate) fn render(s: &Statement, service_name: &str, engine: EngineType) -> Result<Vec<(String, Value)>, CompileError>`.
  - `AdminAllAccess` → `admin_all_access_bodies(type, service_name, users)`, where users are the audience's `MemberName::User` names in order.
  - `DelegatorImpersonate` → `infra_bodies::delegator_impersonate_policy(service_name)`.
  - `SelfImpersonate` → `self_impersonate_policy`.
  - `ExecuteQuery` → `execute_query_policy`.
  - `QueryTimeout` → `query_timeout_policy`.
  - The name is the builder's `name`. A statement whose `Source` is not `Infra` is an `Err`.

- [ ] **Step 1: Check the dependency direction.** Confirm that `egret-policy-api` already depends on `serde_json` and that `egret-engine-mgmt` and `egret-perm-compile` both depend on `egret-policy-api` (they do: `Cargo.toml` lines 31 and 78). Do not add `egret-engine-mgmt` as a dependency of anything.

- [ ] **Step 2: Move the builders** with `git mv`-style edits: cut from `delegator_policy.rs` and paste into `infra_bodies.rs`, including their tests. Update the imports in `engines.rs` (`dp::delegator_impersonate_policy`, `dp::user_baseline_policies`) and in `crates/egret-perm-compile/tests/golden/fixtures.rs` if it uses them.

- [ ] **Step 3: Write failing renderer tests** in `ranger/infra.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use egret_perm::EngineType;
    use egret_perm_ir::lower::infra::{lower_infra, InfraInputs};
    use egret_policy_api::infra_bodies as b;

    #[test]
    fn trino_renders_todays_bodies_byte_for_byte() {
        let s = lower_infra(&InfraInputs { engine: EngineType::Trino,
            admins: vec!["ann".into(), "bob".into()], delegating: true, delegator_impersonates: true });
        let got: Vec<(String, serde_json::Value)> =
            s.iter().flat_map(|s| render(s, "svc", EngineType::Trino).unwrap()).collect();
        let mut want = egret_policy::default_policies::admin_all_access_bodies(
            "trino", "svc", &["ann".into(), "bob".into()]).unwrap();
        want.push((b::IMPERSONATE_POLICY_NAME.into(), b::delegator_impersonate_policy("svc")));
        want.extend(b::user_baseline_policies("svc").into_iter().map(|(n, v)| (n.to_string(), v)));
        assert_eq!(serde_json::to_vec(&got).unwrap(), serde_json::to_vec(&want).unwrap());
    }

    #[test]
    fn starrocks_renders_the_starrocks_def() {
        let s = lower_infra(&InfraInputs { engine: EngineType::StarRocks,
            admins: vec!["ann".into()], delegating: true, delegator_impersonates: false });
        let got: Vec<_> = s.iter().flat_map(|s| render(s, "sr", EngineType::StarRocks).unwrap()).collect();
        let want = egret_policy::default_policies::admin_all_access_bodies(
            "starrocks", "sr", &["ann".into()]).unwrap();
        assert_eq!(got.len(), 13);
        assert_eq!(serde_json::to_vec(&got).unwrap(), serde_json::to_vec(&want).unwrap());
    }

    #[test]
    fn the_timeout_property_is_one_value() {
        let body = b::query_timeout_policy("svc");
        assert_eq!(body["resources"]["systemproperty"]["values"][0],
                   egret_perm_ir::lower::infra::QUERY_TIMEOUT_PROPERTY);
    }
}
```

  Both sides go through one serializer, so comparing bytes checks the property the download needs: the same keys, in the same order, with the same values. Make `QUERY_TIMEOUT_PROPERTY` `pub` in Task 2's module if it is not already.

- [ ] **Step 4: Run** `cargo test -p egret-perm-compile --lib ranger::infra` (expect FAIL: `render` is missing). Implement it, then run again. Expected: PASS.

- [ ] **Step 5: Run** the moved tests: `cargo test -p egret-policy-api infra_bodies`, and `cargo test -p egret-policy --lib default_policies`. Expected: PASS.

- [ ] **Step 6: Commit** `refactor(policy): infra bodies in one place; Ranger renderer for infra statements`.

---

### Task 4: Convergence replaces the egret-admin sync

**Files:**
- Modify: `crates/egret-policy-api/src/infra_names.rs`. Add `pub fn is_ir_infra_name(name: &str) -> bool`, which is `is_admin_all_access_policy_name(name)` or one of the four fixed names, with a unit test that `all - catalog` is **not** one.
- Modify: `crates/egret-perm-compile/src/ir_load.rs`. Add `pub(crate) struct LoadedInfra { service_name: String, service_type: String, inputs: InfraInputs }` and `pub(crate) async fn load_infra_inputs(conn: &mut PgConnection, service_id: i64) -> Result<Option<LoadedInfra>, CompileError>`. It reads `name, type FROM policy.authz_services` (returning `None` unless `engine_type_for(type)` is `Some`), then `type, origin, spec FROM engine_mgmt.engines WHERE authz_service_id = $1`, applies Task 1's functions, and reads the admins with `members::engine_admin_usernames_via`.
- Create: `crates/egret-perm-compile/src/infra.rs` with:
  - `converge_infra_in_tx` (D4);
  - `pub(crate) async fn infra_services_via(conn) -> Result<Vec<i64>, CompileError>`, ascending and deduped: services holding an `is_ir_infra_name` row **or** with a qualifying bound engine, read through Task 1's `qualifies_for_infra` in Rust;
  - `pub struct InfraSync`.
- Modify: `crates/egret-perm-compile/src/trigger.rs`.
  - `recompile_locked_service_in_tx` calls `crate::infra::converge_infra_in_tx` where it calls `sync_admin_policy_in_tx` today (`:1194`).
  - Delete `sync_admin_policy_in_tx`.
  - `admin_policy_services_in` (`:1251`) and `recompile_can_mint` (`:2188`) use `infra_services_via`.
- Modify: `crates/egret-perm-compile/src/role_membership.rs:405-480`.
  - `set_user_status_and_recompile_in_tx` iterates `infra_services_via` and calls `converge_infra_in_tx`.
  - Rename `sync_admin_policies` to `converge_infra_policies` (same body over `infra_services_via`, calling `converge_infra_in_tx`). Re-export it from `lib.rs`, and update `egret-bin/src/main.rs:155,2912`.
- Modify: `crates/egret-perm-compile/src/members.rs:435`, to use `infra_services_via`.
- Modify: `crates/egret-policy/src/default_policies.rs`. Delete `sync_admin_all_access_in_tx`, `AdminPolicySync`, `service_name_and_type` if unused, and `admin_policy_services_via` once no caller remains. `write_admin_all_access_in_tx` and `seed_admin_all_access` stay until Task 5.
- Modify: `crates/egret-engine-mgmt/src/profile_compiler.rs`. Add `async fn converge_infra(&self, service_id: i64) -> Result<(), egret_perm::CompileError>;` to `ProfileCompilerPort`. Implement it in `egret-bin/src/main.rs` (impls at `:301`/`:315`) as one transaction: `lock_services_in_tx(&[id])`, then `lock_compile_mints_in_tx`, then `converge_infra_in_tx`, then commit. Expose a `pub async fn converge_infra_policy(pool, service_id)` from `egret-perm-compile` for that impl.
- Test: `crates/egret-perm-compile/tests/infra_converge.rs` (new); adapt `crates/egret-perm-compile/tests/admin_policy_sync.rs`.
- Modify: `crates/egret-perm-compile/tests/golden/fixtures.rs` (the fixture changes that keep every golden file byte-identical; see Step 1).

**Interfaces:**
- Consumes: Task 1's `egret_core::engine_delegation::*`, Task 2's `lower_infra` and `InfraInputs`, Task 3's `ranger::infra::render`.
- Produces:
  - `crate::infra::{converge_infra_in_tx, infra_services_via, InfraSync}`;
  - `egret_perm_compile::{converge_infra_policies, converge_infra_policy}`;
  - `ProfileCompilerPort::converge_infra`.

- [ ] **Step 1: Pin the fixtures, on code that does not change behaviour yet.**
  - `engine_bound` (`fixtures.rs:310`) gives a Trino engine the spec `{"trinoConfig": {"delegationMode": "open"}}`. Today's fixtures never ran engine apply, so no golden profile holds infra rows except G16. "open" keeps it that way, because "open" is non-qualifying (D2).
  - G16: replace the `sync_admin_all_access_in_tx(&mut tx, tr, &["g16_stale"])` call with a direct insert of `admin_all_access_bodies("trino", &tr_name, &["g16_stale".into()])` rows through `stored_policy(…, "infra", true)`, in the returned order. Keep `delegator_policies(...)` as it is.
  - G16 then binds a qualifying, delegator-impersonating Trino engine with a new helper `engine_bound_spec(tx, role, profile, "trino", spec, names)`. The spec is `{"trinoConfig": {"coordinator": {}, "delegationMode": "impersonate", "auth": <Task 1 Step 1's auth shape>}}`.

  Run the golden suite on this commit, before any convergence code exists:

```bash
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2a cargo test -p egret-perm-compile --features harness --test golden_compile
```

  Expected: PASS, with every golden file unchanged. If G16 differs only because the engine is now bound (for example, a version-dependent completion pass), choose the engine's image so that `EpInput::engines` resolves to the default version. Do not bless.

- [ ] **Step 2: Write the failing integration tests** `tests/infra_converge.rs`. Use the patterns of `tests/admin_policy_sync.rs` (`test_db`, `SERIAL`, `new_user`, `make_admin_directly`, `unique`), plus a helper that inserts an engine row with a given spec bound to a profile. Each test asserts on `policy.authz_policies` rows (`name`, `id`, `managed_by`, `version`) and on `policy_version`.

```rust
#[tokio::test]
async fn an_impersonate_trino_engine_gets_all_seventeen_rows() {
    // Arrange: one admin; a Trino profile; a bound impersonate+auth engine.
    // Act: converge_infra_policy(&db.pool, svc).
    // Assert: 13 `egret-admin - …` + the 4 fixed names, all managed_by='infra';
    //         egret-admin users == [admin]; policy_version moved by exactly 1.
}

#[tokio::test]
async fn converging_twice_writes_nothing_the_second_time() {
    // Assert: ids, `version`s and policy_version identical after the 2nd call.
}

#[tokio::test]
async fn impersonate_to_jwt_removes_the_delegator_grant_and_keeps_the_other_ids() {
    // Arrange: converge with impersonate; record ids. Update the engine spec to jwt.
    // Act: converge. Assert: `egret-delegator-impersonate` gone; every other id unchanged.
}

#[tokio::test]
async fn unbinding_the_last_qualifying_engine_removes_every_ir_infra_row_but_not_all_defaults() {
    // Arrange: seed_default_trino_policies (`all - …`) + converge with an engine.
    // Act: set engines.authz_service_id = NULL; converge.
    // Assert: no is_ir_infra_name row left; the 13 `all - …` rows untouched (ids, versions).
}

#[tokio::test]
async fn a_starrocks_profile_gets_egret_admin_only() {
    // engine spec {"starrocksConfig":{"delegation":{"mode":"impersonate"}}}: 13 rows, StarRocks def names.
}

#[tokio::test]
async fn an_external_or_open_engine_does_not_qualify() {
    // origin='external' (spec {}), and a Trino engine in "open": zero rows each.
}

#[tokio::test]
async fn a_hand_written_profile_stays_manual() {
    // converge + recompile_service on a manual profile with an engine: origin stays 'manual'.
}

#[tokio::test]
async fn an_existing_row_is_adopted_in_place() {
    // Arrange: insert today's exact `egret-user-execute-query` body by hand (managed_by 'infra').
    // Act: converge. Assert: same id, same version (body equal → no write).
}
```

  Fill every test body: real arrange code, the act call, and assertions with messages. The comments above state exactly what each one asserts.

- [ ] **Step 3: Run** `env -u EGRET_ALLOW_DB_SKIP … cargo test -p egret-perm-compile --test infra_converge`. Expected: FAIL to compile, because `converge_infra_policy` does not exist.

- [ ] **Step 4: Implement** `is_ir_infra_name`, `load_infra_inputs`, `infra.rs` (D4), the trigger, role_membership and members changes, and the port method with its `main.rs` impl, as listed under **Files**.

- [ ] **Step 5: Adapt `tests/admin_policy_sync.rs`.**
  - `seeded_service` and `reseed` keep writing the arrangement rows directly. They also bind a qualifying engine to the service, because convergence keeps `egret-admin` only on such a profile.
  - `sync_admin_policies` becomes `converge_infra_policies`.
  - `admin_policy_services_are_the_seeded_ones` asserts on `infra_services_via` through a public test hook, or moves into `infra_converge.rs` with the same arrangement.
  - `the_sync_rewrites_egret_admin_rows_its_def_no_longer_builds` becomes `convergence_deletes_egret_admin_rows_its_def_no_longer_builds` (decision 2026-11-04, D5 (d)).
  - `an_empty_admin_set_clears_the_body` becomes `an_empty_admin_set_removes_the_rows` (D5 (e)). It still asserts through `trino_can_select` that no one holds ALL access.
  - Every other test keeps its assertions unchanged. That includes `disabling_an_admin_does_not_depend_on_a_profile_compiling`, `the_boot_pass_repairs_healthy_engines_when_one_cannot_compile` and `a_disable_racing_an_admin_grant_leaves_them_out_of_egret_admin`.

- [ ] **Step 6: Run, one at a time:**

```bash
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2a cargo test -p egret-perm-compile --test infra_converge
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2a cargo test -p egret-perm-compile --test admin_policy_sync
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2a cargo test -p egret-perm-compile --features harness --test golden_compile
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2a cargo test -p egret-perm-compile --test profile_lock_sites
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2a cargo test -p egret-policy
```

  Expected: all PASS, and no golden file modified (`git status --short crates/egret-perm-compile/tests/golden` is empty).

- [ ] **Step 7: Mutation checks.** Each of these must make a named test fail; restore after each and record the result.
  - (a) Skip the DELETE branch in convergence.
  - (b) Bump `policy_version` unconditionally.
  - (c) Run the kill switch through `recompile_service_in_tx` instead of convergence alone.
  - (d) Drop `origin <> 'external'` from `qualifies_for_infra`.

- [ ] **Step 8: Commit** `feat(perm-compile): converge infrastructure policies from the IR`.

---

### Task 5: Engine apply and boot stop writing infra rows

**Files:**
- Modify: `crates/egret-engine-mgmt/src/engines.rs`.
  - **Trino apply (`:751-783`):** keep `ensure_delegator_secret` and `inject_delegator_user`. Replace `seed_delegator_policies`, `seed_admin_all_access` and `seed_user_baseline` with one call, made after the binding is resolved: `if let Some(svc_id) = <bound trino/starrocks profile id> { converge_infra_at_apply(st, svc_id).await?; recompile_after_infra_seed(st, svc_id).await; }`. An `Err` maps to `CpError::BadRequest(format!("converge infrastructure policies: {e}"))`, as the seed errors did.
  - **StarRocks apply (`:3857-3873`):** the same.
  - Delete `admin_usernames`, `seed_delegator_policies`, `seed_user_baseline`, `seed_engine_user_baseline` and `seed_user_baselines_for_existing_engines`, and their re-exports in `lib.rs`.
  - `converge_infra_at_apply` calls `st.perm_compiler.as_ref().ok_or(...)?.converge_infra(svc_id)`. With no compiler wired, it is an `Err` that names the missing compiler. Production always wires one (`main.rs:2822,3071`).
- Modify: `cleanup_deleted_engine` (`engines.rs:6469`). Collect the deleted engine's former `authz_service_id` and call `converge_infra` on it, so the rows of a profile whose last qualifying engine was deleted go away. Unbind already recompiles through `recompile_former_profile` (`:5565`), which now converges.
- Modify: `crates/egret-bin/src/main.rs:3093`. Remove the baseline pass. The boot pass `converge_infra_policies` (`:2912`) covers it.
- Modify: `crates/egret-policy/src/default_policies.rs`. Delete `seed_admin_all_access` and `write_admin_all_access_in_tx`. Keep `admin_all_access_bodies`, `build_default_policies` and `seed_default_trino_policies`.
- Modify: `crates/egret-policy/src/port_impl.rs` and `crates/egret-policy-api/src/port.rs`. Delete the `seed_admin_all_access` port method if nothing else calls it. `upsert_infra_policy` stays, for `all - …`.
- Modify tests:
  - `crates/egret-policy/tests/infra_writers.rs`: drop the admin-seeder cases. Keep the default-seeder and name-refusal cases.
  - `engines.rs` inline tests `:9471-9700`: each asserts on the rows after the apply, through convergence. The boot-pass test becomes "the boot pass converges an existing engine and skips open, unbound and external ones", calling `converge_infra_policies`.

**Interfaces:**
- Consumes: `ProfileCompilerPort::converge_infra` and `egret_perm_compile::converge_infra_policies` (Task 4).

- [ ] **Step 1: Write the failing test.** Find the inline test at `engines.rs:9657` ("the boot pass seeds an existing engine …"). Rewrite it against `converge_infra_policies`, and add: "an engine switched impersonate → jwt and re-applied loses `egret-delegator-impersonate`". Run `cargo test -p egret-engine-mgmt` and expect FAIL.
- [ ] **Step 2: Implement** the removals and the apply call.
- [ ] **Step 3: Run, one at a time:**

```bash
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2a cargo test -p egret-engine-mgmt
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2a cargo test -p egret-policy
env -u EGRET_ALLOW_DB_SKIP CARGO_TARGET_DIR=$PWD/target-sp2a cargo test -p egret-perm-compile --features harness --test golden_compile
grep -rn "upsert_infra_policy\|seed_admin_all_access\|seed_user_baseline\|seed_delegator_policies" crates --include='*.rs'
```

  Expected: tests PASS. The grep shows `upsert_infra_policy` only in `store.rs`, `port_impl.rs`, `port.rs`, `default_policies.rs` (the `all - …` seeder) and tests. There is no other writer.
- [ ] **Step 4: Commit** `refactor(engine-mgmt): apply and boot converge infra policies instead of writing them`.

---

### Task 6: New golden scenarios for infra

**Files:**
- Modify: `crates/egret-perm-compile/tests/golden/fixtures.rs` and the fixture list in `golden_compile.rs`.
- Create (bless): `G22_infra_fresh_trino_and_starrocks.json` and `G23_infra_stale_rows_removed.json`.

**Scenarios:**
- **G22.** There is one platform admin. Profile A is Trino with a qualifying impersonate+auth engine and no infra rows. Profile B is StarRocks with an impersonate engine. After the recompile, A has 13 `egret-admin - …` rows plus the 4 fixed rows, and B has 13 StarRocks rows.
- **G23.** A Trino profile carries today's rows (all 17, inserted the way G16 does) but its engine is jwt. A second Trino profile carries the 3 baseline rows with no engine bound. After the recompile, the first loses `egret-delegator-impersonate` and the second loses all three.

- [ ] **Step 1: Add the fixtures. Bless only the two new files:**

```bash
env -u EGRET_ALLOW_DB_SKIP EGRET_GOLDEN_BLESS=1 CARGO_TARGET_DIR=$PWD/target-sp2a cargo test -p egret-perm-compile --features harness --test golden_compile
git status --short crates/egret-perm-compile/tests/golden
```

  Expected: exactly two new `??` files and no ` M` line. If any existing file shows ` M`, stop and report; do not commit it.

- [ ] **Step 2: Read both new files.** Check the row counts and names against the scenario text. Write the counts into the task report.
- [ ] **Step 3: Re-run without bless.** Expected: PASS.
- [ ] **Step 4: Commit** `test(perm-compile): golden scenarios for converged infrastructure policies`.

---

### Task 7: Spec amendment, docs check, demo-stack capture

**Files:**
- Modify: `docs/en/specs/2026-11-04-gov-sp2-trino-opa.md`, section "2a". Replace the IR bullets with D3's names (`Target::Infra(InfraObject)`, `Effect::EnginePrivilege { capability }`; there is no `Target::WholeEp`, `Effect::AllAccess` or `EngineValue`). State:
  - the `all - …` exclusion;
  - convergence as its own path (D4);
  - the deletions (D5);
  - the byte-identical definition: golden files unchanged, and the demo stack identical modulo D5.
- Docs: search `egret-data/docs` for the infra policy names. If a page says these rows are "seeded at apply" or "never removed", open a docs PR that corrects it. Otherwise record "no docs change" in the report.

- [ ] **Step 1: Write the spec amendment** and commit it: `docs(spec): SP2a as built — convergence, all-defaults out of the IR`.
- [ ] **Step 2: Demo-stack capture (the controller runs this, not a subagent).** Follow the demo-stack rules:
  - port 18080;
  - the rebuild lock at `<scratchpad>/egret-rebuild.lock`;
  - never touch `egret-dev-*`, `egretdocs-*` or `egret-s4-*`;
  - no `just up/down/reset-testdata`.

  Steps:
  1. Run `scripts/golden-demo-capture.sh` on the current image, as the "before".
  2. Rebuild egret from this branch. Grep the image binary for `converge_infra_in_tx`, so a stale build cache cannot ship the old binary.
  3. Restart, and run the capture again as the "after".
  4. Compare the downloads with `jq 'del(.policyVersion, .policyUpdateTime) | .policies |= map(del(.version))'`. Every remaining difference must be one of D5 (b)–(e).
  5. Then restart egret a second time and capture again. Expect zero difference, with `policyVersion` unchanged: the +3-per-boot bump is gone.
- [ ] **Step 3:** Record the before and after sums and every D5-classified difference in the PR body.

---

## Self-review notes (for the controller)

- **Spec coverage.**
  - IR source and lowering: Tasks 2 and 4.
  - Ranger target: Task 3.
  - Write path by name with ids kept: Task 4 (D4).
  - Every profile, hand-written included: Task 4, `a_hand_written_profile_stays_manual`.
  - Old writers replaced: Tasks 4 and 5.
  - No bump when unchanged: Task 4, `converging_twice…`, and Task 7 Step 2.
  - StarRocks: Tasks 2, 4 and 6.
  - Golden byte-identical: Tasks 4, 5 and 6.
  - New create → restart golden: Task 6 together with Task 7's live restart check.
  - Mutation tests: Tasks 2 and 4.
- **Deviations from the spec, decided 2026-11-04 and written back in Task 7:**
  - `all - …` stays outside the IR.
  - Stale rows are deleted.
  - Byte-identity is redefined.
  - Convergence runs separately from `apply_diff_in_tx`, so the kill switch keeps working when a compile fails.
