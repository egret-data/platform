# Catalog Engine CE-3 — Per-token permission scope — Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Each `catalog_tokens` token carries a `permissions` scope (`catalog:read`/`catalog:write`), enforced in egret's `front_auth` before lakekeeper. Read-only tokens can read but not mutate.

**Architecture:** Add `permissions text[]` to `catalog_tokens` (default full, back-compat). `authenticate_token` returns `(workspace_id, permissions)` in one lookup. `front_auth` classifies each request read/write by `(method, path)` and 403s if the token lacks the required permission. lakekeeper stays AllowAll (real Authorizer = CE-4).

**Tech Stack:** Rust, sqlx 0.8 (egret-catalog), axum middleware (egret-catalog-engine).

**Branch:** `feat/catalog-engine-ce3-token-scope` (off main). DB: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`; MinIO up.

**Spec:** `docs/en/specs/2026-06-27-catalog-engine-CE3-token-scope-design.md`.

**Existing seams:** `crates/egret-catalog/src/service/tokens.rs` (`create_token`, `workspace_for_token`, `CatalogToken`); `crates/egret-bin/src/main.rs` (`catalog mint-token`/`list-tokens` CLI); `crates/egret-catalog-engine/src/front_auth.rs` (ownership middleware + its DB-gated test).

---

## Task 1: `permissions` column + token store

**Files:** new `migrations/0027_catalog_token_permissions.sql` + touch `crates/egret-metastore/src/lib.rs`; `crates/egret-catalog/src/service/tokens.rs`; test `crates/egret-catalog/tests/store_tokens.rs`.

- [ ] **Step 1 (migration):**
```sql
ALTER TABLE catalog_tokens
  ADD COLUMN permissions text[] NOT NULL DEFAULT '{catalog:read,catalog:write}';
