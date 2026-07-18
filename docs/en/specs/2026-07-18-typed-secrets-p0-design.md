# Typed Secrets P0 — Core model + backward-compat — Design

**Date:** 2026-07-18 · **Status:** Design — approved · **Branch:** `feat/typed-secrets-p0` (off `main`)

> Vietnamese reference: `docs/vi/specs/2026-07-18-typed-secrets-p0-design.md`.

## Problem

egret's secret store is single-value: a `secrets` row holds ONE AES-256-GCM-sealed string plus a loose `kind` label (`db_password`, `workspace_secret`, …). Every credential — storage keys, DB passwords, catalog tokens, OIDC refresh — is one opaque string, referenced by `secret_ref` in ~113 places. There is no way to model **typed, multi-field** secrets like Kubernetes does (`Opaque` key→value maps, `kubernetes.io/tls` = cert+key, `kubernetes.io/basic-auth` = user+password), which blocks the goal of using a single **TLS secret** to back cert-consuming configs (a company-wide cert) and, eventually, unifying every credential under one typed model.

## Goal — this sub-project (P0)

The **foundation only**: extend the `egret-secrets` crate + schema to a **typed, multi-field** secret model (a `type` + a `data: key→sealed-value` map, one seal per key), with per-type key validation, while keeping **every existing consumer working unchanged** via backward-compat shims. No HTTP API, no UI, no k8s projection, no consumer migration — those are P1–P3.

This is P0 of a 4-phase program (see "Program context"). It is verifiable in isolation with the crate's DB-gated tests.

## Out of scope (later phases)

- **P1** — typed-secret HTTP API + control-plane UI (type selector, TLS/basic-auth editors) + migrating `WorkspaceSecretStore` / storage / DB connections onto the typed model.
- **P2** — projecting typed secrets to real k8s `Secret` objects (`kubernetes.io/tls`) that engine pods / ingress mount (the "cert everywhere" use case).
- **P3** — migrating the remaining consumers (OIDC, notebook, cron, catalog) + the company-wide-cert wiring; new types `dockerconfigjson`, `ssh-auth`.

## Program context (the full-unify vision)

Every credential eventually flows through one typed-Secret model (k8s-shaped: `type` + `data` map, projectable to k8s Secrets, usable everywhere). Order: **P0 core model** → P1 API/UI/consumers → P2 k8s projection → P3 remaining consumers + global cert. Each phase is its own spec → plan → implementation. P0 ships `Opaque` + `kubernetes.io/tls` + `kubernetes.io/basic-auth`; `dockerconfigjson` / `ssh-auth` are cheap to add later (one validation rule + one projection mapping — no schema change).

## Context — grounded surface (verified)

- **`secrets`** (migration `0008_database.sql`): `{ id uuid, kind text, nonce bytea, ciphertext bytea, key_id text default 'cfg', created_at }` — one sealed value.
- **`workspace_secrets`** (`0032`): `{ id, workspace_id, name, description, secret_ref → secrets(id), created_at, UNIQUE(workspace_id, name) }` — a named layer over the vault.
- Consumers reference `secrets(id)` directly (e.g. `databases.secret_id → secrets(id)`) — ~113 `secret_ref` usages; all fetch a single string.
- **`egret-secrets`** crate: `SecretStore` (`put(kind, plaintext) → Uuid`, `get(id) → String`, `update`, `delete`), `WorkspaceSecretStore` (named layer, `NewSecret { name, value }`), `Crypto` (`seal(plaintext) → (nonce, ciphertext)`, `open(nonce, ciphertext) → String`, AES-256-GCM). Crate has DB-gated tests (`crates/egret-secrets/tests/`).
- Migrations live in `./migrations`; a new migration file doesn't recompile `egret-metastore` until `metastore/src/lib.rs` is touched (re-embed) — see [[egret-sqlx-migration-staleness]].

## Architecture

Extend the vault to typed multi-field storage; keep the single-value API as a shim.

### A. Schema — new migration `migrations/00NN_typed_secrets.sql`

