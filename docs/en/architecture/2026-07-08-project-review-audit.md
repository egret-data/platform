# Egret — Whole-Project Review & Audit

> **Status:** Review · **Date:** 2026-07-08 · **Author:** architecture review (5-agent parallel audit)
> **Purpose:** A systematic, evidence-based snapshot of the codebase **as it actually is today**, to serve as the shared baseline for two follow-on efforts: **(A) architecture redesign / consolidation** and **(B) a unified authorization model**.
> This document reports *findings only* — no code was changed. Every claim is anchored to a concrete `crate/path/file.rs` or `migrations/NNNN_*.sql`.
> The canonical architecture doc (`2026-06-17-egret-architecture-design.md`, v0.2) is **materially stale** vs. the code; §7 lists the drift. That doc should be revised to v0.3 from this audit before (A) begins.

---

## 0. Executive summary

Egret is in **better shape than its size suggests**. The crate graph is a clean acyclic DAG, error handling has a canonical core type, tech-debt markers are near-zero (no `TODO/FIXME`, no `todo!()`/`panic!()` in product code), and several subsystems (egret-bi, egret-kernel, egret-database) have real integration-test coverage. The problems are **structural, not rot**:

1. **`egret-controlplane` is a god crate** (~7.3k LOC) bundling 5–6 unrelated bounded contexts, held together only by a `require_platform_admin` gate and a 13-field `CpState`. *Highest-leverage refactor.*
2. **Authorization is fragmented across 5 independent mechanisms** that share no decision engine, with the `platform_admin` bypass applied **four different ways** and **no object-level scoping** anywhere in platform RBAC. *This is the core driver for effort (B).*
3. **The architecture doc misrepresents ~40% of the real crate topology** — 4 large crates exist that the doc never mentions; 5 crates the doc lists don't exist.
4. **The catalog data model has been rewritten 3 times** (native → normalized → lakekeeper → Polaris → embedded-RBAC); only 4 tables survive, but the `catalog_*` prefix is overloaded across dead generations.
5. **No audit log exists.** The one `policy_audit` table has zero writers. RBAC grants, secret creation, and catalog-client minting are unaudited.
6. **Duplication hot-spots:** two independent Trino/StarRocks client implementations; the `egret_core::Error → HTTP` mapping copy-pasted in 7 files; two parallel ~40-struct engine-spec models bridged by a translation layer.

None of these block shipping; all of them make the system harder to reason about and extend — exactly the friction the user described as "the tool is growing bigger."

---

## 1. Crate map (reality, not the doc)

22 crates. LOC = `wc -l` over `src/**/*.rs` (includes inline tests). The dependency graph is a **clean DAG — no cycles**, kept acyclic by a deliberate *trait-seam* pattern (interfaces declared in a lower crate, implemented up in `egret-bin`).

