# StarRocks Dev Config — JWT Security Integration (F2-P4)

This directory contains the StarRocks `authentication_jwt` security integration
artifact for F2-P4. The integration is applied manually and validated end-to-end
in **F2-P5** against the running `starrocks/allin1-ubuntu:latest` image.

## Connector status

The egret-engine `StarRocksConnector` (Task 1 of this phase) is **complete and
unit-tested**. It presents the user's OIDC engine JWT via the `mysql_clear_password`
plugin on every query connection. This README covers the server-side configuration.

## How to apply

Wait until the StarRocks FE is healthy (the compose healthcheck polls
`mysql -h127.0.0.1 -P9030 -uroot -e 'select 1'`), then run:

```bash
mysql -h127.0.0.1 -P9030 -uroot < deploy/starrocks/init/security-integration.sql
```

This creates the `egret_oidc` security integration. Activation is a separate step
confirmed at P5 (see below).

## What F2-P5 finalizes

| Item | Detail |
|---|---|
| **JWKS URL host** | The SQL uses compose-internal DNS (`keycloak:8080`). If the FE resolves JWKs from the host instead (e.g. when running StarRocks outside compose), change `jwks_url` to `http://localhost:8081/realms/egret/protocol/openid-connect/certs`. |
| **Issuer / audience** | `required_issuer` and `required_audience` must match what the `starrocks` audience mapper in Keycloak realm `egret` (added in F2-P1) emits. Confirm by decoding a live token and comparing the `iss` and `aud` claims. |
| **Group → role mapping** | Notebook users carry a `groups` JWT claim. Options: (a) native StarRocks RBAC — `CREATE ROLE` per group, `GRANT ... TO ROLE` keyed on the JWT groups claim once the integration surfaces it; (b) a StarRocks GROUP PROVIDER DDL that reads `groups` from the integration and maps to database roles. The right approach depends on the allin1-ubuntu version's DDL surface area. |
| **Integration activation** | `CREATE SECURITY INTEGRATION` registers the integration but does not activate it. Three candidate forms are documented as comments in `init/security-integration.sql` (ADMIN SET FRONTEND CONFIG `authentication_chain`, ALTER SYSTEM, or GROUP PROVIDER DDL). The correct syntax is confirmed and uncommented at P5. |

## Files

| File | Purpose |
|---|---|
| `init/security-integration.sql` | Creates the `egret_oidc` JWT security integration; activation options are commented for P5 confirmation |

## Security note

The `mysql_clear_password` plugin transmits the JWT as cleartext over plain TCP.
Port 9030 in this compose has **no TLS**. This is intentional for local dev only.
Any deployment beyond the dev compose must enable StarRocks FE TLS (or mTLS) on
port 9030 before enabling `mysql_clear_password` authentication in production.

## Ranger authorization (resource-based)

StarRocks' Ranger plugin ships **built into the FE** (`ranger-plugins-*` jars
under `fe/lib/` in the `allin1-ubuntu` image) — there is no separate jar to
install. It's enabled by dropping the standard Ranger security/audit XML into
the FE's conf dir (which is on the FE's Java classpath) and flipping
`access_control = ranger`. The FE then polls egret's Ranger-Admin-compatible
download REST for the StarRocks service's policies, the same way Trino does.

### Prerequisites

1. egret is started with `ranger.enabled=true` in its config, so the
   `seed_starrocks` startup seed (Task 1 of this plan) registers the
   `starrocks` service-def egret needs to serve StarRocks-shaped policies.
2. A StarRocks policy profile named **`sr1`** exists, created via egret's
   Policies UI (`POST /policies/services`, Task 3/4 of this plan) — the name
   must match `ranger.plugin.starrocks.service.name` in
   `ranger/ranger-starrocks-security.xml` below, because the FE requests
   `<policy.rest.url>/service/plugins/policies/download/sr1`.
3. The FE can authenticate. egret's `/ranger` surface requires an engine
   credential by default (`ranger.auth_mode = required`), and the hand-written
   XML here carries none, so the download is refused with `401` as is. Either
   register this StarRocks as an external engine bound to `sr1` and add
   `ranger.plugin.starrocks.policy.rest.client.username` / `.password` with the
   credential egret shows once (the FE then calls
   `<policy.rest.url>/service/plugins/secure/policies/download/sr1`), or run
   the host egret with `EGRET_RANGER__AUTH_MODE=anonymous`, for local
   development only. See `docs/en/guides/ranger-surface.md`.

