# Classification (sub-project #4b) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Governance's **Classification** group opens a tag catalogue (index) and one page per classification with
Where used (Data and BI), Clearance, Mask & filter, View exposure and Hand-written tabs; Data and People pages list the
hand-written Ranger policies that reach an object or name a principal, so none is hidden (programme F12).

**Architecture:** One matching module in `egret-perm-compile` (`handwritten.rs`: explain's request builder, moved, plus
the match-level rule and item summary) used by three read routes (A: object, B: principal, D: classification); one usage
route (C) in `egret-controlplane` with per-object BI read checks through the existing PDP; `GET /tags` gains Data
counts. Frontend: a lazy area `frontend/src/routes/governance/classifications/` with its own bundle guard; a shared
`HandWrittenSection` for Data and People.

**Tech Stack:** Rust (axum, sqlx, utoipa, `egret-ranger-eval`), React 18 + TypeScript, TanStack Query via the orval
client, Radix UI, vitest + Testing Library, Playwright MCP for the live check.

**Spec:** `docs/en/specs/2026-10-19-dg-classification-design.md` (premises 1–18, C1, C2, CL1–CL17, Q1–Q5) on the
programme spec `docs/en/specs/2026-10-16-dg-frontend-redesign-design.md` (F1–F12) and #4a's
`docs/en/specs/2026-10-19-dg-engine-policy-design.md`.

**Q1–Q5 are decided (2026-10-19):** Q1 (user): group- and role-named items listed in their own group with the
"reaches nobody on the engine" sentence (Tasks 2, 5, 16, 17). Q2: applies / part / set on a parent (Tasks 2, 3, 4,
16). Q3: route C is `GET /api/control-plane/classifications/{id}/usage` (Task 6). Q4: route D
`GET /api/control-plane/classifications/{id}/handwritten` (Task 7). Q5 (user): unreadable BI objects are counted only,
"N objects you cannot read", no workspace count or name (Tasks 6, 12, 15). Task 1 and
Task 2 may start at once.

## Global Constraints

- Base main `309405b1`; branch `feat/dg-classification`; worktree `egret-dgcl`. Never read or touch
  `data-portal/egret` (stale checkout) or another worktree. Rebase on fresh main before the final gate; regenerate the
  client after it.
- **No migration expected** (premise 2, CL15). If a task finds it needs one, stop and tell the controller; the number
  is chosen with `ls migrations | tail -3` right before merge (main's last is `0199`; parallel workflow PRs take
  numbers).
- **Private services, never shared ones.** One Postgres and one Valkey for this package:
  ```bash
  docker ps --format '{{.Names}} {{.Ports}}' | grep -E '5549[0-9]|5649[0-9]|1849[0-9]|539[0-9]'   # pick free ports if taken
  docker run -d --name dgcl-pg -e POSTGRES_USER=egret -e POSTGRES_PASSWORD=egret -e POSTGRES_DB=egret \
    -p 127.0.0.1:55498:5432 postgres:16
  docker exec dgcl-pg sh -c 'until pg_isready -U egret; do sleep 1; done; createdb -U egret egret_test'
  docker run -d --name dgcl-valkey -p 127.0.0.1:56498:6379 valkey/valkey:8
  export DATABASE_URL=postgres://egret:egret@127.0.0.1:55498/egret_test
  export EGRET_TEST_SHARED_STORE_URL=redis://127.0.0.1:56498
  export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-dgcl/target
  export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
  ```
  Every container, network, DB and image tag of this package starts with `dgcl-` (`egret_dgcl_live` for the throwaway
  live DB). Never DB `egret`, never `egret-dev-*`, `egret-ct-*`, `dgep-*` or another package's container. Never
  `EGRET_ALLOW_DB_SKIP=1`. `cargo test` binaries read `.env` through dotenvy: confirm `DATABASE_URL` is exported in the
  same shell. Kill only your own PIDs (`pgrep -x`, never `pgrep -f`).
