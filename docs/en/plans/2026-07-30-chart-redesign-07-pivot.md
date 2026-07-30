# Chart Redesign 07 — Pivot: implementation plan

**Spec:** `docs/en/specs/2026-07-30-chart-redesign-07-pivot.md`
**Parity rules:** `docs/en/plans/2026-07-30-chart-redesign-metabase-parity-rules.md`
**Date:** 2026-07-30
**Vietnamese reference:** `docs/vi/plans/2026-07-30-chart-redesign-07-pivot.md`

Every file path and line number below was read on 2026-07-30. The blocker was isolated by running
five specs against the live backend, not by inspection.

**Backend IS in scope** (unlike chart 06). No migration, but the backend must roll before the
frontend — a frontend that sends a bucketed pivot to an un-fixed backend gets a 502.

---

## T1 — The blocker: `GROUPING()` on an expression (backend)

**File:** `crates/egret-bi/src/chartquery.rs`, the `has_rollup` branch (~line 399).

Today every dimension expression is inlined into both `GROUPING()` and `GROUP BY GROUPING SETS`.
When a dimension carries a temporal unit its expression is a function call, and
`GROUPING(date_trunc('month', "order_date"))` is a **parse** error in Trino.

**Change:** when `rollup_groups` is non-empty, wrap the source in a derived table that materialises
every dimension expression under its own name, then reference only those names:

```sql
FROM (SELECT <dim_expr> AS <dim_name>, … , <source columns needed by metrics> FROM <source>) AS __b
```

and emit `GROUPING("dim_name")` and `GROUPING SETS (("a","b"), …)`.

Three details that decide whether this is correct:

- **Metric expressions must still resolve.** A metric like `sum(totalprice)` references raw
  columns, so the inner `SELECT` must project what metrics need, not only the dimensions. Simplest
  correct form: project `*` plus the aliased dimension expressions — but then a dimension alias can
  **collide** with the raw column of the same name (`date_trunc(...) AS "order_date"` alongside
  `order_date`). Alias the buckets to a reserved, non-user-reachable name (`__dim_0`, matching the
  existing `__grp_i` convention) and select them explicitly rather than relying on `*`.
- **`WHERE` stays on the raw column.** `render_filter` already comments that filters compare the
  real timestamp, never the truncated key (`chartquery.rs:284-286`). The filter must therefore be
  applied in the **inner** query, where the raw column still exists.
- **Only wrap when rolling up.** The non-rollup path is fine and widely exercised; leave it byte
  identical so this cannot regress every other chart type.

**Do not "fix" this by dropping the marker columns.** `__grp_i` is how `indexResult`
(`pivot/engine.ts:35-49`) tells a subtotal row from a leaf row. Removing them breaks the pivot.

### The test matrix (this is the point of the task)

The bug survived because rollup and temporal units are each tested and their intersection is not —
four rollup tests all pass `temporal_units: HashMap::new()`. Add, as a matrix:

|  | no unit | truncation (`month`) | extraction (`day_of_week`) |
|---|---|---|---|
| no rollup | exists | exists | **add** |
| one rollup group | exists | **add** | **add** |
| two rollup groups (a real pivot) | exists | **add** | **add** |

Assert the generated SQL contains `GROUPING("…")` over a **plain identifier**, and that no
`GROUPING(` is followed by a `(`. Run for both engines. Mutation-check: revert the wrap, confirm
the new tests fail, restore, report it.

## T2 — Per-dimension subtotals

Metabase: `pivot_table.column_show_totals`, per row dimension, default on for every dimension
except the last (`settings.ts:280-322`) — a subtotal under the innermost dimension would just
repeat the leaf row, which is also what `condense` is for.

- `PivotConfig.showColTotals` gains a per-dimension form: keep the boolean as the global
  shortcut, add `dimTotals?: Record<string, boolean>` resolved at render time, defaulting to
  Metabase's rule. Defaults resolve in the builder, never in stored settings — the program's rule
  since chart 01.
- `engine.ts` reads it where `hasSubtotal` is computed (`engine.ts:226`).
- Editor: a toggle per row dimension in the Totals section of `viz/pivot.tsx`.

## T3 — Column resize, persisted

`pivot.columnWidths: { left?: number[]; value?: Record<number, number> }`, mirroring Metabase's
`leftHeaderWidths` / `valueHeaderWidths` split — card 10 ships real values for both, so this is a
setting its own sample dashboard depends on.

- A drag handle on the right edge of each header cell; `dnd-kit` is already a dependency (used by
  chart 03's column reorder — **check before adding anything**), and chart 03 also established that
  a mouse-only reorder is a defect: give the handle keyboard resize too.
- Widths apply via `<colgroup>` so a body cell never has to know its own width.
- Persist through the same path as `collapsedRows` — interactive state stored with the chart, and
  deliberately **not** part of `buildSpec`, or resizing a column would re-run the query
  (`viz/pivot.tsx:116-118` explains why for collapse and sort).

## T4 — Virtualisation, gated on a measurement

**Do not start this until T1 lands and the numbers exist.** With a bucketed pivot finally possible,
build a genuinely large one and record: result rows, DOM nodes, time to first paint, scroll
smoothness — in both apps at the same viewport. Metabase renders 111 cells for a 48-row result via
`Grid` + two `Collection`s + `ScrollSync`; egret renders every cell.

If egret is comfortable at realistic sizes, **write the numbers down and skip the task.** If it is
not, window the body rows only (the header is small) and keep the sticky panes. The parity rules
exist because chart 05 rewrote a chart it had not measured first.

## Order

T1 alone first — it is a broken-query fix and everything else is cosmetic beside it. Then T2 and T3
in parallel (independent). T4 last, gated.

## Verification gate

Rust: `cargo test -p egret-bi`, fmt, clippy. Frontend: typecheck, lint, tests, build. Deploy to
kind, **backend first**, then the frontend; live check in a new page with the bundle hash confirmed.
Run the full CI locally — GitHub Actions is out of billing quota and every job fails at 3s without
running a step.

Walk the spec's nine live-check cases. Every new test mutation-checked and the mutation reported.

Open the PR and **stop** for review.
