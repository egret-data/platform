# Settings panel geometry — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the chart settings panel Metabase's geometry — one content width, one control
height, icon buttons you can hit — and stop the call sites that broke it from doing so again.

**Architecture:** The defect is 92 height overrides and 22 fixed-width overrides at call sites,
against primitives whose defaults are already correct. So the work is subtractive: widen the panel
so the default width fits, raise the primitive height to 40, then delete overrides file by file.
A source-level test locks the door afterwards.

**Tech stack:** React 18, Tailwind v4 (OKLCH tokens), shadcn/Radix primitives, vitest.

**Read first:** `docs/en/specs/2026-08-10-settings-panel-geometry-design.md` — in particular §5,
which explains why jsdom cannot verify any of this and what is guarded instead.

---

## Task 1: The guard, before the fix

Written first so it fails against today's tree — a guard added after the cleanup would pass
without ever having demonstrated it can fail.

**Files:**
- Create: `frontend/src/components/chart/settings/__tests__/geometry.guard.test.ts`

- [ ] **Step 1: Write the failing test**

```ts
import { describe, it, expect } from "vitest";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

/**
 * WHY THIS IS A SOURCE TEST AND NOT A RENDER TEST. jsdom performs no layout: every
 * `getBoundingClientRect()` returns zeros, so a test asserting "the select is 352 wide" passes
 * against an unchanged tree and proves nothing. The thing that actually regresses is a
 * contributor reaching for `h-8` to make one control fit, so that is what is pinned.
 *
 * `h-9`/`w-full` are the primitive defaults and are not overrides — only the narrowing classes
 * are banned.
 */
const BANNED = /\b(h-7|h-8|w-16|w-24|w-28)\b/g;
const DIR = join(__dirname, "..");

describe("settings panel geometry", () => {
  it("has no call site overriding the primitive control geometry", () => {
    const offenders: string[] = [];
    for (const f of readdirSync(DIR).filter((f) => f.endsWith(".tsx"))) {
      const src = readFileSync(join(DIR, f), "utf8");
      for (const m of src.matchAll(BANNED)) offenders.push(`${f}: ${m[0]}`);
    }
    expect(offenders).toEqual([]);
  });
});
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cd frontend && npx vitest run src/components/chart/settings/__tests__/geometry.guard.test.ts`
Expected: FAIL, listing ~90 offenders across 19 files. Record the count — it is the work queue.

- [ ] **Step 3: Commit the failing guard**

```bash
git add frontend/src/components/chart/settings/__tests__/geometry.guard.test.ts
git commit -m "test(chart): pin settings-panel control geometry at the source

jsdom does no layout, so a rendered assertion about width would pass against
any implementation. The class overrides are what regress; ban those."
```

---

## Task 2: Widen the panel to Metabase's 400px

Must come before the overrides are removed: at 256px of content, a full-width select cannot hold
"Show lines and tick marks", which is why the narrow variants were introduced.

**Files:**
- Modify: `frontend/src/routes/explore/ChartExplore.tsx:633`

- [ ] **Step 1: Change the panel box**

```tsx
// Metabase's chart settings sidebar is 400px with 24px padding, giving 352px of content — the
// width every control in this panel is expected to take. It is written here, once, so that 352
// is a CONSEQUENCE of the panel rather than a number repeated at call sites; repeating it is
// what produced the 8 different widths this replaces.
<div className="w-[400px] shrink-0 overflow-y-auto [scrollbar-gutter:stable] border-r p-6 space-y-4">
```

- [ ] **Step 2: Verify the arithmetic in the browser**

Run the app, open any chart, and evaluate:

```js
const p = document.querySelector('[role=tablist]').parentElement.getBoundingClientRect();
p.width  // expect 352
```

Expected: `352`. If it reads 400, the padding did not apply to the measured box — check that
`[role=tablist]` sits inside the padded div, not outside it.

- [ ] **Step 3: Commit**

```bash
git commit -am "feat(chart): widen the settings panel to Metabase's 400px

352px of content is what a full-width control gets. The narrow call-site
variants exist because 256px could not hold a label like 'Show lines and
tick marks'; widening removes the reason they were added."
```

---

## Task 3: Raise the primitive height to 40px

**Files:**
- Modify: `frontend/src/components/ui/select.tsx:38`
- Modify: `frontend/src/components/ui/input.tsx:15`

- [ ] **Step 1: Change both defaults**

In `select.tsx`, `data-[size=default]:h-9` → `data-[size=default]:h-10`.
In `input.tsx`, `h-9` → `h-10`.

Leave `data-[size=sm]:h-8` alone — it is used outside the chart module.

- [ ] **Step 2: Check the blast radius before trusting it**

Run: `cd frontend && grep -rl "<Input\|<SelectTrigger" src --include=*.tsx | wc -l`

These primitives are shared with the rest of the app, so this changes more than the settings
panel. Run the full suite in Step 3 and read failures as real signal, not noise.

