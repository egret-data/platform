# Engine & policy (sub-project #4a) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Governance's last sidebar group, **Engine & policy**, opens one page per engine (Engine permissions,
Policies, Overlaps, Migrate, Plugin, Advanced) and one per unbound profile, replacing Policies, ProfileDetail,
PolicyEditor, PolicyMigration and OverlapReport, whose URLs redirect.

**Architecture:** One new read route in `egret-controlplane` (`GET /governance/engines/{id}/plugin`) that judges the
plugin ledger through a function extracted from the Overview, so both say the same thing. Everything else uses
existing routes (F11 engine grants, policy admin, perm overlaps and migrate). Frontend: a lazy area
`frontend/src/routes/governance/engines/` with its own bundle guard; the old pages' bodies move into tabs.

**Tech Stack:** Rust (axum, sqlx, utoipa), React 18 + TypeScript, TanStack Query via the orval client, Radix UI,
vitest + Testing Library, Playwright MCP for the live check.

**Spec:** `docs/en/specs/2026-10-19-dg-engine-policy-design.md` (EP1–EP21, Q1–Q4; read "Premises checked against the
code" first) on the programme spec `docs/en/specs/2026-10-16-dg-frontend-redesign-design.md` (F1–F12) and F11's
`docs/en/specs/2026-10-18-perm-engine-grants-design.md`.

**Q1–Q4 are decided (2026-10-19):** Q1 (user): `GET /policies/services` gains `ProfileEngine.origin`,
`ProfileEngine.type` and `ProfileDto.policyCounts`, no new route (Task 4). Q2: editor URLs keep the kind segment
(Tasks 5, 14). Q3: a profile bound to several engines redirects to the first engine by name (Task 5). Q4: write
outcomes are not saved / saved but not compiled / compiled, applies at the next plugin download (Task 9).

## Global Constraints

- Base main `3731f0ce`; branch `feat/dg-engine-policy`; worktree `egret-dgep`. Never read or touch
  `data-portal/egret` (stale checkout). Rebase on fresh main before the final gate; regenerate the client after it.
- **No migration expected** (EP11). If a task finds it needs one, stop and tell the controller; the number is chosen
  with `ls migrations | tail` right before merge (main's last is `0199`; parallel workflow packages take numbers too).
- **Private services, never shared ones.** One Postgres and one Valkey for this package:
  ```bash
  docker run -d --name dgep-pg -e POSTGRES_USER=egret -e POSTGRES_PASSWORD=egret -e POSTGRES_DB=egret \
    -p 127.0.0.1:55497:5432 postgres:16
  docker exec dgep-pg sh -c 'until pg_isready -U egret; do sleep 1; done; createdb -U egret egret_test'
  docker run -d --name dgep-valkey -p 127.0.0.1:56497:6379 valkey/valkey:8
  export DATABASE_URL=postgres://egret:egret@127.0.0.1:55497/egret_test
  export EGRET_TEST_SHARED_STORE_URL=redis://127.0.0.1:56497
  export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-dgep/target
  ```
  Every container, network and image tag of this package starts with `dgep-`. Never DB `egret`, never `egret-dev-*`,
  `egret-ct-*` or another package's container. Never `EGRET_ALLOW_DB_SKIP=1`. `cargo test` binaries read `.env`
  through dotenvy: confirm `DATABASE_URL` is exported in the same shell.
- Rust per task: the named `cargo test -p <crate> --test <file>` lines, then `cargo test -p <crate>` for each touched
  crate, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt --all -- --check`.
- Never widen an existing route's permission. The new route uses `egret_core::authz::require` twice
  (`policy:read`, `engine:read`).
- Frontend: `docs/en/frontend-rules.md` in full; egret + jsx-a11y lint; never add a file to
  `eslint-rules/legacy.js`; OKLCH tokens as `var(--token)`; regenerate with `cargo build -p egret-bin &&
  "$CARGO_TARGET_DIR"/debug/egret openapi > openapi.json` then `cd frontend && npm run gen:api` only (never bare
  `orval`). Shared components change additively (FE-TEST-3): `PrincipalPicker` gains `includeOnly`; `refUrl` gains an
  optional `tab`; `RecentChanges` gains an `id`; `PermissionsTab` gains a section.
- Frontend per task: `npm run lint`, `npx tsc -b`, `npx vitest run <touched paths>`.
- Never `git stash`, `git checkout -- <file>`, `git clean`. Kill only your own PIDs. ≥ 30 GiB free disk before any
  build (`df -h`); `target/` and `$TMPDIR/egret-ranger-diff-*` fill disks. Never commit `.superpowers/` or scratch
  files. Commits: Conventional Commits, identity hoangquocvietbk2.13@gmail.com, trailer
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Do not push.
- UI copy: English, sentence case, short and factual. A label never claims more than its data shows ("egret-compiled",
  not "managed"; the plugin tab says "reports holding", never "enforces").

## File structure

Backend:
- `crates/egret-controlplane/src/plugin_judgement.rs` (new): `ProfileVersions`, `LedgerRow`, `Liveness`,
  `KindState`, `NotReported`, `Judgement`, `judge_plugins`, `ledger_rows_via`. Modify `governance_overview.rs`
  (`drift`, `behind` → call it), `lib.rs`.
- `crates/egret-controlplane/src/governance_engines.rs` (new): the plugin route and DTOs. Modify `lib.rs` (router,
  OpenAPI). Tests: `crates/egret-controlplane/tests/plugin_judgement.rs`, `tests/governance_engine_plugin.rs`.
- Q1: `crates/egret-policy/src/admin/reader.rs` (`EngineBinding.origin`, `.type`),
  `admin/mod.rs` (`ProfileEngine`, `ProfileDto.policy_counts`), `crates/egret-bin/src/main.rs`
  (`list_engine_bindings`), the policy store's count query. Tests in `crates/egret-policy/tests/admin_routes.rs`.
- `openapi.json`, `frontend/src/api/generated/**` (regenerated).

Frontend (`frontend/src/routes/governance/engines/`): `EnginesRoutes.tsx`, `EnginesIndex.tsx`, `EnginePage.tsx`,
`ProfilePage.tsx`, `PageFrame.tsx` (header, banner, tabs shared by both), `PolicyRedirect.tsx`, `engineUrl.ts`,
`compiledSource.ts`, `queryKeys.ts` (only if a hand-written key is needed, FE-DATA-2), `tabs/EnginePermissionsTab.tsx`,
`tabs/EngineGrantDialog.tsx`, `tabs/CheckCapabilityDialog.tsx`, `tabs/PoliciesTab.tsx`, `tabs/OverlapsTab.tsx`,
`tabs/MigrateTab.tsx`, `tabs/PluginTab.tsx`, `tabs/AdvancedTab.tsx`, `PolicyEditorPage.tsx` (moved), `__tests__/`.
Modify `App.tsx`, `components/layout/Sidebar.tsx`, `components/perm/PrincipalPicker.tsx`, `overview/refUrl.ts`,
`overview/AttentionRow.tsx`, `overview/RecentChanges.tsx`, `people/tabs/PermissionsTab.tsx`,
`control-plane/EngineDetail.tsx`, `control-plane/ExternalEngineDetail.tsx`, `control-plane/externalEngine.tsx`,
`control-plane/access/AccessOverview.tsx`, `frontend/package.json`, `docs/en/frontend-rules.md` (FE-ROUTE-2, FE-TEST-4,
the frontend `CLAUDE.md` gate list). New `frontend/scripts/assert-governance-engines-lazy.mjs`. Delete (Task 16)
`Policies.tsx`, `ProfileDetailPage.tsx`, `PolicyMigrationPage.tsx`, `PolicyEditorPage.tsx`, `OverlapReport.tsx` and
their tests once ported.

---

### Task 1: measurement — the premises reading could not settle

No product code. Predictions first, then measure, in `docs/en/plans/2026-10-19-dg-engine-policy-live.md`.

- [ ] Write `## Measurements (Task 1)` and `### 1.0 Predictions` with one predicted value per cell below, and commit
  (`docs(dg): Engine & policy measurement predictions`) **before any container starts**.
- [ ] Stack as `docs/en/plans/2026-10-18-dg-overview-live.md` §2.2, renamed: `dgep-` containers on `dgep-net`
  (`dgep-egret`, `dgep-rustfs`, `dgep-trino` Trino 483 JWT/TLS delegating, `dgep-sr` StarRocks 4.1.6 with Ranger),
  image `egret-backend:dgep-1` from this branch, throwaway DB `egret_dgep_live` in `dgep-pg`, Ranger poll 5000 ms,
  engine rows `provisioned`/`running` by SQL recorded as DEV-n.
- [ ] Seed through the API: one data grant, one mask rule, one per-column StarRocks rule (a locator), a classification
  with a clearance on a column of each engine's catalog, one engine grant per capability that the stack can satisfy,
  one hand-written policy per kind and one by-tag policy.
- [ ] Cells:
  - **M1 name → source.** `SELECT s.name, p.name, p.managed_by FROM policy.authz_policies p JOIN policy.authz_services
    s ON s.id = p.service_id WHERE p.managed_by = 'compiler' OR p.name LIKE 'perm-%'`. For each row: does it parse as
    spec premise 5 says (grant bare uuid; rule uuid with optional `-locator`; clearance tag uuid with optional
    locator, or `perm-clearance-mv`; engine bare uuid), and does the uuid exist in `perm.grants`, `perm.rules`,
    `tags`, `perm.engine_grants`? Which service holds clearance policies (main profile or tag service)? Any
    `compiler` row whose name has no prefix, or a prefixed row that is not `compiler`?
  - **M2 valueKind.** `GET /perm/engine-capabilities`: the set of `resources[].valueKind` per engine type, and which
    capabilities carry `fixed`.
  - **M3 catalogs.** For each engine: the catalogs of `GET /governance/catalogs` whose `engines` names an engine of
    the grant's `reach`, against the list in the 400 a `trino.set_catalog_session_property` with an unreached catalog
    returns (`catalogs_for_service_via`). Equal?
  - **M4 policy lists.** `GET /policies?service=<profile>` and `?service=<tag service>`: `managed_by` values seen,
    `policy_type` values, and whether compiled clearance policies appear in either.
  - **M5 index sources.** `GET /policies/services` JSON for a bound engine (`engines[]` fields) and a tag service row
    (is it distinguishable other than by being named in another row's `tag_service`?); `GET /control-plane/engines`
    as a `policy:read`-only user (predict 403).
  - **M6 impersonation on the engines.** The exact client recipe that exercises a granted impersonation: Trino (JWT
    principal A, session user B, e.g. `X-Trino-User`) and StarRocks (`EXECUTE AS b WITH NO REVERT`). Before the grant:
    the refusal text; after it (and one poll): success. Record both commands; Task 17 reuses them.
  - **M7 explain/capability.** For the same grant: Trino on a delegating profile (predict Undecidable, F11 C9);
    StarRocks (predict Allowed for the grantee, Denied for another user).
- [ ] If M1 shows compiler rows outside premise 5's shapes (other than the MV deny), or M3 shows the two catalog lists
  differ: record it and tell the controller before Task 9/10 start (the dialog's catalog picker or `compiledSource`
  needs another source). If M6 cannot exercise impersonation on one engine: record the reason; Task 17's prediction for
  that engine becomes "not provable live" and the controller is told.
- [ ] Tear down (containers, network, DB, image, scratch secrets); write `### 1.1 Result (read this first)`,
  `### 1.2 Stack and deviations`, `### 1.3 Every cell`, `### 1.4 Left behind`; commit
  `docs(dg): Engine & policy measurements (Task 1)`.

---

### Task 2: one plugin judging function for the Overview and the engine page

**Files:** create `crates/egret-controlplane/src/plugin_judgement.rs`, `crates/egret-controlplane/tests/plugin_judgement.rs`
(or a `#[cfg(test)]` module if the types stay `pub(crate)`; then the DB-free cases live there); modify
`governance_overview.rs` (`drift`, `behind`, `LedgerRow`, the constants), `lib.rs`.

**Interfaces (produces):** spec §4.1 exactly.

- [ ] **Step 1: failing tests** (pure, no DB; `now` fixed): live and confirmed → both `UpToDate`; a report of `-1`
  only since U → `NotReported(VersionUnreported)`; U 30 s ago → `Pending`; a row since U within 5 min with an old
  version and U 2 min ago → `Behind{V}`; no row since U and U 2 min ago → `Behind`; an old plugin id's row before U
  is ignored; newest policies poll 10 min ago → liveness `Quiet` and both kinds `Quiet`; 25 h → `Silent`; tags polled
  20 min ago while policies are live → tags `Quiet`, policies judged; a StarRocks-shaped profile with no tags rows →
  tags `NotReported(NoDownload)`.
- [ ] **Step 2: run** → fails (module missing).
- [ ] **Step 3: implement.** Move the rules out of `drift`/`behind` without changing a threshold or a sentence;
  `drift` maps a `Judgement` to its findings (spec §4.1 mapping). `ledger_rows_via` is the query of
  `governance_overview.rs:670`, plus `plugin_id` and `served_version`.
- [ ] **Step 4: run** the new tests, then `cargo test -p egret-controlplane --test governance_attention` **unchanged**
  (no edit to that file in this task) + `cargo test -p egret-controlplane` + clippy + fmt.
- [ ] **Step 5: commit** `refactor(governance): one plugin judging function`.

---

### Task 3: `GET /governance/engines/{id}/plugin`, OpenAPI and the client

**Files:** create `crates/egret-controlplane/src/governance_engines.rs`,
`crates/egret-controlplane/tests/governance_engine_plugin.rs`; modify `lib.rs` (router, `openapi()`), `openapi.json`,
`frontend/src/api/generated/**`.

**Interfaces (produces):** spec §4.2's DTO (`EnginePluginResponse`, `PluginEngine`, `PluginProfile`, `PluginReportDto`,
`Judged`, `KindReport`, `LedgerRowDto`, camelCase, `ToSchema`); generated hook `useControlPlaneGovernanceEnginePlugin`
(the exact name is orval's; record it in the report).

- [ ] **Step 1: failing tests** (`egret_test_db::url()`; `CpState` as `tests/governance_attention.rs` builds it, real
  `CedarPdp`, roles per test; fixtures prefixed per test):
  - a provisioned running Trino engine with a confirmed ledger → `judged`, policies `up_to_date`, tags `up_to_date`,
    `rows` newest first with plugin ids; the same with an old version after U + 2 min → `behind` with the Overview's
    sentence;
  - a StarRocks engine with only `policies` rows → tags `not_reported`, `notReported = "no_download"`, the StarRocks
    clause in the sentence;
  - an external engine bound to a profile with rows → `not_judged` / `external`, `kinds` empty, `rows` present;
    a provisioned engine with `status = 'failed'` → `not_judged` / `not_running`; an engine with no profile →
    `no_profile`, no rows; unknown id → 404;
  - `plugin.version`/`modelled`/`note` equal `egret_perm::ranger_plugin_report` for the engine row;
  - a legacy shared profile (second engine bound by SQL) → `sharedWith` names it;
  - 403 for a user with `policy:read` only, 403 with `engine:read` only, 200 with both;
  - **agreement:** for each of four fixtures (silent, quiet, tags-quiet, behind) and a clean one, `attention_via`
    (with the real `SOURCES`) raises `plugin_silent` / `plugin_quiet` / `plugin_behind` for the fixture's profile
    exactly when this route reports `silent` / `quiet` (either kind) / `behind` (either kind).
- [ ] **Step 2: run** `cargo test -p egret-controlplane --test governance_engine_plugin` → fails.
- [ ] **Step 3: implement.** Two `require` calls; `begin_snapshot`; read the engine (`engine_mgmt.engines` by id), its
  profile and versions, the other bound engines, `ledger_rows_via`, `judge_plugins`; sentences per spec §4.2.
- [ ] **Step 4: run** `cargo test -p egret-controlplane`, `cargo test -p egret-server --lib` + clippy + fmt.
- [ ] **Step 5: regenerate** the OpenAPI document and the client (Global Constraints); `npx tsc -b` passes. Commit
  `feat(governance): an engine's plugin download state`.

---

### Task 4: origin, type and per-owner counts on `GET /policies/services` (Q1)

**Files:** `crates/egret-policy/src/admin/reader.rs`, `crates/egret-policy/src/admin/mod.rs`, the store's count query
(`policy_counts`), `crates/egret-bin/src/main.rs` (`list_engine_bindings`), every `EngineBinding { .. }` literal
(`grep -rn "EngineBinding {" crates`), `crates/egret-policy/tests/admin_routes.rs`, `openapi.json`, generated client.

- [ ] **Step 1: failing tests** (red first, real `CedarPdp`, fixtures scoped to their own service ids):
  `GET /policies/services` returns each `engines[]` entry with `origin` (`provisioned` and `external` fixtures) and
  `type`; a profile with 2 compiled, 1 seeded and 3 hand-written policies plus 1 hand-written in its tag service
  returns `policyCounts {compiler: 2, infra: 1, operator: 4}`; a profile with no policy returns all zeros;
  `policy_count` unchanged; still 403 without `policy:read`; existing admin route tests green and unedited.
- [ ] **Step 2: run** `cargo test -p egret-policy --test admin_routes` → fails.
- [ ] **Step 3: implement** additively (no field removed or renamed; `serde` camelCase as the DTO already is).
- [ ] **Step 4: run** `cargo test -p egret-policy`, `cargo test -p egret-bin`, `cargo test -p egret-server --lib` +
  clippy + fmt; regenerate `openapi.json` and the client with `npm run gen:api` (Global Constraints); drop the
  hand-widened `Profile` type's `tag_service` cast in `api/policies.ts` only if the generated type now covers it;
  `npx tsc -b` passes.
- [ ] **Step 5: commit** `feat(policy): engine origin and per-owner counts on profiles`.

---

### Task 5: the area, routes, redirects, sidebar and links

**Files:** create `engines/EnginesRoutes.tsx` (index, engine page, profile page and editor routes as stubs rendering
an `h1` only), `engines/PolicyRedirect.tsx`, `engines/engineUrl.ts`, `frontend/scripts/assert-governance-engines-lazy.mjs`,
`engines/__tests__/routing.test.tsx`, `engines/__tests__/PolicyRedirect.test.tsx`; modify `App.tsx`, `Sidebar.tsx`,
`overview/refUrl.ts`, `overview/AttentionRow.tsx`, `control-plane/externalEngine.tsx`, `control-plane/EngineDetail.tsx`,
`control-plane/ExternalEngineDetail.tsx`, `control-plane/access/AccessOverview.tsx`, `frontend/package.json`,
`docs/en/frontend-rules.md`, `frontend/CLAUDE.md`.

**Interfaces (produces):** `engineUrl(id, tab?)`, `profileUrl(name, tab?)`, `editorUrl(subject, kind, policyId?)`
where `subject = {engineId} | {profile}`; `PolicyRedirect` (spec §3).

- [ ] **Step 1: failing tests:**
  - `/governance/engines`, `/governance/engines/<uuid>`, `/governance/engines/profiles/p1` render their stubs (the
    static `profiles` segment wins over `:id`); each requires `policy:read`;
  - every row of spec §3's table: `/governance/policies` → index; `/governance/policies/p1` with p1 bound to E →
    `/governance/engines/E?tab=policies`; `?tab=masking` → `&type=masking`; unbound p2 → the profile page; unknown →
    the index with the notice; a profile bound to engines `b` and `a` → `a` (Q3, first by name); `…/migrate` → `tab=migrate`;
    `…/masking/new?scope=tag` and `…/access/42` → the Advanced editor URLs with `search` kept; `/control-plane/policies/p1`
    reaches the same place; each with `replace` (Back does not return to the old URL);
  - sidebar: the last group is headed "Engine & policy" with one item linking to `/governance/engines`, visible with
    `policy:read` only; no group is headed "Policies"; no heading equals another app's (assert over all arrays);
  - `refUrl({type:"profile",name:"a/b"})` → `/governance/engines/profiles/a%2Fb`; with `tab` → `?tab=…`;
    `{type:"profiles"}` → `/governance/engines`; `AttentionRow` passes `plugin` for each `plugin_*` kind, `policies` for
    `policy_warnings`, `overlaps` for `rule_overlaps` (update the existing overview tests that pinned
    `/governance/policies`; list them in the report);
  - Control Plane: `EngineDetail` and `ExternalEngineDetail` show "Open in Engine & policy" →
    `/governance/engines/:id` with `policy:read`, and not without; `PolicyProfileLink` points at the resolver.
- [ ] **Step 2: run** `npx vitest run src/routes/governance/engines src/components/layout src/routes/governance/overview src/routes/control-plane`
  → fail.
- [ ] **Step 3: implement.** `const EnginesRoutes = lazy(() => import("@/routes/governance/engines/EnginesRoutes"))`
  mounted at `engines/*` inside `LazyRouteArea` under `RequirePermission perm="policy:read"`; the `policies*` routes of
  `App.tsx:615-621` become `PolicyRedirect` routes (statically imported; it imports no page). Bundle guard modelled on
  `assert-governance-overview-lazy.mjs`, one marker per module present (later tasks add theirs); script
  `test:bundle:governance-engines`, listed in FE-ROUTE-2, FE-TEST-4 and `frontend/CLAUDE.md`.
- [ ] **Step 4: run** tests + lint + tsc; `npm run build`, then `npm run test:bundle:governance-engines`,
  `test:bundle:governance-overview`, `test:bundle:governance-data`, `test:bundle:governance-people` (each its own run).
- [ ] **Step 5: commit** `feat(governance): Engine & policy area, routes and redirects`.

---

### Task 6: the index

**Files:** `engines/EnginesIndex.tsx`, `engines/__tests__/EnginesIndex.test.tsx`.

- [ ] **Step 1: failing tests** (the repo's fetch mock for orval hooks, as the people tests use):
  - `ListPageShell` with `h1` "Engine & policy"; `SummaryStrip` counts with written plurals;
  - with `engine:read`: every engine from `GET /control-plane/engines` (an unbound one shows "No profile"), origin,
    type, plugin state from the plugin route (external → "Not judged: external"), engine-grant count per engine from
    `GET /perm/engine-grants`; without `engine:read`: only the bound engines of `ProfileDto.engines`, no plugin column,
    no request to `/control-plane/engines`;
  - hand-written count from `ProfileDto.policyCounts.operator`; origin and type from `ProfileDto.engines[]` for a
    reader without `engine:read`;
  - "Profiles without an engine": an unbound profile listed; a profile named as another's `tag_service` not listed;
    a bound profile not listed; empty state sentence;
  - Delete: hidden without `policy:delete`; the confirm button stays disabled until the exact profile name is typed;
    a 409 shows the server's message; success invalidates the profile list;
  - New profile: hidden without `policy:create`; creates and the new profile appears;
  - each section's loading skeleton, error with Retry, and empty state; one failing source leaves the other section
    rendered.
- [ ] **Step 2–5:** run → implement (move the New profile dialog from `Policies.tsx`; `usePolicyAdminDeleteProfile`)
  → run + lint + tsc → commit `feat(governance): Engine & policy index`.

---

### Task 7: the engine page and profile page frame

**Files:** `engines/PageFrame.tsx`, `engines/EnginePage.tsx`, `engines/ProfilePage.tsx`,
`engines/__tests__/EnginePage.test.tsx`.

- [ ] **Step 1: failing tests:**
  - header from `usePermListEngineGrants(id)`: name, type, origin, profile name or "No profile"; plugin version only
    with `engine:read` (from the plugin route), else the EP19 line; "Open in Control plane" only with `engine:read`;
  - shared-profile banner when `reach.sharedProfile`, naming the other engines; the external-engine sentence for an
    external engine; neither otherwise;
  - tabs in order Engine permissions · Policies · Overlaps · Migrate · Plugin · Advanced; `?tab=` selects; a hidden
    tab in `?tab=` falls back to the first; Migrate hidden while the profile has no hand-written policy;
  - the profile page: no Engine permissions and no Plugin tab; "No engine is bound to this profile";
  - a bound profile's URL redirects to its engine page (EP15); unknown engine id → the not-found state with a link to
    the index; a failing grants request shows the error and Retry without blanking the tab strip.
- [ ] **Step 2–5:** run → implement (tab bodies are placeholders filled by Tasks 8–14) → run + lint + tsc → commit
  `feat(governance): engine and profile pages`.

---

### Task 8: Engine permissions tab (reading)

**Files:** `engines/tabs/EnginePermissionsTab.tsx`, `engines/__tests__/EnginePermissionsTab.test.tsx`.

- [ ] **Step 1: failing tests** (fixtures not pre-sorted; FE-TEST-1):
  - grants grouped by capability in the catalogue's order with label and description; values; an impersonation
    target linked to the People user page, "a user with no username" when `values` lacks it; principals linked via
    `principalUrl`; generated policy name; "written on engine X" only when `engineId` differs;
  - `?grant=<id>` highlights and scrolls to that row;
  - Edit and Delete absent without `policy:update`; Delete confirms, calls `DELETE /perm/engine-grants/{id}`,
    invalidates, toasts; a refusal shows the server message;
  - Managed by egret: one row per `managed` entry with the kind label of spec §6.1, the mapped capability label, and
    no write control;
  - Not grantable here: each `what`/`why` of the engine type and the link to the Advanced tab;
  - no profile: the sentence of spec §6.1 and no Grant button; empty grants: the empty sentence.
- [ ] **Step 2–5:** run → implement → run + lint + tsc → commit `feat(governance): engine permissions on the engine page`.

---

### Task 9: the grant dialog and Check

**Files:** `engines/tabs/EngineGrantDialog.tsx`, `engines/tabs/CheckCapabilityDialog.tsx`,
`components/perm/PrincipalPicker.tsx` (additive `includeOnly`), tests
`engines/__tests__/EngineGrantDialog.test.tsx`, `engines/__tests__/CheckCapabilityDialog.test.tsx`,
`components/perm/__tests__/PrincipalPicker.test.tsx` (one added case; the existing ones untouched).

- [ ] **Step 1: failing tests:**
  - capability list filtered to the engine type; picking each capability renders the controls of its
    `resources[].valueKind` in order (`username` → `SubjectUserField`; `catalog` → a select of the catalogs attached to
    a `reach` engine; `identifier`, `function_signature` → inputs with their help; `fixed` → "Applies to all (*)" and
    no control); every input has a tied `Label`;
  - client checks beside the field: empty, leading/trailing space, `*`, `?`, `{`, `}`; nothing is sent while one
    fails;
  - `PrincipalPicker includeOnly` renders no Exclude list (and without the prop renders it, as before);
  - create sends `{capability, values: [username], principals}` for an impersonation (no `targetUserId`);
    edit sends the whole body with `PATCH`;
  - a 400/409 shows the server's message inside the dialog, which stays open with every input kept;
  - outcomes per Q4: a 409 from a compile failure is "not saved" (dialog open, server reason); success: the toast names the profile; when `reach` lists other engines it names them; when
    `compile.skipped` holds `{sourceKind: "engine_grant", id}` for this grant the tab shows "Saved, but not compiled:
    <reason>. It is not on the engine."; invalidates the engine's grants, all engine grants and the profile's policies;
  - Escape closes the dialog; the first field takes focus on open (FE-A11Y-6);
  - Check: Allowed, Denied and Undecidable render "Allowed", "Denied", "Cannot be decided ahead of time" with an icon
    and word, never colour alone; the deciding policy's source is "egret engine grant" (linked), "egret-seeded",
    "egret-compiled" or "hand-written" by spec §6.1; `no_profile` and `tagResolution = declined` sentences; the Trino
    impersonation cause sentence.
- [ ] **Step 2–5:** run → implement → run + lint + tsc → commit `feat(governance): grant and check engine permissions`.

---

### Task 10: Policies tab

**Files:** `engines/compiledSource.ts`, `engines/tabs/PoliciesTab.tsx`, tests `engines/__tests__/compiledSource.test.ts`,
`engines/__tests__/PoliciesTab.test.tsx`.

**Interfaces (produces):** `compiledSource(name): {kind: "grant"|"rule"|"clearance"|"engine_grant", id: string,
locator?: string} | {kind: "mv_deny"} | null`, the twin of `attribute_policy_name` (spec premise 5; Task 1 M1 is the
evidence).

- [ ] **Step 1: failing tests:**
  - `compiledSource` for every prefix, with and without a locator, a catalog name with hyphens in the locator,
    `perm-clearance-mv`, a truncated uuid, `perm-grant-<uuid>-x` (null), and an unknown prefix (null);
  - the tab lists the profile's and its tag service's policies (two requests), the type filter offers "Tag" only with
    a tag service; `?source=`, `?type=`, `?q=` filter and live in the URL; the count line by source;
  - a compiled row: lock, no Edit/toggle/Delete, "Open source" to `/governance/grants/:id`, `/governance/rules/:id`,
    the classification page (only with `tag:read`; else the kind word), or this engine's Engine permissions tab with
    `&grant=`; MV deny and unparsable → kind only;
  - a seeded row: read-only with its sentence; a hand-written row: "Not managed by egret", toggle (`policy:update`),
    Delete (`policy:delete`, confirm), Edit → `editorUrl(…, row.policy_type, id)` (tag rows → the by-tag editor of the
    main profile, as `ProfileDetailPage.tsx:376-381` does);
  - `PolicyWarningsCallout` rendered; the empty and filter-miss states with Clear.
- [ ] **Step 2–5:** run → implement → run + lint + tsc → commit `feat(governance): every policy of an engine's profile`.

---

### Task 11: Overlaps tab

**Files:** `engines/tabs/OverlapsTab.tsx`, `engines/__tests__/OverlapsTab.test.tsx` (port the cases of
`__tests__/OverlapReport.test.tsx` that describe one profile).

- [ ] **Step 1: failing tests:** only pairs whose `serviceName` is the profile; each pair's two rule links, shared
  object and why; a hand-written member links to its Advanced editor URL; the tab trigger's count badge equals the
  rendered pairs; empty sentence; error with Retry.
- [ ] **Step 2–5:** run → implement → run + lint + tsc → commit `feat(governance): overlaps on the engine page`.

---

### Task 12: Migrate tab

**Files:** `engines/tabs/MigrateTab.tsx` (the body of `PolicyMigrationPage.tsx`, `service` as a prop),
`engines/__tests__/MigrateTab.test.tsx` (port every case of `__tests__/PolicyMigrationPage.test.tsx`; list any case not
ported and why), `overview/RecentChanges.tsx` (`id="recent-changes"`).

- [ ] **Step 1: failing tests:** the ported cases (classify, dry run, apply, undo, the `policy:update` gating and its
  sentence) against the tab; no `h1` and no back link inside the tab; after apply, a link "See it in Recent changes" →
  `/governance#recent-changes`; the Overview's section carries that id.
- [ ] **Step 2–5:** run → implement → run + lint + tsc → commit `feat(governance): migrate as an engine page tab`.

---

### Task 13: Plugin tab

**Files:** `engines/tabs/PluginTab.tsx`, `engines/__tests__/PluginTab.test.tsx`.

- [ ] **Step 1: failing tests:** without `engine:read` the EP19 line and no request; version and "modelled" / the
  note; one row per kind with state word and icon (each of `up_to_date`, `pending`, `behind`, `quiet`, `silent`,
  `not_reported` with both reasons), the server's sentence, last download as `<time dateTime>` plus relative text,
  current / newest reported / served versions with `tabular-nums`; the rule line; `not_judged` (external, not running)
  shows its sentence and the raw rows, no kind rows; `sharedWith` adds the "may come from any engine" line; "Download
  Trino config" only on a Trino profile, opening `TrinoConfigDialog`; loading, error with Retry.
- [ ] **Step 2–5:** run → implement → run + lint + tsc → commit `feat(governance): plugin download state per engine`.

---

### Task 14: Advanced tab and the editor

**Files:** `engines/tabs/AdvancedTab.tsx`, `engines/PolicyEditorPage.tsx` (moved with `git mv` from
`routes/governance/PolicyEditorPage.tsx`; `service` from the parent route, back link and after-save navigation to the
Advanced tab), `EnginesRoutes.tsx` (the editor routes of spec §2, Q2's shape), tests
`engines/__tests__/AdvancedTab.test.tsx`, `engines/__tests__/PolicyEditorPage.test.tsx` (moved and adapted; list any
case changed and why).

- [ ] **Step 1: failing tests:** the warning text exactly as spec §6.6, always present; the hand-written list (only
  `operator` rows) with Edit links; "Create policy" menu (By tag only with a tag service), hidden without
  `policy:create`; Seed default policies only on a Trino profile with `policy:seed`, with its explanation, and the
  created count after success; the editor on `…/advanced/policies/access/new`, `…/masking/42`, `?scope=tag`, for an
  engine and for an unbound profile: loads, saves, and returns to `?tab=advanced`; a compiler-owned or infra policy
  id opens read-only exactly as today.
- [ ] **Step 2–5:** run → implement → run + lint + tsc → commit `feat(governance): advanced tab with the policy editor`.

---

### Task 15: People — engine permissions on a principal page

**Files:** `people/tabs/PermissionsTab.tsx`, `people/__tests__/PermissionsTab.test.tsx` (cases added; existing ones
untouched).

- [ ] **Step 1: failing tests:** a user page lists an engine grant naming the user directly and one naming a group the
  user is in ("through group g", from `standsFor` via `setReach`), not one naming another user; a group page and a role
  page list theirs; the public and workspace pages render no section; each row links to the engine page and shows the
  capability label and values; the empty sentence; an error in the engine-grants request leaves the data grants
  rendered.
- [ ] **Step 2–5:** run → implement → run + lint + tsc → commit `feat(governance): engine permissions on People pages`.

---

### Task 16: delete the old pages; complete the bundle guard

**Files:** delete `routes/governance/{Policies,ProfileDetailPage,PolicyMigrationPage,OverlapReport}.tsx` and their
tests (only after confirming each case was ported in Tasks 10–14, or is obsolete; list obsolete cases with the reason);
`api/policies.ts` (`isCompilerOwnedPolicyName`, `deleteProfile` if no caller: `grep -rn` first); the bundle guard
(one marker per module of `engines/`).

- [ ] **Step 1:** `grep -rn "governance/policies\|ProfileDetailPage\|PolicyMigrationPage\|OverlapReport\|from \"@/routes/governance/Policies\"" frontend/src`
  → only `PolicyRedirect`, its test and the redirect routes remain.
- [ ] **Step 2:** delete; `npm run lint`, `npx tsc -b`, `npx vitest run`, `npm run build`, every bundle test.
- [ ] **Step 3: commit** `refactor(governance): remove the pages Engine & policy replaced`.

---

### Task 17: live browser check

- [ ] **Predictions first:** `docs/en/plans/2026-10-19-dg-engine-policy-live.md` `## 2.` `### 2.0 Predictions`, one
  per cell, committed before the stack starts.
- [ ] Stack as Task 1 (image rebuilt from this branch, `dgep-` names); vite dev server from this worktree on port 5397,
  proxying `/api`, `/catalog`, `/oauth` to `dgep-egret`. Log in as `admin@egret.local` / `Admin12345!`, as a steward
  with `policy:read` + `policy:update` (no `engine:read`), and as a reader with `policy:read` only. Logins are
  rate-limited (5 per 5 min): space them ≥ 60 s apart.
- [ ] Cells (Playwright MCP; SQL steps recorded as DEV-n):
  - L1 grant `trino.impersonate_user` (grantee `ua`, target `ub`) through the dialog; after one poll, Task 1 M6's
    recipe succeeds for `ua` as `ub` on Trino and fails for `uc`; the same with `starrocks.impersonate_user`;
  - L2 the dialog refuses `root` and a platform admin as targets with the server's reason, and stays open;
  - L3 Check for `ua`/`ub` on Trino answers "Cannot be decided ahead of time" with the `{USER}` cause; on StarRocks
    "Allowed" for `ua` and "Denied" for `uc`;
  - L4 stop `dgep-sr`; after 5 min the Plugin tab says `quiet` and the Overview raises `plugin_quiet` for the same
    profile; start it, compile its profile: `pending`, then `up_to_date`, and the Overview finding disappears;
  - L5 Policies tab: one row per source with the right controls; every "Open source" link lands on the named object;
    a hand-written row's Edit opens the Advanced editor and saving returns to the tab;
  - L6 every old URL of spec §3 (typed into the address bar) lands where the table says; Back does not return to it;
  - L7 the steward without `engine:read` sees no Plugin tab content and no plugin column; the `policy:read`-only reader
    sees no write control anywhere;
  - L8 keyboard only through the index, the tabs and both dialogs; Escape closes each dialog; 390 px (no horizontal
    page scroll) and 1280 px, light and dark. Screenshots in the scratchpad, checked with `sips` (pixel size and
    bytes; a 16 or 32 px square is an icon).
- [ ] Record every cell with a verdict; defects go to a fix round, not here. Tear down; write `### 2.1 Result`,
  `### 2.2 Stack and deviations`, `### 2.3 Every cell`, `### 2.4 Defects`, `### 2.5 Left behind`; commit
  `docs(dg): Engine & policy live check`.

---

## Final gate (controller)

Whole-branch review on the most capable model, with the frontend reviewer checklist answered per rule; one fix
dispatch + scoped re-review; rebase on fresh main (if a migration was needed, renumber it now with `ls migrations |
tail`), then regenerate `openapi.json` and the client. Record test counts **before** (on `3731f0ce`, same commands) and
**after**, from the full output, never a `tail`.

With the private services and exports of Global Constraints:

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret-dgep
./deploy/prune-target.sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
just test-shipping-resolution
env -u EGRET_ALLOW_DB_SKIP -u EGRET_ALLOW_STORE_SKIP EGRET_ALLOW_ENGINE_SKIP=1 EGRET_ALLOW_DOCKER_SKIP=1 \
  cargo test --workspace --no-fail-fast 2>&1 | tee "$SCRATCH/dgep-cargo-test.log"
just test-ranger-gates
cargo build -p egret-bin && "$CARGO_TARGET_DIR"/debug/egret openapi > openapi.json && git diff --exit-code openapi.json
cd frontend && npm ci && npm run gen:api && git diff --exit-code src/api/generated \
  && npm run lint && npx tsc -b && npx vitest run && npm run build \
  && npm run test:bundle && npm run test:bundle:governance-data && npm run test:bundle:governance-people \
  && npm run test:bundle:governance-overview && npm run test:bundle:governance-engines
```

(`$SCRATCH` is the session scratchpad.) Count `test result:` lines' passed/failed/ignored from the log; a DB-gated
suite that reports `ok` in 0.00 s is a skip and fails the gate. Stop `dgep-pg` and `dgep-valkey` only after the gate.
