# Typed Secrets P1b — Storage + DB connection creds on typed basic-auth — Design

**Date:** 2026-07-19 · **Status:** Design — approved · **Branch:** `feat/typed-secrets-p1b-connection-creds` (off `main`)

> Vietnamese reference: `docs/vi/specs/2026-07-19-typed-secrets-p1b-connection-creds-design.md`.

## Problem

Storage and DB connections still carry credentials as a plaintext identity column plus a single-value secret (the P0 Opaque shim), not the unified typed model:

| | Non-secret identity (plaintext) | Secret half (single-value `secret_ref` → P0 Opaque shim) |
|---|---|---|
| **Storage** (`egret-controlplane/src/storage.rs`) | `access_key` column | `secret_key` |
| **DB** (query registry `Database`) | `options.user` (JSON) | password |

Reads use `resolve_plaintext` (one string). Nothing ties the full credential pair to the `kubernetes.io/basic-auth` typed shape P0 already supports, so these creds can't be resolved by key or projected to a k8s Secret uniformly.

## Goal — this sub-project (P1b)

Back both storage and DB connections with a `kubernetes.io/basic-auth` typed secret (`username` + `password` keys), read creds by key. **Hybrid model:** the basic-auth secret is the source-of-truth for the credential pair; the connection keeps its plaintext `access_key` / `user` field purely for display/listing (these are not secret). **Eager migration:** a one-shot idempotent Rust boot backfill converts every existing connection's secret to basic-auth now (no lazy phase). Fully backward-compatible reads during rollout (tolerate a still-Opaque secret).

## Out of scope

- Removing the plaintext `access_key`/`user` column (hybrid keeps it for display).
- Projecting these basic-auth secrets to real k8s Secrets for pod mounts (a later consumer; the primitive makes it possible).
- P3 remaining consumers (OIDC/notebook/cron), StarRocks FE ingress TLS.

## Context — grounded surface (verified)