| Crate | LOC | Real responsibility | Key interface |
|---|---|---|---|
| `egret-controlplane` | 7,323 | **God crate** — platform-admin HTTP for engine provisioning, warehouse/catalog admin, storage/secrets CRUD, Ranger policy editor, RBAC console, monitoring | `control_plane_router()`, `CpState`, `require_platform_admin` |
| `egret-bi` | 5,383 | BI: datasets, charts, dashboards, SQL Lab, chart-query→ECharts | `*_router`s, `BiState` |
| `egret-kernel` | 3,224 | Per-(user×workspace) Jupyter Kernel Gateway lifecycle | `KernelManager`, `KernelProxy`, `kernel_router` |
| `egret-orchestrator` | 3,083 | K8s provisioning of engines (Trino/Stackable); **zero egret-\* deps** (clean island) | `EngineProvisioner`, `TrinoProvisioner`, `KubeClient` |
| `egret-auth` | 2,206 | AuthN + identity: OIDC/SSO, sessions, CSRF, users/groups/roles (platform RBAC) | `SessionService`, `Auth` extractor, `*Repo` |
| `egret-bin` | 2,023 | Composition root / CLI; implements the trait seams | `main()` |
| `egret-notebook` | 1,875 | Notebook document CRUD/REST (no execution) | `NotebookStore`, `notebook_router` |
| `egret-engine` | 1,713 | Query execution: Trino (REST) + StarRocks (MySQL), JobManager | `Connector`, `JobManager`, `engine_router` |
| `egret-catalog-engine` | 1,617 | Embeds lakekeeper as in-process Iceberg REST catalog | `mount_catalog_engine`, `CatalogGuard` |
| `egret-workspace` | 983 | Workspace entity, membership, **workspace-scoped RBAC** (a 2nd authorizer) | `WsContext`, `Authorizer`, `*_router`s |
| `egret-database` | 900 | Per-workspace DB connection registry (register/test/health) | `DatabaseState`, `workspace_database_router` |
| `egret-catalog` | 887 | **Muddy** — storage-connection CRUD **and** the embedded catalog-client/role/grant store | `CatalogStore`, `service::embedded` |
| `egret-authz` | 589 | Ranger policy storage/versioning | `AuthzStore`, `RangerPolicy` |
| `egret-storage` | 520 | Object-store IO abstraction (S3/MinIO) | `ObjectStore` |
| `egret-config` | 454 | Typed config (figment) — leaf | `Config::load()` |
| `egret-secrets` | 448 | Envelope-encrypted named secrets | `WorkspaceSecretStore` |
| `egret-ranger` | 446 | Ranger Admin REST-compatible surface (`/service`) | `ranger_router` |
| `egret-machine-idp` | 425 | Embedded OAuth2 machine-IdP; mints egret-signed JWTs for catalog clients | `idp_router`, `ClientVerifier` |
| `egret-core` | 370 | Domain primitives — root leaf | `Principal`, `WorkspaceContext`, `Error`, `rbac` |
| `egret-server` | 259 | HTTP composition root: `AppState`, `build_router` (everything `Option<_>`) | `build_router` |
| `egret-iceberg-ext` | 221 | Iceberg metadata helpers — leaf | format types |
| `egret-metastore` | 64 | sqlx pool + embedded migrator (61 migrations) | `Db`, `MIGRATOR` |

**Structural observations**
- `egret-core` + `egret-metastore` are universal (expected). **`egret-auth` has become a *de facto* second universal dep** — 11 crates pull it, mostly for HTTP glue (`Auth` extractor, `csrf_layer`, `SessionService`); its role crept from "authN" to "the auth toolkit everyone imports."
- Surprising couplings: `egret-catalog → egret-machine-idp` (a "storage" crate implements `ClientVerifier`); `egret-controlplane → egret-catalog::service::embedded` internals for engine auto-wiring.
- `egret-orchestrator` is the best-bounded large crate — a dependency-free island, cleanly splittable.

---

## 2. Authorization model — the core of effort (B)

Authorization is split across **five enforcement mechanisms that do not share a decision engine.**

> **Correction to prior design docs:** ADR-16's `EgretAuthorizer` (lakekeeper `Authorizer` impl) + `catalog_tokens` table **no longer exist**. `catalog_tokens` was dropped in `0048_drop_embedded_catalog.sql`; lakekeeper now runs its permissive `AllowAllAuthorizer`, and egret's decision moved to a front-of-router `CatalogGuard` middleware over JWT claims + `catalog_grants` (`0053`).

### 2.1 The five mechanisms

