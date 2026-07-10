# B1b-0 — Event Bus + Cedar PDP Hot-Reload Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a reusable in-process `egret-events` bus and use it to make `CedarPdp` reload its template set whenever roles/permissions change — removing security-review blocker #3 so a later increment can wire the PDP into serving.

**Architecture:** A concrete, cloneable `EventBus` handle routes events by concrete type over `tokio::sync::broadcast` channels (no central enum; seams own their event types). `egret-auth`'s `RoleRepo` publishes a `RolesChanged` event on `create`/`delete`/`set_permissions`. `CedarPdp` holds its template set behind `arc_swap::ArcSwap` and a background task rebuilds+swaps it on each `RolesChanged`.

**Tech Stack:** Rust, `tokio` (broadcast), `arc-swap`, `sqlx`/Postgres, `cedar-policy` (already integrated in B1a).

**Reference:** [B1b-0 design](../specs/2026-07-10-b1b-0-event-bus-hot-reload-design.md).

---

## File Structure

- `crates/egret-events/` (new crate) — `Cargo.toml`, `src/lib.rs`: `Event` trait + `EventBus`.
- `crates/egret-auth/src/events.rs` (new) — `RolesChanged` event.
- `crates/egret-auth/src/roles.rs` (modify) — `RoleRepo` gains an `EventBus`, publishes on 3 methods.
- `crates/egret-auth/src/pdp/mod.rs` (modify) — `CedarPdp` reload via `ArcSwap` + background task.
- `crates/egret-bin/src/main.rs` (modify) — construct the bus, pass to `RoleRepo::new`.
- `Cargo.toml` (workspace) — add `arc-swap`; `egret-events` is picked up by `members = ["crates/*"]`.
- Tests: `crates/egret-events/src/lib.rs` (inline), `crates/egret-auth/tests/role_events.rs`, `crates/egret-auth/tests/pdp_hot_reload.rs`.

Call sites of `RoleRepo::new` to update (5): `crates/egret-bin/src/main.rs:1246`, `crates/egret-auth/tests/roles_db.rs:25`, `crates/egret-controlplane/tests/routes_api.rs:62,494,1065`.
Call sites of `CedarPdp::build` to update (all in `crates/egret-auth/tests/`): `cedar_pdp.rs`, `pdp_regression.rs`, plus the downgrade test if it builds a PDP (`grant_downgrade.rs` — check).

---

## Task 1: Create the `egret-events` crate

**Files:**
- Create: `crates/egret-events/Cargo.toml`, `crates/egret-events/src/lib.rs`
- Test: inline `#[cfg(test)]` in `src/lib.rs`

- [ ] **Step 1: Create the crate manifest**

`crates/egret-events/Cargo.toml`:
```toml
[package]
name = "egret-events"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio = { workspace = true }

[dev-dependencies]
tokio = { workspace = true }
```
(Match the `edition`/`version` conventions of a sibling crate like `crates/egret-core/Cargo.toml` if they differ.)

- [ ] **Step 2: Write the failing tests + the bus**

