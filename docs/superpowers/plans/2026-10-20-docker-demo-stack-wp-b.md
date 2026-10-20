# Docker Demo Stack, Work Package B (Data Apps without Kubernetes) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In the compose demo stack (`just demo-up`), Data Apps work: create an app, edit it in the marimo editor, publish, and open the published version as a viewer, with no Kubernetes.

**Architecture:** The server already has `apps.pod.launcher = "subprocess"` (`crates/egret-apps/src/subprocess_pod.rs`): one marimo process per author or viewer, started inside the egret container. So the egret container needs Python and the `apps-image` payload next to the Rust binary (a new `demo` stage in `deploy/docker/backend.Dockerfile`), the apps host listener enabled (`apps.origin`, `apps.listen`), and the edge nginx must route a second hostname, `apps.localtest.me`, to that listener. No Rust changes are expected; if the launcher does not work inside a container, stop and report (Task 3 is the proof).

**Tech Stack:** Docker multi-stage build (`python:3.12-slim` runtime for the demo stage), marimo 0.25.0 from `apps-image/requirements.txt` + `constraints.txt`, nginx envsubst templates, Docker Compose.

Spec: `docs/superpowers/specs/2026-10-20-docker-demo-stack-design.md` (WP-B). Builds on WP-A (merged as #704): `deploy/docker-compose.demo.yml`, `deploy/demo/*`, `docs/en/guides/demo-stack.md`.

## Global Constraints

- Do not change Rust or frontend source. Only `deploy/`, `justfile`, `docs/`, `README.md`, `.env.example` (if needed).
- The default (production) image from `deploy/docker/backend.Dockerfile` must be byte-for-byte the same stages as today: add the demo stage BEFORE the final runtime stage so `docker build` without `--target` still yields the production image. `images-build` and the Helm chart keep working unchanged.
- Pin: `python:3.12-slim` is what `apps-image/Dockerfile` already uses (same major as the pod image); copy the `egret` binary from the existing `builder` stage (built on `rust:bookworm`, glibc-compatible with Debian bookworm `python:3.12-slim`).
- The apps origin must be a **sibling hostname**, never another port on the egret host (cookie isolation; `crates/egret-config/src/lib.rs` `AppsConfig.origin` doc). Use `apps.localtest.me` next to `egret.localtest.me`. Both resolve to 127.0.0.1 through public DNS; offline machines add both to `/etc/hosts`.
- Published ports stay loopback-only and unchanged: the edge port only. The apps host listener (`apps.listen`) is NOT published; only the edge reaches it.
- The subprocess launcher is documented as development-only (shared uid, no isolation, sets `PYTEST_CURRENT_TEST` for the pod). The guide's Safety section must say so.
- Demo secrets stay public development values; no new secrets.
- Testing: this machine has port 8080 occupied by an unrelated service. Run every verification with `DEMO_PORT=18080 DEMO_URL=http://egret.localtest.me:18080` exported (shell variables override `--env-file`; never commit those). Never touch containers `egret-dev-*`, `egretdocs-*`; only the new and existing `demo-*` recipes; never `just up/down/reset-testdata`. `exec -T` for `docker compose exec`. Leave the stack DOWN (`just demo-reset`) after each task.
- Browser checks: Playwright as a Node library from `/Users/1pro-l4r8/Documents/github/data-portal/docs/node_modules` (`createRequire`); do NOT use the Playwright MCP tools. Login is rate limited (5 per 5 minutes per ip:email): log in once per identity and reuse the storage state; the login form uses placeholders, not labels.
- Conventional Commits; commit messages end with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- Config values already known (verbatim from the code): env prefix `EGRET_`, section separator `__`; list values are written as a JSON array in the env var, e.g. `EGRET_APPS__POD__COMMAND='["python","-I","-m","egret_marimo.launcher"]'`; `apps.pod.launcher = "subprocess"` requires `command`, `config_dir`, `venv_shim_dir`, `callback_url`, `apps.origin`, `apps.egret_origin`, `idle_secs >= 60`.

## File Structure

| File | Responsibility |
|---|---|
| `deploy/docker/backend.Dockerfile` | Add a `demo` stage: egret binary + Python + apps payload at `/opt/egret-apps` |
| `deploy/docker-compose.demo.yml` | `egret` builds target `demo`; apps env; `frontend` gets `APPS_ORIGIN` |
| `deploy/demo/demo.env` | `DEMO_APPS_HOST`, `DEMO_APPS_URL` |
| `deploy/demo/edge.conf.template` | Second server block for the apps hostname with WebSocket support |
| `deploy/demo/wait.sh` | Print the apps URL; fail fast when `DEMO_APPS_URL` disagrees with host and port |
| `deploy/demo/verify-apps.mjs` | Repeatable browser check of the app lifecycle (kept in the repo, not scratch) |
| `docs/en/guides/demo-stack.md` | Apps now in; Safety note; apps hostname/offline DNS |

---

### Task 1: The `demo` image stage

**Files:**
- Modify: `deploy/docker/backend.Dockerfile` (insert a stage between `builder` and the final runtime stage)

**Interfaces:**
- Produces: Dockerfile stage `demo` (`docker build --target demo`). In it: `/usr/local/bin/egret`, Python 3.12 with marimo and the `egret_marimo` package installed, the locked config at `/opt/egret-apps/config`, the venv shim at `/opt/egret-apps/venv-shim`, a writable `/var/lib/egret-apps` for pod work dirs, running as uid 10001, `ENTRYPOINT ["egret"]`, `CMD ["serve"]`.

- [ ] **Step 1: Read what the pod image does so the stage reproduces it**

Read `apps-image/Dockerfile` (already in the repo). The demo stage must do the same installs, in the same order: `pip install -r requirements.txt -c constraints.txt`, then `pip install --no-deps .` from a separate directory (a `pyproject.toml` in `/work`, a parent of it or `$HOME` makes the launcher refuse to start), then place `venv-shim` and `config` under `/opt/egret-apps`, then `chmod +x venv-shim/bin/python`, then the version assertions (`marimo.__version__` equals `MARIMO_VERSION`; `egret_marimo` imports from site-packages).

- [ ] **Step 2: Insert the stage**

Insert after the builder stage's last line (`cp target/release/egret /usr/local/bin/egret`) and before `# ---- Stage 2: slim runtime ----`:

```dockerfile
# ---- Demo runtime: egret + the Data Apps payload, for `apps.pod.launcher = "subprocess"` ----
# DEMO ONLY. The subprocess launcher runs marimo as the same uid as the server and gives no
# isolation (crates/egret-apps/src/subprocess_pod.rs). Production uses the Kubernetes launcher and
# the separate egret-apps pod image; the default (last) stage below is unchanged.
FROM python:3.12-slim AS demo
ARG MARIMO_VERSION=0.25.0
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates tini \
 && rm -rf /var/lib/apt/lists/* \
 && useradd -r -u 10001 egret
WORKDIR /opt/egret-apps/src
COPY apps-image/requirements.txt apps-image/constraints.txt ./
RUN pip install --no-cache-dir -r requirements.txt -c constraints.txt
COPY apps-image/pyproject.toml ./
COPY apps-image/egret_marimo ./egret_marimo
RUN pip install --no-cache-dir --no-deps . \
 && cd / && rm -rf /opt/egret-apps/src
WORKDIR /opt/egret-apps
COPY apps-image/venv-shim ./venv-shim
COPY apps-image/config ./config
RUN chmod +x venv-shim/bin/python \
 && mkdir -p /var/lib/egret-apps && chown 10001:10001 /var/lib/egret-apps \
 && python -c "import marimo,sys; sys.exit(marimo.__version__ != '${MARIMO_VERSION}')" \
 && cd /tmp && python -I -c "import egret_marimo,sys; sys.exit('site-packages' not in egret_marimo.__file__)"
ENV MARIMO_SKIP_UPDATE_CHECK=1 \
    PYTHONNOUSERSITE=1 \
    EGRET_REAL_PYTHON=/usr/local/bin/python \
    EGRET_MARIMO_VERSION=${MARIMO_VERSION} \
    EGRET_VENV_SHIM=/opt/egret-apps/venv-shim \
    XDG_CONFIG_HOME=/opt/egret-apps/config
COPY --from=builder /usr/local/bin/egret /usr/local/bin/egret
WORKDIR /
USER 10001
EXPOSE 8080
ENTRYPOINT ["egret"]
CMD ["serve"]

```

(If the `COPY apps-image/...` paths fail because the Docker build context is the repository root, they are right: `backend.Dockerfile` is built with `context: ..` from `deploy/`, i.e. the repo root. Confirm with the compose file's `build.context`.)

- [ ] **Step 3: Build the demo stage and check the production image is untouched**

Run (background and poll; the Rust compile is cached from WP-A; marimo's pip install takes a few minutes):

```bash
docker build -f deploy/docker/backend.Dockerfile --target demo -t egret-demo-backend-apps:test .
docker run --rm --entrypoint python egret-demo-backend-apps:test -c "import marimo; print(marimo.__version__)"
docker run --rm --entrypoint id egret-demo-backend-apps:test
docker run --rm egret-demo-backend-apps:test --version
```

Expected: `0.25.0`; `uid=10001(egret)`; an `egret` version line. Then confirm the default image is the same final stage as before: `docker build -f deploy/docker/backend.Dockerfile -t egret-backend-prod-check:test .` succeeds and `docker run --rm --entrypoint sh egret-backend-prod-check:test -c 'command -v python3 || echo no-python'` prints `no-python`. Remove the two `:test` tags afterwards.

- [ ] **Step 4: Commit**

```bash
git add deploy/docker/backend.Dockerfile
git commit -m "feat(deploy): demo image stage with the Data Apps payload for the subprocess launcher"
```

---

### Task 2: Wire the apps host: compose, edge, frontend CSP

**Files:**
- Modify: `deploy/demo/demo.env`
- Modify: `deploy/docker-compose.demo.yml` (`egret` service: build target and apps env; `frontend` service: `APPS_ORIGIN`; `edge` service: pass the apps host variables)
- Modify: `deploy/demo/edge.conf.template`
- Modify: `deploy/demo/wait.sh`

**Interfaces:**
- Consumes: Task 1 stage `demo`.
- Produces: apps origin `http://apps.localtest.me:${DEMO_PORT}` reachable through the edge; env keys on `egret`: `EGRET_APPS__ORIGIN`, `EGRET_APPS__EGRET_ORIGIN`, `EGRET_APPS__LISTEN=0.0.0.0:8081`, `EGRET_APPS__POD__*`.

- [ ] **Step 1: Add the hostname variables to `deploy/demo/demo.env`**

Append:

```bash
# Data Apps are served from a SIBLING hostname of the app (cookie isolation), never another port.
DEMO_APPS_HOST=apps.localtest.me
DEMO_APPS_URL=http://apps.localtest.me:8080
```

`DEMO_APPS_URL` must equal `http://${DEMO_APPS_HOST}:${DEMO_PORT}`, the same rule as `DEMO_URL`.

- [ ] **Step 2: Build the demo target and set the apps keys on `egret`**

In `deploy/docker-compose.demo.yml`, change the `egret` service `build:` block to:

```yaml
    build:
      context: ..
      dockerfile: deploy/docker/backend.Dockerfile
      target: demo
```

and append these keys to its `environment:` block:

```yaml
      # Data Apps (subprocess launcher, DEMO ONLY: no isolation between pods, see the guide).
      EGRET_APPS__ORIGIN: ${DEMO_APPS_URL:-http://apps.localtest.me:8080}
      EGRET_APPS__EGRET_ORIGIN: ${DEMO_URL:-http://egret.localtest.me:8080}
      EGRET_APPS__LISTEN: 0.0.0.0:8081
      EGRET_APPS__POD__LAUNCHER: subprocess
      EGRET_APPS__POD__COMMAND: '["python","-I","-m","egret_marimo.launcher"]'
      EGRET_APPS__POD__WORK_DIR: /var/lib/egret-apps
      EGRET_APPS__POD__CONFIG_DIR: /opt/egret-apps/config
      EGRET_APPS__POD__VENV_SHIM_DIR: /opt/egret-apps/venv-shim
      # Pods run in this same container, so they call egret on loopback.
      EGRET_APPS__POD__CALLBACK_URL: http://127.0.0.1:8080
```

Add a named volume so pod work dirs survive neither restarts nor fill the image layer: `volumes: - demo-appswork:/var/lib/egret-apps` on `egret`, and `demo-appswork:` under the top-level `volumes:`. The volume must be writable by uid 10001; Docker initialises a named volume from the image directory's ownership (set in Task 1), so no extra step is needed. (If it comes up root-owned, fix the image, not the compose file.)

- [ ] **Step 3: Give the frontend its CSP frame source**

The frontend image's nginx template substitutes `APPS_ORIGIN` into the CSP `frame-src` (see the comment in `frontend/Dockerfile`: "`APPS_ORIGIN` is the origin of the Data Apps iframe"). Add to the `frontend` service:

```yaml
    environment:
      APPS_ORIGIN: ${DEMO_APPS_URL:-http://apps.localtest.me:8080}
```

- [ ] **Step 4: Pass the apps variables to the edge and route the second hostname**

In the `edge` service `environment:` block add (interpolated, same reason as `DEMO_PORT`):

```yaml
      DEMO_HOST: ${DEMO_HOST:-egret.localtest.me}
      DEMO_APPS_HOST: ${DEMO_APPS_HOST:-apps.localtest.me}
```

Replace `server_name _;` in the existing server block with `server_name ${DEMO_HOST} _;` and append this second server block to `deploy/demo/edge.conf.template` (WebSocket upgrade is required by marimo):

```nginx
# The Data Apps host: a sibling hostname of the app, served by egret's separate apps listener.
server {
    listen ${DEMO_PORT};
    server_name ${DEMO_APPS_HOST};
    client_max_body_size 64m;

    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection $connection_upgrade;
    proxy_set_header Host $host:$server_port;
    proxy_set_header X-Forwarded-Host $host;
    proxy_set_header X-Forwarded-Port $server_port;
    proxy_set_header X-Forwarded-Proto http;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Forwarded-For $remote_addr;
    proxy_buffering off;
    proxy_read_timeout 3600s;

    # Resolved per request: egret is not up yet when the edge starts.
    location / {
        resolver 127.0.0.11 valid=5s;
        set $apps_upstream http://egret:8081;
        proxy_pass $apps_upstream;
    }
}
```

The `NGINX_ENVSUBST_FILTER` is `^DEMO_`, so `${DEMO_HOST}` and `${DEMO_APPS_HOST}` substitute; nginx's own `$host` etc. stay.

- [ ] **Step 5: `wait.sh` prints the apps URL and checks consistency**

In `deploy/demo/wait.sh`: save and restore `DEMO_APPS_URL` the way `DEMO_URL` is handled; extend the existing mismatch check so it also fails when `DEMO_APPS_URL` is set and differs from `http://${DEMO_APPS_HOST}:${DEMO_PORT}`; add one line to the printed block: `Data Apps  $DEMO_APPS_URL   (opened from inside Egret; needs *.localtest.me DNS or /etc/hosts entries for egret.localtest.me and apps.localtest.me)`.

- [ ] **Step 6: Verify the stack starts with the apps host enabled**

Run with the testing exports (see Global Constraints), adding `DEMO_APPS_URL=http://apps.localtest.me:18080`:

```bash
docker compose --env-file deploy/demo/demo.env -f deploy/docker-compose.demo.yml config -q
docker compose --env-file deploy/demo/demo.env -f deploy/docker-compose.demo.yml up -d --build
```

Poll until `egret` is healthy (the build of the demo stage is new; run it in the background). Then:

```bash
docker compose --env-file deploy/demo/demo.env -f deploy/docker-compose.demo.yml logs egret | grep -E "apps host listening|app pod launcher"
curl -s -o /dev/null -w '%{http_code}\n' -H 'Host: apps.localtest.me:18080' http://127.0.0.1:18080/
curl -s -o /dev/null -w '%{http_code}\n' http://egret.localtest.me:18080/api/auth/config
```

Expected: log lines `apps host listening` with the origin and `app pod launcher` with mode `subprocess`; the apps host answers with an HTTP status from egret's apps proxy (any 4xx such as 401/403/404 proves it reached egret, not 502 from the edge); the app host still returns 200.

- [ ] **Step 7: Commit**

```bash
git add deploy/demo/demo.env deploy/docker-compose.demo.yml deploy/demo/edge.conf.template deploy/demo/wait.sh
git commit -m "feat(deploy): demo apps host, edge route and subprocess launcher wiring"
```

---

### Task 3: Prove the app lifecycle in a browser

This is the spike the spec calls out: it proves the subprocess launcher works inside the container. If it does not, stop with status BLOCKED and report the log lines; do not patch Rust.

**Files:**
- Create: `deploy/demo/verify-apps.mjs`

**Interfaces:**
- Consumes: Task 2 stack at `${DEMO_URL}`; admin `admin@egret.local` / `Admin12345!`.
- Produces: a repeatable Node script `node deploy/demo/verify-apps.mjs` that exits 0 only when the app lifecycle works; it prints one line per step.

- [ ] **Step 1: Discover the real UI and API surface (do not guess)**

With the stack up, read `frontend/src/routes` for the Apps pages (list, create dialog, editor, viewer) and `openapi.json` for `/api/ws/{ws}/apps` operations (create, draft, publish, versions, ticket). Write down, in the report, the exact selectors and endpoints you will use. The editor and viewer are served inside an iframe from the apps origin; the script has to switch into that frame to see marimo.

- [ ] **Step 2: Write `deploy/demo/verify-apps.mjs`**

The script, using Playwright (the repo has no Playwright dependency: load it from `process.env.PLAYWRIGHT_MODULE_DIR`, default `/Users/1pro-l4r8/Documents/github/data-portal/docs/node_modules`, via `createRequire`; fail with a clear message if missing):

1. Read `BASE` (default `http://egret.localtest.me:${DEMO_PORT:-8080}`) and `STATE` (a Playwright storage-state file path; if the file does not exist, log in once with the local admin through the break-glass form using the placeholders, then save the state to that path).
2. Create an app named `Demo app <timestamp>` in workspace `default` through the **UI** (Explore, Apps, New app), asserting it appears in the list.
3. Open its editor; wait for the marimo iframe (hosted on `DEMO_APPS_URL`); assert the editor shows a notebook (a cell is present). Edit: add a cell that computes `1 + 1` and run it; assert the output `2` appears in the frame.
4. Publish the draft from the UI; assert a version `v1` is listed.
5. Open the published app as a viewer (the run view); assert the frame loads and shows the output.
6. Print `OK <step>` per step; on failure print the step, a screenshot path (`/private/tmp/.../verify-apps-<step>.png`, use `process.env.SHOT_DIR`) and exit 1.

Write real selectors from Step 1, not placeholders. Keep the script under 150 lines.

- [ ] **Step 3: Run it**

```bash
STATE=$SCRATCH/apps-state.json SHOT_DIR=$SCRATCH DEMO_PORT=18080 node deploy/demo/verify-apps.mjs
```

Expected: every step prints `OK` and the exit code is 0. If marimo starts but the editor does not load, collect: `docker compose ... logs egret | tail -80`, the pod log files under `/var/lib/egret-apps/*/pod.log` (`docker compose ... exec -T egret sh -c 'tail -40 /var/lib/egret-apps/*/pod.log'`), and the browser console. Typical causes to check before giving up: the pod's `HOME`/work dir permissions, `EGRET_APPS_CONFIG` under `/opt/egret-apps`, the cookie and CSP of the apps origin (browser console), and the edge not forwarding the WebSocket upgrade. Fix in `deploy/` where the cause is configuration; stop with BLOCKED when it needs a Rust change.

- [ ] **Step 4: Per-viewer isolation spot check**

Using the API (admin session), request the viewer ticket for the published version as the admin and confirm two different viewers get different pod refs: sign in once as `member-sso` / `Member12345!` through SSO, have the admin add `member-sso` to workspace `default` as a viewer, then open the same published app as `member-sso` and assert (through `docker compose exec -T egret ls /var/lib/egret-apps`) that a second pod directory exists for the second viewer. Record the result; if the add-member step has no practical UI path, say so rather than skip silently.

- [ ] **Step 5: Commit**

```bash
git add deploy/demo/verify-apps.mjs
git commit -m "test(deploy): browser check of the Data Apps lifecycle in the demo stack"
```

---

### Task 4: Guide, README and the final acceptance

**Files:**
- Modify: `docs/en/guides/demo-stack.md`
- Modify: `justfile` (one recipe)

- [ ] **Step 1: Add `just demo-verify-apps`**

After the `demo-logs` recipe in `justfile`:

```just
# Browser check of the Data Apps lifecycle (needs Node and Playwright; see deploy/demo/verify-apps.mjs)
demo-verify-apps:
    node deploy/demo/verify-apps.mjs
```

- [ ] **Step 2: Update the guide**

In `docs/en/guides/demo-stack.md`: in "What you get" add a Data Apps row (host `apps.localtest.me`, same port); remove Data Apps from "What is not in this stack yet" (engine provisioning stays there; the sentence that logs "kubernetes unavailable" stays); in Requirements add that `apps.localtest.me` must resolve like `egret.localtest.me` (offline: add both names to `/etc/hosts`) and the first app start takes up to a minute (pip-installed marimo cold start; measure it in Task 3 and write the number); in "Change the port" add `DEMO_APPS_URL` to the pair of values that must change together; in Safety add: the apps run in the egret container as the same user with no isolation between apps and users (the subprocess launcher is development-only; production uses the Kubernetes launcher), so never put real data or untrusted authors in the demo. Troubleshooting: blank editor, check the browser console for CSP or cookie errors, `just demo-logs egret`, and the pod logs at `/var/lib/egret-apps/*/pod.log`.

- [ ] **Step 3: Acceptance from a clean state**

Run `just demo-reset && just demo-up` (exports as in Global Constraints, plus `DEMO_APPS_URL`), then:

- [ ] `just demo-verify-apps` exits 0 (all steps `OK`).
- [ ] `docker compose ... ps` shows no unhealthy service; the egret log contains `app pod launcher` with `subprocess`.
- [ ] The Dockerfile default stage check from Task 1 still holds (production image has no Python).
- [ ] `just demo-down` stops pods with the container (no stray marimo processes: `docker top` is gone with the container); `just demo-up` again keeps apps data; `just demo-reset` removes `egret-demo_demo-appswork`.
- [ ] `gitleaks detect --no-git -c .gitleaks.toml` reports nothing new outside `.superpowers/`.
- [ ] Time to first editor load (cold) and warm are recorded in the guide.

- [ ] **Step 4: Commit**

```bash
git add docs/en/guides/demo-stack.md justfile
git commit -m "docs: Data Apps in the demo stack; just demo-verify-apps"
```

---

## Self-review against the spec (WP-B)

| Spec requirement | Task |
|---|---|
| `demo` target in `backend.Dockerfile` with Python, tini and the `apps-image` payload (marimo pinned, `egret_marimo`, `venv-shim`, `config`) | 1 |
| `launcher=subprocess`, `command`, `work_dir`, `config_dir`, `venv_shim_dir`, `callback_url`, `apps.origin`, listener | 2 |
| Second origin: a different hostname on the same machine (`apps.localtest.me`), not a path or port | 2 (edge server block, env) |
| Verify first that the launcher runs unchanged in a container; report before WP-C | 3 (spike, BLOCKED rule) |
| Test: create, edit, run, publish, open as another user through the apps origin, per-viewer isolation in a browser | 3 |
| Docs: guide updated, safety caveat for the dev-only launcher | 4 |

Differences from the spec text: the spec says "a second published port for the apps origin"; the apps host is routed by the edge on the **same** port by hostname, so no new published port exists (loopback-only stays intact). Tini is installed in the demo image but not used as PID 1 (the egret entrypoint script is); it is available for the pod process tree if Task 3 shows zombie kernels, and can be dropped if unused.
