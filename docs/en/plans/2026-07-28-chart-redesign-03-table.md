# Chart Redesign 03 — Table Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a column renameable — everywhere a column is named — and make the table's shell-owned
Display panel survive a wide result, then prove on a live cluster that the table features which
already have code actually work.

**Architecture:** `ColumnFormat.column_title` already exists and is already read by cartesian axis
titles; this wires the missing ends — an input in `FormatSection`, and readers in `ChartTable` and
the pivot. The panel work happens inside the shared sections, because the table descriptor declares
no display controls at all. No settings shape changes and **no migration**.

**Tech Stack:** React 18 + TypeScript, TanStack Table, Vitest.

**Spec:** `docs/en/specs/2026-07-28-chart-redesign-03-table.md`

---

## Facts established by reading the code — verify, don't re-derive

Checked against the tree at `1ee0819e`. If a snippet disagrees with what you find, STOP and report.

1. `FormatSection` (`settings/FormatSection.tsx`) is an accordion, one panel per column. The open
   column's settings are `fmt`, and `patch(p)` merges into `value[open]`. The expanded body starts at
   `<div className="space-y-3 border-t p-3">`, and everything inside it today is gated on
   `isNumericType(c.type)` or on `view_as`.
2. `ChartTable` (`ChartTable.tsx`) already receives `columnSettings` and uses it in `cell` — so the
   header change is one line and `columnSettings` is already in the `useMemo` deps.
   Line ~155: `header: c.name`.
3. The pivot's metric header text comes from `pivot/engine.ts` — `topHeader.push(colLeaves.map((cl) => ({ label: cl.metric, colSpan: 1 })))`
   — a pure function with no access to `column_settings`. `PivotTable` renders `{c.label}` for every
   header cell and cannot tell which level is the metric level without counting.
4. `SharedDisplaySections` returns a single sentence — *"Run the chart to configure column
   formatting."* — for EVERY viz when `resultColumns` is empty.
5. `TableColumnsSection` is three ungrouped blocks: a checkbox per column (unbounded), a "Freeze
   first columns" number input, and a pagination switch with a page-size select.
6. The table descriptor (`viz/table.tsx`) declares only two `data` controls and **no display
   controls**. Do not add display controls to it — the table's Display tab is shell-owned by design,
   and chart 01 regressed once by moving shell sections into a descriptor.

---

### Task 1: An input that writes `column_title`

**Files:**
- Modify: `frontend/src/components/chart/settings/FormatSection.tsx`
- Test: `frontend/src/components/chart/settings/__tests__/FormatSection.test.tsx`

- [ ] **Step 1: Write the failing test**

Follow the file's existing render/query idiom:

- opening a column's panel shows a "Column title" input, empty when unset;
- typing into it emits `{ [colName]: { column_title: "Revenue" } }`;
- clearing it emits `column_title: undefined` (not `""`) — an empty title must mean "use the
  column's own name", not "a blank header";
- the input appears for a TEXT column too, not only numeric ones — renaming is not a number
  concern, and everything else in that panel is gated on type.

- [ ] **Step 2: Run it, watch it fail**

Run: `cd frontend && npm test -- src/components/chart/settings/__tests__/FormatSection.test.tsx`

- [ ] **Step 3: Implement**

As the FIRST thing inside the expanded body, above the numeric block:

```tsx
                <Field label="Column title">
                  <Input
                    className="h-8"
                    placeholder={c.name}
                    value={fmt.column_title ?? ""}
                    onChange={(e) => patch({ column_title: e.target.value || undefined })}
                  />
                </Field>
```

`|| undefined` is the point: an empty box stores nothing, so the column falls back to its real name
rather than rendering a blank header.

Also show the title in the accordion's own row, so a renamed column is findable by its new name
while still being traceable to the query:

```tsx
              <span className="flex-1 truncate">
                {value[c.name]?.column_title || c.name}
                {value[c.name]?.column_title && (
                  <span className="ml-1.5 text-xs text-muted-foreground">{c.name}</span>
                )}
              </span>
```

Note this reads `value[c.name]`, not `fmt` — `fmt` is only the OPEN column's settings, and this row
renders for every column. Getting that wrong makes every collapsed row show the open column's title.

- [ ] **Step 4: Verify and commit**

```
npm test -- src/components/chart && npx tsc --noEmit && npm run lint
```

```bash
git add -A && git commit -m "feat(charts): rename a column from the formatting panel"
```

---

### Task 2: The table reads it

**Files:**
- Modify: `frontend/src/components/chart/ChartTable.tsx`
- Test: `frontend/src/components/chart/__tests__/ChartTable.*.test.tsx` (find the existing one)

- [ ] **Step 1: Write the failing test**

Render `ChartTable` with `columnSettings={{ total_revenue: { column_title: "Revenue" } }}` and
assert the header reads `Revenue` and not `total_revenue`. Add a second case proving sorting still
works by clicking the renamed header — the header is the sort control, so a rename must not break
it.

- [ ] **Step 2: Run it, watch it fail**

- [ ] **Step 3: Implement**

```tsx
          header: columnSettings[c.name]?.column_title || c.name,
```

