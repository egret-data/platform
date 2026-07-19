# Typed secrets — dockerconfigjson + ssh-auth types (+ dockerconfigjson → imagePullSecrets) — Design

**Date:** 2026-07-19 · **Status:** Design — approved · **Branch:** `feat/dockerconfig-ssh-secret-types` (off `main`)

> Vietnamese reference: `docs/vi/specs/2026-07-19-dockerconfig-ssh-secret-types-design.md`.

## Problem

The typed-secret registry ships `Opaque` + `kubernetes.io/tls` + `kubernetes.io/basic-auth`. Two standard k8s types are missing: `kubernetes.io/dockerconfigjson` (private image-registry pull credentials) and `kubernetes.io/ssh-auth` (SSH private key). The former has a real egret consumer — an engine pulling images from a private registry needs a pod `imagePullSecrets` — which currently only accepts a **raw pre-existing k8s Secret name** (`config.image_pull_secrets`), not a typed secret from egret's store.

## Goal — this slice

1. Add the two types to the registry (validation + all existing typed CRUD/console become available for free).
2. Console editors for both.
3. Wire `kubernetes.io/dockerconfigjson` to a **Trino** engine's `imagePullSecrets`: an engine can reference a typed dockerconfigjson secret; egret projects it into a k8s `type: kubernetes.io/dockerconfigjson` Secret `{name}-pull` at provision and adds it to the pod's `imagePullSecrets` — the same ref→resolve→project→wire pattern as P3.

## Out of scope

- **StarRocks** imagePullSecrets wiring — StarRocks' pull secrets live under `starrocksConfig.pod.image_pull_secrets` (a nested raw-JSON pod config), a different path than Trino's `config.image_pull_secrets`; a later follow-up.
- `ssh-auth` engine consumer — there is no egret consumer today; this slice adds the type + console only (future git/ssh use).

## Context — grounded surface (verified)

- **`secret_type.rs`** (`egret-secrets`): `required_keys` match — `OPAQUE`/`TLS`/`BASIC_AUTH`. `validate` requires each listed key present + non-empty. Adding a type = one arm (no schema change).
- **`config.image_pull_secrets: Vec<String>`** (`egret-engine-mgmt-api/src/lib.rs:349`): raw k8s Secret names. Trino renders them into `pod_spec["imagePullSecrets"]` (`trino.rs:852`). Parsed from the config wire `v["imagePullSecrets"]` (`engines.rs:1165`).
- **StarRocks** sets `imagePullSecrets` from `pod.image_pull_secrets` (`starrocks.rs:458`, under `starrocksConfig.pod`) — the out-of-scope path.
- **Typed secret CRUD + console** (`egret-controlplane/src/secrets.rs`, `ControlPlaneSecrets.tsx`) are generic over `type` + `keys` (P1a) — a new type needs only a console editor; the API `create_typed`/list/`SecretDto{type,keys}` already handle it.
- **P3 pattern to mirror** for the engine wiring: `tls_cert_secret_ref` on the ingress config → resolved at provision (`resolve_tls_cert_secret`) → projected `{name}-ingress-tls` Secret → referenced. `resolve_tls_cert_secret` is TLS-specific; a sibling resolver is needed for dockerconfigjson.

## Architecture

### A. Registry types (`secret_type.rs`)

```rust
pub const DOCKER_CONFIG_JSON: &str = "kubernetes.io/dockerconfigjson";
pub const SSH_AUTH: &str = "kubernetes.io/ssh-auth";
// required_keys:
DOCKER_CONFIG_JSON => Some(&[".dockerconfigjson"]),
SSH_AUTH => Some(&["ssh-privatekey"]),
```
`validate` already enforces present + non-empty. New `SecretsError` variants are NOT needed (reuses `InvalidType`/`MissingKey`).

### B. Console (`ControlPlaneSecrets.tsx`)

- Add both to the type `Select`.
- **dockerconfigjson editor**: registry / username / password / email inputs → build the `.dockerconfigjson` JSON client-side:
  `{"auths":{"<registry>":{"username","password","email","auth":<base64("user:pass")>}}}` → stored under key `.dockerconfigjson`.
- **ssh-auth editor**: a private-key `Textarea` → key `ssh-privatekey`.
- List keeps showing type + keys (never values).

### C. dockerconfigjson → Trino imagePullSecrets (mirror P3)

- **Resolver** (`egret-engine-mgmt`): `resolve_dockerconfigjson_secret(st, ref) -> String` (the `.dockerconfigjson` value) — `resolve_typed`, assert `type == kubernetes.io/dockerconfigjson`, return the key. (A sibling of `resolve_tls_cert_secret`.)
- **Config field**: `image_pull_secret_ref: Option<Uuid>` in the config (lives in the config JSON, no migration; parsed in `build_trino_config`/`parse`).
- **Provision** (`build_provision_req`): if set, `resolve_dockerconfigjson_secret` → set a runtime field carrying the projected `.dockerconfigjson` value + append `{name}-pull` to `config.image_pull_secrets`.
- **Orchestrator**: a `trino_pull_secret(spec)` render (a `type: kubernetes.io/dockerconfigjson` Secret `{name}-pull` from the resolved value) applied in `apply_all` before the Deployments; GC in `delete` (`{name}-pull`).
- **Validation**: create/update resolve the ref (400 on missing / non-dockerconfigjson).

## Testing / acceptance

- **Unit (`egret-secrets`):** `validate` accepts `dockerconfigjson`{`.dockerconfigjson`} + `ssh-auth`{`ssh-privatekey`}, rejects each when the key is missing/empty.
- **Unit (orchestrator):** `trino_pull_secret` renders a `type: kubernetes.io/dockerconfigjson` Secret `{name}-pull` with the `.dockerconfigjson` data; `trino_deployment` includes `{name}-pull` in `imagePullSecrets` when set.
- **DB-gated (engine-mgmt):** create validates a bad `image_pull_secret_ref` (400).
- **Live (kind):** create a dockerconfigjson secret (API) → provision a Trino engine referencing it → verify `{name}-pull` Secret (`type: kubernetes.io/dockerconfigjson`) + the coordinator/worker pod `imagePullSecrets` includes `{name}-pull` → GC on delete. Create an ssh-auth secret via the console → lists as `kubernetes.io/ssh-auth`, key `ssh-privatekey`.
- Gates: `cargo fmt/clippy/test`, frontend `lint`/`typecheck`/`test`/`build` green.

## Risks

1. **`.dockerconfigjson` well-formedness** — the console builds the JSON; a malformed value would make the kubelet fail image pulls. Keep the console the source of truth; the backend stores the value opaquely.
2. **imagePullSecrets is a list** — append `{name}-pull` (don't replace) so any raw `image_pull_secrets` already configured survive; de-dup if already present.
3. **No migration / no OpenAPI regen** — config field is opaque JSON; console reuses the existing `/secrets` API.

## Next

StarRocks `pod.image_pull_secrets` dockerconfigjson wiring; an `ssh-auth` consumer (git-sourced notebooks/pipelines) — each its own slice. OIDC/notebook/cron → typed is the remaining typed-secrets item.
