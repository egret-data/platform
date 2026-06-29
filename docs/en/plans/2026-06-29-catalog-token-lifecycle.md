# Catalog Token Lifecycle + Table-Level Scope — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let catalog tokens be **edited** (label/permissions/scope) and **rotated** (new secret, same row), and add **table-level** scoping enforced by `EgretAuthorizer`.

**Architecture:** Add `table_scope text[]` + `rotated_at` columns; thread `table_scope` through the token store → `TokenAuth` → the front-auth `egret` claim → the authorizer's table-action gate (match the target table's `ns.name` FQN). Add `update_token` / `rotate_token` store methods + `PUT` / `rotate` routes + edit/rotate/table-scope UI.

**Tech Stack:** Rust (sqlx/Postgres, axum, embedded lakekeeper authz) + React 18 + Vitest.

**Spec:** `docs/en/specs/2026-06-29-catalog-token-lifecycle-design.md`.

---

## Background facts (verified)

- `catalog_tokens(id, workspace_id, warehouse_id, token_hash, label, permissions text[], namespace_scope text[], created_at, revoked_at)`; only `hash_token(raw)` (SHA-256 hex) is stored; `generate_raw_token()` → `egcat_…`.
- `tokens.rs` types: `CatalogToken { id, workspace_id, warehouse_id, label, permissions, namespace_scope, created_at }`, `TokenAuth { workspace_id, warehouse_id, permissions, namespace_scope }`, internal `TokenRow` / `AuthRow` (sqlx::FromRow). Helpers `validate_permissions`, `validate_namespace_scope`, `validate_namespace(ns)` (non-empty, no empty segments).
- `create_token(ws, wid, label, &[perm], &[ns]) -> (CatalogToken, raw)`; `authenticate_token(raw) -> TokenAuth` (SELECT … WHERE token_hash AND revoked_at IS NULL); `list_tokens_for_warehouse(ws, wid)`; `revoke_token_for_warehouse(ws, wid, id) -> bool` (warehouse-scoped guard).
- `browse.rs` token routes: `list_tokens` / `mint_token` / `revoke_token`, all `require_workspace_admin` + `st.store.get_warehouse(ws, wid)` ownership pre-check. `MintTokenRequest { label, permissions: Vec<String> (short "read"/"write"), namespace_scope }`, `map_permission("read"|"write")`, `CatalogTokenDto { id, label, permissions, namespace_scope, created_at }`, `MintTokenResponse { token, raw }`. Routes registered in `browse_router()` at `/ws/{workspace}/catalog/warehouses/{wid}/tokens` (get+post) and `…/tokens/{id}` (delete).
- `front_auth.rs` builds the claim: `.claims(serde_json::json!({ "egret": { "perms": auth.permissions, "ns": auth.namespace_scope } }))` (~line 139). `TokenAuth` is produced by `authenticate_token`.
- `authorizer.rs`: `egret_claim(metadata) -> Option<(Vec<String> perms, Vec<String> ns)>` (reads `v["perms"]`, `v["ns"]`); `gate(metadata, target_ns, is_write)`; `decide_scope(perms, ns_scope, target_ns, is_write)`; `ns_prefix_matches`. `are_allowed_table_actions_impl` maps each `(ns, action_on_table)` → `gate(metadata, ns.namespace_ident().as_ref(), table_action_is_write(&action))`. The table ident is reachable as `action_on_table.info.table_ident()` (`AuthZTableInfo::table_ident() -> &TableIdent`; `TableIdent { namespace: NamespaceIdent, name: String }`, `namespace.inner() -> Vec<String>` / `.as_ref()`).
- FE `api/catalog.ts`: `CatalogTokenDto { id, label, permissions, namespace_scope, created_at }`, `MintTokenRequest { label, permissions, namespace_scope }`, `MintTokenResponse { token, raw }`, hooks `useCatalogTokens/useMintToken/useRevokeToken`, `catalogKeys.tokens(ws, wid)`. `CatalogAccess.tsx`: `MintTokenDialog` (label, read/write checkboxes, namespaces textarea, two-phase form→reveal), list table, revoke confirm.

