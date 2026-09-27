# SP4b-2 — the permission UI

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** make egret's data-permission model usable by someone who thinks in sentences, without taking away what an administrator needs.

**Spec:** `docs/en/specs/2026-09-27-permission-ui-sp4b-design.md` (D3–D10). **Backend contract:** SP4b-1, merged as `cd48121e`.

**Architecture:** four pages under Data governance, each with one audience (D4). Two shared pickers carry the knowledge the API cannot express, so it is written once instead of five times.

## Global Constraints

- **The frontend has NO prettier.** Only `eslint .`. Never run `npx prettier --write` — it will reformat the entire file and bury the change.
- **Colors come from semantic utilities**, never hardcoded: `bg-primary/10`, `text-muted-foreground`, `text-destructive`, `border-border`. Raw `oklch(...)` lives only in `frontend/src/index.css`. In the rare inline style, use `var(--token)` directly — **never** `hsl(var(--token))`.
- `--accent` is already used for hover/focus backgrounds in button/badge/dialog/dropdown; do **not** repurpose it. `--highlight` (amber) is for a page's single primary emphasis.
- **Use the established scaffold, do not invent one:** `SummaryStrip` for header stats, `FilterableTable` + `Column[]` (`components/explore/FilterableTable`) for lists, `FacetSelect` for facets, `ListSkeleton` while loading, `ConfirmDeleteDialog` (`components/ui/confirm-delete-dialog`) for anything destructive. Read `routes/control-plane/catalogs/Catalogs.tsx` and `routes/governance/Classifications.tsx` as the exemplars.
- **Consume the orval-generated hooks directly** — `usePermListGrants`, `usePermCreateGrant`, `usePermPatchGrant`, `usePermDeleteGrant`, `usePermListRules`, …, `usePermExplain`, `usePermPreview` — the way the newest Catalogs UI does. Do **not** add a hand-written wrapper layer in `frontend/src/api/`; two live patterns already exist in this codebase and this work follows the newer one.
- **The generated mutator wraps responses:** unwrap with `data.status === 200 ? data.data : undefined`. A hook's `data` is not the payload.
- If any API type changes, regenerate with `npm run gen:api` — **never** bare `orval`, which drops `encodeURIComponent` from generated URLs — and commit exactly what it produces.
- **Do not touch the Engines/Warehouses `StatCard`-versus-`SummaryStrip` question.** It is an open decision the user has not made; new pages use `SummaryStrip`.
- `npx tsc --noEmit` clean and `npx eslint .` clean before every commit. Run the frontend tests: `npx vitest run`.
- Never launch a background task and wait on it. Foreground only.

### The knowledge the API cannot express (D3, D6, D7)

Every one of these is hardcoded in the UI **because the contract cannot carry it**, and each must be written where a maintainer will find it, with the reason:

- `workspace_members` is a real `PrincipalKind` in the generated union but **always 400s** from this surface — it exists for compiler-internal assignment rows. The picker excludes it.
- `group` is accepted and compiles but **enforces nothing on Trino**: SP4a's D7 measured live that Trino has no group provider, egret writes none, and there is no usersync, so a Ranger policy item naming a group matches nobody. The picker offers it **with a warning at the point of choice**, and the comment says to remove the warning when a group provider arrives.
- `exempt.exclude`, and a `role` principal inside `exempt`, are storable per the schema and **always refused** at write time — Ranger's mask and row-filter items have no exception block and no roles bucket. Disabled in the form, not validated after.
- A path-selected **row filter** must name exactly catalog, schema and table. `rowFilterDef` declares only those three levels, all mandatory, and no column.
- `mask_type` is an unconstrained string in the API. The UI ships Ranger's vocabulary as a `Select` with `CUSTOM` revealing an expression field, and the list is a **hand-maintained mirror**.

---

### Task 1: Four reachable pages, and nothing in them yet

**Files:** Create `frontend/src/routes/governance/{DataPermissions,MaskFilterRules,ExplainAccess}.tsx`; modify the router and the sidebar group.

- [ ] Add the three new routes under Data governance alongside the existing Policies, gated `policy:read` the way the governance routes already are. Each page renders its header and an empty state; no data yet.
- [ ] Sidebar labels, in this order: **Data permissions**, **Mask & filter rules**, **Explain access**, **Policies**. Read the existing `CONTROL_PLANE_GROUPS` and follow its shape.
- [ ] **Note the naming hazard and avoid it:** an "Access → Permissions" page already exists for the RBAC permission *catalog*. These pages are about **data** access. Do not reuse the word "Permissions" alone for any of them.
- [ ] Test: each route renders its heading for a `policy:read` user. That is enough for this task — it exists so every later task has somewhere to land.

### Task 2: The two shared pickers

**Files:** Create `frontend/src/components/perm/{PrincipalPicker,ScopePicker}.tsx`.

**Interfaces — later tasks depend on these exact shapes:**
- `PrincipalPicker` edits a `PrincipalSet` (`{include, exclude}`) of `{kind, ref}`, and takes a prop saying whether it is being used for `exempt` (where `exclude` and `role` must be unavailable).
- `ScopePicker` edits a `Scope` (`{catalog_id, schema?, table?, column?}`), and takes a prop for the row-filter mode that requires exactly catalog+schema+table.

- [ ] `PrincipalPicker`: excludes `workspace_members`; offers `group` with the warning above rendered where the kind is chosen, not in a distant help panel; `ref` for a user is the **bare username**, never an email — resolve the display name to it and show which is being stored.
- [ ] Source principals from the existing principal endpoint the raw-JSON editor already uses (`policyAdminListPrincipals`) rather than adding one under `/perm`.
- [ ] `ScopePicker`: catalog from the catalog-registry API (the model stores `catalog_id`, a bare uuid, so the picker resolves the name); then schema, table, column narrowing progressively, each optional except where the row-filter mode forbids it.
- [ ] Tests: `workspace_members` is not offerable; choosing `group` shows the warning; in `exempt` mode neither `exclude` nor `role` can be selected; in row-filter mode a scope missing table cannot be submitted. **Each of these is a test that the control is ABSENT or DISABLED — asserting the form merely reports an error afterwards is the shape this spec rejects.**

