# EO2 — `egret-cedar` compile target + matrix + differential — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a new `egret-cedar` crate that compiles the Cedar `PolicySet` the control-plane PDP runs today from a neutral input, add the `egret-cedar` capability-matrix entry, and prove the compiled policy is decision-equivalent to the live PDP over a corpus — with zero runtime behaviour change.

**Architecture:** `egret-cedar` holds the pure Cedar text formatters (moved from the PDP), a `compile(&CompileInputs) -> PolicySet` that assembles the same template set the PDP's `load_inner` builds (per-role permit/forbid templates + admin-graph `forbid` + classification `forbid`s), and a `load_inputs(db)` that reads the legacy control-plane tables into `CompileInputs`. `egret-auth` depends on `egret-cedar` and re-uses the formatters, so the PDP is byte-identical. The decision-equivalence differential lives in `egret-auth/tests` (the only crate that sees both the live PDP oracle and the compiler). The capability-matrix entry is a standalone target in `egret-perm-ir/src/matrix.rs`, mirroring `iceberg_rest_*`.

**Tech Stack:** Rust, `cedar-policy` 4.12 (workspace dep), `egret-metastore::Db` (Postgres via `sqlx`), the governance IR crates.

## Global Constraints

- **Zero runtime behaviour change.** The live `CedarPdp::authorize` / `load_inner` produce byte-identical Cedar text and identical decisions after this work. No migration. No new policy decisions — EO2 reproduces today's policy *shape* only; proving that is the differential's job.
- **Reuse, don't re-type, the Cedar text.** The three formatters (`template_for_role`, `deny_template_for_role`, `classification_forbid`) and the admin-graph `forbid` builder have exactly one home: `egret_cedar::text`. The PDP and the compiler both call them. No second copy of any Cedar policy string.
- **`egret-cedar` is its own crate**, not an `egret-perm-compile` backend: role-templated Cedar has no `Path` and does not fit the data-plane `Statement`/`lower_*` IR. Its deps are `cedar-policy`, `egret-core`, `egret-metastore`, `uuid` — it must **not** depend on `egret-auth` (that would cycle).
- **Matrix entry mirrors `iceberg_rest_*` exactly** (`EGRET_CEDAR` const + `*_support`/`*_gap` fns); the Ranger/OPA `ENTRIES` table is untouched.
- **One action catalog.** Every action list is driven from `egret_core::rbac` (`PERMISSIONS`, `ADMIN_GRAPH_ACTIONS`) through `egret_perm_ir::egret_object::classify` — never a hand-maintained second list.
- **The differential is a real test-DB test CI runs.** Follow the repo convention (see `crates/egret-auth/tests/cedar_pdp.rs`): obtain the DB via `egret_test_db::url()`, and when it is absent **fail** unless `EGRET_ALLOW_DB_SKIP` is set (never a silent skip).
- **Build with `CARGO_TARGET_DIR=target-eo2`** for every cargo invocation in this worktree (shared target dir otherwise collides — see memory `shared-cargo-target-across-checkouts`). `target-eo2/` is not gitignored; never stage it.
- **Conventional Commits**; stage by explicit path (never `git add -A`). Every commit ends with `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.

---

### Task 1: New `egret-cedar` crate; move the pure Cedar formatters into it

**Files:**
- Create: `crates/egret-cedar/Cargo.toml`, `crates/egret-cedar/src/lib.rs`, `crates/egret-cedar/src/text.rs`
- Modify: `crates/egret-auth/Cargo.toml` (add `egret-cedar` dep)
- Modify: `crates/egret-auth/src/pdp/templates.rs` (becomes a re-export)
- Modify: `crates/egret-auth/src/pdp/mod.rs` (`load_inner` calls `egret_cedar::text::admin_graph_forbid()`)

**Interfaces:**
- Produces: `egret_cedar::text::{template_for_role, deny_template_for_role, classification_forbid, admin_graph_forbid}`, and PolicyId-string helpers `role_template_id(id)`, `role_deny_id(id)`, `classification_id(tag_id)`.
- `template_for_role(role_name: &str, scope: &str, perms: &[String]) -> String`
- `deny_template_for_role(role_name: &str, scope: &str, perms: &[String]) -> String`
- `classification_forbid(tag_key: &str, required_roles: &[String], required_groups: &[uuid::Uuid]) -> String`
- `admin_graph_forbid() -> String`

- [ ] **Step 1: Create the crate skeleton.**

`crates/egret-cedar/Cargo.toml`:

```toml
[package]
name = "egret-cedar"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
cedar-policy.workspace = true
egret-core = { workspace = true }
uuid.workspace = true

[dev-dependencies]
serde_json.workspace = true
```

`crates/egret-cedar/src/lib.rs`:

```rust
//! EO2: compile the Cedar policy the control-plane PDP runs today from a
//! neutral input. The pure text formatters (`text`) are the single source of
//! Cedar policy strings, shared with `egret-auth`'s PDP; `compile` assembles
//! the same template set `load_inner` builds; `load` reads the legacy
//! control-plane tables into `CompileInputs`. Not wired to the runtime (EO3).
pub mod text;
```

- [ ] **Step 2: Move the three formatters verbatim into `text.rs`.**

Cut `template_for_role`, `deny_template_for_role`, `classification_forbid` (and their doc comments and the whole `#[cfg(test)] mod tests`) out of `crates/egret-auth/src/pdp/templates.rs` and paste them unchanged into `crates/egret-cedar/src/text.rs`. They are already pure (`uuid` + returned `String`; tests use `cedar_policy`). Add a `uuid` import if the moved code names `uuid::Uuid` unqualified.

- [ ] **Step 3: Add `admin_graph_forbid()` and the id helpers to `text.rs`.**

