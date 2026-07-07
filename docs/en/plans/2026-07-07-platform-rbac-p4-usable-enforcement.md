# Platform RBAC P4 — Make Enforcement Usable — Implementation Plan

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development.

**Goal:** Close the gaps that make egret's fine-grained RBAC actually usable + honest: (A) permission-aware **frontend** gating (so non-`platform_admin` roles can use the console), (B) reconcile **dead/drifted** catalog entries (`database:*`/`workspace:*`/`database:manage`) + a drift-lint, (C) fix the under-gated **`monitor:kill`**.

**Design basis:** Apache Superset study (`docs/…` research). Adopt Superset's *reuse-the-same-permission-for-menu-visibility* idea (minimal version: gate sections on `*:read`, no new `menu_access` permission kind). SKIP Superset's per-object ViewMenu model (too costly); defer object-scoping to a future `scope_id` column. Upgrade drift handling but do NOT copy silent-delete — hard-fail in debug.

**Spec:** `docs/en/specs/2026-07-07-platform-rbac-admin-design.md` (this is the follow-up "P4" pass). **Constraints:** DB tests on `egret_test`; touch metastore lib.rs after a migration; `cargo fmt --all`; no `cargo build -p egret-controlplane` alone; never run DB tests vs demo :5432.

---

### Task 1 (backend): reconcile catalog + expose permissions + fix monitor:kill

**Files:** `crates/egret-core/src/rbac.rs`, `crates/egret-controlplane/src/monitor.rs`, `crates/egret-auth/src/routes.rs`, `migrations/0059_rbac_permission_seed.sql` (edit), `migrations/0060_rbac_catalog_reconcile.sql` (new), `crates/egret-metastore/src/lib.rs` (touch), `crates/egret-bin/src/main.rs` (startup check), + a drift-lint test.