`crates/egret-events/src/lib.rs`:
```rust
//! Minimal in-process event bus. Seams define their own event types (each
//! `impl Event`) and the bus routes by concrete type over broadcast channels —
//! no central event enum, so seams stay isolated. In-proc now; a remote adapter
//! is a future internal swap behind this same surface.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

/// A domain event carried by the bus.
pub trait Event: Clone + Send + Sync + 'static {}

const DEFAULT_CAPACITY: usize = 64;

/// A cheaply-cloneable handle to the in-process bus. Clones share the same
/// channels. (Concrete, not a `dyn` trait — the generic publish/subscribe
/// methods are not object-safe.)
#[derive(Clone, Default)]
pub struct EventBus {
    channels: Arc<Mutex<HashMap<TypeId, Box<dyn Any + Send + Sync>>>>,
}

impl EventBus {
    pub fn new() -> Self {
        Self::default()
    }

    /// Deliver `event` to all current subscribers of `E`. No-op if there are
    /// none. Never blocks (a lagging subscriber drops old events).
    pub fn publish<E: Event>(&self, event: E) {
        let map = self.channels.lock().unwrap();
        if let Some(boxed) = map.get(&TypeId::of::<E>()) {
            if let Some(tx) = boxed.downcast_ref::<broadcast::Sender<E>>() {
                let _ = tx.send(event);
            }
        }
    }

    /// Subscribe to future `E` events.
    pub fn subscribe<E: Event>(&self) -> broadcast::Receiver<E> {
        let mut map = self.channels.lock().unwrap();
        let boxed = map.entry(TypeId::of::<E>()).or_insert_with(|| {
            let (tx, _rx) = broadcast::channel::<E>(DEFAULT_CAPACITY);
            Box::new(tx)
        });
        boxed
            .downcast_ref::<broadcast::Sender<E>>()
            .expect("channel stored under its own TypeId")
            .subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct Ping(u32);
    impl Event for Ping {}
    #[derive(Clone, Debug, PartialEq)]
    struct Pong;
    impl Event for Pong {}

    #[tokio::test]
    async fn subscriber_receives_published_event() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe::<Ping>();
        bus.publish(Ping(7));
        assert_eq!(rx.recv().await.unwrap(), Ping(7));
    }

    #[tokio::test]
    async fn event_types_are_routed_independently() {
        let bus = EventBus::new();
        let mut ping = bus.subscribe::<Ping>();
        let mut pong = bus.subscribe::<Pong>();
        bus.publish(Pong);
        assert_eq!(pong.recv().await.unwrap(), Pong);
        // Ping channel saw nothing; try_recv is Empty, not a Pong.
        assert!(ping.try_recv().is_err());
    }

    #[tokio::test]
    async fn publish_with_no_subscriber_is_a_noop() {
        let bus = EventBus::new();
        bus.publish(Ping(1)); // must not panic
        let mut rx = bus.subscribe::<Ping>();
        bus.publish(Ping(2));
        assert_eq!(rx.recv().await.unwrap(), Ping(2)); // only events after subscribe
    }

    #[tokio::test]
    async fn clones_share_channels() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe::<Ping>();
        let bus2 = bus.clone();
        bus2.publish(Ping(9));
        assert_eq!(rx.recv().await.unwrap(), Ping(9));
    }
}
```

- [ ] **Step 3: Run the tests**

Run: `rtk proxy cargo test -p egret-events`
Expected: PASS (4 tests). (`rtk proxy` gives raw output; note the shell aliases plain `rm` to a no-op — use `/bin/rm -f` if you ever need to delete a file.)

- [ ] **Step 4: Commit**

```bash
git add crates/egret-events/Cargo.toml crates/egret-events/src/lib.rs Cargo.lock
git commit -m "feat(events): in-process EventBus + Event trait (egret-events)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `RolesChanged` event + `RoleRepo` publishes it

**Files:**
- Create: `crates/egret-auth/src/events.rs`
- Modify: `crates/egret-auth/src/lib.rs` (add `pub mod events;`), `crates/egret-auth/src/roles.rs`, `crates/egret-auth/Cargo.toml`
- Modify call sites: `crates/egret-bin/src/main.rs:1246`, `crates/egret-auth/tests/roles_db.rs:25`, `crates/egret-controlplane/tests/routes_api.rs:62,494,1065`
- Test: `crates/egret-auth/tests/role_events.rs`

- [ ] **Step 1: Add the dependency**

In `crates/egret-auth/Cargo.toml` `[dependencies]` add `egret-events = { workspace = true }`. Add `egret-events = { path = "crates/egret-events" }` to the workspace root `Cargo.toml` `[workspace.dependencies]` (match how other `egret-*` path deps are declared there). Also add `egret-events = { workspace = true }` to `crates/egret-controlplane/Cargo.toml` `[dev-dependencies]` (its tests will construct a bus).

- [ ] **Step 2: Define the event**

`crates/egret-auth/src/events.rs`:
```rust
//! Events published by the auth/RBAC seam.

/// The role/permission catalog changed (a role was created, deleted, or had its
/// permission set replaced). Consumers that cache role→permission data (e.g. the
/// Cedar PDP template set) rebuild on this. Grant *assignments* do NOT emit it —
/// they are read per-request, not cached.
#[derive(Clone, Debug)]
pub struct RolesChanged;
impl egret_events::Event for RolesChanged {}
```
Add `pub mod events;` to `crates/egret-auth/src/lib.rs` (match existing module ordering).

- [ ] **Step 3: Thread the bus into `RoleRepo` and publish**

In `crates/egret-auth/src/roles.rs`, add a field + constructor arg and publish after successful writes. Read the current `RoleRepo` first; apply these exact changes:
```rust
use egret_events::EventBus;
use crate::events::RolesChanged;

pub struct RoleRepo {
    db: Db,
    bus: EventBus,
}