1. `ALTER TABLE secrets ADD COLUMN type text NOT NULL DEFAULT 'Opaque';`
2. `CREATE TABLE secret_data ( secret_id uuid NOT NULL REFERENCES secrets(id) ON DELETE CASCADE, key text NOT NULL, nonce bytea NOT NULL, ciphertext bytea NOT NULL, key_id text NOT NULL DEFAULT 'cfg', PRIMARY KEY (secret_id, key) );`
3. **Migrate existing values:** `INSERT INTO secret_data (secret_id, key, nonce, ciphertext, key_id) SELECT id, 'value', nonce, ciphertext, key_id FROM secrets;` (every legacy secret becomes `Opaque` with a single `value` key — the `DEFAULT 'Opaque'` already set `type`).
4. `ALTER TABLE secrets DROP COLUMN nonce, DROP COLUMN ciphertext;` (forward-only; `secret_data` is now the single source of truth for values). `kind` and `key_id`-on-secrets are unaffected; `kind` stays as the legacy category label alongside the new `type`.

### B. Crate — `egret-secrets` typed model

- **Model:** `TypedSecret { type: String, data: BTreeMap<String, String> }` (plaintext map, returned only on read). Each value sealed individually into a `secret_data` row (per-key `seal`).
- **`SecretStore` new methods:**
  - `put_typed(kind: &str, type_: &str, data: &BTreeMap<String,String>) -> Uuid` — validates the type's required keys, inserts the `secrets` row (`kind`, `type`), seals + inserts one `secret_data` row per entry (transactional).
  - `get_typed(id) -> TypedSecret` — reads `type` + all `secret_data` rows, `open`s each.
  - `get_key(id, key) -> String` — reads + opens one `secret_data` row.
  - `update_typed(id, data)` — re-validates + replaces the `secret_data` rows.
- **Backward-compat shims (unchanged signatures):**
  - `put(kind, plaintext)` = `put_typed(kind, "Opaque", { "value": plaintext })`.
  - `get(id)` = `get_key(id, "value")`.
  - `update(id, plaintext)` = replace the `value` key.
  - ⇒ every existing consumer (~113 `secret_ref`, `WorkspaceSecretStore`, storage/DB connections) works unchanged.
- **Type registry / validation** (`secret_type.rs`): a small table of `type → required keys`: `Opaque` (none), `kubernetes.io/tls` (`tls.crt`, `tls.key`), `kubernetes.io/basic-auth` (`username`, `password`). `put_typed`/`update_typed` reject data missing a required key (or, for the non-Opaque types, containing unexpected keys — decide during impl; lenient is fine). Unknown types → rejected (only the three registered types allowed in P0).

### C. Tests (crate, DB-gated)

Extend `crates/egret-secrets/tests/`:
- put_typed + get_typed round-trips an Opaque multi-key map, a TLS (`tls.crt`+`tls.key`), and a basic-auth secret.
- Validation rejects a TLS secret missing `tls.key`, and an unknown type.
- The **backward-compat shim**: `put`/`get` still round-trip a single value (stored as `Opaque{value}`); `get_typed` on a shim-written secret shows `type=Opaque, data={value}`.
- (Migration is exercised by the existing suite running against a migrated DB — the old `get(id)` path now reads `secret_data`.)

## Testing / acceptance

- `cargo test -p egret-secrets` (DB-gated) — new + existing tests pass (existing single-value tests prove the shim + migration).
- `cargo fmt --all --check` + `cargo clippy -p egret-secrets` clean. A full workspace `cargo build` compiles (the migration re-embeds after touching `metastore/src/lib.rs`).
- No behavior change for any current consumer — this phase is purely additive + a transparent storage migration.

## Risks

1. **Migration correctness** — every legacy `secrets` value must land in `secret_data(key='value')` before dropping the old columns; the migration does the copy in the same file before the DROP. Irreversible (forward-only), consistent with egret's migration style.
2. **sqlx staleness** — the new migration won't take effect in `serve`/tests until `metastore/src/lib.rs` is touched to re-embed; documented.
3. **Transactionality** — `put_typed`/`update_typed` must write the `secrets` row + all `secret_data` rows atomically (one transaction) so a partial secret never exists.
4. **key_id / rotation** — per-key `key_id` is carried through (rotation reserve), same as today; no rotation logic added in P0.

## Next (P1)

Typed-secret HTTP API + control-plane UI (type selector, TLS cert/key + basic-auth editors), and moving `WorkspaceSecretStore` + storage/DB connections onto `put_typed`/`get_typed`. Its own spec → plan.
