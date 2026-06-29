# Catalog Token Scope v2 (Resource Grants) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the flat `namespace_scope`+`table_scope` (AND) token model with an OR-evaluated list of **resource grants** (`{namespace, table?, permissions}`, per-grant read/write), built in the UI with namespace/table pickers.

**Architecture:** New `scope jsonb` column (grants) replaces the 3 scope columns; the claim carries `grants`; `EgretAuthorizer` evaluates grants with OR for table/namespace/view/generic actions; front-auth's coarse gate uses the union of grant perms. UI gains a multi-row grant builder.

**Tech Stack:** Rust (sqlx/Postgres jsonb, axum, embedded lakekeeper authz) + React 18 + Vitest.

**Spec:** `docs/en/specs/2026-06-29-catalog-token-grants-design.md`. **Supersedes** the scope model from PR #89 (migration 0035).

---

## Background facts (verified, current `feat/catalog-token-grants` = main after #89)

- `catalog_tokens(id, workspace_id, warehouse_id, token_hash, label, permissions text[], namespace_scope text[], table_scope text[], created_at, revoked_at, rotated_at)`.
- `tokens.rs`: `CatalogToken { id, workspace_id, warehouse_id, label, permissions, namespace_scope, table_scope, created_at }`, `TokenAuth { workspace_id, warehouse_id, permissions, namespace_scope, table_scope }`, `TokenRow`/`AuthRow` (sqlx::FromRow); `create_token(ws, wid, label, &[perm], &[ns], &[tbl])`, `update_token_for_warehouse(... label, &[perm], &[ns], &[tbl])`, `rotate_token_for_warehouse`, `authenticate_token`, `list_tokens_for_warehouse`; validators `validate_permissions` (subset of `{catalog:read,catalog:write}`), `validate_namespace`, `validate_namespace_scope`, `validate_table_scope`; `hash_token`, `generate_raw_token`; consts `PERM_READ`/`PERM_WRITE` live in authorizer.rs (`"catalog:read"`/`"catalog:write"`).
- `authorizer.rs`: `ns_prefix_matches(scope_entry, target) -> bool`; `decide_scope`; `decide_table_scope`; `egret_claim -> Option<(perms, ns, tables)>`; `claim_string_array`; methods `gate` / `gate_table`. Action impls each map `(ns, action)` → `self.gate(metadata, ns.namespace_ident().as_ref(), <X>_action_is_write(&action))` for namespace (`:~308`), view (`:~353`), generic (`:~376`); table (`:~330`) → `self.gate_table(metadata, ns.namespace_ident().as_ref(), &action_on_table.info.table_ident().name, table_action_is_write(&action))`. `table_action_is_write` / `namespace_action_is_write` / `view_action_is_write` classifiers exist and are exhaustive.
- `front_auth.rs`: `token_request_metadata(auth)` builds `.claims(json!({ "egret": { "perms": auth.permissions, "ns": auth.namespace_scope, "tables": auth.table_scope } }))`; `required_permission(method, path) -> Option<&str>` returns the needed `catalog:read`/`catalog:write` and the middleware checks `auth.permissions.contains(...)`.
- `browse.rs`: `CatalogTokenDto { id, label, permissions, namespace_scope, table_scope, created_at }`; `MintTokenRequest`/`UpdateTokenRequest { label, permissions: Vec<String> (short), namespace_scope, table_scope }`; `map_permission("read"|"write") -> catalog:*`; handlers `mint_token`/`update_token`/`rotate_token`/`list_tokens`/`revoke_token`.
- FE `api/catalog.ts`: `CatalogTokenDto`/`MintTokenRequest`/`UpdateTokenRequest` (with `namespace_scope`/`table_scope`), hooks `useCatalogTokens/useMintToken/useUpdateToken/useRotateToken/useRevokeToken`. `CatalogAccess.tsx`: `TokenFormFields` (label, read/write toggles, namespaces textarea, tables textarea), `RevealSecret`, mint/edit/rotate. `useCatalogNamespaces(ws,wid)` → `{levels}[]`; `useNamespaceContents(ws,wid,ns)` → `{tables, views}`.

**Grant shape** (canonical, used across layers):
- Storage jsonb + claim + Rust: `Grant { namespace: String, table: Option<String>, permissions: Vec<String> }` with **full** perm strings (`catalog:read`/`catalog:write`) in storage + claim.
- API (request + DTO): `{ namespace: string, table: string | null, permissions: ("read"|"write")[] }` (**short**), mapped at the route boundary.
- `namespace == "*"` is the all-namespaces sentinel.