- Rust per task: the named `cargo test -p <crate> --test <file>` lines, then `cargo test -p <crate>` for each touched
  crate, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt --all -- --check`. Commit **before** a long
  suite (long commands are auto-backgrounded); never run two suites at once; read full output, never a `tail`.
- Never widen an existing route's permission. New routes: A, B, D `policy:read`; C `require_governance_read`.
- **One matching rule** (CL7): no file but `egret-perm-compile/src/handwritten.rs` decides a match or a match level;
  a reviewer greps for `MatchType::` outside it and `egret-ranger-eval` and finds only explain's existing evaluator.
- Frontend: `docs/en/frontend-rules.md` in full; egret + jsx-a11y lint; never add a file to `eslint-rules/legacy.js`;
  OKLCH tokens as `var(--token)`; regenerate with `cargo run -p egret-bin --bin egret -- openapi > openapi.json` then
  `cd frontend && npm run gen:api` only (never bare `orval`). Shared components change additively (FE-TEST-3).
- Frontend per task: `npm run lint`, `npx tsc -b`, `npx vitest run <touched paths>`.
- **Lessons from #4a, each a test somewhere below:** a load failure never renders as an empty fact; one judging rule in
  one place; any 2xx is success (create answers 201); a dialog returns focus to its opener; a write invalidates every
  key that reads what it changed.
- Never `git stash`, `git checkout -- <file>`, `git clean`. ≥ 30 GiB free disk before any build (`df -h`); `target/`
  and `$TMPDIR/egret-ranger-diff-*` fill disks. `rg` is not installed: use `grep -rn`/`find`. Never commit
  `.superpowers/` or scratch files. Commits: Conventional Commits, identity hoangquocvietbk2.13@gmail.com, trailer
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Do not push.
- UI copy: English, sentence case, short and factual; a label never claims more than its data shows ("applies to this
  object" is a resource match, never "grants"; a group item is "reaches nobody on the engine", Q1).

## File structure

Backend:
- `crates/egret-perm-compile/src/handwritten.rs` (new): `ObjectTarget`, `object_request`, `ObjectMatch`, `object_match`,
  `ItemSummary`, `item_summary`, and the DB functions `handwritten_for_object_via`, `handwritten_for_principal_via`,
  `handwritten_for_tag_via`. Modify `routes.rs` (explain calls `object_request`; `request_resource` deleted), `lib.rs`.
  Tests `crates/egret-perm-compile/tests/handwritten_object.rs`, `handwritten_principal.rs`, `handwritten_tag.rs`,
  `handwritten_differential.rs`; pure cases in a `#[cfg(test)]` module of `handwritten.rs`.
- `crates/egret-controlplane/src/governance_handwritten.rs` (new, routes A, B, D), `governance_tag_usage.rs` (new,
  route C); modify `lib.rs` (router, `openapi()`), `tags.rs` (`TagDto.catalog_tables`, `.catalog_columns`). Tests
  `crates/egret-controlplane/tests/governance_handwritten.rs`, `governance_tag_usage.rs`, `tags_counts.rs`.
- `crates/egret-bi/src/shares.rs` (`pub fn read_action_for_gate`, `pub async fn object_name`), `tags.rs`
  (`catalog_counts`).
- `openapi.json`, `frontend/src/api/generated/**` (regenerated).

