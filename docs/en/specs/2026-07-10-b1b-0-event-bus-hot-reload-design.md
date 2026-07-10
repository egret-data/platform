# B1b-0 — Internal Event Bus + Cedar PDP Hot-Reload

> **Status:** Design (approved to plan) · **Date:** 2026-07-10 · **Version:** 1.0
>
> **Parent:** [Effort B authorization design](../architecture/2026-07-10-effort-b-authorization-design.md). This is the first increment of **B1b** (the PEP cutover), landed first because it unblocks the rest.
>
> **Prerequisite it removes:** security-review finding **#3** — `CedarPdp::build` snapshots `roles` + `role_permissions` once, so RBAC edits are stale until process restart. Both legacy authorizers read per-request, so the PDP cannot be wired into serving (B1b-1) until it reloads on change.

---

## 1. Why

B1a shipped a dormant Cedar PDP whose template set is built once at construction. Before B1b-1 wires the PDP into request handling, it must stay current with the live RBAC console (roles created/deleted, permissions changed). The service-ready architecture (Effort A §3.3/§4.6) planned an internal **event bus** (`egret-events`) for exactly this class of async cross-seam signal, but it was never built. We build it now — a small, reusable transport — and wire **one** topic (RBAC changes → PDP reload) end-to-end as both the #3 fix and the bus's first real consumer.

Scope is deliberately narrow: build the bus and the roles→reload path only. The other planned topics (`workspace.deleted`, `warehouse.provisioned`, `policy.changed`, `catalog.changed`, `query.finished`) are future work that this bus enables but does not implement here.

## 2. `egret-events` — the transport

A new crate `egret-events` providing an in-process publish/subscribe bus. "In-proc now, remote adapter later" mirrors the Port pattern used across egret.

### 2.1 Event marker
```rust
/// A domain event. Each seam defines its own event types (no central enum) so
/// seams stay isolated — the bus routes by concrete type.
pub trait Event: Clone + Send + Sync + 'static {}
```

