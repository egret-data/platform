# Catalog Engine CE-4 — Minimal lakekeeper Authorizer + namespace scope — Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Replace lakekeeper's inert `AllowAllAuthorizer` with a minimal `EgretAuthorizer` that authorizes catalog data actions (namespace/table/view) by per-token scope, and add per-token `namespace_scope[]` restriction — defense-in-depth behind egret's `front_auth`.

**Architecture:** `front_auth` injects a non-admin **token principal** (`Actor::Principal`) carrying the egret scope in a custom `Authentication` claim `egret = {perms, ns}` (instead of the authz-bypassing `new_lakekeeper_internal`). `EgretAuthorizer` (copy of `AllowAllAuthorizer` with only the three data-action methods overridden) reads that claim and decides read/write + namespace-subtree. The egret **system principal** (bootstrap + in-process browse) is recognized by `UserId` and allowed for everything. Control-plane actions are allowed (front_auth gates entry).

**Tech Stack:** Rust, sqlx 0.8 (egret-catalog), sqlx 0.9 + lakekeeper (egret-catalog-engine), axum middleware, `async_trait`.

**Branch:** `feat/catalog-engine-ce4-authorizer` (off main). DB: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`; MinIO up (same as CE-2/CE-3 integration tests).

**Spec:** `docs/en/specs/2026-06-27-catalog-engine-CE4-authorizer-design.md`.

**Existing seams (verified):**
- `crates/egret-catalog/src/service/tokens.rs`: `create_token(ws,label,permissions)`, `authenticate_token(raw)->TokenAuth{workspace_id,permissions}`, `CatalogToken{...,permissions}` (CE-3).
- `crates/egret-bin/src/cli.rs` + `main.rs`: `catalog mint-token --perms`, `list-tokens`.
- `crates/egret-catalog-engine/src/front_auth.rs:172`: the `RequestMetadata::new_lakekeeper_internal(Uuid::now_v7())` injection point; ownership + `required_permission` checks are just above it.
- `crates/egret-catalog-engine/src/bootstrap.rs`: `system_user_id(system_user)->UserId` (UUIDv5, fixed `EGRET_SYSTEM_USER_NAMESPACE`), `system_request_metadata(system_user)->RequestMetadata` (builds `Authentication::builder().claims(json!({})).subject(uid.into())…` + `set_authentication(Actor::Principal(uid), auth)`). Copy this shape for the token principal.
- `crates/egret-catalog-engine/src/mount.rs:73`: `pub type EngineState = State<AllowAllAuthorizer, PostgresBackend, SecretsState>;` and `:78 EngineServer`; `build_state` constructs `AllowAllAuthorizer { server_id }` at ~`:113`.
- `bootstrap.rs:86` / `browse.rs:72`: `type EngineApiServer = ApiServer<PostgresBackend, AllowAllAuthorizer, SecretsState>;` (two more sites to swap).
- Reference skeleton: `data-portal/lakekeeper/crates/lakekeeper/src/service/authz/implementations/allow_all.rs` (full trait impl — copy it, override 3 methods).
- Action enums + accessors (lakekeeper): `CatalogTableAction{Drop,WriteData,ReadData,GetMetadata,Commit{..},Rename,IncludeInList,Undrop,GetTasks,ControlTasks,SetProtection}`; `CatalogNamespaceAction`/`CatalogViewAction` analogous (inspect `service/authz/mod.rs`). `NamespaceWithParent::namespace_ident()->&NamespaceIdent`; `AuthZNamespaceInfo::namespace()->&Namespace` + `namespace_id()`; the `_parent_namespaces: &HashMap<NamespaceId, NamespaceWithParent>` arg lets you resolve a namespace-action target's ident by `info.namespace_id()`. `NamespaceIdent` (iceberg) derefs to its `Vec<String>` levels (use `.as_ref()`/`.inner()` — confirm in source).

---

## Task 1: `namespace_scope` column + token store

**Files:** Create `migrations/0028_catalog_token_namespace_scope.sql`; modify `crates/egret-metastore/src/lib.rs` (touch); `crates/egret-catalog/src/service/tokens.rs`; test `crates/egret-catalog/tests/store_tokens.rs`.

- [ ] **Step 1 (migration):**
```sql
ALTER TABLE catalog_tokens
  ADD COLUMN namespace_scope text[] NOT NULL DEFAULT '{}';
