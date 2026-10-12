# Egret end-to-end tests (Playwright)

These drive a **real browser against a running local stack**. They are separate
from the Vitest unit suite (`npm test`) and are **not** run in standard CI —
most need a live backend (some also a real in-cluster kind deploy), so they
are a manual / opt-in smoke of their respective money-paths.

## Prerequisites

The plain-local-stack specs (not the `*-kind` ones, which deploy their own
in-cluster stack via `just e2e-kind*`) need:

1. **Docker services** up:
   ```bash
   docker compose -f ../../deploy/docker-compose.dev.yml up -d
   ```
2. **Backend** running on `:8080`:
   ```bash
   cd ../..   # egret repo root
   EGRET_DATABASE__URL="postgres://egret:egret@localhost:5432/egret" \
   EGRET_CATALOG__ENABLED=true EGRET_CATALOG__AUTH_TOKEN=dev \
   cargo run -p egret-bin -- serve
   ```
3. **Frontend dev server** on `:5173` (the Vite proxy forwards `/api`):
   ```bash
   npm run dev
   ```

## SSO login (Keycloak)

The specs authenticate through the **real Keycloak** in
`docker-compose.dev.yml` (`:8081`, realm `egret`, user `tester@egret.local` /
`pw` in group `data-team`) — the `loginViaOidc` helper clicks **Sign in with
SSO**, fills the Keycloak-hosted login form, and follows the callback back into
a workspace. A break-glass `loginLocalAdmin` (local admin form) is also in
`helpers.ts`.

For this to work:

1. **Run the backend with the OIDC env** from `.env.example` — in addition to
   the env already shown in Prerequisite step 3, the backend also needs the OIDC
   vars (`EGRET_OIDC__ENABLED=true`,
   `EGRET_OIDC__REDIRECT_URL=http://localhost:5173/api/auth/oidc/callback`, plus
   the issuer / client id / secret vars) so `/api/auth/oidc/login` redirects to
   Keycloak and the callback resolves the user.
2. **Bind the `data-team` group to a workspace that has a provisioned
   warehouse** (the "admin grants access" step) so `tester` lands in a usable
   workspace. Do this once:
   ```bash
   cargo run -p egret-bin -- workspace add-group --workspace <your-workspace-slug> --group data-team --role ws_editor
   ```

SSO creds default to `tester@egret.local` / `pw` and can be overridden with
`EGRET_E2E_OIDC_USER` / `EGRET_E2E_OIDC_PASSWORD`.

### Seeded SSO test users

The realm ships two extra fixed-identity users for manual/role testing:

| User | Password | Group | Privilege |
| --- | --- | --- | --- |
| `admin-sso@egret.local` | `Admin12345!` | `platform-admins` | platform admin |
| `member-sso@egret.local` | `Member12345!` | `data-team` | member |

OIDC users JIT-provision as plain members. `admin-sso` becomes a platform admin
because the `platform-admins` **group** is granted the `platform_admin` role —
apply that one-time RBAC seed against the egret DB:

```bash
docker exec -i egret-dev-postgres-1 psql -U egret -d egret < deploy/seed-sso-test-rbac.sql
```

A fresh stack (`docker compose ... up`, first import) creates both realm users
automatically; the seed SQL above wires the group→role grant on the egret side.
`member-sso` reaches a workspace via the same `data-team` binding as `tester`.

## Run

```bash
npm run test:e2e
```

Override the target / credentials via env: `EGRET_E2E_BASE_URL`, the SSO creds
`EGRET_E2E_OIDC_USER` / `EGRET_E2E_OIDC_PASSWORD`, and the break-glass local
admin `EGRET_E2E_EMAIL` / `EGRET_E2E_PASSWORD`.

## User impersonation e2e (`impersonation.spec.ts`)

Opt-in (`EGRET_E2E_IMPERSONATION=1`) against the ordinary dev stack, same
pattern as `embedded-catalog.spec.ts` / `workspace-features.spec.ts` (not `*-kind`).

The break-glass local admin logs in once, seeds a plain local member with no
control-plane permission (`POST /api/control-plane/users`, then added to the
shared workspace as `ws_editor`), and starts a platform impersonation of that
member from **Control Plane → Users → Impersonate**. Asserts the persistent
banner names the target, the app's identity resolves to the target (user menu
+ `GET /api/auth/me`'s `impersonating` overlay), a target-only view differs
from the admin's (no Control Plane link), and **Exit** reverts everything.
Exits any still-active impersonation and deletes the seeded user in a
`finally`.