`columnSettings` is already a dependency of that `useMemo` — confirm that, and say so.

- [ ] **Step 4: Pin the precedence between the two rename channels**

The spec states the rule: `column_title` renames a **column**, `seriesLabels` renames a **series**,
and on a cartesian chart — where a metric is both — the series rename wins because it is the more
specific choice. That precedence already exists in `titleOf` in
`echarts/options/cartesian.ts` (chart 01 built it), but **nothing tests it**, so nobody would
notice it flipping.

Add a case to `echarts/__tests__/cartesian.defaults.test.ts`:

```ts
  it("prefers a renamed series over a renamed column — the more specific rename wins", () => {
    const o = opt({
      kind: "bar", xAxis: "region", yMetrics: ["total_revenue"],
      seriesLabels: { total_revenue: "Series name" },
      columnSettings: { total_revenue: { column_title: "Column name" } },
    });
    expect(o.yAxis.name).toBe("Series name");
  });
```

Match that file's existing `opt(...)` helper rather than copying mine. Confirm it passes without any
change — it should; this is a characterisation test, not a fix. If it FAILS, the precedence is not
what the spec claims and you should stop and report rather than changing either side.

- [ ] **Step 5: One thing to check and report**

Column widths are persisted by column NAME, and `hiddenColumns`/`columnOrder` key on name too.
Confirm a rename does not disturb any of them — i.e. that nothing keys on the header TEXT. Say what
you checked. Change nothing for it.

- [ ] **Step 6: Verify and commit**

```bash
git add -A && git commit -m "feat(charts): table headers follow the column title"
```

---

### Task 3: The pivot reads it

**Files:**
- Modify: `frontend/src/components/chart/pivot/engine.ts`
- Modify: `frontend/src/components/chart/pivot/PivotTable.tsx`
- Test: `frontend/src/components/chart/pivot/__tests__/PivotTable.test.tsx`

The engine builds header labels without access to `column_settings`, and `PivotTable` cannot tell
which header level holds metrics without counting levels. Rather than counting — which breaks the
moment the header shape changes — have the engine **tag** the cells it builds from a metric. That
adds data, not behaviour.

- [ ] **Step 1: Write the failing test**

Render `PivotTable` with `columnSettings={{ revenue: { column_title: "Revenue" } }}` and assert the
metric header reads `Revenue`. Add a case proving a DIMENSION header is untouched by a
`column_title` that happens to share its name — the tag is what distinguishes them.

- [ ] **Step 2: Run it, watch it fail**

- [ ] **Step 3: Implement**

In `engine.ts`, where the metric level is pushed:

```ts
      topHeader.push(colLeaves.map((cl) => ({ label: cl.metric, colSpan: 1, metric: cl.metric })));
```

Add the optional `metric?: string` to whatever type that array element has (read it first — the
type is declared near the top of the file). Then in `PivotTable`:

```tsx
                  {c.metric ? (columnSettings[c.metric]?.column_title || c.label) : c.label}
```

- [ ] **Step 4: Verify and commit**

```bash
git add -A && git commit -m "feat(charts): pivot metric headers follow the column title"
```

---

### Task 4: An empty state that names what this chart needs

**Files:**
- Modify: `frontend/src/components/chart/settings/SharedDisplaySections.tsx`
- Test: `frontend/src/components/chart/settings/__tests__/SharedDisplaySections.test.tsx` (create if absent)

- [ ] **Step 1: Write the failing test**

With `resultColumns: []`, the message for `vizType: "table"` mentions columns and running the query;
for a cartesian viz it says something appropriate to a chart. Assert the two are different — that is
the whole point, and a test asserting only one of them would pass with a shared sentence.

- [ ] **Step 2: Run it, watch it fail**

- [ ] **Step 3: Implement**

Replace the single sentence with one chosen by viz. Keep it short and say what to DO:

```tsx
  if (resultColumns.length === 0) {
    // One sentence for every viz named formatting — a concern the reader has not reached yet —
    // and said nothing about what this chart is waiting for.
    const empty =
      vizType === "table" || vizType === "object"
        ? "Pick columns and run the query — they appear here once the table has data."
        : vizType === "pivot_table"
          ? "Pick rows, columns and a metric, then run the query."
          : "Run the chart to format its values.";
    return <p className="text-xs text-muted-foreground">{empty}</p>;
  }
```

Adjust the wording to match the voice of the rest of the panel if it differs; report what you chose.

- [ ] **Step 4: Verify and commit**

```bash
git add -A && git commit -m "feat(charts): empty states that name what the chart is waiting for"
```

---

### Task 5: A column list that survives a wide result

**Files:**
- Modify: `frontend/src/components/chart/settings/TableColumnsSection.tsx`
- Test: `frontend/src/components/chart/settings/__tests__/TableColumnsSection.test.tsx`

Today: one checkbox per column, unbounded. A forty-column result gives forty checkboxes with no way
to find one, and no way to hide all but three.

- [ ] **Step 1: Write the failing tests**

