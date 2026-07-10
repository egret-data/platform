# B1b-1 — Wire PDP into serving + unified `require()` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Construct the hot-reloading Cedar PDP in the running app from the SAME event bus as `RoleRepo` (closes review finding F1), introduce a single `require()` PEP over `Principal`/`WorkspaceContext`, and migrate the `secrets` seam off the legacy static gate as proof — behavior-preserving.

**Architecture:** `egret_core::authz` gains an `AuthTarget` trait (maps `&Principal`→`(Subject, Platform)` and `&WorkspaceContext`→`(Subject, Workspace(id))`) and an async `require(pdp, target, action)`. `Principal` carries `group_ids` (loaded at session validate). `egret-bin` builds `CedarPdp` from the shared bus as `Arc<dyn Pdp>` and injects it into the seam state that enforces (`CpState` for secrets). Only the `secrets` seam is migrated here.

**Tech Stack:** Rust, axum, `sqlx`/Postgres, `cedar-policy` + `egret-events` (from B1a/B1b-0).

**Reference:** [B1b-1 design](../specs/2026-07-10-b1b-1-pdp-wiring-require-design.md).

---

## File Structure

- `crates/egret-core/src/context.rs` (modify) — add `group_ids: Vec<GroupId>` to `Principal`.
- `crates/egret-core/src/authz.rs` (modify) — `AuthTarget` trait + impls + `require()`.
- `crates/egret-auth/src/session.rs` (modify) — populate `Principal.group_ids` in `validate`.
- `crates/egret-controlplane/src/lib.rs` (modify) — `CpState` gains `pdp: Arc<dyn Pdp>`.
- `crates/egret-controlplane/src/secrets.rs` (modify) — 4 handlers call `require()`.
- `crates/egret-bin/src/main.rs` (modify) — build `CedarPdp` from the shared `bus`, inject into `CpState`.
- `crates/egret-server/src/lib.rs` (modify only if `AppState`/`FromRef` needs the pdp — see Task 3).
- Tests: `crates/egret-core/src/authz.rs` (inline), `crates/egret-auth/tests/pdp_app_wiring.rs` (F1 e2e), secrets tests.

Principal literal construction sites to fix after adding the field (grep `Principal {` across the workspace): `crates/egret-auth/src/session.rs`, `crates/egret-auth/src/authz.rs` (test), `crates/egret-controlplane/src/auth.rs` (test), `crates/egret-core/src/context.rs` (test), and any test helpers. Each gets `group_ids: vec![]` unless the test needs real groups.

---

## Task 1: `egret-core` — `Principal.group_ids` + `AuthTarget` + `require()`

**Files:**
- Modify: `crates/egret-core/src/context.rs`, `crates/egret-core/src/authz.rs`
- Test: inline `#[cfg(test)]` in `authz.rs`

- [ ] **Step 1: Add `group_ids` to `Principal`**

In `crates/egret-core/src/context.rs`, add to `Principal`:
```rust
pub struct Principal {
    pub user_id: UserId,
    pub email: String,
    pub platform_roles: Vec<String>,
    pub permissions: Vec<String>,
    /// Group ids the user belongs to (direct memberships), for the PDP subject.
    pub group_ids: Vec<crate::ids::GroupId>,
}
```
Update the `ctx(...)` test helper in that file's `#[cfg(test)]` to set `group_ids: vec![]`.

- [ ] **Step 2: Run build to find every broken `Principal` literal**