Extract the admin-graph `forbid` string builder that is inline in `load_inner` (`crates/egret-auth/src/pdp/mod.rs`) into a function, and add the PolicyId-string helpers:

```rust
use egret_core::rbac;
use uuid::Uuid;

/// The single static admin-graph `forbid`: the authz-graph-mutating actions
/// (`rbac::ADMIN_GRAPH_ACTIONS`) are forbidden unless the request context flags
/// the subject as platform_admin. `forbid` beats every `permit`, so even a role
/// force-granted one of these cannot escalate. Verbatim the string `load_inner`
/// builds today.
pub fn admin_graph_forbid() -> String {
    let actions = rbac::ADMIN_GRAPH_ACTIONS
        .iter()
        .map(|a| format!("Egret::Action::\"{a}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "forbid(principal, action in [{actions}], resource) \
         unless {{ context has is_platform_admin && context.is_platform_admin }};"
    )
}

/// The PolicyId strings the PDP keys its templates/forbids by. One home so the
/// compiler and `link_grants` agree.
pub fn role_template_id(role_id: Uuid) -> String { format!("role:{role_id}") }
pub fn role_deny_id(role_id: Uuid) -> String { format!("role-deny:{role_id}") }
pub fn classification_id(tag_id: Uuid) -> String { format!("classification:{tag_id}") }
```

- [ ] **Step 4: Point `egret-auth` at the moved code.**

In `crates/egret-auth/Cargo.toml` add under `[dependencies]`:

```toml
egret-cedar = { workspace = true }
```

Replace the body of `crates/egret-auth/src/pdp/templates.rs` with a re-export so every existing call site (`crate::pdp::templates::template_for_role`, used in `mod.rs` and `entities.rs` tests) keeps resolving:

```rust
//! One Cedar policy template per role. The formatters now live in the shared
//! `egret-cedar` crate (EO2); re-exported here so the PDP's call sites are
//! unchanged and the Cedar text has exactly one source.
pub use egret_cedar::text::{classification_forbid, deny_template_for_role, template_for_role};
```

In `crates/egret-auth/src/pdp/mod.rs`, replace the inline admin-graph forbid construction (the `let actions = …ADMIN_GRAPH_ACTIONS… ; let forbid_src = format!(…);` block) with:

```rust
let forbid: cedar_policy::Policy = egret_cedar::text::admin_graph_forbid()
    .parse()
    .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("forbid policy: {e:?}")))?;
templates
    .add(forbid)
    .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("add forbid: {e:?}")))?;
```

- [ ] **Step 5: Verify behaviour is unchanged.**

Run:
```bash
CARGO_TARGET_DIR=target-eo2 cargo test -p egret-cedar
CARGO_TARGET_DIR=target-eo2 cargo test -p egret-auth --lib
```
Expected: the moved formatter tests pass under `egret-cedar`; `egret-auth` compiles and its unit tests pass (the PDP emits identical strings). DB-gated integration tests may self-skip without a DB — that is fine for this task.

- [ ] **Step 6: Commit.**

```bash
git add crates/egret-cedar/Cargo.toml crates/egret-cedar/src/lib.rs crates/egret-cedar/src/text.rs \
        crates/egret-auth/Cargo.toml crates/egret-auth/src/pdp/templates.rs crates/egret-auth/src/pdp/mod.rs
git commit -m "refactor(cedar): move PDP Cedar formatters into a shared egret-cedar crate"
```

---

### Task 2: `egret-cedar` compile — inputs + `compile()`

**Files:**
- Create: `crates/egret-cedar/src/compile.rs`
- Modify: `crates/egret-cedar/src/lib.rs` (add `pub mod compile;`)

**Interfaces:**
- Consumes: `egret_cedar::text::*` (Task 1).
- Produces:
  - `pub struct RoleInput { pub id: Uuid, pub name: String, pub scope: String, pub effective_perms: Vec<String> }`
  - `pub struct ClassificationInput { pub tag_id: Uuid, pub key: String, pub required_roles: Vec<String>, pub required_groups: Vec<Uuid> }`
  - `pub struct CompileInputs { pub roles: Vec<RoleInput>, pub classifications: Vec<ClassificationInput> }`
  - `pub struct Compiled { pub policy_set: PolicySet, pub role_names: HashMap<Uuid, String>, pub skipped_classifications: Vec<Uuid> }`
  - `pub fn compile(inputs: &CompileInputs) -> Result<Compiled, CompileError>`
  - `pub enum CompileError { Policy(String) }`

- [ ] **Step 1: Write the failing test.**

In `crates/egret-cedar/src/compile.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use cedar_policy::{Authorizer, Context, Decision, Entities, Entity, Request};

    fn role(id: Uuid, name: &str, scope: &str, perms: &[&str]) -> RoleInput {
        RoleInput { id, name: name.into(), scope: scope.into(),
            effective_perms: perms.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn compile_emits_role_templates_admin_forbid_and_classification() {
        let r = Uuid::from_u128(1);
        let tag = Uuid::from_u128(2);
        let inputs = CompileInputs {
            roles: vec![role(r, "ws_editor", "workspace", &["dashboard:read", "dashboard:manage"])],
            classifications: vec![
                ClassificationInput { tag_id: tag, key: "confidential".into(),
                    required_roles: vec!["finance_cleared".into()], required_groups: vec![] },
                // both-empty → must be skipped, emits nothing.
                ClassificationInput { tag_id: Uuid::from_u128(3), key: "public".into(),
                    required_roles: vec![], required_groups: vec![] },
            ],
        };
        let out = compile(&inputs).expect("compile");
        // The role's permit + forbid templates are present by PolicyId.
        let ids: Vec<String> = out.policy_set.templates().map(|t| t.id().to_string()).collect();
        assert!(ids.contains(&format!("role:{r}")), "permit template, got {ids:?}");
        assert!(ids.contains(&format!("role-deny:{r}")), "forbid template, got {ids:?}");
        // Static policies: the admin-graph forbid + one classification forbid.
        let static_ids: Vec<String> =
            out.policy_set.policies().map(|p| p.id().to_string()).collect();
        assert!(static_ids.iter().any(|i| i == &format!("classification:{tag}")),
            "classification forbid present, got {static_ids:?}");
        // The both-empty classification was skipped (no policy, not in skipped list
        // because it was never a gate).
        assert!(!static_ids.iter().any(|i| i == &format!("classification:{}", Uuid::from_u128(3))));
        assert_eq!(out.role_names.get(&r).map(String::as_str), Some("ws_editor"));
    }
}
```

