# W2 — Edit-Mode Safety and Fidelity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make dashboard edit mode safe to leave and honest about what it is telling you. Cancel
stops destroying work silently, the three destructive draft edits become undoable, the edit-mode
guide grid stops shouting, and the filter→card mapping control stops being a 97px select with no
explanation attached.

**Architecture:** All four items are *route behaviour* today, and `DashboardView.tsx` is already
769 lines with seven workstreams queued against it. So every one of them lands as a pure function
or a component in its own file, and the route keeps only the wiring. The single most important
structural decision here is the first one: **dirtiness is a pure, exported function over two draft
snapshots**, so the thing that decides whether a user loses their work can be tested exhaustively
without mounting a route that needs a QueryClient, a router and four API mocks.

**Tech Stack:** React 18 + TypeScript, Tailwind v4 (OKLCH tokens), shadcn/Radix (`AlertDialog`),
sonner for toasts, vitest + Testing Library.

**Design reference:** `docs/en/specs/2026-08-09-dashboard-parity-design.md` §2.2, §2.3, §2.4 and
§4 W2. §9 of that document lists findings that turned out **not** to be defects — in particular the
per-card edit toolbar and "Replace chart" clearing `options` are both correct as they stand. Do not
plan or write work for them.

**Branch:** `feat/dashboard-parity-02-edit-safety`, branched from
`feat/dashboard-parity-01-responsive-grid` — **not** from `main`. W1 rewrote
`DashboardGridLayout.tsx` wholesale and this workstream edits two lines of the rewritten version.
The PR is opened with `--base feat/dashboard-parity-01-responsive-grid` so W1's commits do not
appear in it.

```bash
git fetch origin
git checkout feat/dashboard-parity-01-responsive-grid
git checkout -b feat/dashboard-parity-02-edit-safety
```

---

## Background the implementer needs

### Why the dirtiness check must be a pure function, and what it compares

`startEdit()` (`DashboardView.tsx:240-248`) builds four pieces of draft state — `draftCards`,
`draftParameters`, `draftTabs`, `draftName` — and keeps **no** record of what they started as.
`cancelEdit()` (`:249`) then drops all four synchronously on one click. There is nothing to compare
against, which is why there is no confirmation: the route does not know whether anything changed.

The snapshot cannot be the server payload. `startTabsDraft` (`tabs.ts:15-30`) mints a *fresh client
key* per tab and rewrites every card's tab link from the server `tab_id` onto that key, so the draft
and the server payload are not the same shape and comparing them would need the transform run twice.
Snapshot the **draft** at `startEdit()` instead — same shape, no transform, and it is exactly what
Cancel throws away.

Three normalisation traps, each of which would make a clean dashboard report dirty:

1. **`options` key order.** A card's `options` is a free-form bag and `updateCardOptions`
   (`:331-336`) writes `{ ...c.options, ...patch }`, which reorders keys whenever a patch
   overwrites an existing one. `JSON.stringify` preserves insertion order, so a plain stringify
   comparison would call an unchanged card changed. Keys must be sorted at every depth.
2. **`tab_id` vs `tabKey`.** `DraftCard` carries both — the server id, copied through untouched, and
   the draft's own link. Comparing `tab_id` would make a card moved between draft tabs look
   unchanged. Compare `tabKey`; ignore `tab_id`.
3. **Card array order.** `duplicateCard` and `addCard` append; nothing else reorders. Position lives
   in `grid_col`/`grid_row`, so array order carries no meaning — sort by `id` before comparing.
   **Parameter order is the opposite**: §4 W6 records that filter chips render in creation order, so
   a parameter reorder *is* a change and the array order must be compared as-is.

**One hazard that only the live check can settle.** `compactType="vertical"` means react-grid-layout
may compact the layout on mount and emit an `onLayoutChange` with positions that differ from the
saved ones — which would write into `draftCards` and make an untouched dashboard report dirty. It
should not happen, because the saved layout was itself written back by this same RGL after
compaction on the previous save. But "should not" is not evidence, so Task 9 Step 3 measures it:
enter edit mode, touch nothing, press Cancel, and require that no dialog appears.

### The undo toasts are draft-level, so undo is a `setState`

All three actions Metabase makes undoable are pre-save draft edits in egret too — nothing has been
`PUT`. So undo restores a snapshot taken immediately before the edit and never touches the server.
The house pattern for an undo toast already exists at `lib/runBulk.ts:38-57`:
`toast(msg, { action: { label: "Undo", onClick } })`, with the test pattern at
`lib/__tests__/runBulk.test.ts:26-31` (mock sonner, pull `cfg.action.onClick` out of the mock call
and invoke it).

Metabase's wording, verbatim from the source:

| action | message | source |
|---|---|---|
| remove card | `Removed card` | `dashboard/actions/cards-typed.ts:508` |
| delete tab | `Deleted "<tab name>"` | `dashboard/components/DashboardTabs/use-dashboard-tabs.ts:57` |
| replace card | `Question replaced` | `dashboard/components/DashboardGrid.tsx:699-713` |

The replace message is `getUndoReplaceCardMessage(card)` and has three forms in Metabase
(`Model replaced` / `Metric replaced` / `Question replaced`) because a Metabase card can be any of
the three. An egret dashboard card is always a chart, so it has one form: **`Chart replaced`**.

**Tab delete has three call sites, not one.** `DashboardView.tsx:547-558` deletes a tab inline when
it has no cards, and the dialog at `:713-728` deletes it two more ways (reassign the cards, or drop
them). All three must go through one helper or the snapshot gets taken three different ways — and
consolidating them makes `DashboardView.tsx` *shrink*, which is the point.

### The grid stroke, and why the unit test cannot use a canvas

`DashboardGridLayout.tsx:36` (pre-W1 numbering; after W1 it is the `probe.style.color` line inside
the same effect) sets the guide-grid stroke to
`color-mix(in oklch, var(--border) 80%, var(--foreground))` and reads the computed result back off a
probe element, because a CSS custom property does not resolve inside an SVG embedded as a data-URL
background. The comment above it says `--border` was "nearly invisible" — which is precisely what a
Metabase grid guide is. It is a hint under the cards, not a table of boxes.

Composited against the dashboard canvas (`--dashboard`, which is what `DashboardView.tsx:452`
paints under the grid) and computed from `index.css`:

| | stroke expression | composited | canvas | contrast |
|---|---|---|---|---|
| dark, today | `color-mix(… --border 80%, --foreground)` → `oklch(0.992857 0 0 / 0.28)` | `#50…` | `oklch(0.16 0 0)` | **2.42 : 1** |
| light, today | same → `oklch(0.778 0.022 281.92)` | opaque | `oklch(0.955 0.004 270)` | **1.77 : 1** |
| dark, after | `var(--border)` → `oklch(1 0 0 / 10%)` | | | **1.27 : 1** |
| light, after | `var(--border)` → `oklch(0.92 0 286.61)` | opaque | | **1.11 : 1** |
| Metabase | `hsla(195, 6%, 87%)` on `#f9f9fa` | | | 1.27 : 1 |

(§2.4 of the design quotes 2.36:1 for dark. That was measured against `--background` `#090909`; the
grid actually paints on `--dashboard` `#0d0d0d`, which is 2.42:1. The direction and the conclusion
are identical.)

A 90 % blend was considered as a middle ground and **rejected**: `--border` is 10 % white in the
dark theme, so mixing in 10 % of an opaque `--foreground` nearly doubles the composited alpha
(0.10 → 0.19) and the dark contrast climbs back to 1.73:1, over the 1.5 target. Plain `var(--border)`
is the only single expression that clears the target in both themes.

**The canvas-pixel technique belongs in the live check, not the unit test.** jsdom runs no layout and
no paint, `canvas` is not in `frontend/package.json`, so `getContext("2d")` returns `null` in vitest
— a canvas readback there measures nothing. The maths, however, is closed-form, and this repo
already has the house pattern for exactly this:
`frontend/src/__tests__/focusIndicatorContrast.test.ts` parses `index.css`, converts OKLCH→sRGB with
the published Ottosson matrices and computes WCAG relative luminance, precisely because
`getComputedStyle` hands back `oklch(…)` and a naive rgb regex reads L, C and H as R, G and B and
reports confident garbage. Task 6 reuses that method for the unit test and puts the real canvas
readback in the browser, in Task 9 Step 4, where there is a real compositor.

### The mapping overlay: what is in scope at the call site, and what is not

`CardFilterMapOverlay.tsx:12` renders `w-48` — 192px nominal, measured at **97px** because the card
around it was 125px and `max-w-full` clamped it. It is called from `DashboardView.tsx:648-658`,
guarded by `activeParamId && c.card_type === "chart" && baseChart`. Everything the guard rejects
renders *nothing at all*: a text card sitting beside a chart card that has a mapping control gets no
explanation whatsoever.

In scope at that call site today, with no lift at all:

- `c.card_type` — `"chart" | "text" | "heading" | "link" | "placeholder"`
- `baseChart` — `undefined` when the chart is missing or not yet loaded
- `baseChart.params.spec` — the dimensions/columns/`temporal_units` that decide whether there is
  anything to map
- `c.size_y` — the card's height in rows, which is what Metabase's `layoutHeight <= 2` header rule
  keys on (`DashCardCardParameterMapperContent.tsx:107-109`)
- `parameters.find(p => p.id === activeParamId)?.type` — needed for `Connect to` vs
  `Column to filter on`

