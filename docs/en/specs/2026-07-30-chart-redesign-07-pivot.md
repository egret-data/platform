# Chart Redesign 07 — Pivot Table

**Status:** approved design, pending implementation plan
**Date:** 2026-07-30
**Program spec:** `docs/en/specs/2026-07-28-chart-redesign-program-design.md`
**Parity rules:** `docs/en/plans/2026-07-30-chart-redesign-metabase-parity-rules.md`
**Previous unit:** scalar `2ae3f6b1`
**Vietnamese reference:** `docs/vi/specs/2026-07-30-chart-redesign-07-pivot.md`

## The program comparison has no pivot row

Chart 03 corrected a wrong table row, chart 04 a wrong gauge row, chart 05 a wrong pie row. Pivot
has no row at all — the comparison covered 4 of 19 types. So this spec was written by reading
**both** implementations end to end and running both apps, not by reading Metabase and recalling
egret. Everything below is cited to a file or a measurement taken on 2026-07-30.

---

## The blocker: a pivot with a time bucket cannot run at all

Building the most ordinary pivot there is — region × category down the side, a date across the top,
grouped by month — returns:

```
Error: engine: line 2:164: mismatched input '('. Expecting: ')', ',', '.'
```

**Every pivot with a temporal unit fails.** Not a rendering fault: the query never executes. And
the studio *defaults the unit to `Month`* the moment a date column is dropped into Columns, so a
user meets this on their first attempt.

### Isolated, not guessed

Five specs run against the live backend, varying one thing at a time:

| # | spec | result |
|---|---|---|
| A | plain `GROUP BY` + `date_trunc('month', …)` | **200 OK** |
| B | `rollup_groups`, no temporal unit | **200 OK** |
| C | `rollup_groups` + month on a **rolled-up** dimension | **502** |
| D | `rollup_groups` + month on a dimension **not in any rollup group** | **502** |
| E | `rollup_groups` + `day_of_week` (an extract, not a truncation) | **502** |

**D is the one that identifies it.** The bucketed column is not in any grouping set, yet the query
still fails — so the fault is not in `GROUPING SETS`. It is in the marker column emitted for
*every* dimension (`crates/egret-bi/src/chartquery.rs:401-404`):

```rust
for (i, e) in dim_exprs.iter().enumerate() {
    select.push(format!("GROUPING({e}) AS __grp_{i}"));
}
```

With a unit, `e` is `date_trunc('month', "order_date")`, so this emits:

```sql
GROUPING(date_trunc('month', "order_date"))
```

**`GROUPING()` takes column references, not function calls.** Trino's parser reads `date_trunc` as
the start of a qualified name and then finds `(` — which is exactly what
`Expecting: ')', ',', '.'` says. E confirms it generalises past `date_trunc`: any expression form
breaks it, so this is about *expressions in `GROUPING()`*, not about truncation.

### The fix: bucket in a subquery, group on the alias

`GROUPING()` needs a plain column, so give it one. Compute every dimension expression in an inner
`SELECT`, then group and mark on the aliases:

```sql
SELECT "region", "order_date", sum(...) AS "total_revenue",
       GROUPING("region") AS __grp_0, GROUPING("order_date") AS __grp_1
FROM (SELECT "region", date_trunc('month', "order_date") AS "order_date", ... FROM t) AS __b
GROUP BY GROUPING SETS (("region", "order_date"), ("region"), ("order_date"), ())
```

This is portable — it removes expressions from `GROUPING()` **and** from `GROUP BY GROUPING SETS`,
so it also protects StarRocks, which is fussier than Trino here
([[egret-starrocks-vs-trino-sql]]). It is a backend change, so unlike chart 06 the backend gate is
in scope and the backend must roll before the frontend.

### Why no test caught it

`chartquery.rs` tests both features — four with a non-empty `rollup_groups`, one with a real
temporal unit — and **not one that has both**. Every rollup test passes
`temporal_units: HashMap::new()`. The bug lives exactly in the empty cell of that 2×2 matrix.

That is the process finding of this unit: *features are tested individually and their intersection
is not*. The plan's test task is a matrix, not a list.

---

## What each side actually has

Read from `pivot/engine.ts` + `pivot/PivotTable.tsx` + `viz/pivot.tsx` (547 lines) and Metabase's
`visualizations/PivotTable/*` (3,032 lines) plus `lib/data_grid.js`.

### Already at parity — do not rebuild these

egret's pivot is far better than the "unpolished" framing suggests. All of this exists and works:

