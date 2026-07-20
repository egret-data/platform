# Per-user enforcement across surfaces (Security e2e — Sub-project B) — Design

> Part of the "full security flow" effort. Sub-project **B** of three:
> A (Ranger auto-admin policies — **merged** `16cca4d` #293) → **B = per-user
> enforcement across SQL Lab → dataset → chart → dashboard → notebook (this doc)**
> → C = notebook write isolation (later).

## 1. Goal

Prove that per-user Ranger enforcement is **consistent across all five query
surfaces** — SQL Lab, saved dataset, chart, dashboard tile, and notebook — by
driving **two identities against the SAME objects** and asserting they see
**different governed results**: a full-access user sees every row with real
values, while a restricted member sees only the row-filtered subset with the
sensitive column masked. The member's governed result must be **identical across
all five surfaces**.

## 2. Current state (verified by exploration)

**Enforcement is already consistent and per-user on every surface** — B needs
**no backend change**; it is a pure end-to-end regression test.

- Every query on every surface runs under the **caller's** minted JWT
  (`EngineTokenBroker::mint(user_id)`), never an owner/service identity:
  - SQL Lab: `egret-query/src/execution/routes.rs:137` (jwt) / caller username at
    `:127` (impersonate).
  - Dataset preview `ds_data`, chart `ch_run`, dashboard tile `db_card_run` — all
    via `resolve_delegation(&st, &ctx, database_id)` → `egret-bi/routes.rs:1809`.
    No "run as dashboard creator" path exists.
  - Notebook: `egret-kernel/routes.rs:782 & 825` — the kernel token is bound to
    `(user_id, workspace_id)`; the query runs as that user.
- **No cross-user result cache.** BI paths return `{columns, rows}` inline
  (nothing persisted/keyed by chart/dashboard id). SQL Lab persists result rows
  but reads are owner-gated (`routes.rs:198`); only `platform_admin` may read
  another user's persisted SQL-Lab rows (admin-only, not member-to-member).

## 3. Requirements (decisions from brainstorming)

| Decision | Choice |
| --- | --- |
| What to prove | Same object → **full-access user sees all, restricted member sees masked+filtered**, consistently across all surfaces |
| Surfaces | **All five**: SQL Lab → dataset → chart → dashboard → notebook (notebook last, best-effort) |
| Scenario | **admin vs member** comparison on the same table/dataset/chart/dashboard/notebook |
| Governance primitives | table `select` (+ `execute`/`impersonate` plumbing) for both; **column MASK** + **row-filter `country='US'`** for the member only |
| Code change | **None** — pure e2e regression test |

## 4. Identities & engine mode (resolved)

Per-user JWT minting needs a stored **OIDC refresh token** (`offline_access`); the
notebook surface mints a per-user JWT **unconditionally** (even against an
impersonate-mode engine, `kernel/routes.rs:782`). The break-glass `admin@egret.local`
has **no** refresh token, so it 401s on the notebook — unusable for a 5-surface test.

- **Full-access identity: `admin-sso@egret.local`** (pw `Admin12345!`) — realm
  group `platform-admins` → `platform_admin` (via `deploy/seed-sso-test-rbac.sql`),
  has `offline_access` → refresh token. Trino identity slug = **`admin-sso`**.
- **Restricted identity: `member-sso@egret.local`** (pw `Member12345!`) — realm
  group `data-team` → `ws_editor` in `demo`/`eng`, has `offline_access`. Trino
  identity slug = **`member-sso`**. Helper `loginViaOidcMember` already targets it.
- **Engine delegation: `delegationMode: "jwt"`, set explicitly** in the engine
  spec (`enableEngineJwt` only sets `auth.jwt`, leaving mode = impersonate default).
  jwt-mode presents ONE uniform identity (the JWT `preferred_username`) to Ranger
  across all five surfaces, removing the impersonate/jwt identity-source mismatch.

**Critical consequence of A's known limitation:** A's `platform_admin_user_ids()`
enumerates only **direct** user grants; `admin-sso` is **group**-granted, so A does
**not** auto-seed it. Therefore B must **explicitly create a full-access Ranger
policy for `admin-sso`** (it is NOT covered by A's auto-seed). This is a real,
useful finding — B demonstrates the A group-grant gap (candidate follow-up: extend
`platform_admin_user_ids()` with group expansion).

## 5. Architecture (the e2e)

One opt-in kind spec `frontend/tests/e2e/per-user-enforcement-kind.spec.ts`
(`EGRET_E2E_KIND=1`, `test.setTimeout(900_000)`), Ranger-on deploy. Structure:
serial `describe` with a shared `beforeAll` setup, then one assertion block per
surface.

### 5.1 Setup (control-plane as break-glass admin; data as the OIDC users)

Control-plane ops use `loginLocalAdmin` + CSRF (no refresh token needed):
1. `provisionStorageAndWarehouse` → provision a Trino engine **with
   `delegationMode:"jwt"`** (extend/parameterize `provisionTrinoEngine` or PUT the
   spec) → `waitForEngineReady` → `assignEngineToWorkspace` (workspace = `demo`).
2. Resolve the Ranger service (`engineProfileName`).
3. Create Ranger policies (`createPolicy`, service-scoped):
   - **`admin-sso` full:** `access` policy granting `admin-sso` `execute`
     (queryid `*`), `impersonate` (trinouser `*`), and table `select`/`insert`/
     `create` on the target catalog/schema/table — NO mask, NO row-filter.
   - **`member-sso` restricted:** `access` granting `member-sso` `execute` +
     `impersonate` + table `select` only; `masking` (MASK) on the sensitive
     column; `rowfilter` `country='US'`.
4. Ensure both OIDC users can reach the `demo` workspace surfaces: `member-sso` is
   `ws_editor` there; confirm `admin-sso` reaches ws surfaces (platform_admin
   bypass — see §7) or grant it a ws role. Ensure the engine is **notebook-enabled**
   for `demo` (`PUT /api/ws/demo/notebook-settings` with the analytics connection
   id — see the A live-debug findings).

### 5.2 Seed data + shared BI objects (as `admin-sso`, which now has full access)

5. Log in as `admin-sso` (new helper / `EGRET_E2E_OIDC_USER=admin-sso@egret.local`).
   Seed a governed table, e.g. `egret.gov.sales(country varchar, item varchar,
   amount bigint)` with rows across countries — e.g. 2×`US`, 1×`VN`, 1×`SG` — and a
   sensitive column (`item`) to be masked. Use SQL Lab `runQuery` DDL/DML (works
   because `admin-sso` has the full policy).
6. Create the shared BI objects once (dataset → chart → dashboard) on that table
   via the BI API (`POST /datasets`, `/charts`, `/dashboards` + a card). The
   dataset is a virtual/physical ref to `egret.gov.sales`; the chart selects
   `country`, `item`, `amount`; the dashboard has one card = that chart.

### 5.3 Per-surface assertions (drive BOTH identities on the SAME objects)

For each surface, run the equivalent "show country + item" query/read as
`admin-sso` and as `member-sso`, and assert:
- **admin-sso**: all rows (incl. `VN`/`SG`) with **real** `item` values.
- **member-sso**: only `country='US'` rows, with `item` **masked** (not the real
  value).

Surfaces + endpoints (from exploration):
| Surface | Endpoint | admin-sso | member-sso |
| --- | --- | --- | --- |
| SQL Lab | `runQuery` → `/queries` (`routes.rs:94`) | all rows, real item | US rows, masked item |
| Dataset preview | `POST /datasets/{id}/data` (`routes.rs:1004`) | all | US, masked |
| Chart data | `POST /charts/{id}/data` (`routes.rs:1829`) | all | US, masked |
| Dashboard tile | `POST /dashboards/{id}/cards/{card_id}/data` (`routes.rs:2080`) | all | US, masked |
| Notebook | `trino("SELECT country,item FROM egret.gov.sales")` → `.to_pydict()` | all | US, masked (**best-effort**) |

The cross-surface consistency assertion: the member's `(rows, masked values)` is
the SAME on all five, and strictly a subset/masked view of the admin's.

Cleanup: `finally { deleteEngine }` (removes the engine + its Ranger service +
policies + BI objects tied to the ws as applicable).

## 6. Test helpers (frontend/tests/e2e)

- Reuse: `loginLocalAdmin` (control-plane), `loginViaOidcMember` (member-sso),
  `csrf`, `firstWorkspace`, `provisionStorageAndWarehouse`, `waitForEngineReady`,
  `assignEngineToWorkspace`, `engineProfileName`, `createPolicy`, `runQuery`,
  `deleteEngine`.
- Add: a `loginViaOidcAsAdminSso` helper (mirrors `loginViaOidc` but targets
  `admin-sso@egret.local` / `Admin12345!`), a jwt-mode engine provisioner (or a
  `delegationMode` param), and small BI-object create helpers if none exist.
- The notebook surface drives the kernel like the existing notebook e2es (set
  Monaco source, Run, read output) under each user's session.

## 7. Edge cases, risks & open questions (resolve in planning)

1. **admin-sso not auto-seeded by A** (group-granted) → B must create its
   full-access policy explicitly (§4). Do NOT rely on A for the admin side.
2. **admin-sso workspace-surface access:** does `platform_admin` bypass the ws
   `dataset:read`/`chart:read`/`notebook:read` gates, or must `admin-sso` be added
   as a ws member? Verify (`require_permission` platform_admin bypass) and, if
   needed, grant `admin-sso` a `demo` ws role in setup.
3. **Refresh tokens:** both OIDC users have `offline_access` in the realm seed;
   confirm the login actually stores a refresh token (the A engine work proved the
   OIDC path stores it). If `seed-sso-test-rbac.sql` wasn't applied after a DB
   reset, admin-sso may lack `platform_admin` — ensure the deploy applies it.
4. **Notebook kernel-token lifecycle:** per prior notebook e2e work, a stale
   per-(user,workspace) kernel record can desync its token; drive the notebook on
   a FRESH kernel and treat notebook as best-effort (assert member-restricted at
   minimum; admin-full if the kernel cooperates).
5. **Masking assertion:** assert the member's `item` value is NOT equal to the
   real seeded value (masked), rather than hard-coding the exact mask output (mask
   format may vary) — and that the member sees exactly the US row count.
6. **BI object creation perms:** confirm `admin-sso` (or the setup identity) can
   create datasets/charts/dashboards in `demo` (dataset:write etc.).

## 8. Testing strategy

B *is* the test. Verify locally: `npm run typecheck` + `eslint` on the new spec
(it can't run without a cluster). Live: `just`-style Ranger-on deploy +
`npx playwright test per-user-enforcement-kind`. Add a `just e2e-kind-per-user`
recipe mirroring `e2e-kind-governance`. Reuse the A rollout-restart lesson: after
`deploy-kind-governance`, `kubectl rollout restart deploy/egret` before running.

## 9. Out of scope

- Any backend enforcement change (enforcement is already consistent).
- Extending A's `platform_admin_user_ids()` to group-granted admins (noted as a
  follow-up; B works around it with an explicit admin-sso policy).
- **C** — notebook write isolation to a per-workspace namespace.