Frontend:
- `frontend/src/routes/governance/classifications/`: `ClassificationsRoutes.tsx`, `ClassificationsIndex.tsx`,
  `TagPage.tsx`, `TagHeader.tsx`, `DeleteTagDialog.tsx`, `tagUrl.ts`, `tabs/WhereUsedTab.tsx`, `tabs/ClearanceTab.tsx`
  (body of `TagDetail`'s clearance and `ClearanceTable`, moved), `tabs/RulesTab.tsx`, `tabs/ViewsTab.tsx`,
  `tabs/HandWrittenTagTab.tsx`, `__tests__/`.
- `frontend/src/routes/governance/handwritten/HandWrittenSection.tsx`, `handwrittenText.ts`, `__tests__/`.
- Modify `App.tsx`, `api/tags.ts` (`Tag` fields, invalidation), `data/tabs/PermissionsTab.tsx`,
  `data/tabs/MasksFiltersTab.tsx`, `data/ColumnPanel.tsx`, `people/tabs/PermissionsTab.tsx`,
  `people/tabs/MasksFiltersTab.tsx`, `tabParts.tsx` (`EgretGrantsOnlyNote` wording or removal),
  `engines/tabs/PoliciesTab.tsx`, `people/tabs/ClearancesTab.tsx`, `people/RulePage.tsx`, `frontend/package.json`,
  `docs/en/frontend-rules.md` (FE-ROUTE-2, FE-TEST-4), `frontend/CLAUDE.md`. New
  `frontend/scripts/assert-governance-classification-lazy.mjs`. Delete (Task 17) `Classifications.tsx`,
  `TagDetail.tsx` and their tests once ported. `TagRulesSection`, `ViewExposureSection`, `tagDeleteFailure.ts` stay
  where they are (other pages import them).

---

### Task 1: measurement — the premises reading could not settle

No product code. Predictions first, then measure, in `docs/en/plans/2026-10-19-dg-classification-live.md`.

- [ ] Write `## Measurements (Task 1)` and `### 1.0 Predictions`, one predicted value per cell, and commit
  (`docs(dg): Classification measurement predictions`) **before any container starts**.
- [ ] Stack as #4a's `docs/en/plans/2026-10-19-dg-engine-policy-live.md` §1.2, renamed `dgcl-`: `dgcl-egret`,
  `dgcl-rustfs`, `dgcl-trino` (Trino 483, JWT/TLS, profile `cltr` with tag service `cltr-tag`), `dgcl-sr` (StarRocks
  4.1.6, `access_control = ranger`, profile `clsr`) on `dgcl-net`; image `egret-backend:dgcl-1` from a `git archive` of
  this branch; DB `egret_dgcl_live` in `dgcl-pg`; Ranger poll 5000 ms; engine rows `provisioned`/`running` and
  attachments by SQL (DEV-n); catalog `ice` (egret Iceberg) attached to both with `ice.s.t (id, email, country)` and
  `ice.s.u (id)`. Hand-written policies written while the profile is briefly `manual` (#4a DEV-4).
- [ ] Cells:
  - **M1 catalog name.** For each engine, the catalog name its plugin's access request carries for `ice` (a
    hand-written allow on `catalog = ice` works; on `catalog = ICE`; on another spelling) against
    `registry.catalogs.name`. Equal on both? Case-folded?
  - **M2 match levels.** Hand-written allows (user `ua`) on (a) `{catalog: ice}` only, (b) `{ice, schema: s}` only,
    (c) `{ice, s, table: *, column: *}`, (d) `{ice, s, t}` no column, (e) `{ice, s, t, column: email}`, (f) exclude
    `table: u` with `*`, (g) `isRecursive` on schema. For each: `egret_ranger_eval` `match_policy_resources` against
    requests for `ice`, `ice.s`, `ice.s.t`, `ice.s.t.email` (a scratch example binary or test), and the engine's
    answer to `SELECT email FROM ice.s.t` as `ua` (only that policy present). Predict: (a)(b) Ancestor and refused;
    (c) SelfAndAllDescendants and allowed; (d) Ancestor for the column request and refused; (e) Descendant for the
    table, Self for the column; (f) None for `u`. Settles Q2's mapping with engine evidence.
  - **M3 groups and roles in hand-written items.** A hand-written allow naming group `g1` (egret group, `ua` a member)
    and one naming role `r1` (held by `ua`): `ua`'s query on each engine. Predict refused on both (D7/D0). Settles Q1's
    wording.
  - **M4 BI read-check cost.** 500 tagged dashboards across 5 workspaces; time 500 sequential
    `object_ref` + `Pdp::authorize` calls (a scratch test against the live DB; p50, p95, total), as admin and as a
    user with one workspace. Predict total < 2 s. If > 5 s, tell the controller (the cap or a batch read changes).
  - **M5 explain names hand-written deciders.** For `ua` and each policy of M2 that the engine applied: does
    `POST /perm/explain` return that policy as the decision's policy, with its id/name? Predict yes. Basis of Task 3's
    differential test.
  - **M6 BI URLs.** The app URL that opens a dataset, chart, dashboard, app, saved query and folder by id (CL13).
  - **M7 tag-name matching.** A hand-written tag policy on `PII` vs a tag named `pii` on `ice.s.t.email`: does Trino
    apply it? (Predict no: tag service-def `ignoreCase: false`.)
  - **M8 delete consequences.** Delete a gated tag with a table and a column assignment and no rules: both assignment
    tables empty for it, the clearance deny gone from the next download, 204. With a rule selecting its key: 409, the
    message text.
- [ ] If M1 shows a spelling other than `registry.catalogs.name`, M2 contradicts Q2's mapping, M3 shows a group
  or role item applied, or M4 exceeds 5 s: record it and tell the controller before Tasks 3–6 start.
- [ ] Tear down (containers, network, DB, image, scratch secrets); write `### 1.1 Result (read this first)`,
  `### 1.2 Stack and deviations`, `### 1.3 Every cell`, `### 1.4 Left behind`; commit
  `docs(dg): Classification measurements (Task 1)`.

---

### Task 2: one matching module (CL7)

**Files:** create `crates/egret-perm-compile/src/handwritten.rs`; modify `routes.rs` (delete `request_resource`; explain
calls `handwritten::object_request`), `lib.rs`.

**Interfaces (produces):** spec §4.1: `ObjectTarget`, `object_request(engine, catalog_name, &target)`, `ObjectMatch`,
`object_match(&prepared, &policy, &request)`, `Bucket`, `PrincipalMark`, `ItemSummary`, `item_summary(&policy)`.

- [ ] **Step 1: failing tests** (pure, `#[cfg(test)]`, service-defs from `egret_policy::{trino,starrocks,tag}_service_def`):
  `object_request` equals the old `request_resource` for catalog, schema, table and column targets on both engine
  types (StarRocks uses `database`), and stops at the first absent level; `object_match` → `Applies{through_pattern:
  false}` for an exact policy, `Applies{true}` for `schema: *`, for `t?`, for an exclude, for `isRecursive`;
  `Applies{false}` for a literal differing only in case; `Part` for a column policy against its table; `Parent` for
  `{catalog, schema}` against a table; `None` for an excluded table; best verdict over a primary `None` and an
  additional `Self_`; `item_summary` per bucket (allow, deny, both exceptions, mask with `dataMaskInfo.dataMaskType`,
  row filter with `filterExpr`), `public` and `{USER}` → `everyone: true`, any other group and every role
  `on_engines: false`.
- [ ] **Step 2: run** `cargo test -p egret-perm-compile --lib handwritten` → fails.
- [ ] **Step 3: implement.** Move `request_resource` (body unchanged) as `object_request`; explain's tests
  (`cargo test -p egret-perm-compile --test explain_principal`, `--test explain_engine_mismatch`,
  `--test engine_grants_explain` and `--lib`) pass **unedited**. `object_match` uses
  `PreparedPolicyResources::matches` only.
- [ ] **Step 4: run** `cargo test -p egret-perm-compile` + clippy + fmt.
- [ ] **Step 5: commit** `refactor(perm): one module matches hand-written policies to objects`.

---

### Task 3: route A — hand-written policies reaching a Data object (Q2)

**Files:** `handwritten.rs` (`handwritten_for_object_via`), `crates/egret-perm-compile/tests/handwritten_object.rs`,
`crates/egret-controlplane/src/governance_handwritten.rs` (handler A, DTOs), `crates/egret-controlplane/src/lib.rs`,
`crates/egret-controlplane/tests/governance_handwritten.rs`.

**Interfaces (produces):** spec §4.2's DTO (`ObjectHandWritten`, `HandWrittenProfile`, `HandWrittenPolicy`,
`ItemSummaryDto`, `SeededCount`; camelCase; `ToSchema`).

