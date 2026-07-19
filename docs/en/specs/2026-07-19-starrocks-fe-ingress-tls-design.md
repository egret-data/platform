# StarRocks FE ingress TLS from a typed secret — Design

**Date:** 2026-07-19 · **Status:** Design — approved · **Branch:** `feat/starrocks-fe-ingress-tls` (off `main`)

> Vietnamese reference: `docs/vi/specs/2026-07-19-starrocks-fe-ingress-tls-design.md`.

## Problem

P3 (`[[egret-typed-secrets]]`) gave a **Trino** engine's coordinator Ingress the ability to reference a typed `kubernetes.io/tls` secret, which egret projects into a real k8s `type: kubernetes.io/tls` Secret. StarRocks has **no ingress at all** in the orchestrator, so a StarRocks FE endpoint cannot be exposed externally, let alone with a typed cert. This slice brings StarRocks to parity using the same P3 projection primitive.

## Goal — this slice

Let a StarRocks engine's **FE HTTP endpoint (`:8030`, the web console / REST API)** be exposed through an Ingress whose TLS is backed by a typed `kubernetes.io/tls` secret, projected into `{name}-ingress-tls` at provision — the same mechanism P3 built for Trino. Backward-compatible (no ingress unless configured); reuses the shared `Ingress` config struct (already carries `tls_cert_secret_ref` from P3) and the projection helper.

## Out of scope — important

- **The StarRocks query path (`:9030`, MySQL protocol) is raw TCP and cannot be fronted by an HTTP/nginx Ingress.** This slice exposes only the FE HTTP endpoint (`:8030`). TLS for the query path is already handled in-band on `:9030` by the delegation work (`[[egret-starrocks-delegation]]`) and is unchanged.
- Removing/renaming any existing StarRocks Service.

## Context — grounded surface (verified)

- **StarRocks has no ingress render** (`egret-orchestrator/src/starrocks.rs` — no `Ingress`/`networking.k8s.io` today). Trino's is `trino_ingress` / `trino_ingress_tls_secret` (`trino.rs`).
- **Shared `Ingress` config** (`egret-engine-mgmt-api/src/lib.rs:254`): `{ enabled, class_name, host, tls_secret, tls_cert_secret_ref: Option<Uuid>, tls_cert_pem, tls_key_pem, annotations }`. `EngineSpec.config.ingress: Option<Ingress>` is shared across kinds.
- **StarRocks FE Service** is `{name}-fe-service` (`starrocks.rs:340`); FE HTTP is `http_port = 8030`, query is `query_port = 9030` (`starrocks.rs:100`/`102`).
- **StarRocks orchestrator**: `apply_all` (`starrocks.rs:714`), `update` (`793`), `delete(namespace, name)` (`893`).
- **`build_starrocks_provision_req`** (`egret-engine-mgmt/src/engines.rs:2109`) builds the StarRocks `EngineSpec`; `ensure_tls_cert` is called at `2235`. The P3 ingress-resolution block was added to the **Trino** builder (`build_provision_req`) only — StarRocks was not wired.
- **P3 create/update validation** of the ingress `tls_cert_secret_ref` (via `parse_ingress`) is kind-agnostic — expected to already cover StarRocks (to verify).
- **UI**: the `EngineSecurity.tsx` ingress picker is shared — expected to already reach StarRocks engines (to verify).

## Architecture (mirror P3)

### A. Shared projection helper (refactor)

Extract P3's `trino_ingress_tls_secret` body into a reusable free fn (in a shared orchestrator module, or duplicated minimally if no shared module fits):

```rust
pub fn ingress_tls_secret(name: &str, namespace: &str, ing: &Ingress) -> Option<Value> {
    if !ing.enabled { return None; }
    let (crt, key) = (ing.tls_cert_pem.as_ref()?, ing.tls_key_pem.as_ref()?);
    Some(json!({
        "apiVersion": "v1", "kind": "Secret", "type": "kubernetes.io/tls",
        "metadata": { "name": format!("{name}-ingress-tls"), "namespace": namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": name } },
        "stringData": { "tls.crt": crt, "tls.key": key }
    }))
}
```
`trino_ingress_tls_secret` becomes a thin caller of this (keeps Trino behavior identical).