```
Add a trivial whitespace/comment change to `crates/egret-metastore/src/lib.rs` so the embedded migrator re-embeds (the staleness gotcha). Reset test DB:
`docker exec egret-dev-postgres-1 psql -U egret -d egret -c "DROP DATABASE IF EXISTS egret_test; CREATE DATABASE egret_test;"`.

- [ ] **Step 2 (failing test):** in `store_tokens.rs`:
  - `create_token_with_namespace_scope`: `create_token(ws,"scoped",&["catalog:read".into(),"catalog:write".into()],&["marts".into(),"staging.raw".into()])` → `authenticate_token(raw)` returns `TokenAuth{ workspace_id==ws, permissions==[read,write], namespace_scope==["marts","staging.raw"] }`.
  - `create_token_empty_namespace_scope_means_all`: `create_token(ws,"all",&[read,write],&[])` → `authenticate_token` returns `namespace_scope == []`.
  - `create_token_rejects_malformed_namespace`: `create_token(ws,"x",&[read],&["".into()])` and `&[".bad".into()]` and `&["bad.".into()]` → `Err(CatalogError::BadRequest(_))`.
  - update existing token tests to the new `create_token` arity (append `&[]` for the namespace_scope arg).
  Run → FAIL (compile).

- [ ] **Step 3 (implement tokens.rs):**
  - `create_token(&self, workspace_id: Uuid, label: &str, permissions: &[String], namespace_scope: &[String]) -> Result<(CatalogToken, String), CatalogError>`: keep the existing perms validation; additionally validate each `namespace_scope` entry with a pure helper `fn validate_namespace(ns: &str) -> bool` = non-empty, no leading/trailing `.`, no empty dot-segment (`ns.split('.').all(|s| !s.is_empty())`). On violation → `CatalogError::BadRequest`. `INSERT INTO catalog_tokens (workspace_id, token_hash, label, permissions, namespace_scope) VALUES ($1,$2,$3,$4,$5) RETURNING …, namespace_scope` (bind `namespace_scope` as `&[String]`).
  - `pub struct TokenAuth { pub workspace_id: Uuid, pub permissions: Vec<String>, pub namespace_scope: Vec<String> }`.
  - `authenticate_token`: `SELECT workspace_id, permissions, namespace_scope FROM catalog_tokens WHERE token_hash = $1 AND revoked_at IS NULL`; populate all three.
  - `CatalogToken` gains `pub namespace_scope: Vec<String>`; `list_tokens` SELECT includes it.

- [ ] **Step 4:** `cargo test -p egret-catalog --test store_tokens` → PASS. `cargo fmt --all && cargo clippy -p egret-catalog --all-targets -- -D warnings`. Commit `feat(catalog): per-token namespace_scope on catalog_tokens`.

---

## Task 2: CLI `--namespaces`

**Files:** `crates/egret-bin/src/cli.rs` + `main.rs` (`catalog mint-token` / `list-tokens` handlers).

- [ ] **Step 1:** Add `--namespaces <String>` (default `""`) to `mint-token`. Parse comma-separated, trim, drop empties → `Vec<String>`; pass to `create_token` (alongside the existing `--perms` parse). Empty input → `vec![]` (= all).
- [ ] **Step 2:** `list-tokens` output includes a `namespaces` column: join with `,`, or `*` when empty.
- [ ] **Step 3:** Update existing `cli.rs` parse tests for the new arg; add one asserting `--namespaces marts,staging` → `["marts","staging"]` and absent → `[]`. `cargo build -p egret-bin`; `cargo test -p egret-bin`; clippy.
- [ ] **Step 4:** Commit `feat(cli): mint catalog tokens with --namespaces scope`.

---

## Task 3: `EgretAuthorizer` (standalone, unit-tested)

**Files:** Create `crates/egret-catalog-engine/src/authorizer.rs`; export from `crates/egret-catalog-engine/src/lib.rs`. No wiring yet (Task 4 swaps the type alias).

- [ ] **Step 1 (scaffold + failing unit tests):** Create `authorizer.rs`. Define:
```rust
#[derive(Clone, Debug)]
pub struct EgretAuthorizer {
    pub server_id: ServerId,
    pub system_principal: UserId,
}
```
Add pure helpers (unit-testable without lakekeeper State):
```rust
/// True if a write permission is required for this table action.
fn table_action_is_write(a: &CatalogTableAction) -> bool { /* match: Drop|WriteData|Commit{..}|Rename|Undrop|ControlTasks|SetProtection => true; ReadData|GetMetadata|IncludeInList|GetTasks => false */ }
fn namespace_action_is_write(a: &CatalogNamespaceAction) -> bool { /* create/drop/update-props/set-protection/control-tasks => true; get/list/include => false */ }
fn view_action_is_write(a: &CatalogViewAction) -> bool { /* drop/commit/rename/undrop/set-protection => true; get-metadata/include => false */ }