1. **Platform RBAC (control-plane)** — catalog of 35 `resource:action` keys in `egret-core/src/rbac.rs`; enforced in-handler by `require_permission` / `require_platform_admin` in `egret-controlplane/src/auth.rs` (~90 call sites). Subject = session `Principal` (`platform_roles`, `permissions`); **resource = global, no object id**. Data: `roles`, `role_permissions`, `platform_role_assignments` (`0003`, seeded `0059`–`0061`).
2. **Workspace-scoped RBAC (a separate authorizer)** — `egret-workspace/src/rbac.rs` `Authorizer`, enforced via the `WsContext` extractor + `require_workspace` / `require_platform`. **Shares** the `roles`/`role_permissions` tables (distinguished by `roles.scope='workspace'`) but is a **distinct enforcement path**. Data: `workspace_members`.
3. **Catalog data-plane guard (Iceberg REST)** — `CatalogGuard` trait (`egret-catalog-engine/src/mount.rs:245`), impl in `egret-bin/src/main.rs:183`. Subject = a **machine JWT** (no end-user identity); validates warehouse-prefix ownership + `catalog_grants` coverage (`catalog ⊇ namespace ⊇ table`). Data: `catalog_clients`, `catalog_roles`, `catalog_grants` (`0053`).
4. **Data-policy layer (egret-authz + egret-ranger)** — native Ranger `RangerPolicy` JSON served at `/plugins/policies/download/{service}`; **evaluated inside Trino/StarRocks**, not in Egret. Serving-surface auth is a single **shared static bearer, anonymous when unset** (`egret-ranger/src/auth.rs:20`). Data: `authz_services`, `service_defs`, `authz_policies` (`0007`).
5. **Machine IdP** — `egret-machine-idp` mints RS256 JWTs (`client_credentials`) for catalog clients: `{sub=client_id, aud=catalog, warehouse, roles:[catalog_role], exp:+1h}`. Consumed by mechanism 3.

### 2.2 Overlaps, conflicts & gaps