Run: `rtk proxy cargo build --workspace 2>&1 | rg "missing field .group_ids." -n` (or just read the errors). Add `group_ids: vec![]` to each `Principal { ... }` literal the compiler flags (session.rs is fixed in Task 2 with real ids; for now `vec![]` there too so it compiles). Do NOT skip any — the workspace must build after this step (it will still have the old authz behavior; that's fine).

- [ ] **Step 3: Write the failing `AuthTarget`/`require` test**

Append to `crates/egret-core/src/authz.rs`:
```rust
/// A request context that resolves to a PDP subject + the resource scope of the
/// action. Implemented by the request-context shapes so `require` accepts either.
pub trait AuthTarget {
    fn subject(&self) -> Subject;
    fn resource(&self) -> Resource;
}

impl AuthTarget for crate::Principal {
    fn subject(&self) -> Subject {
        Subject { user_id: self.user_id, group_ids: self.group_ids.clone() }
    }
    fn resource(&self) -> Resource {
        Resource::Platform
    }
}

impl AuthTarget for crate::WorkspaceContext {
    fn subject(&self) -> Subject {
        self.principal.subject()
    }
    fn resource(&self) -> Resource {
        Resource::Workspace(self.workspace_id)
    }
}

/// Enforce `action` for `target`: `Ok(())` if the PDP allows, `Error::Forbidden` otherwise.
pub async fn require(
    pdp: &dyn Pdp,
    target: &impl AuthTarget,
    action: &str,
) -> crate::Result<()> {
    if pdp.authorize(&target.subject(), action, &target.resource()).await?.allow {
        Ok(())
    } else {
        Err(crate::Error::Forbidden)
    }
}

#[cfg(test)]
mod require_tests {
    use super::*;
    use crate::ids::{UserId, WorkspaceId};

    struct StubPdp {
        allow: bool,
    }
    #[async_trait::async_trait]
    impl Pdp for StubPdp {
        async fn authorize(&self, _s: &Subject, _a: &str, _r: &Resource) -> crate::Result<Decision> {
            Ok(Decision { allow: self.allow, reasons: vec![] })
        }
    }

    fn principal() -> crate::Principal {
        crate::Principal {
            user_id: UserId::new(),
            email: "a@b.c".into(),
            platform_roles: vec![],
            permissions: vec![],
            group_ids: vec![crate::ids::GroupId::new()],
        }
    }

    #[test]
    fn principal_targets_platform_and_carries_groups() {
        let p = principal();
        assert!(matches!(p.resource(), Resource::Platform));
        assert_eq!(p.subject().group_ids.len(), 1);
    }

    #[test]
    fn workspace_context_targets_its_workspace() {
        let ws = WorkspaceId::new();
        let ctx = crate::WorkspaceContext { workspace_id: ws, principal: principal(), workspace_role: "ws_viewer".into() };
        match ctx.resource() {
            Resource::Workspace(w) => assert_eq!(w, ws),
            _ => panic!("expected workspace resource"),
        }
        assert_eq!(ctx.subject().group_ids.len(), 1);
    }

    #[tokio::test]
    async fn require_allows_and_forbids() {
        let p = principal();
        assert!(require(&StubPdp { allow: true }, &p, "x:y").await.is_ok());
        assert!(matches!(
            require(&StubPdp { allow: false }, &p, "x:y").await,
            Err(crate::Error::Forbidden)
        ));
    }
}
```
Re-export `AuthTarget` and `require` from `crates/egret-core/src/lib.rs` alongside the existing `authz` re-exports (`pub use authz::{require, AuthTarget, ...};`). Ensure `tokio` is a dev-dependency of `egret-core` for the `#[tokio::test]` (check; add `tokio = { workspace = true }` under `[dev-dependencies]` if missing).

- [ ] **Step 4: Run the tests**

Run: `rtk proxy cargo test -p egret-core authz` → PASS (the three new tests + existing). Then `rtk proxy cargo build --workspace` → SUCCESS.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-core/src/context.rs crates/egret-core/src/authz.rs crates/egret-core/src/lib.rs crates/egret-core/Cargo.toml
git add -u  # the Principal-literal fixes across crates
git commit -m "feat(authz): Principal.group_ids + AuthTarget + require() PEP in egret-core

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```
(Review `git status` before committing — only `group_ids: vec![]` additions to Principal literals should be in the `-u` set, nothing else.)

---

## Task 2: Populate `Principal.group_ids` at session validate

**Files:**
- Modify: `crates/egret-auth/src/session.rs`
- Test: `crates/egret-auth/tests/session_db.rs` (extend) or a new assertion

- [ ] **Step 1: Load group ids in `validate`**

Read `crates/egret-auth/src/session.rs` around the `Principal { ... }` construction (~line 104) and the existing `permissions(user_id)` helper (which queries `user_groups`). Add a sibling query (or fold into the existing flow) that returns the user's group ids:
```rust
async fn group_ids(&self, user_id: UserId) -> Result<Vec<egret_core::ids::GroupId>> {
    let rows: Vec<(uuid::Uuid,)> =
        sqlx::query_as("SELECT group_id FROM user_groups WHERE user_id = $1")
            .bind(user_id.0)
            .fetch_all(&self.db.pool)
            .await
            .map_err(db_err)?;
    Ok(rows.into_iter().map(|(g,)| egret_core::ids::GroupId(g)).collect())
}
```
(Match the crate's `db_err`/error style; confirm `GroupId`'s field is `.0` / has a tuple constructor — if it's `GroupId(uuid)` use that, else adapt.) Then in `validate`, set `group_ids: self.group_ids(user_id).await?` on the returned `Principal` (replace the temporary `vec![]` from Task 1).

- [ ] **Step 2: Test it**

Extend `crates/egret-auth/tests/session_db.rs` (DB-gated): create a user, a group, add the user to the group (`user_groups`), create a session, `validate` the token, assert the returned `Principal.group_ids` contains the group id. Use the existing harness/seed helpers in that file.

- [ ] **Step 3: Run**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-auth --test session_db -- --nocapture` → PASS, confirm it RAN.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-auth/src/session.rs crates/egret-auth/tests/session_db.rs
git commit -m "feat(authz): SessionService loads Principal.group_ids for the PDP subject

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Inject the shared PDP into `CpState` + F1 wiring test

**Files:**
- Modify: `crates/egret-controlplane/src/lib.rs` (`CpState`), `crates/egret-bin/src/main.rs`, `crates/egret-controlplane/Cargo.toml` (dep on `egret-auth` for `CedarPdp` at the bin? no — see below)
- Test: `crates/egret-auth/tests/pdp_app_wiring.rs` (new)

- [ ] **Step 1: Add `pdp` to `CpState`**

In `crates/egret-controlplane/src/lib.rs`, add to `CpState`:
```rust
pub pdp: std::sync::Arc<dyn egret_core::authz::Pdp>,
```
`CpState` derives/uses `Clone` — `Arc<dyn Pdp>` is `Clone`, fine. Confirm `egret-controlplane` already depends on `egret-core` (it does).

- [ ] **Step 2: Build the PDP from the shared bus in `egret-bin` and inject it**

In `crates/egret-bin/src/main.rs`, after `let bus = egret_events::EventBus::new();` and after `db` is available, build the PDP ONCE from that SAME bus:
```rust
// Cedar PDP — built from the SAME bus as RoleRepo so RolesChanged hot-reloads it (B1b-0 F1).
let pdp: std::sync::Arc<dyn egret_core::authz::Pdp> =
    std::sync::Arc::new(egret_auth::pdp::CedarPdp::build(db.clone(), &bus).await?);
```
Then set `pdp: pdp.clone()` in the `CpState { ... }` construction. Add `egret-events`/`egret-auth` deps to `egret-bin` as needed (both already present from B1b-0). If `CedarPdp::build` returns `Result` with an error type the `?` needs mapping, map it as the surrounding code maps other fallible setup calls.

- [ ] **Step 3: Write the F1 app-wiring regression test**

`crates/egret-auth/tests/pdp_app_wiring.rs` (DB-gated). This is the guard that the PDP and `RoleRepo` share one bus so hot-reload actually fires end-to-end through the *pdp handle the app would use*:
```rust
// let bus = EventBus::new();
// let pdp: Arc<dyn Pdp> = Arc::new(CedarPdp::build(db.clone(), &bus).await.unwrap());
// let repo = RoleRepo::new(db.clone(), bus.clone());   // SAME bus
// Seed U with a fresh zero-perm workspace role R in workspace W (workspace_members row).
// let subject = Subject { user_id: U, group_ids: vec![] };
// assert!(!pdp.authorize(&subject, "dashboard:read", &Resource::Workspace(W)).await.unwrap().allow);
// repo.set_permissions(R_id, &["dashboard:read".into()]).await.unwrap();
// poll_until(|| async { pdp.authorize(&subject, "dashboard:read", &Resource::Workspace(W)).await.unwrap().allow }, 3s)
// assert allowed  -> proves the Arc<dyn Pdp> (as stored in state) reloads from the shared bus.
```
Reuse the `poll_until` helper shape from `crates/egret-auth/tests/pdp_hot_reload.rs`. The point distinct from B1b-0's test: the PDP is held as `Arc<dyn Pdp>` (trait object, as `CpState`/app state stores it), proving the dyn-dispatched handle still reloads.

- [ ] **Step 4: Run + build**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-auth --test pdp_app_wiring -- --nocapture` → PASS, confirm it RAN.
Run: `rtk proxy cargo build --workspace` → SUCCESS (CpState's new field is set at every construction site — grep `CpState {` to find them; the main one is `egret-bin/src/main.rs`, plus any test builders in `egret-controlplane/tests` which need a pdp — pass `Arc::new(CedarPdp::build(...))` or, if those tests don't exercise authz, a tiny inline stub `Pdp` that denies/allows as the test expects).

- [ ] **Step 5: Commit**

```bash
git add crates/egret-controlplane/src/lib.rs crates/egret-bin/src/main.rs crates/egret-auth/tests/pdp_app_wiring.rs
git add -u  # CpState construction fixes in controlplane tests
git commit -m "feat(authz): build CedarPdp from the shared bus, inject into CpState (closes F1)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Migrate the `secrets` seam + full gate

**Files:**
- Modify: `crates/egret-controlplane/src/secrets.rs`
- Test: existing secrets tests + a new PDP-path assertion (locate them: `crates/egret-controlplane/tests/*` referencing `secret:`)

- [ ] **Step 1: Migrate the 4 gate calls**

Read `crates/egret-controlplane/src/secrets.rs`. Each handler currently does `require_permission(&principal, "secret:<action>")?` (sync, from `egret_controlplane::auth`). Replace each with the PDP-backed PEP, using the `pdp` now on the state the handler extracts:
```rust
// was: require_permission(&principal, "secret:read")?;
egret_core::authz::require(&*state.pdp, &principal, "secret:read").await?;
```
Do this for `secret:read`, `secret:create`, `secret:update`, `secret:delete` at their exact sites. The handler must have the state (holding `pdp`) and the `Principal` in scope — confirm the extractor names (`State<CpState>`/`principal`) at each site and adapt. Remove the now-unused `require_permission` import from this file if nothing else in it uses it (leave it if other handlers in the file still do — secrets.rs is only secrets, so likely removable).

- [ ] **Step 2: Update / add tests**

Find the secrets route tests (grep `secret:` / `secrets` under `crates/egret-controlplane/tests`). They construct a `CpState` — give it a real `pdp` (`Arc::new(CedarPdp::build(db.clone(), &bus).await.unwrap())` with a fresh bus) so the handlers authorize through the PDP. Ensure the seeded principals in those tests hold the right platform role/permission so outcomes are unchanged (a `platform_admin` still passes; a principal with `secret:read` via a platform role can read but not create). Add one explicit assertion: a non-admin principal granted `secret:read` (through a platform role) gets 200 on read and 403 on create.

- [ ] **Step 3: Run the secrets tests**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-controlplane secrets -- --nocapture` (or the exact test binary) → PASS, confirm the authz assertions RAN.

- [ ] **Step 4: Full local CI gate (sequential — never two workspace test runs at once)**

- `rtk proxy cargo fmt --all --check` (fmt + restage if needed)
- `rtk proxy cargo clippy --workspace --all-targets -- -D warnings` → clean
- `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test --workspace` → 0 failures
- Frontend gate: `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build` → green (no FE surface expected)

- [ ] **Step 5: Commit**

```bash
git add crates/egret-controlplane/src/secrets.rs
git add -u  # secrets test updates
git commit -m "feat(authz): migrate the secrets seam onto the PDP-backed require() (proof)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review Notes

- **Spec coverage:** §2 PDP in state + F1 e2e → Task 3; §3 `Principal.group_ids` → Tasks 1+2; §4 `AuthTarget`+`require` (both overloads) → Task 1; §5 secrets proof → Task 4; §6 tests distributed across tasks.
- **Behavior preservation:** the secrets decisions are unchanged because the PDP's Platform decision equals the legacy `require_permission` decision (B1a regression matrix); tests assert identical outcomes.
- **Scope discipline:** no other seam migrated; the two legacy authorizers, `Principal.permissions`, and grant-writer flip remain untouched (B1b-2/3/final).
- **Type consistency:** `AuthTarget::{subject, resource}`, `require(pdp: &dyn Pdp, target: &impl AuthTarget, action)`, `Principal.group_ids: Vec<GroupId>`, `CpState.pdp: Arc<dyn Pdp>` — consistent across tasks.
- **No placeholders:** test bodies in Task 3/4 are described with concrete assertions and reuse the existing `poll_until`/seed helpers; the only prose-sketched items are which exact test files to edit, resolved by grep in-task.

---

## Next: B1b-2 (separate plan)

Migrate the remaining platform seams (engine-mgmt, warehouse-admin, policy-admin, storage, access, control-plane) off `require_permission` onto `require()`, injecting the shared `Arc<dyn Pdp>` into each seam state. Then B1b-3 (workspace seams: bi, notebook, query, kernel, workspace) and B1b-final (remove the legacy authorizers + `Principal.permissions`, flip grant writers to `access_grants`, drop the sync triggers + legacy tables).