- [ ] **Step 2: Run it to verify it fails.**

Run: `CARGO_TARGET_DIR=target-eo2 cargo test -p egret-cedar compile_emits_role_templates`
Expected: FAIL — `compile` / the input structs are not defined.

- [ ] **Step 3: Write `compile()`.**

At the top of `crates/egret-cedar/src/compile.rs`:

```rust
//! Assemble the Cedar template set the PDP's `load_inner` builds, from a neutral
//! input. Same PolicyIds, same formatters (EO2). No DB, no clock: pure.
use std::collections::HashMap;

use cedar_policy::{Policy, PolicyId, PolicySet, Template};
use uuid::Uuid;

use crate::text;

#[derive(Debug, Clone)]
pub struct RoleInput {
    pub id: Uuid,
    pub name: String,
    pub scope: String,
    pub effective_perms: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ClassificationInput {
    pub tag_id: Uuid,
    pub key: String,
    pub required_roles: Vec<String>,
    pub required_groups: Vec<Uuid>,
}

#[derive(Debug, Clone, Default)]
pub struct CompileInputs {
    pub roles: Vec<RoleInput>,
    pub classifications: Vec<ClassificationInput>,
}

pub struct Compiled {
    pub policy_set: PolicySet,
    pub role_names: HashMap<Uuid, String>,
    /// tag_ids whose classification forbid failed to parse and was skipped —
    /// mirrors `load_inner`'s log-and-skip so a malformed row does not fail the
    /// whole compile (and the differential stays equivalent to the PDP).
    pub skipped_classifications: Vec<Uuid>,
}

#[derive(Debug)]
pub enum CompileError {
    Policy(String),
}

/// Build the template set: per role a permit + forbid template (keyed
/// `role:{id}` / `role-deny:{id}`), the one static admin-graph forbid, and one
/// classification forbid per gated tag (both-empty skipped; malformed skipped,
/// exactly as `load_inner`).
pub fn compile(inputs: &CompileInputs) -> Result<Compiled, CompileError> {
    let mut policy_set = PolicySet::new();
    let mut role_names = HashMap::new();

    for r in &inputs.roles {
        let tid: PolicyId = text::role_template_id(r.id)
            .parse()
            .map_err(|e| CompileError::Policy(format!("{e:?}")))?;
        let tmpl = Template::parse(Some(tid), text::template_for_role(&r.name, &r.scope, &r.effective_perms))
            .map_err(|e| CompileError::Policy(format!("{e:?}")))?;
        policy_set.add_template(tmpl).map_err(|e| CompileError::Policy(format!("{e:?}")))?;

        let did: PolicyId = text::role_deny_id(r.id)
            .parse()
            .map_err(|e| CompileError::Policy(format!("{e:?}")))?;
        let dtmpl = Template::parse(Some(did), text::deny_template_for_role(&r.name, &r.scope, &r.effective_perms))
            .map_err(|e| CompileError::Policy(format!("{e:?}")))?;
        policy_set.add_template(dtmpl).map_err(|e| CompileError::Policy(format!("{e:?}")))?;

        role_names.insert(r.id, r.name.clone());
    }

    let forbid: Policy = text::admin_graph_forbid()
        .parse()
        .map_err(|e| CompileError::Policy(format!("admin forbid: {e:?}")))?;
    policy_set.add(forbid).map_err(|e| CompileError::Policy(format!("add admin forbid: {e:?}")))?;

    let mut skipped_classifications = Vec::new();
    for c in &inputs.classifications {
        // Both empty ⇒ informational tag, no gate ⇒ emit nothing (as load_inner).
        if c.required_roles.is_empty() && c.required_groups.is_empty() {
            continue;
        }
        let pid: PolicyId = match text::classification_id(c.tag_id).parse() {
            Ok(p) => p,
            Err(_) => { skipped_classifications.push(c.tag_id); continue; }
        };
        match Policy::parse(Some(pid), text::classification_forbid(&c.key, &c.required_roles, &c.required_groups)) {
            Ok(p) => {
                if policy_set.add(p).is_err() { skipped_classifications.push(c.tag_id); }
            }
            Err(_) => skipped_classifications.push(c.tag_id),
        }
    }

    Ok(Compiled { policy_set, role_names, skipped_classifications })
}
```

Add `pub mod compile;` to `crates/egret-cedar/src/lib.rs`.

- [ ] **Step 4: Run the test to verify it passes.**