| | |
|---|---|
| row/column/value zones | `FieldZone`, drag-ordered |
| nested row groups with subtotals | `Totals for X`, `emitChildren` |
| grand totals row | `kind: "grand"` |
| row-totals column | `showRowTotals` → a `Total` column group |
| condense duplicate totals | `condense`, same default as Metabase (`true`) |
| collapse/expand row groups | `+`/`–` toggle, persisted as `collapsedRows` |
| sort a row dimension | click the header, cycles asc/desc |
| conditional formatting | `buildConditional`, **single and range** |
| per-column titles + number formats | `column_settings`, honoured in headers, cells and CSV |
| drill from a value cell | `pivotCellDrill` → row+col dims |
| sticky headers and first column | `sticky` + z-index layering |

Measured live: 2 header levels, metric sub-headers, 21 body rows, 4 collapse toggles, subtotal
bands, 12px, all correct.

### Real gaps, each verified against code

| Gap | Metabase | egret | Evidence |
|---|---|---|---|
| **Time buckets** | works (card 10 shows `Q2 2025 … Q4 2026`) | **query fails** | the blocker above |
| **Virtualisation** | `react-virtualized` `Grid` + 2 `Collection`s + `ScrollSync`; 111 cells in the DOM for a 48-row result | every cell in the DOM | `PivotTable.tsx:15`; measured |
| **Column resize** | `pivot_table.column_widths` (`leftHeaderWidths`, `valueHeaderWidths`), dnd-kit, persisted — **card 10 ships resized columns** | none | `settings.ts:188`, card 10's settings |
| **Per-dimension subtotals** | `pivot_table.column_show_totals`, per row dimension, defaults on for all but the last | one global `showColTotals` | `settings.ts:280-322` |
| **Smart default split** | partitions by `distinct-count`: ≤3 dims → 1 column + rest rows; ≥4 → 2 columns | user drags every field | `settings.ts:113-134` |

### Where egret is ahead, and should stay

- **Conditional formatting is richer.** Metabase's pivot `isValid` demands `type === "single"` and
  forbids `highlight_row` (`settings.ts:218-240`), yet its own card 10 stores a `range` rule —
  its code and its sample data disagree. egret supports single *and* range on pivots already.
- **Sorting is a click, not a settings panel.** Metabase hides sort direction behind a per-column
  setting widget; egret cycles it from the header. Keep egret's.

### Deliberately not copied

- **`GROUPING()` semantics are not Metabase's problem to solve.** Metabase pivots client-side from
  a flat result; egret pushes rollup into SQL. Different architecture, and egret's is the reason
  its pivots survive large results — the fix belongs in egret's SQL, not in a port.
- **Metabase's `column_split` field-ref legacy format** (`settings.ts:294-309` carries both column
  names and field refs, with a migration). egret has only ever stored names. Nothing to copy.

---

## Scope

**T1 — the blocker (backend).** Subquery-wrap dimension expressions whenever `rollup_groups` is
non-empty, so `GROUPING()` and `GROUPING SETS` only ever see plain columns. Plus the missing test
matrix: {no rollup, rollup} × {no unit, truncation unit, extraction unit} × {Trino, StarRocks}.

**T2 — per-dimension subtotals.** `pivot.showColTotals` becomes per-row-dimension, defaulting to
on for every dimension except the last (Metabase's rule). The global switch stays as the "all/none"
shortcut so nothing regresses.

**T3 — column resize, persisted.** `pivot.columnWidths`, applied to left-header columns and value
columns, dragged from a handle. This is the one Metabase setting its own sample dashboard actually
ships, so it is not a hypothetical.

**T4 — virtualisation, only if measured to matter.** Row-window the body when the pivot exceeds a
threshold. **Gated on a measurement, not assumed**: build a pivot large enough to hurt, record the
numbers, and only then decide. Chart 05 spent 40 commits on a rewrite that a measurement would have
scoped first.

**Not in this unit:** smart default split (it changes what a user's first pivot looks like and
deserves its own discussion), and any change to sorting or conditional formatting.

---

## Live check

1. Region × category × month — **the case that fails today** — returns and renders.
2. Every temporal unit, including an extraction one (`day_of_week`).
3. A pivot with no unit is byte-identical to `main`.
4. Subtotals off for one dimension, on for another.
5. Resize a left-header column and a value column; reload; widths persist.
6. Collapse/expand still works after resize.
7. Drill from a cell carries both row and column dimensions.
8. A large pivot: record rows, DOM nodes, and time to first paint before and after T4.
9. Both themes; a dashboard card as well as the studio.

## Verification

Full gate. **There is a backend change, so the Rust gate is in scope and the backend must roll
before the frontend** — chart 02 established why. No migration: `pivot.columnWidths` and the
per-dimension totals shape are additive, and `chartparams.rs` leaves settings keys open.