### Task 3: Data permissions — a grant reads as a sentence

**Files:** Create `frontend/src/routes/governance/DataPermissions.tsx` (fill in), plus a `GrantDialog`.

- [ ] List on the scaffold. **The sentence is the primary column** (D5): `analyst can read wh.sales.* except column pii`. Render `effect`, `actions`, the resolved scope and the principal subtract as one readable line — the subtract is included because it changes the sentence's meaning.
- [ ] Facets for effect (allow/deny) and action. A `SummaryStrip` with counts.
- [ ] Mark a grant whose `origin` is `workspace_assignment` as one the user did not write, with its source named (D3 of SP4a: never invisible). Such a grant is **not** editable here.
- [ ] Create and edit through one dialog using the two pickers. **`PATCH` is a full-body replace** — the dialog must send every field, pre-filled from the existing grant. A partial body blanks what it omits; the server now refuses empty `principals`, but do not let that 400 be the only guard.
- [ ] **Before committing, call `POST /perm/preview` and show what it reports** — the profiles that would recompile, by name, and the compile error if there is one. This is the confirm step, and it exists because a rule or grant's blast radius is also a failure radius.
- [ ] Delete through `ConfirmDeleteDialog`, naming the grant's sentence rather than its uuid.
- [ ] Surface a refusal verbatim. The backend's messages already name the rule, the profile and the running total for a ceiling; do not replace them with a generic failure.
- [ ] Tests: the sentence renders for allow, deny and a subtract; an assignment-origin grant shows its source and offers no edit; the preview result is shown before the write; a 400 surfaces its message.

### Task 4: Mask & filter rules

**Files:** Create `frontend/src/routes/governance/MaskFilterRules.tsx` (fill in), plus a `RuleDialog`.

- [ ] List on the scaffold: name, kind (mask or row filter), what it selects (a tag, or a path), and the exemptions.
- [ ] A tag selector that carries the tag **key** — the model selects by `key` while Classifications browses by `name` and `id`, so the picker must bridge the two and store the key.
- [ ] Mask kind: the `mask_type` `Select` from the hand-maintained list, with `CUSTOM` revealing the expression field. Row-filter kind: the expression, and a scope in row-filter mode.
- [ ] The `exempt` set uses `PrincipalPicker` in `exempt` mode, so `exclude` and `role` are unavailable.
- [ ] Preview before commit, refusals verbatim, delete via `ConfirmDeleteDialog` — as Task 3.
- [ ] Tests: a tag-selected mask stores the key not the name; `CUSTOM` reveals the expression; a row filter cannot be scoped to a column; the preview of a tag rule shows **more than one profile** where several exist, because the installation-wide fan-out is the reason the preview exists.

### Task 5: Explain access

**Files:** Create `frontend/src/routes/governance/ExplainAccess.tsx` (fill in).

- [ ] A form: subject (user, optionally groups and roles), action, target scope. Result: allowed or not, **the reason class**, and the tags considered.
- [ ] **The reason class is the answer, not the boolean** (D9). `no_grant_matched` and `explicit_deny` are different administrative problems and must read differently; Ranger's own audit shows `-` for the first. A reason carrying a `grant_id` or `rule_id` links to that row — SP4b-1 made the id stable precisely so this link survives an edit.
- [ ] **State the two limits in the UI, where the answer is shown, not in a help page:** `explain` is blind to hand-written policy, so it can answer "denied" for a subject the engine allows through a seeded admin policy; and `tags` populate only when the target names a schema and a table. A tool that looks like a complete oracle while being partial is worse than one that says what it does not know.
- [ ] Tests: each reason class renders distinguishably; a reason with a `grant_id` links to the grant; the limits are stated on the result.

### Task 6: The Policies page finally warns before the overwrite

**Files:** Modify `frontend/src/routes/governance/{Policies,ProfileDetailPage,PolicyEditorPage}.tsx`.

- [ ] `ProfileDto.origin` has carried `generated | manual` since SP4a with a comment saying the frontend must show it, and **nothing has ever read it**. Mark a generated profile.
- [ ] Handle the refusal SP4a added: a write the compiler owns comes back 409, and the message explains the compiler owns that policy. Surface that explanation rather than a bare error — there is no existing 409-handling pattern in `governance/*` to copy, so this establishes it.
- [ ] **Reflect the distinction SP4a deliberately made:** the guard is on the **policy**, not the profile. A foreign policy inside a generated profile stays editable, and freezing the whole profile in appearance would misrepresent that — it is the one-way door SP4a had to reopen.
- [ ] Tests: a generated profile is marked; a 409 renders its explanation; a non-compiler-owned policy in a generated profile is still offered for edit.

### Task 7: Drive it in a browser

Every defect that mattered in this codebase's UI work was found in a browser, not in a review and not by a test.

- [ ] Start the stack, sign in, and drive all four pages: create a grant, edit it, watch the preview, delete it; create a mask rule on a tag; explain a subject that is allowed and one that is denied, recording the reason class each time; open a generated profile and attempt a write the compiler owns.
- [ ] Record the **actual values** seen — the sentence as rendered, the profiles the preview named, the reason class returned, the 409 text. A screenshot of a form is not evidence that a write worked.
- [ ] Anything that fails is recorded as FAILED with what was on screen. Partial honest results are the success condition.