- with 40 columns a filter box appears; typing `rev` narrows the list to matching columns;
- with 5 columns no filter box appears — a search box over five items is clutter;
- "Hide all" emits every column in `hiddenColumns`; "Show all" emits none;
- **the filter narrows the list but "Hide all" still applies to EVERY column, not just the visible
  matches** — pin this; it is the bug this pairing invites;
- toggling one column still works while a filter is active.

- [ ] **Step 2: Run them, watch them fail**

- [ ] **Step 3: Implement**

Add local `const [filter, setFilter] = useState("")`, show the input only when
`columns.length > COLUMN_FILTER_MIN`, and render the filtered list. Put the threshold at module
scope:

```tsx
/** Below this many columns a search box is clutter rather than help. */
const COLUMN_FILTER_MIN = 12;
```

**Provisional** — tuned against the panel's real width in task 7.

Show all / Hide all sit next to the "Visible columns" heading and always act on the whole column
list, not the filtered view. Make that unmistakable in the code:

```tsx
  // Deliberately `columns`, not the filtered list: a filter is a way to FIND a column, not a
  // selection. "Hide all" while a filter is active must not quietly hide only what is on screen.
  const setAllHidden = (hide: boolean) =>
    onChange({ ...value, hiddenColumns: hide ? columns.map((c) => c.name) : [] });
```

- [ ] **Step 4: Group the three blocks**

The section is three ungrouped blocks. Give the two rarely-touched ones — Freeze and Paginate — a
heading each, so the column list is not visually continuous with a number input. Use the
`SectionLabel` component the shell already uses, or the file's own uppercase label idiom — match
what is there; do not introduce a third heading style.

- [ ] **Step 5: Verify and commit**

```
npm test -- src/components/chart && npx tsc --noEmit && npm run lint
```

```bash
git add -A && git commit -m "feat(charts): find a column in a wide table's settings"
```

---

### Task 6: Prove the existing table features work

**Files:** none up front — this task's output is a report, plus fixes for whatever it finds.

Pagination, freeze, drag-to-reorder and persisted widths all have code and tests. Chart 01 shipped
`maxCategories` with green unit tests and a renderer that never received it. Chart 02's controls
displayed the opposite of what the chart was doing. Both were invisible to tests and obvious in a
browser.

- [ ] **Step 1: Deploy the branch to kind**

```bash
docker build -f frontend/Dockerfile -t egret-frontend:dev frontend
kind load docker-image egret-frontend:dev --name egret
kubectl rollout restart deploy/egret-frontend -n egret-system
kubectl rollout status deploy/egret-frontend -n egret-system --timeout=180s
```

Read the build output before trusting the rollout — a failed build leaves the old image loadable and
`rollout status` still reports success. Then open the app **with a cache-busting query parameter**;
the browser caches `index.html` and chart 01 wasted a cycle screenshotting a stale bundle.

No migration in this unit, so the backend does not need rebuilding.

- [ ] **Step 2: Drive each feature and record the outcome**

On a table over Demo Sales:

| # | Do this | Expect |
|---|---|---|
| 1 | Turn pagination on at 25 | a footer with Prev/Next; Next advances |
| 2 | Change page size to 50 | the page resets to the first |
| 3 | Change the query so the row count changes | the page resets, no empty page |
| 4 | Freeze 2 columns, scroll right | the two stay put, with the tint |
| 5 | Drag a header to a new position | the order changes |
| 6 | Reload the chart | the dragged order survives |
| 7 | Drag a column edge to resize, reload | the width survives |
| 8 | Remove that column from the query, re-add it | no stale width applied |

Record what actually happened for each, with a screenshot for anything that misbehaves.

- [ ] **Step 3: Fix what is broken**

Anything that fails becomes a commit in this PR, TDD as usual: a test that reproduces it first,
confirmed failing, then the fix. If everything passes, say so plainly — "nothing found" is a real
result here, and it is the first evidence any of this has been exercised.

---

### Task 7: Full verify, deploy, live check, PR

- [ ] **Step 1: Frontend gate**

```
cd frontend && npm run lint && npm run typecheck && npm test && npm run build
```

No migration, so the backend gate is out of scope unless task 6 dragged it in.

- [ ] **Step 2: Redeploy and run the spec's live check**

The spec's eight steps. Tune `COLUMN_FILTER_MIN` here against the panel's real width and say what
you changed it to.

Step 2 of that list is the one that proves this unit: rename `total_revenue` to `Revenue` in
Formatting, and confirm the table header AND a bar chart's axis title on the same column both
follow — one rename, both places.

- [ ] **Step 3: Before/after screenshots at both sizes**

Deploy `main` first for a genuine "before", screenshot, then redeploy the branch. Chart 01 did this
and it was worth the extra builds.

- [ ] **Step 4: Open the PR — do not merge**

```bash
git -c http.version=HTTP/1.1 push -u origin feat/chart-redesign-table
gh pr create --base main --title "feat(charts): redesign 03 — table" --body-file <a file you write>
```

The body must state: that the program spec's premise for this unit was wrong and has been corrected;
what a person can now do that they could not; what task 6 found; and anything left undone. Then stop
and wait for approval.
