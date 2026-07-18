# Deploy egret to kind — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One command (`just deploy-kind`) builds egret's images, loads them into the existing kind cluster, and `helm install`s egret (backend + frontend + bundled Postgres/MinIO) so egret runs **in-cluster** — resolving engine service DNS and unblocking data-plane e2e (SQL Lab / dataset / query).

**Architecture:** Two images (new backend multi-stage Dockerfile; reuse `frontend/Dockerfile` as-is by naming the backend Service `egret`). One Helm chart `deploy/helm/egret/` with bundled deps. Backend auto-selects `in_cluster()`; only a ServiceAccount + RBAC is needed. **Milestone A** ships OIDC **off** (local-admin login) — the data-plane unblock. **Milestone B** adds Keycloak + nginx-ingress single-origin for OIDC features.

**Tech Stack:** Rust (`egret-bin`, binary `egret`), Docker multi-stage, Helm 3, kind, Postgres 16, MinIO, (Milestone B) Keycloak 26 + nginx-ingress.

**Spec:** `docs/en/specs/2026-07-17-deploy-egret-to-kind-design.md`. **Branch:** `feat/deploy-egret-kind` (off `main`, already checked out).

**Verification note (IaC, not unit-TDD):** these tasks build/lint/render/dry-run then live-verify (docker build succeeds · `helm lint` · `helm template` renders · pods Ready · a real query). Where a task lists "Run → expected", that IS its test gate.

**Config facts (verified, `crates/egret-config/src/lib.rs`):** figment layers defaults → `egret.toml` (CWD) → `EGRET_*` env (`__` nests, e.g. `EGRET_DATABASE__URL`). We configure **entirely via env** (no `egret.toml` file in the image). Server defaults `0.0.0.0:8080`. Admin seed reads plain env `EGRET_ADMIN_EMAIL` / `EGRET_ADMIN_PASSWORD` (`main.rs:98`). Backend tries `in_cluster()` then kubeconfig (`main.rs:1324`). Migrations embed at compile time (run at boot). Frontend `nginx-prod.conf` proxies `/api/ → http://egret:8080`.

---

# Milestone A — in-cluster egret on kind, OIDC off (the data-plane unblock)

## Task A1: Backend Docker image + `.dockerignore`

**Files:**
- Create: `deploy/docker/backend.Dockerfile`
- Create: `.dockerignore` (repo root)

- [ ] **Step 1: Add `.dockerignore`** (repo root) so the build context isn't gigabytes:

```
target/
node_modules/
frontend/node_modules/
frontend/dist/
.git/
.codegraph/
**/*.log
docs/
tests/
```

- [ ] **Step 2: Write `deploy/docker/backend.Dockerfile`** (multi-stage; context = repo root):

```dockerfile
# ---- Stage 1: build the egret binary ----
FROM rust:bookworm AS builder
WORKDIR /src
# Dep-layer cache: copy manifests first. (Workspace has many crates; copying the
# whole tree is acceptable for a local image — keep it simple and correct.)
COPY . .
RUN cargo build --release -p egret-bin --locked

# ---- Stage 2: slim runtime ----
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd -r -u 10001 egret
COPY --from=builder /src/target/release/egret /usr/local/bin/egret
USER 10001
EXPOSE 8080
ENTRYPOINT ["egret"]
CMD ["serve"]
```

- [ ] **Step 3: Build it.**

Run: `docker build -f deploy/docker/backend.Dockerfile -t egret-backend:dev .`
Expected: build succeeds; final image present (`docker images | grep egret-backend`).

- [ ] **Step 4: Verify the binary runs + links resolve** (catches a missing runtime lib):

Run: `docker run --rm egret-backend:dev --help`
Expected: egret CLI help prints (no `error while loading shared libraries`). If it fails on a missing `.so`, add the lib to the `apt-get install` line (or switch the runtime base to `debian:bookworm`) and rebuild.