---

## Task 1: Migration + token store (Grant model)

**Files:** Create `migrations/0036_catalog_token_scope.sql`; modify `crates/egret-catalog/src/service/tokens.rs`; touch `crates/egret-metastore/src/lib.rs`.

### Step 1: Migration

`migrations/0036_catalog_token_scope.sql`:
```sql
-- Token scope v2: a single jsonb list of resource grants replaces the flat
-- permissions / namespace_scope / table_scope columns.
ALTER TABLE catalog_tokens ADD COLUMN scope jsonb NOT NULL DEFAULT '[]'::jsonb;

-- Backfill existing rows into grants.
-- (a) no namespace_scope AND no table_scope -> one all-namespaces grant.
UPDATE catalog_tokens
   SET scope = jsonb_build_array(jsonb_build_object(
       'namespace', '*', 'table', NULL, 'permissions', to_jsonb(permissions)))
 WHERE cardinality(namespace_scope) = 0 AND cardinality(table_scope) = 0;

-- (b) each namespace_scope entry -> a whole-namespace grant.
UPDATE catalog_tokens t
   SET scope = t.scope || ns.g
  FROM (
    SELECT id, jsonb_agg(jsonb_build_object(
        'namespace', n, 'table', NULL, 'permissions', to_jsonb(permissions))) AS g
      FROM catalog_tokens, unnest(namespace_scope) AS n
     GROUP BY id, permissions
  ) ns
 WHERE t.id = ns.id;

-- (c) each table_scope entry "a.b.tbl" -> {namespace:"a.b", table:"tbl"}.
UPDATE catalog_tokens t
   SET scope = t.scope || ts.g
  FROM (
    SELECT id, jsonb_agg(jsonb_build_object(
        'namespace', regexp_replace(fqn, '\.[^.]+$', ''),
        'table', regexp_replace(fqn, '^.*\.', ''),
        'permissions', to_jsonb(permissions))) AS g
      FROM catalog_tokens, unnest(table_scope) AS fqn
     GROUP BY id, permissions
  ) ts
 WHERE t.id = ts.id;

ALTER TABLE catalog_tokens
    DROP COLUMN permissions,
    DROP COLUMN namespace_scope,
    DROP COLUMN table_scope;
```

### Step 2: `Grant` type + store rewrite