Run: `CARGO_TARGET_DIR=target-eo2 cargo test -p egret-cedar compile_emits_role_templates`
Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/egret-cedar/src/compile.rs crates/egret-cedar/src/lib.rs
git commit -m "feat(cedar): compile a Cedar PolicySet from a neutral input"
```

---

### Task 3: `egret-cedar` loader — `load_inputs(db)`

**Files:**
- Create: `crates/egret-cedar/src/load.rs`
- Modify: `crates/egret-cedar/src/lib.rs` (add `pub mod load;`), `crates/egret-cedar/Cargo.toml` (add `egret-metastore`, `sqlx`, `anyhow` deps; `egret-test-db` dev-dep)

**Interfaces:**
- Consumes: `CompileInputs`/`RoleInput`/`ClassificationInput` (Task 2), `egret_metastore::Db`.
- Produces: `pub async fn load_inputs(db: &Db) -> anyhow::Result<CompileInputs>` — reads the legacy control-plane tables, computing effective (own ∪ ancestors') perms per role exactly as `load_inner`.

- [ ] **Step 1: Write the failing test.**

In `crates/egret-cedar/src/load.rs` (DB-gated, repo convention):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    async fn connect() -> Option<egret_metastore::Db> {
        let url = egret_test_db::url().await?;
        let db = egret_metastore::Db::connect(&url, 5).await.expect("connect");
        db.migrate().await.expect("migrate");
        Some(db)
    }

    #[tokio::test]
    async fn effective_perms_include_parent_closure() {
        let Some(db) = connect().await else {
            if std::env::var("EGRET_ALLOW_DB_SKIP").is_ok() { eprintln!("skip: no DB"); return; }
            panic!("no DB and EGRET_ALLOW_DB_SKIP unset");
        };
        // parent role holds dashboard:read; child (parent_role_id=parent) holds
        // dashboard:manage. Child's effective set must contain BOTH.
        let parent: Uuid = sqlx::query_scalar(
            "INSERT INTO roles (name, scope) VALUES ($1, 'workspace') RETURNING id")
            .bind(format!("p-{}", Uuid::new_v4().simple())).fetch_one(&db.pool).await.unwrap();
        let child: Uuid = sqlx::query_scalar(
            "INSERT INTO roles (name, scope, parent_role_id) VALUES ($1, 'workspace', $2) RETURNING id")
            .bind(format!("c-{}", Uuid::new_v4().simple())).bind(parent).fetch_one(&db.pool).await.unwrap();
        for (rid, perm) in [(parent, "dashboard:read"), (child, "dashboard:manage")] {
            sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
                .bind(rid).bind(perm).execute(&db.pool).await.unwrap();
        }

        let inputs = load_inputs(&db).await.expect("load");
        let child_in = inputs.roles.iter().find(|r| r.id == child).expect("child present");
        assert!(child_in.effective_perms.iter().any(|p| p == "dashboard:read"),
            "parent perm inherited");
        assert!(child_in.effective_perms.iter().any(|p| p == "dashboard:manage"),
            "own perm present");
    }
}
```

- [ ] **Step 2: Run it to verify it fails.**

Run: `CARGO_TARGET_DIR=target-eo2 DATABASE_URL=$DATABASE_URL cargo test -p egret-cedar effective_perms_include_parent_closure`
Expected: FAIL — `load_inputs` not defined (or, with no DB, a panic demanding the DB — set `DATABASE_URL`, or `EGRET_ALLOW_DB_SKIP=1` only to confirm the skip path).

- [ ] **Step 3: Write `load_inputs`.**

Port the two bulk queries + the effective-perm closure from `load_inner` (`crates/egret-auth/src/pdp/mod.rs`) and the classification query, producing `CompileInputs`:

```rust
//! Read the legacy control-plane tables into `CompileInputs`. The grant *source*
//! migrates to `perm.*` at EO4; EO2 reads it where it lives today.
use std::collections::{BTreeSet, HashMap};

use egret_metastore::Db;
use uuid::Uuid;

use crate::compile::{ClassificationInput, CompileInputs, RoleInput};

pub async fn load_inputs(db: &Db) -> anyhow::Result<CompileInputs> {
    let roles: Vec<(Uuid, String, String, Option<Uuid>)> =
        sqlx::query_as("SELECT id, name, scope, parent_role_id FROM roles")
            .fetch_all(&db.pool).await?;
    let perm_rows: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT role_id, permission FROM role_permissions")
            .fetch_all(&db.pool).await?;

    let mut own: HashMap<Uuid, Vec<String>> = HashMap::new();
    for (rid, perm) in perm_rows { own.entry(rid).or_default().push(perm); }
    let parent: HashMap<Uuid, Option<Uuid>> =
        roles.iter().map(|(id, _, _, p)| (*id, *p)).collect();

    // Effective = own ∪ ancestors' own, walking parent_role_id (cap at 64 hops,
    // matching load_inner's defensive bound on an acyclic chain).
    let effective = |start: Uuid| -> Vec<String> {
        let mut acc: BTreeSet<String> = Default::default();
        let mut cur = Some(start);
        let mut hops = 0;
        while let Some(c) = cur {
            if let Some(ps) = own.get(&c) { acc.extend(ps.iter().cloned()); }
            hops += 1;
            if hops > 64 { break; }
            cur = parent.get(&c).copied().flatten();
        }
        acc.into_iter().collect()
    };

    let role_inputs = roles.iter().map(|(id, name, scope, _)| RoleInput {
        id: *id, name: name.clone(), scope: scope.clone(), effective_perms: effective(*id),
    }).collect();

    let class_rows: Vec<(Uuid, String, Vec<String>, Vec<Uuid>)> = sqlx::query_as(
        "SELECT cp.tag_id, t.key, cp.required_roles, cp.required_groups \
         FROM classification_policies cp JOIN tags t ON t.id = cp.tag_id")
        .fetch_all(&db.pool).await?;
    let classifications = class_rows.into_iter()
        .map(|(tag_id, key, required_roles, required_groups)| ClassificationInput {
            tag_id, key, required_roles, required_groups })
        .collect();

    Ok(CompileInputs { roles: role_inputs, classifications })
}
```

Add `pub mod load;` to `lib.rs`. Add to `Cargo.toml` `[dependencies]`: `egret-metastore.workspace = true`, `sqlx.workspace = true`, `anyhow.workspace = true`; `[dev-dependencies]`: `egret-test-db.workspace = true`, `tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }`. (Match the feature spellings used in `crates/egret-auth/Cargo.toml`.)