- [ ] **Step 5: Commit.**

```bash
git add deploy/docker/backend.Dockerfile .dockerignore
git commit -m "build(deploy): egret backend Docker image (multi-stage) + .dockerignore"
```

---

## Task A2: Helm chart scaffold — backend Deployment/Service/SA/RBAC + config

**Files:**
- Create: `deploy/helm/egret/Chart.yaml`
- Create: `deploy/helm/egret/values.yaml`
- Create: `deploy/helm/egret/templates/_helpers.tpl`
- Create: `deploy/helm/egret/templates/serviceaccount.yaml`
- Create: `deploy/helm/egret/templates/rbac.yaml`
- Create: `deploy/helm/egret/templates/secret.yaml`
- Create: `deploy/helm/egret/templates/configmap-env.yaml`
- Create: `deploy/helm/egret/templates/backend-deployment.yaml`
- Create: `deploy/helm/egret/templates/backend-service.yaml`
- Create: `deploy/helm/egret/templates/namespace-engines.yaml`

- [ ] **Step 1: `Chart.yaml`.**

```yaml
apiVersion: v2
name: egret
description: egret data portal — local kind deployment (dev only, NOT a production chart)
type: application
version: 0.1.0
appVersion: "dev"
```

- [ ] **Step 2: `values.yaml`** (dev/kind defaults):

```yaml
backend:
  image: { repository: egret-backend, tag: dev, pullPolicy: IfNotPresent }
  replicas: 1
frontend:
  image: { repository: egret-frontend, tag: dev, pullPolicy: IfNotPresent }
  nodePort: 30080
engineNamespace: egret-engines
admin:
  email: admin@egret.local
  password: Admin12345!
oidc:
  enabled: false
deps:
  enabled: true
  postgres:
    image: postgres:16
    user: egret
    password: egret
    db: egret
  minio:
    image: minio/minio:latest
    mcImage: minio/mc:latest
    rootUser: egret
    rootPassword: egretsecret
    bucket: warehouse
storage:
  region: us-east-1
  pathStyle: true
# master/secret keys: generated-and-persisted by the chart (see secret.yaml); do
# NOT put real secrets here.
```

- [ ] **Step 3: `_helpers.tpl`** — name helpers + the generate-once-persist secret values:

```
{{- define "egret.fullname" -}}egret{{- end -}}
{{/* Reuse an existing egret-secrets value if present (stable across upgrades),
     else generate. master_key ≥32 bytes b64; secret_key ≥64 bytes b64. */}}
{{- define "egret.masterKey" -}}
{{- $s := (lookup "v1" "Secret" .Release.Namespace "egret-secrets") -}}
{{- if and $s (index $s.data "master-key") -}}{{ index $s.data "master-key" | b64dec }}{{- else -}}{{ randBytes 32 }}{{- end -}}
{{- end -}}
{{- define "egret.secretKey" -}}
{{- $s := (lookup "v1" "Secret" .Release.Namespace "egret-secrets") -}}
{{- if and $s (index $s.data "secret-key") -}}{{ index $s.data "secret-key" | b64dec }}{{- else -}}{{ randBytes 64 }}{{- end -}}
{{- end -}}
```

(`randBytes N` returns a base64 string of N random bytes — exactly the base64 form the config expects.)

- [ ] **Step 4: `serviceaccount.yaml`** — SA `egret` in the release namespace.

```yaml
apiVersion: v1
kind: ServiceAccount
metadata: { name: egret, namespace: {{ .Release.Namespace }} }
```

- [ ] **Step 5: `namespace-engines.yaml`** — create the provisioning target namespace.

```yaml
apiVersion: v1
kind: Namespace
metadata: { name: {{ .Values.engineNamespace }} }
```

- [ ] **Step 6: `rbac.yaml`** — Role in `egret-engines` covering the CURRENT provisioner surface, + RoleBinding to the `egret` SA:

