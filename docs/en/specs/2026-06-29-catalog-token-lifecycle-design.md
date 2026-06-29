# Catalog — token lifecycle (edit + rotate) + table-level scope — Design

**Date:** 2026-06-29 · **Status:** Design — approved · **Branch:** `feat/catalog-token-lifecycle` (off main)
**Program:** Catalog access, sub-project B (after the Browse redesign). Closes the token-management gaps: edit, rotate, and finer (table-level) scoping.

## Goal
Make catalog tokens fully manageable: **edit** a token's label/permissions/scope in place, **rotate** its secret, and scope a token down to **specific tables** (not just namespaces).

## Background (current state)
- `catalog_tokens(id, workspace_id, warehouse_id, token_hash, label, permissions text[], namespace_scope text[], created_at, revoked_at)`. Only the **SHA-256 hash** of the raw `egcat_…` is stored (raw shown once at mint). `egret-catalog/src/service/tokens.rs` has `create_token` / `list_tokens(_for_warehouse)` / `revoke_token(_for_*)` + `hash_token` + `generate_token`.
- Auth chain: front-auth hashes the presented Bearer → resolves `TokenAuth { workspace_id, warehouse_id, permissions, namespace_scope }` → builds the claim `{ "egret": { "perms": [...], "ns": [...] } }` (`front_auth.rs`). `EgretAuthorizer` reads that claim and gates data actions via `decide_scope(perms, ns_scope, target_ns, is_write)` (`authorizer.rs`) — **namespace-prefix** granularity only. The table-action gate (`are_allowed_table_actions_impl`) currently calls `gate(metadata, ns, is_write)` but has the full `TableIdent` available via `action_on_table.info.table_ident()`.
- Token routes in `browse.rs`: `GET/POST /ws/{ws}/catalog/warehouses/{wid}/tokens` (list/mint), `DELETE …/tokens/{id}` (revoke) — all gated `workspace:manage_members`, warehouse-ownership-checked. FE `CatalogAccess.tsx`: mint dialog (label, read/write, namespace_scope) + list + revoke.

## Decisions

### 1. Edit token (no re-issue)
- `PUT /ws/{ws}/catalog/warehouses/{wid}/tokens/{id}` — gated `workspace:manage_members`; updates `label`, `permissions[]`, `namespace_scope[]`, `table_scope[]` via a single `UPDATE` scoped by `(id, workspace_id, warehouse_id)` (cross-tenant `{id}` → 404). The `token_hash` is untouched, so the **same secret keeps working** with the new scope, effective on the token's next request (auth reads the row per call).
- `tokens.rs`: `update_token(id, ws, wid, label, permissions, namespace_scope, table_scope)`.
- FE: an **Edit** action per row → a dialog pre-filled from the row (same fields as mint, minus the reveal).

### 2. Rotate token
- `POST /ws/{ws}/catalog/warehouses/{wid}/tokens/{id}/rotate` — gated `workspace:manage_members`, ownership-checked. Generates a new `egcat_…`, overwrites `token_hash` for the **same id** (label/permissions/scopes/created_at preserved; sets a new `rotated_at`), returns `{ token, raw }` **once**. The previous secret stops authenticating immediately (its hash no longer matches any row).
- DB: add `rotated_at timestamptz` to `catalog_tokens`.
- `tokens.rs`: `rotate_token(id, ws, wid) -> (CatalogToken, raw)`.
- FE: a **Rotate** action → reuses the one-time reveal UI from mint.

### 3. Table-level scope
- **DB:** add `table_scope text[] NOT NULL DEFAULT '{}'` to `catalog_tokens` (migration `0035`).
- **Plumbing:** thread `table_scope` through `CatalogToken`, `TokenAuth`, `create_token`/`update_token`; the front-auth claim becomes `{ "egret": { "perms": [...], "ns": [...], "tables": [...] } }`.
- **Enforcement (`EgretAuthorizer`):** `egret_claim` also parses `tables`. The **table-action** path gains a table-aware decision: when `table_scope` is non-empty, the target table's fully-qualified name — `namespace_levels.join(".") + "." + table_name`, from `action_on_table.info.table_ident()` — must be present in `table_scope`, **in addition to** the existing perm + `namespace_scope` checks. Empty `table_scope` = no table-level restriction (current behaviour). Namespace-, view-, and generic-table-action gates are unchanged (governed by `namespace_scope`).
- **List filtering bonus:** lakekeeper invokes the table-action gate with `IncludeInList` per table, so a table-scoped token automatically **only sees its tables** in `list_tables` results — no extra code.
- **Semantics:** `namespace_scope` and `table_scope` are independent; for a *table* action both must pass when both are set. `table_scope` restricts **tables only** (views remain namespace-scoped; view-name scoping is out of scope).
- **UI:** the mint **and** edit dialogs get a `table_scope` field — **free-text fully-qualified `namespace.table` entries** (one per line / comma-separated), mirroring how `namespace_scope` is entered today; each entry validated as a dotted `ns.table` (≥2 segments). A tree/checkbox table picker is a future nicety.

### 4. Components touched
- Migration `0035_catalog_token_table_scope.sql` (`table_scope text[]` + `rotated_at`).
- `egret-catalog/src/service/tokens.rs` — `table_scope` on `CatalogToken`; `update_token`, `rotate_token`; `create_token` gains `table_scope`.
- `egret-catalog/src/service/...` `TokenAuth` — add `table_scope` (resolved alongside permissions/namespace_scope).
- `egret-catalog-engine/src/front_auth.rs` — claim `tables`.
- `egret-catalog-engine/src/authorizer.rs` — `egret_claim` parses `tables`; table-action gate matches the FQN.
- `egret-catalog-engine/src/browse.rs` — `PUT` + `rotate` routes; `mint` accepts `table_scope`.
- `frontend/src/api/catalog.ts` + `CatalogAccess.tsx` — edit dialog, rotate action, `table_scope` field, types.

## Testing
- **Backend integration (DB + MinIO + lakekeeper, mirror `catalog_tokens_api.rs`):** mint with `table_scope`; **edit** changes label/perms/scope and the new scope is enforced on the next call; **rotate** — old raw 401s, new raw works, row id/label unchanged; a **table-scoped** token can `GetMetadata`/read its table but is **denied** a sibling table in the same namespace **and** the sibling is absent from `list_tables`; a namespace-only token is unaffected; cross-workspace/warehouse `{id}` on PUT/rotate → 404; non-admin → 403.
- **Authorizer unit tests:** extend the `decide_scope`/gate tests with a table-FQN match/deny case.
- **Frontend (Vitest, mock `@/api/catalog`):** edit dialog pre-fills from the row and submits the updated fields; rotate shows the one-time reveal; `table_scope` free-text parses into entries; invalid entry (single segment) is rejected.
- **Manual real-Trino integration test** (per the standing rule for authz changes) before merge.

## Risks
- **Authz enforcement path change** — table-level gating runs on every table action; a logic error could over- or under-authorize. Mitigate: empty `table_scope` preserves exact current behaviour; explicit allow/deny integration tests incl. list filtering; run the manual Trino test.
- **Claim size** — many table entries enlarge the claim/JWT-like blob; acceptable for realistic counts (note as a soft limit, not enforced).
- **Edit is immediate** — narrowing a token's scope can break a running client on its next call; this is the intended, documented behaviour (same as revoke).

## Out of scope (later)
View-name scoping; wildcard table patterns (`ns.*`); a table-picker UI; token expiry/TTL; per-token last-used/audit.
