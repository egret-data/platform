# Service accounts SA3 — Service accounts tab — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A dedicated "Service accounts" tab (sidebar peer to "People & groups") to manage service accounts as first-class principals — list/create/edit, platform/workspace scope, group/role membership, and credential management across both audiences — on the SA1 `/service-accounts` and SA2 `/service-accounts/{id}/credentials` routes.

**Architecture:** A thin API-wrapper module over the generated control-plane-access client (mirroring `frontend/src/api/policies.ts`), a lazily-mounted `ServiceAccountsRoutes` with a list page and a detail page, and a sidebar entry. React + TanStack Query + the generated orval client + shadcn/ui + vitest/Testing Library. Frontend only; no backend, no new endpoint.

**Tech Stack:** React, react-router-dom, TanStack Query, generated orval client, shadcn/ui, vitest + Testing Library.

## Global Constraints
- Branch `feat/svc-accounts-sa3` (worktree `../egret-sa3`, off main `b0c27d61f` which has SA1 + SA2). FE-only — no `CARGO_TARGET_DIR`.
- **Read `docs/en/frontend-rules.md` before changing anything under `frontend/`** — its MUST rules bind (FE-ROUTE-*, FE-PAGE-*, FE-TEST-*). No prettier in this tree. Regenerate the client ONLY with `npm run gen:api`, never bare orval — but SA3 adds NO endpoint, so NO regen is needed.
- Chat VI; code/commits/docs EN. Conventional Commits; `Closes #N` only in PR body. Plain `git commit`; stage by explicit path. Co-author `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. No secrets.
- FE gate (run from `frontend/`): `npm run lint`, `npx tsc -b`, `npm run test`, `npm run build`, and the governance bundle test (`npm run test:bundle:governance-engines` or the matching bundle target — confirm the exact script in package.json). Per-task runs the relevant subset; T5 runs the full gate.
- No `any`; reuse generated client types; invalidate queries on mutation (mirror People/engines patterns).

**Generated client fns (verified on main, in `@/api/generated/control-plane-access/control-plane-access`):**
- SA: `controlPlaneAccessListServiceAccounts`, `...CreateServiceAccount`, `...GetServiceAccount`, `...DisableServiceAccount`, `...EnableServiceAccount`, `...DeleteServiceAccount`, `...AddServiceAccountGroup`, `...RemoveServiceAccountGroup`, `...AddServiceAccountRole`, `...RemoveServiceAccountRole` (+ their `get...QueryKey`/`use...` variants).
- Credentials: `controlPlaneAccessListCredentials`, `...CreateCredential`, `...RotateCredential`, `...RevokeCredential`, `...SetCredentialExpiry`.
- Models (in `@/api/generated/model`): `ServiceAccountDto`, `ServiceAccountDetailDto`, `CreateServiceAccountBody`, `AddGroupBody`, `AddRoleBody`, `Credential`, `CreatedCredential`, `CreateCredentialBody`, `SetExpiryBody`, `Audience`, `CatalogBinding`, `CredentialStatus`, `ControlPlaneAccessListServiceAccountsParams`.

---

### Task 1: API wrapper module `frontend/src/api/serviceAccounts.ts`

**Files:**
- Create: `frontend/src/api/serviceAccounts.ts`
- Test: `frontend/src/api/__tests__/serviceAccounts.test.ts`

**Interfaces:**
- Produces thin async helpers + query-key factories over the generated fns, unwrapping orval's `{status,data,headers}` envelope (mirror `policies.ts`): `listServiceAccounts(params)`, `getServiceAccount(id)`, `createServiceAccount(body)`, `disableServiceAccount(id)`, `enableServiceAccount(id)`, `deleteServiceAccount(id)`, `addServiceAccountGroup(id, body)`, `removeServiceAccountGroup(id, groupId)`, `addServiceAccountRole(id, body)`, `removeServiceAccountRole(id, roleId)`, `listCredentials(id)`, `createCredential(id, body)`, `rotateCredential(id, cid)`, `revokeCredential(id, cid)`, `setCredentialExpiry(id, cid, body)`; and `serviceAccountsQueryKey(params)`, `serviceAccountQueryKey(id)`, `credentialsQueryKey(id)`. Re-export the model types from `@/api/generated/model`.

- [ ] **Step 1: Write failing tests** — mock the generated module; assert each helper calls the right generated fn and returns the unwrapped `.data`; assert the query-key factories are stable/parameterized. (Model on `frontend/src/api/__tests__/policies.*` if present.)
- [ ] **Step 2: Run, verify fail** (`npm run test -- serviceAccounts`).
- [ ] **Step 3: Implement** the wrapper per the `policies.ts` adapter pattern (unwrap the envelope; keep names/signatures; re-export types).
- [ ] **Step 4: Run, verify pass.**
- [ ] **Step 5: Commit.** `feat(sa): service-accounts API wrappers (SA3)`

---

### Task 2: Sidebar entry + routes + the list page

**Files:**
- Create: `frontend/src/routes/governance/service-accounts/ServiceAccountsRoutes.tsx`, `.../ServiceAccountsIndex.tsx`, `.../serviceAccountsUrl.ts` (URL helpers)
- Modify: `frontend/src/App.tsx` (lazy-import + mount `ServiceAccountsRoutes` at `/governance/service-accounts/*` behind `user:read`, like `PeopleRoutes` at :67), `frontend/src/components/layout/Sidebar.tsx` (add the nav entry after "People & groups": `{ label: "Service accounts", icon: <pick a bot/key icon imported in Sidebar>, perm: "user:read", to: "service-accounts" }`)
- Test: `.../__tests__/ServiceAccountsIndex.test.tsx`

**Interfaces:**
- Consumes Task 1 (`listServiceAccounts`, `createServiceAccount`, `disable/enable/deleteServiceAccount`, keys). URL state (FE-PAGE-2): `?scope=platform|workspace`, `?workspace=<id>`, `?page=`.

- [ ] **Step 1: Write failing tests** — the list renders SAs (name/scope/workspace/status) from a mocked `listServiceAccounts`; the create dialog validates (workspace required iff scope=workspace) and calls `createServiceAccount` then invalidates + navigates to detail; disable/enable/delete call the right helper and invalidate; scope filter drives the query param.
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** the `<Routes>` wrapper (mirror `PeopleRoutes.tsx`: `index` → `ServiceAccountsIndex`, `:id` → the detail page from Task 3 — stub the detail route import so this task compiles, or land the detail route in T3), the list page (table + scope filter + create dialog + row actions), the sidebar entry, and the App.tsx mount. Follow FE-ROUTE-1/2 (absolute redirects, perm-gated route) and the PeopleIndex loading/empty/error patterns.
- [ ] **Step 4: Run, verify pass** (`npm run test -- ServiceAccountsIndex`). Also `npx tsc -b` to confirm the route wiring compiles.
- [ ] **Step 5: Commit.** `feat(sa): service accounts tab — list page + nav (SA3)`

---

### Task 3: Detail page — Identity + Membership

**Files:**
- Create: `frontend/src/routes/governance/service-accounts/ServiceAccountDetail.tsx` (identity + membership sections), supporting components as needed
- Modify: `ServiceAccountsRoutes.tsx` (wire the `:id` route to the detail page)
- Test: `.../__tests__/ServiceAccountDetail.membership.test.tsx`

**Interfaces:**
- Consumes `getServiceAccount(id)` (→ `ServiceAccountDetailDto` with its group/role memberships), `addServiceAccountGroup/Role`, `removeServiceAccountGroup/Role`. Group/role pickers reuse existing people/perm pickers if available (check `frontend/src/components/perm/PrincipalPicker.tsx` and any group/role select).

- [ ] **Step 1: Write failing tests** — detail renders identity (name/description editable, scope, `svc-` username, status) + current group/role memberships; adding a group/role calls the helper + invalidates; a server refusal (OIDC group 409, platform_admin 4xx) renders as an INLINE human error, not a crash (mock the helper to reject with the ErrorBody shape).
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** the detail page's Identity + Membership sections; surface guard refusals inline (reuse the app's error-to-message mapping, e.g. `apiErrors.ts`). Keep the credentials section as a stub/placeholder that Task 4 fills.
- [ ] **Step 4: Run, verify pass.**
- [ ] **Step 5: Commit.** `feat(sa): service account detail — identity + membership (SA3)`

---

### Task 4: Detail page — Credentials panel

**Files:**
- Create: `frontend/src/routes/governance/service-accounts/CredentialsPanel.tsx` (+ a one-time-secret reveal component, a catalog-binding picker)
- Modify: `ServiceAccountDetail.tsx` (mount the credentials panel)
- Test: `.../__tests__/CredentialsPanel.test.tsx`

**Interfaces:**
- Consumes `listCredentials(id)` (→ `Credential[]` with `audience`, `status`, catalog binding), `createCredential(id, body)` (→ `CreatedCredential` with the one-time `secret`), `rotateCredential`, `revokeCredential`, `setCredentialExpiry`. Catalog create carries `CatalogBinding { warehouse_id, catalog_role_id, consumer_type }` — reuse existing warehouse/catalog-role pickers if available (check the engines/catalog routes for a warehouse select); otherwise a minimal select fed by the catalog list API.

- [ ] **Step 1: Write failing tests** — the list shows both audiences with audience + status (active/revoked/expired) badges, catalog rows show warehouse/role; create (data-api) returns a secret shown ONCE in a copy-able reveal with a "won't be shown again" warning; create (catalog) requires warehouse/role/consumer_type; a **platform-scoped** SA has the Data-API create option disabled with an explanation; rotate shows the new secret once + marks the old revoked; revoke and set-expiry call the right helper + invalidate; the "catalog credentials read Iceberg directly, outside DG masking" note is present.
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** the credentials panel: list, per-audience create form, the one-time-secret reveal (never re-fetchable — hold it only in component state, clear on close), rotate/revoke/expiry, the platform-SA-no-dataapi rule, and the catalog/DG note. Permission-aware controls (hide/disable what the caller can't do; a 403 surfaces inline).
- [ ] **Step 4: Run, verify pass.**
- [ ] **Step 5: Commit.** `feat(sa): service account detail — credentials panel (SA3)`

---

### Task 5: Full FE gate + screenshots

- [ ] **Step 1:** from `frontend/`: `npm run lint && npx tsc -b && npm run test && npm run build` — all green; fix anything flagged (the rulebook MUST rules bind).
- [ ] **Step 2:** run the governance bundle test (confirm the exact `test:bundle:*` script covering `routes/governance`) — green.
- [ ] **Step 3:** capture screenshots for the docs PR (the Service accounts tab list, the create dialog, a detail page with memberships, the credentials panel incl. the one-time-secret reveal and the catalog/DG note). Save them where the docs expect (coordinate with the docs PR; e.g. a `scratchpad` drop the docs task picks up, or the `egret-data/docs` static path).
- [ ] **Step 4: Commit** any snapshot/test updates. `test(sa): service accounts tab gate green (SA3)`

---

## Self-Review
- **Spec coverage:** wrappers → T1; tab + list + nav → T2; detail identity+membership → T3; credentials panel → T4; gate+screenshots → T5. Deferred items (deprecating Data-API Clients, SA4 platform_admin/mask docs) named in the spec.
- **Placeholder scan:** the only "confirm in code" notes are the exact bundle-test script name, the existing picker components to reuse (PrincipalPicker / warehouse select), and the `apiErrors.ts` error-mapping — each with the file to read. No vague steps; every task lists concrete test assertions.
- **Type consistency:** the generated fn names + model types are used verbatim as listed in Global Constraints; the wrapper helper names in T1 match their call sites in T2–T4; URL params (`scope`, `workspace`) are consistent across T2's list and the detail.
- **Rulebook:** every FE task reads `docs/en/frontend-rules.md` first; routes follow FE-ROUTE-*, URL state FE-PAGE-2, tests FE-TEST-*.