---

## Task 1: Migration + token store (`table_scope`, `update_token`, `rotate_token`)

**Files:**
- Create: `migrations/0035_catalog_token_table_scope.sql`
- Modify: `crates/egret-catalog/src/service/tokens.rs`
- Touch (force sqlx re-embed): `crates/egret-metastore/src/lib.rs`

### Step 1: Write the migration

`migrations/0035_catalog_token_table_scope.sql`:
```sql
-- Table-level token scope + rotation timestamp.
ALTER TABLE catalog_tokens
    ADD COLUMN table_scope text[] NOT NULL DEFAULT '{}',
    ADD COLUMN rotated_at  timestamptz;
```

### Step 2: Add `table_scope` to the store types + create + auth

In `tokens.rs`:
- Add `pub table_scope: Vec<String>` to `CatalogToken` (after `namespace_scope`) and to `TokenAuth`.
- Add `table_scope: Vec<String>` to `TokenRow` and `AuthRow`; map it in `From<TokenRow>` and wherever `TokenAuth` is built.
- Add a validator:
```rust
/// Validate a fully-qualified table scope entry: dotted `namespace.table`,
/// at least two non-empty segments (namespace + table).
fn validate_table_scope(scope: &[String]) -> Result<(), CatalogError> {
    for t in scope {
        let segs: Vec<&str> = t.split('.').collect();
        if segs.len() < 2 || segs.iter().any(|s| s.is_empty()) {
            return Err(CatalogError::BadRequest(format!(
                "invalid table scope '{t}': must be a dotted namespace.table with no empty segments"
            )));
        }
    }
    Ok(())
}
```
- Extend `create_token` to take `table_scope: &[String]`, call `validate_table_scope`, and INSERT/RETURNING the column.
- Extend `authenticate_token`'s SELECT + `AuthRow` to include `table_scope`, set it on `TokenAuth`.
- Update `list_tokens*` SELECTs + `TokenRow` to include `table_scope`.

### Step 3: Add `update_token` + `rotate_token` (warehouse-scoped)

```rust
/// Update a token's label/permissions/scopes in place (hash untouched, so the
/// existing secret keeps working). Scoped to (id, workspace, warehouse): returns
/// the updated row, or `NotFound` when the id is not this warehouse's token.
pub async fn update_token_for_warehouse(
    &self,
    workspace_id: Uuid,
    warehouse_id: Uuid,
    id: Uuid,
    label: &str,
    permissions: &[String],
    namespace_scope: &[String],
    table_scope: &[String],
) -> Result<CatalogToken, CatalogError> {
    validate_permissions(permissions)?;
    validate_namespace_scope(namespace_scope)?;
    validate_table_scope(table_scope)?;
    let row: Option<TokenRow> = sqlx::query_as::<_, TokenRow>(
        "UPDATE catalog_tokens \
         SET label = $4, permissions = $5, namespace_scope = $6, table_scope = $7 \
         WHERE id = $1 AND workspace_id = $2 AND warehouse_id = $3 AND revoked_at IS NULL \
         RETURNING id, workspace_id, warehouse_id, label, permissions, namespace_scope, table_scope, created_at",
    )
    .bind(id).bind(workspace_id).bind(warehouse_id)
    .bind(label).bind(permissions).bind(namespace_scope).bind(table_scope)
    .fetch_optional(self.pool()).await.map_err(CatalogError::Db)?;
    row.map(CatalogToken::from).ok_or_else(|| CatalogError::NotFound("catalog token".into()))
}

/// Rotate a token's secret: generate a new raw, overwrite token_hash for the
/// SAME row (label/scopes/created_at preserved, rotated_at = now). Returns the
/// row + the new raw (shown once). NotFound when not this warehouse's token.
pub async fn rotate_token_for_warehouse(
    &self,
    workspace_id: Uuid,
    warehouse_id: Uuid,
    id: Uuid,
) -> Result<(CatalogToken, String), CatalogError> {
    let raw = generate_raw_token();
    let hash = hash_token(&raw);
    let row: Option<TokenRow> = sqlx::query_as::<_, TokenRow>(
        "UPDATE catalog_tokens \
         SET token_hash = $4, rotated_at = now() \
         WHERE id = $1 AND workspace_id = $2 AND warehouse_id = $3 AND revoked_at IS NULL \
         RETURNING id, workspace_id, warehouse_id, label, permissions, namespace_scope, table_scope, created_at",
    )
    .bind(id).bind(workspace_id).bind(warehouse_id).bind(&hash)
    .fetch_optional(self.pool()).await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => CatalogError::AlreadyExists("token".into()),
        _ => CatalogError::Db(e),
    })?;
    let token = row.map(CatalogToken::from).ok_or_else(|| CatalogError::NotFound("catalog token".into()))?;
    Ok((token, raw))
}
```
`generate_raw_token` is currently `fn` (private to the module) — it's already callable here (same module). Keep it private.