### B. `starrocks_ingress` render (new)

Mirror `trino_ingress`, but target the FE HTTP Service/port:

```rust
pub fn starrocks_ingress(spec: &EngineSpec) -> Option<Value> {
    let ing = spec.config.ingress.as_ref()?;
    if !ing.enabled { return None; }
    // rules → backend Service `{name}-fe-service` port 8030 (FE HTTP);
    // ingressClassName from class_name; annotations; and when `tls_secret` is set:
    // spec.tls = [{ hosts:[host], secretName: tls_secret }].
    // (Same shape as trino_ingress with a different backend service+port.)
}
```

### C. StarRocks `apply_all` + GC

- In `apply_all` (`starrocks.rs:714`): apply `ingress_tls_secret(name, ns, ing)` **before** `starrocks_ingress(spec)` (both only render when enabled + PEMs present).
- In `update` (`793`): when ingress is disabled/absent, best-effort delete the stale `{name}-ingress-tls` Secret + the Ingress (mirror P3 Trino `update`).
- In `delete` (`893`): best-effort delete `{name}-ingress-tls` (the FE ingress) + the Ingress object.

### D. `build_starrocks_provision_req` — ingress resolution

Add the same block P3 added to the Trino builder, adapted to StarRocks' local `config` binding (guard on `config.ingress.as_mut()`; resolve `tls_cert_secret_ref` via `resolve_tls_cert_secret`; inject `tls_cert_pem`/`tls_key_pem`; set `tls_secret = "{name}-ingress-tls"`).

### E. Validation + UI (verify, likely no code)

- Confirm create/update already validate the ingress `tls_cert_secret_ref` for StarRocks (kind-agnostic `parse_ingress` path from P3). Add the check to the StarRocks create/update path only if a gap is found.
- Confirm the `EngineSecurity.tsx` ingress picker is reachable for StarRocks engines. Add wiring only if a gap is found.

## Testing / acceptance

- **Unit (orchestrator):** `starrocks_ingress` renders an Ingress whose backend is `{name}-fe-service`:8030 with `spec.tls[].secretName` when `tls_secret` set, `None` when disabled; the shared `ingress_tls_secret` renders a `type: kubernetes.io/tls` `{name}-ingress-tls` Secret; Trino tests unchanged after the refactor.
- **Live (opt-in, kind):** create a `kubernetes.io/tls` secret (SAN = the FE ingress host), provision a StarRocks engine with ingress enabled + host + typed cert ref → verify `{name}-ingress-tls` k8s Secret carries the provided cert, the Ingress has `spec.tls[].secretName`, and `curl https://<host>/api/bootstrap` (or an FE HTTP path) through nginx serves the provided cert → GC on engine delete.
- Gates: `cargo fmt/clippy/test`, frontend `lint`/`typecheck`/`test`/`build` green.

## Risks

1. **SAN = ingress host** (same as P3); nginx-lenient leaf (CA:FALSE not required).
2. **FE Service name/port** — confirm the exact FE HTTP Service (`{name}-fe-service`) + port (8030) the render targets; a wrong backend yields a 502 through nginx.
3. **Shared-helper refactor** — extracting `ingress_tls_secret` must not change Trino's rendered Secret (byte-identical); the Trino unit test guards this.
4. **StarRocks provision builder parity** — the ingress-resolution block must land in `build_starrocks_provision_req` (the SR path), not only the Trino builder.

## Next

egret's own-ingress Helm TLS, global company-cert wiring, remaining P3 consumers (OIDC/notebook/cron), and the `dockerconfigjson`/`ssh-auth` types — each its own spec → plan.
