# Typed Secrets P1a — Typed-secret console (API + UI) — Design

**Date:** 2026-07-18 · **Status:** Design — approved · **Branch:** `feat/typed-secrets-p1a` (off `main`)

> Vietnamese reference: `docs/vi/specs/2026-07-18-typed-secrets-p1a-design.md`.

## Problem

P0 ([[egret-typed-secrets]], PR #277) gave `egret-secrets` a k8s-style typed multi-field model (`type` + `data` map, `Opaque`/`kubernetes.io/tls`/`kubernetes.io/basic-auth`), but nothing above the crate can create or manage a typed secret: the `/secrets` HTTP API and the control-plane UI are still single-value (`{name, value}`), and `WorkspaceSecretStore` (the named layer) only creates single-value secrets. So an admin cannot store a **TLS cert (cert+key)** — the enabler for P2's "cert everywhere".

## Goal — this sub-project (P1a)

A **typed-secret console**: the `/secrets` API and control-plane UI can create / list / edit **typed** named secrets (`Opaque` key→value maps, `kubernetes.io/tls` cert+key, `kubernetes.io/basic-auth` user+pass), and `WorkspaceSecretStore` gains typed create/update. Plaintext values are never exposed (list/get shows the `type` + the data KEY names only). Existing single-value secrets keep working and display as `Opaque{value}`.

This is P1a; P1b (migrating storage/DB connections onto typed basic-auth) is a separate follow-on.

## Out of scope

- **P1b** — migrating storage / DB connections (and their credential resolution) onto typed basic-auth secrets. They keep working through the P0 single-value shims here.
- **P2** — projecting typed secrets to real k8s `Secret` objects (cert mount).
- k8s projection, engine wiring, new types (`dockerconfigjson`/`ssh-auth`).

## Context — grounded surface (verified)

- **Crate (P0):** `SecretStore::{put_typed(kind, type, data), get_typed(id)→TypedSecret{r#type,data}, get_key, update_typed}`; `secret_type::{OPAQUE, TLS, BASIC_AUTH, validate}`. Single-value `put`/`get`/`update` are shims over `Opaque{value}`.
- **Named layer:** `WorkspaceSecretStore::create(workspace?, name, description?, plaintext)` = `secrets.put("workspace_secret", plaintext)` + INSERT `workspace_secrets(workspace_id, name, description, secret_ref)`; `WorkspaceSecretMeta { id, name, description, created_at, used_by }` (never exposes the value). Global secrets = `workspace_id` NULL.
- **API** (`egret-controlplane/src/secrets.rs`): `GET/POST /secrets`, `PUT/DELETE /secrets/{id}`. `CreateSecretInput { name, description?, value }`, `UpdateSecretInput { name?, description?, value? }`, `SecretDto { id, name, description, created_at, used_by }`.
- **UI** (`frontend/src/routes/control-plane/ControlPlaneSecrets.tsx`): "Add secret" dialog with a single password input; a secrets table; imports `type Secret, type SecretInput` from the frontend api module.

## Architecture

### A. `egret-secrets` — typed named-secret layer

- `WorkspaceSecretStore::create_typed(workspace: Option<Uuid>, name, description?, r#type: &str, data: &BTreeMap<String,String>) -> Uuid` — `secrets.put_typed("workspace_secret", r#type, data)` (validates) + INSERT the `workspace_secrets` row (same duplicate-name rollback as `create`).
- `WorkspaceSecretStore::update_typed(id, r#type, data)` — resolve `secret_ref`, `secrets.update_typed(ref, ...)`.
- `WorkspaceSecretMeta` gains `r#type: String` + `keys: Vec<String>` — the list/get query LEFT JOINs `secrets` (`type`) and aggregates `secret_data.key` (names only). Existing `create`/`update` stay (single-value shims).

### B. HTTP API (`egret-controlplane/src/secrets.rs`)

- `CreateSecretInput { name, description?, r#type: Option<String>, data: Option<BTreeMap<String,String>>, value: Option<String> }`:
  - typed path: `r#type` + `data` (default `r#type = "Opaque"` when `data` present).
  - legacy alias: `value` present → `data = { "value": value }`, `r#type = "Opaque"`.
  - reject if neither `data` nor `value` present, or both; `secret_type::validate` enforces required keys.
- `UpdateSecretInput { name?, description?, r#type?, data?, value? }` — same one-of; typed rotation via `update_typed`; `value` alias rotates the `value` key.
- `SecretDto` gains `r#type: String` (serialized `type`) + `keys: Vec<String>`. `GET /secrets` + `/secrets/{id}` expose `type` + `keys` (never values).

### C. Control-plane UI (`ControlPlaneSecrets.tsx` + api module)

- **Add/edit dialog:** a **type Select** (`Opaque` / `kubernetes.io/tls` / `kubernetes.io/basic-auth`) drives the fields:
  - `Opaque` → a key→value rows editor (add/remove pairs; one row default).
  - `kubernetes.io/tls` → `tls.crt` + `tls.key` textareas (paste PEM).
  - `kubernetes.io/basic-auth` → `username` + `password` inputs.
  - Submit posts `{ name, description?, type, data }`.
- **List:** a `type` Badge column + the data key names (a small "keys: a, b" line); the value is never shown.
- **api module** (`frontend/src/api/secrets*` or generated): `Secret` gains `type` + `keys`; `SecretInput` gains `type` + `data`.

## Testing / acceptance

- **Backend** (`egret-secrets` DB-gated + `egret-controlplane` unit/DB): `create_typed` + list shows `type` + `keys` and NEVER the value; typed create/update round-trips Opaque/tls/basic-auth via the API; validation rejects a TLS missing `tls.key`; the legacy `value` alias still creates an `Opaque{value}`.
- **Frontend** (vitest): the dialog renders the right fields per type; `npm run lint`/`typecheck`/`test`/`build` green.
- **Live (opt-in):** create a TLS secret in the control-plane UI against the kind deploy; it appears in the list as `kubernetes.io/tls` with keys `tls.crt, tls.key`, no values shown.

## Risks

1. **API back-compat** — the `/secrets` API is consumed by the UI (updated here) and possibly others; keep the `value` alias so single-value callers still work.
2. **Never leak values** — the list/get DTO must expose only `type` + key names; the SQL selects `secret_data.key`, never `ciphertext`.
3. **`type` keyword** — Rust field `r#type`; JSON serialized as `type` (serde rename). Frontend uses `type`.
4. **Duplicate-name rollback** — `create_typed` must preserve `create`'s behavior (roll back the vault row on a duplicate workspace_secrets name).

## Next (P1b)

Migrate storage (access_key + secret_key) and DB (user + password) connections onto typed `kubernetes.io/basic-auth` secrets, reading credentials by key, with a data migration for existing connections. Its own spec → plan.
