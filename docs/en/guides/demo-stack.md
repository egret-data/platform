# Run the demo stack on Docker

> **Demo only. Never expose to a network.** This stack uses fixed, public development secrets and credentials. It is not a production topology.

One command starts the whole platform on Docker, with no Kubernetes: Egret, Postgres, an S3-compatible object store, Keycloak for SSO, a Trino engine, a mail catcher, a PDF/PNG renderer, and a sample workspace with a dataset, three charts and a dashboard.

```bash
just demo-up
```

## What you get

| Service | Address | Credentials |
|---|---|---|
| Egret (UI and API) | `http://egret.localtest.me:8080` | `admin@egret.local` / `Admin12345!` (use "Admin sign-in (break-glass)" on the login page) |
| SSO users (Keycloak, behind the same address under `/auth`) | "Sign in with SSO" button | `admin-sso` / `Admin12345!`, `member-sso` / `Member12345!` |
| Mailpit (every mail the demo sends) | `http://localhost:8025` | none |
| Trino | internal only, registered in workspace `default` as database `trino-demo` | catalogs `tpch` and `memory` |

Behind the one address `http://egret.localtest.me:8080`, an edge nginx serves the frontend and Keycloak (`/auth`), so the browser and the backend see the same OIDC issuer. Postgres, the object store (bucket `warehouse`), the renderer and Trino are not published to the host.

The seed creates, in workspace `default`: database `trino-demo`, dataset `orders` (TPC-H `tpch.tiny.orders`) with metrics `revenue` and `order_count`, charts "Revenue by order status", "Revenue by month" and "Orders by priority", the dashboard "Sales overview", and a workspace SMTP setting that points at Mailpit.

## Requirements

- Docker with Compose v2, [`just`](https://github.com/casey/just), `curl` and `python3` (the seed and readiness scripts use them).
- About 6 to 8 GB of RAM free for Docker.
- The first `just demo-up` builds the backend (a Rust compile) and takes 20 to 40 minutes. Later runs reuse the cached images.
- `*.localtest.me` resolves to `127.0.0.1` through public DNS. On an offline machine add this line to `/etc/hosts`:

  ```text
  127.0.0.1 egret.localtest.me
  ```

## Run it

```bash
just demo-up
```

The command builds and starts the services, waits until Egret answers, prints the address and logins, then seeds the sample content. Open `http://egret.localtest.me:8080`.

- **Local admin**: on the login page choose "Admin sign-in (break-glass)" and use `admin@egret.local` / `Admin12345!`. This account sees the `default` workspace and everything seeded.
- **SSO users**: the "Sign in with SSO" button signs `admin-sso` or `member-sso` in through Keycloak. They have **no workspace** until an admin adds them ("No workspaces yet" is the expected first screen), and egret only knows them after their first sign-in. Sign in once with the SSO user, then add the user to a workspace as the local admin.

The first dashboard load can take a few seconds while Trino warms up.

To re-run the seed alone (every line prints `exists` once the content is there):

```bash
just demo-seed
```

To follow the logs, optionally of one service:

```bash
just demo-logs egret
```

## Stop and reset

| Command | Effect |
|---|---|
| `just demo-down` | Stops and removes the containers, including engines egret provisioned (label `egret.managed=true`). **Keeps the data volumes**, so `just demo-up` returns with your data. Mailpit keeps no volume, so caught mail is lost. |
| `just demo-reset` | Same, and **deletes the data volumes** (Postgres and the object store). The next `just demo-up` starts from zero. |

## Change the port

The default port is 8080. If something else uses it, change **both** values in `deploy/demo/demo.env`, which must match:

```bash
DEMO_PORT=18080
DEMO_URL=http://egret.localtest.me:18080
```

`DEMO_PORT` is the port the stack publishes. `DEMO_URL` is what egret, Keycloak and the browser use as the public address, so a mismatch breaks SSO. Then run `just demo-reset && just demo-up`, because Keycloak and egret read the address when they first start.

To try another port without editing the file, export both in the shell before the command (shell values win over `demo.env`):

```bash
export DEMO_PORT=18080 DEMO_URL=http://egret.localtest.me:18080
just demo-up
```

Mailpit stays on `http://localhost:8025`.

## What is not in this stack yet

- **Data Apps** (the marimo-based apps) and **engine provisioning** arrive in the next work packages. Egret logs "kubernetes unavailable; engine provisioning disabled" in this stack, which is expected.
- Workflow tasks that run containers (the container runner) are not configured. Plain tasks such as `Log` run; container and script tasks are expected to fail.
- Only the Trino engine is included. StarRocks and RisingWave are not.

## Safety

- Every secret here is a **public development value** committed to the repository (`deploy/demo/demo.env`): the database password, the object store keys, the master key and the admin password. Never reuse them anywhere real.
- Only the edge (`DEMO_PORT`) and Mailpit (`8025`) are published. Do not put the stack behind a public address, a tunnel or a shared network.
- `egret init` prints the bootstrap admin password to the container log on first start (a known issue). The demo credentials are public anyway, but keep it in mind before you copy this setup for anything else.
- This is not a production topology: one node, no TLS, no backups, no resource limits.

## Troubleshooting

- **The port is in use** (`bind: address already in use`): another service holds 8080. Follow "Change the port" above.
- **SSO fails with an issuer mismatch or a redirect error**: the address in the browser must be exactly `DEMO_URL` (host `egret.localtest.me`, same port), and `DEMO_PORT` and `DEMO_URL` must agree. After changing them, run `just demo-reset && just demo-up`. Opening `http://localhost:8080` instead of `http://egret.localtest.me:8080` breaks SSO.
- **`egret.localtest.me` does not resolve**: you are offline or a DNS filter blocks it. Add the `/etc/hosts` line from Requirements.
- **A service is unhealthy or the page does not load**: check which one with `docker compose --env-file deploy/demo/demo.env -f deploy/docker-compose.demo.yml ps`, then read its log with `just demo-logs egret` (or `keycloak`, `edge`, `trino-demo`).
- **The login form rejects you after several tries**: sign-in is rate limited (5 attempts per 5 minutes per address and email). Wait a few minutes.
- **The login is `admin-sso` and nothing shows**: that is the "No workspaces yet" screen. See "Run it".