- [ ] **Step 3: Run the full frontend suite**

Run: `cd frontend && npm run test`
Expected: PASS. A failure here means a test asserted a height somewhere — fix the test only if the
new height is the intended one; otherwise reconsider.

- [ ] **Step 4: Commit**

```bash
git commit -am "feat(ui): default Input and Select to 40px, matching Metabase

The chart settings panel overrode this 92 times because 36px did not match
anything it wanted; the default is what should have matched."
```

---

## Task 4 … 8: Delete the overrides, file by file

Split by file so each step is reviewable and the guard's offender count falls monotonically. Order
is by count, largest first.

| Task | Files | overrides |
|---|---|---|
| 4 | `CartesianDisplaySection.tsx` (19), `FormatSection.tsx` (15) | 34 |
| 5 | `SegmentRangesSection.tsx` (8), `RuleEditor.tsx` (8), `PieDisplaySection.tsx` (8) | 24 |
| 6 | `WaterfallDisplaySection.tsx` (7), `ChartAxesSection.tsx` (7) | 14 |
| 7 | `TrendComparisonsSection.tsx` (6), `TableColumnsSection.tsx` (6), `BoxplotDisplaySection.tsx` (6) | 18 |
| 8 | `SeriesListSection.tsx` (4), `SankeyDisplaySection.tsx` (3), `SeriesLineStyleSection.tsx` (2), `ScalarDisplaySection.tsx` (2), `CurrencySelect.tsx` (2), `AutoToggle.tsx` (2), `SeriesSettingsPopover.tsx` (1), `ColumnSettingsPopover.tsx` (1) | 17 |

For **each** task, for **each** file:

- [ ] **Step 1: List this file's offenders**

```bash
cd frontend && grep -nE '\b(h-7|h-8|w-16|w-24|w-28)\b' src/components/chart/settings/<FILE>
```

- [ ] **Step 2: Remove them**

Delete the class from the `className`. Do **not** replace `w-24` with `w-[352px]` — the control
should take the panel's width by default. If a control genuinely must be narrow (a decimal-places
stepper beside a label, say), that is a *row layout* decision: put the label and control in a
`flex items-center justify-between` row and let the control keep its intrinsic width, rather than
pinning a pixel count. Say so in a comment where you do it.

- [ ] **Step 3: Re-run the guard and watch the count fall**

```bash
npx vitest run src/components/chart/settings/__tests__/geometry.guard.test.ts 2>&1 | grep -c "<FILE>"
```
Expected: `0` for the file just cleaned.

- [ ] **Step 4: Look at the panel in the browser**

Open a chart of the family this file serves (`CartesianDisplaySection` → bar; `PieDisplaySection`
→ pie; …) and confirm nothing overflows or wraps badly. This is the step the tests cannot do.

- [ ] **Step 5: Commit per task**

```bash
git commit -am "refactor(chart): drop geometry overrides in <files>

<N> call sites pinned a height or width against primitives whose defaults are
now correct. Removing them is what makes the panel one width and one height."
```

---

## Task 9: Icon buttons that can be hit

**Files:**
- Create: `frontend/src/components/ui/icon-button.tsx`
- Modify: `frontend/src/components/chart/settings/SeriesListSection.tsx:132-137`
- Modify: `frontend/src/components/chart/settings/TableColumnsSection.tsx` (same pattern)
- Modify: `frontend/src/components/chart/settings/SegmentRangesSection.tsx` (remove-range, 18×18)
- Test: `frontend/src/components/chart/settings/__tests__/icon-button.test.tsx`

- [ ] **Step 1: Write the failing test**

```tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { IconButton } from "@/components/ui/icon-button";

/**
 * jsdom cannot measure the rendered box, so this asserts the CLASS that sets it. Thin, but it is
 * the difference between a control that has a declared size and one whose size is whatever a
 * `▲` glyph happens to be — which is how these ended up 8×10.
 */
describe("IconButton", () => {
  it("declares a hit target of at least 24px", () => {
    render(<IconButton aria-label="Move up">▲</IconButton>);
    const b = screen.getByRole("button", { name: "Move up" });
    expect(b.className).toMatch(/\bsize-6\b/);
  });
});
```

- [ ] **Step 2: Run it, expect FAIL** — "Failed to resolve import".

- [ ] **Step 3: Create the component**

```tsx
import * as React from "react";
import { cn } from "@/lib/utils";

/**
 * A square icon-only control with a declared hit target. Exists because the reorder arrows in the
 * settings panel were bare `▲`/`▼` glyphs at `text-[10px]` with no box, which measured 8×10 —
 * about one sixth the area of Metabase's 22×22 equivalent, on a control people use repeatedly
 * while arranging a chart.
 *
 * `size-6` (24px) is the floor, not the aspiration. Prefer `size-8` where the row has room.
 */
export const IconButton = React.forwardRef<HTMLButtonElement, React.ComponentProps<"button">>(
  ({ className, ...props }, ref) => (
    <button
      ref={ref}
      type="button"
      className={cn(
        "inline-flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground",
        "hover:bg-accent disabled:pointer-events-none disabled:opacity-40",
        className,
      )}
      {...props}
    />
  ),
);
IconButton.displayName = "IconButton";
```

