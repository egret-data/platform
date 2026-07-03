# Trino-on-Kubernetes e2e harness (kind + Stackable operator)

This is the **opt-in, manual** end-to-end path for egret's engine provisioning
against a real Kubernetes cluster with the [Stackable](https://stackable.tech/)
Trino operator. It is **NOT part of CI** — same posture as the existing manual
Trino integration test. Run it locally when you change the
`LiveKube` provisioner or the Trino / TrinoCatalog manifest generation.

## Prerequisites

Install these on your host (Homebrew names shown):

- `kind`    — `brew install kind`
- `helm`    — `brew install helm`
- `kubectl` — `brew install kubectl`
- Docker (kind runs the cluster as containers)

## `just k8s-up`

`just k8s-up` does everything needed to stand the cluster up:

1. `kind create cluster --name egret --config deploy/k8s/kind.yaml`
2. `helm repo add stackable-stable https://repo.stackable.tech/repository/helm-stable/`
   + `helm repo update`
3. Installs the Stackable operators into the `stackable-operators` namespace
   (`--create-namespace`). Trino needs its dependency operators, so all four
   are installed:
   - `commons-operator`
   - `secret-operator`
   - `listener-operator`
   - `trino-operator`
4. `kubectl create namespace egret-engines` — the target namespace egret
   provisions Trino clusters into.

Tear the cluster down with `just k8s-down` (`kind delete cluster --name egret`).

## RBAC for egret's ServiceAccount

egret provisions and reconciles Trino resources in the `egret-engines`
namespace, so its ServiceAccount needs permission to manage
`trinoclusters` + `trinocatalogs` (apiGroup `trino.stackable.tech`), the
`secrets` that back catalog credentials (core), and to read `pods` for
readiness polling.

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
  - apiGroups: ["trino.stackable.tech"]
    resources: ["trinoclusters", "trinocatalogs"]
    verbs: ["create", "get", "patch", "delete"]
  - apiGroups: [""]
    resources: ["secrets"]
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

1. **Bring up the cluster + operators:**

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
   `TrinoCluster` + `TrinoCatalog` (+ credential `Secret`) into `egret-engines`.

4. **Wait for the pods to become Ready:**

   ```bash
   kubectl -n egret-engines get pods -w
   ```

   Wait until the coordinator and worker pods are `Running` / `Ready`.

5. **Query the wired catalog** via the Trino CLI / coordinator. The coordinator
   listener is reachable on the host at `localhost:30678` (see the
   `extraPortMappings` in `deploy/k8s/kind.yaml`); otherwise
   `kubectl -n egret-engines port-forward svc/<trino-coordinator> 8080:8080`.

   ```sql
   SELECT * FROM <catalog>.demo_ns.events LIMIT 5;
   ```

   where `<catalog>` is the catalog name egret wired to `demo-catalog`. Rows
   coming back confirms provisioning + catalog wiring end-to-end.

## Cleanup

```bash
just k8s-down
```

---

**Reminder:** this harness is opt-in and NOT wired into CI. Nothing here runs
automatically; it exists for manual verification of the Kubernetes provisioning
path.
