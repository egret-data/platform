# Docker demo stack: every feature on one `docker compose`

Date: 2026-10-20 · Status: approved by owner (chat), not yet implemented

## Goal

One command (`just demo-up`) starts a complete Egret on Docker, and **every feature the docs describe runs**:
Data Apps, *Add engine* (engine provisioning), PNG/PDF reports, email alerts, SSO, a Trino with sample data,
seeded demo content. Two uses:

1. The docs site (`egret-data/docs`) is written and screenshotted against it, so no page has to say
   "not run locally".
2. People trying Egret get a working instance without Kubernetes.

**Not a production topology.** The Docker engine provisioner needs the Docker socket; the docs say so.

## Why this is needed (measured, October 2026 docs run)

| Gap | Today | Known issue |
|---|---|---|
| Clean `cp .env.example .env; just up; just serve` cannot start | `.env.example` lacks `EGRET_STORAGE__{BUCKET,ACCESS_KEY,SECRET_KEY}`; `Config::validate` requires them | KI-136 |
| Nothing creates the object-store bucket | the Helm chart has a bucket Job; compose has none | KI-137 |
| Compose Trino has no catalogs and enforces JWT over TLS | no first-dashboard path | KI-138 |
| Apps | `apps.pod.launcher = "none"` by default; Kubernetes otherwise | KI-040 |
| *Add engine* | provisioner emits Stackable/StarRocks operator resources, needs Kubernetes | KI-022 |
| PNG/PDF reports | no `egret-renderer` service in compose | |
| Email | no SMTP server | |
| The backend is not in compose at all | "the Egret backend is not containerized in this compose yet" | |

(The KI numbers are in `egret-data/docs` → Known issues; renumber if they moved.)

## Scope

Three work packages, one PR each, in order. Trino is the only engine the Docker provisioner supports at first;
StarRocks is a follow-up and the UI must say "Trino only" for the Docker provisioner until then.

### WP-A: Demo compose stack (deploy only, no product code)

New `deploy/docker-compose.demo.yml` (the existing `docker-compose.dev.yml` stays as the contributor stack).

| Service | Source | Notes |
|---|---|---|
| `postgres`, `rustfs`, `keycloak` | as in dev compose | Keycloak imports `deploy/keycloak/egret-realm.json` |
| `rustfs-init` | one-shot, mirrors the Helm `deps-rustfs-bucket-job` | creates the bucket, then exits |
| `mailpit` | `axllent/mailpit`, pinned | SMTP :1025, UI :8025; the workspace SMTP settings are pre-seeded to it |
| `renderer` | the image the chart uses for `egret-renderer` | `render.url` points at it |
| `trino-demo` | `trinodb/trino`, pinned | plain HTTP, `http-server.process-forwarded=true`, catalogs `tpch` and `memory`; registered as a database by the seed |
| `egret` | `deploy/docker/backend.Dockerfile` (+ apps runtime, WP-B) | `migrate` then `init` then `serve` via an entrypoint script; waits for `postgres` and `rustfs-init` |
| `frontend` | existing nginx build, proxying `/api` to `egret` | the only port a user opens (8080) |

Also in this package:

- `.env.example` gains the three storage variables, matching the demo compose (fixes KI-136).
- `just demo-up`, `just demo-down` (stops containers **and** removes provisioned engine containers, keeps data
  volumes), `just demo-reset` (also deletes volumes), `just demo-seed` (idempotent; reuses `deploy/seed-*.sh`).
- A readiness script `deploy/demo-wait.sh` prints the URL and the admin credentials once `/readyz` is green.
- Secrets in the demo are fixed, documented, public development values (same policy as `.env.example`); the compose
  file carries a banner comment and the docs say never to expose it.

### WP-B: Data Apps without Kubernetes

The server already has `apps.pod.launcher = "subprocess"` (one marimo process per author on the host, spec in
`crates/egret-apps/src/subprocess_pod.rs`). Plan:

- A `demo` target in `deploy/docker/backend.Dockerfile` that adds Python, tini and the `apps-image` payload
  (marimo pinned to `apps-image/constraints.txt`, `egret_marimo`, `venv-shim`, `config`) next to the `egret` binary.
- `EGRET_APPS__POD__LAUNCHER=subprocess`, `command`, `work_dir`, `config_dir`, `venv_shim_dir`, `callback_url`
  (`http://egret:8080`, the compose service name), `EGRET_APPS__ORIGIN`, `EGRET_APPS__EGRET_ORIGIN`, `listen`.
- A second published port for the apps origin. The apps origin must be a **different origin** from the app (cookie
  and `frame-ancestors` isolation): use a second hostname on the same machine (`apps.localtest.me` resolves to
  127.0.0.1), not a path.
- **To verify first:** that the subprocess launcher runs unchanged inside a container (it was written for a host
  checkout). If it needs code changes they stay inside `egret-apps` and are covered by its existing tests. A failure
  here is reported before WP-C starts.

### WP-C: `DockerEngineProvisioner`

A new implementation of `egret_engine_mgmt_api::EngineProvisioningPort` (provision, status, update, delete, restart),
next to the Kubernetes one wired in `crates/egret-bin/src/main.rs` (`EgretEngineProvisioningPort`).

**Selection.** New config `[engines] provisioner = "kubernetes" | "docker" | "none"`
(`EGRET_ENGINES__PROVISIONER`). Default keeps today's behaviour (try Kubernetes, else disabled). `docker` is never
auto-selected.

