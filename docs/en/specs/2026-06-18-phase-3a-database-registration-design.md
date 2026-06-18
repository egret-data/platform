# Phase 3a — Database Registration Design Spec

> **Status:** Approved design · **Date:** 2026-06-18 · **Version:** 0.1
> Canonical (English). Vietnamese reference: [`docs/vi/specs/2026-06-18-phase-3a-database-registration-design.md`](../../vi/specs/2026-06-18-phase-3a-database-registration-design.md).
> Implements the database-registry foundation of [Master Architecture](../architecture/2026-06-17-egret-architecture-design.md) §10.1 / §9.2. This was a Phase 1 roadmap item that was skipped (only identity 1a shipped); it is the prerequisite for the Phase 3 query engine (3b) and SQL Lab (3c).

---

## 1. Goal & scope

Register the databases (query engines) Egret will run queries against: store connection config, encrypt credentials, grant databases to workspaces, and verify connectivity on demand. No query execution yet (that is Phase 3b).

**In scope**
- Two new crates: `egret-secrets` (AES-256-GCM credential encryption + Postgres `SecretStore`) and `egret-database` (database registry CRUD, workspace grants, on-demand connection test).
- **Engines:** Trino (REST) and StarRocks (MySQL). An `engine` enum; the connection test probes each natively.
- **Credentials:** AES-256-GCM, master key from config; per-secret nonce + ciphertext stored in a `secrets` table.
- **Connection test:** a real `SELECT 1` (verifies reachability + auth + query), not just a ping. Trino via the statement protocol; StarRocks via `mysql_async`.
- **Grants:** databases are registered globally (platform admin) then granted to workspaces; the Explorer lists a workspace's granted databases.
- Admin REST CRUD + test + grant; an Explorer listing endpoint.

**Out of scope (deferred)**
- Query execution, `JobManager`, `ResultStore`, result caching — Phase 3b (`egret-engine`).
- SQL Lab, datasets, saved queries — Phase 3c (`egret-bi`).
- Periodic health monitoring + `database_health` table + scheduler — later (this phase is on-demand test only).
- External KMS/Vault and key rotation — local config key only (the `key_id` column reserves rotation).
- Admin UI for database management (frontend) — a later Explorer/Admin frontend phase.
- Engine capability detection.

## 2. Decisions

| # | Decision | Choice | Grounding |
|---|---|---|---|
| 1 | Phase split | 3a registration first; engine = 3b, SQL Lab = 3c | Registration is the skipped prerequisite for all query work |
| 2 | Credential encryption | AES-256-GCM, master key from config | No external infra; mirrors the existing `security.secret_key` pattern |
| 3 | Connectivity | On-demand "test connection" (no periodic health yet) | Verify on register without a scheduler |
| 4 | Engines | Trino (REST) + StarRocks (MySQL) | Both are the architecture's MVP engines |
| A | Test depth | Real `SELECT 1` (auth + query), not a ping | Catches auth/permission issues at registration |
| B | StarRocks client | `mysql_async` (dedicated crate), not sqlx's `mysql` feature | Keeps sqlx's feature set (`postgres`) untouched; isolates the dep in `egret-database` |

## 3. Architecture / crate layout

```text
crates/
├── egret-secrets/
│   └── src/
│       ├── lib.rs
│       ├── crypto.rs    # AES-256-GCM seal()/open(): (nonce[12], ciphertext)
│       ├── store.rs     # SecretStore (Postgres): put/get/delete
│       ├── config.rs    # SecretsConfig { master_key }
│       └── error.rs     # SecretsError
│
└── egret-database/
    └── src/
        ├── lib.rs
        ├── model.rs     # Engine enum, Database row, ConnConfig, TestOutcome
        ├── repo.rs      # DatabaseRepo (CRUD over `databases`, seals creds via SecretStore)
        ├── grants.rs    # workspace_database_grant CRUD + grant-aware listing
        ├── tester.rs    # test_connection(engine, conn, secret) per engine
        ├── routes.rs    # admin CRUD/test/grant + me/ws list; DatabaseState
        └── error.rs     # DatabaseError
```