- [ ] **Step 4: Run the test to verify it passes.**

Run: `CARGO_TARGET_DIR=target-eo2 DATABASE_URL=$DATABASE_URL cargo test -p egret-cedar effective_perms_include_parent_closure`
Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/egret-cedar/src/load.rs crates/egret-cedar/src/lib.rs crates/egret-cedar/Cargo.toml
git commit -m "feat(cedar): load CompileInputs from the control-plane tables"
```

---

### Task 4: `egret-cedar` capability-matrix entry

**Files:**
- Modify: `crates/egret-perm-ir/src/statement.rs` (add `ReasonCode::NotApplicable`)
- Modify: `crates/egret-perm-ir/src/matrix.rs` (add `EGRET_CEDAR` target + tests)

**Interfaces:**
- Consumes: `Capability`, `Privilege`, `RuleEffectKind`, `Support`, `Answer`, `ReasonCode`, `answer_support`, `privilege_name` (all in `matrix.rs`/`statement.rs`); `egret_object::classify` (same crate).
- Produces: `pub const EGRET_CEDAR: &str`, `pub fn egret_cedar_support(cap: &Capability) -> Support`, `pub fn egret_cedar_gap(cap: &Capability) -> Option<String>`.

- [ ] **Step 1: Write the failing tests.**

Append to the `#[cfg(test)] mod tests` in `crates/egret-perm-ir/src/matrix.rs`:

```rust
#[test]
fn egret_cedar_enforces_every_privilege_natively() {
    use Privilege::*;
    for p in [Browse, Read, Write, Create, Manage] {
        assert_eq!(egret_cedar_support(&Capability::EnforcePrivilege(p)), Support::Native, "{p:?}");
        assert_eq!(egret_cedar_gap(&Capability::EnforcePrivilege(p)), None, "{p:?}");
    }
}

#[test]
fn egret_cedar_withhold_is_native_mask_and_rowfilter_not_applicable() {
    assert_eq!(egret_cedar_support(&Capability::WithholdByClassification), Support::Native);
    for k in [RuleEffectKind::Mask, RuleEffectKind::RowFilter] {
        assert_eq!(egret_cedar_support(&Capability::ClassificationSelector(k)),
            Support::Missing(ReasonCode::NotApplicable), "{k:?}");
        let gap = egret_cedar_gap(&Capability::ClassificationSelector(k)).unwrap();
        assert!(gap.contains("not applicable"), "gap should read not-applicable, got: {gap}");
    }
}

#[test]
fn egret_cedar_unknown_capability_is_not_measured() {
    let cap = Capability::MaskType("sha256".into());
    assert_eq!(egret_cedar_support(&cap), Support::Missing(ReasonCode::NotMeasured));
    assert!(egret_cedar_gap(&cap).unwrap().contains("not measured"));
}

#[test]
fn every_object_data_grant_maps_to_a_native_egret_cedar_privilege() {
    // Tie EO1's classify to the matrix: each object-data key's privilege is
    // Native on egret-cedar (object grants are enforced by Cedar directly).
    use crate::egret_object::{classify, PermissionMapping};
    for p in egret_core::rbac::PERMISSIONS {
        if let Some(PermissionMapping::ObjectGrant { privilege, .. }) = classify(p.key) {
            assert_eq!(egret_cedar_support(&Capability::EnforcePrivilege(privilege)),
                Support::Native, "object-data key {} must be Native", p.key);
        }
    }
}
```

- [ ] **Step 2: Run them to verify they fail.**

Run: `CARGO_TARGET_DIR=target-eo2 cargo test -p egret-perm-ir egret_cedar`
Expected: FAIL — `EGRET_CEDAR`/`egret_cedar_support`/`ReasonCode::NotApplicable` not defined.

- [ ] **Step 3: Add `ReasonCode::NotApplicable`.**

In `crates/egret-perm-ir/src/statement.rs`, add a variant to `ReasonCode` (after `PrivilegeNotEnforceable`):

```rust
    /// EO2: the capability has no meaning for this EP (e.g. a column mask or
    /// row filter on an egret object, which has no columns or rows). Distinct
    /// from `NotMeasured`: it is not that we did not measure, it does not apply.
    NotApplicable,
```

- [ ] **Step 4: Add the `EGRET_CEDAR` target.**

In `crates/egret-perm-ir/src/matrix.rs`, after the `iceberg_rest_gap` function, add:

