# OIDC (Keycloak) as the default auth for dev + e2e — Design

> Bilingual: English canonical; VI reference at `docs/vi/specs/2026-07-01-oidc-keycloak-default-dev-e2e-design.md`.

- **Date:** 2026-07-01
- **Status:** Approved (brainstorming) — ready for implementation plan
- **Scope:** Make real OIDC (local Keycloak) the default sign-in path for the
  dev stack and Playwright e2e. Local email/password stays as a break-glass
  admin login. Enterprise direction: exercise the real SSO flow (redirect →
  Keycloak → callback → per-user JWT delegation) instead of local login during
  everyday testing.

## Background / current state

Everything on the OIDC backend already exists; this work is about **defaulting
the dev + e2e experience onto it**, not building OIDC.

Already present:

- **Keycloak in the dev stack** — `deploy/docker-compose.dev.yml` service
  `keycloak` (image `quay.io/keycloak/keycloak:26.0`, `start-dev
  --import-realm`, host port **8081** → 8080), importing
  `deploy/keycloak/egret-realm.json`.
- **Realm `egret`** — client `egret` (secret `egret-secret`, `redirectUris`
  include `http://localhost:8080/*`, `http://localhost/*`, `*`), a `groups`
  mapper on the client, audiences for `egret-catalog` / `trino` / `starrocks`,
  group `data-team`, and user `tester@egret.local` / password `pw` in
  `/data-team`.
- **OIDC backend** — `egret-auth`: `oidc.rs` (`OidcService::discover`,
  `provision_and_sync`), `oidc_routes.rs` (`/api/auth/oidc/login`, `.../callback`),
  `oidc_cookie.rs`, `oidc_refresh.rs`. JIT-provision matches by `sub`
  (`UserRepo::insert_oidc`) and syncs OIDC groups into local groups
  (`GroupRepo::sync_user_oidc_groups`). F2 epic delegates a downscoped per-user
  JWT to Trino / StarRocks / catalog.
- **Config** — `OidcConfig { enabled, issuer_url, client_id, client_secret,
  redirect_url, groups_claim, ... }` (env prefix `EGRET_OIDC__`), gated by
  `security.secret_key` (base64 ≥64 bytes, `EGRET_SECURITY__SECRET_KEY`).
  `validate()` requires `issuer_url` + `client_id` + `secret_key` when enabled.
- **Login page** — `frontend/src/auth/LoginPage.tsx` already renders a "Sign in
  with SSO" button when `GET /api/auth/config` returns `{ oidc_enabled: true }`,
  alongside the local email/password form.
- **RBAC** — `platform_role_assignments` (platform roles; admin elevation via
  `Rbac::replace_platform_roles`) and `workspace_members` (per-workspace role,
  principal is **user OR group**; add via `POST /{workspace}/members`, requires
  `workspace:manage_members`). `IdpRoleMapping` (`egret-workspace/mapping.rs`)
  can map a claim value to a **platform** role, but workspace-membership mapping
  is explicitly a later phase.
- **Dev serving model** — Vite dev server on **:5173** proxies `/api` →
  `http://localhost:8080` with `changeOrigin: true, ws: true`. Playwright
  `baseURL` defaults to `http://localhost:5173`.

Current gap: the dev `.env` sets only the local admin
(`admin@egret.local` / `Admin12345!`) with **no `EGRET_OIDC__*`**, so `serve`
runs with `oidc.enabled = false` → local login. The notebook e2e
(`frontend/tests/e2e/notebook.spec.ts`) logs in through the local form.

## Decisions (from brainstorming)

1. **Login UX:** SSO primary; local form hidden behind a small "Admin sign-in
   (break-glass)" disclosure that reveals it on click.
2. **e2e:** rewrite the notebook e2e to authenticate through the **real
   Keycloak UI** (redirect → KC form → callback), not the local form.
3. **Admin model:** OIDC users JIT-provision as **plain members**. No
   admin group in the realm and no group→`platform_admin` mapping. The local
   admin (`admin@egret.local`) is the bootstrap super-admin / break-glass, and
   is used to elevate OIDC users later via the existing admin RBAC path.
4. **Workspace access for the SSO test user — Approach A (chosen):** seed a
   workspace and bind the **`data-team` group** as a workspace member with a
   notebook-capable role. Because OIDC syncs `tester` into the local
   `data-team` group and `workspace_members` supports group principals, `tester`
   inherits workspace access on first SSO login — no per-user admin click, and
   the e2e stays single-actor and deterministic. (Approach B — a two-actor e2e
   where local admin adds the user live — is deferred; may be added later as a
   separate spec to demonstrate the admin-elevation path. Approach C —
   elevating `tester` to `platform_admin` — is rejected as over-privileged and
   contrary to the member model.)