```bash
cd frontend && EGRET_E2E_IMPERSONATION=1 npx playwright test impersonation
```

## Embedded-catalog e2e (`embedded-catalog.spec.ts`)

Opt-in machine-path loop for the embedded catalog: provision a warehouse + role
+ grant + Catalog Client → `/oauth/token` (mini-IdP) → `/catalog/v1` (embedded
lakekeeper) → egret guard. Also asserts a role with no grants is **403** on the
data plane.

```bash
EGRET_E2E_CATALOG=1 EGRET_E2E_BUCKET=e2e-auto npx playwright test embedded-catalog
```

Requirements:
- The live stack up (`serve` + Vite; Vite proxies `/api`, `/catalog`, `/oauth`).
- A **reserved, initially-empty** RustFS bucket (`EGRET_E2E_BUCKET`, default `e2e`)
  with **no prefix-less warehouse** on it — each run uses a unique `key_prefix`,
  so repeated runs on that bucket don't overlap. Create it once:
  `AWS_ACCESS_KEY_ID=egret AWS_SECRET_ACCESS_KEY=egretsecret aws --endpoint-url http://localhost:9000 s3 mb s3://e2e-auto`.
- S3 overrides (defaults match dev RustFS): `EGRET_E2E_S3_ENDPOINT`,
  `EGRET_E2E_S3_KEY`, `EGRET_E2E_S3_SECRET`.

Skipped unless `EGRET_E2E_CATALOG=1`. See `deploy/embedded-catalog/engine-wiring.md`
to wire a real engine (Trino / pyiceberg).

## Data-plane e2e on kind (`dataplane-kind.spec.ts`)

Opt-in (`EGRET_E2E_KIND=1`), and unlike the other specs it targets the
**in-cluster kind deploy** (`http://egret.localtest.me`), not the host dev stack.
It provisions the full lifecycle via the egret API — storage → warehouse → Trino
engine (1Gi, 1 worker) → enable egret-JWT → assign workspace — confirms the engine
lists + auto-selects in the **SQL Lab** Database picker (the real UI path), then
runs `SELECT current_user` through the SQL Lab query backend (`POST
/api/ws/{ws}/queries`, the exact endpoint the Run button calls) and asserts it
returns the **caller** (`admin`), proving per-user impersonation (OIDC → engine
JWKS → JWT → TLS:8443 → impersonation → query). The query is driven via the API
rather than the Monaco editor + results grid because that DOM chrome is flaky to
script; the money-path exercised is identical. It deletes the engine in a
`try/finally` (not `afterAll`, which runs after `page.request` is torn down) to
free node memory. A freshly JWT-re-provisioned coordinator 500s for ~30–40s until
Trino is `SERVER STARTED`, so the query step retries through that warmup.

### One-time prereq

```bash
just k8s-up            # kind `egret` cluster
just ingress-install   # nginx-ingress (kind flavor)
just coredns-rewrite   # egret.localtest.me resolves cluster-wide
```

**Reserved RustFS bucket.** lakekeeper rejects a new warehouse whose storage profile
overlaps an existing one, and a **prefix-less** warehouse claims a whole bucket — so
point the e2e at a bucket that has no root-level warehouse, via `EGRET_E2E_BUCKET`
(default `warehouse`, the chart-seeded bucket). If that bucket already holds a
prefix-less warehouse (e.g. a hand-provisioned `demo-catalog`), use a dedicated
bucket and create it once in the in-cluster RustFS, e.g.:

```bash
kubectl port-forward -n egret-system svc/rustfs 9000:9000 &
AWS_ACCESS_KEY_ID=egret AWS_SECRET_ACCESS_KEY=egretsecret \
  aws --endpoint-url http://localhost:9000 s3 mb s3://e2e
# then: EGRET_E2E_BUCKET=e2e just e2e-kind   (each run uses a unique key_prefix)
```

> Note: deleting a warehouse via the CP API removes egret's row but does **not**
> deregister it from the embedded lakekeeper, so its storage profile keeps claiming
> the bucket — prefer a dedicated bucket over reusing one that held a warehouse.

### Run

```bash
just e2e-kind          # deploy-kind-oidc → wait-ready → playwright dataplane-kind
```

