# egret Helm chart (local kind)

Deploys the full egret stack into a local **kind** cluster for development.

> **Dev only — NOT for production.** This chart bundles Postgres and MinIO,
> ships an insecure local-admin login, and targets NodePorts on `localhost`. It
> is meant for a single-developer kind cluster, not a real deployment.

## Prerequisites

- **Docker** running.
- A **kind cluster named `egret`** with the target namespace and engine
  operators. Create it with `just k8s-up` (see [`deploy/k8s/README.md`](../../k8s/README.md)),
  which also maps the NodePorts used below in [`deploy/k8s/kind.yaml`](../../k8s/kind.yaml).
- The **kube-starrocks operator** installed (see [`deploy/starrocks-operator/`](../../starrocks-operator/)),
  so the backend can provision StarRocks engines.

## Deploy

```bash
just deploy-kind
```

This runs `just images-build` (builds `egret-backend:dev` and
`egret-frontend:dev`), then:

```bash
kind load docker-image egret-backend:dev egret-frontend:dev --name egret
helm upgrade --install egret deploy/helm/egret -n egret-system --create-namespace --wait --timeout 10m
```

> The backend image is a **full Rust release build** — the first build is slow.
> Subsequent builds reuse the Docker layer cache and are much faster.

## Access

- **SPA:** <http://localhost:30080> — the frontend is exposed on NodePort
  `30080`, mapped to the host in [`deploy/k8s/kind.yaml`](../../k8s/kind.yaml).
- **Login:** local-admin `admin@egret.local` / `Admin12345!`.
  OIDC is **off** in this milestone, so the local-admin account is the only way in.

## What runs

- **egret backend** (in-cluster) — provisions engines into the `egret-engines`
  namespace.
- **nginx frontend** — serves the built SPA.
- **Bundled Postgres** — the control-plane metastore.
- **Bundled MinIO** — object storage, pre-created bucket `warehouse`.

## Configuration

Everything is driven by `EGRET_*` environment variables, sourced from the
ConfigMap `egret-env` and the Secret `egret-secrets`. Edit the values in
[`values.yaml`](./values.yaml).

To use an **external Postgres/MinIO** instead of the bundled ones, set
`deps.enabled=false` and point the `EGRET_*` connection values at your
services.

## Teardown

```bash
just kind-undeploy   # uninstall the egret release only (keeps cluster + operators)
just k8s-down        # delete the whole kind cluster
```

## Roadmap

OIDC / Keycloak login is **Milestone B** — see the deployment plan/spec. This
milestone (A) ships local-admin auth only.
