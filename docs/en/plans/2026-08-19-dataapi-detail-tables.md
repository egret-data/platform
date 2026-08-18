# Data API Detail Tables Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the column of near-empty cards in the Data API endpoint editor with one full-width table per tab, and write down the conventions the last four phases established.

**Architecture:** One additive export on the shared `ConfigTable` (a group header row), then two detail sections rebuilt around it, then the width cap removed from the detail `Outlet`. Frontend only.

**Tech Stack:** React 18, TypeScript, shadcn/Radix, Tailwind v4 OKLCH tokens, vitest + Testing Library.

**Spec:** `docs/en/specs/2026-08-18-data-api-frontend-rework-design.md` §5 and §7 — this is **phase 5**, the last before trash.

## Global Constraints

- **`ConfigTable` is shared by 15 files and `ConfigSection` by 16** (14 under Control Plane, plus `components/alerts/ScheduleEditor.tsx`, which is not Control Plane at all). Measured with `grep -rl`, after the spec's earlier "nine Control Plane usages" proved to be a guess written as a fact. **Any change to `ConfigTable` must be purely additive**, and no file outside `routes/explore/dataapi/detail/` may change.
- **`ConfigTableRow.details` is a plain string with no markdown.** Backticks in it render as literal backtick characters — a real defect this project found in a browser. A wider table makes longer copy tempting; the constraint does not move.
- **The `Query` tab keeps its current form.** A SQL editor and a result grid are not settings, and forcing them into a `Setting | Details | Value` row is the same mistake as the cards, inverted.
- **Removing `max-w-3xl` affects all six tabs**, not just the two being rebuilt. Query, Access, Try it and Versions must be checked, not assumed.
- **The eslint baseline is 0 errors / 177 warnings.** No msw. `ListPageShell` owns each page's single `h1`.
- **Every assertion is mutation-checked before being believed, and a mutation that fails to compile proves nothing** — build to zero errors first, confirm, then read. Report an uncompiled run as inconclusive, never as evidence.
- **Run the full frontend suite before every commit, unpiped, from the repo root or with absolute paths.** Never pipe a test run into `head` or `tail`. `ChartExplore.mount.test.tsx` intermittently times out at 5s under parallel load on this machine — unrelated, passes alone in ~9.6s.
- **`cp` is aliased to `cp -i` in this shell and will silently decline to overwrite.** Use `python3 -c "import shutil; shutil.copyfile(...)"` or `git checkout --` to restore a mutated file, and verify the restore rather than assuming it.
- **Before asserting anything is unused, missing or unreachable, measure it** and put the number in your report. Seven claims in this programme have been wrong in that exact shape.

---

## File Structure

| File | Responsibility |
|---|---|
| `frontend/src/components/config/ConfigTable.tsx` | **Modify, additively.** New `ConfigTableGroup` export: a full-width header row inside the same table. 15 existing consumers untouched. |
| `frontend/src/routes/explore/dataapi/detail/Config.tsx` | **Modify.** One table; `Endpoint`, `Cache`, `Limits`, `Rate limit` become group rows; `ConfigSection` gone from this file. |
| `frontend/src/routes/explore/dataapi/detail/ResponseSection.tsx` | **Modify.** Same treatment for its own groups. |
| `frontend/src/routes/explore/dataapi/EndpointDetail.tsx` | **Modify.** `max-w-3xl` removed from the `Outlet` wrapper. |
| `docs/en/ui-conventions.md` (+ `docs/vi/`) | **New.** The conventions, written from what now exists. |

---

### Task 1: `ConfigTableGroup`

**Files:**
- Modify: `frontend/src/components/config/ConfigTable.tsx`
- Test: `frontend/src/components/config/__tests__/ConfigTable.test.tsx` (create if absent)

**Interfaces:**
- Produces: `export function ConfigTableGroup({ label }: { label: string })` — one `<TableRow>` containing a single `<TableCell colSpan={3}>`.

**Why additive is the whole design.** `ConfigTable` renders `{children}` straight into `<TableBody>`, so a group row is simply another child. Nothing about the existing `ConfigTableRow` changes, no prop is added to `ConfigTable`, and the 15 other consumers cannot be affected — **verify that claim with `grep -rl` and put the count in your report** rather than repeating mine.

- [ ] **Step 1: Write the failing test**

```tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { ConfigTable, ConfigTableGroup, ConfigTableRow } from "@/components/config/ConfigTable";

describe("ConfigTableGroup", () => {
  it("spans the whole table, so a group heading is not mistaken for a Setting", () => {
    render(
      <ConfigTable>
        <ConfigTableGroup label="Cache" />
        <ConfigTableRow label="Enabled" details="Serve repeat calls from a cache."><span /></ConfigTableRow>
      </ConfigTable>,
    );
    const cell = screen.getByText("Cache").closest("td");
    // colSpan is the point: a group row that occupied only the Setting column
    // would read as a setting with no value, which is what the cards were
    // replaced to avoid.
    expect(cell).toHaveAttribute("colspan", "3");
  });

  it("keeps ordinary rows in their three columns", () => {
    render(
      <ConfigTable>
        <ConfigTableGroup label="Cache" />
        <ConfigTableRow label="Enabled" details="Serve repeat calls from a cache."><span data-testid="ctl" /></ConfigTableRow>
      </ConfigTable>,
    );
    expect(screen.getByText("Enabled").closest("td")).not.toHaveAttribute("colspan", "3");
    expect(screen.getByTestId("ctl")).toBeInTheDocument();
  });

  it("is announced as a heading for its group, not as data", () => {
    render(<ConfigTable><ConfigTableGroup label="Rate limit" /></ConfigTable>);
    // A screen reader hitting a bare cell mid-table learns nothing about
    // structure; the row has to say it starts a section.
    expect(screen.getByRole("rowheader", { name: "Rate limit" })).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd frontend && npx vitest run src/components/config/__tests__/ConfigTable.test.tsx`
Expected: FAIL — `ConfigTableGroup` is not exported.

- [ ] **Step 3: Write it**

```tsx
/**
 * A full-width heading row inside a `ConfigTable`.
 *
 * The Data API detail sections used to stack one `ConfigSection` card per group,
 * each framing a table of one or two rows — read down the page, a column of
 * mostly-empty boxes. Groups belong inside the table instead, and a group row
 * has to span all three columns: one that occupied only the Setting column
 * would read as a setting with no value, which is the confusion the cards were
 * replaced to remove.
 *
 * `scope="colgroup"` and the `rowheader` role are what make it a heading rather
 * than a stray cell for anything not reading it visually.
 */
export function ConfigTableGroup({ label }: { label: string }) {
  return (
    <TableRow className="hover:bg-transparent">
      <TableCell
        colSpan={3}
        scope="colgroup"
        role="rowheader"
        className="pt-6 text-xs font-semibold uppercase tracking-wide text-muted-foreground"
      >
        {label}
      </TableCell>
    </TableRow>
  );
}
```

If `role="rowheader"` on a `<TableCell>` does not produce the accessible role the test asks for, find what does — do not weaken the test to match markup that fails to announce itself.

- [ ] **Step 4: Run to verify it passes**

- [ ] **Step 5: Mutation-check**

Change `colSpan={3}` to `colSpan={1}`. Typecheck to zero errors, confirm, then the first test must fail. Restore. Then drop `role="rowheader"`: the third test must fail.

- [ ] **Step 6: Full gates and commit**

```bash
cd frontend && npx vitest run && npm run typecheck && npm run lint
git add frontend/src/components/config
git commit -m "feat(config): a group heading row for ConfigTable"
```

---

### Task 2: `Config` becomes one table

**Files:**
- Modify: `frontend/src/routes/explore/dataapi/detail/Config.tsx`
- Modify: `frontend/src/routes/explore/dataapi/__tests__/EndpointDetail.test.tsx`

**Interfaces:**
- Consumes: `ConfigTable`, `ConfigTableGroup`, `ConfigTableRow` from Task 1.

Today this file has four `ConfigSection` cards: `Endpoint` (name, description, method, path), `Cache`, `Limits`, `Rate limit`. The first is a form; the other three are already `ConfigTable`s.

**All four become groups in ONE table.** The identity fields become ordinary `ConfigTableRow`s with their inputs in the Value column — that is the change the user asked for, and it is why the `Endpoint` group is the interesting one: `Description` is a `Textarea` and `Path` is a long string, so check they are usable in a `w-52` Value column and say what you find. **If a field genuinely does not fit the row shape, say so rather than forcing it** — the plan would rather hear that than get a cramped control.

- [ ] **Step 1: Write the failing test**

Add to `EndpointDetail.test.tsx`, using its existing `renderConfig` helper:

```tsx
  it("renders its groups as rows of one table, not as separate cards", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint }, isLoading: false, isError: false, refetch: vi.fn(),
    } as never);
    renderConfig(endpoint);
    await screen.findByLabelText("Name");
    // One table, four group headings inside it. Two tables would mean the
    // cards were renamed rather than removed.
    expect(screen.getAllByRole("table")).toHaveLength(1);
    for (const g of ["Endpoint", "Cache", "Limits", "Rate limit"]) {
      expect(screen.getByRole("rowheader", { name: g })).toBeInTheDocument();
    }
  });
```

- [ ] **Step 2-4: Run, rebuild the section, run again**

Move every existing row across unchanged — labels, `details` copy, and controls. The `details` strings are already written and reviewed; **retyping them is how copy drifts**.

- [ ] **Step 5: Mutation-check**

Split `Cache` back into its own `ConfigTable`. Typecheck, then the new test must fail on the table count. Restore.

- [ ] **Step 6: Full gates and commit**

---

### Task 3: `ResponseSection` becomes one table

**Files:**
- Modify: `frontend/src/routes/explore/dataapi/detail/ResponseSection.tsx`
- Modify: `frontend/src/routes/explore/dataapi/__tests__/EndpointDetail.test.tsx`

