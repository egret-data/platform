# Deploy egret to Kubernetes (kind) — Design

**Date:** 2026-07-17 · **Status:** Design — awaiting approval · **Branch:** `feat/deploy-egret-kind` (off `main`)

> Vietnamese reference: `docs/vi/specs/2026-07-17-deploy-egret-to-kind-design.md`.

## Problem

egret provisions query engines (Trino/StarRocks) *into* a Kubernetes cluster, but egret itself has only ever run **on the host** (`just serve` + local kubeconfig). That host process cannot resolve the cluster-internal engine DNS (`<name>-fe-service.egret-engines.svc:9030`), so the entire **data plane** — SQL Lab queries, dataset previews, StarRocks/Trino execution, the delegation/root bootstrap SQL — has been **dev-DNS-blocked** and never end-to-end verified (documented across the StarRocks slices).

There is **no backend container image and no Helm chart** today (only `frontend/Dockerfile` = nginx, and `kernel-image/Dockerfile` = the notebook kernel).

## Goal

One command stands egret up **inside** the existing kind cluster — backend + frontend + its runtime dependencies — so egret runs in-cluster, resolves engine service DNS, and finally makes the data plane testable end-to-end. Scope is **local kind for e2e**, not a production chart.

This is **Project 1** of two. **Project 2** (a separate spec) is the full automated e2e suite that runs against this deployment.

## Non-goals

- **Not a production chart** — no Ingress/TLS termination for prod, no HA, no external managed Postgres/OIDC, no resource-tuning for scale. Dev/kind values only. (Templates stay reasonably structured, but prod values are out of scope.)
- **No application code changes** — the backend already supports in-cluster kube config; we only containerize + chart it.
- **Not the notebook kernel image** — `kernel-image/` is provisioned per-workspace and orthogonal; out of scope here.
- **Not CI-wired** — like the existing `deploy/k8s` harness, this is opt-in/manual.

## Background — what the code already gives us (verified)

- **In-cluster kube is automatic.** `egret-bin serve` does `LiveKube::in_cluster().await` first, falling back to `from_kubeconfig()` (`crates/egret-bin/src/main.rs:1324`). `KubeClient::in_cluster()` uses `kube::Config::incluster()` (`kube_client.rs:266`). So a pod with a mounted ServiceAccount token needs **no config** to talk to the API — it just needs **RBAC**.
- **The frontend image is reusable as-is.** `frontend/nginx-prod.conf` proxies `/api/ → http://egret:8080`. If the backend Service is named **`egret`** on port 8080, the existing `frontend/Dockerfile` works **unchanged**.
- **OIDC is toggleable.** `config.oidc.enabled` gates all OIDC wiring (`main.rs:985`), and `/api/.../config` exposes `oidc_enabled` to the SPA. The seeded local admin (`EGRET_ADMIN_EMAIL`/`EGRET_ADMIN_PASSWORD`, default `admin@egret.local` / `Admin12345!`) works with OIDC off. This gives an incremental bring-up: **Milestone A** deploys with OIDC **off** (local-admin login — unblocks the whole data plane immediately); **Milestone B** layers Keycloak back on for the OIDC-dependent features (group-mapped roles, JWT delegation).
- **Runtime deps** (from `deploy/docker-compose.dev.yml`): Postgres 16, MinIO, Keycloak 26, plus the engines egret provisions. `deploy/keycloak/egret-realm.json` is the realm import.
- **Rust toolchain is `stable`** (`rust-toolchain.toml`) → builder `rust:bookworm`. Migrations are **embedded at compile time** (sqlx) and run at boot — the runtime image needs no migration files.

## Architecture

Two images, one Helm chart, dependencies bundled, deployed onto the existing kind cluster.

### A. Backend image — `deploy/docker/backend.Dockerfile` (new)

Multi-stage, build context = repo root (needs `crates/`, `third_party/`, `Cargo.*`, `migrations/`):
- **Builder** `rust:bookworm`: copy the workspace, `cargo build --release -p egret-bin --locked`. Dependency-layer caching (copy `Cargo.toml`/`Cargo.lock` + crate manifests first) to keep rebuilds cheap.
- **Runtime** `debian:bookworm-slim`: `ca-certificates` (outbound TLS: OIDC discovery, S3/MinIO) plus any dynamic libs the binary links (confirm with `ldd` during impl). Non-root user. Copy the `egret` binary only. `ENTRYPOINT ["egret", "serve"]`, `EXPOSE 8080`. No migrations dir (embedded).

