# Chart Redesign 03 — Table

**Status:** approved design, pending implementation plan
**Date:** 2026-07-28
**Program spec:** `docs/en/specs/2026-07-28-chart-redesign-program-design.md`
**Previous units:** bar (main `1fe27573`), line (main `1ee0819e`)
**Vietnamese reference:** `docs/vi/specs/2026-07-28-chart-redesign-03-table.md`

## The premise this unit started from was wrong

The program comparison called table "the widest gap" and said egret had "no pagination, no
freezing". Reading the code instead of recalling it: `TableState` already carries `pagination`,
`pinnedLeft`, `columnWidths`, `hiddenColumns` and `columnOrder`, and `ChartTable` implements all
five, plus a sticky header and drag-to-reorder on the header row. The program spec has been
corrected.

So this unit is not feature-filling. It is one genuine gap, a UX pass over the panel, and — because
chart 01 and 02 both found their real defects at seams rather than in pure functions — a live
verification of the features that already have code but no evidence anyone has ever run them.

## What a table is for

Reading exact values, and comparing rows by scanning. Everything below follows: a column has to be
identifiable, the panel that configures forty columns has to stay usable, and a table that has not
run yet has to say what it needs rather than apologise generically.

## The one real gap: renaming a column is impossible

`ColumnFormat.column_title` exists in the type. Chart 01 taught the cartesian axis titles to read
it. **Nothing writes it**, and `ChartTable` renders `header: c.name` — the raw column name from the
result set. So a user looking at `total_revenue` has no way to make it say `Revenue`, anywhere.

This unit wires it end to end:

- an input in `FormatSection`'s per-column accordion, which already has the `patch()` path for
  every other `ColumnFormat` field;
- `ChartTable` headers read it;
- the pivot's metric headers read it.

The pivot needs care, and the spec says so rather than letting the plan discover it: pivot receives
`columnSettings` but only uses it to format **values** (`formatValue`). Its metric header text is
built in `pivot/engine.ts` as `{ label: cl.metric }` — a pure function with no access to
`column_settings`. So the rename has to resolve when the header is rendered, not where the label is
built. It is in scope anyway, because a table reading `Revenue` beside a pivot still reading
`total_revenue` is exactly the inconsistency a user notices.

**The rule for the two rename channels**, so this does not become the confusion chart 01's review
flagged: `column_title` renames a **column** wherever a column is named. `seriesLabels` renames a
**series**. On a cartesian chart a metric is both, and the series rename wins because it is the
more specific choice — that precedence already exists in `titleOf` and does not change.

## Standard audit

Held against the program's seven, only where table deviates:

**Standard 5 — empty states that name what this chart needs.** Today the Display tab of an unrun
table says *"Run the chart to configure column formatting."* That is one sentence shared by every
viz, it names formatting rather than the table, and it appears where the column list should be.
Replace it with something that says what a table needs: columns, and a run to discover them.

**Standard 2 — progressive disclosure.** `TableColumnsSection` is three ungrouped blocks: a
checkbox per column, a "Freeze first columns" number input, and a pagination switch. Bar and line
got their grouping from `StudioPanel`'s `ControlGroup`, but the table's descriptor declares **no
display controls at all** — its whole Display tab is shell-owned. So the grouping has to happen
inside the shared section, not through the descriptor.

**Standard 3 — awkward data.** The visible-columns list renders one checkbox per column with no
bound. A forty-column result gives forty checkboxes, and a hundred-column one gives a hundred. The
panel needs to cope with a wide result the way the chart copes with a long tail: a filter box once
the list is long, and Show all / Hide all.

**Standard 6 — both sizes.** All of the above, at a dashboard card's width.

## Verify what already exists, live

Pagination, freeze-first-columns, drag-to-reorder and persisted column widths all have code and
tests. None of them has evidence of ever being exercised in a browser. Chart 01 shipped
`maxCategories` with passing unit tests and a renderer that never received it; chart 02's controls
lied about their own state. Both were only found live.

Each of these is therefore a live check with a stated outcome, and **anything found broken becomes
this PR's work**:

- pagination: page size changes, Prev/Next move, and the page resets when the data changes;
- freeze: the first N columns stay put under horizontal scroll and the tint marks them;
- reorder: dragging a header persists and survives a reload;
- widths: a drag persists, and a removed column does not leave a stale width behind.

## Deliberately not done

- **No frozen rows.** Metabase does not have them either, and the sticky header already solves the
  problem frozen rows would.
- **No change to `TableState`.** It stores the right things; this is a wiring and UX unit. **No
  migration.**
- **No change to `ChartTable`** beyond reading `column_title` and whatever the live check proves
  broken.
- **Column reorder stays direct manipulation** — dragging the header, not a list in the panel. That
  is standard 4 already satisfied, and a second way to do it in the panel would be two sources of
  truth for one order.

## Live check

On the Demo Sales dataset:

1. A table with no configuration: every column, readable headers, no empty-state text.
2. Rename `total_revenue` to `Revenue` in Formatting: the table header changes, and so does the
   axis title of a bar chart built on the same column — one rename, both places.
3. A wide result (many columns): the visible-columns list stays usable; filter and Show/Hide all
   work.
4. Pagination on at 25: Prev/Next move, page resets when the query re-runs.
5. Freeze 2 columns and scroll right: they stay, and the tint marks them.
6. Drag a header to reorder, reload: the order survives.
7. The same table as a small dashboard card.
8. An unrun table: the empty state names what a table needs.

## Verification

The program's gate: lint, typecheck, tests, build, deploy to kind, live check, before/after
screenshots at both sizes. No migration this time, so the backend gate is not in scope unless
something drags it in.
