# The table's column lists — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Stop egret's table from telling a user a column is on while refusing to draw it, and give
its column settings the one-list shape Metabase uses.

**Architecture:** Two PRs. **PR A is correctness only** — a reconcile step plus a pinning-order fix,
no UI restructure. **PR B merges three parallel column lists into one.** A stands alone and ships
first; if B is ever cut, A still closes the bugs.

**Tech stack:** React 18, tanstack table, shadcn/Radix, vitest.

---

## What was measured

An investigation drove **both** apps — Metabase at `localhost:3033` and egret on the kind deployment
(`:30080`, served at `:5174`). Everything in this section is measured, not inferred, except where said.

**egret has THREE column lists, not two** (four on an aggregated table):

| | **A** — Data ▸ "Columns" | **B** — Display ▸ "Visible columns" | **C** — Display ▸ "Formatting" |
|---|---|---|---|
| Component | `TableColumnsControl` (`viz/table.tsx:46-58`) | `TableColumnsSection.tsx` | `FormatSection.tsx` |
| Membership | all dataset columns | result columns | result columns |
| Writes | `TableConfig.columns` → `table.columns`, and `spec.columns`/`spec.dimensions` via `buildSpec` | `table.state.{hiddenColumns,pinnedLeft,rowIndex,pagination}` | `column_settings[name]` |
| Can do | toggle into/out of **the query** | hide/show, Show all / Hide all | drill-in → rename, decimals, currency, mini-bar |

Reorder and resize have **no panel control at all** — both are only reachable by dragging on the
rendered table (`ChartTable.tsx:282` / `:259`).

**A and B are genuinely different jobs** — query projection versus display visibility — and Metabase
has both too. So neither list is redundant. The defect is that they are near-identically labelled,
identically presented as flat checkbox lists, sit in different tabs, and **are never reconciled**.

### The four disagreements

1. **Membership differs.** On an aggregated table, `revenue` appears in B and is absent from A (it
   lives in a separate Metrics list) — four parallel field lists on one chart.
2. **Order differs and the rendered table follows neither list.** `TableColumnsSection` never reads
   `columnOrder` (`:38-42`, `:82`); the table follows `tableState.columnOrder`
   (`ChartTable.tsx:206-213`).
3. **"Freeze first columns" counts in RESULT order, not display order.** `columnPinning.left` is built
   from positional ids `col_0…col_{n-1}` assigned by result index (`ChartTable.tsx:200-204`) while
   `columnOrder` reorders them. Measured: after dragging `state` to position 1, freeze = 1 pinned
   `id`, and because tanstack draws pinned columns first, **the drag was visually undone**.
4. **A column turned back ON in the Data tab silently stays hidden.** Hide `vendor` in B → remove it
   in A → re-add it in A. The Data tab says ON; the table has no `vendor`; `hiddenColumns` still holds
   the stale entry and nothing prunes it. **The user gets no signal that the fix is in another tab.**

Not a defect: hiding in B does not prune the query. Metabase behaves the same. (INFERRED from
`buildSpec` reading `cfg.columns` only — the one claim in this section not driven.)

### What Metabase does

- Sidebar tabs are `Columns | Display | Conditional Formatting`. **One** column list, in **Columns**.
  The Display tab holds only row-index and freeze controls — **no column list**.