**Layering:** `egret-database::repo` uses `egret-secrets::SecretStore` to seal/open credentials; `tester` uses `reqwest` (Trino) and `mysql_async` (StarRocks). Depends on `egret-{core,config,metastore,auth,workspace}` + `egret-secrets`.

## 4. `egret-secrets`

- **Config:** `[secrets] master_key = "<base64, ≥32 bytes>"`.
- **Crypto** (`aes-gcm` crate, `Aes256Gcm`): `seal(plaintext: &str) -> (nonce: [u8;12], ciphertext: Vec<u8>)` with a random nonce; `open(nonce, ciphertext) -> String`. A tampered ciphertext or a wrong key fails on the GCM auth tag.
- **Store** over `secrets(id, kind, nonce, ciphertext, key_id, created_at)`:
  - `put(kind: &str, plaintext: &str) -> Uuid`
  - `get(id: Uuid) -> String`
  - `delete(id: Uuid)`
  - `key_id` defaults to `"cfg"`; it identifies which master key sealed the secret (reserved for rotation).
- **Deps:** `aes-gcm`, `rand`, `base64`, `uuid`, `sqlx`, `thiserror`.

## 5. Data model (`migrations/0008_database.sql`)

```sql
CREATE TABLE secrets (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    kind       text NOT NULL,                       -- 'db_password' | 'db_token'
    nonce      bytea NOT NULL,
    ciphertext bytea NOT NULL,
    key_id     text NOT NULL DEFAULT 'cfg',
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE databases (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name       text NOT NULL UNIQUE,
    engine     text NOT NULL CHECK (engine IN ('trino','starrocks')),
    host       text NOT NULL,
    port       int  NOT NULL,
    options    jsonb NOT NULL DEFAULT '{}',         -- user, http_scheme, catalog/database, ssl…
    secret_id  uuid REFERENCES secrets(id) ON DELETE SET NULL,
    status     text NOT NULL DEFAULT 'unknown',     -- unknown | ok | error (last test)
    created_by uuid REFERENCES users(id),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE workspace_database_grant (
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    database_id  uuid NOT NULL REFERENCES databases(id) ON DELETE CASCADE,
    granted_by   uuid REFERENCES users(id),
    granted_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, database_id)
);
```

## 6. Connection model & test

Non-secret connection fields live in `options` (jsonb); only the password/token goes to `secrets`.

| Engine | host/port | `options` | secret |
|---|---|---|---|
| Trino | host, 8080/8443 | `user`, `http_scheme` (http/https), `catalog?` | password/token (optional) |
| StarRocks | host, 9030 | `user`, `database?` | password |

**`test_connection` (Decision A — real `SELECT 1`)** returns `TestOutcome { ok: bool, latency_ms: u64, detail: String }` and updates `databases.status`:
- **Trino:** `POST {scheme}://{host}:{port}/v1/statement` with body `SELECT 1`, headers `X-Trino-User: <user>` (+ Basic/JWT from the secret if present); follow `nextUri` until state `FINISHED` or `FAILED`. A `FAILED` payload's `error` becomes `detail`. (This is a minimal slice of the full statement connector built in 3b.)
- **StarRocks (Decision B — `mysql_async`):** open a connection to `host:port` with `user`/password/`database`, run `SELECT 1`, read one row.
- Timeouts are bounded (connect + total, e.g. 10s) so a dead host fails fast.

## 7. REST surface + RBAC

**Admin** — requires a platform manage-database permission (`platform_admin` or `data_steward`). Mounted under `/api/admin` behind the existing session + CSRF stack (like the admin workspace routes).