### Files

| File | Purpose |
|---|---|
| `ranger/ranger-starrocks-security.xml` | `ranger.plugin.starrocks.*` properties: service name (`sr1`), egret's download REST URL, poll interval, local policy cache dir. Standard Ranger resource-name convention (`ranger-<serviceType>-security.xml`), auto-loaded from the classpath by the bundled `ranger-plugins-common` jar. |
| `ranger/ranger-starrocks-audit.xml` | Turns off audit-to-Ranger (`xasecure.audit.is.enabled=false`) — egret has its own `audit_log` (Effort B / B4) and dev has no separate Ranger Admin UI to receive audit events. |

Both files are mounted read-only into
`/data/deploy/starrocks/fe/conf/` by `docker-compose.dev.yml`. **This path was
verified directly against the real image** (`docker run --rm
starrocks/allin1-ubuntu:latest sh -lc 'find / -name fe.conf'` ->
`/data/deploy/starrocks/fe/conf/fe.conf`; `fe/bin/start_fe.sh` puts
`${STARROCKS_HOME}/conf` — i.e. this same dir — on `CLASSPATH`), so this is not
a guessed path.

### Base URL reasoning

`ranger.plugin.starrocks.policy.rest.url` is set to
`http://host.docker.internal:8080/ranger`. The egret backend is **not containerized**
in this compose (see the `frontend` service comment above: the dev path is
`cargo run -p egret-bin serve` on the host), so the StarRocks container must
reach it via the host gateway, exactly like the existing
`deploy/embedded-catalog/engine-wiring.md` convention for Trino/engine ->
egret catalog reachability. This was **verified live**: with egret running on
the host and a throwaway `allin1-ubuntu` container booted with
`access_control=ranger`, the FE log showed
`RangerAdminRESTClient.getServicePoliciesIfUpdated` hitting egret and getting
back a real (404, since no `sr1` profile existed in that test) JSON body —
proving the container-to-host path resolves and the URL/port are correct.

### Activation: `access_control = ranger`

Mounting the two XML files is not enough on its own — StarRocks' native
(non-Ranger) authorizer is still the default until `access_control` is
flipped, **and this plugin is a boot-time singleton**: `GlobalStateMgr`
constructs the (Ranger or native) access controller exactly once, very early
in `StarRocksFEServer.start()`. That has a consequence the plan going into
this task did not anticipate, caught only by actually driving the real image:

> ⚠️ **`ADMIN SET FRONTEND CONFIG ("access_control"="ranger")` alone does
> nothing.** Verified live, twice (once against a throwaway container, once
> against the compose-managed `egret-dev-starrocks-1`): running it against an
> already-booted FE returns success and flips the in-memory `Config` value
> (`ADMIN SHOW FRONTEND CONFIG LIKE "access_control"` shows `ranger`
> immediately after), but **no `RangerAccessController` / `PolicyRefresher`
> log lines ever appear** — the running FE process keeps using whichever
> authorizer it booted with. And since this mutable config is **not
> persisted to disk**, a restart silently reverts it to `native` rather than
> picking up `ranger`, so restarting right after running the command does not
> help either.

The only procedure that actually activates Ranger (verified end-to-end,
including through the compose-defined mount): put `access_control = ranger`
in `fe/conf/fe.conf` **before** the FE process (re)starts, then restart it —
only then does the boot-time singleton construct `RangerAccessController` and
start polling egret. In dev:

```bash
docker compose exec starrocks sh -c \
  "echo 'access_control = ranger' >> /data/deploy/starrocks/fe/conf/fe.conf"
docker compose restart starrocks
```

This isn't wired into `docker-compose.dev.yml` itself as a `command:`/env
override because the image's `entrypoint.sh` builds up parts of `fe.conf`
itself on each boot (see its `append_if_missing` helper) and there's no clean
compose-level hook to append a line to a file that lives inside the
container's own filesystem before the FE process reads it once at boot; a
`:ro` bind-mounted full replacement `fe.conf` would work but means owning and
keeping in sync the entire default file (ports, JVM opts, etc.) rather than
just the one line, which is more fragile than a documented manual step for a
dev compose. Task 6 (live smoke) drives this same sequence.