```rust
/// EO2: the egret-cedar EP (egret's own control-plane PDP). Like
/// [`ICEBERG_REST`] it is not an `EpKind` and has no rows in [`ENTRIES`]; egret
/// has no engine or version axis. Cedar enforces every grant vocabulary
/// privilege and the clearance withhold natively; column masks and row filters
/// have no meaning on an egret object.
pub const EGRET_CEDAR: &str = "egret-cedar";

const EGRET_CEDAR_PRIVILEGES: &[(Privilege, Answer)] = &[
    (Privilege::Browse, Answer::Native),
    (Privilege::Read, Answer::Native),
    (Privilege::Write, Answer::Native),
    (Privilege::Create, Answer::Native),
    (Privilege::Manage, Answer::Native),
];

const EGRET_CEDAR_EFFECTS: &[(RuleEffectKind, Answer)] = &[
    (RuleEffectKind::Mask, Answer::Missing(ReasonCode::NotApplicable)),
    (RuleEffectKind::RowFilter, Answer::Missing(ReasonCode::NotApplicable)),
];

/// The egret-cedar EP's answer for `cap`. Grant-vocabulary privileges and the
/// classification withhold are native; mask/row-filter selectors are not
/// applicable; anything else is missing, reason "not measured" (spec D7).
pub fn egret_cedar_support(cap: &Capability) -> Support {
    match cap {
        Capability::EnforcePrivilege(p) => EGRET_CEDAR_PRIVILEGES
            .iter().find(|(q, _)| q == p)
            .map_or(Support::Missing(ReasonCode::NotMeasured), |(_, a)| answer_support(*a)),
        Capability::ClassificationSelector(k) => EGRET_CEDAR_EFFECTS
            .iter().find(|(q, _)| q == k)
            .map_or(Support::Missing(ReasonCode::NotMeasured), |(_, a)| answer_support(*a)),
        Capability::WithholdByClassification => Support::Native,
        _ => Support::Missing(ReasonCode::NotMeasured),
    }
}

/// Why egret-cedar does not enforce `cap` as asked; `None` when native.
pub fn egret_cedar_gap(cap: &Capability) -> Option<String> {
    match (egret_cedar_support(cap), cap) {
        (Support::Native, _) => None,
        (Support::Lowered(_), _) => None,
        (Support::Missing(ReasonCode::NotApplicable), _) => Some(format!(
            "masks/row filters are not applicable to egret objects ({EGRET_CEDAR})")),
        (Support::Missing(_), Capability::EnforcePrivilege(p)) => Some(format!(
            "{} not enforceable by {EGRET_CEDAR}", privilege_name(*p))),
        (Support::Missing(_), _) => Some(format!("not measured for {EGRET_CEDAR}")),
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass.**

Run: `CARGO_TARGET_DIR=target-eo2 cargo test -p egret-perm-ir egret_cedar`
Expected: PASS (all four tests).

- [ ] **Step 6: Commit.**

```bash
git add crates/egret-perm-ir/src/statement.rs crates/egret-perm-ir/src/matrix.rs
git commit -m "feat(perm-ir): egret-cedar capability-matrix entry"
```

---

### Task 5: Decision-equivalence differential — roles, grants, admin-graph

**Files:**
- Modify: `crates/egret-auth/src/pdp/mod.rs` (add a `#[doc(hidden)]` test accessor + a `decide_with` seam)
- Create: `crates/egret-auth/tests/cedar_differential.rs`
- Modify: `crates/egret-auth/Cargo.toml` (dev-deps if missing: `egret-test-db`)

**Interfaces:**
- Consumes: `egret_cedar::{compile::compile, load::load_inputs}`; the PDP's `entities`/`grants` (already `pub`); `egret_core::rbac`.
- Produces (on `CedarPdp`):
  - `#[doc(hidden)] pub async fn compiled_policy_set_for_test(db: &Db) -> Result<(PolicySet, HashMap<Uuid, String>)>`
  - `#[doc(hidden)] pub async fn decide_with_for_test(&self, templates: &PolicySet, role_names: &HashMap<Uuid, String>, subject: &Subject, action: &str, resource: &Resource) -> Result<Decision>`

- [ ] **Step 1: Add the test seam to the PDP (behaviour-preserving refactor).**

In `crates/egret-auth/src/pdp/mod.rs`, extract the body of `authorize` (everything after loading `grants` and `inner`) into a private `async fn decide_with(&self, templates: &PolicySet, role_names: &RoleNames, grants: &[Grant], subject, action, resource) -> Result<Decision>` and have `authorize` call it with `&inner.templates, &inner.role_names`. Then add thin `#[doc(hidden)]` wrappers:

```rust
impl CedarPdp {
    /// Test-only: the PDP's compiled template set + role-name map, for the EO2
    /// decision-equivalence differential. Not for runtime use.
    #[doc(hidden)]
    pub async fn compiled_policy_set_for_test(db: &Db) -> Result<(PolicySet, HashMap<uuid::Uuid, String>)> {
        let inner = Self::load_inner(db).await?;
        Ok((inner.templates, inner.role_names))
    }

    /// Test-only: run the request path against an arbitrary template set (the
    /// live PDP's, or the egret-cedar-compiled one), loading grants/tags from the
    /// DB exactly as `authorize`. Isolates the compiled set in the differential.
    #[doc(hidden)]
    pub async fn decide_with_for_test(
        &self,
        templates: &PolicySet,
        role_names: &HashMap<uuid::Uuid, String>,
        subject: &Subject,
        action: &str,
        resource: &Resource,
    ) -> Result<Decision> {
        let (ws, object_ids) = match resource {
            Resource::Platform => (None, Vec::new()),
            Resource::Workspace(w) => (Some(*w), Vec::new()),
            Resource::Object(o) => (Some(o.workspace_id),
                match o.folder_id { Some(f) => vec![o.id, f], None => vec![o.id] }),
        };
        let grants = self.grants.grants_for(subject.user_id, ws, &object_ids).await?;
        self.decide_with(templates, role_names, &grants, subject, action, resource).await
    }
}
```

Run `CARGO_TARGET_DIR=target-eo2 cargo test -p egret-auth --lib` and the existing PDP integration tests you have a DB for (e.g. `cedar_pdp`, `pdp_regression`) to confirm the refactor preserved behaviour.

- [ ] **Step 2: Write the differential (failing until the generator + comparison exist).**

Create `crates/egret-auth/tests/cedar_differential.rs`. Structure (mirrors `crates/egret-perm-compile/tests/opa_differential.rs`): a seeded LCG `Rng`, a `draw_state` that seeds roles/role_permissions/access_grants into a fresh test DB, `anchors()` of deterministic (subject, resource, action, note) cases, and the comparison loop:

