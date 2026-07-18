# Typed Secrets P3 — Trino ingress TLS from a typed secret — Design

**Date:** 2026-07-18 · **Status:** Design — approved · **Branch:** `feat/typed-secrets-p3-trino-ingress-tls` (off `main`)

> Vietnamese reference: `docs/vi/specs/2026-07-18-typed-secrets-p3-trino-ingress-tls-design.md`.

## Problem

A provisioned Trino engine can already be exposed externally through an Ingress (`trino_ingress`, `egret-orchestrator/src/trino.rs:982`). Its TLS is wired via `ingress.tlsSecret` — a **raw string naming a pre-existing k8s `Secret`** the admin must create by hand in the engine namespace. Nothing connects that to egret's typed-secret store: an admin cannot pick a `kubernetes.io/tls` company/host cert from the Secrets console and have the ingress use it. This is the second "cert everywhere" consumer (after P2's coordinator TLS), and the first to build the **"project a typed-tls secret → a real k8s `kubernetes.io/tls` Secret"** primitive that P2 earmarked (P2 built a Trino `keystore.pem`, a different shape).

## Goal — this sub-project (P3-ingress)

Let a Trino engine's coordinator Ingress reference a typed `kubernetes.io/tls` secret (chosen in the UI) for its TLS. egret **projects** that typed secret into a real k8s `type: kubernetes.io/tls` Secret (`{name}-ingress-tls`) in the engine namespace at provision time; the rendered Ingress references it via `spec.tls[].secretName`. One company/host cert secret can back many engines' ingresses. Backward-compatible: the existing raw `tlsSecret` string (a pre-existing k8s Secret name) still works.

## Out of scope