### Step 4: Force sqlx re-embed + build

Run: `touch crates/egret-metastore/src/lib.rs && cargo build -p egret-catalog`
Expected: compiles (call-site of `create_token` in `browse.rs` will break until Task 3 — that's fine for this task's own crate build; if the workspace build is run, do Task 3 in the same branch before a full build). To keep this task self-contained, also update the single `create_token` call site in `browse.rs:1422` to pass `&req.table_scope` here (Task 3 finishes the route surface). Simplest: do Step 4's build as `cargo build -p egret-catalog` only.

### Step 5: Commit
```bash
git add migrations/0035_catalog_token_table_scope.sql crates/egret-catalog/src/service/tokens.rs crates/egret-metastore/src/lib.rs
git commit -m "feat(catalog): token table_scope column + update_token/rotate_token store methods"
```

---

## Task 2: Authorizer + front-auth (`tables` claim, table-FQN gate)

**Files:**
- Modify: `crates/egret-catalog-engine/src/front_auth.rs`
- Modify: `crates/egret-catalog-engine/src/authorizer.rs`
- Test: `crates/egret-catalog-engine/src/authorizer.rs` (`#[cfg(test)]` module — add cases)

### Step 1: front-auth claim carries `tables`

`front_auth.rs` (~line 139): change the claim to include the token's table scope:
```rust
.claims(serde_json::json!({
    "egret": { "perms": auth.permissions, "ns": auth.namespace_scope, "tables": auth.table_scope }
}))
```
(`auth: TokenAuth` now has `table_scope` from Task 1.)

### Step 2: Write failing authorizer tests

In the `#[cfg(test)]` module of `authorizer.rs`, add (adapt to the existing test helpers/imports there):
```rust
#[test]
fn table_scope_allows_listed_table_and_denies_others() {
    // perms=read, ns empty, tables=["sale.orders"]
    let perms = vec!["catalog:read".to_string()];
    let ns: Vec<String> = vec![];
    let tables = vec!["sale.orders".to_string()];
    // target table sale.orders → allowed (read)
    assert!(decide_table_scope(&perms, &ns, &tables, &["sale".into()], "orders", false));
    // sibling sale.customers → denied
    assert!(!decide_table_scope(&perms, &ns, &tables, &["sale".into()], "customers", false));
}

#[test]
fn empty_table_scope_falls_back_to_namespace_decision() {
    let perms = vec!["catalog:read".to_string()];
    assert!(decide_table_scope(&perms, &[], &[], &["sale".into()], "anything", false));
}
```

### Step 3: Implement the table-scope decision + wire the table gate

Add the table claim to `egret_claim` and a `decide_table_scope` helper, then use it in the table-action gate.