impl RoleRepo {
    pub fn new(db: Db, bus: EventBus) -> Self {
        Self { db, bus }
    }
    // ... unchanged read methods ...
}
```
Then, in each of the three mutating methods, publish **after** the query succeeds (just before `Ok(...)`):
- `create(...)` — after the `RETURNING id` succeeds, before returning the id: `self.bus.publish(RolesChanged);`
- `delete(...)` — after the `DELETE FROM roles` succeeds: `self.bus.publish(RolesChanged);`
- `set_permissions(...)` — after the delete+insert of `role_permissions` commits: `self.bus.publish(RolesChanged);`

Do NOT publish in `update_description`, `assign`, or `unassign`.

- [ ] **Step 4: Update the 5 `RoleRepo::new` call sites**

Each becomes `RoleRepo::new(db.clone(), bus.clone())`. For the production site `crates/egret-bin/src/main.rs:1246`, first create a bus near where the repos are built (Task 4 refines this; for now `let bus = egret_events::EventBus::new();` above the `AppState { ... }` construction, and pass `bus.clone()`). For the three test sites in `crates/egret-controlplane/tests/routes_api.rs` and the one in `crates/egret-auth/tests/roles_db.rs`, pass `egret_events::EventBus::new()` (a detached bus — those tests don't assert on events).

- [ ] **Step 5: Write the publish test**

`crates/egret-auth/tests/role_events.rs` (DB-gated, mirror the harness in `crates/egret-auth/tests/roles_db.rs` — reads `DATABASE_URL`, self-skips if unset):
```rust
// Build a bus, subscribe to RolesChanged, build RoleRepo with that bus, and
// assert set_permissions / create / delete each emit exactly one event while
// update_description / assign emit none.
```
Concretely: `let bus = EventBus::new(); let mut rx = bus.subscribe::<RolesChanged>(); let repo = RoleRepo::new(db.clone(), bus.clone());` then create a role, assert `rx.try_recv().is_ok()` and the channel is then empty; call `set_permissions`, assert one event; call `update_description`, assert `rx.try_recv()` is `Err(Empty)`; call `delete`, assert one event. Use `try_recv()` after `tokio::task::yield_now().await` if needed so the send is observed (broadcast send is synchronous, so `try_recv` should see it immediately).

- [ ] **Step 6: Run tests + build**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-auth --test role_events -- --nocapture` → PASS, confirm it RAN (not skipped).
Run: `rtk proxy cargo build --workspace` → SUCCESS (all 5 call sites updated; nothing else references `RoleRepo::new` with the old arity).

- [ ] **Step 7: Commit**

```bash
git add crates/egret-auth/src/events.rs crates/egret-auth/src/lib.rs crates/egret-auth/src/roles.rs crates/egret-auth/Cargo.toml crates/egret-controlplane/Cargo.toml crates/egret-bin/src/main.rs crates/egret-auth/tests/roles_db.rs crates/egret-controlplane/tests/routes_api.rs crates/egret-auth/tests/role_events.rs Cargo.toml Cargo.lock
git commit -m "feat(authz): RoleRepo publishes RolesChanged on role/permission edits

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `CedarPdp` hot-reload via `ArcSwap`

**Files:**
- Modify: `crates/egret-auth/src/pdp/mod.rs`, `crates/egret-auth/Cargo.toml`, workspace `Cargo.toml`
- Update call sites: `crates/egret-auth/tests/cedar_pdp.rs`, `crates/egret-auth/tests/pdp_regression.rs`, and `grant_downgrade.rs` (only if it calls `CedarPdp::build`)

- [ ] **Step 1: Add the `arc-swap` dependency**

Workspace root `Cargo.toml` `[workspace.dependencies]`: add `arc-swap = "1"`. `crates/egret-auth/Cargo.toml` `[dependencies]`: add `arc-swap = { workspace = true }`.

- [ ] **Step 2: Refactor `CedarPdp` to hold a swappable inner + reload task**

In `crates/egret-auth/src/pdp/mod.rs`, restructure (read the current file first — preserve the existing `load` logic exactly; only move it into `load_inner`). Target shape:
```rust
use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use cedar_policy::{Authorizer, Context, Decision as CedarDecision, PolicyId, PolicySet, Request, Template};
use egret_core::authz::{Decision, Pdp, Resource, Subject};
use egret_core::Result;
use egret_events::EventBus;
use egret_metastore::Db;
use tokio::sync::broadcast;