```rust
//! EO2 decision-equivalence differential: the egret-cedar-compiled PolicySet
//! must return the SAME Cedar decision as the live PDP for every
//! (principal, object, action). Oracle = CedarPdp (today). DB-gated: fails
//! without a DB unless EGRET_ALLOW_DB_SKIP.
use egret_auth::pdp::CedarPdp;
use egret_core::authz::{Pdp, Resource, Subject};
use egret_core::{UserId, WorkspaceId};
use egret_metastore::Db;
use egret_events::EventBus;
use uuid::Uuid;

struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 { // SplitMix64
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize { (self.next_u64() % n as u64) as usize }
    fn chance(&mut self, p: f64) -> bool { (self.next_u64() as f64 / u64::MAX as f64) < p }
}

const SEED: u64 = 0xED2_0000_0001;
const STATES: usize = 24;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

#[tokio::test]
async fn egret_cedar_decisions_agree_with_the_pdp() {
    let Some(db) = connect().await else {
        if std::env::var("EGRET_ALLOW_DB_SKIP").is_ok() { eprintln!("skip: no DB"); return; }
        panic!("no DB and EGRET_ALLOW_DB_SKIP unset");
    };
    let bus = EventBus::new();
    let pdp = CedarPdp::build(db.clone(), &bus).await.expect("pdp");
    let mut rng = Rng(std::env::var("EGRET_CEDAR_DIFF_SEED").ok()
        .and_then(|s| s.parse().ok()).unwrap_or(SEED));

    let mut diffs: Vec<String> = Vec::new();
    for s in 0..STATES {
        let state = draw_state(&db, &mut rng, s).await;
        compare_state(&db, &pdp, &state, &mut diffs).await;
    }
    // Anchors: deterministic regression cases independent of the RNG.
    for a in anchors(&db).await {
        compare_case(&db, &pdp, &a, &mut diffs).await;
    }
    assert!(diffs.is_empty(),
        "egret-cedar diverged from the PDP on {} case(s):\n{}\n(reproduce with EGRET_CEDAR_DIFF_SEED={})",
        diffs.len(), diffs.join("\n"), SEED);
}
```

The helpers the implementer completes (seed rows modeled on the existing tests cited in Step 3, run both sides via the Step-1 seam):

```rust
struct State { /* user ids, ws id, role ids, the (subject,resource,action) questions */ }

async fn draw_state(db: &Db, rng: &mut Rng, idx: usize) -> State { /* seed roles, role_permissions, access_grants, workspace, users; draw questions whose actions come from rbac::PERMISSIONS + ADMIN_GRAPH_ACTIONS across classify buckets */ }

struct Case { subject: Subject, resource: Resource, action: String, note: String }

async fn compare_case(db: &Db, pdp: &CedarPdp, c: &Case, diffs: &mut Vec<String>) {
    let (a_set, a_names) = CedarPdp::compiled_policy_set_for_test(db).await.unwrap();
    let ir = egret_cedar::compile::compile(&egret_cedar::load::load_inputs(db).await.unwrap()).unwrap();
    let today = pdp.decide_with_for_test(&a_set, &a_names, &c.subject, &c.action, &c.resource).await.unwrap();
    let compiled = pdp.decide_with_for_test(&ir.policy_set, &ir.role_names, &c.subject, &c.action, &c.resource).await.unwrap();
    if today.allow != compiled.allow {
        diffs.push(format!("[{}] action={} pdp={} egret-cedar={}", c.note, c.action, today.allow, compiled.allow));
    }
}
```

