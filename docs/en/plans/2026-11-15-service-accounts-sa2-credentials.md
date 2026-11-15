# Service accounts SA2 — Unified credentials — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make catalog clients SA-owned credentials, bring the Catalog audience under the SA kill switch, give catalog credentials the same revoke/expire lifecycle as Data-API, and expose credential management under the service account. Backend only.

**Architecture:** `catalog.catalog_clients` gains `service_account_id` (nullable) + `revoked_at` + `expires_at`. The catalog `ClientVerifier` (both `verify` and `current`) joins to the owning SA's `users` row and refuses a disabled/revoked/expired/unowned client, mirroring the Data-API `ClientStore::verify` refusal. A ledger-correct CLI backfill creates one workspace-scoped SA per existing catalog client. A `Credential` repo abstraction unifies lifecycle (rotate/revoke/set-expiry) over the two audience tables, and a control-plane surface under `/service-accounts/{id}/credentials` lists and manages them.

**Tech Stack:** Rust, axum, sqlx (Postgres), `egret-machine-idp` (`ClientVerifier`), clap (egret CLI), utoipa.

## Global Constraints
- Branch `feat/svc-accounts-sa2` off **main that already contains SA1 (#867)** — worktree `../egret-sa2`, `CARGO_TARGET_DIR=target-sa2` on EVERY cargo command.
- Throwaway Postgres `DATABASE_URL` required; DB-gated tests MUST run, not skip.
- Chat VI; code/commits/docs EN. Conventional Commits; `Closes #N` only in PR body. Plain `git commit`; stage by explicit path (`target-sa2/` not gitignored — never `git add -A`). Co-author `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. No secrets.
- Migration number provisional; renumber above main AND open PRs at merge.
- `cargo fmt` + `cargo clippy -p <crate> --all-targets -- -D warnings` before each commit.
- SA2 does NOT build the frontend (SA3), does NOT consolidate the two creation UIs (SA3/later), does NOT touch the SA1 platform_admin invariant (SA4).

**Backfill mechanism (decided):** column is added **NULLABLE**; a **ledger-correct CLI backfill** (new `egret` subcommand, following `crates/egret-bin/src/backfill.rs`) creates one SA per existing catalog client via `UserRepo::insert_service_with` + `ensure_username_via` (so the `username_ledger` stays correct); the catalog verifier is **fail-closed on a NULL `service_account_id`** (an unowned client cannot mint until backfilled), so there is no fail-open window and NO second NOT-NULL migration is required. New catalog-client creation requires an SA (Task 4), so new rows are never NULL.

---

### Task 1: Migration — `service_account_id` + lifecycle columns on `catalog.catalog_clients`

**Files:**
- Create: `migrations/NNNN_catalog_client_service_account.sql`
- Test: extend/〔new〕 a DB-gated test in `crates/egret-catalog-engine/tests/` asserting the columns + FK exist and a client can be inserted with and without an SA.

**Interfaces:**
- Produces: `catalog.catalog_clients.service_account_id uuid NULL REFERENCES service_accounts(user_id) ON DELETE CASCADE`, `revoked_at timestamptz`, `expires_at timestamptz`, and an index on `service_account_id`.

- [ ] **Step 1: Write the migration.**
```sql
-- NNNN_catalog_client_service_account.sql
-- SA2: a catalog client becomes a credential of a service account, with the
-- same revoke/expire lifecycle as dataapi.clients. service_account_id is
-- nullable: existing rows are backfilled by the `egret backfill
-- catalog-client-sa` CLI, and the verifier is fail-closed on NULL so an
-- un-backfilled client cannot mint in the meantime.
ALTER TABLE catalog.catalog_clients
  ADD COLUMN service_account_id uuid REFERENCES service_accounts(user_id) ON DELETE CASCADE,
  ADD COLUMN revoked_at  timestamptz,
  ADD COLUMN expires_at  timestamptz;
CREATE INDEX catalog_clients_service_account_idx
  ON catalog.catalog_clients (service_account_id);
```
- [ ] **Step 2: Write the failing DB test** (model on an existing `egret-catalog-engine` DB-gated test): insert a `catalog.catalog_clients` row with `service_account_id = NULL` (allowed) and one with a real SA (seed a `service_accounts` row first); assert the FK rejects a bogus SA id; assert `revoked_at`/`expires_at` default NULL.
- [ ] **Step 3: Run it, verify it passes** — `CARGO_TARGET_DIR=target-sa2 cargo test -p egret-catalog-engine <test> -- --nocapture`. Must run, not skip.
- [ ] **Step 4: Commit.** `feat(sa): catalog clients gain an owning service account + revoke/expire (SA2)`

---

### Task 2: Catalog `ClientVerifier` — kill switch (gate on SA status + revoked/expired + fail-closed on NULL)

**Files:**
- Modify: `crates/egret-catalog-engine/src/store/embedded.rs` (the `verify` ~:846 and `current` ~:870 impls + the shared row reader ~:575-635)
- Modify: `crates/egret-machine-idp/src/verified_cache.rs` only if a status change can be served stale (confirm `current` is consulted on every cache hit; if `verify`/`current` both gate, the cache is safe)
- Test: DB-gated in `crates/egret-catalog-engine/tests/` — a disabled SA, a revoked client, an expired client, and a NULL-SA client each fail to verify; an active owned client still verifies.

**Interfaces:**
- Consumes: SA1's `service_accounts` + `users.status`. Mirrors the Data-API refusal in `crates/egret-dataapi/src/clients.rs` `ClientStore::minting_row`/`verify` (fetch `status, revoked_at, expires_at`; return `Ok(None)` for every failure — no distinguishing error, it is a probing oracle).

- [ ] **Step 1: Write failing tests.** Seed a warehouse + catalog_role + catalog client owned by a seeded SA. Assert:
  - active owned client → `verify` returns `Some(ClientState)`.
  - SA disabled (`users.status='disabled'`) → `verify` AND `current` return `Ok(None)`.
  - `revoked_at` set → `Ok(None)`.
  - `expires_at` in the past → `Ok(None)`.
  - `service_account_id IS NULL` → `Ok(None)` (fail-closed).
- [ ] **Step 2: Run, verify they fail.**
- [ ] **Step 3: Implement.** In the row reader the catalog `verify`/`current` use (~:575-635), change the SELECT to `JOIN service_accounts sa ON sa.user_id = c.service_account_id JOIN users u ON u.id = sa.user_id` and also select `u.status, c.revoked_at, c.expires_at, c.service_account_id`. After fetching, refuse with `Ok(None)` when: `service_account_id IS NULL`, OR `u.status <> 'active'`, OR `revoked_at IS NOT NULL`, OR `expires_at <= now()` — in a single refusal block, exactly like `ClientStore::minting_row`. Apply to BOTH `verify` and `current` (so a cached client whose SA was just disabled is revalidated to `None`). Keep `Ok(None)` for every failure (no oracle). Do not touch the `last_used_at` update semantics beyond adding the gate.
- [ ] **Step 4: Run, verify pass.** Also run the existing catalog-engine verifier tests — unchanged clients (now owned in the test via seeded SA) still verify.
- [ ] **Step 5: Commit.** `feat(sa): the SA kill switch reaches the catalog audience (SA2)`

---

### Task 3: Ledger-correct CLI backfill — one SA per existing catalog client

**Files:**
- Create: `crates/egret-bin/src/backfill_catalog_sa.rs` (or extend `crates/egret-bin/src/backfill.rs` — match the existing backfill module's shape)
- Modify: `crates/egret-bin/src/cli.rs` (add the subcommand), `crates/egret-bin/src/main.rs` (dispatch)
- Test: DB-gated — seed 2 catalog clients with NULL SA (different workspaces), run the backfill fn, assert each now has a distinct workspace-scoped SA whose `users` row is `provider='service'`, `svc-…` username (ledger row present), and that re-running is a no-op (idempotent).

**Interfaces:**
- Consumes: `UserRepo::insert_service_with` + `ensure_username_via` (the ledger-correct mint), `ServiceAccountRepo`/`service_accounts` insert from SA1, the existing `backfill.rs` CLI wiring.
- Produces: `egret backfill catalog-client-sa` (idempotent) — for each `catalog.catalog_clients WHERE service_account_id IS NULL`, in one tx per client: create a workspace-scoped SA (workspace_id = the client's workspace_id; deterministic name `catalog:{client.name}`; email/username derived so they never collide — key off the client id, e.g. local-part `svc-catalog-{client_id}`), then `UPDATE catalog.catalog_clients SET service_account_id = $sa WHERE id = $client`.

- [ ] **Step 1: Write failing DB test** per the Files/Interfaces above (idempotency asserted by running twice and counting SAs).
- [ ] **Step 2: Run, verify fail** (subcommand/fn absent).
- [ ] **Step 3: Implement** the backfill fn + wire the `clap` subcommand + `main.rs` dispatch. Reuse `ensure_username_via` so the `username_ledger` is correct (do NOT hand-insert usernames). One tx per client so a failure mid-run leaves the rest retryable; skip clients that already have an SA (idempotent). Derive the SA name/email deterministically from the client id to avoid collisions with human/other SAs.
- [ ] **Step 4: Run, verify pass** (incl. idempotent re-run).
- [ ] **Step 5: Commit.** `feat(sa): backfill an owning service account for existing catalog clients (SA2)`

---

### Task 4: Owner-on-create — every `create_client` path attaches a service account

> Discovered during execution: the fail-closed verifier (Task 2) refuses a catalog
> client with a NULL owner, so EVERY catalog-client creation path must attach an owner
> or new engines/warehouses break. There are ~5 call sites, so this is its own task
> (the user-facing credential API is Task 5).

**Files:**
- Modify: `crates/egret-catalog-engine/src/store/embedded.rs` (`NewClient` + `create_client` insert ~:514-540 — add a REQUIRED `service_account_id: Uuid` field so the compiler forces every caller to supply one).
- Modify the user-facing creator: `crates/egret-catalog-engine/src/warehouse_admin/mod.rs` (`create_client` route ~:855) — take/resolve an owning SA (synthesize a workspace-scoped SA for the client like the Task-3 backfill does, or accept one; keep it consistent with the backfill's per-client-SA model).
- Modify the engine-provisioning callers: `crates/egret-engine-mgmt/src/catalog_apply.rs` (~:1159) and `crates/egret-engine-mgmt/src/engines.rs` (~:4023, ~:5055, ~:10153) — each engine's minted catalog client gets a synthesized workspace-scoped SA (reuse the backfill primitives `insert_service_with` + `ensure_username_via` + the `service_accounts` insert; name/email keyed off the engine or client id for determinism/idempotency). Factor the "ensure an SA owns this new catalog client" logic into ONE reusable helper (e.g. in egret-auth or a small shared fn) rather than copy-pasting across 5 sites.
- Test: DB-gated — creating a catalog client (user-facing route AND an engine-provisioning path) yields a client with a non-NULL `service_account_id`, and that client then PASSES the Task-2 verifier (end-to-end: create → verify succeeds), proving no new NULL-owner clients are minted.

**Interfaces:**
- `NewClient` gains `service_account_id: Uuid` (required). `create_client` writes it. The "ensure/synthesize an owning SA" helper: `ensure_catalog_client_sa(tx, workspace_id, deterministic_key, name) -> Uuid` (ledger-correct), reused by the backfill (Task 3) if practical and by every creator here.

- [ ] **Step 1: Write failing tests** — a catalog client created via the warehouse-admin route has a non-NULL owner and verifies; an engine-provisioning mint has a non-NULL owner and verifies. (Reuse Task 1/2 seed helpers + the Task-2 verifier.)
- [ ] **Step 2: Run, verify fail** (the `service_account_id` field / helper absent).
- [ ] **Step 3: Implement** — add the required field to `NewClient`; write the one shared `ensure_catalog_client_sa` helper (ledger-correct, deterministic/idempotent, mirroring Task 3); fix every `create_client` caller to call it and pass the SA. Keep the engine-provisioning ordering intact (the existing code is careful about mint-before/after — do not reorder; only thread the owner through). If a caller lacks a workspace in scope, derive it from the warehouse (`warehouses.workspace_id`).
- [ ] **Step 4: Run, verify pass.** Run the egret-engine-mgmt + egret-catalog-engine suites (this task touches their creation paths) — they must stay green.
- [ ] **Step 5: Commit.** `feat(sa): every catalog client is created with an owning service account (SA2)`

---

### Task 5: `Credential` repo + lifecycle + the SA-scoped credential API

**Files:**
- Create: a `Credential` abstraction — `crates/egret-controlplane/src/access/credentials.rs` (or a small shared module) reading/writing both `dataapi.clients` and `catalog.catalog_clients`.
- Modify: `crates/egret-controlplane/src/access/mod.rs` (mount the routes). (Owner-on-create already landed in Task 4 — this task is the read/list + lifecycle + the user-facing credential CRUD under the SA.)
- Test: DB-gated in `crates/egret-controlplane/tests/` — list across audiences; create a catalog credential (carries warehouse/role) under an SA; rotate/revoke/set-expiry; the workspace-consistency guard.

**Interfaces:**
- Produces, under `/api/control-plane/service-accounts/{id}`:
  - `GET …/credentials` — `[{ id, audience: 'data-api'|'catalog', name, status: active|revoked|expired, created_at, last_used_at, expires_at, catalog?: { warehouse_id, catalog_role_id, consumer_type } }]`. Perm `user:read`.
  - `POST …/credentials` `{ audience, catalog?: { warehouse_id, catalog_role_id, consumer_type } }` → `{ …, secret }` (one-time). Perm: the credential-mint permission the existing routes use (confirm in code; else `user:update`).
  - `POST …/credentials/{cid}/rotate` | `/revoke` | `/expiry {expires_at}`. Perm `user:update`.
- Consumes: the existing dataapi client create/rotate (`dataapi/src/clients.rs`) and the catalog client create (`embedded.rs`), now owner-attached.

- [ ] **Step 1: Write failing tests** (model on `service_account_principals.rs`): list shows both audiences with correct status; create catalog cred under a workspace SA in the SAME workspace succeeds, in a DIFFERENT workspace is refused (workspace-consistency guard: a workspace SA owns only its workspace's credentials; a platform SA may own catalog creds in any workspace); rotate sets `revoked_at` on the old + returns a new secret; revoke makes the catalog verifier refuse (reuse Task 2); set-expiry in the past makes verify refuse.
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** the `Credential` repo (dispatch by audience to the right table), the routes + handlers (authz first on every handler), the workspace-consistency guard. Owner-attachment on creation already landed in Task 4; here the user-facing credential-create reuses it. Reuse the dataapi rotate/revoke logic; add the equivalent for catalog (`revoked_at`). utoipa-annotate + register DTOs/paths (regen is Task 6).
- [ ] **Step 4: Run, verify pass.** Existing SA1 `service_account_principals` + catalog tests still pass.
- [ ] **Step 5: Commit.** `feat(sa): manage service-account credentials across both audiences (SA2)`

---

### Task 6: OpenAPI + client regen; full gate

- [ ] **Step 1:** `CARGO_TARGET_DIR=target-sa2 cargo run -p egret-bin --bin egret -- openapi > openapi.json`; then `frontend/` `npm run gen:api`.
- [ ] **Step 2:** confirm the credential routes appear + CI drift tests pass (`committed_openapi_json_is_current`, `generated_client_was_emitted_from_the_committed_spec`).
- [ ] **Step 3:** full gate — `cargo fmt --check`; `cargo clippy -p egret-catalog-engine -p egret-controlplane -p egret-bin -p egret-engine-mgmt -p egret-machine-idp --all-targets -- -D warnings`; `cargo test -p egret-catalog-engine -p egret-controlplane -p egret-bin -p egret-engine-mgmt` (DB-gated run — includes the engine-provisioning suites touched by Task 4).
- [ ] **Step 4:** commit `chore(sa): regenerate OpenAPI + client for credential routes (SA2)`.

---

## Self-Review
- **Spec coverage:** schema+backfill → T1/T3; kill switch → T2; owner-on-create everywhere → T4; lifecycle + Credential repo + SA-scoped API → T5; regen + full gate → T6. Deferred items (FE, creation-UI consolidation, platform_admin invariant) named in Global Constraints.
- **Security:** T2 fail-closed on NULL SA + mirrors the dataapi `Ok(None)`-for-everything oracle discipline; gating BOTH `verify` and `current` closes the cache-staleness hole; backfill is ledger-correct (no hand-minted usernames); T4 makes `NewClient.service_account_id` REQUIRED so the compiler forbids any new NULL-owner client.
- **No placeholders:** the only "confirm in code" notes are the credential-mint permission string, the exact dataapi rotate/revoke helpers, and the ~5 `create_client` call sites — all read during planning from named files.
- **Type consistency:** `service_account_id`, `revoked_at`, `expires_at`, the `Credential`/audience shape, and the `/service-accounts/{id}/credentials` routes are identical across T1–T6.