- egret's **own** ingress (`egret.localtest.me`) TLS — configured via the Helm chart (deploy-time), handled separately by the operator, not egret runtime.
- StarRocks FE ingress TLS (same primitive, later slice).
- A standing projection reconciler / cert rotation without re-provision (projection happens at apply, like P2's `ensure_tls_cert`).
- P1b (storage/DB → typed basic-auth).

## Context — grounded surface (verified)

- **`trino_ingress(spec) -> Option<Value>`** (`egret-orchestrator/src/trino.rs:982`): renders a `networking.k8s.io/v1` Ingress for the `{name}-trino` coordinator Service (host, path `/`, backend port 8080), `ingressClassName` from `class_name`, and — when `ing.tls_secret` is `Some` — `spec.tls = [{ hosts:[host], secretName: ts }]`. **TLS terminates at the ingress; Trino stays HTTP internally.**
- **`Ingress` render struct** (`egret-engine-mgmt-api/src/lib.rs:253`): `{ enabled, class_name: Option<String>, host: String, tls_secret: Option<String>, annotations: BTreeMap }`.
- **`parse_ingress`** (`egret-engine-mgmt/src/engines.rs:1180`): builds `IngressReq` from the config wire object `ingress { enabled, className, host, tlsSecret, annotations }`.
- **Engine `config` is a JSON blob** (`serde_json::Value`, `store.rs:40`) — ingress lives inside it. **No migration needed** to add a field.
- **P2 primitives reused:** `resolve_tls_cert_secret(st, secret_ref) -> (cert_pem, key_pem)` (asserts `type == kubernetes.io/tls`, non-empty `tls.crt`/`tls.key`); the Add-engine coordinator-cert picker pattern (`ControlPlaneEngines.tsx`).
- **UI:** `EngineSecurity.tsx` already exposes `ingressEnabled/Host/ClassName/TlsSecret/Annotations`; `ingressTlsSecret` is a free-text input today.
- **Apply:** `apply_all` (`trino.rs:1152`) applies the Ingress only when enabled; other k8s objects (e.g. the `{name}-tls` keystore Secret) are applied alongside.

## Architecture

### A. Projection primitive (new — the earmarked one)

In `egret-orchestrator` (Trino module or a shared helper): from `(cert_pem, key_pem)`, render a k8s Secret

```json
{ "apiVersion": "v1", "kind": "Secret", "type": "kubernetes.io/tls",
  "metadata": { "name": "<engine>-ingress-tls", "namespace": "<ns>",
                "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": "<engine>" } },
  "stringData": { "tls.crt": "<cert_pem>", "tls.key": "<key_pem>" } }
```

This is the reusable "project a typed-tls secret → real k8s `kubernetes.io/tls` Secret" primitive (StarRocks FE ingress / other consumers reuse it). Cert+key come from `resolve_tls_cert_secret` (P2).

### B. Config surface (no migration)

- `Ingress` render struct + `IngressReq` gain `tls_cert_secret_ref: Option<Uuid>`; `parse_ingress` reads `ingress.tlsCertSecretRef`.
- **Apply-time resolution** (in the engine apply/provision path, before rendering the Ingress): if `ingress.tls_cert_secret_ref` is `Some` → `resolve_tls_cert_secret` → project the `{name}-ingress-tls` Secret → set the render `Ingress.tls_secret = "{name}-ingress-tls"`. If only the raw `tls_secret` string is present → pass it through unchanged (back-compat). Precedence: `tls_cert_secret_ref` (typed) wins over a raw `tls_secret` when both are set.
- **Validation** at engine create/update: if `tls_cert_secret_ref` is set, `resolve_tls_cert_secret` must succeed (secret exists + valid TLS secret) — reject with 400 otherwise (same as P2).

### C. Orchestrator apply + GC

- Apply the projected `{name}-ingress-tls` Secret **before** the Ingress in `apply_all` (so nginx sees the referenced Secret when it reconciles). On update, re-project (rotation = update the typed secret, then re-provision — same as the P2 config-checksum roll).
- **GC (decided):** the projected Secret is labeled `app.kubernetes.io/managed-by: egret` + `egret/engine: {name}` and **best-effort deleted on engine delete** (fixes the observed lingering `{name}-tls` keystore Secret debt; apply the same delete to the ingress-tls Secret, and — if cheap — the keystore Secret too).

### D. UI (`EngineSecurity.tsx`)

- Replace the `ingressTlsSecret` free-text input with a **typed-TLS-secret picker** (a `Select` listing `type === "kubernetes.io/tls"` secrets from `/secrets`, plus a "None" default), mirroring the P2 coordinator-cert picker (Radix sentinel for the empty option). On submit, include `ingress.tlsCertSecretRef` when a secret is chosen. The raw `tlsSecret` string stays supported at the API level (back-compat) but the UI leads with the picker.

## Data flow

1. Admin creates a `kubernetes.io/tls` company/host cert secret (P1a console). Its SAN must match the ingress host.
2. Engine → Security → Ingress: enable, set host (e.g. `trino-p3.localtest.me`), pick the TLS secret.
3. On provision/update: egret resolves the typed secret → projects `{name}-ingress-tls` (tls.crt/tls.key) → renders the coordinator Ingress with `spec.tls[].secretName = {name}-ingress-tls`.
4. nginx-ingress terminates TLS with the company cert for the engine host; Trino stays HTTP internally.

## Testing / acceptance

- **Unit (orchestrator):** the projection renders a valid `type: kubernetes.io/tls` Secret with keys `tls.crt`/`tls.key`, name `{name}-ingress-tls`, managed-by labels; `trino_ingress` sets `secretName` to the projected name when `tls_cert_secret_ref` is set and falls back to the raw `tls_secret` otherwise; typed-ref precedence over raw when both set.
- **DB-gated (engine-mgmt):** create/update validates the ref (400 on non-TLS / missing key); `parse_ingress` round-trips `tlsCertSecretRef`.
- **Live (opt-in, kind):** create a `kubernetes.io/tls` secret (SAN = the ingress host), provision a Trino engine with ingress enabled + host + the typed ref → verify the `{name}-ingress-tls` k8s Secret carries the provided cert, the Ingress has `spec.tls[].secretName`, and `curl https://<host>/v1/info` (through nginx-ingress) serves the provided cert. A raw-`tlsSecret` engine still works.
- Gates: `cargo fmt/clippy/test`, frontend `lint`/`typecheck`/`test`/`build` green.

## Risks

1. **Cert SAN = ingress HOST** — the provided cert's SAN must match the **ingress host** (e.g. `trino-p3.localtest.me`), NOT the coordinator svc host (`{name}-trino.{ns}.svc`) as in P2. Different SAN requirement; document it. (nginx terminates + serves the cert to external clients, who verify against the host.)
2. **CA:FALSE not required here** — nginx (openssl) is lenient about the leaf's `basicConstraints`, unlike reqwest/rustls in P2. A proper leaf cert is still recommended, but the P2 `CA:TRUE`-leaf failure does not apply to the ingress-termination path.
3. **GC** — projected Secret cleanup is best-effort on engine delete; a failed delete leaves a harmless orphan Secret (labeled managed-by egret for manual cleanup).
4. **Apply ordering** — the projected Secret must be applied before the Ingress so nginx resolves `secretName` on first reconcile.
5. **Rotation** — updating the typed secret requires re-provisioning the engine to re-project the k8s Secret (no standing reconciler in scope).

## Next

StarRocks FE ingress TLS (same primitive) and the remaining P3 consumers (OIDC/notebook/cron) — each its own spec → plan. egret's own-ingress Helm TLS and P1b (storage/DB → basic-auth) remain out of this slice.