/// `scope_entry` (dot-split) is a subtree-prefix of `target` (dot-split levels).
fn ns_prefix_matches(scope_entry: &str, target: &[String]) -> bool {
    let want: Vec<&str> = scope_entry.split('.').collect();
    want.len() <= target.len() && want.iter().zip(target).all(|(a,b)| a == b)
}

/// Core decision. `egret_claim` is the `{perms,ns}` object (None ⇒ no claim).
fn decide_scope(perms: &[String], ns_scope: &[String], target_ns: &[String], is_write: bool) -> bool {
    let need = if is_write { "catalog:write" } else { "catalog:read" };
    if !perms.iter().any(|p| p == need) { return false; }
    if ns_scope.is_empty() { return true; }
    ns_scope.iter().any(|e| ns_prefix_matches(e, target_ns))
}
```
Write unit tests (no DB):
  - `table_action_is_write`/`namespace_action_is_write`/`view_action_is_write` for representative variants (Drop=write, GetMetadata=read, ReadData=read, Commit=write, IncludeInList=read).
  - `ns_prefix_matches`: `("marts", ["marts"])`=T, `("marts", ["marts","sales"])`=T, `("marts", ["marketing"])`=F, `("staging.raw", ["staging","raw","t"])`=T, `("staging.raw", ["staging"])`=F.
  - `decide_scope`: perms `[read]` + write → F; `[read,write]` + write → T; ns `[]` → any target T; ns `["marts"]` + target `["marketing"]` → F.
  Run `cargo test -p egret-catalog-engine --lib authorizer` → FAIL (compile / not impl).

- [ ] **Step 2 (implement the helpers + claim parsing):** Implement the four helpers above. Add:
```rust
/// Extract (perms, ns) from `metadata.authentication()?.claims("egret")`. None ⇒ no egret claim.
fn egret_claim(metadata: &RequestMetadata) -> Option<(Vec<String>, Vec<String>)> {
    let v = metadata.authentication()?.claims("egret")?;
    let perms = v.get("perms")?.as_array()?.iter().filter_map(|x| x.as_str().map(String::from)).collect();
    let ns = v.get("ns").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    Some((perms, ns))
}
```
Run unit tests → PASS.

- [ ] **Step 3 (impl the Authorizer trait):** Copy the **entire** `impl Authorizer for AllowAllAuthorizer` block from `lakekeeper/.../allow_all.rs` into `authorizer.rs` as `impl Authorizer for EgretAuthorizer` (same associated types, same `HealthExt`, same `ApiDoc`, `implementation_name() = "egret"`, `server_id()`/`new_router()`/`api_doc()` identical). Then override ONLY these three methods to evaluate per-tuple:
```rust
// Shared per-tuple gate:
fn gate(&self, metadata: &RequestMetadata, target_ns: &[String], is_write: bool) -> bool {
    if metadata.user_id() == Some(&self.system_principal) { return true; }       // bootstrap/browse
    match egret_claim(metadata) {
        Some((perms, ns)) => decide_scope(&perms, &ns, target_ns, is_write),
        None => false,                                                            // fail-closed
    }
}
```
  - `are_allowed_namespace_actions_impl`: for each `(info, action)`, resolve the target namespace levels — prefer `parent_namespaces.get(&info.namespace_id()).map(|n| n.namespace_ident())`, else fall back to the info's own namespace ident; convert `NamespaceIdent` → `Vec<String>` (confirm the accessor: `.as_ref().to_vec()` or `.inner()`). `bool = self.gate(metadata, &levels, namespace_action_is_write(&action))`.
  - `are_allowed_table_actions_impl`: for each `(parent_ns, action_on_table)`, `levels = parent_ns.namespace_ident()` → `Vec<String>`; `is_write = table_action_is_write(action_on_table.action())` (inspect `ActionOnTable` for the action accessor — likely `.action()` or a field; confirm in `service/authz/mod.rs`). `bool = self.gate(metadata, &levels, is_write)`.
  - `are_allowed_view_actions_impl`: analogous with `view_action_is_write`.
  - Leave `are_allowed_generic_table_actions_impl` and ALL control-plane / hook methods exactly as the AllowAll copy (allow / `Ok`).
  Add `mod authorizer;` + `pub use authorizer::EgretAuthorizer;` to `lib.rs`.

- [ ] **Step 4:** `cargo test -p egret-catalog-engine --lib` → PASS; `cargo build -p egret-catalog-engine`; `cargo clippy -p egret-catalog-engine --all-targets -- -D warnings`; `cargo fmt --all`. Commit `feat(catalog-engine): EgretAuthorizer (data-action scope + namespace subtree)`.

---

## Task 4: Scope bridge in front_auth + wire EgretAuthorizer

**Files:** `crates/egret-catalog-engine/src/front_auth.rs`; `mount.rs`; `bootstrap.rs`; `browse.rs`; `provisioner.rs`; test `crates/egret-catalog-engine/tests/front_auth.rs`.

- [ ] **Step 1 (token principal + bridge helper):** In `front_auth.rs` add, mirroring `bootstrap::system_user_id` but with a DISTINCT namespace+subject so it can never equal the system principal:
```rust
use lakekeeper::limes::Authentication;
use lakekeeper::service::authn::{Actor, UserId};
// fixed, random-but-constant namespace UUID literal, different from EGRET_SYSTEM_USER_NAMESPACE
const EGRET_TOKEN_USER_NAMESPACE: Uuid = uuid::uuid!("…generate one…");
fn token_principal_id() -> UserId {
    let subject = Uuid::new_v5(&EGRET_TOKEN_USER_NAMESPACE, b"egret-catalog-token");
    UserId::try_from(format!("oidc~{subject}").as_str()).expect("valid user id")
}
fn token_request_metadata(auth: &TokenAuth) -> RequestMetadata {
    let uid = token_principal_id();
    let authentication = Authentication::builder()
        .token_header(None)
        .claims(serde_json::json!({ "egret": { "perms": auth.permissions, "ns": auth.namespace_scope } }))
        .subject(uid.clone().into())
        .name(Some("Egret Catalog Token".to_string()))
        .email(None)
        .principal_type(None)
        .build();
    let mut md = RequestMetadata::new_lakekeeper_internal(Uuid::now_v7());
    md.set_authentication(Actor::Principal(uid), authentication);
    md
}
```

- [ ] **Step 2 (use the bridge):** Replace the injection at `front_auth.rs:172`:
```rust
req.extensions_mut().insert(token_request_metadata(&auth));
```
Keep the ownership check and the `required_permission` 403 above it unchanged (`auth` is the `TokenAuth` already in scope).

- [ ] **Step 3 (swap the Authorizer type everywhere):**
  - `mount.rs`: `pub type EngineState = State<EgretAuthorizer, PostgresBackend, SecretsState>;` and `pub type EngineServer = CatalogServer<PostgresBackend, EgretAuthorizer, SecretsState>;`. In `build_state`, change the signature to accept the system user — `build_state(dsn: &str, system_user: &str)` — and construct `let authz = EgretAuthorizer { server_id, system_principal: crate::bootstrap::system_user_id(system_user)? };` (make `system_user_id` `pub(crate)`). Update `build_state`'s caller(s) in `egret-bin/src/main.rs` to pass `catalog.system_user`.
  - `bootstrap.rs:86` and `browse.rs:72`: `type EngineApiServer = ApiServer<PostgresBackend, EgretAuthorizer, SecretsState>;`.
  - `provisioner.rs`: any `AllowAllAuthorizer` / state reference → `EgretAuthorizer` / `EngineState`.
  - `grep -rn AllowAllAuthorizer crates/egret-catalog-engine/src` must return NOTHING after this step (browse.rs:60 import removed too).

- [ ] **Step 4 (integration tests — DB+MinIO):** extend `front_auth.rs` tests. For workspace A with a provisioned warehouse:
  - **full-scope rw token** (`create_token(ws,"rw",&[read,write],&[])`): create namespace `marts`, create+commit a table under `marts`, `GET` it, create namespace `marketing` + a table under it → all 200/204. (Proves control-plane + data flow works under the real Authorizer — do this FIRST.)
  - **`marts`-scoped rw token** (`…,&["marts".into()]`): create a table under `marts` → 200; create a table under `marketing` (create the ns with the full-scope token first) → **403**; `GET` a `marts` table → 200; `GET`/list a `marketing` table → **403**.
  - **read-only full token**: `GET` → 200; any write → 403 (CE-3 regression).
  - **regression**: cross-workspace token A on B's prefix → 403; garbage token → 401.
  Use the same mounted-router `oneshot` harness as the existing front_auth test; assert lakekeeper returns 403 (forbidden) for the denied data actions.

- [ ] **Step 5 (defense-in-depth unit/integration test):** In the engine test crate, build a `RequestMetadata` via a test copy of `token_request_metadata` for a token with `perms=[read]`, `ns=["marts"]`, then call a lakekeeper `Service` (e.g. `TablesService::list_tables` or a create) in-process on a `marketing` namespace with that metadata (bypassing front_auth) → assert it is **denied** by the Authorizer. Also assert a `system_request_metadata` principal is **allowed** for the same call (bypass path). (If wiring a direct Service call is heavy, instead assert via `EgretAuthorizer` unit-level: construct it + a fake `RequestMetadata` with the egret claim and verify `gate`/the three methods return the expected `Vec<bool>` — but prefer the real Service path if the existing test harness already resolves a warehouse.)

- [ ] **Step 6:** Verify browse + bootstrap unaffected: `cargo test -p egret-catalog-engine -p egret-server` → green (browse tests exercise the system-principal bypass). `cargo build --workspace`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all --check`; confirm single `iceberg` (`cargo tree -i iceberg -e features | head`). Commit `feat(catalog-engine): enforce token scope + namespace_scope via EgretAuthorizer; bridge token principal in front_auth`.

