# Data API UI redesign — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to
> implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the 20 findings of the 2026-08-28 live audit of the Data API console, so the
surface shows state instead of explaining itself in paragraphs.

**Architecture:** No new design system and no rewrite. The answer already ships in this codebase —
`Config`'s `Setting | Details | Value` table (`detail/Config.tsx`) puts each explanation beside the
one control it describes. The other screens adopt that pattern; long caveats move behind
progressive disclosure; copy that is factually wrong gets corrected.

**Tech Stack:** React 18 + TypeScript, shadcn/Radix, Tailwind v4 with OKLCH tokens, Monaco,
vitest + Playwright.

## Global Constraints

- **The project's own conventions govern.** `docs/en/ui-conventions.md` is canonical. Read §1
  before touching a list toolbar, §2 for the three states, §4 for `Setting | Details | Value`.
- **Explore idiom, not control-plane idiom.** `p-4`, one `h1` from `ListPageShell`, `text-lg`,
  `+ Noun` primary buttons with no icon. The control-plane's `text-2xl` + `size="sm"` + `<Plus/>`
  idiom is a different section — do not copy it in.
- **OKLCH tokens only.** In inline styles use `var(--token)` directly, never `hsl(var(--token))`.
  Never introduce a raw Tailwind palette colour; `--highlight` is amber, `--primary` is teal,
  `--accent` is already taken by hover/focus backgrounds and must not be repurposed.
- **Do not swap the Metabase-lifted chart palette.**
- **Frontend has NO prettier.** Run `npx eslint .` only. Never `npx prettier --write`.
- Gates: `npx vitest run && npx tsc --noEmit && npx eslint .` → green, tsc 0,
  **eslint 177 warnings / 0 errors**. Report the exact numbers.
- **jsdom computes no layout.** Any finding about size, overflow or position needs a real-browser
  test (`frontend/tests/e2e/data-api-script-editor-height.spec.ts` is the precedent), never a
  jsdom assertion.
- `toBeDisabled()` walks up to an ancestor `fieldset[disabled]`, so a guard test can pass with the
  guard deleted. `toBeUndefined()` passes on a never-set field. A bare `vi.fn()` spy cannot
  accumulate `userEvent.type` into a controlled field.
- Commit before mutating; confirm every mutant builds before believing it.

---

### Task 1: Copy that is factually wrong

**Files:**
- Modify: `frontend/src/routes/explore/dataapi/detail/ParamEditor.tsx:372`
- Modify: `frontend/src/routes/explore/dataapi/detail/Versions.tsx:95`
- Modify: `frontend/src/routes/explore/dataapi/Endpoints.tsx`, `Clients.tsx`, `Activity.tsx`
- Test: the existing `__tests__` beside each

**Interfaces:** none — copy only.

The audit found two strings that state something untrue, and three that read as unfinished.

1. **`ParamEditor.tsx:372` teaches `:name`.** The Data API renders through minijinja and only
   understands `{{ name }}` — `crates/egret-dataapi/src/render.rs` contains no `:name` handling at
   all, and its module doc (`render.rs:17-45`) records `{{ … }}` as the only path. An author
   following the current hint is refused at publish for an unreferenced parameter. Replace with
   `{{ name }}`. **Check the surrounding sentence too** — it says parameters are "read from the
   query string", which stays true.
2. **`Versions.tsx:95` names a section that does not exist.** It says "the draft sitting in
   Definition"; the six labels in `EndpointDetail.tsx:127` are Query, Config, Response, Access,
   Try it, Versions. Name the real one.
3. **Pluralisation.** "1 endpoints total" and "1 service accounts" both render. Fix at the call
   sites; do not add an i18n library.
4. **Two wordings for one empty state on Activity**: "No calls in this range." and "No calls in
   range." Pick one and use it in both places.

- [ ] **Step 1: Write the failing tests**

Assert the rendered text, not the variable:

```tsx
it("tells the author the placeholder syntax the renderer actually accepts", () => {
  // render the param editor with zero parameters
  // expect the empty state to contain "{{ name }}" and NOT ":name"
});

it("names a section that exists", () => {
  // render Versions; the subtitle must not contain "Definition"
});

it("counts one endpoint without an s", () => {
  // one row -> "1 endpoint", never "1 endpoints"
});
```

- [ ] **Step 2: Run them and confirm each fails for its own reason**
- [ ] **Step 3: Implement**
- [ ] **Step 4: Prove they bite** — revert each string one at a time, confirm the build is clean, re-run, restore
- [ ] **Step 5: Commit**

---

### Task 2: A disabled control must be visibly disabled in both themes

**Files:**
- Modify: `frontend/src/components/ui/button.tsx`
- Test: `frontend/src/components/ui/__tests__/button.test.tsx` (create if absent) + a real-browser check

**Interfaces:** Produces the disabled treatment every later task relies on.