**Not in scope, and no small lift puts it there: "The selected filter is on another tab."**
Metabase's precondition for that string is `editingParameterInlineDashcard != null &&
editingParameterInlineDashcard.dashboard_tab_id !== dashcard.dashboard_tab_id`
(`DashCardCardParameterMapperContent.tsx:86-92, 188-195`) — a parameter that has been *moved onto a
specific card*, which is Metabase's inline-parameter model. egret has no such model:
`DashboardParameter` (`api/dashboards.ts:54-65`) has `id`, `name`, `slug`, `type`, `operator`,
`default`, `required`, `widget`, `filteringParameters`, `valuesSource` — and no owning card and no
tab. Every egret parameter is dashboard-level. On top of that, `visibleCards` is already filtered to
the active tab (`DashboardView.tsx:188-192`), so a card from another tab is never mounted and never
renders an overlay in the first place.

Inventing a tab association on `DashboardParameter` to make the string fire would be inventing an
API, which this plan is told not to do. Inline parameters are **W7**
(`…-07-inline-parameters.md`). So the branch is built and unit-tested in `cardMapState`, keyed on an
explicit `filterTabKey` input that `DashboardView` passes as `undefined` today, and W7 activates it
by passing a value. That is the smallest lift that puts the information in scope without fabricating
a data model, and the branch is marked as inert in both the code comment and the test name so nobody
reads it as live.

**Two Metabase strings are not copied verbatim, because they would be false in egret:**

- `"You can connect widgets to {{variables}} in link cards."` — egret's `LinkCard.tsx:8,18` uses the
  raw `url` and `label` with no `interpolateParams` call, so a link card cannot use a filter's value
  at all. The honest text is used instead.
- `heading: ""` in Metabase's `mappingInfoText` map means a heading card gets no hint. egret's
  heading card *does* interpolate (`DashboardCardView.tsx:104` runs `interpolateParams` on it, same
  as text at `:99`), so it gets the same hint text as a text card. egret is ahead of Metabase here
  and the UI should say so.
- Metabase says "widgets"; egret's UI calls them filters everywhere (`EditFilterChips`, "Add a
  filter"). The strings say "filters".

`"Variable to map to"` (`DashCardCardParameterMapperContent.tsx:119`) has no egret analogue — it is
the header for a *native SQL* card with template variables, and egret dashboard cards are always
spec-built charts. It is deliberately not ported.

---

## File structure

| Path | Responsibility after this workstream |
|---|---|
| `frontend/src/components/chart/dashboard/edit-draft.ts` | **New.** `DashboardDraft`, `draftFingerprint`, `isDraftDirty`. Pure — no React, no DOM, no sonner. |
| `frontend/src/components/chart/dashboard/draft-undo.ts` | **New.** The three Metabase undo messages and `undoableDraftEdit`, the one place a draft-level destructive edit reaches sonner. |
| `frontend/src/components/chart/dashboard/DiscardChangesDialog.tsx` | **New.** Metabase's leave confirmation, on the house `AlertDialog` primitive. |
| `frontend/src/components/chart/dashboard/grid-stroke.ts` | **New.** The guide-grid stroke expression and the contrast band it must stay inside. |
| `frontend/src/components/chart/dashboard/card-map-state.ts` | **New.** `cardMapState` + `columnsForParam`: what the mapping overlay should show on one card, as a pure function. |
| `frontend/src/__tests__/support/wcag.ts` | **New.** OKLCH→sRGB, relative luminance, contrast, alpha compositing and `index.css` token lookup, for contrast tests. Not a spec file, so vitest does not collect it. |
| `frontend/src/components/chart/dashboard/CardFilterMapOverlay.tsx` | Renders a `CardMapState`: a full-width picker, or the reason there is no picker. |
| `frontend/src/components/chart/dashboard/DashboardGridLayout.tsx` | Two lines: the stroke expression comes from `grid-stroke.ts`. (W1 rewrote this file; edit the rewritten version.) |
| `frontend/src/routes/explore/DashboardView.tsx` | Wiring only: a baseline ref, a confirm-dialog flag, one `deleteTabWithUndo` helper that replaces three inline copies, and the overlay's props. Net line change must be ≈ 0. |

---

## Task 1: Dirtiness as a pure function

**Files:**
- Create: `frontend/src/components/chart/dashboard/edit-draft.ts`
- Test: `frontend/src/components/chart/dashboard/__tests__/edit-draft.test.ts`

This is the structural decision the rest of the discard work rests on. Read "Why the dirtiness check
must be a pure function" above before starting.

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/edit-draft.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { isDraftDirty, type DashboardDraft } from "../edit-draft";
import type { DraftCard, DraftTab } from "../tabs";

const card = (over: Partial<DraftCard> = {}): DraftCard => ({
  id: "c1", chart_id: "ch1", card_type: "chart",
  grid_col: 0, grid_row: 0, size_x: 6, size_y: 4,
  options: { autoHeight: false, hideEmpty: true }, tab_id: "server-t1", tabKey: "tab-0",
  ...over,
});

const tab = (over: Partial<DraftTab> = {}): DraftTab => ({ key: "tab-0", name: "Tab 1", position: 0, ...over });

const draft = (over: Partial<DashboardDraft> = {}): DashboardDraft => ({
  name: "Sales",
  cards: [card(), card({ id: "c2", grid_col: 6 })],
  parameters: [
    { id: "p1", name: "Country", slug: "country", type: "category", operator: "is" },
    { id: "p2", name: "Date", slug: "date", type: "date", operator: "range" },
  ],
  tabs: [tab(), tab({ key: "tab-1", name: "Tab 2", position: 1 })],
  ...over,
});

describe("isDraftDirty", () => {
  it("is clean against an identical draft and against a deep copy of itself", () => {
    const base = draft();
    expect(isDraftDirty(base, base)).toBe(false);
    expect(isDraftDirty(base, JSON.parse(JSON.stringify(base)))).toBe(false);
  });

  it("is dirty on a rename, but not on leading or trailing whitespace alone", () => {
    // onSave writes `draftName.trim()` (DashboardView.tsx:390), so a draft that differs only by
    // surrounding whitespace would save byte-identical bytes. Prompting for that is a false alarm.
    expect(isDraftDirty(draft(), draft({ name: "Revenue" }))).toBe(true);
    expect(isDraftDirty(draft(), draft({ name: "  Sales " }))).toBe(false);
  });

  it("is dirty when a card is added, removed, moved or resized", () => {
    const base = draft();
    expect(isDraftDirty(base, draft({ cards: [...base.cards, card({ id: "c3" })] }))).toBe(true);
    expect(isDraftDirty(base, draft({ cards: [base.cards[0]] }))).toBe(true);
    expect(isDraftDirty(base, draft({ cards: [card({ grid_row: 4 }), base.cards[1]] }))).toBe(true);
    expect(isDraftDirty(base, draft({ cards: [card({ size_y: 8 }), base.cards[1]] }))).toBe(true);
  });

  it("is dirty when a card's chart is replaced or its options change", () => {
    const base = draft();
    expect(isDraftDirty(base, draft({ cards: [card({ chart_id: "ch9" }), base.cards[1]] }))).toBe(true);
    expect(isDraftDirty(base, draft({ cards: [card({ options: { autoHeight: true, hideEmpty: true } }), base.cards[1]] }))).toBe(true);
  });

  it("ignores card array order and options key order", () => {
    // duplicateCard and addCard append; positions live in grid_col/grid_row, so array order carries
    // no meaning. updateCardOptions spreads `{ ...c.options, ...patch }`, which reorders keys the
    // moment a patch overwrites an existing one — JSON.stringify preserves insertion order, so
    // without sorting, opening a settings panel and changing nothing would report a dirty board.
    const base = draft();
    expect(isDraftDirty(base, draft({ cards: [base.cards[1], base.cards[0]] }))).toBe(false);
    expect(isDraftDirty(base, draft({
      cards: [card({ options: { hideEmpty: true, autoHeight: false } }), base.cards[1]],
    }))).toBe(false);
  });

  it("ignores tab_id, which is the server id the draft does not edit", () => {
    const base = draft();
    expect(isDraftDirty(base, draft({ cards: [card({ tab_id: "anything-else" }), base.cards[1]] }))).toBe(false);
  });

  it("is dirty when a card moves between draft tabs", () => {
    // moveCardToTab (tabs.ts:137-140) rewrites `tabKey` and leaves `tab_id` alone, so a comparison
    // that keyed on tab_id would call this unchanged — the exact inverse of the case above.
    const base = draft();
    expect(isDraftDirty(base, draft({ cards: [card({ tabKey: "tab-1" }), base.cards[1]] }))).toBe(true);
  });

  it("is dirty when a parameter is added, removed, edited or REORDERED", () => {
    const base = draft();
    const [p1, p2] = base.parameters;
    expect(isDraftDirty(base, draft({ parameters: [p1] }))).toBe(true);
    expect(isDraftDirty(base, draft({ parameters: [p1, p2, { id: "p3", name: "F", slug: "f", type: "text", operator: "is" }] }))).toBe(true);
    expect(isDraftDirty(base, draft({ parameters: [{ ...p1, name: "Region" }, p2] }))).toBe(true);
    // Filter chips render in creation order (DashboardFilterBar.tsx:22-25), so a reorder is a real
    // change — the opposite rule from cards, and the reason parameters are not sorted.
    expect(isDraftDirty(base, draft({ parameters: [p2, p1] }))).toBe(true);
  });

  it("is dirty when a tab is renamed, added, deleted or reordered", () => {
    const base = draft();
    const [t1, t2] = base.tabs;
    expect(isDraftDirty(base, draft({ tabs: [{ ...t1, name: "Overview" }, t2] }))).toBe(true);
    expect(isDraftDirty(base, draft({ tabs: [t1] }))).toBe(true);
    expect(isDraftDirty(base, draft({ tabs: [...base.tabs, tab({ key: "tab-2", name: "Tab 3", position: 2 })] }))).toBe(true);
    expect(isDraftDirty(base, draft({ tabs: [{ ...t1, position: 1 }, { ...t2, position: 0 }] }))).toBe(true);
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/edit-draft.test.ts`

Expected: FAIL — `Failed to resolve import "../edit-draft"`.

- [ ] **Step 3: Write the implementation**

Create `frontend/src/components/chart/dashboard/edit-draft.ts`:

```ts
import type { DashboardParameter } from "@/api/dashboards";
import type { DraftCard, DraftTab } from "./tabs";

/** Everything `startEdit()` puts into the draft, and therefore everything Cancel throws away
 *  (DashboardView.tsx:240-249). Snapshotting the DRAFT rather than the server payload is
 *  deliberate: `startTabsDraft` mints fresh client tab keys and rewrites every card's tab link,
 *  so the two are not the same shape and comparing them would mean running that transform twice. */
export interface DashboardDraft {
  name: string;
  cards: DraftCard[];
  parameters: DashboardParameter[];
  tabs: DraftTab[];
}

/** JSON with object keys sorted at every depth.
 *
 *  `JSON.stringify` preserves INSERTION order, and a card's `options` is a free-form bag written by
 *  `updateCardOptions` as `{ ...c.options, ...patch }` — which reorders keys the moment a patch
 *  overwrites one that is already there. Two drafts that differ only in that order are the same
 *  draft, so the comparison has to be blind to it or opening a settings panel and changing nothing
 *  would report a dirty dashboard and prompt on Cancel. */
function stableStringify(value: unknown): string {
  if (value === null || typeof value !== "object") return JSON.stringify(value) ?? "null";
  if (Array.isArray(value)) return `[${value.map(stableStringify).join(",")}]`;
  const entries = Object.entries(value as Record<string, unknown>)
    // An absent key and an explicit `undefined` are the same draft: `{ ...p }` copies of a
    // parameter carry whichever optional keys the server sent, and a fresh one carries none.
    .filter(([, v]) => v !== undefined)
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  return `{${entries.map(([k, v]) => `${JSON.stringify(k)}:${stableStringify(v)}`).join(",")}}`;
}

/** The comparable projection of one draft card. Named fields, not a spread: `DraftCard` also
 *  carries `tab_id`, the SERVER tab id, which `startTabsDraft` copies through unchanged while
 *  `tabKey` is the draft's own link (tabs.ts:26-28). Comparing `tab_id` would make a card moved
 *  between draft tabs look unchanged, and comparing both would make a card on a newly created tab
 *  look changed for a field the draft never edits. */
function cardShape(c: DraftCard) {
  return {
    id: c.id,
    chart_id: c.chart_id,
    card_type: c.card_type,
    grid_col: c.grid_col,
    grid_row: c.grid_row,
    size_x: c.size_x,
    size_y: c.size_y,
    options: c.options,
    tabKey: c.tabKey,
  };
}

/** A stable string identity for a draft. Equal fingerprints mean Save would write the same bytes.
 *
 *  Cards are sorted by id (array order is meaningless — position lives in grid_col/grid_row, and
 *  only `addCard`/`duplicateCard` ever append). Parameters are NOT sorted: filter chips render in
 *  creation order (DashboardFilterBar.tsx:22-25), so reordering them is a real change. Tabs are
 *  sorted by `position`, which is their own source of truth (`reorderTab` renumbers it). */
export function draftFingerprint(draft: DashboardDraft): string {
  return stableStringify({
    // onSave writes `draftName.trim()`, so trailing whitespace alone is not a change.
    name: draft.name.trim(),
    cards: [...draft.cards]
      .sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))
      .map(cardShape),
    parameters: draft.parameters,
    tabs: [...draft.tabs]
      .sort((a, b) => a.position - b.position)
      .map((t) => ({ key: t.key, name: t.name, position: t.position })),
  });
}

/** Would Cancel lose anything? Compares the live draft against the snapshot `startEdit()` took. */
export function isDraftDirty(baseline: DashboardDraft, current: DashboardDraft): boolean {
  return draftFingerprint(baseline) !== draftFingerprint(current);
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/edit-draft.test.ts`

Expected: PASS, 9 tests.

- [ ] **Step 5: Prove the tests survive mutation**

Score every mutation by the process **exit code**, never by matching stdout text.

1. Change `isDraftDirty` to `return false;`. Re-run. Expected: FAIL (7 of the 9 go red).
2. Restore. Delete the `.sort(...)` in `stableStringify`. Re-run. Expected: FAIL on
   "ignores card array order and options key order".
3. Restore. Change `cardShape` to include `tab_id: c.tab_id` and drop `tabKey`. Re-run. Expected:
   FAIL on both "ignores tab_id" and "is dirty when a card moves between draft tabs".
4. Restore. Add `.sort(...)` to the `parameters` projection in `draftFingerprint`. Re-run.
   Expected: FAIL on the parameter-reorder assertion.
5. Restore. Drop the `.trim()` from `name`. Re-run. Expected: FAIL on the whitespace assertion.

All five must go red. This step is not optional: six assertions in the chart programme passed
against a stubbed implementation, and doing exactly this is the only thing that caught them.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/dashboard/edit-draft.ts \
        frontend/src/components/chart/dashboard/__tests__/edit-draft.test.ts
git commit -m "feat(dashboard): a pure dirtiness check over two edit-draft snapshots"
```

---

## Task 2: The discard confirmation dialog

**Files:**
- Create: `frontend/src/components/chart/dashboard/DiscardChangesDialog.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/DiscardChangesDialog.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/DiscardChangesDialog.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { DiscardChangesDialog } from "../DiscardChangesDialog";