- **[HIGH] `platform_admin` bypass applied 4 ways:** `require_permission` bypasses; `require_workspace` bypasses via `is_platform_admin()`; `require_platform` **does not** bypass (works only because `0003` *grants* platform_admin `workspace:*` — migration `0060`'s own comment warns dropping those grants would "silently lock platform_admin out"). The superuser property is **data-dependent, not structural.**
- **[HIGH] `workspace:*` keys enforced by two authorizers against two role scopes** — `workspace:read/create/update/archive` → `egret-workspace::require_platform` (platform-scope role); `workspace:manage_members` → `require_workspace` (workspace-scope role). One key prefix, two subject models.
- **[HIGH] No object-level scoping in platform RBAC** — `engine:update`, `warehouse:delete`, `secret:read`, `storage:delete` are global: holding the key authorizes the action on *every* object platform-wide. No per-resource owner / per-workspace filter (the missing `scope_id`).
- **[HIGH] Catalog data-plane has no end-user attribution** — authorizes a `(warehouse, catalog_role)` machine token, not a user; shared client ⇒ writes are indistinguishable across users; no per-user accountability, no catalog-write audit.
- **[MED] Two disjoint "role" systems** — RBAC `roles` vs per-warehouse `catalog_roles`; a user's role has zero relationship to what a catalog token can do.
- **[MED] Cross-scope grants are silently inert** — `set_role_permissions` lets an admin grant `workspace:manage_members` to a *platform* role, but the session never consults workspace membership, so it does nothing (no error, no effect).
- **[MED] Ranger serving surface can be fully anonymous** — empty token ⇒ any network peer can enumerate all governance policies.
- **[MED] `policy_audit` exists but is never written** (0 Rust writers) — no audit trail for any authorization change.

### 2.3 What a unified model must subsume (hard constraints)

1. **One subject model** covering session users, groups (direct + OIDC), **and** machine/service principals (today the catalog JWT `sub` and Ranger's engine-user live outside the RBAC graph).
2. **One resource taxonomy with object-level identity** — platform objects by id, workspace + workspace-scoped objects, catalog securables, engine data objects (schema/table/column/row).
3. **One structural superuser rule** (or explicit `*` grant) replacing the 4 divergent `platform_admin` treatments.
4. **One grant store** reconciling coarse RBAC (`resource:action`) with fine-grained data policy (row-filter/column-mask) and `catalog_roles`.
5. **Audit for every decision.**

**Genuinely hard:**
- **Engine-tier enforcement cannot move in-process** — Ranger policies are evaluated inside Trino/StarRocks; Egret can author & distribute but not evaluate them for engine queries.
- **Token scope ≠ user RBAC** — bridging the catalog plane requires per-user **delegation/downscoping** (mint a per-user catalog token from the session identity), not the current shared client-credential.
- **Identity reconciliation across planes** — Ranger authorizes by engine-user, Egret by session-user, catalog by machine-client — three namespaces needing a stable OIDC-subject mapping.
- **Object-level scoping is a schema change, not a refactor** — touches every control-plane table and every `require_permission` call site, and must define inheritance (workspace → warehouse → namespace → table).

Key files for (B): `egret-core/src/rbac.rs`, `egret-controlplane/src/auth.rs` + `egret-workspace/src/rbac.rs` (the two authorizers to merge), `egret-bin/src/main.rs:183` (catalog guard), `egret-machine-idp/src/lib.rs` (delegation seam), `egret-authz/` + `egret-ranger/` (engine-tier), migrations `0003`/`0053`/`0007`.

---

## 3. Data model & migrations

61 forward-only migrations (`0001`–`0061`), no down-migrations. Contiguous numbering, consistent `NNNN_snake_case` naming. lakekeeper's ~63 Iceberg tables live in a search-path-isolated `lakekeeper` schema (runtime-created, not in these files).

### 3.1 Live schema (egret-owned `public` tables, by domain)
- **Identity/RBAC:** `users`, `groups`, `user_groups`, `sessions`, `roles`, `role_permissions`, `platform_role_assignments`, `workspace_members`, `oidc_refresh_tokens`, `oidc_signing_keys`.
- **Workspace:** `workspaces`.
- **Catalog (embedded-RBAC, `0053`):** `warehouses`, `catalog_roles`, `catalog_grants`, `catalog_clients`.
- **Engine/DB:** `databases` (ws-scoped), `queries`, `saved_queries`, `engines` (global, provisioned), `engine_catalogs`.
- **Authz/Ranger:** `authz_service_defs`, `authz_services`, `authz_policies`, `policy_audit` (**dead**).
- **BI:** `datasets`, `dataset_columns`, `dataset_metrics`, `charts`, `dashboards`, `dashboard_cards`, `dashboard_tabs`, `bookmarks`, `folders`.
- **Notebook/kernel:** `notebooks`, `notebook_cells`, `notebook_engine_access`, `kernels`, `notebook_kernels`.
- **Secrets/storage:** `secrets`, `workspace_secrets`, `storage_connections`.

### 3.2 Findings
- **[HIGH] Catalog model churn** — native (`0006`) → normalized 22-table (`0023`) → dropped for lakekeeper (`0026`) → Polaris (`0051`) → Polaris dropped (`0054`) → embedded-RBAC (`0053`). ~40 catalog tables created & destroyed; 4 survive. `catalog_*` prefix overloaded across 3 dead generations. **The MVP "metadata-as-JSONB" debt is resolved** (normalized then removed). *Recommend baselining a fresh schema for GA.*
- **[HIGH] `databases.workspace_id` is nullable** (`0031`) — schema permits a globally-visible database row; workspace isolation is convention-only here. Should be `NOT NULL` after cleanup.
- **[MED] No schema-level same-workspace constraint** across FK chains (`charts→datasets`, `datasets→databases`, `notebook_engine_access→databases`) — cross-workspace joins prevented only in app code (IDOR surface).
- **[MED] `bookmarks`** — polymorphic `(entity_type, entity_id)`, **no FK, no workspace_id**; nothing cleans them up when the target is deleted.
- **[MED] Missing FK / FK-indexes** — `engine_catalogs.secret_ref` & legacy `secret_ref` are bare `uuid` (no `REFERENCES`); many FK columns unindexed (`*.folder_id`, `catalog_grants.catalog_role_id`, `warehouses.storage_conn_id`, `engines.warehouse_id`, `databases.secret_id`) → cascade deletes seq-scan.
- **[LOW] Three lifecycle idioms coexist** — `deleted_at` (BI) vs `status` enum (workspaces/databases/engines) vs hard-delete (catalog_clients). Enums-as-text with CHECK everywhere (value change = constraint-swap migration). `workspace_members.role` is free-text with no FK to `roles`.
- **Audit:** `policy_audit` is the only audit table and it's dead; all history/log tables were dropped with the old catalog. **No general `audit_log`.**

---

## 4. Product subsystems & quality

Unusually clean: **zero `TODO/FIXME/HACK`, no `todo!()`/`unimplemented!()`/`panic!()` in product src, one `#[allow(dead_code)]`.** The debt is architectural.

| Crate | ~test fns | Integration? | Top concern |
|---|---|---|---|
| egret-bi | 31+34 | Yes (12 files) | `routes.rs` 1651 LOC / 80 fns — split by resource |
| egret-notebook | 18+22 | Yes | `store.rs` 1023 LOC |
| egret-kernel | 26+25 | Yes | mutex `.lock().unwrap()` in request paths |
| egret-engine | 14+19 | Yes (8 files) | connector duplicated by egret-database |
| egret-database | 0+14 | Yes (5 files) | `tester.rs` re-implements Trino/StarRocks clients |
| egret-orchestrator | 40+0 | No | `trino.rs` 2153 LOC; not engine-generic |
| egret-catalog-engine | 2+3 | Thin | **least-tested crate wrapping a load-bearing dep**; `anyhow` only, no domain error → opaque 500s |

**Duplication hot-spots**
1. **Two Trino clients** — `egret-engine/connector/trino.rs` vs `egret-database/tester.rs::test_trino` (the latter doesn't even depend on engine).
2. **Two StarRocks clients** — both must independently remember the `prefer_socket(false)` gotcha.
3. **`egret_core::Error → HTTP` mapping copy-pasted in 7 files** — a drift in one won't propagate.
4. **Parallel "columns + rows" result types** reshaped at every boundary (`QueryResult` / `ChartResult` / `columns_jsonb` / `DatasetColumn`).
5. **Three token schemes** (kernel `egk_`, notebook JWT broker, catalog machine JWT) with no shared minting/hashing primitive.

**Error handling** — a canonical `egret_core::Error` exists and is used for the authz/validation arm, but each crate also rolls its own `XxxError` + near-identical `XxxApiError{Auth, Domain}` wrapper. `egret-catalog-engine` is the outlier (`anyhow` only).

---

## 5. Frontend architecture

**It is a flat single-package npm app, not the `apps/web + packages/*` monorepo the doc claims** (no `pnpm-workspace.yaml`, no `packages/`). React 18 + Vite + Tailwind v4, ~410 files, React Router v6 code-defined in `src/App.tsx`, two portals (`/explore/:workspace/*`, `/control-plane/*`). The "sparklabx design system" is a **vendored copy** (`src/components/ui/*`), not a package.

- **[HIGH — feeds B] Permission gating is scattered, not centralized** — no `can(perm)` helper, no `<Can>` component; inline `me.permissions.includes(...)` across ≥5 places (`auth/guards.tsx`, `App.tsx`, `Sidebar.tsx`, access pages). Keys are **bare string literals with no shared type**; FE references ~16 of the backend's 35. **Write actions gate coarsely on `isAdmin` (platform_admin)** rather than fine-grained `*:create/update/delete`, so a delegated role sees read pages but no write UI. Drift is silent (typo'd literal = silent deny).
- **[MED] API layer entirely hand-written** — no OpenAPI/utoipa codegen; DTOs re-declared by hand (BE/FE drift). `controlPlane.ts` is 933 lines. TanStack Query usage is inconsistent (query-key factories in some domains, inline arrays in others).
- **[MED] God components** — `WarehouseDetail.tsx` **2064 LOC**, `useEngineConfigForm.ts` **1383 LOC**, plus `ChartExplore.tsx` (986), `EngineDetail.tsx` (967).
- **[LOW] Two live OKLCH token bugs** in `components/chart/ChartTable.tsx:92,335` — `hsl(var(--token))` wrapping OKLCH tokens → invalid color → silent `none` (the exact anti-pattern from project convention). Only two `hsl(var(` occurrences and both are wrong.

---

## 6. Prioritized recommendations

Grouped by the two efforts, plus quick wins. These are *findings-driven suggestions*, not a committed plan — the plan comes from the follow-on brainstorming.

### Effort A — Architecture consolidation
- **A1 [HIGH]** Break up `egret-controlplane` along its module seams: engine-mgmt (`engines.rs` + orchestrator) → one vertical; policy-admin → `egret-authz`/`egret-ranger`; warehouse-admin → `egret-catalog-engine`; RBAC-access → `egret-auth`; storage/secrets CRUD → owning crates.
- **A2 [HIGH]** Collapse the duplicated engine-spec model (`controlplane/engines.rs` `*Req` vs `orchestrator/spec.rs` `EngineSpec`) into one.
- **A3 [MED]** Extract shared connectors (fold `egret-database/tester.rs` into `egret-engine`); unify a "tabular result" type in core.
- **A4 [MED]** Move `egret_core::Error → Response` into `egret-core` (kill the 7× copy); give `egret-catalog-engine` a real domain error + `IntoResponse`.
- **A5 [MED]** Clarify `egret-catalog`'s identity (split storage-connection registry from the catalog-client store); consider merging `egret-notebook`+`egret-kernel` and `egret-authz`+`egret-ranger`.
- **A6 [LOW]** Formalize & document the trait-seam pattern; consider `egret-auth-http` leaf to shrink auth's blast radius.

### Effort B — Unified authorization
- **B1 [HIGH]** Single decision point: one authorizer subsuming platform + workspace RBAC, one **structural** superuser rule.
- **B2 [HIGH]** Object-level scoping (`scope_id` + inheritance) — the missing dimension; schema + every call site.
- **B3 [HIGH]** Per-user catalog delegation (downscoped token from session identity) to give the data-plane end-user attribution + audit.
- **B4 [MED]** Reconcile the two role systems and the Ranger policy store into one grant model; secure the Ranger serving surface (per-engine auth, no anonymous).
- **B5 [MED]** Introduce a real `audit_log` and wire every authorization decision/mutation.
- **B6 [MED — FE]** Central `can(perm)`/`<Can>`, permission-key type generated from `rbac.rs`, gate writes on fine-grained keys.

### Quick wins (independent, low-risk)
- Fix the two OKLCH token bugs in `ChartTable.tsx`.
- Add the missing FK indexes in one sweep.
- Make `databases.workspace_id NOT NULL` (after cleanup); add the `engine_catalogs.secret_ref` FK.
- **Revise the architecture doc to v0.3 from this audit** before starting A — otherwise the redesign fights a fictional baseline.

---

## 7. Architecture-doc drift (v0.2 → needs v0.3)
- **Crates in code but absent from the doc:** `egret-controlplane` (largest!), `egret-orchestrator`, `egret-kernel`, `egret-machine-idp`.
- **Crates in the doc but not in code:** `egret-dataapi`, `egret-workflow`, `egret-notify`, `egret-monitor`, `egret-events` (monitoring lives as `controlplane/monitor.rs`).
- **`egret-notebook` labeled "(planned)" but fully built** (+ the Daft/kernel path shipped as `egret-kernel`).
- **Migrations:** doc references ~0029; code is at 0061.
- **ADR-16 stale:** `EgretAuthorizer` + `catalog_tokens` replaced by `CatalogGuard` + `catalog_grants` (see §2).
- **Frontend:** doc claims a monorepo; it's a flat npm app.
- **`egret-catalog` description** tells three contradictory stories across the doc and its own `lib.rs`.

---

*Review baseline for efforts A (architecture consolidation) and B (unified authorization). Update the canonical architecture doc to v0.3 from these findings before design work begins.*