Disabled is currently encoded by **opacity alone**. Measured on the deployed build: the endpoint
detail Save button reports `disabled=true`, `opacity=0.5`, background unchanged. Against white,
half-opacity teal washes out and reads unavailable. Against near-black it stays vivid and reads
**ready** — so a user cannot see that Save does nothing.

**Do not simply lower the opacity further** — that fails the other direction in light mode and
pushes contrast under 4.5:1. Change the *hue and weight*, not only the alpha: a disabled primary
should lose its fill and read as a muted, bordered control in both themes.

**`disabled:pointer-events-none` was deliberately removed app-wide** (9 sites) so a disabled
control can still be hovered for its tooltip. Do not reintroduce it.

This file is shared by the whole app, so verify the change on at least one control-plane page and
one Explore page as well as Data API.

- [ ] **Step 1: Write the failing test** — assert the disabled variant carries a token-based
      treatment beyond opacity; render *outside* any `fieldset` so the ancestor walk cannot satisfy it
- [ ] **Step 2: Run to verify it fails**
- [ ] **Step 3: Implement**
- [ ] **Step 4: Measure contrast in a real browser, light and dark, and record the numbers**
- [ ] **Step 5: Commit**

---

### Task 3: Prose moves beside the control it describes

**Files:**
- Modify: `frontend/src/routes/explore/dataapi/detail/Query.tsx`, `TryIt.tsx`, `AccessSection.tsx`
- Reuse: the `ConfigTable` / `Setting | Details | Value` primitives already used by `Config.tsx`
- Test: the existing `__tests__` beside each

This is the audit's central finding. Measured prose against the interactive content it explains:

| Screen  | Prose on screen              | What it explains          |
|---------|------------------------------|---------------------------|
| Try it  | ~950 chars in 3 blocks       | 1 dropdown, 1 Run button  |
| Access  | ~430 chars before row 1      | 1 table row               |
| Query   | 3 blocks around the editor   | 1 editor, 1 checkbox      |
| Config  | short, per row, in a column  | 9 settings — **the model** |

**The rule:** an explanation that describes one control belongs in that control's `Details` cell,
capped at about two lines. An explanation that describes a *consequence of acting* belongs behind
progressive disclosure — a popover or a `<details>` — not permanently on screen.

Specific moves:

- **Try it**: the 314-character Details cell for `Service account` is longer than the rule allows.
  Keep the first sentence; move the rest behind a disclosure. The ~500-character trial-consequences
  box becomes a popover on the Run control. Delete nothing — every sentence is true and was written
  because a reviewer needed it; relocate it.
- **Access**: the page-level info box and the `Service accounts` paragraph say overlapping things
  about invoke-right versus data-access. Merge into one, and move it beside the `Invoke` column
  where the distinction bites.
- **Query**: the mode-switch caveat is three lines above the editor. Compress to one line beside
  the checkbox with the rest behind a disclosure. The Preview box moves to a popover on Preview.

**A `<details>`/`<summary>` is exempt from `fieldset[disabled]`; a Radix collapsible is not** — on a
trashed endpoint a Radix disclosure becomes impossible to open, and collapsing is reading, not
editing.

- [ ] **Step 1: Write the failing tests** — the sentence still reachable, not merely still present
- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement**
- [ ] **Step 4: Prove they bite**
- [ ] **Step 5: Commit**

---

### Task 4: Clients — a table, not a table inside a cell

**Files:**
- Modify: `frontend/src/routes/explore/dataapi/Clients.tsx`
- Test: `frontend/src/routes/explore/dataapi/__tests__/Clients.test.tsx`

The worst-shaped screen in the surface. One cell currently nests a variable-length list, each entry
carrying a name, a 36-character UUID, a status badge, a last-used time and an action, with no
columns — so nothing aligns down the page.

Four changes:

1. **Give clients their own rows.** A service account is a group header; its clients are rows under
   it with real columns: Client · ID · Status · Last used · actions. Follow the sibling BI lists'
   hand-rolled table, **not** `FilterableTable` — it cannot model grouping, and using it makes the
   table visibly different (bold dark headers vs muted, `border-b` vs `border-t`,
   `hover:bg-muted/50` vs `hover:bg-accent`). The user has spotted that difference before.
2. **Revoke becomes a real destructive control** with a confirmation, not body text sitting between
   timestamps. Reuse `ConfirmDeleteDialog`.
3. **Truncate the client id and make it copyable.** Nobody reads a 36-character UUID, and it
   currently dominates the column.
4. **One noun.** The sidebar says *Clients*, the table says *service accounts*. They are genuinely
   different things — an account holds clients — so say that once, in the page description, rather
   than letting the labels read as disagreeing synonyms.

Add a `SummaryStrip` so the page matches its sibling `Endpoints`.

- [ ] **Step 1: Write the failing tests** — grouping, the confirm step, and that a revoked client is
      not encoded by strikethrough alone
- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement**
- [ ] **Step 4: Prove they bite**
- [ ] **Step 5: Commit**

---

### Task 5: Activity — one honest empty state, and filters that can act