| Method + path | Purpose |
|---|---|
| `POST /api/admin/databases` | Register: `{ name, engine, host, port, options, secret? }` → seal `secret`, insert row. |
| `GET /api/admin/databases` | List (no secret; each row carries `has_secret: bool`). |
| `GET /api/admin/databases/{id}` | Detail (no secret). |
| `PUT /api/admin/databases/{id}` | Update fields; if `secret` present, re-seal and replace (old secret deleted). |
| `DELETE /api/admin/databases/{id}` | Delete (and its secret). |
| `POST /api/admin/databases/{id}/test` | Test connection of a stored database. |
| `POST /api/admin/databases/test` | Test an unsaved connection (body = full config + secret) before registering. |
| `POST /api/admin/databases/{id}/grants` `{ workspace_id }` | Grant to a workspace. |
| `DELETE /api/admin/databases/{id}/grants/{workspace_id}` | Revoke. |

**Explorer** — `GET /api/me/databases?workspace=<slug>`: the databases granted to that workspace (the SQL Lab DB picker uses this in 3c). A workspace member sees its grants; `platform_admin` sees all. Returns `{ id, name, engine }` (no connection internals).

**Credential hygiene:** the plaintext password/token is accepted only on register/update; it is never returned by any read endpoint. Detail responses expose `has_secret` and the non-secret `options` only.

## 8. Config + wiring

- **`egret-config`:** add `[secrets] master_key: String`. `validate()`: if non-empty, it must base64-decode to ≥32 bytes; registering a database **with** a secret fails clearly when `master_key` is empty.
- **`egret-server`:** `AppState` gains `database: Option<DatabaseState>`; mount the admin database router under `/api/admin` (joining the existing CSRF-layered admin group) and the `/api/me/databases` listing.
- **`egret-bin`:** build `DatabaseState { repo: DatabaseRepo, secrets: SecretStore }` from config + DB.

## 9. Testing

- **`egret-secrets`:** unit — `seal`→`open` round-trips; a flipped ciphertext byte fails; a different key fails to `open`. DB-gated — `put`/`get`/`delete` round-trip.
- **`egret-database`:** DB-gated — register (seals a secret) / list (no secret, `has_secret` true) / update (rotates secret) / delete; grant + revoke; `/api/me/databases` returns only granted databases. HTTP tests via `oneshot` for the admin + me routers (auth-gated).
- **Connection tester (engine-gated):** self-skips unless `EGRET_TEST_TRINO_URL` / `EGRET_TEST_STARROCKS_URL` is set; when set (e.g. the dev `docker compose` Trino/StarRocks), registers that engine and asserts `test_connection` returns `ok = true`. Mirrors the catalog MinIO / Keycloak gates.
- **Config validate tests** (figment::Jail): master_key length check; register-with-secret without a key errors.

## 10. Decomposition for the plan

Suggested tasks: (1) scaffold `egret-secrets` + `egret-database` crates + workspace deps; (2) `egret-secrets::crypto` (AES-GCM seal/open + unit tests); (3) `migrations/0008_database.sql` + `SecretStore` (DB-gated); (4) `egret-config` `[secrets]` + validate; (5) `egret-database` model + `DatabaseRepo` CRUD (seals creds) (DB-gated); (6) `grants` + grant-aware `/api/me/databases` listing (DB-gated); (7) `tester` — Trino REST `SELECT 1` (engine-gated); (8) `tester` — StarRocks `mysql_async` `SELECT 1` (engine-gated); (9) admin + me routers + RBAC + error mapping; (10) `egret-server`/`egret-bin` wiring; (11) final verification.

## 11. References

- Architecture: [Master design](../architecture/2026-06-17-egret-architecture-design.md) §9.2 (database/secrets data model), §10.1 (`egret-database`), §10.2 (engine — consumes this in 3b), §8.2 (the two authz layers — RBAC gates registration).
- Trino statement protocol: `POST /v1/statement` + `nextUri` polling (reused fully by `egret-engine` in 3b).
- Existing patterns: `egret-catalog` (config + `AppState` Option sub-state + DB-gated/engine-gated tests), `egret-workspace::routes` (admin/me router + CSRF layer + RBAC).