- [ ] **Step 1: failing tests** (`egret_test_db::url()`; a catalog `cl_<test>` with a warehouse, a Trino engine with
  profile + tag service and a StarRocks engine, both attached; policies inserted as `operator` rows; fixtures scoped):
  every case of spec §9 "Route A", each asserting `match`, `throughPattern`, `viaTag` and `enabled` of the row, the
  `seeded` count, and the full `items` of one row. Handler: 403 for a user without `policy:read`, 400 for `table`
  without `schema` and for `column` without `table`, 404 for an unknown catalog, 200 shape.
- [ ] **Step 2: run** `cargo test -p egret-perm-compile --test handwritten_object` and
  `cargo test -p egret-controlplane --test governance_handwritten` → fail.
- [ ] **Step 3: implement** spec §4.2 steps 1–6 in one `REPEATABLE READ, READ ONLY` transaction; external engines read
  from the DB only (a test asserts no network call: the fake engine client is never constructed).
- [ ] **Step 4: run** both crates' tests + clippy + fmt.
- [ ] **Step 5: commit** `feat(governance): hand-written policies that reach a Data object`.

---

### Task 4: the differential test — route A against explain

**Files:** `crates/egret-perm-compile/tests/handwritten_differential.rs`.

- [ ] **Step 1: write** the grid: Trino and StarRocks profiles holding only hand-written policies (allow on a wildcard
  schema, deny on one column, allow on one table with an exclude, mask on a column, a tag access deny via a table tag)
  and users `ua`, `ub`; objects: the catalog, the schema, each table, each column. For each (object, user): call the
  explain function the route uses (`explain_users` or the handler) and `handwritten_for_object_via`; assert (a)
  explain's deciding hand-written policy, when there is one, is in route A's `applies` set for that object; (b) every
  enabled `applies` policy with an allow item naming the user directly is explain's decider or is outranked by a
  decider explain names (deny or higher priority). Fixtures with `unmodelled` constructs excluded and listed in the
  test's doc comment. M5's measurement is cited.
- [ ] **Step 2: run** → it must pass against Task 3's code; then **mutate** `object_match` (map `Descendant` to
  `Applies`) and confirm the test fails, restore, re-run (record both runs in the commit body).
- [ ] **Step 3: commit** `test(governance): hand-written reach agrees with explain`.