- **The row follows the same pattern as its series list** (which egret adopted in #408): identity
  only — type icon and name — plus a drag handle, an eye button and an ellipsis opening a drill-in.
  Hidden rows **stay in the same list** with `data-enabled="false"`; there is no separate hidden list.
- Per-column formatting is in the drill-in popover, rename included.
- Query projection **shares the slot**: an `Add or remove columns` button replaces the list with the
  query field panel and becomes `Done picking columns`. Gated on an editable GUI query.
- `table.columns: [{name, enabled}]` — **one array carrying order and visibility**.
- **The reconciliation egret lacks:** `syncTableColumns`
  (`querying/viz-settings/utils/sync-viz-settings.ts:178-200`) drops settings for removed columns and
  appends new ones as `{name, enabled: true}`. Measured: the same scenario as egret's #4 brings the
  column back **enabled**.

---

# PR A — the two bugs

Correctness only. No new control, no moved control.

## Task A1: reconcile stored column state against the result

**Files:**
- Create: `frontend/src/components/chart/table/reconcileColumns.ts` (or wherever the table's helpers
  already live — check first and follow the existing placement rather than inventing a directory)
- Modify: wherever `resultColumns` is set for a table
- Test: a new `__tests__` file beside the helper

- [ ] **Step 1: Write the failing test** for the measured scenario, as a sequence, not an end state:
  hide `vendor` → drop `vendor` from the query → re-add `vendor` → assert the rendered table has it.
  **Assert after each step**, because the bug is that step 3 does not undo step 1 — an end-state-only
  test can pass against broken code.

- [ ] **Step 2: Run it and watch it fail.** Record the text for the PR body.

- [ ] **Step 3: Implement the reconcile.** Prune `hiddenColumns`, `columnOrder` and `columnWidths` of
  names no longer in the result, and **never resurrect a stale hidden flag for a column that has just
  re-entered**. That second half is the actual fix; pruning alone leaves the bug if the flag is
  re-applied on re-entry.

  Decide and state **where** this runs. It must not fight the user: a reconcile on every render that
  writes settings back would fire on a transient empty result and destroy state. Say what guards it.

- [ ] **Step 4: Run tests. Step 5: Commit.**

## Task A2: freeze against display order

**Files:** `frontend/src/components/chart/table/ChartTable.tsx:200-213`

- [ ] **Step 1: Write the failing test:** with `columnOrder` moving `state` first and `pinnedLeft: 1`,
  the pinned column is `state`. Watch it fail — measured, today it pins `id`.

- [ ] **Step 2: Fix** `columnPinning.left` to count against display order.

- [ ] **Step 3: Count the blast radius before committing.** This is a **behaviour change on an existing
  key**, not a shape change — no migration, but it silently re-points any chart holding both
  `pinnedLeft > 0` and a `columnOrder`. Query the deployment for how many charts hold both and put the
  number in the PR body. If it is not zero, say what those charts will do differently.

- [ ] **Step 4: Gates, commit, open PR A. Do NOT merge.**

---

# PR B — one list

Only after A is merged. Branch off main.

## Task B1: the merged list

**Files:** `TableColumnsSection.tsx` (rewrite as an ordered list), `SharedDisplaySections.tsx`,
`FormatSection.tsx`, `viz/table.tsx`, `ChartExplore.tsx`, `CardVizPanel.tsx`, `ChartTable.tsx`.

- [x] **Step 1: Merge B into C.** C's rows already have the right anatomy — type icon, name, drill-in,
  and `column_title` already shown beside the raw name (`FormatSection.tsx:83-91`). Add an eye toggle
  and a drag handle. **The template is already in this repo**: `SeriesListSection.tsx` +
  `SeriesSettingsPopover.tsx`, shipped in #408. Reuse it rather than writing a second one.

  **CORRECTED while executing PR B: the template has no drag handle, and should not grow one.**
  `SeriesListSection` reorders with a pair of ▲▼ BUTTONS, not a grabber. The merged column list
  copies that, because a button carries its keyboard path with it — PR #356 in this programme
  shipped a column reorder that was mouse-only because a `dnd-kit` list was handed no
  `KeyboardSensor`, and a visual grabber in the panel would be that same bet taken again for a
  cosmetic gain. The rendered header's drag stays and is unchanged; it already has its
  `KeyboardSensor`.

  Also corrected: the "eye toggle" is genuinely an EYE here and not the template's `Switch`.
  Metabase's column row uses an eye, and forty switches down a 300px panel is a different weight of
  control than the series list's ten. `aria-pressed` carries the state so it is not colour-only.

  One thing the plan does not mention and that the merge requires: `FormatSection` still serves
  every OTHER viz, whose formattable columns are a genuine subset of the result. The accordion is
  dropped **for the table only**, and its per-column editor is extracted (`ColumnFormatFields`) so
  both it and the new drill-in stay one editor over one `column_settings` key.

- [x] **Step 2: Keep it in the Display tab.** Do NOT move it to Data — `CardVizPanel` has no Data tab,
  and `ChartTable.tsx:290-292` already records that constraint.

- [x] **Step 3: Give reorder a home in the list.** Today it exists only as a drag on the rendered
  header and is invisible to the panel. Dragging in the list writes the same `columnOrder`.

- [x] **Step 4: Hidden columns stay in the list**, marked, as Metabase's do — not moved to the end and
  not moved to a separate group. Measured behaviour, and it is what makes "turn it back on" findable.

- [x] **Step 5: Gates.** Existing tests WILL move. Every one that does must be understood and
  explained in the PR body — this programme has shipped a passing suite over a dead code path twice.

  **Ten moved, in exactly two files**, and both moves are the same fact: the per-column control
  changed from a checkbox whose `<Label>` was the DISPLAY TITLE to an eye toggle labelled by the RAW
  name. Eight in `TableColumnsSection.test.tsx`, two in PR A's
  `ChartExplore.tableColumns.test.tsx`. One of the eight is inverted rather than translated — the
  old list showed the title INSTEAD of the raw name and the merged row shows both. No other test
  moved.

## Task B2: fold query projection behind a toggle

- [x] **Step 1:** Put A behind an `Add or remove columns` affordance in the same section, Metabase's
  exact shape, so query projection stops being a permanently-visible second checkbox list.

  **DONE — the full affordance, not the escape hatch.** It does not fight `viz/table.tsx` at all,
  because it does not go through the descriptor: `SharedDisplaySections` takes an opaque
  `queryFields: {label, options, selected, onChange}[]` and `ChartExplore` fills it from `descConfig`
  and the dataset. `CardVizPanel` omits it and the affordance is simply absent there, which is
  correct — a dashboard card overrides how a column is DRAWN and cannot change what its query
  selects.

  It carries a group per field list, so the METRICS list is in it too. That is what closes
  disagreement #1, which relabelling could not have: an aggregated table showed `revenue` in the
  display list while it lived in a fourth field list one tab over.

  **The Data tab's own controls STAY, which is a correction to this step's premise.** The plan reads
  as though query projection should stop existing outside the fold. Two things say otherwise. First,
  `TableColumnsControl` anchors the per-column temporal-unit pickers (`unitRow`), which are query
  GRAIN and not display, and which have no home in a display list. Second, Metabase keeps both too —
  its notebook Data step has a field picker AND its viz settings have `Add or remove columns`, over
  the same stored field list. Two entry points to one setting is Metabase's arrangement; what this
  PR removes is two entry points that DISAGREED.

## Task B3: live check and PR

- [x] Drive it: reproduce all four measured disagreements and show each is gone or explain why one
  remains. Then the 19 "ZZ Chart Parity" charts. Screenshots to the session scratchpad, ABSOLUTE paths.

  All four driven on the kind deployment. #1 and #2 NEWLY CLOSED by PR B; #3 and #4 were PR A's and
  were RE-VERIFIED here, #3 specifically against the panel reorder that did not exist when A shipped.
  All 19 parity cards render unchanged.

  The live check found three defects that no test and no review had: a dashboard card's rows all
  badged TEXT (its columns carry no types), the picker's group heading repeating the section's own,
  and a UA-hairline focus ring on the 10px reorder arrows. Fixed in the second commit.

- [ ] Open PR B. **Do NOT merge.**

---

## Out of scope, deliberately

**Do NOT adopt Metabase's single `table.columns: [{name, enabled}]` array.** It is a rename-and-remove
of `hiddenColumns` + `columnOrder`, so it needs a migration for every stored table chart, for no
user-visible gain — egret's two parallel arrays already express the same thing. This plan's whole
storage cost is **zero new keys, zero renamed keys, zero migration**; every key the merged list needs
already exists and is already read and written.

Whether egret's drill-in contents match Metabase's popover field-for-field is a separate parity
question and is not part of this.

## Self-review

- **PR A is the one that matters.** The bugs are user-visible and silent; the list merge is
  presentation. Splitting them means A is not held hostage to a seven-file UI change.
- **A2 is the risky half of A**, not A1 — it changes what an existing stored setting does. Hence the
  explicit count-before-merging step.
- **Known soft spot:** Task A1 Step 3 does not say where the reconcile runs, because the investigation
  did not establish where `resultColumns` is set for a table. Find out before writing it, and if the
  honest answer is that there is no single place, say so rather than sprinkling the call.

  **ANSWERED while executing PR A: there is exactly one place — `ChartExplore`.** `resultColumns` is
  derived in two components (`ChartExplore.tsx:199` from `data.columns`, and `CardVizPanel.tsx:62`
  from its `columns` prop), but only one of them can make a column LEAVE the result: the reconcile
  is needed where the query is editable, and the query is only editable from `ChartExplore`'s Data
  tab. `CardVizPanel` composes the identical display sections with no Data tab, and the drill dialog
  and dataset preview render `ChartTable` with no `TableState` at all. It is guarded on
  `data.hasResult` + a non-empty column list + `viz === "table"`, and the helper returns its argument
  by identity when there is nothing to prune, so the common case is a React bail-out. PR B does not
  need to add a second call site.
- The backend validates nothing here: `crates/egret-bi/src/chartparams.rs:83-91` asserts only that
  `params.settings` is a JSON object, and a test pins that permissiveness. So malformed stored state
  can reach this code and every read needs to tolerate it.
