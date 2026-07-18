# Typed Secrets P2 — Projection primitive + engine coordinator TLS — Design

**Date:** 2026-07-18 · **Status:** Design — approved · **Branch:** `feat/typed-secrets-p2-engine-tls` (off `main`)

> Vietnamese reference: `docs/vi/specs/2026-07-18-typed-secrets-p2-engine-tls-design.md`.

## Problem

P0/P1a ([[egret-typed-secrets]], PRs #277/#278) let an admin create a typed `kubernetes.io/tls` secret (cert+key). But nothing can USE it: a jwt-mode Trino engine's coordinator TLS is always an **egret-generated per-engine self-signed cert** (`ensure_tls_cert` → `generate_self_signed`). There is no way to back the coordinator's HTTPS listener with a provided cert — e.g. a single company-wide cert reused across engines ("cert everywhere").

## Goal — this sub-project (P2)

Let an engine optionally reference a P1a `kubernetes.io/tls` secret for its coordinator TLS: **if a cert is provided, use it; otherwise self-generate** (unchanged). This introduces the reusable **projection primitive** (resolve a typed-tls secret → cert+key PEM) plus its first consumer (Trino coordinator TLS). One company-cert secret can be referenced by many engines. Fully backward-compatible: existing engines (no reference) keep self-signed.

## Out of scope

- StarRocks FE-TLS-from-a-provided-secret (a variant — same primitive, later slice).
- Ingress TLS (public-facing egret.localtest.me HTTPS with a company cert) — the primitive is designed to be reused for it, but wiring the chart Ingress `tls:` is a later slice.
- P1b (storage/DB → typed basic-auth).

## Context — grounded surface (verified)

- **`ensure_tls_cert(st, engine) -> (keystore_pem, cert_pem)`** (`egret-engine-mgmt/src/engines.rs:1544`): if `engine.tls_secret_id` + `coordinator_cert` are set, resolve + reuse; else `crate::tls_cert::generate_self_signed(&[engine.coordinator_host()])` → `(cert_pem, key_pem)`, `keystore = cert_pem + key_pem`, store the keystore in a GLOBAL egret secret `engine-tls-{id}` (via `secrets.create`), and record `tls_secret_id` + `coordinator_cert`.
- The keystore feeds `EngineSpec.config.tls: Option<Tls { keystore_pem, port }>` (`egret-engine-mgmt-api/src/lib.rs`), rendered by `trino_tls_secret` (`egret-orchestrator/src/trino.rs:430`) into the `{name}-tls` k8s Secret (`stringData.keystore.pem`), mounted at `/etc/trino/tls/keystore.pem`. `coordinator_cert` (the public cert) is what the query connector pins.
- Engine TLS is jwt-mode-only (`0086_engine_tls.sql`: `engines.tls_secret_id`, `coordinator_cert`). Latest migration is `0088` → new file `0089`.
- P1a: `SecretStore::get_typed(id) -> TypedSecret{r#type, data}`; `WorkspaceSecretStore` exposes named secrets (`resolve_plaintext` returns a single value; a TYPED resolver is needed). `secret_type::TLS = "kubernetes.io/tls"` (keys `tls.crt`, `tls.key`).
- `CreateEngineBody` (`egret-engine-mgmt/src/engines.rs`): the engine-create body (name, warehouseId, kind, resources, trinoConfig…).

## Architecture

### A. Schema — new migration `migrations/0089_engine_tls_cert_ref.sql`

```sql
-- Optional: back the jwt-mode coordinator TLS with a user-provided typed
-- `kubernetes.io/tls` secret (a shared/company cert) instead of the auto
-- self-signed cert. NULL keeps the current per-engine self-signed behavior.
ALTER TABLE engine_mgmt.engines ADD COLUMN tls_cert_secret_ref uuid;
```

### B. Projection primitive (`egret-secrets` + a resolver)

- `WorkspaceSecretStore::resolve_typed(workspace: Option<Uuid>, id) -> TypedSecret` — the typed analogue of `resolve_plaintext` (resolve `secret_ref`, `secrets.get_typed(ref)`).
- A helper (in `engine-mgmt`, reusable): `resolve_tls_cert_secret(secrets, secret_ref) -> (cert_pem, key_pem)` — `resolve_typed` the named secret, assert `type == "kubernetes.io/tls"` with non-empty `tls.crt` + `tls.key`, return `(tls.crt, tls.key)`. (This is the "project a typed-tls secret" primitive that a future ingress slice reuses.)

### C. `ensure_tls_cert` — provided-cert branch

At the top of `ensure_tls_cert`, before the self-signed path (but the existing `tls_secret_id`/`coordinator_cert` reuse still wins first for idempotency):

- If `engine.tls_cert_secret_ref` is `Some`: `resolve_tls_cert_secret` → `(cert_pem, key_pem)`; `keystore = format!("{cert_pem}{key_pem}")`; `coordinator_cert = cert_pem`. Materialize it exactly like the self-signed path does today (store the keystore in the `engine-tls-{id}` egret secret + set `tls_secret_id`/`coordinator_cert`), so everything downstream (`{name}-tls` Secret, coordinator mount, connector pin) is unchanged.
- Else: the current `generate_self_signed` path.

The `{name}-tls` k8s Secret + mount are untouched — they simply carry the provided cert's keystore. Rotation: updating the provided secret re-provisions the engine to pick up the new keystore (same as the config-checksum roll; details finalized in the plan).

### D. API — engine create carries the reference

- `CreateEngineBody` gains `tls_cert_secret_ref: Option<Uuid>`; `create_engine` persists it on the engine row. (Update follows if the engine-update body should allow changing it — decide in the plan; create-time is the minimum.)
- Validation: if set, `resolve_tls_cert_secret` must succeed (the secret exists + is a valid TLS secret) — reject at create with a 400 otherwise.

### E. UI — optional TLS-cert picker

- The Add-engine dialog (`ControlPlaneEngines.tsx`) gains an optional **"Coordinator TLS cert"** `Select` listing the P1a `kubernetes.io/tls` named secrets (fetched from `/secrets`, filtered `type === "kubernetes.io/tls"`), plus a "Self-signed (default)" option. On submit, include `tlsCertSecretRef` when a secret is chosen. Only meaningful for jwt/TLS-mode engines (a hint suffices; the backend ignores it for non-TLS).

## Testing / acceptance

- **Backend** (`egret-secrets` + `egret-engine-mgmt` DB-gated): `resolve_typed` returns a TLS secret's data; `resolve_tls_cert_secret` returns cert+key and rejects a non-TLS / missing-key secret; `ensure_tls_cert` uses the provided cert (keystore = provided cert+key, `coordinator_cert` = provided cert) when `tls_cert_secret_ref` is set, and self-signs when NULL.
- **Live (opt-in, reuses the data-plane e2e path):** create a `kubernetes.io/tls` secret (a test cert+key), provision a jwt-mode Trino engine referencing it, enable JWT → the coordinator serves HTTPS with the PROVIDED cert (the `{name}-tls` Secret carries it) and egret connects (pinning `coordinator_cert` = the provided `tls.crt`) → a query succeeds. A NULL-reference engine still self-signs.
- Gates: `cargo fmt/clippy/test`, frontend `lint`/`typecheck`/`test`/`build` green.

## Risks

1. **Data-plane TLS** — a wrong keystore/pin breaks the coordinator HTTPS + egret's connection. Verify with the live data-plane path (provision + JWT + query). Keep the self-signed fallback the default.
2. **Cert validity** — a provided cert must (a) have a SAN matching the coordinator host egret dials (`coordinator_host()` = `{name}-trino.{ns}.svc`), or TLS verification fails, and (b) be a **leaf cert** (`basicConstraints CA:FALSE`, `extendedKeyUsage serverAuth`). reqwest/rustls (webpki) rejects a `CA:TRUE` cert presented as the server leaf — the query then fails with a generic `connect: error sending request` (curl/openssl are lenient and mask this). The self-signed path never hits this because rcgen defaults to `NoCA`. Document that a company/shared cert needs the right SAN (or a wildcard) **and** must be a proper leaf cert; `coordinator_cert` pinning otherwise tolerates a self-signed cert. **Live-proven (2026-07-18):** a leaf company cert (`CA:FALSE` + `serverAuth` + SAN) referenced by a jwt-mode Trino engine → `{name}-tls` keystore = the provided cert (sha256-identical) → `SELECT 1, current_user` over the pinned-cert TLS dial returned `1, admin`.
3. **PEM ordering** — the self-signed keystore is `cert + key`; the provided keystore must be the same order (`tls.crt` then `tls.key`); `coordinator_cert` = `tls.crt` only.
4. **Migration re-embed** — new migration `0089` needs `metastore/src/lib.rs` touched ([[egret-sqlx-migration-staleness]]).

## Next

StarRocks FE-TLS-from-secret (same primitive) and Ingress TLS (chart `tls:` referencing a projected k8s TLS Secret) — each its own spec → plan. P1b (storage/DB → basic-auth) remains deferred.