> ⚠️ **Re-apply after any container recreation.** The line above is appended
> to the container's own `fe/conf/fe.conf` (not to a host-mounted file), so it
> only survives an in-place FE restart (`docker compose restart starrocks`). A
> **fresh container regenerates `fe.conf` from scratch**, which means the
> activation is **silently lost** on `docker compose down && up`,
> `docker compose up --force-recreate`, `docker rm`, or any stack reset such
> as `just reset-testdata`. After any of those, `access_control` is back to
> `native` — Ranger enforcement is off **with no error or warning** — and you
> must re-run the two commands above. If a query that should be denied/masked
> suddenly returns clear data, check this first.

⚠️ **XML comment gotcha found live**: a literal doubled-hyphen anywhere inside
an XML comment in `ranger-starrocks-security.xml` (e.g. from pasting a `--
flag` example) fails Ranger's Hadoop-`Configuration` XML parser and **crashes
the entire FE boot** with `ExceptionInInitializerError` — not just a plugin
warning. Keep comments in that file free of `--`.

### Confirming the FE is polling egret

Once `access_control=ranger` is active, the FE log
(`/data/deploy/starrocks/fe/log/fe.log` inside the container) shows, on every
boot and again every `pollIntervalMs` (30s):

```text
RangerAccessController.buildRangerPluginContext():59  Interacting with Ranger Admin Server using SIMPLE authentication
...
RangerAdminRESTClient.getServicePoliciesIfUpdated():... service not found / 200 with policies, serviceName=sr1
```

and egret's own server logs show the inbound
`GET /ranger/service/plugins/policies/download/sr1` request (the
`.../plugins/secure/policies/download/sr1` form once the XML carries a
credential). A `401` means the FE presented no valid credential (Prerequisite
3). A `404` after authentication just means no `sr1` profile exists yet in
egret (create one via the Policies UI); once it does, the request returns the
real policy payload. A benign `getRolesIfUpdated ... 404 ... Ignoring` warning
is expected and harmless — egret's Ranger router implements the plain and
secure `/plugins/.../policies/download/{service}` routes and
`/tags/download/{service}` (see `crates/egret-policy/src/ranger/routes.rs`),
not a Ranger role-download endpoint; the StarRocks plugin degrades gracefully
when that 404s.

### Limitation: resource-based only, no tag masking

StarRocks' Ranger plugin supports **resource-based policies only**
(allow/deny on catalog/database/table/column). Tag-based masking and
row-filter policies — which egret's Trino integration already enforces — are
**not supported** by StarRocks' Ranger plugin upstream
(`apache/starrocks#67458` is still open). Don't expect a StarRocks-side
column mask or row filter to do anything even if a matching tag policy exists
for the same data in Trino; only Trino gets tag-based enforcement today.

## Provisioning (Kubernetes operator)

egret provisions StarRocks clusters onto Kubernetes via the upstream
[kube-starrocks operator](https://github.com/StarRocks/starrocks-kubernetes-operator)
and a `StarRocksCluster` custom resource. The operator's install manifests are
vendored **verbatim** (unmodified, straight `cp` from the upstream repo) into
`deploy/starrocks-operator/`:

| File | Purpose |
|---|---|
| `deploy/starrocks-operator/crd.yaml` | The `starrocksclusters.starrocks.com` CustomResourceDefinition. |
| `deploy/starrocks-operator/operator.yaml` | Namespace `starrocks`, ServiceAccount, ClusterRole/ClusterRoleBinding, and the `kube-starrocks-operator` Deployment. |

### Install (one-time)

```bash
kubectl apply -f deploy/starrocks-operator/crd.yaml
kubectl apply -f deploy/starrocks-operator/operator.yaml   # installs into ns `starrocks`
kubectl -n starrocks rollout status deploy/kube-starrocks-operator
```

This is a **one-time, pre-installed** step — the same posture egret already
takes toward Kubernetes operators for its engines (e.g. the Stackable Trino
operator used in earlier Trino provisioning phases): egret does **not**
manage the operator's own lifecycle (install, upgrade, uninstall). The
StarRocks provisioner only assumes the `starrocksclusters.starrocks.com` CRD
is already present in the cluster and creates/deletes `StarRocksCluster`
custom resources against it — it never touches the operator Deployment
itself.

Provisioned clusters run in **FE + CN shared-data mode** (object-storage
backed, no local-disk BE nodes).