### 2.2 Bus handle
Because the publish/subscribe methods are **generic over the event type**, the bus cannot be a `dyn` trait object (generic methods aren't object-safe). It is therefore a **concrete, cheaply-cloneable handle** (the idiomatic Rust choice, as with `tokio::sync::broadcast` itself). A future distributed backend is an internal swap behind this same surface (events would then additionally require `Serialize`; out of scope here).

```rust
#[derive(Clone, Default)]
pub struct EventBus { inner: Arc<Mutex<HashMap<TypeId, Box<dyn Any + Send + Sync>>>> }

impl EventBus {
    pub fn new() -> Self { Self::default() }

    /// Publish to all current subscribers of `E`. No-op if none. Never blocks.
    pub fn publish<E: Event>(&self, event: E) {
        if let Some(tx) = self.sender_for::<E>(false) { let _ = tx.send(event); }
    }

    /// Subscribe to future `E` events.
    pub fn subscribe<E: Event>(&self) -> broadcast::Receiver<E> {
        self.sender_for::<E>(true).expect("created on demand").subscribe()
    }

    // get-or-create a broadcast::Sender<E> keyed by TypeId::of::<E>(), downcast from Box<dyn Any>.
    fn sender_for<E: Event>(&self, create: bool) -> Option<broadcast::Sender<E>> { /* ... */ }
}
```
- Backed by `tokio::sync::broadcast` (bounded buffer, e.g. capacity 64). Fire-and-forget: a lagging subscriber receives `RecvError::Lagged(n)` rather than blocking the publisher.
- `sender_for` locks the map briefly, keys on `TypeId::of::<E>()`, downcasts the stored `Box<dyn Any>` to `broadcast::Sender<E>`, creating it on `subscribe` (and lazily on `publish` only if a channel already exists — publishing with no subscribers is a cheap no-op).

### 2.3 Delivery contract
Events are **best-effort and coalescible**. Consumers must tolerate missed/duplicated signals: the roles→reload consumer treats both a received event and a `Lagged` error identically ("something changed → rebuild"). This keeps the bus simple and the reload idempotent.

## 3. `RolesChanged` event + publish points

Defined in `egret-auth` (the seam that owns `roles`/`role_permissions`):
```rust
#[derive(Clone)] pub struct RolesChanged;
impl egret_events::Event for RolesChanged {}
```

`RoleRepo` (`egret-auth/src/roles.rs`) is the single funnel for role/permission mutations — even the control-plane `set_role_permissions` handler goes through it — so publishing there covers every caller. `RoleRepo` gains an `EventBus` and publishes `RolesChanged` **after a successful write** in exactly the three methods that change the template set:

| Method | Publishes? | Why |
|---|---|---|
| `create(scope, name, description)` | ✅ | new role → new template |
| `delete(id)` | ✅ | role gone → template removed |
| `set_permissions(id, perms)` | ✅ | role's action list changed → template body changed |
| `update_description(id, …)` | ❌ | description isn't in the template |
| `assign` / `unassign` | ❌ | grant assignments are read per-request via `AccessGrantStore::grants_for` — not cached in templates |

`RoleRepo::new` takes an `EventBus`. Tests and any other constructor pass a bus (a fresh `EventBus::new()` where no subscriber is needed).

**Out-of-band writes** (seed, migration, manual SQL) do not publish. This is acceptable: startup seed/migration run before serving, so the initial `CedarPdp::build` captures them; runtime manual SQL is rare and picked up by the next app-driven change or a restart. (The DB-trigger alternative was rejected to avoid a second DB→bus path; revisit only if runtime out-of-band RBAC writes become a real workflow.)

## 4. `CedarPdp` hot-reload

Restructure so the template set lives behind a swappable cell and a background task rebuilds it on `RolesChanged`.

```rust
struct PdpInner { templates: PolicySet, role_names: HashMap<Uuid, String> }

#[derive(Clone)]
pub struct CedarPdp {
    grants: AccessGrantStore,
    db: Db,                       // retained to rebuild
    inner: Arc<ArcSwap<PdpInner>>,
}

impl CedarPdp {
    /// Build the initial template set and spawn a task that rebuilds it whenever
    /// a `RolesChanged` event arrives.
    pub async fn build(db: Db, bus: &EventBus) -> Result<Self> {
        let inner = Arc::new(ArcSwap::from_pointee(Self::load_inner(&db).await?));
        let pdp = Self { grants: AccessGrantStore::new(db.clone()), db, inner };
        pdp.spawn_reload(bus.subscribe::<RolesChanged>());
        Ok(pdp)
    }

    async fn load_inner(db: &Db) -> Result<PdpInner> { /* the existing build() body, factored out */ }

    fn spawn_reload(&self, mut rx: broadcast::Receiver<RolesChanged>) {
        let (db, inner) = (self.db.clone(), self.inner.clone());
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {
                        match CedarPdp::load_inner(&db).await {
                            Ok(next) => inner.store(Arc::new(next)),
                            Err(e) => tracing::error!(?e, "PDP reload failed; keeping previous policy set"),
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}
```
- `authorize` reads `let inner = self.inner.load();` and uses `inner.templates` / `inner.role_names` (otherwise unchanged from B1a).
- **Fail-safe:** if a rebuild errors (e.g. a transient DB blip), the previous good policy set is kept — the PDP never falls back to allow-all or empty.
- New dependency: `arc-swap` (lock-free read-mostly cell). Alternative `tokio::sync::RwLock<Arc<PdpInner>>` needs no new dep; `arc-swap` is preferred for the read-heavy `authorize` path. The plan may pick either.

## 5. Composition & wiring

- `egret-bin` (composition root) constructs one `EventBus` and stores it in the shared app state, then hands it to `RoleRepo` (publisher).
- The bus is available for `CedarPdp::build` to subscribe. **Constructing `CedarPdp` into the live serving state is B1b-1** (when `require()` consumes it); B1b-0 does not wire the dormant PDP into request handling. B1b-0 proves the reload with an integration test that constructs a `CedarPdp` from a bus, mutates a role via `RoleRepo`, and observes the decision change.

## 6. Testing

- **egret-events unit tests:** publish→subscribe delivers; two event types are independently routed; a subscriber that joins after a publish misses it (documented semantics); a slow subscriber gets `Lagged` rather than blocking the publisher.
- **RoleRepo publish test:** `set_permissions`/`create`/`delete` each emit exactly one `RolesChanged`; `update_description`/`assign` emit none.
- **Hot-reload integration test (the #3 regression):** build `CedarPdp::build(db, &bus)`; assert a subject is denied `action X` at some scope; via `RoleRepo` (sharing `bus`) grant `X` to the subject's role with `set_permissions`; await the reload (poll `authorize` until it flips, with a bounded timeout); assert it is now allowed — **without** reconstructing the PDP. Also assert removing the permission flips it back.
- **Full local gate:** `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and the frontend gate (no FE surface expected here, but run it per project practice).

## 7. Scope boundary

- **In:** `egret-events` crate (bus + `Event` + in-proc impl + tests); `RolesChanged` + publishing in `RoleRepo`; `CedarPdp` reload via `ArcSwap` + reload task; bus construction in `egret-bin` + threading into `RoleRepo`; the reload integration test.
- **Out:** wiring `CedarPdp` into serving / `require()` (B1b-1); any other event topic; a remote/serialized bus backend; the DB-trigger publish path.

## 8. Open questions (for the plan)

- `arc-swap` vs `RwLock<Arc<PdpInner>>` — pick one (default: `arc-swap`).
- `broadcast` channel capacity (default 64) — large enough that normal RBAC-edit bursts don't force `Lagged`, though `Lagged` is handled correctly anyway.
- Whether `RoleRepo::new`'s new `EventBus` argument should be threaded through every existing constructor call site now, or a `RoleRepo::with_bus` added alongside a bus-less `new` that uses a detached `EventBus::new()` (fewer call-site edits). The plan decides; prefer explicit threading if the call sites are few.

---

*Design v1.0 (2026-07-10). First increment of B1b; unblocks wiring the Cedar PDP into serving by making it reload on RBAC change. Builds the reusable `egret-events` bus (Effort A §3.3), wiring only the roles→reload topic.*