```yaml
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata: { name: egret-engine-provisioner, namespace: {{ .Values.engineNamespace }} }
rules:
  - apiGroups: ["apps"]
    resources: ["deployments", "statefulsets"]
    verbs: ["create","get","list","watch","patch","update","delete"]
  - apiGroups: [""]
    resources: ["services","configmaps","secrets","pods","pods/log","persistentvolumeclaims"]
    verbs: ["create","get","list","watch","patch","update","delete"]
  - apiGroups: ["batch"]
    resources: ["jobs"]
    verbs: ["create","get","list","watch","patch","delete"]
  - apiGroups: ["autoscaling"]
    resources: ["horizontalpodautoscalers"]
    verbs: ["create","get","list","watch","patch","delete"]
  - apiGroups: ["starrocks.com"]
    resources: ["starrocksclusters"]
    verbs: ["create","get","list","watch","patch","update","delete"]
  - apiGroups: [""]
    resources: ["events"]
    verbs: ["get","list","watch"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata: { name: egret-engine-provisioner, namespace: {{ .Values.engineNamespace }} }
subjects:
  - kind: ServiceAccount
    name: egret
    namespace: {{ .Release.Namespace }}
roleRef: { kind: Role, name: egret-engine-provisioner, apiGroup: rbac.authorization.k8s.io }
```