### B. Frontend image

**Reuse `frontend/Dockerfile` unchanged.** The chart names the backend Service `egret` so the baked-in `/api/ → http://egret:8080` proxy resolves in-cluster.

### C. Helm chart — `deploy/helm/egret/`

Templates:
- **Namespaces**: the chart's own release namespace (e.g. `egret-system`) for egret + deps, and **`egret-engines`** (the provisioning target; created here).
- **`egret` backend**: Deployment + Service (`egret`, ClusterIP :8080) + **ServiceAccount `egret`** + `egret.toml` ConfigMap + Secret (master_key, storage keys, DB URL/password, OIDC client secret). Readiness/liveness on `/api/health` (or the served health route). The SA is bound to a **Role in `egret-engines`** (see RBAC below).
- **`egret-frontend`**: Deployment (built frontend image) + Service **NodePort** so the browser reaches the SPA via the kind port-map.
- **Bundled dependencies** (values-toggled `deps.enabled`, default true for kind):
  - **Postgres 16** Deployment + Service + PVC; the chart seeds `DATABASE_URL`.
  - **MinIO** Deployment + Service + PVC; buckets pre-created (init or console), keys wired into the egret Secret + `egret.toml [storage]`.
  - **Keycloak 26** Deployment + Service, importing `deploy/keycloak/egret-realm.json` (only when OIDC is enabled — Milestone B).
- **values.yaml**: image repo/tag for backend+frontend, `deps.enabled`, `oidc.enabled` (default false for Milestone A), NodePorts, admin email/password, resource requests, storage/secret values.

### D. RBAC — extend the Trino-era Role to the current provisioner surface

`deploy/k8s/README.md`'s Role covers only `apps/deployments` + core `services/configmaps/secrets/pods`. The current provisioner also creates **`batch/jobs`** (StarRocks initPassword + keystore), **`autoscaling/v2 horizontalpodautoscalers`** (CN HPA), and **`starrocks.com` CRs** (`starrocksclusters`). The chart ships a **Role in `egret-engines`** (CRs are namespaced, so a namespaced Role suffices) with `create/get/list/patch/delete` on:
`apps/{deployments}` · core/{services,configmaps,secrets,pods,pods/log,persistentvolumeclaims} · `batch/{jobs}` · `autoscaling/{horizontalpodautoscalers}` · `starrocks.com/{starrocksclusters}` · core/{events} (read). A `RoleBinding` binds the `egret` SA (in `egret-system`) to it. (Operators — kube-starrocks, Trino/Stackable if used — are installed separately by `just k8s-up`, unchanged.)

### E. OIDC single-origin (Milestone B)

The classic kind gotcha: the OIDC **issuer URL must be identical** for the browser (redirect) and the backend (token/JWKS validation), but `localhost:<nodeport>` means different things to each. Design:
- Front the stack with **nginx-ingress** on the kind node, using a wildcard-resolving host like **`egret.localtest.me`** (resolves to `127.0.0.1` publicly). SPA at `http://egret.localtest.me/`, Keycloak at `http://egret.localtest.me/auth/…`, so **one origin, one port (80)** → the issuer string is consistent.
- The **browser** resolves `egret.localtest.me → 127.0.0.1 →` kind extraPortMapping → ingress. The **backend pod** resolves the same host to the ingress-controller ClusterIP via a **`hostAliases`** entry (or a CoreDNS rewrite). Same host, same port → the issuer matches on both sides.
- `config.oidc.issuer_url = http://egret.localtest.me/auth/realms/egret`; Keycloak `KC_HOSTNAME`/frontend-URL set to match.
- **Milestone A ships first with `oidc.enabled=false`** (local-admin login, no ingress needed) to unblock the data plane fast; Milestone B adds the ingress + Keycloak + this single-origin wiring for the OIDC-dependent features.

### F. Orchestration — `just deploy-kind`

A justfile target chaining: build both images → `kind load docker-image egret-backend:<tag> egret-frontend:<tag> --name egret` → `helm upgrade --install egret deploy/helm/egret -n egret-system --create-namespace`. Plus `just kind-undeploy` (`helm uninstall`). `deploy/k8s/kind.yaml` gains the `extraPortMappings` for the frontend NodePort (Milestone A) and ingress :80 (Milestone B).