**Files:**
- Modify: `frontend/src/routes/explore/dataapi/Activity.tsx`, `ActivityCallList.tsx`
- Test: the existing `__tests__`

1. **Filters render over zero rows**, against this repo's own rule — `docs/en/ui-conventions.md`
   §1.2: *"a filter or a bulk mode over zero rows is a control that cannot do anything."* Gate the
   filter row on `hasData`. A *time range* is a view switch, not a filter, so it stays.
2. **Four metric cards all showing an em dash** is an empty dashboard, not an empty state. When the
   range has no calls, show one empty state that says so and offers the next action.
3. **The two full-width footnotes** — the 90-day retention note and the "rows can sum to less than
   the total" note — move behind an info affordance beside the control each describes.
4. **`isError` must not fold into empty** (§2.2). Check the current branches; a failed fetch must
   not render as "No calls in range."

- [ ] **Step 1: Write the failing tests** — including an error-state test that fails if the branch folds
- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement**
- [ ] **Step 4: Prove they bite**
- [ ] **Step 5: Commit**

---

### Task 6: Width discipline, the create form, and the rate-limit sentinel

**Files:**
- Modify: `frontend/src/routes/explore/dataapi/detail/ResponseSection.tsx`,
  `detail/Config.tsx`, `EndpointCreate.tsx`
- Test: the existing `__tests__` + a real-browser width check

1. **Response stacks four content widths** — 575px, 510px, 1020px, 530px inside a 1600px viewport,
   leaving a ragged edge and ~280px of permanent gutter. Put every block on one measure.
   `ui-conventions.md` §5 already decided width for routes that host settings tables; follow it
   rather than inventing a new cap.
2. **`EndpointCreate` gives SQL a bare `<textarea>`** while the same content is a Monaco editor once
   the endpoint exists. Use `components/sqllab/SqlEditor.tsx` — it takes
   `{value, onChange, schemaWords, schema?, onRun?, onSelectionChange?}`. Schema-aware completion is
   deliberately absent here: SQL Lab's `words` come from its explorer via a `SchemaSink`, not a
   reusable hook, so pass what is available and do not invent a hook.
3. **The create page's back link sits top-right against the primary submit.** Every detail screen
   puts `← Endpoints` top-left. Move it, so an escape is not adjacent to a commit.
4. **A rate limit of `0` does not say whether it means unlimited or blocked.** Establish which it is
   from the backend before writing the copy — do not guess — then say it in the `Details` cell and
   as a placeholder.

- [ ] **Step 1: Write the failing tests**
- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement**
- [ ] **Step 4: Prove they bite** — the width claim needs a real browser
- [ ] **Step 5: Commit**

---

### Task 7: Endpoints list — stop showing machinery the workspace is not using

**Files:**
- Modify: `frontend/src/routes/explore/dataapi/Endpoints.tsx`
- Test: the existing `__tests__`

1. **A single endpoint is filed under a collapsible "Uncategorized (1)" group**, introducing folders
   to a workspace that has none. Render the group header only when a real folder exists.
2. **Four unlabelled icon buttons** sit together in the header — bookmark, bulk, folder, trash —
   distinguishable only by glyph, with the destructive one un-separated from the view controls.
   They already come from `ListToolbarButton` (§1.3); give them accessible names and separate the
   destructive one. Do not convert them to text buttons: §1.2 fixes the control order and the
   Explore idiom keeps these compact.

- [ ] **Step 1: Write the failing tests**
- [ ] **Step 2: Run to verify they fail**
- [ ] **Step 3: Implement**
- [ ] **Step 4: Prove they bite**
- [ ] **Step 5: Commit**

---

### Task 8: Live check

**Every defect that mattered in this programme was found in a browser.** The audit this plan
implements was itself a browser pass, and the suites were green over all twenty findings.

Start the stack against a **throwaway** database — the dev `egret` database cannot boot the server
and **must not be repaired**. Credentials `admin@egret.local` / `Admin12345!`. **Login is
rate-limited 5 per 5 minutes — log in once.** Confirm the server is built from this branch by
grepping the binary for a string only this branch has.

**Seed enough data to exercise density.** The audit ran against one endpoint, one service account
and zero calls, so grouping, pagination, bulk selection and long-list behaviour were never
exercised — and density is where a list page usually fails. That gap is the single most important
thing this task closes.

Walk every screen the audit covered, in light and dark, and report what you **saw**:

1. Endpoints — with enough rows to group and paginate
2. Clients — with several accounts, several clients each, at least one revoked
3. Activity — with calls, errors, and an empty range
4. All six detail sections, plus the create pages
5. A narrow viewport, since several findings concern width

Fix what the browser finds. A layout claim needs a real-browser test.

- [ ] **Step 1: Run every gate** (`npx vitest run && npx tsc --noEmit && npx eslint .`)
- [ ] **Step 2: Seed, then walk the screens**
- [ ] **Step 3: Commit whatever the browser made you fix**
