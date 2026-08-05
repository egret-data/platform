# Where the panel and the chart disagree about what is real — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Remove three controls no chart reads, add controls for four settings every chart already
honours, and pin four behaviours that a comment claims and no assertion holds.

**Architecture:** Three independent groups, all in the settings layer. **Group A removes** a dead
control from 17 of 18 viz types. **Group B adds** controls for settings the formatter already reads.
**Group C is tests only** — no production change.

**Tech stack:** React 18, vitest.

---

## Where these came from

A three-way audit at `1d9b0d30` plus the mutation sweep that ran beside it. Findings labelled
MEASURED were driven or executed; the rest were read. **Line numbers below were correct at
`1d9b0d30` and PRs #410, #412 and #413 have since moved this code — locate each by content, not by
line, and say so if anything has drifted.**

The unifying shape: **a control and the chart disagree about what exists.** Group A is a control with
no reader; Group B is a reader with no control — the codebase names this defect itself, twice
(`FormatSection.tsx` on `scale`/`number_separators`, and "the same shape of defect that cost the pie
four settings"). Group C is a comment with no test.

---

# Group A: "Display as / Link text / Link URL" is dead on 17 of 18 viz types

**Offered:** `settings/FormatSection.tsx`, in the `!isNumericType(c.type) && !isTemporalType(c.type)`
branch — a `Display as` select (`auto/text/link/email/image`) plus `Link text` and `Link URL` inputs
when `link` is chosen.

**Mounted:** `settings/SharedDisplaySections.tsx`, for **every viz except `table`**.

**Read:** `ChartTable.tsx:68-82` **only**. `format.ts:146` states it outright —
`view_as: false, link_text: false, link_url: false`.

So a pivot row-dimension, a cartesian x-axis, a pie dimension, a sankey node column and an
object-detail field all show a "Display as → Link" select and two template inputs, all of which save
and none of which paints anything.

**The fix already exists in this file and is already in use.** `show_mini_bar` had exactly this defect
and was closed with an `allowMiniBar` prop; `SharedDisplaySections` passes `allowMiniBar={false}` for
every non-table viz, with a comment explaining that `false` is flat rather than conditional because
the table's own switch now lives in the drill-in. **The identical gate was simply never extended to
`view_as`.**

`FormatSection.tsx:52-55` and `:142-145` already argue the principle — *offering a setting that cannot
take effect is worse than not offering it* — while the three controls below them do exactly that.

- [ ] **Step 1: Write the failing test** — render the section for a non-table viz and assert `Display
  as` is absent; render it for the table's drill-in and assert it is present. Watch it fail.

- [ ] **Step 2: Add the gate**, mirroring `allowMiniBar` exactly. Do not invent a second mechanism.

- [ ] **Step 3: Decide what happens to already-stored values** and say which you chose. A chart that
  saved `view_as: "link"` on a pie dimension now has a setting with no control. Since nothing ever
  read it outside the table, leaving it stored is harmless — but say that rather than leaving it
  unexamined, and check `ColumnSettingsPopover` (#410) still offers it where it IS read.

- [ ] **Step 4: It reaches dashboard cards too.** `DashboardView.tsx:340-341` gives dimension columns
  `type: ""`, which falls through `isNumericType`/`isTemporalType` into precisely this branch. Verify
  the gate covers that path.

- [ ] **Step 5: Gates, commit.**

# Group B: four settings the formatter honours with no control anywhere

## B1: conditional formatting cannot set a custom range

**Read:** `conditional.ts:91-97` fully honours `min_type` / `max_type` (`"custom"` and `"all"`) and
`min_value` / `max_value`.

**Written:** `settings/RuleEditor.tsx` hard-codes `{ min_type: null, max_type: null }` when a range
rule is created and offers **no control at all** for any of the four.

Metabase offers all of it — `ChartSettingsTableFormatting/RuleEditor.tsx:203-261`, "Start the range
at" / "End the range at" with a Custom-value input.

- [ ] Failing test, then the controls. **Read Metabase's widget before designing egret's** — the
  programme's rule is copy the RULE, not the value, and its option set (`all` vs `custom`) is the rule.

## B2: two operators no dropdown can produce

`conditional.ts:33-34` implements `is-true` / `is-false` in `matchOperator`, and `RuleEditor.tsx`
lists them in `NO_VALUE_OPS` — but neither `NUMERIC_OPS`, `STRING_OPS` nor `NULL_OPS` ever offers
them, so no dropdown can produce one.

- [ ] Establish whether egret has a boolean column type that would justify a `BOOLEAN_OPS` list. **If
  it does not, the honest fix is to DELETE the dead implementation rather than add a control for a
  type that cannot occur** — say which and why.

## B3: three date/time format fields with no control

`format.ts:102` reads `date_separator`; `format.ts:105-108` reads `time_enabled` and `time_style`.
`CONSULTED_BY_FORMAT_DATE` marks all three `true`. No settings component writes any of them —
`FormatSection`'s temporal branch offers only `date_style` and `date_abbreviate`.

Metabase offers all three as column widgets: `visualizations/lib/settings/column.ts:177`
(`date_separator`), `:214` (`time_enabled`), `:230` (`time_style`).

- [ ] Failing tests, then the controls, in the temporal branch. **`time_enabled` gates `time_style`** —
  check Metabase's own `getHidden` rather than guessing the dependency.

- [ ] **These land in `ColumnSettingsPopover` too** (#410 made the table's formatting a drill-in), so
  verify both surfaces.

# Group C: four assertions that survive the full suite

**MEASURED.** A mutation sweep ran 64 mutations across 12 source files, scored against 15 of 145 chart
test files; 55 of 59 scoreable mutants died. These four survived **all** tests, and all four share one
shape: **a behaviour a comment explicitly claims and no assertion pins.**

| mutation | what it breaks |
|---|---|
| `TableColumnsSection` reorder arrows: `disabled={!!q \|\| i === 0}` → `disabled={i === 0}` | under an active search, "up" swaps a visible row with one off screen — the comment says exactly this |
| `TableColumnsSection` type icon: `c.type ? colIcon(c.type) : null` → `colIcon(c.type)` | an unknown type renders a guessed icon; the comment argues at length it must render an ABSENT type. Only the badge half is pinned |
| `TableColumnsSection` freeze: `Math.max(0, Math.min(n, columns.length))` → `n` | freeze accepts a count past the column count |
| `options/pie.ts` sliceOrder: `if (level === 0 && cfg.sliceOrder?.length)` → `if (cfg.sliceOrder?.length)` | sliceOrder applies to inner rings; the comment states it is ring-0-only |

- [ ] **Step 1: For each, write the test the comment implies**, run it against the mutated code, and
  confirm it fails. The audit named the minimal shapes: type into the search box and assert `Move X
  up` is `toBeDisabled()`; assert the untyped row renders no `svg` in the icon slot; fire `change`
  with `"99"` on a 2-column fixture and assert the emitted `pinnedLeft`; a two-ring fixture asserting
  an inner ring keeps `buildSlices` order under a `sliceOrder`.

- [ ] **Step 2: This group changes no production code.** If writing a test reveals the comment is
  wrong rather than unpinned, **fix the code and say so** — that is a finding, not a detour.

---

## Task: live check and the PR

- [ ] **Step 1: Gates**, then drive it. If you build, **check the loaded bundle hash** against
  `ls -t frontend/dist/assets/index-*.js` — the scratchpad `serve.py` serves `index.html` with no cache
  headers, so the first reload after a rebuild keeps the OLD bundle.

- [ ] **Step 2: Group A** — confirm `Display as` is gone from a pivot / cartesian / pie panel and
  still present where the table reads it. **Check a dashboard card too** (Step 4 above).

- [ ] **Step 3: Group B** — set each new control and confirm the rendered output changes. A control
  that saves but does not paint is the defect this plan exists to remove; do not add four more.

- [ ] **Step 4:** the 19 "ZZ Chart Parity" charts, no regressions.

- [ ] **Step 5: Open the PR. Do NOT merge.**

---

## Out of scope, deliberately

The light-theme focus ring at 2.58:1 — #412 established it is **product-wide** (34 call sites across
26 files, reaching notebook, SQL Lab and the control plane, only 5 files under `components/chart/`).
Its own PR, and arguably its own reviewer.

Also out: the trend's number overflowing a narrow card and a combo's left axis title reading a hidden
series' name after a legend click (both found in #413/#412); a keyboard route to per-cell drill, which
`ChartTable`, `PivotTable` and object detail all lack equally; and the Metabase-side gaps (scatter's
Bubble size and value-axis block, trend's Comparisons, the Compact label mode, Unpin from zero, pie's
Show total).

## Self-review

- **Group A removes a control and Group B adds four.** That is not a contradiction — it is the same
  rule applied in both directions: the panel should offer exactly what the chart reads.
- **Group C is the cheapest and the most likely to be skipped**, which is why it is a named group
  rather than a footnote. All four are one test each.
- **B2 may be a deletion, not an addition.** Resist the pull to add a control for symmetry; if no
  boolean column can reach `matchOperator`, the implementation is the dead thing.
- **Known soft spot:** Group A assumes `view_as` is read *only* by `ChartTable`. That came from a
  code read, not a run. Verify it before deleting the control from 17 viz types — if some other
  renderer consults it, the gate is wrong and the finding changes shape.

---

## What executing this plan corrected in it

Recorded here rather than by editing the sections above, so the plan and what it turned out to be
stay legible side by side.

- **"17 of 18 viz types" is 18 of 19.** `VizType` has nineteen members —
  `ChartRenderer.clickCoverage.test.tsx` asserts exactly that — and `table` is the only one whose
  renderer reads `view_as`, so the dead control was mounted on eighteen.

- **Group A's known soft spot held, and was checked by RUNNING.** A throwaway probe rendered all
  nineteen viz types through `ChartRenderer` with `view_as` / `link_text` / `link_url` behind
  getters that recorded a stack on every access. `table` was the only type whose stacks reached a
  consuming read (`ChartTable.renderTableCell`). Every other access was an object SPREAD copying the
  format into a new one bound for `formatValue` — `cartesian.ts`'s `{ ...col, compact }`,
  `scalar.ts`'s `compactifyValue`, `boxplot.ts`'s `fmt` — or a `JSON.stringify` memo key in
  `ChartRenderer`. The probe is not committed (it asserted nothing); it became
  `__tests__/viewAsIsTableOnly.test.ts`, which pins the two things that keep the conclusion true:
  `formatValue` is blind to all three fields, and no production file outside a named allow-list
  mentions them in code.

- **B2 is an ADDITION, not a deletion — established live.** Running
  `SELECT true AS is_paid, CAST(1 AS TINYINT) AS tiny, 'x' AS label` through SQL Lab on the kind
  deployment returned `{"name":"is_paid","type":"boolean"}` with the cell value JSON `true`.
  `ColumnMeta.type` is the engine's own type name, and neither `isNumericType` nor `isTemporalType`
  claims `boolean`, so such a column is already offered to a conditional-formatting rule and lands
  on the STRING operator list. The implementation in `matchOperator` is reachable data with no
  dropdown; `BOOLEAN_OPS` is the missing half. (StarRocks is an asymmetry, not a counter-example:
  its connector reports the MySQL protocol type, so a StarRocks `BOOLEAN` arrives as
  `MYSQL_TYPE_TINY` and `isNumericType` matches it on `tiny` — the same limitation `typegen.rs`
  already records.)

- **B3's stated dependency is right, but not because Metabase says so.** The plan says to check
  Metabase's `getHidden` for `time_enabled` gating `time_style`. Measured on the running reference at
  :3033: with `time_enabled` set to `null`, Metabase still renders the `time_style` radios, and it
  renders the `date_separator` radios whatever the date style is. egret gates both anyway —
  `formatDate` reads `time_style` only inside `if (fmt.time_enabled)`, and applies `date_separator`
  only as `dateFmt.replace(/\//g, …)` — because an ungated control here would be exactly the defect
  Group A removes. The option sets and labels are Metabase's; the visibility rule is this PR's.

- **Found while wiring B2, and fixed: the operator select could show nothing.** A new rule is minted
  with `operator: ">"` (`ConditionalFormattingSection`), which is not in `STRING_OPS` — so choosing a
  text column left the Select with a value none of its options carried, which Radix renders as an
  EMPTY trigger, and saving stored a `>` that `matchOperator` then evaluated as a string comparison.
  Pre-existing; `BOOLEAN_OPS` would have added a third way in. `RuleEditor` now falls back to the
  first operator the chosen columns can express, and what the trigger reads is what `onSave` writes.

- **All four Group C comments turned out to be TRUE, not wrong.** Each mutation was applied and each
  new test failed against it; no production change was needed for that group.