`just e2e-kind` re-deploys (idempotent `helm upgrade --install`), waits for pods +
OIDC discovery, then runs the spec. Re-runnable on the same cluster — each run uses
a unique `EGRET_E2E_RUN_ID` suffix and cleans up its engine.

Env overrides: `EGRET_E2E_BASE_URL`, `EGRET_E2E_RUN_ID`, `EGRET_E2E_BUCKET`,
`EGRET_E2E_S3_ENDPOINT` / `EGRET_E2E_S3_KEY` / `EGRET_E2E_S3_SECRET`, and the
break-glass admin `EGRET_E2E_EMAIL` / `EGRET_E2E_PASSWORD`.

> If the single kind node is memory-wedged, delete stale engines first:
> `kubectl get deploy -n egret-engines` then `kubectl delete deploy <name>` (or
> delete the engine via the CP UI).

## Auth + control-plane admin e2e (`auth-gating.spec.ts`, `cp-admin.spec.ts`)

Opt-in (`EGRET_E2E_KIND=1`), same `just e2e-kind` run, target `egret.localtest.me`.
Pure browser UI — deterministic, no async k8s (runs in seconds).

- `auth-gating`: a break-glass admin reaches `/control-plane`; a JIT OIDC member
  (`member-sso@egret.local`) is denied it (403 — Forbidden).
- `cp-admin`: the access console pages load seeded data; a group CRUD round-trip
  works through the UI; an audited `role:assign` (triggered via the API) shows in
  the audit log.

Needs the seeded `member-sso` realm user (a fresh `deploy-kind-oidc` imports it).
Member creds override via `EGRET_E2E_OIDC_MEMBER_USER` / `EGRET_E2E_OIDC_MEMBER_PASSWORD`.

## Governance enforcement e2e (`governance-kind.spec.ts`)

Opt-in (`EGRET_E2E_KIND=1`), needs a **Ranger-on** deploy — run via `just
e2e-kind-governance` (deploys with `--set ranger.enabled=true`, so provisioned Trino
engines bind Ranger; egret embeds the Ranger admin at `/service`, no separate server).

