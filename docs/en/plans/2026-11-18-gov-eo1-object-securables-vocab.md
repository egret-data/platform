# EO1 — Object securables in the IR + vocabulary mapping Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a pure, target-neutral representation of egret's object securables and an authoritative mapping of every `rbac` permission key onto the common grant vocabulary, as the foundation for compiling Cedar from the one grant model.

**Architecture:** One new module `egret_object` in the pure `egret-perm-ir` crate. It defines the object-securable kinds, their tree shape, and a `classify` function that partitions every `egret_core::rbac` permission key into an object-data grant `(kind, privilege)`, a typed enforcement-point privilege, or a non-grantable admin-graph forbid. The module is additive: no existing code path calls it, there is no migration, and nothing is wired to the runtime PDP or to any compiler. Behaviour is unchanged.

**Tech Stack:** Rust, `egret-perm-ir` (pure, no I/O), re-using `egret_perm_ir::statement::Privilege` and `egret_core::rbac`.

## Global Constraints

- **Pure, zero behaviour change.** No new dependency in `crates/egret-perm-ir/Cargo.toml` (it already has `egret-perm`, `egret-core`, `uuid`, `tracing`). No I/O, no sqlx, no migration. The new module must be referenced by **no** existing code path. `crates/egret-perm-ir/tests/boundary.rs` must stay green (no `egret-policy`/`egret-policy-api`/`egret-ranger-eval` dep, no "Ranger" in a public type name — none of this plan adds any).
- **Re-use the existing privilege enum.** Use `crate::statement::Privilege` (`Browse`, `Read`, `Write`, `Create`, `Manage`). Do **not** define a new privilege type.
- **The seven object kinds (EO-D6), exactly:** `dashboard`, `chart`, `dataset`, `folder`, `saved_query`, `app`, `data_api_endpoint`. No `notebook`, `workflow` or `alert`.
- **The vocabulary mapping (EO-D3), exactly:**
  - Object-data grant keys (13): `dashboard:read→(Dashboard,Read)`, `dashboard:manage→(Dashboard,Manage)`, `chart:read→(Chart,Read)`, `chart:manage→(Chart,Manage)`, `dataset:read→(Dataset,Read)`, `dataset:manage→(Dataset,Manage)`, `saved_query:read→(SavedQuery,Read)`, `saved_query:manage→(SavedQuery,Manage)`, `app:read→(App,Read)`, `app:write→(App,Write)`, `app:manage→(App,Manage)`, `data_api_endpoint:read→(DataApiEndpoint,Read)`, `data_api_endpoint:manage→(DataApiEndpoint,Manage)`.
  - Typed EP privileges: every other key in `rbac::PERMISSIONS`, including `dashboard:certify`, `dataset:certify`, and `data_api_endpoint:invoke`.
  - Admin-graph forbids: every key in `rbac::ADMIN_GRAPH_ACTIONS` (these are **not** in `PERMISSIONS`).
  - `folder` has **no** permission key of its own; no key maps to `ObjectKind::Folder`.
  - `Create` and `Browse` are **not** produced by `classify`: there are no per-object create/browse keys. `Create` is implied at the parent and `Browse` by any object-data grant; both are derived later (EO2), not mapped here.
- **The exhaustiveness test is a chokepoint.** Adding a new `rbac` key must break the EO1 test suite until the author classifies it (see Task 5). Keep that property.
- Conventional Commits; commit messages, code and comments in English. Co-author line on every commit: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. Stage by explicit path; never `git add -A`.

---

## File Structure