use crate::events::RolesChanged;
use crate::grants::AccessGrantStore;

struct PdpInner {
    templates: PolicySet,
    role_names: HashMap<uuid::Uuid, String>,
}

#[derive(Clone)]
pub struct CedarPdp {
    grants: AccessGrantStore,
    db: Db,
    inner: Arc<ArcSwap<PdpInner>>,
}

impl CedarPdp {
    /// Build the initial template set and spawn a task that rebuilds it on every
    /// `RolesChanged` event from `bus`.
    pub async fn build(db: Db, bus: &EventBus) -> Result<Self> {
        let inner = Arc::new(ArcSwap::from_pointee(Self::load_inner(&db).await?));
        let pdp = Self {
            grants: AccessGrantStore::new(db.clone()),
            db,
            inner,
        };
        pdp.spawn_reload(bus.subscribe::<RolesChanged>());
        Ok(pdp)
    }

    /// The B1a `build` body: load roles + role_permissions, compile one template
    /// per role keyed by `role:{id}`, gate the admin body on scope+name.
    async fn load_inner(db: &Db) -> Result<PdpInner> {
        // ... move the existing template-building loop here, returning PdpInner ...
    }

    fn spawn_reload(&self, mut rx: broadcast::Receiver<RolesChanged>) {
        let db = self.db.clone();
        let inner = self.inner.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {
                        match CedarPdp::load_inner(&db).await {
                            Ok(next) => inner.store(Arc::new(next)),
                            Err(e) => {
                                tracing::error!(error = ?e, "PDP reload failed; keeping previous policy set")
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}
```
Then update `authorize` to read the current inner:
```rust
#[async_trait::async_trait]
impl Pdp for CedarPdp {
    async fn authorize(&self, subject: &Subject, action: &str, resource: &Resource) -> Result<Decision> {
        let ws = match resource { Resource::Workspace(w) => Some(*w), Resource::Platform => None };
        let grants = self.grants.grants_for(subject.user_id, ws).await?;
        let inner = self.inner.load();
        let policies = entities::link_grants(inner.templates.clone(), subject, &grants, &inner.role_names);
        let ents = entities::build_entities(subject, resource);
        let (p, a, r) = entities::request_uids(subject, action, resource);
        let req = Request::new(p, a, r, Context::empty(), None)
            .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("{e:?}")))?;
        let resp = Authorizer::new().is_authorized(&req, &policies, &ents);
        Ok(Decision {
            allow: resp.decision() == CedarDecision::Allow,
            reasons: resp.diagnostics().reason().map(|id| id.to_string()).collect(),
        })
    }
}
```
Keep `tracing` available (add `tracing = { workspace = true }` to `egret-auth/Cargo.toml` deps if not already present — check).

- [ ] **Step 3: Update `CedarPdp::build` call sites in tests**

In `crates/egret-auth/tests/cedar_pdp.rs`, `crates/egret-auth/tests/pdp_regression.rs` (and `grant_downgrade.rs` if applicable), change `CedarPdp::build(db.clone())` → build a bus first and pass it: `let bus = egret_events::EventBus::new(); let pdp = CedarPdp::build(db.clone(), &bus).await.unwrap();`. These tests don't exercise reload, so a detached bus is fine.

- [ ] **Step 4: Run the affected tests**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-auth` → all green, including the 60-cell `pdp_regression` (still 60/60) and `cedar_pdp`. Confirm they RAN.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-auth/src/pdp/mod.rs crates/egret-auth/Cargo.toml crates/egret-auth/tests/cedar_pdp.rs crates/egret-auth/tests/pdp_regression.rs crates/egret-auth/tests/grant_downgrade.rs Cargo.toml Cargo.lock
git commit -m "feat(authz): CedarPdp reloads its template set on RolesChanged

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Hot-reload integration test + composition wiring + full gate

**Files:**
- Create: `crates/egret-auth/tests/pdp_hot_reload.rs`
- Modify: `crates/egret-bin/src/main.rs` (finalize bus construction near the repo/AppState build)

- [ ] **Step 1: Finalize the composition wiring**

In `crates/egret-bin/src/main.rs`, ensure a single `let bus = egret_events::EventBus::new();` is created before the repos are constructed and that `RoleRepo::new(db.clone(), bus.clone())` uses it (from Task 2). Add a brief comment: `// event bus — RoleRepo publishes RolesChanged; the Cedar PDP subscriber is wired in B1b-1`. Do NOT construct `CedarPdp` here (that is B1b-1). If the bus var is otherwise unused and triggers a warning, that is expected only until B1b-1; if `-D warnings` fails on it, bind it as `let bus = ...;` and pass a clone to `RoleRepo` (a used clone silences the unused warning).

- [ ] **Step 2: Write the hot-reload integration test (the #3 regression)**

`crates/egret-auth/tests/pdp_hot_reload.rs` (DB-gated). It proves an in-flight PDP reflects a permission grant made through `RoleRepo` sharing the same bus, with no rebuild:
```rust
// 1. Seed: a user U with a fresh workspace role R (no permissions) in workspace W.
// 2. let bus = EventBus::new();
//    let pdp = CedarPdp::build(db.clone(), &bus).await.unwrap();
//    let repo = RoleRepo::new(db.clone(), bus.clone());
//    let subject = Subject { user_id: U, group_ids: vec![] };
// 3. Assert denied initially:
//    assert!(!pdp.authorize(&subject, "dashboard:read", &Resource::Workspace(W)).await.unwrap().allow);
// 4. Grant it through the repo (emits RolesChanged):
//    repo.set_permissions(R_id, &["dashboard:read".into()]).await.unwrap();
// 5. Await the async reload with a bounded poll, then assert allowed:
//    let allowed = poll_until(|| async {
//        pdp.authorize(&subject, "dashboard:read", &Resource::Workspace(W)).await.unwrap().allow
//    }, Duration::from_secs(3)).await;
//    assert!(allowed, "PDP did not hot-reload the new permission");
// 6. Revoke and assert it flips back the same way.
```
Implement `poll_until` inline as a loop: `for _ in 0..30 { if cond().await { return true } tokio::time::sleep(Duration::from_millis(100)).await; } false`. Create the fresh role R via `repo.create("workspace", &uniq("hotreload"), "")` so it starts with zero permissions and its name won't collide; add U as a `workspace_members` row with role name R (the 0068/0069 trigger mirrors it into `access_grants`). Use unique uuids/names per run.

- [ ] **Step 3: Run the integration test**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-auth --test pdp_hot_reload -- --nocapture` → PASS, confirm it RAN. If it flakes on timing, widen the poll budget (e.g. 50×100ms) — but it should settle in well under a second.

- [ ] **Step 4: Full local CI gate (sequential — never two workspace test runs at once)**

Run each, capture results:
- `rtk proxy cargo fmt --all --check` (run `cargo fmt --all` and restage if needed)
- `rtk proxy cargo clippy --workspace --all-targets -- -D warnings` → clean
- `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test --workspace` → 0 failures
- Frontend gate (no FE surface expected, but per project practice): `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build` → green

- [ ] **Step 5: Commit**

```bash
git add crates/egret-bin/src/main.rs crates/egret-auth/tests/pdp_hot_reload.rs
git commit -m "test(authz): hot-reload integration — PDP reflects RBAC edits without restart (#3)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review Notes

- **Spec coverage:** §2 bus → Task 1; §3 `RolesChanged` + `RoleRepo` publish points → Task 2 (create/delete/set_permissions publish; update_description/assign/unassign do not); §4 `CedarPdp` `ArcSwap` reload + fail-safe (keep previous on rebuild error) → Task 3; §5 composition wiring → Tasks 2/4; §6 tests (bus unit, publish, hot-reload integration) → Tasks 1/2/4; §7 scope (no serving wiring, no other topics) → respected (Task 4 Step 1 explicitly does NOT construct CedarPdp).
- **Open questions resolved:** `arc-swap` chosen (Task 3); broadcast capacity 64 (Task 1); `EventBus` threaded explicitly through the 5 `RoleRepo::new` call sites (Task 2) rather than adding a bus-less constructor.
- **Type consistency:** `EventBus::{new, publish<E>, subscribe<E>}`, `Event` marker, `RolesChanged`, `CedarPdp::build(db, &EventBus)`, `load_inner`, `spawn_reload`, `PdpInner { templates, role_names }` — used consistently across tasks.
- **No placeholders:** the only `// ...` is in Task 3 Step 2 `load_inner`, which explicitly says "move the existing B1a template-building loop here" — the code already exists in the repo and is not re-authored.

---

## Next: B1b-1 (separate plan)

Construct `CedarPdp` in the `egret-bin`/`egret-server` app state (subscribing to the now-live bus), introduce `AuthContext` + `require(ctx, action[, resource])`, and begin migrating call-sites off the two legacy authorizers.