(`statefulsets` included: the kube-starrocks operator owns StatefulSets, but egret patches restart annotations onto the CR, not the STS — `statefulsets` is a safe read/patch superset; keep it since egret's Trino path uses Deployments and future paths may touch STS. If `helm template`/apply flags it unused it does no harm.)

- [ ] **Step 7: `secret.yaml`** — sensitive env values (generated keys + storage/admin creds):

```yaml
apiVersion: v1
kind: Secret
metadata: { name: egret-secrets, namespace: {{ .Release.Namespace }} }
type: Opaque
stringData:
  master-key: {{ include "egret.masterKey" . | quote }}
  secret-key: {{ include "egret.secretKey" . | quote }}
  storage-access-key: {{ .Values.deps.minio.rootUser | quote }}
  storage-secret-key: {{ .Values.deps.minio.rootPassword | quote }}
  catalog-auth-token: {{ randAlphaNum 40 | quote }}
  admin-password: {{ .Values.admin.password | quote }}
```

- [ ] **Step 8: `configmap-env.yaml`** — non-secret env:

```yaml
apiVersion: v1
kind: ConfigMap
metadata: { name: egret-env, namespace: {{ .Release.Namespace }} }
data:
  EGRET_DATABASE__URL: "postgres://{{ .Values.deps.postgres.user }}:{{ .Values.deps.postgres.password }}@postgres:5432/{{ .Values.deps.postgres.db }}"
  EGRET_STORAGE__ENDPOINT: "http://minio:9000"
  EGRET_STORAGE__BUCKET: "{{ .Values.deps.minio.bucket }}"
  EGRET_STORAGE__REGION: "{{ .Values.storage.region }}"
  EGRET_STORAGE__PATH_STYLE: "{{ .Values.storage.pathStyle }}"
  EGRET_CATALOG__ENABLED: "true"
  EGRET_CATALOG__WAREHOUSE: "s3://{{ .Values.deps.minio.bucket }}"
  EGRET_CATALOG__PUBLIC_URL: "http://egret:8080"
  EGRET_OIDC__ENABLED: "{{ .Values.oidc.enabled }}"
  EGRET_ADMIN_EMAIL: "{{ .Values.admin.email }}"
  EGRET_LOG__FILTER: "info,egret=debug"
```

- [ ] **Step 9: `backend-deployment.yaml`** — the egret backend, SA-bound, env from ConfigMap+Secret:

```yaml
apiVersion: apps/v1
kind: Deployment
metadata: { name: egret, namespace: {{ .Release.Namespace }} }
spec:
  replicas: {{ .Values.backend.replicas }}
  selector: { matchLabels: { app: egret } }
  template:
    metadata: { labels: { app: egret } }
    spec:
      serviceAccountName: egret
      containers:
        - name: egret
          image: "{{ .Values.backend.image.repository }}:{{ .Values.backend.image.tag }}"
          imagePullPolicy: {{ .Values.backend.image.pullPolicy }}
          ports: [{ containerPort: 8080 }]
          envFrom:
            - configMapRef: { name: egret-env }
          env:
            - { name: EGRET_STORAGE__ACCESS_KEY, valueFrom: { secretKeyRef: { name: egret-secrets, key: storage-access-key } } }
            - { name: EGRET_STORAGE__SECRET_KEY, valueFrom: { secretKeyRef: { name: egret-secrets, key: storage-secret-key } } }
            - { name: EGRET_SECRETS__MASTER_KEY, valueFrom: { secretKeyRef: { name: egret-secrets, key: master-key } } }
            - { name: EGRET_SECURITY__SECRET_KEY, valueFrom: { secretKeyRef: { name: egret-secrets, key: secret-key } } }
            - { name: EGRET_CATALOG__AUTH_TOKEN, valueFrom: { secretKeyRef: { name: egret-secrets, key: catalog-auth-token } } }
            - { name: EGRET_ADMIN_PASSWORD, valueFrom: { secretKeyRef: { name: egret-secrets, key: admin-password } } }
          readinessProbe: { tcpSocket: { port: 8080 }, initialDelaySeconds: 5, periodSeconds: 5 }
          livenessProbe: { tcpSocket: { port: 8080 }, initialDelaySeconds: 30, periodSeconds: 20 }
```

- [ ] **Step 10: `backend-service.yaml`** — MUST be named `egret` (so the frontend nginx `/api → http://egret:8080` resolves):

```yaml
apiVersion: v1
kind: Service
metadata: { name: egret, namespace: {{ .Release.Namespace }} }
spec:
  selector: { app: egret }
  ports: [{ port: 8080, targetPort: 8080 }]
```

- [ ] **Step 11: Render + lint.**

Run: `helm lint deploy/helm/egret` then `helm template egret deploy/helm/egret -n egret-system`
Expected: lint passes; template renders all resources with no template errors; the backend Service is named `egret`.

- [ ] **Step 12: Commit.**

```bash
git add deploy/helm/egret
git commit -m "feat(deploy): egret Helm chart — backend Deployment/Service/SA/RBAC + env config"
```

---

## Task A3: Bundled dependencies (Postgres + MinIO + bucket bootstrap)

**Files:**
- Create: `deploy/helm/egret/templates/deps-postgres.yaml`
- Create: `deploy/helm/egret/templates/deps-minio.yaml`
- Create: `deploy/helm/egret/templates/deps-minio-bucket-job.yaml`

- [ ] **Step 1: `deps-postgres.yaml`** — wrap the whole file in `{{- if .Values.deps.enabled }} … {{- end }}`. Deployment `postgres` (image `{{ .Values.deps.postgres.image }}`, env `POSTGRES_USER/PASSWORD/DB` from values), a PVC `postgres-data` (1Gi) mounted at `/var/lib/postgresql/data` (subPath `pgdata`), and a Service `postgres:5432`.

- [ ] **Step 2: `deps-minio.yaml`** — `{{- if .Values.deps.enabled }}`. Deployment `minio` (image, `args: ["server","/data","--console-address",":9001"]`, env `MINIO_ROOT_USER/PASSWORD` from values), PVC `minio-data` (2Gi) at `/data`, Service `minio` exposing `9000` (api) + `9001` (console).

- [ ] **Step 3: `deps-minio-bucket-job.yaml`** — `{{- if .Values.deps.enabled }}`. A `batch/v1` Job (Helm hook `post-install,post-upgrade`, `hook-delete-policy: before-hook-creation`) using `{{ .Values.deps.minio.mcImage }}` that waits for MinIO then creates the bucket:

```yaml
command: ["/bin/sh","-c"]
args:
  - |
    until mc alias set local http://minio:9000 "$MC_USER" "$MC_PASS" 2>/dev/null; do echo waiting for minio; sleep 2; done
    mc mb --ignore-existing local/{{ .Values.deps.minio.bucket }}
    echo "bucket ready"
env:
  - { name: MC_USER, value: "{{ .Values.deps.minio.rootUser }}" }
  - { name: MC_PASS, value: "{{ .Values.deps.minio.rootPassword }}" }
restartPolicy: OnFailure
```

- [ ] **Step 4: Render + lint.**

Run: `helm lint deploy/helm/egret && helm template egret deploy/helm/egret -n egret-system | grep -E "kind: (Deployment|Service|Job|PersistentVolumeClaim)"`
Expected: postgres + minio Deployments, their Services, the bucket Job, and 2 PVCs all render.

- [ ] **Step 5: Commit.**

```bash
git add deploy/helm/egret/templates/deps-*.yaml
git commit -m "feat(deploy): bundle Postgres + MinIO (with bucket bootstrap) in the chart"
```

---

## Task A4: Frontend Deployment + NodePort + kind port-map

**Files:**
- Create: `deploy/helm/egret/templates/frontend-deployment.yaml`
- Create: `deploy/helm/egret/templates/frontend-service.yaml`
- Modify: `deploy/k8s/kind.yaml` (add the frontend NodePort mapping)

- [ ] **Step 1: `frontend-deployment.yaml`** — Deployment `egret-frontend` (image `{{ .Values.frontend.image.repository }}:{{ .Values.frontend.image.tag }}`, `containerPort: 80`, `readinessProbe httpGet: { path: /health, port: 80 }` — the nginx conf serves `/health`).

- [ ] **Step 2: `frontend-service.yaml`** — a NodePort Service:

```yaml
apiVersion: v1
kind: Service
metadata: { name: egret-frontend, namespace: {{ .Release.Namespace }} }
spec:
  type: NodePort
  selector: { app: egret-frontend }
  ports: [{ port: 80, targetPort: 80, nodePort: {{ .Values.frontend.nodePort }} }]
```

- [ ] **Step 3: Add the NodePort to `deploy/k8s/kind.yaml`** `extraPortMappings` (alongside the existing 30678):

```yaml
      - containerPort: 30080
        hostPort: 30080
        protocol: TCP
```

Add a comment: `# egret-frontend NodePort → browse the SPA at http://localhost:30080`.

- [ ] **Step 4: Render + lint.**

Run: `helm lint deploy/helm/egret && helm template egret deploy/helm/egret -n egret-system | grep -A3 "kind: Service" | grep -E "egret-frontend|NodePort|30080"`
Expected: the frontend NodePort Service renders on 30080.

- [ ] **Step 5: Commit.**

```bash
git add deploy/helm/egret/templates/frontend-*.yaml deploy/k8s/kind.yaml
git commit -m "feat(deploy): frontend Deployment + NodePort + kind port-map"
```

---

## Task A5: `just deploy-kind` orchestration + live bring-up (acceptance)

**Files:**
- Modify: `justfile` (add `deploy-kind`, `kind-undeploy`, `images-build`)
- Create: `deploy/helm/egret/README.md`

- [ ] **Step 1: Add justfile targets.** (Match the existing justfile style; `egret` is the kind cluster name from `deploy/k8s/kind.yaml`.)

```make
# Build both egret images (backend + frontend).
images-build:
    docker build -f deploy/docker/backend.Dockerfile -t egret-backend:dev .
    docker build -f frontend/Dockerfile -t egret-frontend:dev frontend

# Build images, load them into the kind `egret` cluster, and (re)install the chart.
deploy-kind: images-build
    kind load docker-image egret-backend:dev egret-frontend:dev --name egret
    helm upgrade --install egret deploy/helm/egret -n egret-system --create-namespace --wait --timeout 10m
    @echo "egret SPA: http://localhost:30080  (login {{ '{{admin}}' }})"

kind-undeploy:
    helm uninstall egret -n egret-system || true
```

- [ ] **Step 2: `deploy/helm/egret/README.md`** — document prerequisites (`just k8s-up` for the cluster + operators, then `just deploy-kind`), the SPA URL (`http://localhost:30080`), local-admin login (`admin@egret.local` / `Admin12345!`), OIDC-off note, and `just kind-undeploy` teardown.

- [ ] **Step 3: Live bring-up.** With the kind cluster up (`just k8s-up`; ensure the kube-starrocks operator is installed per `deploy/starrocks-operator/`):

Run: `just deploy-kind`
Expected: images build + load; `helm upgrade --install … --wait` returns with all pods Ready. Verify: `kubectl -n egret-system get pods` shows `egret`, `egret-frontend`, `postgres`, `minio` all `Running`/`Ready`, and the bucket Job `Completed`.

- [ ] **Step 4: Acceptance smoke (the unblock).**
  1. Browse `http://localhost:30080` → log in `admin@egret.local` / `Admin12345!` → control plane loads.
  2. Seed a warehouse: `just seed-demo` (or create one in the UI against the `warehouse` bucket).
  3. Provision a **StarRocks** engine wired to that warehouse → wait until Running (`kubectl -n egret-engines get pods -w`).
  4. Link the engine to a workspace, open **SQL Lab**, run `SHOW DATABASES;` / `SELECT 1;` against the StarRocks engine → **rows return** (in-cluster egret resolves `<engine>-fe-service.egret-engines.svc:9030`). Screenshot.

- [ ] **Step 5: Report + commit.**

```bash
git add justfile deploy/helm/egret/README.md
git commit -m "feat(deploy): just deploy-kind orchestration + chart README"
```

Report exactly what was verified live (pods Ready, login, provision, the SQL Lab rows) with the screenshot.

---

# Milestone B — OIDC on (Keycloak + nginx-ingress single-origin)

> Do Milestone B only after A's acceptance smoke passes. B swaps NodePort browsing for an ingress host so the OIDC issuer URL is identical for browser and backend.

## Task B1: nginx-ingress + kind :80 mapping + ingress routes

**Files:**
- Modify: `deploy/k8s/kind.yaml` (add `:80`/`:443` extraPortMappings + `node-labels` for ingress-ready)
- Create: `deploy/helm/egret/templates/ingress.yaml`
- Modify: `deploy/helm/egret/values.yaml` (`ingress.enabled`, `ingress.host: egret.localtest.me`)
- Modify: `justfile` (`ingress-install` target: `kubectl apply` the kind nginx-ingress manifest)

- [ ] **Step 1:** Add `ingress.enabled=false` + `ingress.host: egret.localtest.me` to values.
- [ ] **Step 2:** Add kind `extraPortMappings` 80→80 and 443→443 + `kubeadmConfigPatches` node-label `ingress-ready=true` (standard kind ingress recipe). Document that this needs a cluster recreate.
- [ ] **Step 3:** `ingress-install` justfile target applying the kind-specific `ingress-nginx` deploy manifest.
- [ ] **Step 4:** `ingress.yaml` (guarded by `ingress.enabled`): host `{{ .Values.ingress.host }}`, `/` → `egret-frontend:80`, `/auth` (+ `/realms`, `/resources`) → `keycloak:8080` (added in B2). `localtest.me` resolves to `127.0.0.1`, so the browser reaches it via the kind :80 map.
- [ ] **Step 5: Render + lint.** `helm lint` + `helm template … --set ingress.enabled=true` shows the Ingress with both paths. Commit.

## Task B2: Keycloak dependency + backend hostAliases + OIDC wiring

**Files:**
- Create: `deploy/helm/egret/templates/deps-keycloak.yaml`
- Modify: `deploy/helm/egret/templates/backend-deployment.yaml` (hostAliases)
- Modify: `deploy/helm/egret/templates/configmap-env.yaml` (OIDC env when enabled)
- Modify: `deploy/helm/egret/values.yaml` (`oidc.*`)

- [ ] **Step 1:** `deps-keycloak.yaml` (guard `{{- if .Values.oidc.enabled }}`): Keycloak 26 Deployment with `args: ["start-dev","--import-realm"]`, `KC_HOSTNAME=http://{{ .Values.ingress.host }}/auth`, `KC_HTTP_RELATIVE_PATH=/auth`, the realm JSON mounted from a ConfigMap built from `deploy/keycloak/egret-realm.json`, Service `keycloak:8080`.
- [ ] **Step 2:** Add `hostAliases` on the backend Deployment mapping `{{ .Values.ingress.host }}` → the ingress-controller Service ClusterIP (templated via `.Values.ingress.controllerIP`, or a CoreDNS rewrite documented as the alternative) so the backend validates the issuer at the SAME host as the browser.
- [ ] **Step 3:** When `oidc.enabled`, set env: `EGRET_OIDC__ENABLED=true`, `EGRET_OIDC__ISSUER_URL=http://{{ .Values.ingress.host }}/auth/realms/egret`, `EGRET_OIDC__CLIENT_ID` + `EGRET_OIDC__CLIENT_SECRET` (from the realm JSON; secret into `egret-secrets`), `EGRET_OIDC__REDIRECT_URL=http://{{ .Values.ingress.host }}/api/auth/oidc/callback`, `EGRET_CATALOG__PUBLIC_URL` stays `http://egret:8080` (in-cluster).
- [ ] **Step 4: Render + lint.** `helm template … --set oidc.enabled=true` renders Keycloak + the OIDC env + hostAliases. Commit.

## Task B3: OIDC live smoke

- [ ] **Step 1:** Recreate the cluster with the B1 kind config, `just k8s-up`, `just ingress-install`, `just deploy-kind` with `--set oidc.enabled=true`.
- [ ] **Step 2:** Add `egret.localtest.me` resolves to 127.0.0.1 automatically (public DNS) — no `/etc/hosts` edit needed. Browse `http://egret.localtest.me/` → **Login with SSO** → Keycloak → back to egret.
- [ ] **Step 3:** Verify an OIDC-group-mapped role applies, and (if a StarRocks/Trino engine with delegation is provisioned) a JWT-delegated query. Screenshot. Report exactly what worked / what hit the known delegation-bootstrap caveats.

---

## Self-Review

- **Spec coverage:** backend image (A1) · chart backend+RBAC+config (A2) · bundled deps (A3) · frontend+NodePort (A4) · orchestration+acceptance (A5) → Milestone A. OIDC single-origin: ingress (B1) · Keycloak+hostAliases+OIDC env (B2) · live (B3) → Milestone B. All spec sections covered. ✔
- **No placeholders:** real Dockerfile, real env var names (from `egret-config`), real RBAC verbs (batch/jobs + autoscaling/HPA + starrocks.com CR), the `egret`-named Service that reuses the frontend image, `kind load docker-image … --name egret`, generate-and-persist secret helpers. ✔
- **Type/name consistency:** backend Service `egret` (matches nginx `/api → http://egret:8080`); cluster name `egret`; namespace `egret-system` (release) + `egret-engines` (provision target); image tags `egret-backend:dev` / `egret-frontend:dev` consistent across Dockerfile build, `kind load`, and values. ✔
- **kind load:** explicit in A5 (`kind load docker-image egret-backend:dev egret-frontend:dev --name egret`) per the user's note. ✔
