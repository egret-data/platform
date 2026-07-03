# Trino-on-Kubernetes e2e harness (kind, plain manifests)

This is the **opt-in, manual** end-to-end path for egret's engine provisioning
against a real Kubernetes cluster. As of Phase 1.5, egret provisions Trino as
**plain stateless Deployments** (coordinator + worker Deployments, a Service,
two ConfigMaps, and an optional env Secret) that egret templates and applies
itself — **no Stackable operator, no CRs, no PVCs**. It is **NOT part of CI** —
same posture as the existing manual Trino integration test. Run it locally when
you change the `LiveKube` provisioner or the Trino manifest templating.

## Prerequisites

Install these on your host (Homebrew names shown):

- `kind`    — `brew install kind`
- `kubectl` — `brew install kubectl`
- Docker (kind runs the cluster as containers)

`helm` is **no longer required** for Trino — egret applies the workloads
directly. It's only needed if/when the StarRocks/RisingWave phases add their own
operators (installed via helm in `just k8s-up` at that point).

## `just k8s-up`

`just k8s-up` does everything needed to stand the cluster up:

1. `kind create cluster --name egret --config deploy/k8s/kind.yaml`
2. `kubectl create namespace egret-engines` — the target namespace egret
   provisions Trino Deployments into.

No operators are installed. (Future StarRocks/RisingWave phases will add their
operators to this recipe.)

Tear the cluster down with `just k8s-down` (`kind delete cluster --name egret`).

## RBAC for egret's ServiceAccount

egret provisions and reconciles plain Kubernetes workloads in the
`egret-engines` namespace, so its ServiceAccount needs permission to manage
`deployments` (apiGroup `apps`) and the `services` / `configmaps` / `secrets`
(core) that make up an engine, plus read access to `pods` for readiness polling.

Apply this `Role` + `RoleBinding` in the target namespace. Replace
`serviceAccountName` / `namespace` in the subject with wherever egret's pod
runs (for an out-of-cluster egret using a kubeconfig, bind the user/SA that
kubeconfig authenticates as instead).

```yaml
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: egret-engine-provisioner
  namespace: egret-engines
rules:
  - apiGroups: ["apps"]
    resources: ["deployments"]
    verbs: ["create", "get", "patch", "delete"]
  - apiGroups: [""]
    resources: ["services", "configmaps", "secrets"]
    verbs: ["create", "get", "patch", "delete"]
  - apiGroups: [""]
    resources: ["pods"]
    verbs: ["get", "list"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: egret-engine-provisioner
  namespace: egret-engines
subjects:
  - kind: ServiceAccount
    name: egret
    namespace: egret-engines
roleRef:
  kind: Role
  name: egret-engine-provisioner
  apiGroup: rbac.authorization.k8s.io
```

## Manual e2e steps

1. **Bring up the cluster (no operators):**

   ```bash
   just k8s-up
   ```

2. **Run egret pointed at the kind kubeconfig** so `LiveKube::from_kubeconfig()`
   connects to the kind cluster:

   ```bash
   kind export kubeconfig --name egret          # writes to ~/.kube/config
   # or: kind get kubeconfig --name egret > /tmp/egret.kubeconfig
   #     export KUBECONFIG=/tmp/egret.kubeconfig
   just serve
   ```

3. **Provision a Trino from the egret UI**, wired to the `demo-catalog`
   warehouse (seed it first with `just seed-demo` if needed). egret writes a
   coordinator + worker `Deployment`, a `Service`, the config + catalogs
   `ConfigMap`s, and an env `Secret` into `egret-engines` — all plain workloads,
   using the official `trinodb/trino` image.

4. **Wait for the pods to become Ready:**

   ```bash
   kubectl -n egret-engines get pods -w
   ```

   Wait until the coordinator and worker pods are `Running` / `Ready`.

5. **Query the wired catalog** via the Trino CLI / coordinator. The coordinator
   Service is reachable on the host via the NodePort mapping in
   `deploy/k8s/kind.yaml` (`localhost:30678`); otherwise
   `kubectl -n egret-engines port-forward svc/<engine>-trino 8080:8080`.

   ```sql
   SELECT * FROM <catalog>.demo_ns.events LIMIT 5;
   ```

   where `<catalog>` is the catalog name egret wired to `demo-catalog`. Rows
   coming back confirms provisioning + catalog wiring end-to-end.

6. **Add a second catalog and cross-query.** In the egret UI, add a `tpch`
   catalog to the same engine. egret re-renders the catalogs `ConfigMap` and
   bumps the pod-template checksum, so the Deployments **roll** (Trino has no
   hot-reload). Watch them come back:

   ```bash
   kubectl -n egret-engines get pods -w
   ```

   Once Ready, run a cross-catalog query — the wired warehouse catalog plus the
   new `tpch` catalog are both live on the one engine:

   ```sql
   SELECT count(*) FROM tpch.tiny.nation;
   ```

   A row count coming back confirms the multi-catalog + rolling-restart path.

## Cleanup

```bash
just k8s-down
```

---

**Reminder:** this harness is opt-in and NOT wired into CI. Nothing here runs
automatically; it exists for manual verification of the Kubernetes provisioning
path.