(`compare_state` iterates a state's questions through `compare_case`. Recompute `a_set`/`ir` once per state rather than per case for speed if preferred — both are pure over the current DB.)

- [ ] **Step 3: Seed rows using the existing test patterns.**

Model the seeding helpers on these existing files (reuse their exact INSERT column lists — do not invent schema):
- users / workspaces: `crates/egret-auth/tests/cedar_pdp.rs` (`seed_user`, `seed_workspace`).
- roles / role_permissions / access_grants (platform/workspace/object scope, allow/deny): `crates/egret-auth/tests/grant_store.rs`, `crates/egret-auth/tests/object_scope.rs`, `crates/egret-auth/tests/grant_row_semantics.rs`.
- `Subject` construction (user_id + group_ids): `crates/egret-auth/tests/cedar_pdp.rs` / `pdp_regression.rs`.

Draw role perms and question actions from `egret_core::rbac::PERMISSIONS` (and `ADMIN_GRAPH_ACTIONS` for admin-graph questions), bucketed via `egret_perm_ir::egret_object::classify`, so object-data, typed, and admin-graph actions are all exercised.

- [ ] **Step 4: Run the differential.**

Run: `CARGO_TARGET_DIR=target-eo2 DATABASE_URL=$DATABASE_URL cargo test -p egret-auth --test cedar_differential`
Expected: PASS — every case agrees (both paths share the formatters; the loader reproduces `load_inner`).

- [ ] **Step 5: Commit.**

```bash
git add crates/egret-auth/src/pdp/mod.rs crates/egret-auth/tests/cedar_differential.rs crates/egret-auth/Cargo.toml
git commit -m "test(cedar): decision-equivalence differential vs the live PDP"
```

---

### Task 6: Classification/clearance cases + teeth

**Files:**
- Modify: `crates/egret-auth/tests/cedar_differential.rs`

**Interfaces:**
- Consumes: Task 5's harness; `egret_cedar::compile::{compile, CompileInputs}`.

- [ ] **Step 1: Write the failing teeth test.**

Add to `crates/egret-auth/tests/cedar_differential.rs`:

```rust
/// Teeth: a deliberately broken compile (admin-graph forbid dropped) MUST make
/// the differential detect a divergence on an admin-graph action by a non-admin
/// — proving the harness is not vacuous.
#[tokio::test]
async fn differential_catches_a_dropped_admin_forbid() {
    let Some(db) = connect().await else {
        if std::env::var("EGRET_ALLOW_DB_SKIP").is_ok() { eprintln!("skip: no DB"); return; }
        panic!("no DB and EGRET_ALLOW_DB_SKIP unset");
    };
    let bus = EventBus::new();
    let pdp = CedarPdp::build(db.clone(), &bus).await.expect("pdp");

    // A non-admin user + a role granting an admin-graph action at platform scope.
    let case = admin_graph_case(&db).await; // seeds user/role/grant; action in ADMIN_GRAPH_ACTIONS

    let (a_set, a_names) = CedarPdp::compiled_policy_set_for_test(&db).await.unwrap();
    // Mutated compile: same inputs, but strip the admin-graph forbid.
    let inputs = egret_cedar::load::load_inputs(&db).await.unwrap();
    let mutated = compile_without_admin_forbid(&inputs);

    let today = pdp.decide_with_for_test(&a_set, &a_names, &case.subject, &case.action, &case.resource).await.unwrap();
    let broken = pdp.decide_with_for_test(&mutated.policy_set, &mutated.role_names, &case.subject, &case.action, &case.resource).await.unwrap();
    assert!(today.allow != broken.allow,
        "dropping the admin-graph forbid must change the decision for a non-admin on {}", case.action);
}
```

- [ ] **Step 2: Implement `compile_without_admin_forbid` + `admin_graph_case`.**

`compile_without_admin_forbid` builds a `CompileInputs`-equivalent set via `compile`, then removes the admin-graph `Policy` from the returned `PolicySet` (`policy_set.remove(&"policy…".parse().unwrap())` — the admin-graph forbid is the one static policy that is not a `classification:{tag}`). Simplest robust approach: re-build the set by calling `compile` and then `policy_set.remove_static` for every non-`classification:` static policy id. `admin_graph_case` seeds a non-admin user, a role with one `ADMIN_GRAPH_ACTIONS` key, a platform/workspace grant, and returns a `Case` whose action is that key.

- [ ] **Step 3: Extend the main differential corpus with clearance cases.**

In `draw_state` (Task 5), also seed: a `tags` row + `classification_policies` row (required_roles and/or required_groups), and `tag_assignments` attaching the tag to some objects; draw users that do/don't hold the required role/group; add questions reading those tagged objects. Seed modeled on `crates/egret-auth/tests/classification_policy.rs` (reuse its exact INSERT column lists for `tags`, `classification_policies`, `tag_assignments`). This makes the `egret_cedar_decisions_agree_with_the_pdp` corpus cover the clearance/withhold fold (EO-D4).

- [ ] **Step 4: Add deterministic clearance anchors.**

Add to `anchors()`: (a) a cleared-by-role principal reading a tagged object → allow on both; (b) an uncleared principal → deny on both; (c) a platform_admin → allow on both; (d) a cleared-by-group principal → allow on both.

- [ ] **Step 5: Run both differential tests.**

Run: `CARGO_TARGET_DIR=target-eo2 DATABASE_URL=$DATABASE_URL cargo test -p egret-auth --test cedar_differential`
Expected: `egret_cedar_decisions_agree_with_the_pdp` PASS (now including clearance cases); `differential_catches_a_dropped_admin_forbid` PASS (the mutation is caught).

- [ ] **Step 6: Commit.**

```bash
git add crates/egret-auth/tests/cedar_differential.rs
git commit -m "test(cedar): clearance differential cases + teeth (dropped admin forbid)"
```

---

## Global verification (run before finishing the branch)

```bash
CARGO_TARGET_DIR=target-eo2 cargo fmt --all
CARGO_TARGET_DIR=target-eo2 cargo clippy -p egret-cedar -p egret-perm-ir -p egret-auth --all-targets -- -D warnings
CARGO_TARGET_DIR=target-eo2 cargo test -p egret-cedar -p egret-perm-ir
CARGO_TARGET_DIR=target-eo2 DATABASE_URL=$DATABASE_URL cargo test -p egret-auth --test cedar_differential -p egret-cedar
```

Expected: fmt clean; clippy clean; all unit tests green; the differential (and teeth) green against a real DB.

## Self-Review

**Spec coverage:** Scope item "egret-cedar crate rendering the PolicySet" → Tasks 1–3; "capability-matrix entry" → Task 4; "classification/clearance fold" → Task 6 (corpus + anchors) exercising the formatter that already emits it; "decision-equivalence differential" → Tasks 5–6; "zero behaviour change" → Task 1 keeps the PDP byte-identical and Task 5's seam is a behaviour-preserving refactor. EO2-D1 (reuse formatters) → Task 1. EO2-D2 (seeded-DB Cedar-vs-Cedar, in egret-auth) → Tasks 5–6. EO2-D3 (standalone matrix, NotApplicable) → Task 4. EO2-D4 (one action catalog via classify) → Tasks 4–6. EO2-D5 (no new policy decisions) → the differential is the gate.

**Placeholder scan:** The only deferred detail is the exact INSERT column lists for seeding, which are deliberately delegated to named existing test files (reusing verified schema beats re-typing it and risking plan-prose-wrong) — every piece of NEW logic (compile, loader, matrix, harness skeleton, comparison, teeth) is given as real code.

**Type consistency:** `CompileInputs`/`RoleInput`/`ClassificationInput`/`Compiled` are defined in Task 2 and consumed unchanged in Tasks 3/5/6; `compiled_policy_set_for_test`/`decide_with_for_test` signatures are fixed in Task 5 and called as written in Tasks 5/6; `egret_cedar_support`/`gap` signatures in Task 4 match their tests; `ReasonCode::NotApplicable` added in Task 4 before use.

**Deferred to EO3/EO4 (not this plan):** wiring `compile` into `CedarPdp::load_inner` behind a reversible switch (EO3); folding `access_grants` into `perm.*` (EO4). EO2 adds the compiler, the matrix entry, and the proof only.