---

### Task 5: route B — hand-written policies naming a principal (Q1)

**Files:** `handwritten.rs` (`handwritten_for_principal_via`), `tests/handwritten_principal.rs`,
`governance_handwritten.rs` (handler B), `tests/governance_handwritten.rs` (cases added).

- [ ] **Step 1: failing tests:** a user named directly (`direct`); in group `g1` named by a policy (`group`, `via:
  "g1"`, `onEngines: false`); holding role `r1` (`role`, `onEngines: false`); a `public` item and a `{USER}` item
  only in `everyone`, not in `policies`; a user with no username → `no_username` with the `everyone` count; a group page
  lists a policy naming the group; a role page likewise; a disabled policy listed `enabled: false`; compiled and
  seeded rows never listed; 403; 404 for an unknown user, group, role; 400 for an unknown kind.
- [ ] **Step 2–5:** run → implement (identities from `read_user_via`) → run both crates + clippy + fmt → commit
  `feat(governance): hand-written policies that name a principal`.

---

### Task 6: route C and the `/tags` counts (Q3, Q5)

**Files:** `crates/egret-controlplane/src/governance_tag_usage.rs`, `tags.rs`, `lib.rs`;
`crates/egret-bi/src/shares.rs` (`read_action_for_gate`, `object_name`), `crates/egret-bi/src/tags.rs`
(`catalog_counts`); tests `crates/egret-controlplane/tests/governance_tag_usage.rs`, `tests/tags_counts.rs`.

- [ ] **Step 1: failing tests:** spec §9 "Route C": a user who is a member of workspace W1 but not W2 sees W1's tagged
  dashboard by name and slug and W2's as `unreadable {count: 1}` with no workspace field in the JSON; a gated tag's BI object is unreadable
  to an uncleared reader and readable to a cleared one; 501 objects → `checked: 500, remaining: 1`; a deleted object's
  assignment → `deleted: 1`; an assignment in a warehouse whose catalog row was deleted → `orphaned: 1`; Data grouped
  by catalog/schema/table with `onTable` and `columns`; 403 with neither permission; 200 with `tag:read` only and with
  `policy:read` only; 404 unknown tag. `/tags`: a tag with one table-level row and two column rows on another table →
  `catalog_tables: 1, catalog_columns: 2`; other tags 0; `assignment_count` unchanged; existing tag-route tests green
  and unedited.
- [ ] **Step 2–5:** run → implement (spec §4.4; the 500 cap a `const` with a doc comment citing M4) → run
  `cargo test -p egret-controlplane`, `-p egret-bi`, `cargo test -p egret-server --lib` + clippy + fmt → commit
  `feat(governance): where a classification is used`.

---

### Task 7: route D — hand-written tag policies of a classification (Q4)

**Files:** `handwritten.rs` (`handwritten_for_tag_via`), `tests/handwritten_tag.rs`, `governance_handwritten.rs`
(handler D), `tests/governance_handwritten.rs`.

- [ ] **Step 1: failing tests:** a tag-service `operator` policy on `PII` listed for tag named `PII`; on `P*` listed; on
  `pii` not listed for `PII` (no fold); compiled clearance policies not listed; two tags named `PII` → `sameName`
  names the other; no tag service anywhere → `no_tag_service`; each row's profiles and engines; 403; 404.
- [ ] **Step 2–5:** run → implement → run + clippy + fmt → commit `feat(governance): hand-written tag policies of a
  classification`.

---

### Task 8: OpenAPI and the client

- [ ] Regenerate `openapi.json` (`cargo run -p egret-bin --bin egret -- openapi > openapi.json`) and the client
  (`cd frontend && npm run gen:api`); record the generated hook names for routes A–D in the commit body; `npx tsc -b`
  passes. Commit `chore(api): regenerate for classification routes`.

---

### Task 9: the area, routes, bundle guard, links

**Files:** create `classifications/ClassificationsRoutes.tsx` (index and tag page as stubs rendering an `h1`),
`classifications/tagUrl.ts`, `frontend/scripts/assert-governance-classification-lazy.mjs`,
`classifications/__tests__/routing.test.tsx`; modify `App.tsx` (lazy mount at `classifications/*`, the two static
imports removed), `engines/tabs/PoliciesTab.tsx`, `people/tabs/ClearancesTab.tsx`, `people/RulePage.tsx`,
`frontend/package.json`, `docs/en/frontend-rules.md`, `frontend/CLAUDE.md`.

**Interfaces (produces):** `tagUrl(id, tab?)` with `TagTab = "where-used" | "clearance" | "rules" | "views" | "handwritten"`.

