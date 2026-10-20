# Run the demo stack on Docker

> **Demo only. Never expose to a network.** This stack uses fixed, public development secrets and credentials. It is not a production topology.

One command starts the whole platform on Docker, with no Kubernetes: Egret, Postgres, an S3-compatible object store, Keycloak for SSO, a Trino engine, Data Apps (marimo), a mail catcher, a PDF/PNG renderer, and a sample workspace with a dataset, three charts and a dashboard.

```bash
just demo-up
```

## What you get

| Service | Address | Credentials |
|---|---|---|
| Egret (UI and API) | `http://egret.localtest.me:8080` | `admin@egret.local` / `Admin12345!` (use "Admin sign-in (break-glass)" on the login page) |
| SSO users (Keycloak, behind the same address under `/auth`) | "Sign in with SSO" button | `admin-sso` / `Admin12345!`, `member-sso` / `Member12345!` |
| Data Apps (marimo editor and published apps, opened from the UI) | `http://apps.localtest.me:8080` (a sibling hostname behind the same edge port; you do not open it directly) | same login as Egret |
| Mailpit (every mail the demo sends) | `http://localhost:8025` | none |
| Trino | internal only, registered in workspace `default` as database `trino-demo` | catalogs `tpch` and `memory` |

Behind the one port, an edge nginx serves the frontend and Keycloak (`/auth`) on `egret.localtest.me`, and the Data Apps on `apps.localtest.me`, so the browser and the backend see the same OIDC issuer. Postgres, the object store (bucket `warehouse`), the renderer and Trino are not published to the host. The two published ports (the edge and Mailpit) bind to `127.0.0.1` only. On a remote VM, reach them through an SSH tunnel, for example `ssh -L 8080:localhost:8080 -L 8025:localhost:8025 user@vm`; the browser address stays `http://egret.localtest.me:8080`, which resolves to `127.0.0.1` on your own machine.

The seed creates, in workspace `default`: database `trino-demo`, dataset `orders` (TPC-H `tpch.tiny.orders`) with metrics `revenue` and `order_count`, charts "Revenue by order status", "Revenue by month" and "Orders by priority", the dashboard "Sales overview", and a workspace SMTP setting that points at Mailpit.

## Requirements

