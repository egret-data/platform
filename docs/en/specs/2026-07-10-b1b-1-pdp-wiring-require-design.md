# B1b-1 — Wire the Cedar PDP into serving + unified `require()`

> **Status:** Design (approved to plan) · **Date:** 2026-07-10 · **Version:** 1.0
>
> **Parent:** [Effort B authorization design](../architecture/2026-07-10-effort-b-authorization-design.md) · **Predecessor:** [B1b-0 event bus + hot-reload](./2026-07-10-b1b-0-event-bus-hot-reload-design.md).
>
> Second increment of B1b (the PEP cutover). Wires the (now hot-reloading) Cedar PDP into the running app behind a single `require()` PEP, and migrates one small seam as proof. Bulk call-site migration and legacy removal are later increments.

---

## 1. Why & scope

B1a built the Cedar PDP; B1b-0 made it hot-reload so it can safely go live. B1b-1 stands up the enforcement path: construct the PDP in the app (subscribing to the **same** event bus — review finding F1), introduce a single `require()` that both platform and workspace handlers call, and migrate the `secrets` seam off the legacy static gate as an end-to-end proof.

**In scope:** PDP in `AppState` (shared bus + F1 e2e test); `Principal.group_ids`; `egret_core::authz::require()` + `AuthTarget` (both `Principal`→Platform and `WorkspaceContext`→Workspace overloads, unit-tested); migrate the `secrets` seam.
**Out of scope:** migrating any other seam (B1b-2 platform seams, B1b-3 workspace seams); removing the two legacy authorizers or `Principal.permissions`; flipping grant writers / dropping legacy tables (B1b-final).

## 2. Wire the PDP into `AppState` (closes F1)

- `egret-bin` already constructs one `EventBus` and hands it to `RoleRepo` (B1b-0). It now also builds the PDP from the **same** bus: `let pdp: Arc<dyn Pdp> = Arc::new(CedarPdp::build(db.clone(), &bus).await?);` — the shared instance is what makes hot-reload actually fire (a *different* `EventBus` would silently never reload — F1).
- `AppState` (in `egret-server`) gains a `pdp: Arc<dyn egret_core::authz::Pdp>` field with a `FromRef<AppState>` impl, so any seam's handler can extract it. Seams depend only on the `egret-core` `Pdp` trait — never on `CedarPdp` or `egret-auth`'s Cedar internals.
- **F1 regression test** (DB-gated, app-level): build `AppState` (or the minimal slice: one shared `bus`, `RoleRepo::new(db, bus.clone())`, `CedarPdp::build(db, &bus)` stored as the app pdp); grant a permission through *that* `RoleRepo`; assert the *app's* pdp reflects it after the reload. This proves the wiring threads the same bus and guards against a future re-break.

## 3. `Principal.group_ids`

`Subject` (the PDP input) needs the caller's group memberships for the Cedar group path. Add `group_ids: Vec<GroupId>` to `egret_core::Principal`, populated in `SessionService::validate` (which already loads `permissions` from a `user_groups`-joined query — load the group ids in the same place). `Subject` is then built purely in memory from a `Principal`, with no extra query per authorization.

## 4. `require()` — the single PEP

In `egret_core::authz` (co-located with `Subject`/`Resource`/`Decision`/`Pdp`):
```rust
/// A request context that resolves to a PDP subject + the resource scope of the
/// action. Implemented by the two request-context shapes.
pub trait AuthTarget {
    fn subject(&self) -> Subject;
    fn resource(&self) -> Resource;
}

impl AuthTarget for Principal {
    fn subject(&self) -> Subject { Subject { user_id: self.user_id, group_ids: self.group_ids.clone() } }
    fn resource(&self) -> Resource { Resource::Platform }
}

impl AuthTarget for WorkspaceContext {
    fn subject(&self) -> Subject { self.principal.subject() }
    fn resource(&self) -> Resource { Resource::Workspace(self.workspace_id) }
}

/// Enforce `action` for `target`. `Ok(())` if allowed, `Err(Error::Forbidden)` otherwise.
pub async fn require(pdp: &dyn Pdp, target: &impl AuthTarget, action: &str) -> crate::Result<()> {
    if pdp.authorize(&target.subject(), action, &target.resource()).await?.allow {
        Ok(())
    } else {
        Err(crate::Error::Forbidden)
    }
}
```
- **Platform call migration:** `require_permission(&p, "engine:create")?` → `require(&*st.pdp, &p, "engine:create").await?`.
- **Workspace call migration:** `st.authz.require_workspace(&ctx, "dashboard:read").await?` → `require(&*st.pdp, &ctx, "dashboard:read").await?`.
- **Behavior-preserving:** the B1a 60-cell regression matrix already proved the PDP's Platform/Workspace decisions equal the legacy platform/workspace decisions. `require()` returns the same `Error::Forbidden` on denial, so the HTTP mapping is unchanged.
- **Cost note:** a platform check now performs one `grants_for` query (previously an in-memory check on session-cached `Principal.permissions`); the workspace path already queried per request. This is a modest, indexed query; per-request memoization is a possible later optimization, not required here.

## 5. Proof migration — the `secrets` seam

`egret-controlplane/src/secrets.rs` gates ~4 handlers on `require_permission(&p, "secret:{read,create,update,delete}")`. Migrate each to `require(&*st.pdp, &principal, "secret:…").await?`. The handler already extracts the `Principal` (via `Auth`) and the state carries the pdp. No route/response shape changes. Existing secrets tests must still pass (same allow/deny outcomes); add/extend a test asserting a non-`platform_admin` principal holding `secret:read` (via a platform role) is allowed read and denied create — exercising the PDP path, not the old static gate.

Both `AuthTarget` overloads are unit-tested in this increment (a `Principal` resolves to `Platform`; a `WorkspaceContext` resolves to `Workspace(id)` and its subject carries the principal's group ids), even though only the platform overload is applied to routes here — the workspace overload is applied in B1b-3.

## 6. Testing

- `AuthTarget`/`require` unit tests (egret-core): both overloads map to the right `Subject`/`Resource`; `require` returns `Ok` on allow, `Error::Forbidden` on deny (with a stub `Pdp`).
- `Principal.group_ids` populated: a session for a user in N groups exposes those group ids.
- Secrets seam: existing tests green; new PDP-path allow/deny test.
- **F1 app-wiring e2e** (§2).
- Full local gate: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, frontend `lint/typecheck/test/build`.

## 7. Open questions (for the plan)

- `AppState` holds `Arc<dyn Pdp>` — confirm `FromRef` ergonomics and that `egret-server` can name the trait (it depends on `egret-core`).
- Whether `SessionService::validate` should load group ids in the existing `permissions` query (single round-trip) or a second query — prefer folding into the existing join.
- Confirm `secrets.rs` handlers all have the `Principal` in scope as `principal`/`p` (they do per B1a's catalog audit); adjust the variable name in the migration to match each site.

---

*Design v1.0 (2026-07-10). Wires the hot-reloading Cedar PDP into serving behind a single `require()` PEP and proves it on the secrets seam; the same-bus wiring closes review finding F1. Bulk migration (B1b-2/3) and legacy removal (B1b-final) follow.*