- [ ] **Step 1: failing tests:** both URLs render their stubs behind `tag:read` (and 403 page without); `?tab=` kept;
  `/control-plane/classifications/x` still lands on `/governance/classifications/x`; the three links of spec §7 carry
  `?tab=clearance` / `?tab=rules` (update the existing tests that pinned the bare URL; list them in the report).
- [ ] **Step 2–5:** run → implement (bundle guard modelled on `assert-governance-engines-lazy.mjs`, one marker per
  module present; later tasks add theirs) → tests + lint + tsc; `npm run build`, then each `test:bundle:governance-*`
  in its own run → commit `feat(governance): Classification area and routes`.

---

### Task 10: the index

**Files:** `classifications/ClassificationsIndex.tsx`, `api/tags.ts` (`Tag.catalog_tables`, `.catalog_columns`),
`classifications/__tests__/ClassificationsIndex.test.tsx` (port every case of `__tests__/Classifications.test.tsx`
that still describes behaviour; list the others with the reason).

- [ ] **Step 1: failing tests:** `ListPageShell` `h1` "Classifications"; summary strip counts and plurals (1 vs 2;
  unused counts a tag with 0 Data and 0 BI); columns of spec §3, Data "T tables · C columns", "—" for zero; Clearance
  word + icon, not colour alone; New classification hidden without `tag:manage`/platform admin; the key follows the
  name until edited; a 409 shows the server's message beside Key with inputs kept; success navigates to the tag page;
  error with Retry (never the empty state); empty state; filter miss with Clear.
- [ ] **Step 2–5:** run → implement → run + lint + tsc → commit `feat(governance): classification index`.

---

### Task 11: the tag page frame and header

**Files:** `classifications/TagPage.tsx`, `TagHeader.tsx`, `__tests__/TagPage.test.tsx`.

- [ ] **Step 1: failing tests:** header shows swatch, name, key, description, clearance badge (word + icon); Edit only
  with `tag:manage`/platform admin; Save sends `PATCH` with the changed fields, invalidates `["tags"]`, shows a toast
  (stays on page); Cancel restores; a 4xx keeps the form with the server's message; navigating away with edits warns
  (`useUnsavedChangesWarning`); tabs in order Where used · Clearance · Mask & filter · View exposure · Hand-written,
  `?tab=` selects, an unknown tab falls back to Where used; an unknown id shows "No classification with this id" with
  the index link; a failing tags request shows the error with Retry, not "not found".
- [ ] **Step 2–5:** run → implement (tab bodies placeholders) → run + lint + tsc → commit
  `feat(governance): classification page and header`.

---

### Task 12: Where used tab

**Files:** `classifications/tabs/WhereUsedTab.tsx`, `__tests__/WhereUsedTab.test.tsx`.

- [ ] **Step 1: failing tests** (fixtures not pre-sorted, FE-TEST-1): Data grouped by catalog then schema; a table row
  with "Table" and "Columns: email" linking to the Data page and `?column=email`; `orphaned` sentence; BI readable rows
  per kind with links per CL13 (saved query and folder without a link); "N objects you cannot read." (no workspace count or name);
  `deleted` sentence; "Not all BI objects were checked: N remain."; both empty sentences; loading skeleton; a 500 shows
  the error and Retry and **neither** empty sentence.
- [ ] **Step 2–5:** run → implement → run + lint + tsc → commit `feat(governance): where a classification is used`.

---

### Task 13: Clearance tab