- **(B) Remove dead entries** from `PERMISSIONS` in `rbac.rs`: delete all `database:*` (`database:read/create/update/delete`) and all `workspace:*` (`workspace:read/create/update/archive`) — they have ZERO enforcement call sites (verified) and belong to the deferred workspace-scope epic. Keep the `Databases`/`Workspaces` groups out of the catalog until they're enforced.
- **(C) Add** `perm!("engine:kill", "Engines", "Kill running queries")` to the catalog. In `monitor.rs`, change the `kill_query` handler's guard from `require_permission(&p, "engine:read")?` to `require_permission(&p, "engine:kill")?` (the other monitor reads stay `engine:read`).
- **(A-backend) Expose permissions in `/me`**: add `pub permissions: Vec<String>` to `MeResponse` (`routes.rs`) and set it from `principal.permissions` in the `me` handler.
- **Migration `0060_rbac_catalog_reconcile.sql`** (new): remove now-orphaned grants so there's no drift —
  ```sql
  DELETE FROM role_permissions WHERE permission LIKE 'database:%' OR permission LIKE 'workspace:%';
  ```
  (This drops `database:manage`/`database:read`/`workspace:*` rows seeded by 0003+0059; `platform_admin` is unaffected — it bypasses; `data_steward` loses no-op reads.) Also **edit `0059`** to drop `database:read` and `workspace:read` from the `data_steward` seed array (they're leaving the catalog). Touch `crates/egret-metastore/src/lib.rs` to re-embed.
- **Drift-lint test** (`crates/egret-controlplane/tests/` or a unit test in the crate): read the control-plane source files (`engines.rs`, `warehouses.rs`, `storage.rs`, `secrets.rs`, `policies/mod.rs`, `monitor.rs`, `access/mod.rs`) via `include_str!` or `std::fs`, extract every `require_permission(_, "<key>")` string literal, and assert **every `egret_core::rbac::PERMISSIONS` key is enforced by ≥1 call site** (fail listing any catalog key with 0 enforcement — the reverse-drift check). `user:*`/`group:*`/`role:*` are enforced in `access/mod.rs`; `engine:kill` now in `monitor.rs`. If any catalog key legitimately can't be statically found (e.g. built via a helper), maintain a small explicit `ALLOWED_UNENFORCED: &[&str] = &[]` and assert it's empty. (Prefer `std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/…")))` so the test reads the live source.)
- **(E) Startup drift hardening** (`main.rs`): the existing warn-on-unknown-`role_permissions`-key check — in **debug builds** (`#[cfg(debug_assertions)]`) escalate to a `panic!`/hard error listing the offending keys; keep `tracing::warn!` in release. (Do NOT silent-delete.)
- Verify: `cargo test -p egret-core -p egret-controlplane --no-run` compiles; the drift-lint + rbac tests pass; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --all`. `cargo build -p egret-bin`.
- Commit `feat(rbac): reconcile catalog (drop dead database/workspace perms), add engine:kill, expose /me permissions, drift-lint + startup hardening`.

### Task 2 (frontend): permission-aware gating + sidebar filtering

**Files:** `frontend/src/auth/*` (the `me` type + a new guard), `frontend/src/components/layout/Sidebar.tsx`, `frontend/src/App.tsx`.

- **`me` type**: add `permissions: string[]` wherever `MeResponse` is typed (search for `platform_roles` in `src/auth`). 
- **New guard** in `frontend/src/auth/guards.tsx`:
  ```tsx
  export function RequirePermission({ perm, children }: { perm: string | string[]; children: ReactNode }) {
    const { me } = useAuth();
    const keys = Array.isArray(perm) ? perm : [perm];
    const ok = !!me && (me.platform_roles.includes("platform_admin") || keys.some(k => me.permissions.includes(k)));
    if (!ok) return <Forbidden />;
    return <>{children}</>;
  }
  ```
  And relax the control-plane outer gate: replace `RequireGlobalAdmin` around `ControlPlaneRoutes` with a `RequireControlPlaneAccess` that allows `platform_admin` OR **any** control-plane permission (so a scoped role reaches the area, then per-route guards apply). Keep `RequireGlobalAdmin` importable for the genuinely admin-only routes.
- **Per-route gating** (`App.tsx`): wrap each control-plane route element in `RequirePermission`:
  - engines → `engine:read`; warehouses → `warehouse:read`; storage → `storage:read`; secrets → `secret:read`; policies (+ profile/editor) → `policy:read`; users → `user:read`; groups → `group:read`; roles → `role:read`; permissions → `role:read`; workspaces stays `RequireGlobalAdmin` (not permission-gated yet).
- **Sidebar filtering** (`Sidebar.tsx`): give each control-plane `NavItem` a `perm?: string` (the section's read perm), and filter items so a user only sees a nav item if `me.platform_roles.includes("platform_admin") || (item.perm && me.permissions.includes(item.perm))`. Items without a `perm` (e.g. Workspaces) show only for `platform_admin`. Hide an empty group heading if all its items are filtered out. (This reuses the already-resolved `me.permissions` — no second computation, mirroring Superset's menu reuse.)
- **Tests**: update `Sidebar.test.tsx` (a scoped `me` with only `policy:read` sees Policies but not Engines/Users; `platform_admin` sees all). Add/adjust a guard test for `RequirePermission` (allow with perm, allow with platform_admin, deny without). Update any test that mocked `me` to include `permissions: []`.
- Gate: `npm run lint && npm run typecheck && npm run test && npm run build`.
- Commit `feat(frontend): permission-aware route + sidebar gating (RequirePermission)`.

### Task 3: gate + rebuild + browser smoke

- Backend gate: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test -p egret-core -p egret-auth -p egret-controlplane` on `egret_test` → pass (incl. drift-lint).
- Rebuild `serve`. Browser smoke: (1) as `admin@egret.local` (platform_admin) — full Access control + all sections visible, `monitor kill` still works. (2) Create a user with ONLY the `data_steward` role (via the Users page), log in as them (set a password), confirm they reach the control plane and see ONLY the sections their perms allow (Policies + read sections) and NOT Users/Roles management; hitting a forbidden route shows Forbidden. 0 console errors.
- Commit any fixes. P4 done → the epic is usable; proceed to full local CI → PR → merge.

## Notes
- Removing `database:*`/`workspace:*` is deliberate honesty (dead perms = drift). The future workspace-scope epic re-adds them WITH enforcement (and can add object `scope_id` then).
- Do not add a `menu_access` permission kind — section visibility reuses `*:read` (Superset's lesson, minimal form).