describe("DiscardChangesDialog", () => {
  it("carries Metabase's title, message and button labels", () => {
    // Ported from LeaveConfirmModal.tsx:12-22. Asserting the strings here rather than at the route
    // keeps the route test about BEHAVIOUR, so a wording change breaks one test, not five.
    render(<DiscardChangesDialog open onOpenChange={vi.fn()} onDiscard={vi.fn()} />);
    expect(screen.getByRole("alertdialog")).toBeTruthy();
    expect(screen.getByText("Discard your changes?")).toBeTruthy();
    expect(screen.getByText("Your changes haven't been saved, so you'll lose them if you navigate away.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Discard changes" })).toBeTruthy();
  });

  it("Discard changes calls onDiscard; Cancel does not", async () => {
    const user = userEvent.setup();
    const onDiscard = vi.fn();
    const { rerender } = render(<DiscardChangesDialog open onOpenChange={vi.fn()} onDiscard={onDiscard} />);
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onDiscard).not.toHaveBeenCalled();
    rerender(<DiscardChangesDialog open onOpenChange={vi.fn()} onDiscard={onDiscard} />);
    await user.click(screen.getByRole("button", { name: "Discard changes" }));
    expect(onDiscard).toHaveBeenCalledTimes(1);
  });

  it("renders nothing when closed", () => {
    render(<DiscardChangesDialog open={false} onOpenChange={vi.fn()} onDiscard={vi.fn()} />);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.queryByText("Discard your changes?")).toBeNull();
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/DiscardChangesDialog.test.tsx`

Expected: FAIL — `Failed to resolve import "../DiscardChangesDialog"`.

- [ ] **Step 3: Write the implementation**

Create `frontend/src/components/chart/dashboard/DiscardChangesDialog.tsx`:

```tsx
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent,
  AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { buttonVariants } from "@/components/ui/button";

/** Metabase's leave confirmation, ported from LeaveConfirmModal.tsx:12-22 (dispatched from
 *  DashboardHeader.tsx:69-87). It fires only when the draft is dirty, so a clean Cancel still
 *  exits in one click — the confirmation exists to stop silent data loss, not to add a step.
 *
 *  `AlertDialog`, not `Dialog`: this is a confirmation, it has no close button to keep pinned, and
 *  Radix's `alertdialog` role is what makes a screen reader announce it as one rather than as an
 *  ordinary modal. It also matches the other confirmations in this codebase
 *  (`ui/confirm-delete-dialog.tsx`). */
export function DiscardChangesDialog({ open, onOpenChange, onDiscard }: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onDiscard: () => void;
}) {
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Discard your changes?</AlertDialogTitle>
          <AlertDialogDescription>
            Your changes haven't been saved, so you'll lose them if you navigate away.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          {/* AlertDialogAction already applies `buttonVariants()`; the destructive variant is
              appended so twMerge resolves the bg/text conflict in its favour. */}
          <AlertDialogAction className={buttonVariants({ variant: "destructive" })} onClick={onDiscard}>
            Discard changes
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/DiscardChangesDialog.test.tsx`

Expected: PASS, 3 tests.

- [ ] **Step 5: Prove the tests survive mutation**

Change `onClick={onDiscard}` on `AlertDialogAction` to `onClick={() => {}}` and re-run. Expected:
FAIL on the second test. Restore. Swap the two button labels and re-run. Expected: FAIL on the
second test (the `Cancel` button now calls `onDiscard`). Restore.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/dashboard/DiscardChangesDialog.tsx \
        frontend/src/components/chart/dashboard/__tests__/DiscardChangesDialog.test.tsx
git commit -m "feat(dashboard): Metabase's discard-changes confirmation dialog"
```

---

## Task 3: Route Cancel goes through the dialog when the draft is dirty

**Files:**
- Modify: `frontend/src/routes/explore/DashboardView.tsx`
- Test: `frontend/src/routes/explore/__tests__/DashboardView.editsafety.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/routes/explore/__tests__/DashboardView.editsafety.test.tsx`. The mock harness
is lifted from `DashboardView.tabs.test.tsx:8-41`, which is the house shape for rendering this
route:

```tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import DashboardView from "../DashboardView";

vi.mock("sonner", () => ({ toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn() }) }));
vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({
  DashboardCardView: ({ card }: { card: { id: string } }) => <div data-testid={`card-${card.id}`} />,
}));
vi.mock("@/api/charts", async (orig) => ({
  ...(await orig<typeof import("@/api/charts")>()),
  useCharts: () => ({ data: [] }),
}));

const detail = {
  id: "d1", name: "Sales", description: "", created_by: null, updated_at: "",
  parameters: [], tabs: [],
  cards: [
    { id: "c1", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 0, size_x: 6, size_y: 1, options: { text: "h1" }, tab_id: null },
    { id: "c2", chart_id: null, card_type: "heading", grid_col: 6, grid_row: 0, size_x: 6, size_y: 1, options: { text: "h2" }, tab_id: null },
  ],
};
vi.mock("@/api/dashboards", async (orig) => {
  const mod = await (orig as () => Promise<Record<string, unknown>>)();
  return { ...mod,
    useDashboard: () => ({ data: detail, isLoading: false }),
    useDashboardMutations: () => ({ update: { mutateAsync: vi.fn(), isPending: false } }),
  };
});

const renderView = () => {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}>
        <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
};

const enterEdit = () => fireEvent.click(screen.getByRole("button", { name: "Edit" }));
const bannerCancel = () => within(screen.getByText(/You're editing this dashboard/).closest("div")!)
  .getByRole("button", { name: "Cancel" });

beforeEach(() => vi.clearAllMocks());

describe("DashboardView discard confirmation", () => {
  it("exits immediately when nothing was changed", () => {
    // The confirmation exists to stop data loss, not to add a click. A clean draft must not prompt.
    renderView();
    enterEdit();
    fireEvent.click(bannerCancel());
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.getByRole("button", { name: "Edit" })).toBeTruthy();
  });

  it("prompts on a dirty draft and Cancel-the-cancel keeps both the edit and the change", async () => {
    const user = userEvent.setup();
    renderView();
    enterEdit();
    const nameInput = screen.getByLabelText("dashboard name") as HTMLInputElement;
    await user.clear(nameInput);
    await user.type(nameInput, "Revenue");
    fireEvent.click(bannerCancel());
    const dialog = screen.getByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect((screen.getByLabelText("dashboard name") as HTMLInputElement).value).toBe("Revenue");
  });

  it("Discard changes leaves edit mode and drops the draft", async () => {
    const user = userEvent.setup();
    renderView();
    enterEdit();
    const nameInput = screen.getByLabelText("dashboard name") as HTMLInputElement;
    await user.clear(nameInput);
    await user.type(nameInput, "Revenue");
    fireEvent.click(bannerCancel());
    await user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Discard changes" }));
    expect(screen.queryByLabelText("dashboard name")).toBeNull();
    expect(screen.getByRole("heading", { name: "Sales" })).toBeTruthy();
  });

  it("prompts when a card was removed, not only when the name changed", () => {
    // The name is the cheapest draft field to dirty; the one that actually cost a user their work
    // was a structural edit, so at least one structural case has to be covered here too.
    renderView();
    enterEdit();
    fireEvent.click(screen.getAllByLabelText("remove card")[0]);
    fireEvent.click(bannerCancel());
    expect(screen.getByRole("alertdialog")).toBeTruthy();
  });
});
```

> If `remove card` is not the accessible name on `CardEditToolbar`'s remove button, read
> `components/chart/dashboard/CardEditToolbar.tsx` and use the name it actually carries. Do not add
> an alias to make the test pass — the test must name what the UI names.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/routes/explore/__tests__/DashboardView.editsafety.test.tsx`

Expected: FAIL — three of the four go red because no `alertdialog` is ever rendered; only "exits
immediately when nothing was changed" passes, and it passes for the wrong reason (nothing prompts
ever). That is why the other three exist.

- [ ] **Step 3: Wire the baseline and the dialog into the route**

In `frontend/src/routes/explore/DashboardView.tsx`, add the imports beside the other dashboard
imports:

```tsx
import { isDraftDirty, type DashboardDraft } from "@/components/chart/dashboard/edit-draft";
import { DiscardChangesDialog } from "@/components/chart/dashboard/DiscardChangesDialog";
```

Add the two pieces of state beside `interactingRef` (`:72-73`):

```tsx
  // What the draft looked like when Edit was pressed. Cancel compares against this, and a null
  // baseline means "not editing" — never "clean", so a missing snapshot cannot silently disarm
  // the confirmation.
  const editBaselineRef = useRef<DashboardDraft | null>(null);
  const [confirmingDiscard, setConfirmingDiscard] = useState(false);
```

Clear the baseline in the route-change effect (`:152-159`), alongside the other resets:

```tsx
    editBaselineRef.current = null;
    setConfirmingDiscard(false);
```

Replace `startEdit` / `cancelEdit` (`:240-249`) with:

```tsx
  function startEdit() {
    const { draftTabs: dt, draftCards: dc } = startTabsDraft(dashboard.tabs ?? [], dashboard.cards);
    const dp = (dashboard.parameters ?? []).map((p) => ({ ...p }));
    setDraftCards(dc);
    setDraftParameters(dp);
    setDraftTabs(dt);
    setDraftName(dashboard.name);
    setEditing(true);
    setEditPanel(null);
    // The snapshot Cancel compares against. Taken from the DRAFT, not the server payload:
    // startTabsDraft mints fresh client tab keys, so the two are not the same shape.
    editBaselineRef.current = { name: dashboard.name, cards: dc, parameters: dp, tabs: dt };
  }

  /** Leave edit mode and drop the draft. This is the destructive half — reachable only from a
   *  clean draft, or through the confirmation. */
  function discardEdit() {
    setEditing(false); setDraftCards(null); setDraftParameters(null); setDraftTabs(null);
    setEditPanel(null); setDraftName(""); setNewCardId(null);
    editBaselineRef.current = null;
    setConfirmingDiscard(false);
  }

  /** What the Cancel button calls. Metabase routes Cancel through its leave confirmation whenever
   *  the dashboard is dirty (DashboardHeader.tsx:69-87) and exits straight away when it is not. */
  function requestCancelEdit() {
    const baseline = editBaselineRef.current;
    const current: DashboardDraft = {
      name: draftName, cards: (cards as DraftCard[]), parameters, tabs: draftTabs ?? [],
    };
    if (baseline && isDraftDirty(baseline, current)) { setConfirmingDiscard(true); return; }
    discardEdit();
  }
```

Point the banner's Cancel button (`:457`) at it:

```tsx
            <Button size="sm" variant="ghost" className="text-primary-foreground hover:bg-primary-foreground/15 hover:text-primary-foreground" onClick={requestCancelEdit}>Cancel</Button>
```

Clear the baseline on a successful save, inside `onSave` (`:391`), on the line that already resets
the draft:

```tsx
      setEditing(false); setDraftCards(null); setDraftTabs(null); setEditPanel(null); setDraftName(""); setNewCardId(null);
      editBaselineRef.current = null;
```

Render the dialog beside the other dialogs, immediately after the "Replace chart" `<Dialog>` block
(`:733-738`):

```tsx
      <DiscardChangesDialog
        open={confirmingDiscard}
        onOpenChange={(o) => { if (!o) setConfirmingDiscard(false); }}
        onDiscard={discardEdit}
      />
```

Leave `useUnsavedChangesWarning(editing)` (`:71`) exactly as it is. Narrowing it to
`editing && isDirty` would run `draftFingerprint` on every render, including every frame of a drag,
to tighten a guard that already errs on the safe side.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/routes/explore/__tests__/DashboardView.editsafety.test.tsx`

Expected: PASS, 4 tests.

- [ ] **Step 5: Prove the tests survive mutation**

1. Change `requestCancelEdit` to call `discardEdit()` unconditionally. Re-run. Expected: FAIL on
   three tests. Restore.
2. Change `startEdit` to leave `editBaselineRef.current` as `null`. Re-run. Expected: FAIL on the
   same three (a null baseline must exit, but then the prompt never appears). Restore.
3. Change `discardEdit` to leave `editing` true. Re-run. Expected: FAIL on "Discard changes leaves
   edit mode". Restore.

- [ ] **Step 6: Run the existing DashboardView suite**

Run: `cd frontend && npx vitest run src/routes/explore/__tests__/`

Expected: PASS. The seven existing `DashboardView.*.test.tsx` files all press Edit; none presses
Cancel, so nothing should move. If one does fail, it will be because it pressed Cancel with a dirty
draft — fix the test by confirming the dialog, not by weakening `requestCancelEdit`.

- [ ] **Step 7: Gates and commit**

Run each as its own command. Do not chain them, and do not pipe into `rtk` — a swallowed pipe breaks
the `&&` chain behind it and the later gates silently never run.

```bash
cd frontend && npm run lint
```

```bash
cd frontend && npx tsc --noEmit
```

```bash
cd frontend && npm run test
```

Expected: all green. Record the test count.

```bash
git add frontend/src/routes/explore/DashboardView.tsx \
        frontend/src/routes/explore/__tests__/DashboardView.editsafety.test.tsx
git commit -m "fix(dashboard): confirm before Cancel discards a dirty edit draft"
```

---

## Task 4: The undo-toast helper

**Files:**
- Create: `frontend/src/components/chart/dashboard/draft-undo.ts`
- Test: `frontend/src/components/chart/dashboard/__tests__/draft-undo.test.ts`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/draft-undo.test.ts`:

```ts
import { describe, it, expect, vi, beforeEach } from "vitest";
vi.mock("sonner", () => ({ toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn() }) }));
import { toast } from "sonner";
import { REMOVED_CARD, REPLACED_CARD, deletedTab, undoableDraftEdit } from "../draft-undo";