- Docker with Compose v2, [`just`](https://github.com/casey/just), `curl` and `python3` (the seed and readiness scripts use them).
- About 6 to 8 GB of RAM free for Docker.
- The first `just demo-up` builds the backend (a Rust compile) and takes 20 to 40 minutes. The first build also pip-installs the Data Apps payload (marimo, duckdb, polars, pyarrow, plotly and more; the backend image is about 1.5 GB). Later runs reuse the cached images.
- `*.localtest.me` resolves to `127.0.0.1` through public DNS. On an offline machine add this line to `/etc/hosts`:

  ```text
  127.0.0.1 egret.localtest.me apps.localtest.me
  ```

  `apps.localtest.me` must resolve like `egret.localtest.me`: Data Apps run on that sibling hostname so the apps never share cookies with Egret.
- The first Data App start launches a marimo process inside the Egret container (marimo is pip-installed in the demo image). Measured on the development machine: the first editor load after Egret started took 7.4 s, the next one 1.1 s; a viewer's first load of a published app took 4.8 s. Budget up to a minute on a slower machine.

## Run it

```bash
just demo-up
```

The command builds and starts the services, waits until Egret answers, prints the address and logins, then seeds the sample content. Open `http://egret.localtest.me:8080`.

- **Local admin**: on the login page choose "Admin sign-in (break-glass)" and use `admin@egret.local` / `Admin12345!`. This account sees the `default` workspace and everything seeded.
- **SSO users**: the "Sign in with SSO" button signs `admin-sso` or `member-sso` in through Keycloak. They have **no workspace** until an admin adds them ("No workspaces yet" is the expected first screen), and egret only knows them after their first sign-in. Sign in once with the SSO user, then add the user to a workspace as the local admin.

A cold first dashboard load can take 15 seconds or more while Trino warms up.

Reports: CSV works on charts only, not on dashboards (use PDF or PNG for a dashboard).

To re-run the seed alone (every line prints `exists` once the content is there):

```bash
just demo-seed
```

To follow the logs, optionally of one service:

```bash
just demo-logs egret
```

## Data Apps

In workspace `default`, open Apps, choose "+ App", name it, and the marimo editor opens in the page. Publish creates a version that viewers open from the Apps list. Each author and each viewer gets their own marimo process; they live inside the Egret container, so they stop with it.

- The editor has **no autocomplete** in this stack: Egret's proxy refuses marimo's language server (`/api/lsp`) by design.
- To check the whole lifecycle in a browser (create, edit, run a cell, publish, open as a viewer), run `just demo-verify-apps` with the stack up. It needs Node and Playwright: install it (`npm i playwright` and `npx playwright install chromium`) or set `PLAYWRIGHT_MODULE_DIR` to the folder that holds `node_modules/playwright`. Set `DEMO_PORT` (and `DEMO_APPS_URL`) if you changed the port. It prints `OK <step>` per step and exits 0, or exits 1 with a screenshot path.

## Stop and reset

| Command | Effect |
|---|---|
| `just demo-down` | Stops and removes the containers, including engines egret provisioned (label `egret.managed=true`). **Keeps the data volumes** (including the Data Apps files), so `just demo-up` returns with your data; app processes stop with the container and start again on the next open. Mailpit keeps no volume, so caught mail is lost. |
| `just demo-reset` | Same, and **deletes the data volumes** (Postgres, the object store and the Data Apps files). The next `just demo-up` starts from zero. |

## Change the port

The default port is 8080. If something else uses it, change **all three** values in `deploy/demo/demo.env`, which must match:

```bash
DEMO_PORT=18080
DEMO_URL=http://egret.localtest.me:18080
DEMO_APPS_URL=http://apps.localtest.me:18080
```

`DEMO_PORT` is the port the stack publishes. `DEMO_URL` is what egret, Keycloak and the browser use as the public address, so a mismatch breaks SSO. `DEMO_APPS_URL` is the Data Apps address: the same port, host `apps.localtest.me`. The verify script derives the apps host from `DEMO_APPS_URL`, else from `DEMO_APPS_HOST` (default `apps.localtest.me`) on the port of `BASE`; if you point `BASE` at another host, also set `DEMO_APPS_URL`. Then run `just demo-reset && just demo-up`, because Keycloak and egret read the address when they first start.

To try another port without editing the file, export all three in the shell before the command (shell values win over `demo.env`):

```bash
export DEMO_PORT=18080 DEMO_URL=http://egret.localtest.me:18080 DEMO_APPS_URL=http://apps.localtest.me:18080
just demo-up
```

Mailpit stays on `http://localhost:8025`.

## What is not in this stack yet

- **Engine provisioning** arrives in a later work package. Egret logs "kubernetes unavailable; engine provisioning disabled" in this stack, which is expected.
- Workflow tasks that run containers (the container runner) are not configured. Plain tasks such as `Log` run; container and script tasks are expected to fail.
- Only the Trino engine is included. StarRocks and RisingWave are not.

## Safety

- The Data Apps run in the Egret container as the same user, with **no isolation between apps and users** (the subprocess launcher is development-only; production uses the Kubernetes launcher). Never put real data or untrusted authors in the demo.
- Every secret here is a **public development value** committed to the repository (`deploy/demo/demo.env`): the database password, the object store keys, the master key and the admin password. Never reuse them anywhere real.
- The Keycloak admin console is at `/auth/admin` on the same address, login `admin` / `admin`.
- The imported dev realm (`deploy/keycloak/egret-realm.json`) allows redirect URI `*`, the password grant and `sslRequired: none`. That is why this stack must never be exposed.
- The subprocess launcher sets `PYTEST_CURRENT_TEST` for the app processes, which unlocks test-only overrides. Author code runs as the same user as Egret and can read Egret's process environment (`/proc`). Over plain http Egret's session cookies lack the `__Host-` prefix, so author code on the apps host could set cookies for `Domain=localtest.me` that the browser then sends to Egret (cookie tossing). marimo binds all interfaces of the Egret container, so other demo containers can reach it (token-gated). All acceptable for a demo; never use real data.
- Only the edge (`DEMO_PORT`) and Mailpit (`8025`) are published, on `127.0.0.1`. Do not put the stack behind a public address, a tunnel or a shared network.
- `egret init` prints the bootstrap admin password to the container log on first start (a known issue). The demo credentials are public anyway, but keep it in mind before you copy this setup for anything else.
- This is not a production topology: one node, no TLS, no backups, no resource limits.

## Troubleshooting

- **The port is in use** (`bind: address already in use`): another service holds 8080. Follow "Change the port" above.
- **SSO fails with an issuer mismatch or a redirect error**: the address in the browser must be exactly `DEMO_URL` (host `egret.localtest.me`, same port), and `DEMO_PORT` and `DEMO_URL` must agree. After changing them, run `just demo-reset && just demo-up`. Opening `http://localhost:8080` instead of `http://egret.localtest.me:8080` breaks SSO.
- **`egret.localtest.me` does not resolve**: you are offline or a DNS filter blocks it. Add the `/etc/hosts` line from Requirements.
- **A service is unhealthy or the page does not load**: check which one with `docker compose --env-file deploy/demo/demo.env -f deploy/docker-compose.demo.yml ps`, then read its log with `just demo-logs egret` (or `keycloak`, `edge`, `trino-demo`).
- **The login form rejects you after several tries**: sign-in is rate limited (5 attempts per 5 minutes per address and email). Wait a few minutes.
- **`apps.localtest.me` does not resolve**: same cause and fix as above; the `/etc/hosts` line lists both names.
- **The app editor is blank or stuck**: open the browser console and look for CSP or cookie errors (check that you use `egret.localtest.me`, not `localhost`), then read `just demo-logs egret`. Each app process writes its own log at `/var/lib/egret-apps/*/pod.log` inside the Egret container (`docker compose --env-file deploy/demo/demo.env -f deploy/docker-compose.demo.yml exec -T egret sh -c 'tail -n 50 /var/lib/egret-apps/*/pod.log'`).
- **Known behaviour, Data Apps**:
  - The 403 on `/api/lsp/health` and "Language server initialization failed" in the console are expected (see Data Apps).
  - After you publish, the Versions dialog can still say "Nothing has been published yet" until you reload the page.
  - Publish right after Ctrl+S can answer 409 (the header refreshes the draft revision every 5 s); wait a few seconds and publish again.
- **The login is `admin-sso` and nothing shows**: that is the "No workspaces yet" screen. See "Run it".