---

## Self-review checklist
- [ ] `namespace_scope text[]` added (default `{}` = all); malformed entry → `BadRequest`; existing tokens unaffected.
- [ ] `authenticate_token` returns workspace + perms + namespace_scope in one lookup.
- [ ] `EgretAuthorizer` overrides ONLY the 3 data-action methods; all control-plane/hook methods allow (copied from AllowAll); `implementation_name()=="egret"`.
- [ ] `front_auth` injects the token principal with `claims["egret"]`; ownership + read/write checks unchanged; `new_lakekeeper_internal` no longer used for token requests.
- [ ] System principal recognized → bootstrap + browse still work.
- [ ] Full-scope token: all CRUD across namespaces works under the real Authorizer. `marts`-scoped token: allowed in `marts.*`, 403 outside. read-only: 403 on write. A→B 403; bad token 401.
- [ ] Defense-in-depth: scoped principal denied via direct Service path; system principal allowed.
- [ ] No `AllowAllAuthorizer` left in `egret-catalog-engine/src`. Workspace build/clippy/fmt green; single `iceberg`.

## Out of scope
Finer S3-verb scoping on `/sign`; per-token audit identity; full lakekeeper authz parity (roles/projects/server/OpenFGA + authz-management API); CE-5 (storage/secrets), CE-6 (GC workers).