const lastCall = () => (toast as unknown as { mock: { calls: unknown[][] } }).mock.calls.at(-1)!;

beforeEach(() => vi.clearAllMocks());

describe("draft undo messages", () => {
  it("uses Metabase's wording for the three undoable edits", () => {
    // cards-typed.ts:508, DashboardGrid.tsx:699-713, use-dashboard-tabs.ts:57. The replace message
    // has one form here where Metabase has three, because an egret dashboard card is always a chart.
    expect(REMOVED_CARD).toBe("Removed card");
    expect(REPLACED_CARD).toBe("Chart replaced");
    expect(deletedTab("Overview")).toBe('Deleted "Overview"');
  });
});

describe("undoableDraftEdit", () => {
  it("posts one toast carrying an Undo action", () => {
    undoableDraftEdit(REMOVED_CARD, ["a"], () => {});
    const [message, cfg] = lastCall();
    expect(message).toBe("Removed card");
    expect((cfg as { action: { label: string } }).action.label).toBe("Undo");
  });

  it("hands the restore callback the snapshot taken BEFORE the edit", () => {
    // The snapshot is a parameter rather than something `restore` closes over, because closing
    // over it is how a caller silently captures the state AFTER the edit and makes undo a no-op.
    const before = [{ id: "c1" }, { id: "c2" }];
    const restore = vi.fn();
    undoableDraftEdit(REMOVED_CARD, before, restore);
    const [, cfg] = lastCall();
    expect(restore).not.toHaveBeenCalled();          // nothing happens until Undo is pressed
    (cfg as { action: { onClick: () => void } }).action.onClick();
    expect(restore).toHaveBeenCalledTimes(1);
    expect(restore).toHaveBeenCalledWith(before);
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/draft-undo.test.ts`

Expected: FAIL — `Failed to resolve import "../draft-undo"`.

- [ ] **Step 3: Write the implementation**

Create `frontend/src/components/chart/dashboard/draft-undo.ts`:

```ts
import { toast } from "sonner";

/** Metabase's wording for the three destructive edit-mode actions it makes undoable.
 *  `Removed card` — dashboard/actions/cards-typed.ts:508.
 *  `Question replaced` — dashboard/components/DashboardGrid.tsx:699-713; Metabase branches over
 *  model/metric/question there, and an egret dashboard card is always a chart, so this has one
 *  form where Metabase has three. */
export const REMOVED_CARD = "Removed card";
export const REPLACED_CARD = "Chart replaced";

/** `Deleted "<tab name>"` — dashboard/components/DashboardTabs/use-dashboard-tabs.ts:57. */
export function deletedTab(name: string): string {
  return `Deleted "${name}"`;
}

/**
 * Announce a draft-level destructive edit and offer an Undo that restores `previous`.
 *
 * All three call sites are DRAFT edits — nothing has been PUT — so undo is a `setState` with the
 * snapshot taken immediately before the edit, and never a server round trip. That is also why
 * `previous` is a parameter handed back to `restore` rather than something `restore` closes over:
 * closing over it is how a caller accidentally captures the state AFTER the edit, which turns Undo
 * into a no-op that looks like it worked.
 *
 * The snapshot is the whole slice (every card, or every tab plus every card), not just the one
 * item removed. For the few seconds the toast is up those are the same thing in any realistic
 * sequence, and it is what keeps undo a single state write with no reconciliation.
 */
export function undoableDraftEdit<T>(message: string, previous: T, restore: (previous: T) => void): void {
  toast(message, { action: { label: "Undo", onClick: () => restore(previous) } });
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/draft-undo.test.ts`

Expected: PASS, 3 tests.

- [ ] **Step 5: Prove the tests survive mutation**

Change `onClick: () => restore(previous)` to `onClick: () => {}` and re-run. Expected: FAIL on the
third test. Restore. Change `undoableDraftEdit` to call `restore(previous)` eagerly instead of
inside `onClick` and re-run. Expected: FAIL on the third test's
`expect(restore).not.toHaveBeenCalled()`. Restore.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/dashboard/draft-undo.ts \
        frontend/src/components/chart/dashboard/__tests__/draft-undo.test.ts
git commit -m "feat(dashboard): undo-toast helper for draft-level destructive edits"
```

---

## Task 5: The three undoable draft edits

**Files:**
- Modify: `frontend/src/routes/explore/DashboardView.tsx` (`removeCard` `:261-263`, `replaceCard`
  `:274-277`, the three tab-delete paths at `:547-558` and `:713-728`)
- Test: extend `frontend/src/routes/explore/__tests__/DashboardView.editsafety.test.tsx`

- [ ] **Step 1: Write the failing tests**

Append to `frontend/src/routes/explore/__tests__/DashboardView.editsafety.test.tsx`:

```tsx
describe("DashboardView undo toasts", () => {
  const undoLastToast = () => {
    const [, cfg] = (toast as unknown as { mock: { calls: unknown[][] } }).mock.calls.at(-1)!;
    (cfg as { action: { onClick: () => void } }).action.onClick();
  };

  it("removing a card offers an Undo that puts it back", () => {
    renderView();
    enterEdit();
    expect(screen.getByTestId("card-c1")).toBeTruthy();
    fireEvent.click(screen.getAllByLabelText("remove card")[0]);
    expect(screen.queryByTestId("card-c1")).toBeNull();
    const [message] = (toast as unknown as { mock: { calls: unknown[][] } }).mock.calls.at(-1)!;
    expect(message).toBe("Removed card");
    act(() => undoLastToast());
    expect(screen.getByTestId("card-c1")).toBeTruthy();
  });

  it("removing a card and undoing leaves the draft clean, so Cancel does not prompt", () => {
    // The strongest statement this suite can make about undo: it restores the draft so exactly
    // that the dirtiness check from Task 1 cannot tell the round trip happened.
    renderView();
    enterEdit();
    fireEvent.click(screen.getAllByLabelText("remove card")[0]);
    act(() => undoLastToast());
    fireEvent.click(bannerCancel());
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });
});
```

Add `act` to the Testing Library import at the top of the file, and `import { toast } from "sonner";`
below the `vi.mock("sonner", …)` call (the mock is hoisted, so the import order in the source does
not matter, but keep it beside the other imports).

Tab deletion needs the two-tab fixture, so it gets its own file. Create
`frontend/src/routes/explore/__tests__/DashboardView.tabundo.test.tsx` with the mock harness copied
from `DashboardView.tabs.test.tsx:8-41` (two tabs `A`/`B`, one heading card on each), plus the sonner
mock, and:

```tsx
describe("DashboardView tab delete undo", () => {
  it("deleting an empty tab offers an Undo naming the tab, and restores it", async () => {
    const user = userEvent.setup();
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    // Tab B's card is moved away first so the delete takes the no-dialog fast path.
    fireEvent.click(screen.getByRole("tab", { name: "B" }));
    fireEvent.click(screen.getAllByTestId("dashcard-body")[0]);
    fireEvent.change(screen.getByLabelText("move card to tab"), { target: { value: "A" } });
    await user.click(screen.getByRole("button", { name: "tab menu B" }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    expect(screen.queryByRole("tab", { name: "B" })).toBeNull();
    const [message, cfg] = (toast as unknown as { mock: { calls: unknown[][] } }).mock.calls.at(-1)!;
    expect(message).toBe('Deleted "B"');
    act(() => (cfg as { action: { onClick: () => void } }).action.onClick());
    expect(screen.getByRole("tab", { name: "B" })).toBeTruthy();
  });

  it("reassign-and-delete offers the same Undo and restores the cards to their tab", async () => {
    const user = userEvent.setup();
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    await user.click(screen.getByRole("button", { name: "tab menu A" }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const target = screen.getByLabelText("reassign target") as HTMLSelectElement;
    fireEvent.change(target, { target: { value: target.options[0].value } });
    fireEvent.click(screen.getByRole("button", { name: "reassign and delete" }));
    expect(screen.queryByRole("tab", { name: "A" })).toBeNull();
    const [message, cfg] = (toast as unknown as { mock: { calls: unknown[][] } }).mock.calls.at(-1)!;
    expect(message).toBe('Deleted "A"');
    act(() => (cfg as { action: { onClick: () => void } }).action.onClick());
    expect(screen.getByRole("tab", { name: "A" })).toBeTruthy();
    expect(screen.getByTestId("card-c1")).toBeTruthy();
  });
});
```

> The `move card to tab` select's option values are draft tab **keys** (`tab-N`), not names — read
> `EditSidebar`/`CardSettingsPanel` and select by the option whose label is `A` rather than
> hard-coding `"A"` if the value is a key. A test that names a display string where the code keys on
> a raw one is the exact failure this programme has hit six times.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cd frontend && npx vitest run src/routes/explore/__tests__/DashboardView.editsafety.test.tsx src/routes/explore/__tests__/DashboardView.tabundo.test.tsx
```

Expected: FAIL — `toast.mock.calls.at(-1)` is `undefined`, because nothing toasts today.

- [ ] **Step 3: Write the implementation**

In `frontend/src/routes/explore/DashboardView.tsx`, add the import:

```tsx
import { REMOVED_CARD, REPLACED_CARD, deletedTab, undoableDraftEdit } from "@/components/chart/dashboard/draft-undo";
```

Replace `removeCard` (`:261-263`):

```tsx
  function removeCard(cardId: string) {
    const previous = cards as DraftCard[];
    setDraftCards(previous.filter((c) => c.id !== cardId));
    undoableDraftEdit(REMOVED_CARD, previous, setDraftCards);
  }
```

Replace `replaceCard` (`:274-277`):

```tsx
  function replaceCard(cardId: string, chart: Chart) {
    // Resetting `options` is correct and is NOT the divergence — Metabase discards more than this
    // (cards-typed.ts:245-284). What was missing is the way back, which is the toast.
    const previous = cards as DraftCard[];
    setDraftCards(previous.map((c) => (c.id === cardId ? { ...c, chart_id: chart.id, options: {} } : c)));
    setReplacingId(null);
    undoableDraftEdit(REPLACED_CARD, previous, setDraftCards);
  }
```

Add one helper beside them, which the three tab-delete paths now share:

```tsx
  /** Delete a tab from the draft and offer an Undo, as Metabase does (use-dashboard-tabs.ts:43-61).
   *  All three delete paths — the no-cards fast path and the dialog's two buttons — funnel through
   *  here so the snapshot is taken in exactly one place; three inline copies were three chances to
   *  snapshot the state after the edit instead of before it. */
  function deleteTabWithUndo(key: string, opts: { mode: "reassign"; toKey: string } | { mode: "delete" }) {
    const previous = { tabs: draftTabs ?? [], cards: cards as DraftCard[], activeTabPos };
    const name = previous.tabs.find((t) => t.key === key)?.name ?? "";
    const r = deleteTab(previous.tabs, previous.cards, key, opts);
    setDraftTabs(r.draftTabs);
    setDraftCards(r.draftCards);
    setActiveTabPos(Math.min(activeTabPos, r.draftTabs.length - 1));
    undoableDraftEdit(deletedTab(name), previous, (p) => {
      setDraftTabs(p.tabs); setDraftCards(p.cards); setActiveTabPos(p.activeTabPos);
    });
  }
```

Collapse the tab bar's `onDelete` (`:547-558`) to:

```tsx
          onDelete={(key) => {
            const hasCards = (cards as DraftCard[]).some((c) => c.tabKey === key);
            if (!hasCards) { deleteTabWithUndo(key, { mode: "delete" }); return; }
            const firstOther = (draftTabs ?? []).find((t) => t.key !== key) ?? null;
            setReassignTo(firstOther ? firstOther.key : null);
            setDeletingTabKey(key);
          }}
```

Collapse the dialog's "reassign and delete" handler (`:713-720`) to:

```tsx
              <Button size="sm" variant="outline" aria-label="reassign and delete"
                onClick={() => {
                  if (!deletingTabKey || !reassignTo) return;
                  deleteTabWithUndo(deletingTabKey, { mode: "reassign", toKey: reassignTo });
                  setDeletingTabKey(null); setReassignTo(null);
                }}>reassign and delete</Button>
```

and the "delete cards too" handler (`:721-728`) to:

```tsx
              <Button size="sm" variant="ghost" className="text-destructive" aria-label="delete cards too"
                onClick={() => {
                  if (!deletingTabKey) return;
                  deleteTabWithUndo(deletingTabKey, { mode: "delete" });
                  setDeletingTabKey(null); setReassignTo(null);
                }}>delete cards too</Button>
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd frontend && npx vitest run src/routes/explore/__tests__/DashboardView.editsafety.test.tsx src/routes/explore/__tests__/DashboardView.tabundo.test.tsx
```

Expected: PASS, 6 + 2 = 8 tests.

- [ ] **Step 5: Prove the tests survive mutation**

1. In `removeCard`, change `undoableDraftEdit(REMOVED_CARD, previous, setDraftCards)` to pass
   `cards as DraftCard[]` computed *after* the `setDraftCards` call by moving the line above it —
   i.e. snapshot the wrong state. Re-run. Expected: **PASS**, because React batches and `cards` is
   still the pre-edit render's value. Then instead change the third argument to `() => {}`. Re-run.
   Expected: FAIL on both removal tests. Restore. (The first half of this step exists so the
   implementer sees that a stale-closure bug here would *not* be caught, and therefore keeps the
   snapshot as an explicit parameter.)
2. In `deleteTabWithUndo`, drop the `undoableDraftEdit` call. Re-run. Expected: FAIL on both tab
   tests. Restore.
3. Change `deletedTab(name)` to `deletedTab(key)`. Re-run. Expected: FAIL on both tab tests — the
   message would carry the raw draft key `tab-1` where the UI shows `B`. Restore.

- [ ] **Step 6: Gates and commit**

```bash
cd frontend && npm run lint
```

```bash
cd frontend && npx tsc --noEmit
```

```bash
cd frontend && npm run test
```

```bash
git add frontend/src/routes/explore/DashboardView.tsx \
        frontend/src/routes/explore/__tests__/DashboardView.editsafety.test.tsx \
        frontend/src/routes/explore/__tests__/DashboardView.tabundo.test.tsx
git commit -m "feat(dashboard): undo toasts for remove card, replace card and delete tab"
```

---

## Task 6: The edit-mode guide grid stops shouting

**Files:**
- Create: `frontend/src/components/chart/dashboard/grid-stroke.ts`
- Create: `frontend/src/__tests__/support/wcag.ts`
- Modify: `frontend/src/components/chart/dashboard/DashboardGridLayout.tsx` (the post-W1 version)
- Test: `frontend/src/components/chart/dashboard/__tests__/gridStrokeContrast.test.ts`

Read "The grid stroke, and why the unit test cannot use a canvas" above before starting. A snapshot
test asserts nothing here — the whole defect is a *number*.

- [ ] **Step 1: Write the shared contrast helpers**

Create `frontend/src/__tests__/support/wcag.ts`:

```ts
import { readFileSync } from "node:fs";
import path from "node:path";

/**
 * OKLCH → sRGB, WCAG relative luminance, contrast, source-over compositing, and `index.css` token
 * lookup, for tests that have to assert a colour RELATIONSHIP rather than a colour string.
 *
 * Why this is arithmetic and not a rendered measurement: jsdom runs no layout and no paint, and
 * `canvas` is not a dependency of this project, so `getContext("2d")` returns null and a pixel
 * readback in vitest measures nothing. The conversion is closed-form, `index.css` is the single
 * source of every token involved, and the same method is already used by
 * `src/__tests__/focusIndicatorContrast.test.ts` — including the trap it documents: nothing here
 * regexes a colour into three numbers, because `oklch(L C H)` read as `rgb(R G B)` produces
 * confident garbage.
 *
 * That file keeps its own inline copies of these functions and is deliberately NOT refactored to
 * import them: it is ~900 lines of load-bearing commentary about a published accessibility number,
 * and a mechanical extraction across it is a larger risk than the duplication. New contrast tests
 * import from here.
 */

export type Rgb = [number, number, number];
/** A colour plus the alpha it will be painted at. */
export interface Paint { rgb: Rgb; alpha: number }
export interface Oklch { L: number; C: number; h: number; alpha: number }

const CSS = readFileSync(path.resolve(__dirname, "../../index.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");

/** The published oklab→LMS→linear-sRGB matrices (Ottosson). Gamut-clipped the way a browser clips
 *  when it paints, so the result is the colour that reaches the screen, not an out-of-gamut ideal. */
export function oklchToSrgb(L: number, C: number, hDeg: number): Rgb {
  const h = (hDeg * Math.PI) / 180;
  const a = C * Math.cos(h);
  const b = C * Math.sin(h);
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  const linear = [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ];
  const clamp = (x: number) => Math.min(1, Math.max(0, x));
  const encode = (u: number) => (u <= 0.0031308 ? 12.92 * u : 1.055 * Math.pow(u, 1 / 2.4) - 0.055);
  return linear.map((u) => clamp(encode(clamp(u)))) as Rgb;
}

export function relativeLuminance([r, g, b]: Rgb): number {
  const lin = (c: number) => (c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4));
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

export function contrastRatio(a: Rgb, b: Rgb): number {
  const [x, y] = [relativeLuminance(a), relativeLuminance(b)];
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

/** Source-over composite of `fg` onto an opaque `bg`, in gamma space — which is where a browser
 *  composites an SVG stroke painted over a background colour. */
export function over(fg: Paint, bg: Rgb): Rgb {
  return fg.rgb.map((c, i) => c * fg.alpha + bg[i] * (1 - fg.alpha)) as Rgb;
}

/** The `:root { … }` and `.dark { … }` declaration blocks of index.css, by theme. */
export function themeBlock(theme: "light" | "dark"): string {
  const selector = theme === "light" ? ":root" : "\\.dark";
  const match = CSS.match(new RegExp(`(?:^|\\n)${selector}\\s*\\{([\\s\\S]*?)\\n\\}`));
  if (!match) throw new Error(`no ${theme} theme block found in index.css`);
  return match[1];
}

/** Resolve one custom property to OKLCH components plus alpha. Throws rather than guessing —
 *  a token that silently resolved to a default would make every assertion below meaningless.
 *  Handles the translucent form: `--border` is `oklch(1 0 0 / 10%)` in the dark theme. */
export function tokenOklch(theme: "light" | "dark", name: string): Oklch {
  const decl = themeBlock(theme).match(new RegExp(`--${name}:\\s*([^;]+);`));
  if (!decl) throw new Error(`--${name} is not defined in the ${theme} theme`);
  const raw = decl[1].trim();
  const m = raw.match(/^oklch\(\s*([\d.]+)\s+([\d.]+)\s+([\d.]+)\s*(?:\/\s*([\d.]+)(%?)\s*)?\)$/);
  if (!m) throw new Error(`--${name} in ${theme} is not a plain oklch(): ${raw}`);
  const alpha = m[4] === undefined ? 1 : m[5] === "%" ? Number(m[4]) / 100 : Number(m[4]);
  return { L: Number(m[1]), C: Number(m[2]), h: Number(m[3]), alpha };
}

export function token(theme: "light" | "dark", name: string): Paint {
  const { L, C, h, alpha } = tokenOklch(theme, name);
  return { rgb: oklchToSrgb(L, C, h), alpha };
}
```

- [ ] **Step 2: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/gridStrokeContrast.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import {
  GRID_STROKE_CSS, GRID_STROKE_MAX_CONTRAST, GRID_STROKE_MIN_CONTRAST,
} from "../grid-stroke";
import {
  contrastRatio, oklchToSrgb, over, token, tokenOklch, type Paint,
} from "@/__tests__/support/wcag";

/** The surface the guide grid is painted on: `DashboardView.tsx:452` wraps the whole route in
 *  `bg-dashboard`, and the grid background sits directly on it. */
const CANVAS = "dashboard";

/**
 * Resolve the stroke expression the component ACTUALLY uses, in the given theme.
 *
 * Two forms are supported — a bare `var(--token)` and CSS Color 5's `color-mix(in oklch, …)` —
 * so that restoring the old blend makes this test recompute ITS contrast and fail on the number,
 * rather than failing on a changed string. A test that asserted `GRID_STROKE_CSS === "var(--border)"`
 * would pin the string and measure nothing.
 */
function resolveStroke(expr: string, theme: "light" | "dark"): Paint {
  const plain = expr.match(/^var\(--([\w-]+)\)$/);
  if (plain) return token(theme, plain[1]);

  const mix = expr.match(/^color-mix\(in oklch,\s*var\(--([\w-]+)\)\s+(\d+)%,\s*var\(--([\w-]+)\)\)$/);
  if (!mix) throw new Error(`unsupported stroke expression: ${expr}`);
  const p1 = Number(mix[2]) / 100;
  const a = tokenOklch(theme, mix[1]);
  const b = tokenOklch(theme, mix[3]);
  // CSS Color 5 §3.2: each percentage is scaled by that colour's alpha, the components are mixed
  // alpha-premultiplied, and the result is un-premultiplied by the summed alpha. This is why an
  // 80/20 blend of a 10%-alpha --border with an opaque --foreground lands at alpha 0.28, not 0.10 —
  // and it is exactly the arithmetic that made the dark grid twice as loud as Metabase's.
  const alpha = p1 * a.alpha + (1 - p1) * b.alpha;
  const w1 = (p1 * a.alpha) / alpha;
  const w2 = ((1 - p1) * b.alpha) / alpha;
  return {
    rgb: oklchToSrgb(a.L * w1 + b.L * w2, a.C * w1 + b.C * w2, a.h * w1 + b.h * w2),
    alpha,
  };
}

function paintedContrast(expr: string, theme: "light" | "dark"): number {
  const canvas = token(theme, CANVAS).rgb;
  return contrastRatio(over(resolveStroke(expr, theme), canvas), canvas);
}

describe("the edit-mode grid guide stays inside Metabase's contrast band", () => {
  for (const theme of ["light", "dark"] as const) {
    it(`${theme}: the stroke composites into [${GRID_STROKE_MIN_CONTRAST}, ${GRID_STROKE_MAX_CONTRAST}] against --dashboard`, () => {
      const ratio = paintedContrast(GRID_STROKE_CSS, theme);
      const label = `grid stroke on --dashboard in ${theme}: ${ratio.toFixed(2)}:1`;
      // The ceiling is the defect: a guide grid is a hint under the cards, not a table of boxes,
      // and Metabase's measures 1.27:1. The floor exists only so nobody "fixes" it to nothing.
      expect(ratio, label).toBeLessThanOrEqual(GRID_STROKE_MAX_CONTRAST);
      expect(ratio, label).toBeGreaterThanOrEqual(GRID_STROKE_MIN_CONTRAST);
    });
  }

  it("the pre-fix blend is what this band rules out, in both themes", () => {
    // Guards the evaluator itself. If `resolveStroke` returned the same Paint for both expressions
    // the assertions above would pass no matter what the component painted — the precise failure
    // mode the chart programme hit six times.
    const blend = "color-mix(in oklch, var(--border) 80%, var(--foreground))";
    expect(paintedContrast(blend, "dark")).toBeGreaterThan(GRID_STROKE_MAX_CONTRAST);
    expect(paintedContrast(blend, "light")).toBeGreaterThan(GRID_STROKE_MAX_CONTRAST);
  });
});
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/gridStrokeContrast.test.ts`

Expected: FAIL — `Failed to resolve import "../grid-stroke"`.

- [ ] **Step 4: Write the implementation**

Create `frontend/src/components/chart/dashboard/grid-stroke.ts`:

```ts
/**
 * The colour the edit-mode guide grid is stroked with, as a CSS expression that
 * `DashboardGridLayout` resolves against the live theme by painting it onto a probe element and
 * reading the computed value back — a CSS custom property does not resolve inside an SVG embedded
 * as a data-URL background, so the SVG needs a concrete colour baked in.
 *
 * It is PLAIN `--border`, not a blend toward `--foreground`. The blend was added because `--border`
 * looked "nearly invisible" — but nearly invisible is the entire point of a grid guide. It is a
 * hint under the cards, not a table of boxes, and Metabase's measures 1.27:1 against its own canvas.
 *
 * Composited against `--dashboard` and computed from index.css:
 *
 *   | expression                                        | dark   | light  |
 *   |---------------------------------------------------|--------|--------|
 *   | color-mix(in oklch, --border 80%, --foreground)    | 2.42:1 | 1.77:1 |
 *   | var(--border)                                      | 1.27:1 | 1.11:1 |
 *
 * A 90% blend was tried as a middle ground and rejected: `--border` is 10% white in the dark theme,
 * so mixing in 10% of an opaque `--foreground` nearly doubles the composited alpha (0.10 → 0.19)
 * and the dark contrast climbs back to 1.73:1. Plain `--border` is the only single expression that
 * clears the target in both themes. Pinned by __tests__/gridStrokeContrast.test.ts.
 */
export const GRID_STROKE_CSS = "var(--border)";

/** Used only when the browser resolves `GRID_STROKE_CSS` to the empty string — an SVG with no
 *  stroke draws nothing, so a visible neutral is a better failure than an invisible grid. */
export const GRID_STROKE_FALLBACK = "oklch(0.8 0 0)";

/** The band the composited stroke must land in, against the dashboard canvas, in both themes.
 *
 *  These bounds run the opposite way from every other contrast bound in this codebase. WCAG
 *  1.4.11's 3:1 floor governs information a user must be able to READ; an editing guide is
 *  deliberately not that, so the requirement is a CEILING. The floor is here only so that a future
 *  change cannot satisfy the ceiling by painting nothing at all. */
export const GRID_STROKE_MAX_CONTRAST = 1.5;
export const GRID_STROKE_MIN_CONTRAST = 1.05;
```

Then edit `frontend/src/components/chart/dashboard/DashboardGridLayout.tsx`. **This is the W1
version of the file** — W1 rewrote it, so the lines below are the ones W1 left behind.

Add the import beside the `grid-breakpoints` import:

```tsx
import { GRID_STROKE_CSS, GRID_STROKE_FALLBACK } from "./grid-stroke";
```

Replace the stroke state and its comment:

```tsx
  // CSS custom properties do NOT resolve inside an SVG embedded as a background-image data-URL
  // (the SVG is an isolated document), so we must read a CONCRETE color and bake it in. The
  // expression itself lives in grid-stroke.ts, where the contrast band it has to satisfy is
  // documented and pinned by a test. Theme-aware: re-read whenever edit mode toggles.
  const [stroke, setStroke] = useState(GRID_STROKE_FALLBACK);
```

and inside the effect, replace the probe block:

```tsx
    const probe = document.createElement("div");
    probe.style.color = GRID_STROKE_CSS;
    el.appendChild(probe);
    const resolved = getComputedStyle(probe).color;
    probe.remove();
    setStroke(resolved || GRID_STROKE_FALLBACK);
```

That deletes the second `getComputedStyle(document.documentElement).getPropertyValue("--border")`
read and its `else if` branch: with `GRID_STROKE_CSS` already being `var(--border)`, the fallback
chain was reading the same token twice.

- [ ] **Step 5: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/gridStrokeContrast.test.ts`

Expected: PASS, 3 tests. The failure messages, if you make them fail, print the actual ratio: dark
should read `1.27:1` and light `1.11:1`.

- [ ] **Step 6: Prove the tests survive mutation**

1. Set `GRID_STROKE_CSS` back to
   `"color-mix(in oklch, var(--border) 80%, var(--foreground))"`. Re-run. Expected: FAIL on **both**
   theme tests, with `2.42:1` and `1.77:1` in the messages. Restore.
2. Set `GRID_STROKE_CSS` to `"var(--dashboard)"` — the same colour as the canvas, i.e. an invisible
   grid. Re-run. Expected: FAIL on both theme tests, on the FLOOR (`1.00:1`). Restore.
3. Change `over()` in `wcag.ts` to `return fg.rgb;` (ignore alpha). Re-run. Expected: FAIL on the
   dark theme test — `--border` is 10 % white, and unblended white on the dark canvas is 19:1.
   Restore.

- [ ] **Step 7: Run the grid tests and commit**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/`

Expected: PASS. `grid-background.test.ts` passes a literal stroke of its own and is unaffected.

```bash
cd frontend && npm run lint
```

```bash
cd frontend && npx tsc --noEmit
```

```bash
git add frontend/src/components/chart/dashboard/grid-stroke.ts \
        frontend/src/components/chart/dashboard/DashboardGridLayout.tsx \
        frontend/src/__tests__/support/wcag.ts \
        frontend/src/components/chart/dashboard/__tests__/gridStrokeContrast.test.ts
git commit -m "fix(dashboard): stroke the edit-mode guide grid with --border, at 1.27:1 not 2.42:1"
```

---

## Task 7: What the mapping overlay should show, as a pure function

**Files:**
- Create: `frontend/src/components/chart/dashboard/card-map-state.ts`
- Test: `frontend/src/components/chart/dashboard/__tests__/card-map-state.test.ts`

Read "The mapping overlay: what is in scope at the call site, and what is not" above first. In
particular, the `other-tab` branch is **inert in this workstream** and the test names say so.

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/card-map-state.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import {
  cardMapState, columnsForParam,
  HEADER_COLUMN_TO_FILTER_ON, HEADER_CONNECT_TO, HINT_HEADING_CARD, HINT_LINK_CARD,
  HINT_TEXT_CARD, NO_VALID_FIELDS, OTHER_TAB,
} from "../card-map-state";

const chartCard = {
  cardType: "chart", paramType: "category" as const,
  columns: ["country", "city"], rows: 4,
};

describe("columnsForParam", () => {
  const spec = { dimensions: ["created_at", "country"], columns: ["city"], temporal_units: { created_at: "month" } };
  it("offers dimensions and columns for an ordinary parameter", () => {
    expect(columnsForParam(spec, "category")).toEqual(["created_at", "country", "city"]);
  });
  it("offers only the bucketed columns for a temporal_unit parameter", () => {
    // A "Time grouping" parameter re-grids a column the chart ALREADY buckets; offering it a
    // column with no bucket would produce a mapping the query compiler cannot honour.
    expect(columnsForParam(spec, "temporal_unit")).toEqual(["created_at"]);
    expect(columnsForParam({ dimensions: ["a"], columns: [] }, "temporal_unit")).toEqual([]);
  });
});

describe("cardMapState", () => {
  it("offers the picker with Metabase's header on a chart card", () => {
    expect(cardMapState(chartCard)).toEqual({
      kind: "pick", header: HEADER_COLUMN_TO_FILTER_ON, columns: ["country", "city"],
    });
  });

  it("uses 'Connect to' for a temporal_unit parameter", () => {
    // DashCardCardParameterMapperContent.tsx:111-113.
    const s = cardMapState({ ...chartCard, paramType: "temporal_unit", columns: ["created_at"] });
    expect(s).toMatchObject({ kind: "pick", header: HEADER_CONNECT_TO });
  });

  it("drops the header on a card two rows tall or shorter", () => {
    // DashCardCardParameterMapperContent.tsx:107-109 — there is nowhere to put it.
    expect(cardMapState({ ...chartCard, rows: 2 })).toMatchObject({ kind: "pick", header: null });
    expect(cardMapState({ ...chartCard, rows: 3 })).toMatchObject({ kind: "pick", header: HEADER_COLUMN_TO_FILTER_ON });
  });

  it("says why instead of offering an empty select when nothing can be mapped", () => {
    // This is the live-measured defect: a temporal_unit parameter on a chart with no bucketed
    // column rendered a select whose only option was the placeholder.
    expect(cardMapState({ ...chartCard, columns: [] })).toEqual({ kind: "no-fields", text: NO_VALID_FIELDS });
  });

  it("hints on text and heading cards, and tells the truth about link cards", () => {
    expect(cardMapState({ ...chartCard, cardType: "text" })).toEqual({ kind: "hint", text: HINT_TEXT_CARD });
    // egret interpolates heading text too (DashboardCardView.tsx:104), which Metabase does not —
    // its map gives `heading: ""`. Following egret's behaviour, not Metabase's string table.
    expect(cardMapState({ ...chartCard, cardType: "heading" })).toEqual({ kind: "hint", text: HINT_HEADING_CARD });
    // LinkCard.tsx:8,18 uses the raw url and label with no interpolateParams call, so Metabase's
    // "you can connect widgets to {{variables}} in link cards" would be false here.
    expect(cardMapState({ ...chartCard, cardType: "link" })).toEqual({ kind: "hint", text: HINT_LINK_CARD });
  });

  it("renders nothing on a placeholder card or a chart whose chart is missing", () => {
    expect(cardMapState({ ...chartCard, cardType: "placeholder" })).toEqual({ kind: "none" });
    expect(cardMapState({ ...chartCard, chartMissing: true, columns: [] })).toEqual({ kind: "none" });
  });

  it("has an other-tab branch that is INERT until W7 supplies a filterTabKey", () => {
    // egret parameters are dashboard-level and have no owning card or tab (api/dashboards.ts:54-65),
    // so the route passes `undefined` and this cannot fire today. It is built and pinned here so
    // that W7 (inline parameters) activates it by passing a value, not by redesigning the overlay.
    expect(cardMapState({ ...chartCard, filterTabKey: undefined, cardTabKey: "tab-0" }))
      .toMatchObject({ kind: "pick" });
    expect(cardMapState({ ...chartCard, filterTabKey: null, cardTabKey: "tab-0" }))
      .toMatchObject({ kind: "pick" });
    expect(cardMapState({ ...chartCard, filterTabKey: "tab-1", cardTabKey: "tab-0" }))
      .toEqual({ kind: "other-tab", text: OTHER_TAB });
    expect(cardMapState({ ...chartCard, filterTabKey: "tab-0", cardTabKey: "tab-0" }))
      .toMatchObject({ kind: "pick" });
  });

  it("puts the virtual-card hints ahead of the other-tab check, as Metabase does", () => {
    // DashCardCardParameterMapperContent.tsx checks isVirtual&&isDisabled (146) before
    // isInlineParameterFromAnotherTab (188), so a text card says what it is, not where the filter is.
    expect(cardMapState({ ...chartCard, cardType: "text", filterTabKey: "tab-1", cardTabKey: "tab-0" }))
      .toEqual({ kind: "hint", text: HINT_TEXT_CARD });
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/card-map-state.test.ts`

Expected: FAIL — `Failed to resolve import "../card-map-state"`.

- [ ] **Step 3: Write the implementation**

Create `frontend/src/components/chart/dashboard/card-map-state.ts`:

```ts
import type { ParamType } from "@/api/dashboards";

/** Metabase's header strings (DashCardCardParameterMapperContent.tsx:106-120). `Variable to map to`
 *  is deliberately not ported: it is the header for a NATIVE SQL card with template variables, and
 *  every egret dashboard card is a spec-built chart. */
export const HEADER_COLUMN_TO_FILTER_ON = "Column to filter on";
export const HEADER_CONNECT_TO = "Connect to";

/** Metabase's disabled-mapper tooltip is "This card doesn't have any fields or parameters that can
 *  be mapped to this parameter type." (DashCardCardParameterMapperButton.tsx:92). egret cards carry
 *  no parameters of their own, so that clause is dropped; and this codebase calls them filters, not
 *  parameters, everywhere the user can see. */
export const NO_VALID_FIELDS = "This card doesn't have any fields that can be mapped to this filter.";

/** Metabase says "widgets"; egret's UI says filters, so these match egret's own vocabulary.
 *  Heading gets a hint where Metabase's map gives it `""`, because egret's heading card DOES
 *  interpolate `{{slug}}` (DashboardCardView.tsx:104, same as text at :99). The link string is
 *  NOT Metabase's — `LinkCard.tsx:8,18` uses the raw url and label with no interpolation, so
 *  promising `{{variables}}` there would document a feature egret does not have. */
export const HINT_TEXT_CARD = "You can connect filters to {{variables}} in text cards.";
export const HINT_HEADING_CARD = "You can connect filters to {{variables}} in heading cards.";
export const HINT_LINK_CARD = "Link cards can't use a filter's value.";

/** DashCardCardParameterMapperContent.tsx:192. See `filterTabKey` — inert in egret today. */
export const OTHER_TAB = "The selected filter is on another tab.";

/** What the filter-mapping overlay should show on one card while a filter is selected.
 *  Metabase computes the same disjunction inline inside a component that needs a Redux store, a
 *  Question instance and a dashcard; splitting it out here is what lets it be exhaustively tested
 *  without mounting the 769-line dashboard route. */
export type CardMapState =
  /** The card can take this filter: show the header (unless the card is too short) and the picker. */
  | { kind: "pick"; header: string | null; columns: string[] }
  /** A chart card with nothing on it that this parameter type can attach to. */
  | { kind: "no-fields"; text: string }
  /** A virtual card, which reaches filters some other way — or not at all. */
  | { kind: "hint"; text: string }
  /** The filter belongs to a card on a different tab. Inert until W7; see `filterTabKey`. */
  | { kind: "other-tab"; text: string }
  /** Nothing at all — the overlay does not render. */
  | { kind: "none" };

export interface CardMapInput {
  /** `DashboardCard.card_type`: "chart" | "text" | "heading" | "link" | "placeholder". */
  cardType: string;
  /** The type of the parameter currently selected in the edit sidebar. */
  paramType: ParamType;
  /** The columns this card can offer for that parameter — normally from `columnsForParam`. */
  columns: string[];
  /** The card's height in grid rows (`size_y`). Metabase drops the header at 2 or fewer. */
  rows: number;
  /** True for a chart card whose chart could not be resolved (deleted, or not loaded yet). */
  chartMissing?: boolean;
  /**
   * The tab the selected filter itself lives on, when the filter is pinned to a card.
   *
   * THIS IS INERT TODAY AND DELIBERATELY SO. egret's `DashboardParameter` (api/dashboards.ts:54-65)
   * has no owning card and no tab — every filter is dashboard-level — so `DashboardView` passes
   * `undefined` and this branch cannot fire. It exists, tested, so that W7 (inline parameters)
   * turns it on by passing a value rather than by redesigning the overlay. Do not synthesise a
   * value for it from anything else; there is nothing here that means what Metabase means.
   */
  filterTabKey?: string | null;
  /** The draft tab key of the card being drawn. Only read when `filterTabKey` is set. */
  cardTabKey?: string | null;
}

/** The columns a chart card can offer for one parameter type. A `temporal_unit` parameter re-grids
 *  a column the chart ALREADY buckets, so it can only attach to a key of `spec.temporal_units`;
 *  everything else attaches to any dimension or column. Lifted out of the route (it was inline at
 *  DashboardView.tsx:650-654) so every input to `cardMapState` is pure. */
export function columnsForParam(
  spec: { dimensions: string[]; columns: string[]; temporal_units?: Record<string, string> },
  paramType: ParamType,
): string[] {
  if (paramType === "temporal_unit") return Object.keys(spec.temporal_units ?? {});
  return [...spec.dimensions, ...spec.columns];
}

/** Decide what the overlay shows. The order of the branches mirrors Metabase's
 *  (DashCardCardParameterMapperContent.tsx:146-209): virtual-card hints first, then the tab check,
 *  then the no-fields case, then the picker. A text card should say what it is, not where the
 *  filter is. */
export function cardMapState(input: CardMapInput): CardMapState {
  const { cardType, paramType, columns, rows, chartMissing, filterTabKey, cardTabKey } = input;

  if (cardType === "placeholder") return { kind: "none" };
  if (cardType === "text") return { kind: "hint", text: HINT_TEXT_CARD };
  if (cardType === "heading") return { kind: "hint", text: HINT_HEADING_CARD };
  if (cardType === "link") return { kind: "hint", text: HINT_LINK_CARD };
  if (cardType !== "chart" || chartMissing) return { kind: "none" };

  if (filterTabKey != null && filterTabKey !== cardTabKey) return { kind: "other-tab", text: OTHER_TAB };
  if (columns.length === 0) return { kind: "no-fields", text: NO_VALID_FIELDS };

  const header = rows <= 2
    ? null
    : paramType === "temporal_unit" ? HEADER_CONNECT_TO : HEADER_COLUMN_TO_FILTER_ON;
  return { kind: "pick", header, columns };
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/card-map-state.test.ts`

Expected: PASS, 10 tests.

- [ ] **Step 5: Prove the tests survive mutation**

1. Change `rows <= 2` to `rows <= 1`. Re-run. Expected: FAIL on the header-height test. Restore.
2. Delete the `columns.length === 0` branch. Re-run. Expected: FAIL on "says why instead of offering
   an empty select". Restore.
3. Move the `filterTabKey` check above the `cardType === "text"` check. Re-run. Expected: FAIL on
   "puts the virtual-card hints ahead of the other-tab check". Restore.
4. Change `filterTabKey != null` to `filterTabKey !== undefined`. Re-run. Expected: FAIL on the
   inert-branch test's `filterTabKey: null` case. Restore.
5. In `columnsForParam`, drop the `temporal_unit` branch. Re-run. Expected: FAIL on the second
   `columnsForParam` test. Restore.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/dashboard/card-map-state.ts \
        frontend/src/components/chart/dashboard/__tests__/card-map-state.test.ts
git commit -m "feat(dashboard): decide the filter-mapping overlay's content in a pure function"
```

---

## Task 8: The overlay spans the card and renders the reason

**Files:**
- Modify: `frontend/src/components/chart/dashboard/CardFilterMapOverlay.tsx`
- Modify: `frontend/src/routes/explore/DashboardView.tsx:648-658` (and one line near `:409`)
- Test: rewrite `frontend/src/components/chart/dashboard/__tests__/CardFilterMapOverlay.test.tsx`

- [ ] **Step 1: Rewrite the component's test**

Replace `frontend/src/components/chart/dashboard/__tests__/CardFilterMapOverlay.test.tsx` entirely.
The existing three tests take `columns`/`value`/`onChange`; the component now takes a `CardMapState`,
so they cannot be kept as they are:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { CardFilterMapOverlay } from "../CardFilterMapOverlay";
import {
  HEADER_COLUMN_TO_FILTER_ON, HINT_TEXT_CARD, NO_VALID_FIELDS, OTHER_TAB,
} from "../card-map-state";

const pick = { kind: "pick" as const, header: HEADER_COLUMN_TO_FILTER_ON, columns: ["country", "city"] };

describe("CardFilterMapOverlay", () => {
  it("shows the header and the columns and writes the picked column", () => {
    const onChange = vi.fn();
    render(<CardFilterMapOverlay state={pick} value="" onChange={onChange} />);
    expect(screen.getByText(HEADER_COLUMN_TO_FILTER_ON)).toBeTruthy();
    const sel = screen.getByLabelText("column to filter on") as HTMLSelectElement;
    expect(Array.from(sel.options).map((o) => o.value)).toEqual(["", "country", "city"]);
    fireEvent.change(sel, { target: { value: "city" } });
    expect(onChange).toHaveBeenCalledWith("city");
  });

  it("reflects the current mapping", () => {
    render(<CardFilterMapOverlay state={pick} value="country" onChange={vi.fn()} />);
    expect((screen.getByLabelText("column to filter on") as HTMLSelectElement).value).toBe("country");
  });

  it("gives the select the card's width instead of a fixed 192px", () => {
    // The measured defect: `w-48 max-w-full` resolved to 97px inside a 125px card at 1440px, which
    // is narrower than most column names and could not grow when the card did. Asserting the class
    // rather than a rendered width is deliberate — jsdom does no layout, so a width assertion here
    // would read 0 in every case and pass against anything. The NUMBER is asserted live, Task 9.
    render(<CardFilterMapOverlay state={pick} value="" onChange={vi.fn()} />);
    const sel = screen.getByLabelText("column to filter on");
    expect(sel.className).toContain("w-full");
    expect(sel.className).not.toContain("w-48");
  });

  it("omits the header when the state says there is no room for it", () => {
    render(<CardFilterMapOverlay state={{ ...pick, header: null }} value="" onChange={vi.fn()} />);
    expect(screen.queryByText(HEADER_COLUMN_TO_FILTER_ON)).toBeNull();
    expect(screen.getByLabelText("column to filter on")).toBeTruthy();
  });

  it("renders the reason, and NO select, whenever the card cannot take the filter", () => {
    for (const [state, text] of [
      [{ kind: "no-fields" as const, text: NO_VALID_FIELDS }, NO_VALID_FIELDS],
      [{ kind: "hint" as const, text: HINT_TEXT_CARD }, HINT_TEXT_CARD],
      [{ kind: "other-tab" as const, text: OTHER_TAB }, OTHER_TAB],
    ] as const) {
      const { unmount } = render(<CardFilterMapOverlay state={state} value="" onChange={vi.fn()} />);
      expect(screen.getByText(text)).toBeTruthy();
      expect(screen.queryByLabelText("column to filter on")).toBeNull();
      unmount();
    }
  });

  it("renders nothing at all for the none state", () => {
    const { container } = render(<CardFilterMapOverlay state={{ kind: "none" }} value="" onChange={vi.fn()} />);
    expect(container.firstChild).toBeNull();
  });

  it("carries dash-nodrag so the grid does not hijack the select", () => {
    const { container } = render(<CardFilterMapOverlay state={pick} value="" onChange={vi.fn()} />);
    expect(container.querySelector(".dash-nodrag")).toBeTruthy();
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/CardFilterMapOverlay.test.tsx`

Expected: FAIL — the component takes `columns`, not `state`, so TypeScript errors and every
assertion that depends on the new states goes red.

- [ ] **Step 3: Write the implementation**

Replace `frontend/src/components/chart/dashboard/CardFilterMapOverlay.tsx` entirely:

```tsx
import type React from "react";
import { Info } from "lucide-react";
import type { CardMapState } from "./card-map-state";

interface Props {
  /** What to show, decided by `cardMapState`. */
  state: CardMapState;
  value: string;
  onChange: (col: string) => void;
}

/** Overlay shown on each card while a filter is selected in edit mode.
 *
 *  Two changes from what this used to be. The select is `w-full`, not `w-48 max-w-full`: that fixed
 *  width measured 97px inside a 125px card at 1440px — narrower than most column names, and unable
 *  to grow when the card did. And a card that cannot take the current filter now says why, instead
 *  of rendering an empty select or, for text/heading/link cards, nothing at all. Metabase does both
 *  (DashCardCardParameterMapperContent.tsx:106-215); the reasons themselves are decided in
 *  card-map-state.ts, so this component only lays them out. */
export function CardFilterMapOverlay({ state, value, onChange }: Props) {
  if (state.kind === "none") return null;

  const shell = (body: React.ReactNode) => (
    <div className="dash-nodrag absolute inset-0 z-20 flex flex-col items-center justify-center gap-2 bg-card/85 p-3 text-center backdrop-blur-[1px]">
      {body}
    </div>
  );

  if (state.kind !== "pick") {
    return shell(
      <div className="flex items-start gap-1.5 text-xs text-muted-foreground">
        <Info className="mt-px h-3 w-3 shrink-0" aria-hidden />
        <span>{state.text}</span>
      </div>,
    );
  }

  return shell(
    <>
      {state.header && (
        <div className="w-full truncate text-xs font-medium text-muted-foreground">{state.header}</div>
      )}
      <select aria-label="column to filter on" className="h-8 w-full rounded border bg-background px-2 text-sm"
        value={value} onChange={(e) => onChange(e.target.value)}>
        <option value="">Select a column…</option>
        {state.columns.map((c) => <option key={c} value={c}>{c}</option>)}
      </select>
    </>,
  );
}
```

- [ ] **Step 4: Wire the call site**

In `frontend/src/routes/explore/DashboardView.tsx`, add the import:

```tsx
import { CardFilterMapOverlay } from "@/components/chart/dashboard/CardFilterMapOverlay";
import { cardMapState, columnsForParam } from "@/components/chart/dashboard/card-map-state";
```

(the first line already exists at `:25`; add only the second).

Immediately after `activeParamId` (`:409`), add:

```tsx
  const activeParam = activeParamId ? parameters.find((p) => p.id === activeParamId) : undefined;
```

Replace the overlay block (`:648-658`) with:

```tsx
                {activeParamId && activeParam && (
                  <CardFilterMapOverlay
                    state={cardMapState({
                      cardType: c.card_type,
                      paramType: activeParam.type,
                      columns: baseChart ? columnsForParam(baseChart.params.spec, activeParam.type) : [],
                      rows: c.size_y,
                      chartMissing: c.card_type === "chart" && !baseChart,
                      // egret filters are dashboard-level and have no owning tab; W7 supplies this.
                      filterTabKey: undefined,
                    })}
                    value={mappings[activeParamId] ?? ""}
                    onChange={(col) => updateCardOptions(c.id, { paramMappings: { ...mappings, [activeParamId]: col } })}
                  />
                )}
```

Note what moved: the `c.card_type === "chart" && baseChart` guard is gone from the condition, which
is what lets a text card render its hint; the guard now lives inside `cardMapState` and returns
`{ kind: "none" }`, which the component renders as `null`. The temporal-unit column branch moved
into `columnsForParam`.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cd frontend && npx vitest run src/components/chart/dashboard/__tests__/CardFilterMapOverlay.test.tsx
```

Expected: PASS, 7 tests.

```bash
cd frontend && npx vitest run src/routes/explore/__tests__/
```

Expected: PASS. `DashboardView.editsidebar.test.tsx` and `DashboardView.clickbehavior.test.tsx` touch
the filter panel; if one of them now finds an overlay on a heading card where it previously found
none, that is the fix working — update the test's expectation, do not restore the old guard.

- [ ] **Step 6: Prove the tests survive mutation**

1. Change the select's class back to `w-48 max-w-full`. Re-run. Expected: FAIL on the width test.
   Restore.
2. In the component, change `if (state.kind !== "pick")` to `if (false)`. Re-run. Expected: FAIL on
   the reason test (TypeScript will also refuse — that is fine, run it anyway to see it). Restore.
3. At the call site, restore the `c.card_type === "chart" && baseChart &&` guard in the JSX
   condition. Re-run `src/routes/explore/__tests__/`. Expected: this does **not** fail, because no
   route test covers a text card with the filter panel open. Add one to
   `DashboardView.editsafety.test.tsx` that opens a filter and asserts
   `screen.getByText(HINT_HEADING_CARD)` on the fixture's heading card, then re-run and confirm the
   restored guard makes it FAIL. Restore the guard removal and keep the new test.

- [ ] **Step 7: Gates and commit**

```bash
cd frontend && npm run lint
```

```bash
cd frontend && npx tsc --noEmit
```

```bash
cd frontend && npm run test
```

Expected: all green. Record the test count.

```bash
git add frontend/src/components/chart/dashboard/CardFilterMapOverlay.tsx \
        frontend/src/components/chart/dashboard/__tests__/CardFilterMapOverlay.test.tsx \
        frontend/src/routes/explore/DashboardView.tsx \
        frontend/src/routes/explore/__tests__/DashboardView.editsafety.test.tsx
git commit -m "fix(dashboard): give the filter-mapping control the card's width and a reason when it can't map"
```

---

## Task 9: Live check — measure, do not eyeball

**Files:** none. This task produces the numbers for the PR body.

Every defect in this workstream was found by measuring the running app. Three of the four cannot be
seen at a glance: 2.42:1 versus 1.27:1 looks like "a grid", 97px versus 300px looks like "a select",
and a discard prompt that never fires looks like nothing at all.

- [ ] **Step 1: Build and deploy**

```bash
just deploy-kind
```

Expected: `deployment "egret" successfully rolled out`, SPA served at `http://localhost:30080`.

Open `http://localhost:30080/explore/demo/dashboards/8a196343-ad93-4968-8040-67598d29c3cb`
(admin@egret.local / Admin12345!) at 1440×900. Do not modify the harness dashboard — every
measurement below either changes nothing or ends in Discard/Undo.

- [ ] **Step 2: The dirty draft prompts, and the prompt is escapable**

1. Press **Edit**. Change the dashboard name to `ZZ Chart Parity X`.
2. Press **Cancel**. Evaluate:

```js
(() => {
  const d = document.querySelector('[role=alertdialog]');
  return {
    dialogs: document.querySelectorAll('[role=alertdialog]').length,
    title: d?.querySelector('h2,[id]')?.textContent?.trim() ?? null,
    buttons: [...(d?.querySelectorAll('button') ?? [])].map(b => b.textContent.trim()),
  };
})()
```

Required: `{ dialogs: 1, title: "Discard your changes?", buttons: ["Cancel", "Discard changes"] }`.

3. Press the dialog's **Cancel**. Evaluate:

```js
({ dialogs: document.querySelectorAll('[role=alertdialog]').length,
   name: document.querySelector('input[aria-label="dashboard name"]')?.value ?? null })
```

Required: `{ dialogs: 0, name: "ZZ Chart Parity X" }` — the draft survived cancelling the cancel.
This is the assertion §6 of the design names for W2.

4. Press **Cancel**, then **Discard changes**. Required: the name input is gone and the heading reads
   `ZZ Chart Parity`.

- [ ] **Step 3: A clean draft does NOT prompt**

This is the `compactType: "vertical"` hazard from the Background section, and it is the one number
in this task that could go the wrong way.

Press **Edit**, wait two seconds for the grid to settle, touch nothing, press **Cancel**. Evaluate:

```js
({ dialogs: document.querySelectorAll('[role=alertdialog]').length,
   editing: !!document.querySelector('input[aria-label="dashboard name"]') })
```

Required: `{ dialogs: 0, editing: false }`.

If `dialogs` is 1, react-grid-layout compacted the saved layout on mount and wrote new positions into
the draft. Do **not** relax `isDraftDirty` to make this go away — that would re-open the data-loss
hole from the other side. Diagnose it by logging `draftFingerprint` at `startEdit` and again at
`requestCancelEdit` and diffing the two strings; the fix is to take the baseline after RGL's first
`onLayoutChange` settles, not to weaken the comparison. Record the finding either way.

- [ ] **Step 4: The grid stroke, measured in a real compositor**

Press **Edit** (the guide grid only paints while editing, and after W1 only on the desktop
breakpoint). Evaluate:

```js
(() => {
  const layer = document.querySelector('.react-grid-layout')
    ?.parentElement?.querySelector('[aria-hidden]');
  const stroke = decodeURIComponent(getComputedStyle(layer).backgroundImage).match(/stroke='([^']+)'/)[1];
  const canvasEl = document.querySelector('.bg-dashboard') ?? document.body;
  const canvasColour = getComputedStyle(canvasEl).backgroundColor;
  const cv = document.createElement('canvas'); cv.width = cv.height = 1;
  const ctx = cv.getContext('2d');
  const read = (fill, base) => {
    ctx.clearRect(0, 0, 1, 1);
    if (base) { ctx.fillStyle = base; ctx.fillRect(0, 0, 1, 1); }
    ctx.fillStyle = fill; ctx.fillRect(0, 0, 1, 1);
    const [r, g, b] = ctx.getImageData(0, 0, 1, 1).data;
    return [r / 255, g / 255, b / 255];
  };
  const lum = ([r, g, b]) => {
    const f = c => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
    return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
  };
  const canvas = read(canvasColour);
  const painted = read(stroke, canvasColour);
  const [x, y] = [lum(painted), lum(canvas)];
  return { stroke, canvasColour,
           ratio: +(((Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05)).toFixed(2)) };
})()
```

Required: `ratio <= 1.50`. Expected `1.27` in the dark theme and `1.11` in the light theme; the
pre-fix baseline is `2.42` dark / `1.77` light. Run it in **both** themes (toggle from the header)
and record both numbers. If the browser's composited number differs from the unit test's by more
than 0.02, say so in the PR body — that gap is the difference between the arithmetic and the
compositor, and it is worth knowing rather than smoothing over.

- [ ] **Step 5: The mapping control spans the card, and states its reason**

Still in edit mode, open a filter (click a chip in the edit filter bar, or **Add a filter**), then
evaluate:

```js
(() => {
  const sels = [...document.querySelectorAll('select[aria-label="column to filter on"]')];
  const widths = sels.map(s => {
    const card = s.closest('[data-testid="dashcard-body"]');
    return { sel: Math.round(s.getBoundingClientRect().width),
             card: Math.round(card.getBoundingClientRect().width) };
  });
  const reasons = [...document.querySelectorAll('.dash-nodrag.absolute.inset-0')]
    .filter(o => !o.querySelector('select'))
    .map(o => o.textContent.trim());
  return { pickers: sels.length, widths: widths.slice(0, 3), reasons: [...new Set(reasons)] };
})()
```

Required: every entry's `sel` is `card - 24` ± 2 (the overlay's `p-3` on each side), and `sel` is
well over the pre-fix baseline of `97` in a `125` card. `reasons` must be non-empty on the harness
dashboard — it carries heading cards, so at least
`"You can connect filters to {{variables}} in heading cards."` should appear. Record `widths` and
`reasons` verbatim.

Then press **Cancel** → **Discard changes** so the harness is left exactly as found.

- [ ] **Step 6: The undo toasts, counted**

Press **Edit**. Hover a chart card, press its remove button, and evaluate immediately:

```js
({ cards: document.querySelectorAll('.react-grid-item').length,
   toast: document.querySelector('[data-sonner-toast]')?.textContent?.trim() ?? null })
```

Required: `cards` is one fewer than before, and `toast` contains `Removed card` and `Undo`. Click
**Undo** and re-evaluate: `cards` is back to the original count. Repeat for a tab delete (expect
`Deleted "<name>"`) and a chart replace (expect `Chart replaced`). Record the three before/after
card counts and the three message strings.

Press **Cancel** → **Discard changes**.

- [ ] **Step 7: Confirm the PR contains only this workstream**

```bash
git log --oneline feat/dashboard-parity-01-responsive-grid..HEAD
```

Expected: exactly the eight commits from Tasks 1-8, and nothing from W1.

- [ ] **Step 8: Open the PR and stop**

```bash
gh pr create --base feat/dashboard-parity-01-responsive-grid \
  --title "feat(dashboard): W2 edit-mode safety and fidelity"
```

The body carries, verbatim: the four discard-dialog measurements from Steps 2 and 3; both grid
contrast ratios from Step 4 with their pre-fix baselines; the `widths`/`reasons` object from Step 5;
the three undo before/after card counts from Step 6; and the frontend test count from the last
`npm run test`. A count that differs from W1's PR by more than the tests added here means a gate did
not run.

Then stop. The merge is the user's call.

---

## Self-review checklist for the implementer

- [ ] `isDraftDirty` is exported, pure, and covered by a test that goes red when either the sorting
      rule for cards or the *absence* of a sorting rule for parameters is changed.
- [ ] `DashboardView.tsx` did not grow. Check it: `git diff --stat feat/dashboard-parity-01-responsive-grid -- frontend/src/routes/explore/DashboardView.tsx`
      should show roughly as many deletions as insertions — the three inline tab-delete bodies
      collapsing into `deleteTabWithUndo` is what pays for the baseline ref, the dialog and the
      overlay props.
- [ ] Every undo snapshot is passed to `undoableDraftEdit` as a parameter, never captured inside the
      `restore` closure.
- [ ] Tab delete goes through `deleteTabWithUndo` from all **three** call sites, not one.
- [ ] The grid contrast test computes a ratio from `index.css` and fails on the *number*; nothing
      asserts that `GRID_STROKE_CSS === "var(--border)"`.
- [ ] The canvas-pixel readback appears only in Task 9, in a real browser. No vitest test calls
      `getContext("2d")`.
- [ ] The mapping overlay renders for **every** card kind; the `card_type === "chart"` guard lives
      inside `cardMapState`, not in the JSX condition.
- [ ] The `other-tab` branch is documented as inert in the code, in its test name, and in the PR
      body. It is not wired to a fabricated source of truth.
- [ ] The link-card hint says what egret does, not what Metabase's string says.
- [ ] No test asserts a display string where the code keys on a raw one — in particular the tab
      undo test names the tab `B`, and the code keys on `tab-1`.
- [ ] Nothing here touches the per-card edit toolbar or "Replace chart"'s `options` reset; §9 of the
      design records both as non-defects.