Same treatment. Note this file also uses real shadcn `Card`s for "Frozen columns" and "OpenAPI document" — **those are not config rows and must stay cards.** A frozen column list and a downloadable document are not `Setting | Details | Value` shaped, and the spec's complaint was about cards framing near-empty tables, not about cards existing.

Write the test first, mirroring Task 2's, then rebuild, then mutation-check by splitting a group back out.

---

### Task 4: Remove the width cap

**Files:**
- Modify: `frontend/src/routes/explore/dataapi/EndpointDetail.tsx:330`
- Modify: `frontend/src/routes/explore/dataapi/__tests__/EndpointDetail.test.tsx`

`max-w-3xl` on the `Outlet` wrapper is the direct cause of the wasted horizontal space: the `Details` column wraps to three lines while the right third of the page is empty.

**This affects all six tabs.** Query, Access, Try it and Versions were laid out inside that cap. Removing it is one line; checking it is the task. **Open each tab and say what changed** — a table that now stretches to 2560px with a two-word Value column may look worse, not better.

`ConfigTable` is `table-fixed` with `w-72` Setting and `w-52` Value, so `Details` absorbs the new width. If that makes explanatory prose unreadably wide, cap the Details **cell content**, not the page — and say which you did.

- [ ] **Step 1: Write the failing test**

```tsx
  it("does not cap the detail sections at max-w-3xl", async () => {
    // The cap is what left the right third of the page empty while the Details
    // column wrapped to three lines. Asserting its absence is weak on its own —
    // it is paired with the browser check, which is what can actually see the
    // layout.
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint }, isLoading: false, isError: false, refetch: vi.fn(),
    } as never);
    const { container } = renderConfig(endpoint);
    expect(container.querySelector(".max-w-3xl")).toBeNull();
  });
```

**Say in your report that this assertion is weak** — it pins a class name, not a layout. It exists so the cap cannot come back silently; the browser check is what judges the result.

- [ ] **Steps 2-4: Run, remove the cap, run again, and look at all six tabs**

- [ ] **Step 5: Full gates and commit**

---

### Task 5: The conventions document

**Files:**
- Create: `docs/en/ui-conventions.md` and `docs/vi/ui-conventions.md`

Written **from** what now exists, not ahead of it — a convention nobody has implemented is a wish.

Cover, and cite the code for each:

1. **List-page anatomy** — `ListPageShell` owns the single `h1`; the toolbar's control order; `ListStates` and why `isError` is checked before `hasData`.
2. **Three states every list distinguishes** — loading, empty, error — and why error may never fold into empty, citing the two account pickers that told an operator to create a service account that already existed.
3. **Create flows** — when a page, when a dialog is still right. The three Data API creates are pages; the rule that emerged is *a create flow becomes a page when the thing it creates has a URL worth linking to, or when it produces something the user must not lose to a stray click* (the one-time secret).
4. **The `Setting | Details | Value` table**, group rows, and when a card is warranted — a frozen column list and a downloadable document are cards; a group of settings is not.
5. **Width** — no page-level cap on a settings table; cap the prose column instead.
6. **`details` is a plain string.** No markdown, no backticks.

**And one section that is not about UI**, because it is the most expensive thing this programme learned:

7. **An assertion that a hardcoded implementation would also pass, pins nothing.** Give the real examples: a one-row fixture, an empty folders list, one user, one database in a picker, a test value identical to its placeholder, and an enum where asserting the non-default value only moved which constant was unfalsifiable. State the check: *before writing an assertion, ask what constant would satisfy it.*

Every rule cites the code that implements it and, where one exists, the defect that motivated it. **A rule with neither is an opinion, and opinions belong in review comments rather than in a standard.**

---

## After the plan: verify it in a browser

Every phase of this project has found a defect in a browser that no test could see, and this phase is **about layout**, which jsdom cannot judge at all.

1. **Confirm the running server is the binary you just built.**
2. **Open all six tabs at a wide viewport and at ~1280px.** The point of the phase is that the right third is no longer empty; check it is not now a two-word Value column stranded beside a very wide Details.
3. **Read the group rows** — they must look like headings, not like settings with a missing value.
4. **Check the `Endpoint` group's Description and Path** in the Value column; those are the two fields most likely not to fit.
5. **Look for literal backticks** in any `details` copy that moved.

---

## Self-Review

**Spec coverage:** §5's one-table-per-tab is Tasks 2-3, the width cap is Task 4, the group row that makes them possible is Task 1, and §7's conventions document is Task 5. `Query` keeping its form is stated as a constraint rather than a task, because the work is to leave it alone.

**Placeholder scan:** Task 3's test is described by reference to Task 2's rather than written out — the two are the same shape and the differences (which groups, which cards stay) are named. Task 5 has no test, which is correct for a document, and its acceptance is that every rule cites code.

**Type consistency:** `ConfigTableGroup({ label })` is used with that prop in Tasks 2 and 3. `ConfigTable` and `ConfigTableRow` keep their existing signatures throughout — no consumer outside the Data API detail route changes.
