# egret own-ingress Helm TLS — Design

**Date:** 2026-07-19 · **Status:** Design — approved · **Branch:** `feat/egret-own-ingress-tls` (off `main`)

> Vietnamese reference: `docs/vi/specs/2026-07-19-egret-own-ingress-tls-design.md`.

## Problem

egret's own Ingress (`egret.localtest.me`, `deploy/helm/egret/templates/ingress.yaml`) is HTTP-only — there is no `tls:` block. P2/P3/StarRocks brought typed-TLS to engine coordinator/ingress surfaces, but egret's own public endpoint still can't be served over HTTPS. Per the "cert everywhere" scoping decision, the global-cert capstone collapses into this concrete deploy-time change (no platform-default mechanism).

## Goal — this slice

Let egret's own Ingress be served over HTTPS with a cert, configured through the Helm chart. Two modes: **bring-your-own** existing k8s TLS Secret (e.g. cert-manager) via `ingress.tls.secretName`, or **inline** cert/key that the chart materializes into a `egret-ingress-tls` Secret. Fully backward-compatible: `ingress.tls.enabled=false` (default) leaves the current HTTP-only Ingress unchanged.

## Out of scope

- A platform-default / runtime cert-selection mechanism (explicitly dropped in the scoping decision — engine ingresses already reference their own cert via P3/SR).
- cert-manager `Certificate`/`Issuer` resources (the chart references a Secret; provisioning it via cert-manager is the operator's choice through `secretName`).

## Context — grounded surface (verified)

- **`deploy/helm/egret/templates/ingress.yaml`**: `{{- if .Values.ingress.enabled }}` … `spec: { ingressClassName, rules: [{ host: .Values.ingress.host, http: { paths: [/auth→keycloak:8080, /→egret-frontend:80] } }] }`. No `tls:`.
- **`deploy/helm/egret/values.yaml`**: `ingress: { enabled: false, host: egret.localtest.me, className: nginx }`.

## Architecture

### A. `values.yaml` — extend `ingress`

```yaml
ingress:
  enabled: false
  host: egret.localtest.me
  className: nginx
  tls:
    enabled: false
    # Bring-your-own existing k8s TLS Secret (e.g. cert-manager). Wins when set.
    secretName: ""
    # OR inline PEM cert+key — the chart creates `egret-ingress-tls` from them.
    cert: ""
    key: ""
```

### B. `templates/ingress.yaml` — add the `tls:` block

Inside the existing `spec:` (still gated by `ingress.enabled`), append when `ingress.tls.enabled`:

```yaml
{{- if .Values.ingress.tls.enabled }}
  tls:
    - hosts:
        - {{ .Values.ingress.host }}
      secretName: {{ .Values.ingress.tls.secretName | default "egret-ingress-tls" }}
{{- end }}
```

### C. `templates/ingress-tls-secret.yaml` — new (inline mode only)

Create the `kubernetes.io/tls` Secret only when inline cert/key are supplied and no external `secretName` is referenced:

```yaml
{{- if and .Values.ingress.enabled .Values.ingress.tls.enabled (not .Values.ingress.tls.secretName) .Values.ingress.tls.cert .Values.ingress.tls.key }}
apiVersion: v1
kind: Secret
type: kubernetes.io/tls
metadata:
  name: egret-ingress-tls
  namespace: {{ .Release.Namespace }}
data:
  tls.crt: {{ .Values.ingress.tls.cert | b64enc }}
  tls.key: {{ .Values.ingress.tls.key | b64enc }}
{{- end }}
```

Precedence: an explicit `secretName` always wins (no chart-managed Secret is created); inline cert/key are used only when `secretName` is empty.

## Testing / acceptance

- **`helm template`** (rendered-manifest assertions):
  - `ingress.tls.enabled=false` (default) → the Ingress has no `tls:` and no `egret-ingress-tls` Secret (backward-compatible).
  - inline mode (`tls.enabled=true`, `tls.cert`+`tls.key` set, `secretName` empty) → the Ingress `spec.tls[0].secretName == egret-ingress-tls` and an `egret-ingress-tls` `kubernetes.io/tls` Secret is rendered with the b64 cert/key.
  - byo mode (`tls.enabled=true`, `secretName: my-tls`) → the Ingress references `my-tls` and NO chart Secret is rendered.
- **Live (opt-in, kind):** `helm upgrade` the egret release with `ingress.tls.enabled=true` + an inline cert (SAN=`egret.localtest.me`) → `curl https://egret.localtest.me/api/auth/config` through nginx serves the provided cert (fingerprint match) and returns 200.

## Risks

1. **Cert SAN** must match `ingress.host` (`egret.localtest.me`), else browsers/clients reject it (nginx still serves it).
2. **Inline secrets in values** — `tls.cert`/`tls.key` in values are sensitive; document that production should prefer `secretName` (cert-manager) and keep inline for dev. Never commit real cert material to the repo's values.
3. **Existing deploys** — default `tls.enabled=false` keeps every current deploy HTTP-only; no migration.

## Next

Remaining P3 consumers (OIDC/notebook/cron) and the `dockerconfigjson`/`ssh-auth` typed-secret types — each its own spec → plan.