```
Touch metastore lib.rs (re-embed). Reset test DB: `docker exec egret-dev-postgres-1 psql -U egret -d egret -c "DROP DATABASE IF EXISTS egret_test; CREATE DATABASE egret_test;"`.

- [ ] **Step 2 (failing test):** in `store_tokens.rs` add:
  - `create_token_with_read_only_perms`: `create_token(ws, "ro", &["catalog:read".into()])` → `authenticate_token(raw)` returns `TokenAuth { workspace_id == ws, permissions == ["catalog:read"] }`.
  - `create_token_rejects_unknown_permission`: `create_token(ws, "x", &["catalog:admin".into()])` → `Err(CatalogError::BadRequest(_))`.
  - update the existing token tests to the new `create_token` arity (pass `&["catalog:read".into(),"catalog:write".into()]`) and use `authenticate_token` where they used `workspace_for_token`.
  Run → FAIL (compile).

- [ ] **Step 3 (implement tokens.rs):**
  - `create_token(&self, workspace_id: Uuid, label: &str, permissions: &[String]) -> Result<(CatalogToken, String), CatalogError>`: validate every perm ∈ `{"catalog:read","catalog:write"}` (else `BadRequest`); `INSERT INTO catalog_tokens (workspace_id, token_hash, label, permissions) VALUES ($1,$2,$3,$4) RETURNING ...` (bind `permissions` as `&[String]`); RETURNING includes `permissions`.
  - `pub struct TokenAuth { pub workspace_id: Uuid, pub permissions: Vec<String> }`.
  - `authenticate_token(&self, raw: &str) -> Result<TokenAuth, CatalogError>`: hash, `SELECT workspace_id, permissions FROM catalog_tokens WHERE token_hash = $1 AND revoked_at IS NULL`; map miss → `NotFound`.
  - `CatalogToken` struct gains `pub permissions: Vec<String>`; `list_tokens` SELECT includes it. Keep `workspace_for_token` only if still referenced (grep; the engine front_auth migrates to `authenticate_token` in Task 3 — if nothing else uses `workspace_for_token`, remove it).

- [ ] **Step 4:** Run `cargo test -p egret-catalog --test store_tokens` → PASS. `cargo fmt --all && cargo clippy -p egret-catalog --all-targets -- -D warnings`. Commit `feat(catalog): per-token permissions on catalog_tokens`.

---

## Task 2: CLI `--perms`

**Files:** `crates/egret-bin/src/cli.rs` + `main.rs` (the `catalog mint-token`/`list-tokens` handlers).

- [ ] **Step 1:** Add `--perms <String>` (default `"read,write"`) to `mint-token`. Parse comma-separated `read`/`write` → `["catalog:read"]` / `["catalog:write"]` / both; reject other tokens with a clear error. Pass to `create_token`.
- [ ] **Step 2:** `list-tokens` output includes the `permissions` column.
- [ ] **Step 3:** Update the existing `cli.rs` parse tests + add one for `--perms read`. `cargo build -p egret-bin`; `cargo test -p egret-bin`; clippy. Optional live smoke: `cargo run -p egret-bin -- catalog mint-token --workspace eng --perms read --label ro-smoke` (dev DB) then revoke.
- [ ] **Step 4:** Commit `feat(cli): mint catalog tokens with --perms scope`.

---

## Task 3: `front_auth` enforces permissions

**Files:** `crates/egret-catalog-engine/src/front_auth.rs`; test `crates/egret-catalog-engine/tests/front_auth.rs`.

- [ ] **Step 1 (classifier + unit test):** add a pure fn
```rust
/// Returns the permission a request requires. Default-deny: unknown mutating
/// methods/paths require write.
pub fn required_permission(method: &axum::http::Method, path: &str) -> &'static str
```
Rules (path is lakekeeper-relative, i.e. after the `/catalog/v1` nest strip):
  - `GET`/`HEAD` → `"catalog:read"`.
  - `POST` to `/config`, or a path ending `/metrics`, or ending `/aws/s3/sign` → `"catalog:read"`.
  - `DELETE` → `"catalog:write"`.
  - any other `POST` (namespaces create, properties, tables create/commit, register, rename, views create) → `"catalog:write"`.
  - anything else → `"catalog:write"` (default-deny).
  Unit-test it for each route/method (GET load=read, POST create=write, DELETE=write, POST sign=read, POST config=read, POST metrics=read).

- [ ] **Step 2 (enforce):** in the middleware, after the ownership check, switch `workspace_for_token` → `authenticate_token` (now have `permissions`). Compute `required_permission(method, path)`; if `!permissions.contains(required)` → `StatusCode::FORBIDDEN`. Then inject `RequestMetadata` as before. (The ownership check + `/config` handling are unchanged.)

- [ ] **Step 3 (integration test, DB+MinIO):** extend `front_auth.rs` tests: mint a read-only token (`create_token(ws, "ro", &["catalog:read".into()])`) and a read-write token for workspace A. Drive the mounted router:
  - read-only on A's prefix: `GET .../namespaces` → 200; `POST .../namespaces` (create) → **403**; `POST .../namespaces/{ns}/tables` → **403**; `DELETE` → **403**.
  - read-write on A's prefix: create namespace + table → 200.
  - regression (unchanged): token A on B's prefix → 403; garbage token → 401; valid token GET → 200.

- [ ] **Step 4:** `cargo test -p egret-catalog-engine -p egret-server` → green; `cargo build --workspace`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all --check`; single `iceberg`. Commit `feat(catalog-engine): enforce per-token read/write permissions in front_auth`.

---

## Self-review checklist
- [ ] `permissions` column added; existing tokens default to full; invalid perm → BadRequest.
- [ ] `authenticate_token` returns workspace + perms in one lookup; front_auth uses it.
- [ ] read-only token: read 200, write 403; read-write token: write 200; ownership 403/401 + IDOR unchanged.
- [ ] CLI `--perms`; `list-tokens` shows perms.
- [ ] suite green; clippy/fmt; single iceberg.

## Out of scope
`namespace_scope[]`, real lakekeeper Authorizer / S3-verb scoping (CE-4), CE-5/CE-6.