**Files:** `classifications/tabs/ClearanceTab.tsx` (moved from `TagDetail.tsx`: the section and `ClearanceTable`),
`__tests__/ClearanceTab.test.tsx` (port `TagDetail.test.tsx`'s clearance cases; list any not ported).

- [ ] **Step 1: failing tests:** the ported cases (add role, add group, remove, whole-set writes, `skipped` →
  "Saved, but not compiled into any engine: …" and the warning toast, a refusal snaps back and shows the message); the
  four statements of spec premise 3 each under its condition (group warning only with a required group; the other three
  only with a required role); the compile line only with a required role; controls hidden without
  `tag:manage`/platform admin; the picker returns focus to its trigger on close and on Escape.
- [ ] **Step 2–5:** run → implement → run + lint + tsc → commit `feat(governance): clearance tab`.

---

### Task 14: Mask & filter, View exposure and Hand-written tabs

**Files:** `classifications/tabs/RulesTab.tsx`, `ViewsTab.tsx`, `HandWrittenTagTab.tsx`, tests for each.

- [ ] **Step 1: failing tests:** Rules: `TagRulesSection` rendered with `policy:read`, the explanation without (and no
  `/perm/rules` request); Views: `ViewExposureSection` rendered; Hand-written (route D): rows with editor links
  (`editorUrl({engineId}, kind, id)` + `?scope=tag`; the profile page's for an unbound profile), principals with the Q1
  marks, enabled, `unmodelled` sentences, the `sameName` sentence, `no_tag_service` sentence, the explanation without
  `policy:read` (no request), empty sentence, error ≠ empty.
- [ ] **Step 2–5:** run → implement → run + lint + tsc → commit `feat(governance): rules, views and hand-written tabs`.

---

### Task 15: the delete dialog (CL14)

**Files:** `classifications/DeleteTagDialog.tsx`, `__tests__/DeleteTagDialog.test.tsx`; used by the tag page and the
index row menu.

- [ ] **Step 1: failing tests:** the body names T tables, C columns, B BI objects and U unreadable (from route C), the
  clearance's role and group counts, and N rules whose selector key is this tag's key (a rule on another tag with the
  same name not counted); N > 0 adds the refusal sentence; no `policy:read` → the CL14 line and no rules request;
  route C failing → "Usage could not be read: <message>" (the dialog still opens); the confirm button is disabled until
  the exact key is typed (the name does not work); success navigates to the index; a 409 calls `toastTagDeleteFailure`,
  closes the dialog and returns focus to the Delete button; every tag write path invalidates `["tags"]` and route C's
  key (CL16).
- [ ] **Step 2–5:** run → implement → run + lint + tsc → commit `feat(governance): delete a classification with its
  consequences`.

---

### Task 16: `HandWrittenSection` and the Data placements

**Files:** `frontend/src/routes/governance/handwritten/HandWrittenSection.tsx`, `handwrittenText.ts`, tests;
modify `data/tabs/PermissionsTab.tsx`, `data/tabs/MasksFiltersTab.tsx`, `data/ColumnPanel.tsx`, `tabParts.tsx`
(`EgretGrantsOnlyNote` → the new pointer sentence or removed when no caller is left), their tests (cases added; the
ones asserting the old note's text updated and listed).

- [ ] **Step 1: failing tests:** the section per match level word; "through a pattern"; the "Hand-written" badge; item
  summary (deny marked, everyone, group/role with the Q1 sentence); disabled marked; editor link per row (engine page
  Advanced editor; tag-service rows `?scope=tag`); the seeded line with links; `not_determined` per reason; no
  `policy:read` → the explanation and no request; error with Retry ≠ "No hand-written policy reaches this object.";
  Data Permissions lists access rows only, Masks & filters masking and row-filter rows only, the column panel all rows
  for the column (route A called with `column`); the pointer sentence links Who can read? when allowed.
- [ ] **Step 2–5:** run → implement → run + lint + tsc → `npm run build && npm run test:bundle:governance-data` →
  commit `feat(governance): hand-written policies on Data pages`.

---

### Task 17: People placements; delete the old pages

**Files:** `people/tabs/PermissionsTab.tsx`, `people/tabs/MasksFiltersTab.tsx`, their tests (cases added); delete
`Classifications.tsx`, `TagDetail.tsx`, `__tests__/Classifications.test.tsx`, `__tests__/TagDetail.test.tsx` after
confirming each case is ported (Tasks 10–15) or obsolete (list obsolete ones with the reason).

- [ ] **Step 1: failing tests:** a user page groups rows Direct · Through a group or role (Q1 sentence) · Applies to
  everyone (count, expandable); a group page and a role page; a user without username shows the `no_username`
  sentence; the public and workspace pages render no section; no `policy:read` → the explanation; error ≠ empty.
- [ ] **Step 2:** `grep -rn "routes/governance/Classifications\"\|routes/governance/TagDetail\"" frontend/src` → none;
  delete; `npm run lint`, `npx tsc -b`, `npx vitest run`, `npm run build`, every bundle test.
- [ ] **Step 3: commit** `feat(governance): hand-written policies on People pages; remove the old classification pages`.

---

### Task 18: complete the bundle guard

- [ ] One marker per module of `classifications/` and of `handwritten/` (the latter asserted present in the Data and
  People chunks, absent from the entry chunk); `npm run build && npm run test:bundle:governance-classification`; the
  other governance bundle tests still pass. Commit `test(governance): classification bundle guard`.

---

### Task 19: live browser check

- [ ] **Predictions first:** `docs/en/plans/2026-10-19-dg-classification-live.md` `## 2.` `### 2.0 Predictions`, one per
  cell, committed before the stack starts.
- [ ] Stack as Task 1 (image `egret-backend:dgcl-2` from this branch, `dgcl-` names); Vite from this worktree on 5398,
  proxying `/api`, `/catalog`, `/oauth` to `dgcl-egret`. Users: `admin`; steward `vs` (`tag:read`, `tag:manage`,
  `policy:read`); tag reader `vt` (`tag:read` only); `ua` (member of workspace W1 only), `ub`. Logins ≥ 60 s apart.
- [ ] Cells (Playwright MCP; SQL steps recorded as DEV-n):
  - L1 a hand-written allow on `ice.s.*.*` (wildcard schema): listed "Applies to this object · through a pattern" on
    `ice.s.t` Permissions, on the schema, in the column panel of `email`; not on another schema; `ua` can select on
    both engines (Task 1 M2 recipe);
  - L2 a hand-written deny on `ice.s.t.email`: "Applies to part of it" on `ice.s.t`, "Applies to this object" on the
    `email` column panel; `ua`'s select of `email` refused on the engine; Who can read? agrees;
  - L3 a hand-written Trino tag mask on tag `PII` (on `email`): on `email`'s panel and the table's Masks & filters
    ("part"); on the `PII` page's Hand-written tab; masked on Trino;
  - L4 People: `ua`'s Permissions lists L1 and L2 as Direct; a policy naming group `g1` (`ua` in it) under "Through a
    group or role" with the Q1 sentence; a `public` item under Applies to everyone;
  - L5 tag a table, a column and a dashboard in W1 and a dashboard in W2 with `PII`: the index shows "1 table · 1
    column" and 2 BI objects; Where used as admin lists all; as `ua` lists W1's dashboard and "1 object you cannot read."; as `vt` the page works and Mask & filter / Hand-written show the policy:read explanation;
  - L6 clearance: add role `cleared` (holder `ub`) on `PII`: compile line shown; `ua` refused `email` on both engines,
    `ub` reads it; remove: `ua` reads again after one poll;
  - L7 delete `PII` with a rule selecting it: the dialog states every consequence and the refusal sentence; typing the
    name does not enable it, the key does; 409 toast; delete the rule, delete again: index no longer lists it,
    assignments gone;
  - L8 every old link: Overview attention row, Engine & policy clearance source (`?tab=clearance`), People Clearances
    and RulePage, `/control-plane/classifications/<id>`;
  - L9 keyboard only through the index, tabs, Edit form, delete dialog and clearance picker; Escape closes each and
    focus returns to its opener; 390 px (no horizontal page scroll) and 1280 px, light and dark; screenshots checked
    with `sips` (pixel size and bytes).
- [ ] Record every cell with a verdict; defects go to a fix round with a failing test first. Tear down; write
  `### 2.1 Result`, `### 2.2 Stack and deviations`, `### 2.3 Every cell`, `### 2.4 Defects`, `### 2.5 Left behind`;
  commit `docs(dg): Classification live check`.

---

## Final gate (controller)

Whole-branch review on the most capable model, with the frontend reviewer checklist answered per rule; one fix
dispatch + scoped re-review; rebase on fresh main (a migration, if one became necessary, renumbered now with
`ls migrations | tail -3`), then regenerate `openapi.json` and the client. Record test counts **before** (on
`309405b1`, same commands) and **after**, from the full output, never a `tail`.

With the private services and exports of Global Constraints:

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret-dgcl
./deploy/prune-target.sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
just test-shipping-resolution
env -u EGRET_ALLOW_DB_SKIP -u EGRET_ALLOW_STORE_SKIP EGRET_ALLOW_ENGINE_SKIP=1 EGRET_ALLOW_DOCKER_SKIP=1 \
  cargo test --workspace --no-fail-fast 2>&1 | tee "$SCRATCH/dgcl-cargo-test.log"
just test-ranger-gates
cargo run -p egret-bin --bin egret -- openapi > openapi.json && git diff --exit-code openapi.json
cd frontend && npm ci && npm run gen:api && git diff --exit-code src/api/generated \
  && npm run lint && npx tsc -b && npx vitest run && npm run build \
  && npm run test:bundle && npm run test:bundle:governance-data && npm run test:bundle:governance-people \
  && npm run test:bundle:governance-overview && npm run test:bundle:governance-engines \
  && npm run test:bundle:governance-classification
```

(`$SCRATCH` is the session scratchpad.) Count `test result:` lines' passed/failed/ignored from the log; a DB-gated
suite that reports `ok` in 0.00 s is a skip and fails the gate. Stop `dgcl-pg` and `dgcl-valkey` only after the gate.