- **`st.secrets` is a `WorkspaceSecretStore`.** Storage/DB mint a GLOBAL NAMED secret via `create(None, name, None, value)` (the Opaque shim); the connection's `secret_ref` → `workspace_secrets` row → vault `secrets` row. Reads use `resolve_plaintext(None, ref_id)`.
- **Typed API already exists:** `WorkspaceSecretStore::create_typed(ws, name, desc, type, data)` (mints a named typed secret), `resolve_typed(ws, ref_id) -> TypedSecret{r#type, data}`; `SecretStore::get_key(id, key)`, `put_typed`, `update_typed`; `secret_type::BASIC_AUTH = "kubernetes.io/basic-auth"` (keys `username` + `password`, validated).
- **Storage write** (`storage.rs` `resolve_secret_ref`): `new_secret` mints `st.secrets.create(None, ns.name, None, ns.value)`; the connection stores `access_key` + `secret_ref`. **Storage read** (`open_store`, `storage.rs:156`): `resolve_plaintext(None, secret_ref)` → `secret_key`.
- **DB write** (query registry `DatabaseInput`): `new_secret` (mint named) or `secret_id`; `options.user` carries the username. **DB read:** the registry resolves the named secret → the password → `ConnSpec.secret` (the connector's basic-auth password).
- **Migrations run at boot** (`db.migrate()` in the serve path, `egret-bin/src/main.rs:82`/`163`). There is no existing post-migration Rust backfill hook — one will be added right after `db.migrate()`.

## Architecture

### A. Resolver primitive (`egret-secrets`)

`WorkspaceSecretStore::resolve_basic_auth(ws: Option<Uuid>, ref_id) -> Result<(String /*username*/, String /*password*/)>`:
- `resolve_typed(ws, ref_id)`; if `type == kubernetes.io/basic-auth` → return `(data["username"], data["password"])`.
- **Fallback** (legacy / pre-backfill Opaque): return `(String::new(), resolve_plaintext(ws, ref_id))` — password-only, empty username. Keeps reads working before/if a secret hasn't been converted.

Reused by both storage and DB read paths.

### B. Write path

- **Storage** (`resolve_secret_ref`): when minting a new secret, use `create_typed(None, name, None, BASIC_AUTH, {"username": access_key, "password": value})` instead of the Opaque `create`. The connection still stores `access_key` in its column (hybrid display). The `access_key` for the `username` key comes from the same request body.
- **DB** (registry create/update): mint `create_typed(..., BASIC_AUTH, {"username": user, "password": value})` **when `options.user` is non-empty**; if there is no user, mint the Opaque secret as today (`kubernetes.io/basic-auth` validation requires a non-empty `username` — see the note below).
- **Empty-identity rule:** `secret_type::validate` requires `username` present + **non-empty** for basic-auth. Storage's `access_key` is `NOT NULL`, so storage always mints basic-auth. A DB connection without a `user` keeps an Opaque secret (read transparently via the `resolve_basic_auth` fallback). This avoids touching P0's validation contract; the rare user-less DB connection is the only case that stays Opaque.
- **Wire back-compat:** the existing request shape is unchanged — `new_secret.value` maps to `password`, and `access_key` / `options.user` map to `username`. Referencing an existing `secret_id`/`secret_ref` still works (resolved via `resolve_basic_auth`, which tolerates Opaque).

### C. Read path

- **Storage `open_store`:** `let (_user, secret_key) = st.secrets.resolve_basic_auth(None, ref_id).await?;` (username shown from the connection column). Everything downstream (`StorageConfig.secret_key`) is unchanged.
- **DB (password resolution → `ConnSpec.secret`):** resolve via `resolve_basic_auth` and pass the password as `secret`. Username continues to come from `options.user` for the connector.
- Both keep the Opaque fallback for safety during rollout.

### D. Eager migration — Rust boot backfill (no SQL migration)

`backfill_basic_auth_credentials(db, secrets)` runs once, right after `db.migrate()` in the serve path. Idempotent:
- Enumerate storage connections and DB connections that have a `secret_ref`.
- For each, resolve the referenced vault secret's `type`; if it is already `kubernetes.io/basic-auth`, skip.
- Otherwise, if the connection's identity (`access_key` for storage, `options.user` for DB) is **non-empty**, read the current single value (`resolve_plaintext`) and rewrite it via `update_typed(BASIC_AUTH, {"username": <identity>, "password": <current value>})`, per-secret transactional (as `update_typed` already is). A connection with an **empty identity** (rare user-less DB connection) is left Opaque — `resolve_basic_auth` reads it transparently.
- Only touches Opaque secrets referenced by a storage/DB connection (never `kubernetes.io/tls` or unrelated secrets). Logs the converted count (and skipped-empty count).
- **Shared secret guard:** if the same `secret_ref` is referenced by multiple connections with different identities, first-wins and a warning is logged (each connection normally mints its own secret, so this is rare).

Idempotency needs no marker table — a re-run finds everything already `basic-auth` and skips.

### E. UI

Effectively unchanged: the storage and DB forms already collect the identity (`access_key` / `user`) and the secret value; the backend maps `value → password` and identity `→ username`. Optionally surface a small "basic-auth" indicator where the connection's secret type is shown — low priority, not required for the slice.

## Testing / acceptance

- **Unit (`egret-secrets`):** `resolve_basic_auth` returns `(username, password)` for a basic-auth secret and `("", value)` for an Opaque one.
- **DB-gated (`egret-controlplane` + registry):** creating a storage/DB connection mints a `kubernetes.io/basic-auth` secret with keys `username` + `password`; the read path resolves the password correctly; the backfill converts a seeded legacy Opaque connection secret to basic-auth (preserving the password) and is a no-op on a second run.
- **Live (opt-in, kind):** create a storage connection → verify its secret is `kubernetes.io/basic-auth` (keys `username`,`password`) and `open_store` works (provision a warehouse through it); create a DB connection → a query runs; seed a legacy Opaque-secret connection, boot the backfill, verify it converts and still works.
- Gates: `cargo fmt/clippy/test`, frontend `lint`/`typecheck`/`test`/`build` green.

## Risks

1. **Backfill atomicity/idempotency** — reading the old value then writing both keys must be atomic and re-seal the pair (`update_typed` re-validates + reseals); per-secret; skip when already basic-auth. A partial failure must not lose the password.
2. **Shared secret** — a `secret_ref` shared across connections with differing identities → username ambiguity; first-wins + warn (rare, since connections mint their own secrets).
3. **Rollout back-compat** — reads must tolerate both Opaque (pre-backfill) and basic-auth; the `resolve_basic_auth` Opaque fallback covers this.
4. **Two username sources** — storage `access_key` (column) vs DB `options.user` (JSON); the write + backfill handle each connection type's source explicitly.
5. **Empty username** — `secret_type::validate` requires a non-empty `username` for basic-auth. Resolved (no validation change): connections with an empty identity keep an Opaque secret and are read via the `resolve_basic_auth` fallback; only identity-bearing connections become basic-auth. Storage always has `access_key` (NOT NULL) so it always converts; the only Opaque holdout is a user-less DB connection.

## Next

Projecting these basic-auth secrets to real k8s Secrets (pod-mounted DB creds / engine S3 creds) and the remaining P3 consumers (OIDC/notebook/cron), StarRocks FE ingress TLS — each its own spec → plan.