- Create: `crates/egret-perm-ir/src/egret_object.rs` — the whole module (kinds, tree, mapping, `classify`).
- Modify: `crates/egret-perm-ir/src/lib.rs` — add `pub mod egret_object;`.
- Tests live inline in `egret_object.rs` under `#[cfg(test)]` (the crate's convention; `statement.rs`, `matrix.rs` do the same).

---

### Task 1: `ObjectKind` enum

**Files:**
- Modify: `crates/egret-perm-ir/src/egret_object.rs` (create)
- Modify: `crates/egret-perm-ir/src/lib.rs`

**Interfaces:**
- Produces: `pub enum ObjectKind { Dashboard, Chart, Dataset, Folder, SavedQuery, App, DataApiEndpoint }`; `impl ObjectKind { pub fn as_str(&self) -> &'static str; pub fn from_str_opt(s: &str) -> Option<ObjectKind>; }`. The string spellings match the DB/Cedar spellings used in `egret-bi/src/shares.rs` and `egret-dataapi-api` (`"dashboard"`, `"chart"`, `"dataset"`, `"folder"`, `"saved_query"`, `"app"`, `"data_api_endpoint"`).

- [ ] **Step 1: Create the module file with the enum and the failing test**

```rust
//! EO1: the pure, target-neutral representation of egret's object securables
//! and the mapping of `egret_core::rbac` permission keys onto the common grant
//! vocabulary. Additive and unused by any existing code path: no I/O, no
//! migration, no compiler or runtime wiring (that is EO2–EO4). See
//! docs/en/specs/2026-11-18-gov-eo0-egret-objects-decomposition-design.md.

/// An egret object kind that is a securable (EO-D6). The string spellings are
/// the ones the control plane already uses in `egret-bi::shares` and the
/// Data API crate, so they line up with the Cedar `Object` id `"{kind}:{uuid}"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObjectKind {
    Dashboard,
    Chart,
    Dataset,
    Folder,
    SavedQuery,
    App,
    DataApiEndpoint,
}

impl ObjectKind {
    /// Every kind, for exhaustive tests and iteration.
    pub const ALL: [ObjectKind; 7] = [
        ObjectKind::Dashboard,
        ObjectKind::Chart,
        ObjectKind::Dataset,
        ObjectKind::Folder,
        ObjectKind::SavedQuery,
        ObjectKind::App,
        ObjectKind::DataApiEndpoint,
    ];

    /// The control-plane string spelling of this kind.
    pub fn as_str(&self) -> &'static str {
        match self {
            ObjectKind::Dashboard => "dashboard",
            ObjectKind::Chart => "chart",
            ObjectKind::Dataset => "dataset",
            ObjectKind::Folder => "folder",
            ObjectKind::SavedQuery => "saved_query",
            ObjectKind::App => "app",
            ObjectKind::DataApiEndpoint => "data_api_endpoint",
        }
    }

    /// Parse a control-plane kind string; `None` if it is not a securable kind.
    pub fn from_str_opt(s: &str) -> Option<ObjectKind> {
        ObjectKind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_kind_str_round_trips() {
        for k in ObjectKind::ALL {
            assert_eq!(ObjectKind::from_str_opt(k.as_str()), Some(k));
        }
        assert_eq!(ObjectKind::from_str_opt("notebook"), None);
        assert_eq!(ObjectKind::from_str_opt("workflow"), None);
        assert_eq!(ObjectKind::from_str_opt(""), None);
    }
}
```

- [ ] **Step 2: Wire the module in `lib.rs`**

Add, in alphabetical position among the `pub mod` lines (after `pub mod audience;` / before `pub mod enforcement;` is fine — match the existing ordering):

```rust
pub mod egret_object;
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p egret-perm-ir egret_object::tests::object_kind_str_round_trips`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-perm-ir/src/egret_object.rs crates/egret-perm-ir/src/lib.rs
git commit -m "feat(gov): EO1 — ObjectKind enum for egret-object securables"
```

---

### Task 2: The object-securable tree

**Files:**
- Modify: `crates/egret-perm-ir/src/egret_object.rs`

**Interfaces:**
- Consumes: `ObjectKind` (Task 1), `uuid::Uuid`.
- Produces: `pub enum EgretSecurable { Platform, Workspace { id: Uuid }, Folder { id: Uuid, parent: Parent }, Object { kind: ObjectKind, id: Uuid, parent: Parent } }`; `pub enum Parent { Folder(Uuid), Workspace(Uuid) }`; `impl EgretSecurable { pub fn parent(&self) -> Option<Parent>; }`. This is the looser tree of EO0 (`Platform → Workspace → Folder* → Object`); a folder's parent is another folder or a workspace, an object's parent is a folder or a workspace, a workspace's parent is the platform.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `egret_object.rs`:

```rust
    #[test]
    fn securable_parents() {
        let ws = Uuid::from_u128(1);
        let folder = Uuid::from_u128(2);
        let dash = Uuid::from_u128(3);

        assert_eq!(EgretSecurable::Platform.parent(), None);
        // A workspace's parent is the platform root, reported as None (there is
        // no Parent::Platform variant). `Parent::Workspace`/`Parent::Folder`
        // name a node's parent only when that parent is a workspace or folder.
        assert_eq!(EgretSecurable::Workspace { id: ws }.parent(), None);
        assert_eq!(
            EgretSecurable::Folder { id: folder, parent: Parent::Workspace(ws) }.parent(),
            Some(Parent::Workspace(ws)),
        );
        assert_eq!(
            EgretSecurable::Object {
                kind: ObjectKind::Dashboard,
                id: dash,
                parent: Parent::Folder(folder),
            }
            .parent(),
            Some(Parent::Folder(folder)),
        );
    }
```

Add `use uuid::Uuid;` to the `tests` module imports.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p egret-perm-ir egret_object::tests::securable_parents`
Expected: FAIL to compile (`EgretSecurable`/`Parent` not defined).

- [ ] **Step 3: Implement the tree types**

Add to `egret_object.rs` (above the `#[cfg(test)]` module), with `use uuid::Uuid;` at the top of the file:

```rust
use uuid::Uuid;

/// The parent of a securable in the egret-object tree. A `Workspace` parent
/// means the node hangs directly under its workspace (un-foldered objects and
/// top-level folders); a `Folder` parent means it is nested in that folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Parent {
    Folder(Uuid),
    Workspace(Uuid),
}

/// A node in the egret-object securable tree (EO0):
/// `Platform → Workspace → Folder* → Object`. Looser than the data tree:
/// folders nest, objects may or may not be foldered, and some foldered objects
/// live in a cross-schema folder set. This is a pure shape; owner and grant
/// resolution land in later sub-projects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EgretSecurable {
    Platform,
    Workspace { id: Uuid },
    Folder { id: Uuid, parent: Parent },
    Object { kind: ObjectKind, id: Uuid, parent: Parent },
}

impl EgretSecurable {
    /// The securable's parent, or `None` for the platform root. A workspace's
    /// parent is the platform, expressed here as `None` is reserved for the
    /// root, so a workspace reports itself via `Parent::Workspace` only when it
    /// is the *parent* of another node; a `Workspace` node's own parent is the
    /// platform and therefore `None`.
    pub fn parent(&self) -> Option<Parent> {
        match self {
            EgretSecurable::Platform => None,
            EgretSecurable::Workspace { .. } => None,
            EgretSecurable::Folder { parent, .. } => Some(*parent),
            EgretSecurable::Object { parent, .. } => Some(*parent),
        }
    }
}
```

- [ ] **Step 4: Run the test**

Run: `cargo test -p egret-perm-ir egret_object::tests::securable_parents`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-perm-ir/src/egret_object.rs
git commit -m "feat(gov): EO1 — egret-object securable tree (Platform/Workspace/Folder/Object)"
```

---

### Task 3: `classify` — object-data grants

**Files:**
- Modify: `crates/egret-perm-ir/src/egret_object.rs`

**Interfaces:**
- Consumes: `ObjectKind` (Task 1), `crate::statement::Privilege`.
- Produces: `pub enum PermissionMapping { ObjectGrant { kind: ObjectKind, privilege: Privilege }, TypedEpPrivilege, AdminGraphForbid }`; `pub fn classify(key: &str) -> Option<PermissionMapping>`. In this task `classify` handles only the 13 object-data grant keys and returns `None` for everything else (completed in Task 4).

- [ ] **Step 1: Write the failing test**

Add to the `tests` module:

```rust
    use crate::statement::Privilege;

    #[test]
    fn classify_object_data_grants() {
        use ObjectKind::*;
        use Privilege::*;
        let cases: &[(&str, ObjectKind, Privilege)] = &[
            ("dashboard:read", Dashboard, Read),
            ("dashboard:manage", Dashboard, Manage),
            ("chart:read", Chart, Read),
            ("chart:manage", Chart, Manage),
            ("dataset:read", Dataset, Read),
            ("dataset:manage", Dataset, Manage),
            ("saved_query:read", SavedQuery, Read),
            ("saved_query:manage", SavedQuery, Manage),
            ("app:read", App, Read),
            ("app:write", App, Write),
            ("app:manage", App, Manage),
            ("data_api_endpoint:read", DataApiEndpoint, Read),
            ("data_api_endpoint:manage", DataApiEndpoint, Manage),
        ];
        assert_eq!(cases.len(), 13);
        for (key, kind, priv_) in cases {
            assert_eq!(
                classify(key),
                Some(PermissionMapping::ObjectGrant { kind: *kind, privilege: *priv_ }),
                "key {key}",
            );
        }
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p egret-perm-ir egret_object::tests::classify_object_data_grants`
Expected: FAIL to compile (`PermissionMapping`/`classify` not defined).

- [ ] **Step 3: Implement `PermissionMapping` and the object-data arm of `classify`**

Add to `egret_object.rs` (with `use crate::statement::Privilege;` at the top):

```rust
use crate::statement::Privilege;

/// Where a permission key sits in the one grant model (EO-D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMapping {
    /// An object-data grant on a securable of this kind.
    ObjectGrant { kind: ObjectKind, privilege: Privilege },
    /// A typed privilege on the `egret-cedar` enforcement point (an
    /// entitlement, not a securable grant): engine/catalog/workspace/etc.
    /// capabilities and non-data object verbs (`certify`, `invoke`).
    TypedEpPrivilege,
    /// A non-delegatable admin-graph action the PDP `forbid`s for everyone but
    /// `platform_admin`. Never a grant. These are `rbac::ADMIN_GRAPH_ACTIONS`,
    /// which are not in `rbac::PERMISSIONS`.
    AdminGraphForbid,
}

/// Classify an `egret_core::rbac` permission key (either a `PERMISSIONS` key or
/// an `ADMIN_GRAPH_ACTIONS` key) into the one grant model. `None` for a string
/// that is neither — callers treat an unknown key as un-mappable (fail closed).
pub fn classify(key: &str) -> Option<PermissionMapping> {
    // Object-data grants: the only keys that become securable grants (EO-D3).
    let object_grant = |kind: ObjectKind, privilege: Privilege| {
        Some(PermissionMapping::ObjectGrant { kind, privilege })
    };
    match key {
        "dashboard:read" => return object_grant(ObjectKind::Dashboard, Privilege::Read),
        "dashboard:manage" => return object_grant(ObjectKind::Dashboard, Privilege::Manage),
        "chart:read" => return object_grant(ObjectKind::Chart, Privilege::Read),
        "chart:manage" => return object_grant(ObjectKind::Chart, Privilege::Manage),
        "dataset:read" => return object_grant(ObjectKind::Dataset, Privilege::Read),
        "dataset:manage" => return object_grant(ObjectKind::Dataset, Privilege::Manage),
        "saved_query:read" => return object_grant(ObjectKind::SavedQuery, Privilege::Read),
        "saved_query:manage" => return object_grant(ObjectKind::SavedQuery, Privilege::Manage),
        "app:read" => return object_grant(ObjectKind::App, Privilege::Read),
        "app:write" => return object_grant(ObjectKind::App, Privilege::Write),
        "app:manage" => return object_grant(ObjectKind::App, Privilege::Manage),
        "data_api_endpoint:read" => {
            return object_grant(ObjectKind::DataApiEndpoint, Privilege::Read)
        }
        "data_api_endpoint:manage" => {
            return object_grant(ObjectKind::DataApiEndpoint, Privilege::Manage)
        }
        _ => {}
    }
    // Typed vs admin-graph handled in Task 4.
    None
}
```

- [ ] **Step 4: Run the test**

Run: `cargo test -p egret-perm-ir egret_object::tests::classify_object_data_grants`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-perm-ir/src/egret_object.rs
git commit -m "feat(gov): EO1 — classify object-data grant permission keys"
```

---

### Task 4: `classify` — typed privileges, admin-graph, and exhaustiveness chokepoint

**Files:**
- Modify: `crates/egret-perm-ir/src/egret_object.rs`

**Interfaces:**
- Consumes: `egret_core::rbac` (`PERMISSIONS`, `ADMIN_GRAPH_ACTIONS`, `is_admin_graph`), the Task 3 `classify`.
- Produces: the completed `classify` — every `rbac::PERMISSIONS` key is `Some` (object grant or typed), every `rbac::ADMIN_GRAPH_ACTIONS` key is `Some(AdminGraphForbid)`.

- [ ] **Step 1: Write the failing exhaustiveness test**

Add to the `tests` module:

```rust
    use egret_core::rbac;

    #[test]
    fn every_permissions_key_is_classified() {
        // Count guard (chokepoint): adding an rbac permission trips this until
        // the author classifies the new key as an object grant or a typed EP
        // privilege. If you add a key and this fails, update classify() and
        // this count, deciding deliberately which bucket the key belongs in.
        const OBJECT_GRANT_KEYS: usize = 13;

        let mut object_grants = 0usize;
        for p in rbac::PERMISSIONS {
            match classify(p.key) {
                Some(PermissionMapping::ObjectGrant { .. }) => object_grants += 1,
                Some(PermissionMapping::TypedEpPrivilege) => {}
                other => panic!("rbac key {} classified as {other:?}", p.key),
            }
        }
        assert_eq!(object_grants, OBJECT_GRANT_KEYS, "object-grant key count drifted");
    }

    #[test]
    fn admin_graph_actions_are_forbids() {
        for key in rbac::ADMIN_GRAPH_ACTIONS {
            assert_eq!(
                classify(key),
                Some(PermissionMapping::AdminGraphForbid),
                "admin-graph key {key}",
            );
        }
    }

    #[test]
    fn non_data_object_verbs_are_typed() {
        for key in ["dashboard:certify", "dataset:certify", "data_api_endpoint:invoke"] {
            assert_eq!(classify(key), Some(PermissionMapping::TypedEpPrivilege), "key {key}");
        }
    }

    #[test]
    fn unknown_key_is_none() {
        assert_eq!(classify("nonsense:frobnicate"), None);
        assert_eq!(classify(""), None);
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p egret-perm-ir egret_object::tests::`
Expected: `admin_graph_actions_are_forbids`, `non_data_object_verbs_are_typed`, and `every_permissions_key_is_classified` FAIL (admin-graph and typed keys currently return `None`).

- [ ] **Step 3: Complete `classify`**

Replace the trailing `// Typed vs admin-graph handled in Task 4.` / `None` of `classify` with:

```rust
    // Admin-graph actions are forbids, never grants. These are not in
    // `PERMISSIONS`; check them explicitly.
    if rbac::is_admin_graph(key) {
        return Some(PermissionMapping::AdminGraphForbid);
    }
    // Every other known permission is a typed EP privilege (EO-D3): engine,
    // catalog, storage, secret, policy, tag, workspace, user, group, role,
    // service_account, workflow, alert, query, and the non-data object verbs
    // (certify, invoke).
    if rbac::is_known(key) {
        return Some(PermissionMapping::TypedEpPrivilege);
    }
    None
```

Add `use egret_core::rbac;` at the top of `egret_object.rs`.

- [ ] **Step 4: Run the whole module's tests**

Run: `cargo test -p egret-perm-ir egret_object`
Expected: PASS (all tests).

- [ ] **Step 5: Commit**

```bash
git add crates/egret-perm-ir/src/egret_object.rs
git commit -m "feat(gov): EO1 — classify typed privileges + admin-graph, with exhaustiveness guard"
```

---

### Task 5: Module rustdoc, lint, and boundary verification

**Files:**
- Modify: `crates/egret-perm-ir/src/egret_object.rs` (doc only)

**Interfaces:** none new.

- [ ] **Step 1: Add a module-level doc example and a note on the chokepoint**

Ensure the module header (from Task 1) is followed by a short rustdoc example that compiles, demonstrating `classify`:

```rust
/// ```
/// use egret_perm_ir::egret_object::{classify, PermissionMapping, ObjectKind};
/// use egret_perm_ir::statement::Privilege;
///
/// assert_eq!(
///     classify("dashboard:read"),
///     Some(PermissionMapping::ObjectGrant {
///         kind: ObjectKind::Dashboard,
///         privilege: Privilege::Read,
///     }),
/// );
/// assert_eq!(classify("engine:read"), Some(PermissionMapping::TypedEpPrivilege));
/// assert_eq!(classify("role:create"), Some(PermissionMapping::AdminGraphForbid));
/// ```
```

Place this doc comment on the `classify` function (doctests require the item to be public, which it is).

- [ ] **Step 2: Confirm the module is referenced by no existing code path**

Run: `grep -rn "egret_object" crates --include=*.rs | grep -v "crates/egret-perm-ir/src/egret_object.rs" | grep -v "crates/egret-perm-ir/src/lib.rs"`
Expected: no output (nothing outside the module and its `pub mod` line uses it — confirms zero behaviour change).

- [ ] **Step 3: Run lints, format, the crate's tests, and the boundary test**

Run:
```bash
cargo fmt -p egret-perm-ir
cargo clippy -p egret-perm-ir --all-targets -- -D warnings
cargo test -p egret-perm-ir
cargo test -p egret-perm-ir --doc
```
Expected: all PASS, including `tests/boundary.rs`.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-perm-ir/src/egret_object.rs
git commit -m "docs(gov): EO1 — classify doctest and module docs"
```

---

## Self-Review

- **Spec coverage.** EO0's EO1 asks for: the object tree in the neutral IR (Task 2), the vocabulary mapping of object-data permissions (Task 3), and the grant-vs-typed classification of every `rbac` key (Task 4). EO0 also mentions representing the tree "in the perm model"; the **DB/model** side (tables, migration) is deliberately deferred to EO4 (the store merge), because adding model tables now would be unused and would not be zero-behaviour-change. If the reviewer wants a model-side type in EO1, that is a scope change to raise before execution.
- **Placeholder scan.** No TBD/TODO; every code step has complete code.
- **Type consistency.** `Privilege` is `crate::statement::Privilege` throughout; `ObjectKind` spellings match `egret-bi/src/shares.rs` and the Data API `OBJECT_KIND`; `classify` signature is stable across Tasks 3–5. `EgretSecurable::parent()` is platform-rooted: a `Workspace` node's parent is the platform, reported as `None` (there is no `Parent::Platform`).
- **Chokepoint.** `every_permissions_key_is_classified` panics on any `None` and pins the object-grant count, so a new `rbac` key forces a deliberate classification — the exhaustiveness-guard-as-chokepoint the constraints require.