- Extend `egret_claim` to also return `tables` (make it return `(perms, ns, tables)`; update its one caller in `gate`, OR add a sibling `egret_claim_tables`). Cleanest: change `egret_claim` to `-> Option<(Vec<String>, Vec<String>, Vec<String>)>` and have `gate` ignore the third.
- Add:
```rust
/// Table-action decision: perm + (namespace_scope prefix) AND, when `table_scope`
/// is non-empty, the target table FQN (`ns.join(".") + "." + name`) must be listed.
fn decide_table_scope(
    perms: &[String],
    ns_scope: &[String],
    table_scope: &[String],
    target_ns: &[String],
    table_name: &str,
    is_write: bool,
) -> bool {
    if !decide_scope(perms, ns_scope, target_ns, is_write) {
        return false;
    }
    if table_scope.is_empty() {
        return true;
    }
    let mut fqn = target_ns.join(".");
    fqn.push('.');
    fqn.push_str(table_name);
    table_scope.iter().any(|t| t == &fqn)
}
```
- Add a `gate_table` method on `EgretAuthorizer` mirroring `gate` but using the table claim + name:
```rust
fn gate_table(&self, metadata: &RequestMetadata, target_ns: &[String], table_name: &str, is_write: bool) -> bool {
    if metadata.user_id() == Some(&self.system_principal) {
        return true;
    }
    match egret_claim(metadata) {
        Some((perms, ns, tables)) => decide_table_scope(&perms, &ns, &tables, target_ns, table_name, is_write),
        None => false,
    }
}
```
- In `are_allowed_table_actions_impl`, replace the `self.gate(...)` call with:
```rust
let ident = action_on_table.info.table_ident();
self.gate_table(
    metadata,
    ident.namespace().as_ref(),
    &ident.name,
    table_action_is_write(&action),
)
```
(`TableIdent::namespace() -> &NamespaceIdent`, `.as_ref() -> &[String]`; `.name` is the table name `String`. Confirm field/accessor names against the iceberg-rust fork at implementation; if `namespace()` isn't available use the `ns` already passed in the tuple — `ns.namespace_ident().as_ref()` — for the namespace and only take the name from `ident`.)
- Views/namespaces/generic-tables: leave calling `self.gate(...)` (unchanged).

### Step 4: Run tests + clippy
Run: `cargo test -p egret-catalog-engine authorizer` (unit tests, no DB needed) then `cargo clippy -p egret-catalog-engine --all-targets -- -D warnings`.
Expected: PASS + clean.

### Step 5: Commit
```bash
git add crates/egret-catalog-engine/src/front_auth.rs crates/egret-catalog-engine/src/authorizer.rs
git commit -m "feat(catalog): enforce table-level token scope in EgretAuthorizer + front-auth claim"
```

---

## Task 3: Routes (`PUT` update, `POST` rotate; mint + DTO carry `table_scope`)

**Files:**
- Modify: `crates/egret-catalog-engine/src/browse.rs`
- Test: `crates/egret-catalog-engine/tests/catalog_tokens_api.rs`

### Step 1: Write failing integration tests

Extend `catalog_tokens_api.rs` (mirror its existing fixture + helpers). Add tests:
- `mint_with_table_scope_persists_and_lists` — POST mint with `table_scope: ["sale.orders"]`; GET list shows it.
- `edit_token_updates_scope` — PUT changes label + permissions + namespace_scope + table_scope; GET reflects it; the original raw still authenticates (`authenticate_token`) with the new scope.
- `rotate_token_invalidates_old_secret` — capture raw1; POST rotate → raw2; `authenticate_token(raw1)` → NotFound, `authenticate_token(raw2)` → Ok with same id/label.
- `edit_and_rotate_cross_warehouse_404` — PUT/rotate with an `{id}` from another warehouse → 404.
- `table_scoped_token_denied_sibling_and_filtered_from_list` — (DB+MinIO+lakekeeper) mint a token scoped `["<ns>.t_allowed"]`, create two tables `t_allowed`/`t_denied`; via the iceberg REST front-auth path, loading `t_allowed` succeeds, `t_denied` is forbidden, and `list_tables` returns only `t_allowed`. (If wiring the full REST path in the test is heavy, at minimum assert the store/authorizer decision via a focused test and rely on the manual Trino test for the end-to-end; note which you did.)

### Step 2: Run to verify they fail
Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-catalog-engine --test catalog_tokens_api -- --test-threads=1`
Expected: FAIL/compile-error (routes + fields missing).

### Step 3: Add `table_scope` to the token surface + the two routes

In `browse.rs`:
- `CatalogTokenDto`: add `table_scope: Vec<String>` (map from `t.table_scope`).
- `MintTokenRequest`: add `#[serde(default)] table_scope: Vec<String>`. In `mint_token`, validate each entry maps OK and pass `&req.table_scope` to `create_token(ws, wid, label, &perms, &req.namespace_scope, &req.table_scope)`.
- Add an update body + handler:
```rust
#[derive(Deserialize)]
struct UpdateTokenRequest {
    label: String,
    permissions: Vec<String>,
    #[serde(default)] namespace_scope: Vec<String>,
    #[serde(default)] table_scope: Vec<String>,
}

/// PUT /ws/{ws}/catalog/warehouses/{wid}/tokens/{id} — edit label/permissions/scope.
async fn update_token(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, wid, id)): Path<(String, Uuid, Uuid)>,
    Json(req): Json<UpdateTokenRequest>,
) -> Result<Json<CatalogTokenDto>, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;
    let ws = ctx.workspace_id.0;
    st.store.get_warehouse(ws, wid).await?;
    if req.permissions.is_empty() {
        return Err(BrowseApiError::BadRequest("permissions must not be empty".into()));
    }
    let mut perms = req.permissions.iter().map(|p| map_permission(p)).collect::<Result<Vec<_>, _>>()?;
    perms.sort(); perms.dedup();
    let token = st.store
        .update_token_for_warehouse(ws, wid, id, &req.label, &perms, &req.namespace_scope, &req.table_scope)
        .await?;
    Ok(Json(token.into()))
}

/// POST /ws/{ws}/catalog/warehouses/{wid}/tokens/{id}/rotate — new secret, same row.
async fn rotate_token(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, wid, id)): Path<(String, Uuid, Uuid)>,
) -> Result<Json<MintTokenResponse>, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;
    let ws = ctx.workspace_id.0;
    st.store.get_warehouse(ws, wid).await?;
    let (token, raw) = st.store.rotate_token_for_warehouse(ws, wid, id).await?;
    Ok(Json(MintTokenResponse { token: token.into(), raw }))
}
```
- Register in `browse_router()`: add `.put(update_token)` to the existing `…/tokens/{id}` route (currently `delete(revoke_token)` → `axum::routing::put(update_token).delete(revoke_token)`), and a new route `…/tokens/{id}/rotate` → `post(rotate_token)`.

### Step 4: Run tests + fmt + clippy
Run: `DATABASE_URL=… cargo test -p egret-catalog-engine --test catalog_tokens_api -- --test-threads=1` → PASS.
Then `cargo fmt --all` + `cargo clippy -p egret-catalog-engine --all-targets -- -D warnings`.

### Step 5: Commit
```bash
git add crates/egret-catalog-engine/src/browse.rs crates/egret-catalog-engine/tests/catalog_tokens_api.rs
git commit -m "feat(catalog): token edit (PUT) + rotate routes; mint/list carry table_scope"
```

---

## Task 4: Frontend (edit dialog, rotate, table_scope field)

**Files:**
- Modify: `frontend/src/api/catalog.ts`
- Modify: `frontend/src/routes/explore/catalog/CatalogAccess.tsx`
- Test: `frontend/src/routes/explore/catalog/__tests__/CatalogAccess.test.tsx` (create if absent)

### Step 1: API types + hooks (`api/catalog.ts`)
- `CatalogTokenDto`: add `table_scope: string[]`.
- `MintTokenRequest`: add `table_scope: string[]`.
- Add `UpdateTokenRequest { label, permissions: string[], namespace_scope: string[], table_scope: string[] }`.
- Add hooks (mirroring `useMintToken`/`useRevokeToken`, invalidating `catalogKeys.tokens(ws, wid)`):
  - `useUpdateToken(ws, wid)` → `PUT /ws/${ws}/catalog/warehouses/${wid}/tokens/${id}` with the body; returns `CatalogTokenDto`.
  - `useRotateToken(ws, wid)` → `POST …/tokens/${id}/rotate`; returns `MintTokenResponse`.

### Step 2: UI (`CatalogAccess.tsx`)
- **Mint dialog:** add a `table_scope` textarea (same UX as the existing namespaces field) — free-text, split on newline/comma, trim, drop empties → `string[]`.
- **Per-row actions:** add **Edit** and **Rotate** alongside Revoke.
  - **Edit dialog:** reuse the mint form fields (label, read/write checkboxes pre-checked from `token.permissions.includes("catalog:read"|"catalog:write")`, namespaces + table_scope pre-filled from the row joined by `\n`); submit → `useUpdateToken`; no reveal.
  - **Rotate:** a confirm → `useRotateToken` → show the returned `raw` in the **same one-time reveal UI** the mint dialog uses.
- Show `table_scope` in the list row (like `namespace_scope`, e.g. "All tables" when empty).

### Step 3: Vitest
`CatalogAccess.test.tsx` (mock `@/api/catalog`): 
- mint dialog submits `table_scope` parsed from the textarea;
- edit opens pre-filled from a row and submits updated fields via `useUpdateToken`;
- rotate calls `useRotateToken` and reveals the returned raw;
- an invalid table_scope entry (single segment, e.g. `orders`) is rejected client-side (or surfaces the server 400).

### Step 4: Verify
Run (from `frontend/`): `npm run lint` · `npm run typecheck` · `npx vitest run` · `npm run build` — all clean.

### Step 5: Commit
```bash
git add frontend/src/api/catalog.ts frontend/src/routes/explore/catalog/CatalogAccess.tsx frontend/src/routes/explore/catalog/__tests__/CatalogAccess.test.tsx
git commit -m "feat(catalog): token edit + rotate UI and table_scope field"
```

---

## Final verification (before merge)

1. **Full local CI** (GitHub out of quota): `cargo fmt --all --check` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo test -p egret-catalog -p egret-catalog-engine` (DB+MinIO up) · FE `npm run lint`/`typecheck`/`vitest`/`build`.
2. **Manual real-Trino integration test** — REQUIRED for authz changes (`[[egret-manual-trino-integration-test]]`): run the real-Trino catalog+ranger manual test; additionally mint a table-scoped token and confirm via the iceberg REST path that it can read its table but not a sibling, and that listing hides the sibling.
3. **Rebuild + restart backend** (stale-binary trap, `[[egret-seed-catalog-table]]`), then **live smoke**: Access tab → mint with a table scope, Edit a token (change perms/scope), Rotate (new raw revealed), confirm the list shows table_scope.
4. **Admin squash-merge** with identity `hoangquocvietbk2.13@gmail.com`.
5. **Memory:** note sub-project B done in `[[egret-three-layer-management-model]]`.

---

## Self-review
- **Spec coverage:** edit (§1)→Task 3 `update_token` + Task 1 store + Task 4 UI; rotate (§2)→Task 3 route + Task 1 `rotate_token` + Task 4 UI; table-level (§3)→Task 1 column/store + Task 2 authorizer/front-auth + Task 3 mint/DTO + Task 4 field; testing (§Testing)→Tasks 2–4 + manual Trino. ✅
- **Placeholders:** none — migration SQL, store methods (full SQL), authorizer decision + gate, route handlers all given; FE contracts + test list explicit; JSX defers to existing `CatalogAccess.tsx`/`api/catalog.ts` patterns.
- **Type consistency:** `table_scope: Vec<String>` flows CatalogToken→TokenAuth→claim `tables`→`egret_claim` (now 3-tuple)→`decide_table_scope`; `update_token_for_warehouse`/`rotate_token_for_warehouse` signatures match their route call sites; FE `table_scope: string[]` matches the BE DTO field name.