Provisions a Ranger-bound Trino engine, seeds a governed Iceberg table, creates
access + column-mask + row-filter policies (via `/api/control-plane/policies`,
targeting the admin's own Trino user), and asserts the admin's `SELECT` returns
MASKED `item` values and ONLY `country='US'` rows.

Notes (baked into `just e2e-kind-governance`):
- `EGRET_E2E_BUCKET=e2e` — a reserved bucket (the default `warehouse` bucket carries
  a prefix-less orphan warehouse; see the data-plane section's reserved-bucket note).
- `EGRET_E2E_S3_ENDPOINT=http://rustfs.egret-system.svc.cluster.local:9000` — the FQDN,
  because Trino (namespace `egret-engines`) can't resolve the bare `rustfs` (in
  `egret-system`) for the Iceberg write path.
- The egret Trino provisioner sets the Ranger plugin poll interval to **30s**, so a
  new mask/row-filter takes up to ~30s to enforce — the spec re-queries until it does.
- The Trino catalog egret provisions is named **`egret`** (not `iceberg`).

## Engine provisioning via the UI (`provisioning-ui-kind.spec.ts`)

Opt-in (`EGRET_E2E_KIND=1`), runs against any in-cluster deploy (e.g. after
`just e2e-kind`). Seeds a warehouse via the API, then drives the control-plane
UI: fills the **Add engine** form for a Trino cluster (1 worker, 1Gi), waits for
the row's status badge to reach **running** (~1–2 min of real k8s provisioning),
then deletes the engine via its detail page. Needs `EGRET_E2E_BUCKET=e2e` (the
default `warehouse` bucket carries a lakekeeper orphan — see the data-plane note).

```bash
cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
  EGRET_E2E_BUCKET=e2e npx playwright test provisioning-ui-kind
```

## Chart settings drag lists (`settings-dnd-kind.spec.ts`)

Opt-in (`EGRET_E2E_KIND=1`), runs against any in-cluster deploy. Drags a table
column and a pie slice with the **keyboard** (`Space`, `ArrowDown`, `Space`) and
asserts both the settings list and the rendered chart follow, then that dragging
back restores the original order, then that clicking a row's colour swatch opens
the picker instead of picking the row up.

**Why it is not a unit test.** `dnd-kit` resolves a drop by measuring elements —
`PointerSensor` needs a real 6px movement and `sortableKeyboardCoordinates` reads
`getBoundingClientRect`. jsdom performs no layout, so every rect is zeros, a
"drag" resolves to nothing, and the assertion passes against an unmoved list.

Two things that will waste an afternoon if rediscovered the hard way:

- **Wait after the activating `Space`.** `dnd-kit` binds its move handlers in an
  effect, so an `ArrowDown` in the same tick is silently dropped and the list
  never moves — which looks exactly like a broken product. The spec waits two
  animation frames, and asserts the move landed on a *different* droppable (the
  pickup announcement uses the same "was moved over" wording).
- **One login for the file.** Sign-in is rate limited to 5 per 5 minutes; a login
  per test spends the budget in one run and the later tests fail at
  `waitForURL`.

```bash
cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://localhost:30080 \
  npx playwright test settings-dnd
```

Chart fixtures are overridable: `EGRET_E2E_TABLE_CHART`, `EGRET_E2E_PIE_CHART`,
`EGRET_E2E_WORKSPACE`.

## Workflow flows (`workflows-flows.spec.ts`)

Opt-in (`EGRET_E2E_WORKFLOWS=1`). W7a's money path in the browser: create a flow,
see `type:` completion, an error marked and listed in Problems, Save, Run the saved
draft with one STRING input until the execution page says SUCCESS, Create version
with a note, then move the editor's tabs by pointer (a split that survives a reload)
and by keyboard (the tab actions menu), and compare v1 with the draft in Revisions.

It needs a backend with the workflow worker on (`EGRET_WORKFLOW__WORKER_ENABLED=true`)
and the break-glass local admin; it uses only core tasks, because default settings
allow no script image. Run it on a stack of its own, never on the shared dev stack:
a dedicated database on the branch's Postgres, a private RustFS, `serve` on a free
port, and Vite through the committed `tests/e2e/vite.e2e.config.mts` (port
`EGRET_E2E_VITE_PORT`, default 5197; backend `EGRET_E2E_API`, default
`http://localhost:8097`). Every command, setup and teardown, is in the spec file's header.

Sign-in is limited to 5 attempts per 5 minutes and each run signs in once. When
iterating, raise the limit **on the private backend only** with
`EGRET_RATE_LIMIT__LOGIN_MAX_ATTEMPTS=200`; never do that on a shared stack.

```bash
cd frontend && npx vite --config tests/e2e/vite.e2e.config.mts   # in one shell
cd frontend && EGRET_E2E_WORKFLOWS=1 EGRET_E2E_BASE_URL=http://localhost:5197 \
  npx playwright test workflows-flows
```

## Workflow executions (`workflows-executions.spec.ts`)

Opt-in (`EGRET_E2E_WORKFLOWS=1`). W7b's money path in the browser. The spec creates and starts one flow through the
API: a task that fails once and succeeds on retry, a `Write` of a small CSV, 400 log lines, a `Loop` over three values
and a five-minute `Pause`. It then asserts:

- the list shows the root and the "Show child executions" switch adds the three iterations;
- the gantt draws two attempt segments on the retried run, measured in the DOM, also at 900 px;
- a selected run's panel shows only its own log lines;
- the Logs tab fits its panel;
- the file preview shows its rows;
- the logs follow live until the execution is complete, then stop reading;
- Resume releases the pause long before its deadline;
- Replay opens a new execution that says "Replayed from".

The stack is private, as for `workflows-flows.spec.ts`:

- a database of its own on the branch's Postgres (`egret_w7b_e2e` on `egret-w7b-postgres`, :5442);
- a private RustFS with one bucket (the `Write` task stores its file there);
- `serve` on :8197 with the worker on;
- Vite on :5207 through the committed `tests/e2e/vite.e2e.config.mts`.

`egret init` generates a random admin password unless `EGRET_ADMIN_PASSWORD` is set, so set it to the password the spec
signs in with (`Admin12345!`, or override `EGRET_E2E_PASSWORD`). Every command, setup and teardown, is in the spec
file's header. A run takes about a minute, and its replay is left paused in the throwaway database, which teardown drops.

```bash
cd frontend && EGRET_E2E_VITE_PORT=5207 EGRET_E2E_API=http://localhost:8197 \
  npx vite --config tests/e2e/vite.e2e.config.mts                       # in one shell
cd frontend && EGRET_E2E_WORKFLOWS=1 EGRET_E2E_BASE_URL=http://localhost:5207 \
  npx playwright test workflows-executions
```