`forwardRef`, not a plain function — see `[[shadcn-button-forwardref-react18]]`: plain-function
primitives break Radix `asChild`.

- [ ] **Step 4: Run the test, expect PASS**

- [ ] **Step 5: Replace the glyph buttons**

In `SeriesListSection.tsx`, the pair at lines 132–137 becomes:

```tsx
<div className="flex shrink-0 flex-col">
  <IconButton aria-label={`Move ${name} up`} disabled={i === 0} onClick={() => move(i, -1)}
    className="size-6 text-[10px]">▲</IconButton>
  <IconButton aria-label={`Move ${name} down`} disabled={i === ranked.length - 1}
    onClick={() => move(i, 1)} className="size-6 text-[10px]">▼</IconButton>
</div>
```

The `aria-label`s are unchanged — existing tests select on them, and renaming would be churn that
hides whether the replacement worked.

- [ ] **Step 6: Run the full suite**

Run: `cd frontend && npm run test`
Expected: PASS. Any test selecting these buttons by label still finds them.

- [ ] **Step 7: Commit**

---

## Task 10: Fold the temporal bucket into the column control

**Files:**
- Modify: `frontend/src/routes/explore/ChartExplore.tsx:187-196`
- Test: `frontend/src/routes/explore/__tests__/ChartExplore.bucket.test.tsx`

- [ ] **Step 1: Write the failing test**

```tsx
/**
 * The old shape put the bucket on its own row labelled "Group order_month by" — repeating a
 * column name shown directly above it, in a 112px control next to a 255px one. Metabase folds the
 * bucket into the column itself (`Created At: Month`).
 */
it("shows the bucket on the column control, not on a row of its own", async () => {
  renderStudio({ xAxis: "order_month", temporalUnits: { order_month: "month" } });
  expect(screen.queryByText(/Group order_month by/)).toBeNull();
  expect(screen.getByRole("combobox", { name: /x-axis/i })).toHaveTextContent("order_month: Month");
});
```

- [ ] **Step 2: Run it, expect FAIL** on the first assertion (the label is still there).

- [ ] **Step 3: Implement** — render the selected x-axis column as `${col}: ${unitLabel}` and move
the unit choice into that control's menu as a submenu or a second group. Delete the separate row.

- [ ] **Step 4: Run the test, expect PASS. Then mutate:** hard-code the label back to
`Group ${shown} by` and confirm the test goes red. A test that passes either way is worthless.

- [ ] **Step 5: Commit**

---

## Task 11: Sentence case

**Files:**
- Modify: `frontend/src/components/chart/settings/SectionLabel.tsx`

- [ ] **Step 1:** Remove `uppercase` (and any `tracking-wider`) from `SectionLabel`. Metabase uses
sentence case; ALL-CAPS reads as shouting in a panel this dense. `SERIES`, `FORMAT`, `FORMATTING`,
`TEXT`, `NUMBER` become `Series`, `Format`, `Formatting`, `Text`, `Number`.

- [ ] **Step 2:** Run the suite. Any test asserting on the shouted string is a real hit — update it.

- [ ] **Step 3:** Commit.

> **Do NOT "fix" the FORMATTING rows' 253 @ x=265.** This looked like a 1px ragged edge and is not
> one: the bordered card at `FormatSection.tsx:80` is itself `255 @ x=264`, flush with every
> sibling, and the 253 is the button inside the card's own 1px border. Measured and settled — see
> spec §6. Changing it would misalign a card that is currently correct.

---

## Task 12: Remove `More options (2)`

**Files:**
- Modify: `frontend/src/components/chart/settings/CartesianDisplaySection.tsx`

- [ ] **Step 1:** Delete the disclosure and render its children inline, as Metabase does. A
disclosure with a count announces that something is hidden without saying what, and the two
settings behind it are not advanced.
- [ ] **Step 2:** Run the suite; fix any test that asserted the collapsed state.
- [ ] **Step 3:** Commit.

---

## Task 13: Re-measure, and only then claim it

**Files:** none — this is verification.

- [ ] **Step 1: Run the full gates, as separate commands**

```bash
cd frontend && npm run lint && npx tsc --noEmit && npm run test
```

- [ ] **Step 2: Re-run the live sweep from the spec §1**

Both apps up. For each of the 19 `MB · …` charts, measure distinct control widths and heights in
the Display and Axes panels, bounding the panel by the tablist's own box — **not** by a guessed x
coordinate, which swept the chart canvas into the first pass and invented an 830px control.

Expected: widths **85 → ≤ 25**, heights **67 → ≤ 25**.

- [ ] **Step 3: Report both numbers in the PR**, before and after, with the method. A claim of
"more consistent" without the count is not checkable by a reviewer.