5. **Redirect URL / port:** in dev + e2e the browser lives on the Vite origin
   `:5173`, so the OIDC redirect must return there for the session cookie to be
   set on the app origin. Use
   `EGRET_OIDC__REDIRECT_URL=http://localhost:5173/api/auth/oidc/callback`
   (allowed by the realm's `*` / `http://localhost/*` `redirectUris`). The full
   flow is then same-origin from the browser's view: SSO button →
   `/api/auth/oidc/login` (Vite proxy → :8080) → Keycloak → `:5173/.../callback`
   (Vite proxy → :8080 sets cookie, redirects to `/`).

## Components / work items

### 1. Config & dev env

- `.env.example` and `.env`: add, alongside the existing local-admin vars
  (kept as break-glass):
  - `EGRET_OIDC__ENABLED=true`
  - `EGRET_OIDC__ISSUER_URL=http://localhost:8081/realms/egret`
  - `EGRET_OIDC__CLIENT_ID=egret`
  - `EGRET_OIDC__CLIENT_SECRET=egret-secret`
  - `EGRET_OIDC__REDIRECT_URL=http://localhost:5173/api/auth/oidc/callback`
  - `EGRET_OIDC__GROUPS_CLAIM=groups`
  - `EGRET_SECURITY__SECRET_KEY=<base64, ≥64 bytes>` — a fixed dev value in
    `.env.example` (clearly marked dev-only, not a real secret).
- Confirm `config::validate()` passes with these values; adjust the
  `redirect_url` default in `egret-config` only if needed (do not change
  behavior when OIDC is disabled).
- `justfile`: `up` already starts Keycloak. Add a short doc note (README /
  recipe comment) on the SSO dev flow and the `tester@egret.local` / `pw`
  credentials. Optionally a `seed-oidc-workspace` recipe (see §3).

### 2. Login UX — `frontend/src/auth/LoginPage.tsx`

- When `cfg.oidc_enabled`:
  - "Sign in with SSO" is the primary, prominent action.
  - The email/password form is collapsed behind a small text link
    ("Admin sign-in (break-glass)") and only rendered after the user expands it.
- When `!cfg.oidc_enabled`: unchanged — local form is primary (today's
  behavior).
- Update `frontend/src/auth/__tests__/LoginRedirect.test.tsx` (and
  `AuthProvider.test.tsx` if affected) for the new conditional rendering; keep
  the local-login path covered for the OIDC-disabled case.

### 3. Realm / seed data

- Keep `tester@egret.local` / `pw` in `data-team` (member — unchanged).
- Seed a workspace and bind the `data-team` **group** as a member with a
  notebook-capable workspace role. Preferred: a small idempotent seed step
  (SQL migration seed or a `just seed-oidc-workspace` script that calls the
  existing admin API `POST /{workspace}/members` with a group principal, or a
  direct `workspace_members` insert) run as part of dev/e2e setup. Decide the
  exact role name against the seeded roles so it grants notebook create + run.
- No admin group added to the realm; platform admin stays on the local account.

### 4. e2e — `frontend/tests/e2e`

- Add an `loginViaOidc(page, user, pass)` helper: `goto('/login')` → click "Sign
  in with SSO" → on the Keycloak-hosted login page fill username/password and
  submit → wait for redirect back into `/explore/{ws}/`.
- Keep a `loginLocalAdmin(page)` helper (break-glass / any admin-only setup).
- `notebook.spec.ts`: authenticate via `loginViaOidc` as
  `tester@egret.local` / `pw`; the rest of the test (create notebook, run cell,
  assert output, reload-restore) is unchanged.
- Playwright global setup / `playwright.config.ts` and the e2e README:
  document/ensure the target stack has Keycloak up (`:8081`), the server running
  with the `EGRET_OIDC__*` env, and the `data-team`→workspace binding seeded.
  Parameterize the SSO test creds via env
  (`EGRET_E2E_OIDC_USER` / `EGRET_E2E_OIDC_PASSWORD`) with the realm defaults.

### 5. Verification

- **Manual:** `just up` (includes Keycloak) → `just serve` with the OIDC env →
  browser `/login` → "Sign in with SSO" → Keycloak login as
  `tester@egret.local` / `pw` → lands in the seeded workspace → create + run a
  notebook cell. Separately confirm the "Admin sign-in (break-glass)" path still
  logs in the local admin.
- **Automated:** `npm run test:e2e` (notebook spec now via OIDC) green.
- **Backend:** existing `egret-auth` OIDC tests
  (`oidc_keycloak.rs`, `oidc_db.rs`, `oidc_refresh.rs`) still pass; run
  workspace `fmt` + `clippy` + `test` and frontend `lint`/`typecheck`/`test`/
  `build` before merge (out-of-quota CI — run full CI locally).

## Out of scope

- Group→workspace *mapping rules* driven by the IdP (later phase; here we seed
  one static `data-team`→workspace binding).
- Real external IdPs (Okta / Azure AD) hardening and multi-workspace
  provisioning.
- Two-actor live admin-elevation e2e (Approach B) — possible follow-up spec.

## Risks / notes

- **Cookie/port pitfall:** getting `REDIRECT_URL` wrong (e.g. pointing at
  `:8080` while the app is on `:5173`) breaks the session silently. Decision 5
  pins it to `:5173`. If e2e ever runs the app served directly from the backend
  on `:8080`, the redirect URL and Playwright `baseURL` must move together.
- **e2e brittleness:** driving the Keycloak-hosted login page depends on KC's
  markup; use resilient selectors (field `name=username` / `name=password`,
  submit button) and generous timeouts (KC cold start).
- **Realm `redirectUris` currently include `*`** — fine for dev, but a
  production realm must pin exact URIs. Note for the prod-hardening spec.