In `tokens.rs`:
- Define:
```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Grant {
    /// Dotted namespace prefix, or "*" for all namespaces.
    pub namespace: String,
    /// None = all tables in the namespace subtree; else an exact table name.
    #[serde(default)]
    pub table: Option<String>,
    /// Full perm strings: subset of {"catalog:read","catalog:write"}.
    pub permissions: Vec<String>,
}
```
- `CatalogToken`: replace `permissions/namespace_scope/table_scope` with `pub scope: Vec<Grant>`.
- `TokenAuth`: `pub grants: Vec<Grant>` + `pub permissions: Vec<String>` (the **union** of all grants' permissions, deduped — for the front-auth coarse gate).
- `TokenRow`/`AuthRow`: select `scope` as `sqlx::types::Json<Vec<Grant>>` (or `serde_json::Value` then `serde_json::from_value`). For `TokenAuth`, compute `permissions` = union over `grants`.
- Validator:
```rust
const ALL_NS: &str = "*";
fn validate_grants(grants: &[Grant]) -> Result<(), CatalogError> {
    if grants.is_empty() {
        return Err(CatalogError::BadRequest("scope must have at least one grant".into()));
    }
    for g in grants {
        if g.namespace != ALL_NS && !validate_namespace(&g.namespace) {
            return Err(CatalogError::BadRequest(format!("invalid grant namespace '{}'", g.namespace)));
        }
        if let Some(t) = &g.table {
            if t.is_empty() || t.contains('.') {
                return Err(CatalogError::BadRequest(format!("invalid grant table '{t}'")));
            }
            if g.namespace == ALL_NS {
                return Err(CatalogError::BadRequest("a table grant needs a concrete namespace".into()));
            }
        }
        validate_permissions(&g.permissions)?; // reuse: non-empty subset of catalog:read/write
    }
    Ok(())
}
```
- `create_token(ws, wid, label, &[Grant])` / `update_token_for_warehouse(... label, &[Grant])`: validate, store `scope` via `serde_json::to_value`/`Json(...)`. `RETURNING ... scope`. (`rotate_token_for_warehouse` unchanged except its `RETURNING`/`TokenRow` now has `scope`.)
- `authenticate_token`: select `scope`; parse to `Vec<Grant>`; `TokenAuth { ..., grants, permissions: union }`.
- Remove `validate_namespace_scope`/`validate_table_scope`/`validate_permissions`-as-top-level only if unused (keep `validate_permissions` — reused by `validate_grants`).

### Step 3: build the crate, then commit
Run: `touch crates/egret-metastore/src/lib.rs && cargo build -p egret-catalog`. (Call sites in `browse.rs`/`main.rs`/tests update in Task 3.)
Commit: `feat(catalog): token Grant model + scope jsonb store (migration 0036)`.

---

## Task 2: Authorizer + front-auth (grants claim, OR enforcement)

**Files:** `crates/egret-catalog-engine/src/front_auth.rs`, `crates/egret-catalog-engine/src/authorizer.rs` (+ its `#[cfg(test)]`).

### Step 1: front-auth claim + coarse gate
- `token_request_metadata`: claim becomes `json!({ "egret": { "grants": grants_json } })` where `grants_json` serializes `auth.grants` as `[{"ns","table","perms"}]`. Build it explicitly:
```rust
let grants: Vec<_> = auth.grants.iter().map(|g| serde_json::json!({
    "ns": g.namespace, "table": g.table, "perms": g.permissions
})).collect();
```
- The coarse `required_permission` check already reads `auth.permissions` — which is now the union — no change there.

### Step 2: Write failing authorizer unit tests
In `authorizer.rs` tests, add a `Grant`-based matrix (adapt helpers/imports):
```rust
fn g(ns: &str, table: Option<&str>, perms: &[&str]) -> Grant {
    Grant { namespace: ns.into(), table: table.map(|t| t.into()),
        permissions: perms.iter().map(|p| p.to_string()).collect() }
}
#[test]
fn mixed_grants_or_semantics() {
    let grants = vec![g("sale", None, &["catalog:read"]),
                      g("marketing", Some("campaigns"), &["catalog:read","catalog:write"])];
    // table actions
    assert!(decide_table(&grants, &["sale".into()], "orders", false));   // sale read
    assert!(!decide_table(&grants, &["sale".into()], "orders", true));   // sale write -> no
    assert!(decide_table(&grants, &["marketing".into()], "campaigns", true)); // mkt.campaigns write
    assert!(!decide_table(&grants, &["marketing".into()], "leads", false));   // other table
    assert!(!decide_table(&grants, &["finance".into()], "ledger", false));    // other ns
    // namespace navigation: a table-only grant lets you read its namespace + root
    assert!(decide_ns_read(&grants, &["marketing".into()]));
    assert!(decide_ns_read(&grants, &[]));                 // root list
    assert!(!decide_ns_write(&grants, &["marketing".into()])); // no whole-ns write grant
    assert!(decide_ns_write(&grants, &["sale".into()]) == false); // sale is read-only
}
#[test]
fn star_grant_covers_all() {
    let grants = vec![g("*", None, &["catalog:read"])];
    assert!(decide_table(&grants, &["anything".into()], "t", false));
    assert!(!decide_table(&grants, &["anything".into()], "t", true));
}
#[test]
fn empty_scope_denies() {
    assert!(!decide_table(&[], &["sale".into()], "orders", false));
    assert!(!decide_ns_read(&[], &["sale".into()]));
}
```

### Step 3: Implement the grant decisions + gates
Replace `decide_scope`/`decide_table_scope`/`egret_claim`/`gate`/`gate_table` with:
```rust
use egret_catalog::service::tokens::Grant; // or wherever Grant is exported

fn need(is_write: bool) -> &'static str { if is_write { PERM_WRITE } else { PERM_READ } }
fn grant_covers_ns(g: &Grant, target: &[String]) -> bool {
    g.namespace == "*" || ns_prefix_matches(&g.namespace, target)
}
fn decide_table(grants: &[Grant], target_ns: &[String], name: &str, is_write: bool) -> bool {
    let need = need(is_write);
    grants.iter().any(|g|
        g.permissions.iter().any(|p| p == need)
        && grant_covers_ns(g, target_ns)
        && g.table.as_deref().map_or(true, |t| t == name))
}
fn decide_ns_read(grants: &[Grant], target_ns: &[String]) -> bool {
    grants.iter().any(|g|
        g.permissions.iter().any(|p| p == PERM_READ)
        && (grant_covers_ns(g, target_ns) || ns_prefix_matches_levels(target_ns, &g.namespace)))
}
fn decide_ns_write(grants: &[Grant], target_ns: &[String]) -> bool {
    grants.iter().any(|g|
        g.table.is_none()
        && g.permissions.iter().any(|p| p == PERM_WRITE)
        && grant_covers_ns(g, target_ns))
}
fn decide_object_ns(grants: &[Grant], ns: &[String], is_write: bool) -> bool { // views + generic
    let need = need(is_write);
    grants.iter().any(|g|
        g.table.is_none()
        && g.permissions.iter().any(|p| p == need)
        && grant_covers_ns(g, ns))
}
```
- `ns_prefix_matches(scope_entry: &str, target: &[String])` exists. Add the reverse helper `ns_prefix_matches_levels(target_is_ancestor_of: &[String], grant_ns: &str)` = "is `target` an ancestor-or-self of `grant_ns`" (split `grant_ns` on `.`; `target.len() <= grant_levels.len() && target.iter().zip(grant_levels).all(eq)`); skip when `grant_ns=="*"` (covered already). Keep both pure + unit-tested.
- `egret_grants(metadata) -> Option<Vec<Grant>>`: `metadata.authentication()?.claims("egret")?`, read `grants` array, map each `{ns, table, perms}` → `Grant`. `None` (fail-closed) when no auth/claim.
- Gate methods (system bypass + `None => false` preserved):
  - `gate_table(metadata, target_ns, name, is_write)` → `decide_table`.
  - `gate_namespace(metadata, target_ns, is_write)` → `is_write ? decide_ns_write : decide_ns_read`.
  - `gate_object(metadata, ns, is_write)` → `decide_object_ns` (views + generic).
- Action impls:
  - table impl → `self.gate_table(metadata, ns.namespace_ident().as_ref(), &action_on_table.info.table_ident().name, table_action_is_write(&action))` (unchanged call site, new body).
  - namespace impl → `self.gate_namespace(metadata, ns.namespace_ident().as_ref(), namespace_action_is_write(&action))`.
  - view impl → `self.gate_object(metadata, ns.namespace_ident().as_ref(), view_action_is_write(&action))`.
  - generic impl → `self.gate_object(metadata, ns.namespace_ident().as_ref(), <generic>_is_write(&action))` (keep its existing classifier).

### Step 4: tests + clippy + commit
`cargo test -p egret-catalog-engine authorizer` → PASS; `cargo clippy -p egret-catalog-engine --all-targets -- -D warnings`.
Commit: `feat(catalog): OR resource-grant enforcement in EgretAuthorizer + grants claim`.

---

## Task 3: Routes (DTO/request carry `scope`)

**Files:** `crates/egret-catalog-engine/src/browse.rs`; test `crates/egret-catalog-engine/tests/catalog_tokens_api.rs`.

### Step 1: failing integration tests
Extend `catalog_tokens_api.rs` (mirror the fixture). Cover: mint with a multi-grant scope (e.g. `[{sale,null,[read]},{marketing,campaigns,[read,write]}]`) → list DTO reflects it; edit replaces scope → `authenticate_token` returns the new grants; empty scope → 400; cross-warehouse 404; rotate keeps scope.

### Step 2: route surface
- `GrantDto` (API, short perms): `{ namespace: String, table: Option<String>, permissions: Vec<String> }`. Map to/from `Grant` (`map_permission` short→full; add `unmap_permission` full→short for the DTO).
- `CatalogTokenDto`: replace `permissions/namespace_scope/table_scope` with `scope: Vec<GrantDto>` (+ keep `id, label, created_at`; add `rotated_at: Option<...>` if cheap).
- `MintTokenRequest`/`UpdateTokenRequest`: `{ label: String, scope: Vec<GrantDto> }`. Convert each `GrantDto` → `Grant` (map perms, validate ≥1 via the store's `validate_grants`); call `create_token`/`update_token_for_warehouse` with `&[Grant]`.
- Drop `map`-of-top-level-permissions logic in mint/update (now per grant). Update `main.rs` CLI `create_token` call site to pass a single all-namespaces grant from its `--read/--write` flags (preserve CLI behaviour: `[{namespace:"*", table:None, permissions: <flags>}]`).

### Step 3: tests + fmt + clippy + commit
`DATABASE_URL=… cargo test -p egret-catalog-engine --test catalog_tokens_api -- --test-threads=1`; `cargo fmt --all`; `cargo clippy --workspace --all-targets -- -D warnings`.
Commit: `feat(catalog): token mint/edit carry resource-grant scope`.

---

## Task 4: Frontend (grant builder with pickers)

**Files:** `frontend/src/api/catalog.ts`; `frontend/src/routes/explore/catalog/CatalogAccess.tsx`; test `__tests__/CatalogAccess.test.tsx`.

### Step 1: API types/hooks
- Replace `permissions/namespace_scope/table_scope` on `CatalogTokenDto`/`MintTokenRequest`/`UpdateTokenRequest` with `scope: Grant[]` where `Grant = { namespace: string; table: string | null; permissions: ("read"|"write")[] }`.
- Hooks unchanged in shape (`useMintToken`/`useUpdateToken` now send `{label, scope}`).

### Step 2: Grant builder UI (`CatalogAccess.tsx`)
- Replace the namespaces/tables textareas in `TokenFormFields` with a **GrantBuilder**: a list of rows, each:
  - **Namespace** `Select`: options = `useCatalogNamespaces(ws, wid)` dotted levels **plus** an "All namespaces" option (value `"*"`).
  - **Table** `Select`: when the row's namespace is a concrete ns, options = "All tables" (`null`) + `useNamespaceContents(ws, wid, ns).tables`; disabled/"All tables" only when namespace is `"*"`.
  - **Read** / **Write** toggles (per row).
  - **Remove** (✕) button.
  - "**＋ Add resource**" appends a row (default `{namespace: firstNamespaceOr"*", table: null, permissions: ["read"]}`).
- Validation: ≥1 grant; each grant ≥1 permission; surface the server 400 too. Submit `scope`.
- Edit dialog pre-fills the builder rows from `token.scope` (map full→short perms). Mint starts with one default row.
- List view: summarize each token's grants (e.g. `sale (read)`, `marketing.campaigns (read,write)`, `* (read)`), instead of the old namespace/tables columns.

### Step 3: Vitest
`CatalogAccess.test.tsx`: builder adds/removes rows; choosing a namespace loads its tables into the table select; ≥1-grant guard; mint submits the parsed `scope`; edit pre-fills from `token.scope` and submits; list renders the grant summary. (Mock `@/api/catalog` incl. `useCatalogNamespaces`/`useNamespaceContents`.)

### Step 4: verify + commit
From `frontend/`: `npm run lint` · `npm run typecheck` · `npx vitest run` · `npm run build` — clean.
Commit: `feat(catalog): resource-grant builder UI (namespace/table pickers, per-grant read/write)`.

---

## Final verification (before merge)
1. **Full local CI**: `cargo fmt --all --check` · `cargo clippy --workspace --all-targets -- -D warnings` · per-binary `cargo test -p egret-catalog -p egret-catalog-engine` (DB+MinIO up) · FE lint/typecheck/vitest/build.
2. **Rebuild + restart backend** (stale-binary trap), **live REST e2e** of mixed grants: mint `[{sale,null,[read]},{sale,"orders2",[read,write]}]`-style token; confirm read on the read grant, write only where granted, deny elsewhere, across two namespaces/tables. Clean up seeded artifacts.
3. **Manual real-Trino test** offered (authz change) per `[[egret-manual-trino-integration-test]]`.
4. **Admin squash-merge** (identity `hoangquocvietbk2.13@gmail.com`).
5. **Memory**: update `[[egret-three-layer-management-model]]` — token scope v2 supersedes the #89 flat model.

---

## Self-review
- **Spec coverage:** grant model (§Model)→Task 1; OR enforcement incl. navigation/write/`*`/empty (§Enforcement)→Task 2 + tests; storage/claim/API (§Storage)→Tasks 1–3; UI builder + pickers (§Components)→Task 4; testing→Tasks 2–4 + live e2e + Trino. ✅
- **Placeholders:** none — migration SQL, Grant type, decision fns (full code), gate wiring, validators given; routes + FE are contract-level mirroring existing handlers/`TokenFormFields`.
- **Type consistency:** `Grant{namespace,table:Option<String>,permissions}` flows store→`TokenAuth.grants`→claim `grants[{ns,table,perms}]`→`egret_grants`→`decide_*`; API `GrantDto` short perms ↔ `Grant` full via `map_permission`/`unmap_permission`; FE `Grant{namespace,table,permissions:("read"|"write")[]}` matches `GrantDto`. `permissions` union on `TokenAuth` feeds the unchanged front-auth coarse gate.