**Per engine** (Trino first), keyed by `EngineSpec.name`:

1. Render the engine's config directory from the same `EngineSpec` the Kubernetes path uses: `config.properties`,
   `jvm.config`, one `catalog/<name>.properties` per `CatalogSpec`, the Ranger plugin files
   (`ranger_plugin_files`, already kube-free), the JWT/auth settings. Reuse the existing renderers; do not fork them.
   Files go to `<engines.docker.work_dir>/<name>/` and are bind-mounted read-only.
2. Create and start one container through the Docker Engine API (crate `bollard`) on the compose network, image
   `trinodb/trino:<image_version>`, labels `egret.engine=<name>`, `egret.managed=true`, CPU/memory limits from the
   spec, `no-new-privileges`, all capabilities dropped, no published ports (egret reaches it by container name).
3. `status`: inspect the container, map `created|restarting` → `provisioning`, `running` + health → `running`,
   `exited|dead` → `failed` with the last log lines as `message`. Same `EngineStatus` shape as today.
4. `update`: re-render, recreate the container. `restart`: Docker restart. `delete`: stop, remove, delete the config
   directory. Everything idempotent; unknown containers (no `egret.managed` label) are never touched.
5. Startup reconcile: containers labelled `egret.managed=true` with no engine row are reported, not removed
   (`demo-down` removes them by label).

**What else uses `kube` today and must also work on Docker.** The Control Plane engine monitor
(`crates/egret-engine-mgmt/src/monitor.rs`: query list, query detail, kill, cluster view) reaches the coordinator
through `kube.coordinator_request`. Add a small `CoordinatorTransport` seam: Kubernetes keeps its API-server proxy,
Docker uses plain HTTP to `http://<container>:8080`. Ingress features (`ingress.rs`) are Kubernetes-only and are
hidden or disabled under the Docker provisioner with a clear message.

**Addresses.** Engine containers must reach egret (Ranger policy download, JWT issuer/JWKS, audit) by the compose
service name, not `localhost`. `catalog.public_url` stays the browser-facing URL, so the Docker provisioner takes a
separate `engines.docker.callback_url` (default `http://egret:8080`) and renders it wherever `public_url` is used
for engine-side callbacks (compare KI-141 and KI-149).

**Security.**

- Mounting `/var/run/docker.sock` into `egret` is root-equivalent on the host. Therefore: off by default, enabled
  only by the demo compose, and the compose file puts a `docker-socket-proxy` in front that allows only container
  create/start/stop/inspect/delete/logs, networks and images-pull; `egret` talks to the proxy, not the socket.
- A provisioned container can mount only its own config directory, never a host path from user input, and image names
  are restricted to `engines.docker.allowed_images` (default `trinodb/trino`).
- The docs page and the compose banner state the risk.

### Out of scope

- StarRocks and RisingWave on Docker (follow-up).
- Docker as a production engine target, multi-host, Swarm.
- Replacing the Kubernetes path; it is unchanged.
- Fixing unrelated known issues (only those listed under "Why this is needed").

## Testing

| Package | Test |
|---|---|
| A | Fresh clone → `just demo-up` → `/readyz` green, login works, SSO login works, seed idempotent (run twice), `demo-down` leaves no managed containers. |
| B | Create an app, edit, run, publish, open as another user through the apps origin; verify per-viewer isolation in a browser. |
| C | Unit: config rendering from a golden `EngineSpec` equals the Kubernetes renderer's inputs; status mapping table; allowed-image rejection. Integration (needs Docker, gated like other DB tests with a visible skip message): provision → running → query → update → delete. Browser: full *Add engine* wizard, then SQL Lab query through the new engine, then Control Plane monitor shows the query. |

Each package's PR follows the repo rules: docs in the same PR (CONTRIBUTING), no committed secrets, Conventional
Commits.

## Docs follow-up (after merge)

In `egret-data/docs`: a "Run the demo stack" page replacing the Quick start's source-build path as the primary
route; re-shoot the pages that currently say "not run locally" (Apps, Engines provisioning and Migrate, Alerts
reports and email, Notifications); close the matching known issues.

## Risks

| Risk | Mitigation |
|---|---|
| Subprocess launcher assumes a host checkout | verify first (WP-B step 1); fall back to a sidecar `apps` container that the launcher reaches by name if needed |
| Provisioned Trino cannot reach egret / JWT issuer mismatch | `engines.docker.callback_url`, covered by the integration test |
| Docker socket exposure | socket proxy, off by default, allow-listed images |
| Demo needs ~6 to 8 GB RAM | state it in the docs; Trino heap and resource limits are set small in the demo |
| Pinned images disappear (MinIO precedent) | pin by digest where the dev compose does; no `latest` |

## Notes from WP-A for WP-C

- WP-A sets `EGRET_CATALOG__PUBLIC_URL=http://egret:8080` (the service name) because only containers use it today. WP-C must keep a browser-facing value where one is needed and add `engines.docker.callback_url`, as described above, rather than reuse this key for engine callbacks.
- `just demo-down` and `demo-reset` remove containers by the label `egret.managed=true`, host-wide. WP-C should also label provisioned containers with the compose project (for example `egret.project=egret-demo`) and have the recipes filter on both.
- Files differ from the names given above: `deploy/demo/wait.sh` (not `deploy/demo-wait.sh`) and a new `deploy/demo/seed.sh` (the older `deploy/seed-*.sh` scripts need a dev checkout and `psql`).
- Ports bind to `127.0.0.1` only; WP-C must not publish engine ports.
