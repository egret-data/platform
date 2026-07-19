# egret own-ingress Helm TLS — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax. (This is a Helm-only slice — the controller may implement inline given its size.)

**Goal:** Serve `egret.localtest.me` over HTTPS via the Helm chart — bring-your-own `secretName` or inline cert/key (chart-materialized `egret-ingress-tls`). Default off, backward-compatible.

**Architecture:** Extend `values.yaml` `ingress` with a `tls` sub-block; add a `tls:` block to the Ingress template gated on `ingress.tls.enabled`; add a new template that renders a `kubernetes.io/tls` Secret only in inline mode. Verified with `helm template` assertions + a live `helm upgrade` on kind.

**Tech Stack:** Helm (Go templates), `deploy/helm/egret/`.

---

## Design references

- Spec: `docs/en/specs/2026-07-19-egret-own-ingress-tls-design.md`
- `deploy/helm/egret/templates/ingress.yaml` (HTTP-only, gated `{{- if .Values.ingress.enabled }}`), `deploy/helm/egret/values.yaml` (`ingress: {enabled, host, className}`).

---

### Task 1: values + Ingress `tls:` block + inline Secret template

**Files:**
- Modify: `deploy/helm/egret/values.yaml`
- Modify: `deploy/helm/egret/templates/ingress.yaml`
- Create: `deploy/helm/egret/templates/ingress-tls-secret.yaml`

- [ ] **Step 1: values.yaml** — extend the `ingress` block:

```yaml
ingress:
  enabled: false
  host: egret.localtest.me
  className: nginx
  tls:
    enabled: false
    secretName: ""   # bring-your-own existing k8s TLS Secret (cert-manager); wins if set
    cert: ""         # OR inline PEM cert — chart creates egret-ingress-tls
    key: ""          # inline PEM key
```

- [ ] **Step 2: templates/ingress.yaml** — add the `tls:` block inside `spec:` (after `rules:`), gated on `ingress.tls.enabled`:

```yaml
{{- if .Values.ingress.tls.enabled }}
  tls:
    - hosts:
        - {{ .Values.ingress.host }}
      secretName: {{ .Values.ingress.tls.secretName | default "egret-ingress-tls" }}
{{- end }}
```

- [ ] **Step 3: templates/ingress-tls-secret.yaml** — new file:

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

- [ ] **Step 4: `helm template` assertions** (run from repo root):

```bash
CH=deploy/helm/egret
# default: no tls, no secret
helm template t $CH --set ingress.enabled=true | grep -q "kind: Secret" && echo "UNEXPECTED secret" || echo "ok: no secret by default"
helm template t $CH --set ingress.enabled=true | grep -A3 "kind: Ingress" | grep -q "tls:" && echo "UNEXPECTED tls" || echo "ok: no tls by default"
# inline mode: tls block + egret-ingress-tls secret
helm template t $CH --set ingress.enabled=true --set ingress.tls.enabled=true --set ingress.tls.cert=CERTPEM --set ingress.tls.key=KEYPEM | grep -q "name: egret-ingress-tls" && echo "ok: inline secret rendered"
helm template t $CH --set ingress.enabled=true --set ingress.tls.enabled=true --set ingress.tls.cert=CERTPEM --set ingress.tls.key=KEYPEM | grep -q "secretName: egret-ingress-tls" && echo "ok: ingress references chart secret"
# byo mode: references my-tls, no chart secret
helm template t $CH --set ingress.enabled=true --set ingress.tls.enabled=true --set ingress.tls.secretName=my-tls | grep -q "secretName: my-tls" && echo "ok: byo secretName referenced"
```
Expected: all `ok:` lines; no `UNEXPECTED`.

- [ ] **Step 5: Commit** — `feat(deploy): egret own-ingress Helm TLS (byo secretName or inline cert/key)`

---

### Task 2: Live acceptance on kind (controller-run)

**Files:** none (verification)

- [ ] **Step 1:** Generate a leaf cert with SAN=`egret.localtest.me` (CA:FALSE + serverAuth).
- [ ] **Step 2:** `helm upgrade` the running egret release with `--reuse-values --set ingress.tls.enabled=true --set-file ingress.tls.cert=<crt> --set-file ingress.tls.key=<key>` (or edit values + upgrade). Confirm the `egret-ingress-tls` Secret + the Ingress `tls` appear (`kubectl get secret egret-ingress-tls -n egret-system`, `kubectl get ingress egret -n egret-system -o jsonpath='{.spec.tls}'`).
- [ ] **Step 3:** `curl -k --resolve egret.localtest.me:443:127.0.0.1 https://egret.localtest.me/api/auth/config` → 200 and the served cert fingerprint == the provided cert.
- [ ] **Step 4:** Revert the release to HTTP-only (`--set ingress.tls.enabled=false`) or leave as-is per preference; note the state.
- [ ] **Step 5:** Use superpowers:finishing-a-development-branch → open PR → `gh pr merge --admin --squash`.

---

## Self-review notes

- **Spec coverage:** values tls block (Task 1 s1), Ingress tls (s2), inline Secret + precedence (s3), the three `helm template` modes (s4), live HTTPS proof (Task 2). All mapped.
- **Type consistency:** chart-managed Secret name `egret-ingress-tls` consistent in the Ingress `default` + the Secret template + the live checks. `secretName` precedence: the `| default` in the Ingress + the `(not .secretName)` guard in the Secret template together ensure byo wins and no duplicate Secret.
- **Backward-compat:** every new value defaults off/empty; `helm template` default-mode assertions guard that the current HTTP-only render is unchanged.