## Data flow

`just deploy-kind` → images loaded into kind → `helm install` brings up Postgres/MinIO(/Keycloak) + egret backend (SA+RBAC, `in_cluster()` auto) + frontend NodePort. Browser → frontend NodePort → SPA → `/api` → backend Service `egret:8080`. egret provisions engines into `egret-engines`; being **in-cluster**, it reaches `<engine>-fe-service.egret-engines.svc:9030` → **SQL Lab / dataset / query now work**.

## Testing strategy

- **Build/lint:** both images build; `helm lint deploy/helm/egret`; `helm template` renders cleanly; chart unit checks (e.g. `helm template | kubeconform` if available).
- **Live bring-up (the acceptance test):**
  - `just deploy-kind` → all pods Ready (backend, frontend, postgres, minio[, keycloak]).
  - Browser via NodePort → login (local-admin, Milestone A) → control plane loads.
  - Seed a warehouse (`just seed-demo`) → provision a **StarRocks** engine → it reaches Running.
  - **SQL Lab: run `SHOW DATABASES` / `SELECT 1` against the StarRocks engine → rows return** (the core unblock — in-cluster egret resolves the FE service DNS).
  - Provision a **Trino** engine + cross-catalog query (the existing `deploy/k8s/README.md` flow, now from in-cluster egret).
  - Milestone B: OIDC login via `egret.localtest.me`, a group-mapped role, a JWT-delegated query.
- **Screenshots** of the working in-cluster stack + a successful SQL Lab result.

## Risks

1. **OIDC dual-URL (Milestone B)** — the single hardest piece. Mitigated by shipping Milestone A (OIDC off) first and using the `localtest.me` single-origin + `hostAliases` pattern for B. If B stalls, the data-plane e2e still runs under local-admin.
2. **Backend image build time / size** — a full release build of the Rust workspace is slow. Mitigated by dependency-layer caching in the Dockerfile; acceptable for a local/opt-in image (not CI-hot-path).
3. **Runtime dynamic-lib gaps** — a slim runtime may miss a lib the binary links. Mitigated by an `ldd` check during impl and adding only what's needed (or falling back to `debian:bookworm` non-slim).
4. **MinIO/Postgres data & bucket bootstrap** — bucket + DB must exist before egret boots. Mitigated by init steps / an initContainer and readiness gating (egret retries DB on boot).
5. **Engine RBAC drift** — if a future engine type adds a new resource kind, the Role must grow. Mitigated by centralizing the verb list in the chart with a comment tying it to the provisioner.

## Project 2 (separate spec) — full automated e2e, decomposed

Once Project 1 is live, Project 2 authors the full Playwright e2e suite on the existing `frontend/tests/e2e/` harness, run against the in-cluster deployment. Because "all features" is a huge surface, Project 2 is itself split into per-area specs/plans: **auth/login** · **control-plane admin** (users/groups/roles/permissions/access-explorer/audit) · **engine provisioning** (Trino + StarRocks lifecycle) · **data plane** (SQL Lab / datasets / charts / dashboards / saved queries) · **governance** (policies / masking / row-filter / classifications / Cedar ABAC) · **notebooks** (Daft kernel). Each area is a bite-sized plan with its own happy-path + key-edge specs. (Test-infra note: egret's e2e is Vitest+Playwright; these run opt-in/manual, not in the infra-red CI.)

## Self-review

- **Scope focused:** local-kind deploy only (backend image + reuse frontend + one chart + bundled deps + RBAC); prod concerns and the e2e suite are explicitly separate. ✔
- **No placeholders:** the in-cluster auto-selection, the `egret`-named Service that makes the frontend image reusable, the exact RBAC verb/resource surface, the OIDC single-origin strategy with its A/B milestones, and the deps list are all concrete. ✔
- **Honest about the hard part:** OIDC dual-URL is called out as the top risk with a milestone-A fallback that still delivers the data-plane unblock. ✔
- **Ambiguity:** Milestone A (OIDC off, NodePort, local admin) is the first shippable; Milestone B (OIDC on, ingress, single-origin) follows. ✔
