# Catalog Engine CE-3 — Per-token permission scope — Design

**Date:** 2026-06-27
**Status:** Design — approved
**Branch:** `feat/catalog-engine-ce3-token-scope` (off main; CE-1+CE-2 merged)
**Parent:** `docs/en/specs/2026-06-26-catalog-engine-lakekeeper-embed-design.md`; findings `docs/en/specs/2026-06-26-catalog-engine-CE1-findings.md`

## 0. Goal
Give each per-workspace `catalog_tokens` token its own **permission scope** (`catalog:read` / `catalog:write`), enforced in egret's `front_auth` middleware before lakekeeper. A read-only token can browse/read tables but cannot create/commit/drop. Multiple tokens per workspace, each with a different scope. (Namespace-level scoping `namespace_scope[]` is deferred to a later spec.) Real lakekeeper `Authorizer` (defense-in-depth) stays CE-4.

## 1. Decisions (locked)
- **Scope model (this spec):** `permissions text[]` only (`catalog:read` / `catalog:write`). `namespace_scope[]` deferred.
- **Enforcement:** in egret's `front_auth` (the middleware that already enforces per-workspace ownership) — NOT lakekeeper's `Authorizer` (that's CE-4). lakekeeper stays AllowAll inside; egret is the live boundary.
- **Back-compat:** existing tokens default to full perms `{catalog:read, catalog:write}`.

## 2. Components

### 2.1 Schema (migration)
```sql
ALTER TABLE catalog_tokens
  ADD COLUMN permissions text[] NOT NULL DEFAULT '{catalog:read,catalog:write}';
```
Existing tokens keep full access. (`catalog_tokens` was created in migration 0023, survived the CE-2 drop.)

### 2.2 `egret-catalog` token store (`service/tokens.rs`)
- `create_token(workspace_id, label, permissions: &[String]) -> (CatalogToken, raw)` — gains a `permissions` param; INSERT them. Validate each is `catalog:read` or `catalog:write`; reject others with `BadRequest`.
- Replace/augment `workspace_for_token(raw) -> Uuid` with `authenticate_token(raw) -> Result<TokenAuth, CatalogError>` where `TokenAuth { workspace_id: Uuid, permissions: Vec<String> }` — one lookup returns BOTH (the front_auth needs workspace + perms). Keep the `revoked_at IS NULL` filter + hash lookup. (Keep `workspace_for_token` if other callers need it, or migrate them.)
- `CatalogToken` DTO gains a `permissions: Vec<String>` field (for `list-tokens` display). NEVER expose the hash.

### 2.3 CLI (`egret catalog mint-token`)
- Add `--perms <read|write|read,write>` (default `read,write`). Parse to the `catalog:read`/`catalog:write` set; pass to `create_token`. `list-tokens` shows the permissions column.

### 2.4 `front_auth` enforcement (`egret-catalog-engine/src/front_auth.rs`)
After the existing ownership check (token→workspace→`{prefix}==warehouse`, 401/403), and using the SAME `authenticate_token` result (now carrying `permissions`):
- **Classify the request** by `(method, lakekeeper-relative path)`:
  - **read** (requires `catalog:read`): `GET`, `HEAD`; and `POST` to `/config`, `.../metrics`, `.../aws/s3/sign`.
  - **write** (requires `catalog:write`): every `DELETE`; `POST` to `…/namespaces` (create), `…/namespaces/{ns}/properties`, `…/namespaces/{ns}/tables` (create), `…/namespaces/{ns}/tables/{t}` (commit), `…/namespaces/{ns}/register`, `…/tables/rename`, `…/namespaces/{ns}/views` (create).
- If the token's `permissions` lacks the required one → **403** (fail-closed). Otherwise inject the lakekeeper `RequestMetadata` (unchanged — internal actor; AllowAll inside) and proceed.
- `/config` requires only a valid token (read) — unchanged ownership behavior.

**Classification helper:** a small pure fn `required_permission(method: &Method, path: &str) -> &'static str` returning `"catalog:read"` or `"catalog:write"`, unit-tested against the route table above. Default-deny posture: an unrecognized mutating method/path defaults to requiring `catalog:write` (safer to over-require).

## 3. Testing
- Unit: `required_permission` for each route/method (read vs write).
- Integration (DB+MinIO, extend `front_auth` test): mint a **read-only** token (`--perms read`) and a **read-write** token for a workspace; then:
  - read-only token: `GET` table/namespace list → 200; `POST` create-namespace / create-table / `DELETE` → **403**.
  - read-write token: create-namespace/table → 200.
  - Regression: cross-workspace ownership (token A on B) → 403; bad token → 401 (unchanged).
- token store test: `create_token` with `["catalog:read"]` → `authenticate_token` returns those perms; invalid perm string → `BadRequest`.

## 4. Out of scope (later)
- `namespace_scope[]` (per-namespace restriction) — its own spec (needs namespace extraction across all routes).
- Real lakekeeper `Authorizer` impl (RBAC + scope, deny-before-sign), finer S3-verb scoping on `sign` — **CE-4**.
- CE-5 (storage/secrets), CE-6 (GC workers).
