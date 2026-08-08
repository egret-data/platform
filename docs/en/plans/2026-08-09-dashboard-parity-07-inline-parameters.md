# W7 — Inline Parameters Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a dashboard author move a filter off the top bar and into a heading or question card,
as Metabase's `MoveParameterMenu` does, and collapse that card's filter list to a "Show filters"
button when the card is too narrow to hold both its title and its filters.

**Architecture:** an inline parameter is a **placement**, not a new kind of filter. The parameter
stays in `dashboard.parameters`, keeps its slug, keeps its per-card `paramMappings`, and keeps
filtering every card it is mapped to. All that changes is *where its widget is drawn*. So every
decision in this workstream is one of two pure questions — "which card, if any, hosts this
parameter?" and "does this row have room for the list?" — and both live in
`components/chart/dashboard/inlineParameters.ts` with `DashboardView.tsx` holding wiring only.
That rule is not stylistic: `DashboardView.tsx` is 769 lines and seven of the nine workstreams
touch it.

**Tech Stack:** React 18 + TypeScript, Tailwind v4 (OKLCH tokens), Radix (`Popover`), vitest +
Testing Library. **No new dependency.**

**Design reference:** `docs/en/specs/2026-08-09-dashboard-parity-design.md` §4 W7. §9 lists findings
that did **not** survive inspection — do not plan work for them.

**Metabase reference:** the sibling checkout at `data-portal/metabase`. Files quoted in this plan:
`dashboard/utils.ts:118-149` (`supportsInlineParameters`, `hasInlineParameters`,
`findDashCardForInlineParameter`),
`dashboard/components/ParameterSettings/MoveParameterMenu/MoveParameterMenu.tsx:30,67-134`,
`dashboard/components/ParameterSettings/ParameterSettings.tsx:293`,
`dashboard/components/CollapsibleDashboardParameterList/CollapsibleDashboardParameterList.tsx:36,40-86,87-101`,
`dashboard/hooks/use-responsive-parameter-list.ts:23-35`,
`dashboard/components/DashCard/DashCardVisualization.tsx:436-446,468-487`,
`dashboard/visualizations/Heading/Heading.tsx:110-215`,
`dashboard/selectors.ts:447-480`,
`dashboard/actions/cards-typed.ts:245-284` (replace), `:398-441` (duplicate), `:472-523` (remove),
`dashboard/actions/parameters.tsx:172-269` (move), `:357-401` (remove parameter).

**Branch:** `feat/dashboard-parity-07-inline-parameters`, branched from
**`feat/dashboard-parity-06-filters`**, not from `main`.

```bash
git fetch origin
git checkout feat/dashboard-parity-06-filters
git checkout -b feat/dashboard-parity-07-inline-parameters
```

The PR is opened **against `feat/dashboard-parity-06-filters`**, so W6's commits are not re-included
in the diff. If W6 has already merged to `main` by the time this branch opens, rebase onto `main`
and target `main` instead — but never cherry-pick W6's commits into this branch.

**W7 is on the cut line.** The index (`…-00-index.md`) says W7, W8 and W9 can each be dropped
without leaving anything half-built. Concretely, for this workstream that means: **nothing outside
this branch may come to depend on `inlineParameters.ts`.** Deleting the branch must restore the
previous behaviour exactly, which it does — every parameter simply has no host card, so
`topBarParameters` returns all of them and every card renders as it does on W6.

---

## Background the implementer needs

### The field this workstream adds, and why it goes on the CARD and not on the parameter

The obvious shape is `DashboardParameter.cardId` — one field, in a JSON blob the backend already
takes unvalidated (`crates/egret-analytics-api/src/bi.rs:365,424`: `parameters: serde_json::Value`).
**It is wrong, and the reason is in the save path.**

`serializeLayout` (`tabs.ts:34-48`) sends cards **without ids**, and `replace_layout`
(`crates/egret-bi/src/dashboard.rs:344-376`) does `DELETE FROM analytics.dashboard_cards WHERE
dashboard_id = $1` followed by an `INSERT` per card. **Every card id churns on every save.** A
`parameter.cardId` would therefore point at a row that no longer exists the moment the dashboard is
saved, and the filter would silently reappear on the top bar.

Parameter ids do **not** churn: `parameters` is one JSONB column written verbatim
(`dashboard.rs:180-200`, `UPDATE … SET parameters=$4`) and read back verbatim
(`bi.rs:365`). Card `options` is likewise verbatim on both legs — `CardInput.options` is
`serde_json::Value` with `#[serde(default = "empty_obj")]` and **no validation**
(`bi.rs:393-394`), it is bound straight into the `options` jsonb column (`dashboard.rs:373`), and
`DashboardCardDto.options` echoes it back (`bi.rs:349`).

**So the field is `card.options.inlineParameters: string[]` — a list of PARAMETER ids hosted by
this card.** This is Metabase's shape too (`dashcard.inline_parameters`, `utils.ts:128`), arrived at
here for a different and stronger reason. It needs **no migration, no DTO change and no Rust
change**, exactly like `options.hideEmpty` before it (`hide-empty.ts:6-9` records the same
argument).

`DashboardCard.options` is already typed `Record<string, unknown>` (`api/dashboards.ts:27`), so
TypeScript needs no change either; Task 1 adds a doc comment naming the key, because an untyped bag
with six live keys deserves an index.

### Which card types may host a filter

Metabase: `supportsInlineParameters` is `isQuestionDashCard(dashcard) || isHeadingDashCard(dashcard)`
(`utils.ts:118-122`) — the question card and the heading card, nothing else.

egret's `card_type` vocabulary is `chart | text | heading | link | placeholder`
(`DashboardCardView.tsx:97-111`; the column is free text with no CHECK,
`migrations/0015_dashboard.sql:21`). The rule maps one-for-one: **`chart` and `heading`**. egret's
`chart` card *is* Metabase's question card. `text` is markdown that already interpolates `{{slug}}`
inline (`params.ts` `interpolateParams`), `link` is a single anchor, `placeholder` has no content at
all — none of the three has a header row a filter widget could sit in, which is precisely the line
Metabase draws.

### An inline parameter still filters EVERY card it is mapped to

This is the question the name gets wrong, and the answer is unambiguous in the code.
`getDashboardHeaderParameters` (`selectors.ts:447-454`) subtracts inline parameters **from the
header list only**. `getParameters` — the list that feeds parameter *values*, `parameter_mappings`
and every query — is untouched. Moving a filter onto a card changes nothing about what it filters.

egret is the same by construction: `cardQueryArgs` (`DashboardView.tsx:216-229`) builds each card's
filters with `paramFiltersForCard(parameters, values, mappings)` from the **full** `parameters`
array, and `applyParamsToChart` / `interpolateParams` do likewise. **The filtered list must reach
exactly two call sites** — the view-mode `DashboardFilterBar` and the edit-mode `EditFilterChips`
(both at `DashboardView.tsx:562-564`). Feeding it anywhere else silently un-applies the filter, and
the symptom is a card that quietly stops being filtered rather than an error.

### The host lookup is GLOBAL, not per-tab

`getDashboardHeaderParameters` composes `getCurrentDashcards`, which is
`dashboard?.dashcards || []` (`selectors.ts:223-226`) — **every card on the dashboard, not the
active tab's**. So a filter hosted by a card on Tab 2 is absent from the top bar while you are
looking at Tab 1. That is deliberate: the alternative makes a filter appear and disappear as you
switch tabs, which reads as a bug.

egret must match: `findHostCardId` scans `cards`, not `visibleCards` and not `tabCards`.

### The collision test, and why the reserved width must be `scrollWidth`

Metabase's predicate, verbatim (`use-responsive-parameter-list.ts:29-34`):

```ts
const requiredWidth = reservedWidth + parametersWidth + bufferSpace;
return requiredWidth > containerWidth;
```

with `reservedWidth = measureTextWidth(cardTitle, …)` and `bufferSpace: 100` for a question card
(`DashCardVisualization.tsx:436-446`, commented "Bigger buffer space to account for varying chart
padding"), and the default `bufferSpace = 24` for a heading card (`Heading.tsx:110-116`).

Two porting traps:

1. **`getBoundingClientRect().width` on the title node is the wrong number.** The title sits in a
   flex row with `min-w-0 truncate`, so its box *shrinks to the space available* — the sum can then
   never exceed the container and the list never collapses. Metabase sidesteps this by measuring the
   text on a canvas. The DOM equivalent of "how wide would this text like to be" is
   **`scrollWidth`** on the truncating element, and that is what this plan uses.
2. **The list must be measured EXPANDED.** Measuring the visible list reads the collapsed trigger's
   width once collapsed, concludes there is room, expands, and oscillates. Metabase renders a second,
   invisible, always-expanded copy purely for measurement
   (`CollapsibleDashboardParameterList.tsx:87-101`, commented "Invisible expanded parameter list for
   measurements"). Port that.

jsdom reports `0` for every width and has no `ResizeObserver`, so **the predicate is a pure exported
function taking four numbers** and is tested with the numbers fed in directly. The measurement hook
around it is inert in jsdom (guarded exactly as `DashboardGridLayout.tsx:404` guards its own
`ResizeObserver`), and the real geometry is measured in the browser in Task 7.

### egret's 100px buffer is not slack — it is already spoken for

On a question card the row that holds the title also has to clear the hover export affordance.
`ChartExportMenu`'s trigger is `<Button size="sm">` with a single 16px glyph, which resolves to
`h-8 … has-[>svg]:px-2.5` (`components/ui/button.tsx:66`) = **36px wide**, positioned
`absolute right-1 top-1` (`DashboardCardView.tsx:66`) — and `absolute` means it contributes **no
layout width at all**. Add the row's own 16px left padding and 64 of Metabase's 100px buffer is
accounted for before a single pixel of "varying chart padding". The heading card is chrome-less
(`DashboardView.tsx:631` gives it `bg-dashboard`, `HeadingCard.tsx:28` gives it `px-1`), has no
export menu, and keeps the 24px default.

**This is a real layout collision, not a detail.** Without a reserved right gutter the hover menu
draws on top of the last filter pill, and it only does so on hover, so it survives every static
review. Task 3 reserves it with `pr-10` (40px = the 36px button + its 4px offset) on the header row,
and only when `canExport` is true.

### What happens to a card-hosted filter when its host card is deleted, moved or replaced

Decided from Metabase's code, one case at a time. All four are draft-level; nothing reaches the
server until Save, and Cancel restores the whole draft.

| Case | egret behaviour | Why |
|---|---|---|
| **Host card deleted** (`removeCard`, `DashboardView.tsx:261-263`) | The hosted parameters are **deleted with the card**, and their ids are stripped from every remaining parameter's `filteringParameters`. | Metabase does exactly this (`cards-typed.ts:490-495` calls `removeParameterAndReferences` per inline parameter, `parameters.tsx:357-376`). Returning them to the top bar instead would make filters materialise out of a delete, which is the surprise Metabase's design avoids. Recovery today is **Cancel**; W2's "remove card" undo toast covers it properly when it lands, and W7 does not depend on W2. |
| **Host card moved to another tab** (`moveCardToTab`, `tabs.ts:137-140`) | The filter **travels with its card**. It disappears from the top bar of both tabs and appears on the card wherever the card now is. **No code change** — `moveCardToTab` spreads `{...c}`, so `options` rides along. | Metabase keeps `inline_parameters` on the dashcard through a tab move, and `setEditingParameter` even switches the view to the host's tab to follow it (`parameters.tsx:279-291`). Task 6 pins this with a test so a future refactor of `moveCardToTab` that rebuilds `options` breaks loudly instead of silently orphaning filters. |
| **Host card replaced** (`replaceCard`, `DashboardView.tsx:274-277`) | `options` is still reset to `{}` **except** `inlineParameters`, which is carried across. | Metabase's `replaceCard` sets `card`, `card_id`, `series`, `parameter_mappings`, `visualization_settings` and **leaves `inline_parameters` untouched** (`cards-typed.ts:262-270`). The list is a placement, not a property of the chart. §9 of the design records that the `options: {}` reset is *not* a divergence; this narrows it by one key, and the narrowing is what stops "Replace chart" from teleporting a filter back to the top bar. |
| **Host card duplicated** (`duplicateCard` `:265-273`, `duplicateTab` `tabs.ts:130-132`) | The clone gets **no** inline parameters; the original keeps them. | This one is a **deliberate divergence**, and it must be stated in the PR body. Metabase duplicates the parameters themselves, mints new ids and remaps `parameter_mappings` (`cards-typed.ts:416-441`). egret's clone path today does a shallow `{...src, options: {...src.options}}`, so without a strip **two cards would claim the same parameter id** — `findHostCardId` returns the first, the clone renders nothing, and the dashboard has an invisible inconsistency. Stripping is one line and cannot be wrong; minting duplicate filters (with fresh slugs that then enter the URL contract) is a second feature and belongs in its own change. |

### What happens at W1's mobile breakpoint

W1 (`…-01-responsive-grid.md`) stacks every card into one column below 753px of **container** width
and forces per-kind heights — `heading` 2 rows, `scalar` 4, everything else 6.

**W7 needs no mobile special case, and must not import W1's module.** This branch is cut from W6,
which is cut from `main`, so `grid-breakpoints.ts` does not exist here — and coupling to it would
break the cut line in both directions. The collapse predicate is driven purely by measured widths,
so mobile falls out of the arithmetic:

- at a 390px viewport the card container is ~362px (W1's measured number), so for a question card
  `reservedWidth + listWidth + 100 > 362` is true for any non-trivial title, and the list collapses
  to the "Show filters" trigger by itself;
- a heading card on mobile is 2 rows ≈ 92px tall, and the collapsed trigger is a 32px-high button —
  it fits. An expanded list would not, which is exactly why the predicate collapses it;
- a `FilterPill` is `h-8` = 32px (`FilterPill.tsx:51`) and the grid's minimum row height is 40px
  (`dashboard-grid.ts:5`), so even a **1-row** heading — the desktop default, `24 × 1`
  (`dashboard-grid.ts:76`) — has room for the collapsed trigger;
- drag and resize are off at that breakpoint, and the inline chips are click targets, not drag
  handles, so nothing competes.

When W1 lands, the only interaction is that a card gets narrower and therefore collapses. Verify it
in Task 7 Step 6 at 390px.

### One strand this workstream deliberately does not close

A filter hosted by a card that "Hide this card if there are no results" has hidden is not on screen,
so its widget is unreachable until the card comes back. Its **value still applies** — it lives in
the URL and in `parameters` — and the filter does not return to the top bar, matching the global
host lookup above.

The dangerous version of this — a **required** inline filter on a hide-empty card, which would block
every card forever — **cannot happen**: `hiddenEmptyCardIds` returns an empty set whenever a required
parameter is unset (`hide-empty.ts:39`, the `blocked` guard), so the host card renders (blocked, with
its "Set the required filters to load this card" body) and its filter is right there. Say this in the
PR body rather than leaving the reader to work it out.

---

## Files this plan adds beyond the index's table, and why

The index's file table predates W7 and lists no files for it. Four are added here, and every one is
a pure module or a small component extracted **so `DashboardView.tsx` does not grow** — the rule the
table exists to enforce.

| New file | Why it is not in `DashboardView.tsx` |
|---|---|
| `inlineParameters.ts` | The whole model: host lookup, the top-bar/inline split, the move, the lifecycle transforms, the move-menu destinations, and the collapse predicate. Pure; every behaviour in this workstream is tested against it. |
| `useCollapsedParameterList.ts` | The three-ref measurement hook. A hook, not route code, and inert in jsdom. |
| `CardInlineFilters.tsx` | The list a card draws: expanded pills, or the "Show filters" popover. Rendered by `DashboardCardView`/`HeadingCard`, so the route passes one prop object and nothing else. |
| `MoveParameterField.tsx` | The grouped destination `<select>`. Rendered by `FilterSettingsPanel`. |

---

## File structure

| Path | Responsibility after this workstream |
|---|---|
| `frontend/src/components/chart/dashboard/inlineParameters.ts` | **New.** `supportsInlineParameters`, `inlineParameterIds`, `findHostCardId`, `inlineParametersFor`, `topBarParameters`, `mergeTopBarOrder`, `moveParameterToCard`, `dropParameters`, `keepInlineParameters`, `withoutInlineParameters`, `destinationLabel`, `moveDestinations`, `shouldCollapseParameterList`, the two buffer constants. Pure — no React, no DOM. |
| `frontend/src/components/chart/dashboard/useCollapsedParameterList.ts` | **New.** Container / reserved / expanded-copy measurement, feeding `shouldCollapseParameterList`. |
| `frontend/src/components/chart/dashboard/CardInlineFilters.tsx` | **New.** A card's filter list: the invisible measurement copy, plus either expanded pills (view) / chips (edit) or the "Show filters" trigger. |
| `frontend/src/components/chart/dashboard/MoveParameterField.tsx` | **New.** "Top of page" + one `<optgroup>` per tab that has a hostable card. |
| `frontend/src/api/dashboards.ts` | Doc comment naming `options.inlineParameters`. No type change — `options` is already `Record<string, unknown>`. |
| `frontend/src/components/chart/dashboard/DashboardCardView.tsx` | A chart card's header becomes one row: title left, filters right, export gutter reserved. Threads one `inline` prop. |
| `frontend/src/components/chart/dashboard/HeadingCard.tsx` | Heading text and its filter list share one row (both modes). |
| `frontend/src/components/chart/dashboard/FilterSettingsPanel.tsx` | Hosts `MoveParameterField`, after Required and before Delete filter. |
| `frontend/src/components/chart/dashboard/EditSidebar.tsx` | Three pass-through props. |
| `frontend/src/components/chart/dashboard/tabs.ts` | `duplicateTab` strips `inlineParameters` from its clones. |
| `frontend/src/routes/explore/DashboardView.tsx` | Wiring only: split the parameter list at the two render sites, hand each card its own list, and route the four lifecycle cases through `inlineParameters.ts`. |

---

## Task 1: The model

**Files:**
- Create: `frontend/src/components/chart/dashboard/inlineParameters.ts`
- Modify: `frontend/src/api/dashboards.ts`
- Test: `frontend/src/components/chart/dashboard/__tests__/inlineParameters.test.ts`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/inlineParameters.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import type { DashboardParameter } from "@/api/dashboards";
import { moveCardToTab } from "../tabs";
import {
  INLINE_PARAM_BUFFER_PX,
  INLINE_PARAM_HEADING_BUFFER_PX,
  destinationLabel,
  dropParameters,
  findHostCardId,
  inlineParameterIds,
  inlineParametersFor,
  keepInlineParameters,
  mergeTopBarOrder,
  moveDestinations,
  moveParameterToCard,
  shouldCollapseParameterList,
  supportsInlineParameters,
  topBarParameters,
  withoutInlineParameters,
} from "../inlineParameters";

const p = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "p1", name: "Region", slug: "region", type: "category", operator: "is", ...over });

const params: DashboardParameter[] = [
  p({ id: "p1", name: "Region", slug: "region" }),
  p({ id: "p2", name: "City", slug: "city" }),
  p({ id: "p3", name: "Owner", slug: "owner" }),
];

interface TestCard {
  id: string;
  card_type: string;
  chart_id: string | null;
  grid_col: number;
  grid_row: number;
  size_x: number;
  size_y: number;
  options: Record<string, unknown>;
  tab_id: string | null;
  tabKey: string | null;
}
const card = (over: Partial<TestCard>): TestCard => ({
  id: "c1", card_type: "chart", chart_id: "ch1", grid_col: 0, grid_row: 0, size_x: 12, size_y: 6,
  options: {}, tab_id: null, tabKey: "t1", ...over,
});

const cards: TestCard[] = [
  card({ id: "head", card_type: "heading", chart_id: null, options: { text: "Sales" } }),
  card({ id: "bar", options: { title: "Revenue", inlineParameters: ["p1"] } }),
  card({ id: "far", tabKey: "t2", options: { inlineParameters: ["p3"] } }),
  card({ id: "note", card_type: "text", chart_id: null, options: { text: "hi" } }),
];

describe("supportsInlineParameters", () => {
  it("admits egret's question and heading cards and nothing else", () => {
    // Ported from Metabase utils.ts:118-122 (isQuestionDashCard || isHeadingDashCard) onto
    // egret's card_type vocabulary (DashboardCardView.tsx:97-111).
    expect(supportsInlineParameters("chart")).toBe(true);
    expect(supportsInlineParameters("heading")).toBe(true);
    expect(supportsInlineParameters("text")).toBe(false);
    expect(supportsInlineParameters("link")).toBe(false);
    expect(supportsInlineParameters("placeholder")).toBe(false);
  });
});

describe("inlineParameterIds", () => {
  it("reads the raw array and tolerates every other shape the bag could hold", () => {
    expect(inlineParameterIds({ options: { inlineParameters: ["p1", "p2"] } })).toEqual(["p1", "p2"]);
    expect(inlineParameterIds({ options: {} })).toEqual([]);
    expect(inlineParameterIds({ options: { inlineParameters: "p1" } })).toEqual([]);
    expect(inlineParameterIds({ options: { inlineParameters: ["p1", 7, null] } })).toEqual(["p1"]);
  });
});

describe("findHostCardId", () => {
  it("finds the host on ANY tab, not just the one being viewed", () => {
    // selectors.ts:223-226 — getCurrentDashcards is every dashcard on the dashboard. A per-tab
    // lookup makes a filter appear and vanish as you switch tabs, which reads as a bug.
    expect(findHostCardId("p1", cards)).toBe("bar");
    expect(findHostCardId("p3", cards)).toBe("far");
  });

  it("returns null for a parameter no card claims", () => {
    expect(findHostCardId("p2", cards)).toBeNull();
  });
});

describe("topBarParameters", () => {
  it("drops every hosted parameter, including one hosted on another tab, and keeps the order", () => {
    expect(topBarParameters(params, cards).map((x) => x.id)).toEqual(["p2"]);
    expect(topBarParameters(params, []).map((x) => x.id)).toEqual(["p1", "p2", "p3"]);
  });
});

describe("inlineParametersFor", () => {
  it("returns the card's own parameters in the CARD's order, not the dashboard's", () => {
    const both = card({ id: "bar", options: { inlineParameters: ["p3", "p1"] } });
    expect(inlineParametersFor(both, params).map((x) => x.id)).toEqual(["p3", "p1"]);
  });

  it("skips an id no parameter answers to, rather than rendering a hole", () => {
    const stale = card({ id: "bar", options: { inlineParameters: ["p1", "gone"] } });
    expect(inlineParametersFor(stale, params).map((x) => x.id)).toEqual(["p1"]);
  });
});

describe("moveParameterToCard", () => {
  it("moves a top-bar parameter onto a card", () => {
    const next = moveParameterToCard(cards, "p2", "head");
    expect(inlineParameterIds(next.find((c) => c.id === "head")!)).toEqual(["p2"]);
  });

  it("moves a hosted parameter back to the top of the page", () => {
    const next = moveParameterToCard(cards, "p1", null);
    expect(inlineParameterIds(next.find((c) => c.id === "bar")!)).toEqual([]);
    expect(findHostCardId("p1", next)).toBeNull();
  });

  it("moves between cards without leaving it on both", () => {
    const next = moveParameterToCard(cards, "p1", "head");
    expect(inlineParameterIds(next.find((c) => c.id === "bar")!)).toEqual([]);
    expect(inlineParameterIds(next.find((c) => c.id === "head")!)).toEqual(["p1"]);
  });

  it("refuses a destination that cannot host, leaving the array untouched by reference", () => {
    // A text card has no header row to draw a filter in. Silently accepting the move would
    // make the filter vanish from the top bar AND from the board.
    expect(moveParameterToCard(cards, "p2", "note")).toBe(cards);
  });
});

describe("mergeTopBarOrder", () => {
  it("applies a reordered top bar without losing the hosted parameters", () => {
    // THE BUG THIS EXISTS TO PREVENT: W6's drag-reorder writes the whole draft array back
    // (`onReorder={setDraftParameters}`). Handing it a list that had the inline parameters
    // filtered out would DELETE them.
    const merged = mergeTopBarOrder(params, [p({ id: "p2" })]);
    expect(merged.map((x) => x.id).sort()).toEqual(["p1", "p2", "p3"]);
    expect(merged[0].id).toBe("p2");
  });

  it("keeps the reordered entries, not the originals", () => {
    const renamed = mergeTopBarOrder(params, [p({ id: "p2", name: "Town" })]);
    expect(renamed.find((x) => x.id === "p2")!.name).toBe("Town");
  });
});

describe("dropParameters", () => {
  it("removes the named parameters and every reference to them", () => {
    // Metabase's removeParameterAndReferences (parameters.tsx:357-376).
    const linked = [p({ id: "p1" }), p({ id: "p2", filteringParameters: ["p1", "p3"] })];
    expect(dropParameters(linked, new Set(["p1"]))).toEqual([
      p({ id: "p2", filteringParameters: ["p3"] }),
    ]);
  });

  it("returns the same array reference when nothing is dropped", () => {
    expect(dropParameters(params, new Set())).toBe(params);
  });
});

describe("keepInlineParameters / withoutInlineParameters", () => {
  it("keep preserves only the placement across a Replace chart", () => {
    expect(keepInlineParameters({ title: "T", inlineParameters: ["p1"] })).toEqual({ inlineParameters: ["p1"] });
    expect(keepInlineParameters({ title: "T" })).toEqual({});
  });

  it("without strips the placement for a clone", () => {
    expect(withoutInlineParameters({ title: "T", inlineParameters: ["p1"] })).toEqual({ title: "T" });
    expect(withoutInlineParameters({ title: "T" })).toEqual({ title: "T" });
  });
});

describe("destinationLabel", () => {
  it("names a heading by its text and a chart by its title, then its chart name", () => {
    expect(destinationLabel({ card_type: "heading", options: { text: "Sales" } }, undefined)).toBe("Sales");
    expect(destinationLabel({ card_type: "heading", options: { text: "  " } }, undefined)).toBe("Heading");
    expect(destinationLabel({ card_type: "chart", options: { title: "Revenue" } }, "Orders")).toBe("Revenue");
    expect(destinationLabel({ card_type: "chart", options: {} }, "Orders")).toBe("Orders");
    expect(destinationLabel({ card_type: "chart", options: {} }, undefined)).toBe("Card");
  });
});

describe("moveDestinations", () => {
  const tabs = [{ key: "t1", name: "Overview", position: 0 }, { key: "t2", name: "Detail", position: 1 }];
  const chartNameOf = (id: string) => (id === "bar" ? "Orders" : "");

  it("groups hostable cards by tab, in tab order, skipping non-hostable cards", () => {
    expect(moveDestinations({ cards, tabs, chartNameOf })).toEqual([
      { cardId: "head", label: "Sales", tabName: "Overview" },
      { cardId: "bar", label: "Revenue", tabName: "Overview" },
      { cardId: "far", label: "Card", tabName: "Detail" },
    ]);
  });

  it("emits ungrouped destinations on a dashboard with no tabs", () => {
    // MoveParameterMenu.tsx:117-124 pushes the cards into the root group when no tab has any.
    expect(moveDestinations({ cards, tabs: [], chartNameOf }).map((d) => d.tabName))
      .toEqual([null, null, null]);
  });
});

describe("shouldCollapseParameterList", () => {
  it("is Metabase's arithmetic: reserved + list + buffer > container", () => {
    // use-responsive-parameter-list.ts:29-34. Fed as numbers because jsdom measures nothing.
    const m = { reservedWidth: 120, listWidth: 200, bufferPx: INLINE_PARAM_BUFFER_PX };
    expect(shouldCollapseParameterList({ ...m, containerWidth: 421 })).toBe(false);
    expect(shouldCollapseParameterList({ ...m, containerWidth: 420 })).toBe(true);
    expect(shouldCollapseParameterList({ ...m, containerWidth: 419 })).toBe(true);
  });

  it("stays expanded before anything has been measured", () => {
    // First paint and jsdom both report 0. Metabase returns false when a ref is missing
    // (use-responsive-parameter-list.ts:25-27); starting collapsed would flash a
    // "Show filters" button on every card on every load.
    expect(shouldCollapseParameterList({ containerWidth: 0, reservedWidth: 0, listWidth: 0, bufferPx: 100 })).toBe(false);
  });

  it("carries Metabase's two buffers", () => {
    expect(INLINE_PARAM_BUFFER_PX).toBe(100);          // DashCardVisualization.tsx:445
    expect(INLINE_PARAM_HEADING_BUFFER_PX).toBe(24);   // use-responsive-parameter-list.ts:13 default
  });
});

describe("a host card carries its filters to another tab", () => {
  it("moveCardToTab preserves options.inlineParameters", () => {
    // Metabase keeps inline_parameters through a tab move (parameters.tsx:279-291 even follows
    // the card to its tab). egret gets this free because moveCardToTab spreads {...c} — this
    // test is what makes a future refactor that rebuilds `options` fail loudly.
    const moved = moveCardToTab(cards as never, "bar", "t2");
    expect(inlineParameterIds(moved.find((c) => c.id === "bar")!)).toEqual(["p1"]);
    expect(moved.find((c) => c.id === "bar")!.tabKey).toBe("t2");
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/inlineParameters.test.ts`

Expected: FAIL — `Failed to resolve import "../inlineParameters"`.

- [ ] **Step 3: Write the implementation**

Create `frontend/src/components/chart/dashboard/inlineParameters.ts`:

```ts
import type { DashboardParameter } from "@/api/dashboards";

/**
 * INLINE PARAMETERS — a filter drawn on a card instead of on the top bar.
 *
 * THE FIELD IS `card.options.inlineParameters: string[]`, a list of PARAMETER ids hosted by that
 * card. It is Metabase's shape (`dashcard.inline_parameters`, dashboard/utils.ts:128), and it goes
 * on the card rather than on the parameter for a reason specific to egret: `serializeLayout`
 * (tabs.ts:34-48) sends cards WITHOUT ids and `replace_layout`
 * (crates/egret-bi/src/dashboard.rs:344-376) deletes and re-inserts every row, so a card id does
 * not survive a save. A `parameter.cardId` would dangle the first time the dashboard was saved.
 * Parameter ids do survive: `parameters` is one JSONB column written and read verbatim.
 *
 * NO BACKEND CHANGE. `CardInput.options` is `serde_json::Value` with no validation
 * (crates/egret-analytics-api/src/bi.rs:393-394), bound straight into the `options` jsonb column
 * and echoed back by `DashboardCardDto.options` (bi.rs:349) — the same argument `options.hideEmpty`
 * made in hide-empty.ts:6-9.
 *
 * AN INLINE PARAMETER STILL FILTERS EVERY CARD IT IS MAPPED TO. Only the WIDGET moves. Metabase
 * subtracts inline parameters from the header list alone (selectors.ts:447-454) and leaves the
 * parameter list that feeds values and mappings untouched; egret's `paramFiltersForCard` likewise
 * reads the full `parameters` array. `topBarParameters` must therefore reach exactly two call
 * sites — the view-mode filter bar and the edit-mode chip row.
 */

/** The minimum a card has to look like for this module. Structural, not `DraftCard`, so the tests
 *  never have to build a dashboard. */
export interface InlineHostCard {
  id: string;
  card_type: string;
  options: Record<string, unknown>;
  tabKey?: string | null;
}

/** Metabase's `supportsInlineParameters` (utils.ts:118-122): the question card and the heading
 *  card. egret's question card is `chart`. A text card interpolates {{slug}} in its own body, a
 *  link card is one anchor and a placeholder has no content — none of them has a header row a
 *  filter widget could sit in, which is the same line Metabase draws. */
export function supportsInlineParameters(cardType: string): boolean {
  return cardType === "chart" || cardType === "heading";
}

/** The ids a card claims. Defensive about the shape because `options` is an untyped JSON bag that
 *  a hand-edited dashboard row can put anything into. */
export function inlineParameterIds(card: { options: Record<string, unknown> }): string[] {
  const raw = card.options.inlineParameters;
  return Array.isArray(raw) ? raw.filter((x): x is string => typeof x === "string") : [];
}

/** Which card hosts `paramId`, across the WHOLE dashboard — not the active tab. Metabase's
 *  `findDashCardForInlineParameter` (utils.ts:140-149) composed with `getCurrentDashcards`
 *  (selectors.ts:223-226), which is every dashcard. A per-tab lookup would make a filter appear
 *  and disappear as the viewer switched tabs. */
export function findHostCardId(paramId: string, cards: readonly InlineHostCard[]): string | null {
  return cards.find((c) => inlineParameterIds(c).includes(paramId))?.id ?? null;
}

/** The parameters this card draws, in the CARD's order — the card's list is the authored order,
 *  the dashboard's array is the top bar's. An id with no matching parameter is skipped rather than
 *  rendered as a hole. */
export function inlineParametersFor(
  card: InlineHostCard,
  parameters: readonly DashboardParameter[],
): DashboardParameter[] {
  const byId = new Map(parameters.map((p) => [p.id, p]));
  return inlineParameterIds(card).map((id) => byId.get(id)).filter((p): p is DashboardParameter => !!p);
}

/** The parameters the top bar draws: everything no card has claimed. */
export function topBarParameters(
  parameters: readonly DashboardParameter[],
  cards: readonly InlineHostCard[],
): DashboardParameter[] {
  const hosted = new Set(cards.flatMap(inlineParameterIds));
  return parameters.filter((p) => !hosted.has(p.id));
}

/** Fold a reordered TOP-BAR list back into the full parameter array.
 *
 *  W6's drag-reorder hands its `onReorder` the whole list it was given and the route writes it
 *  straight into the draft. Once the top bar is a SUBSET, writing it back unmerged would delete
 *  every inline parameter. The hosted ones are appended, which is lossless: their position in this
 *  array never renders anything — a card draws them in its own `inlineParameters` order. */
export function mergeTopBarOrder(
  parameters: readonly DashboardParameter[],
  orderedTopBar: readonly DashboardParameter[],
): DashboardParameter[] {
  const seen = new Set(orderedTopBar.map((p) => p.id));
  return [...orderedTopBar, ...parameters.filter((p) => !seen.has(p.id))];
}

/** Move `paramId` to `destCardId`, or to the top bar when `destCardId` is null. Removes it from
 *  whichever card held it first, so it can never be on two cards at once.
 *
 *  A destination that cannot host is refused by returning the SAME array reference, so the caller
 *  writes no draft. Accepting it would take the filter off the top bar and give it nowhere to
 *  draw. */
export function moveParameterToCard<T extends InlineHostCard>(
  cards: readonly T[],
  paramId: string,
  destCardId: string | null,
): T[] {
  if (destCardId !== null) {
    const dest = cards.find((c) => c.id === destCardId);
    if (!dest || !supportsInlineParameters(dest.card_type)) return cards as T[];
  }
  return cards.map((c) => {
    const held = inlineParameterIds(c);
    const shouldHold = c.id === destCardId;
    const holds = held.includes(paramId);
    if (holds === shouldHold) return c;
    const next = shouldHold ? [...held, paramId] : held.filter((id) => id !== paramId);
    return { ...c, options: { ...c.options, inlineParameters: next } };
  });
}

/** Delete parameters and every reference to them — Metabase's `removeParameterAndReferences`
 *  (parameters.tsx:357-376), which also strips the ids out of other parameters'
 *  `filteringParameters` so a linked filter is not left pointing at nothing. Same reference back
 *  when nothing is dropped, so the caller can skip the draft write. */
export function dropParameters(
  parameters: readonly DashboardParameter[],
  ids: ReadonlySet<string>,
): DashboardParameter[] {
  if (ids.size === 0) return parameters as DashboardParameter[];
  return parameters
    .filter((p) => !ids.has(p.id))
    .map((p) =>
      p.filteringParameters?.some((id) => ids.has(id))
        ? { ...p, filteringParameters: p.filteringParameters.filter((id) => !ids.has(id)) }
        : p,
    );
}

/** The only key of a replaced card's `options` that survives. `replaceCard` resets `options` to
 *  `{}` and §9 of the design records that as NOT a divergence — but the filter list is a
 *  PLACEMENT, not a property of the chart, and Metabase's own replaceCard leaves
 *  `inline_parameters` alone (cards-typed.ts:262-270). Without this, replacing a chart teleports
 *  its filter back to the top bar. */
export function keepInlineParameters(options: Record<string, unknown>): Record<string, unknown> {
  const ids = inlineParameterIds({ options });
  return ids.length > 0 ? { inlineParameters: ids } : {};
}

/** The inverse, for a CLONE. Two cards claiming one parameter id is not a state this model can
 *  render — `findHostCardId` picks the first and the clone draws nothing — so a duplicated card
 *  starts with no filters and the original keeps them.
 *
 *  DELIBERATE DIVERGENCE, called out in the PR body: Metabase duplicates the parameters as well,
 *  minting fresh ids and remapping `parameter_mappings` (cards-typed.ts:416-441). Doing that here
 *  would fork the URL contract (a new slug appears in the query string) and belongs in its own
 *  change; stripping is one line and cannot be wrong. */
export function withoutInlineParameters(options: Record<string, unknown>): Record<string, unknown> {
  if (!("inlineParameters" in options)) return options;
  const { inlineParameters: _dropped, ...rest } = options;
  return rest;
}

/** Metabase's `getDashcardTitle` (MoveParameterMenu.tsx:136-146), in egret's terms: a heading is
 *  named by its own text, a chart by its title override, then by the chart's name. */
export function destinationLabel(
  card: { card_type: string; options: Record<string, unknown> },
  chartName: string | undefined,
): string {
  const str = (v: unknown) => (typeof v === "string" ? v.trim() : "");
  if (card.card_type === "heading") return str(card.options.text) || "Heading";
  return str(card.options.title) || (chartName ?? "").trim() || "Card";
}

export interface MoveDestination {
  cardId: string;
  label: string;
  /** The tab whose `<optgroup>` this belongs in, or null on a dashboard with no tabs. */
  tabName: string | null;
}

/** Every card a filter could be moved to, grouped the way `MoveParameterMenu` groups them
 *  (`:104-129`): one group per tab that actually has a hostable card, in tab order; and when no
 *  tab has one, a flat ungrouped list. */
export function moveDestinations(args: {
  cards: readonly InlineHostCard[];
  tabs: readonly { key: string; name: string; position: number }[];
  chartNameOf: (cardId: string) => string | undefined;
}): MoveDestination[] {
  const { cards, tabs, chartNameOf } = args;
  const hostable = cards.filter((c) => supportsInlineParameters(c.card_type));
  const label = (c: InlineHostCard) => destinationLabel(c, chartNameOf(c.id));
  const ordered = [...tabs].sort((a, b) => a.position - b.position);
  const grouped = ordered.flatMap((t) =>
    hostable
      .filter((c) => (c.tabKey ?? null) === t.key)
      .map((c) => ({ cardId: c.id, label: label(c), tabName: t.name })),
  );
  if (grouped.length > 0) return grouped;
  return hostable.map((c) => ({ cardId: c.id, label: label(c), tabName: null }));
}

/** Metabase's buffers. 100 for a question card (DashCardVisualization.tsx:445, "Bigger buffer
 *  space to account for varying chart padding"), and the 24 default for a heading
 *  (use-responsive-parameter-list.ts:13).
 *
 *  On an egret question card the 100 is already spoken for: the hover export menu is
 *  `absolute right-1` and 36px wide (`Button size="sm"` with one glyph — ui/button.tsx:66), so it
 *  contributes NO layout width, and the row reserves 40px for it plus 16px of left padding. */
export const INLINE_PARAM_BUFFER_PX = 100;
export const INLINE_PARAM_HEADING_BUFFER_PX = 24;

/** Metabase's collision test, verbatim (use-responsive-parameter-list.ts:29-34):
 *  `reserved + list + buffer > container`.
 *
 *  `reservedWidth` is what SHARES the row — the card title or the heading text — measured at its
 *  intrinsic width, not its laid-out width (see `useCollapsedParameterList`). `listWidth` is the
 *  ALWAYS-EXPANDED copy of the list, because measuring the visible one oscillates. */
export function shouldCollapseParameterList(m: {
  containerWidth: number;
  reservedWidth: number;
  listWidth: number;
  bufferPx: number;
}): boolean {
  // Nothing measured yet — first paint, or jsdom, which reports 0 for everything. Metabase bails
  // to false when a ref is missing; starting collapsed would flash a "Show filters" button on
  // every card on every load.
  if (m.containerWidth <= 0) return false;
  return m.reservedWidth + m.listWidth + m.bufferPx > m.containerWidth;
}
```

- [ ] **Step 4: Name the key in the API type**

In `frontend/src/api/dashboards.ts`, replace the `options` line inside `interface DashboardCard`:

```ts
  /** Untyped per-card bag, stored as `jsonb` and echoed back verbatim (bi.rs:349,393-394).
   *  Live keys: `title`, `text`, `url`, `label`, `vizOverride`, `paramMappings`, `autoHeight`,
   *  `hideEmpty`, `clickBehavior`, and `inlineParameters` — the ids of the dashboard parameters
   *  this card draws instead of the top bar (see `dashboard/inlineParameters.ts`). */
  options: Record<string, unknown>;
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/inlineParameters.test.ts`

Expected: PASS, 24 tests.

- [ ] **Step 6: Prove the tests survive mutation**

Six mutations, each run on its own, each restored before the next. Score by process **exit code**,
never by matching stdout text — the `PASS (n) FAIL (m)` line is the rtk wrapper's reformatting and
raw vitest never prints it.

1. `supportsInlineParameters` → `return true;`. Re-run. Expected: FAIL — the text/link/placeholder
   case, and the "refuses a destination that cannot host" case.
2. `findHostCardId` → search `cards.filter((c) => c.tabKey === "t1")`. Re-run. Expected: FAIL —
   `p3` on the other tab.
3. `moveParameterToCard` → drop the `shouldHold ? … : held.filter(…)` removal and always append.
   Re-run. Expected: FAIL — "moves between cards without leaving it on both".
4. `mergeTopBarOrder` → `return [...orderedTopBar];`. Re-run. Expected: FAIL — `p1` and `p3` gone.
5. `dropParameters` → skip the `filteringParameters` rewrite. Re-run. Expected: FAIL — the stale
   `p1` reference survives.
6. `shouldCollapseParameterList` → `>=` instead of `>`. Re-run. Expected: FAIL — the `421` boundary
   case.

If any mutation leaves the suite green, the assertion is not verifying what it claims and must be
rewritten before moving on.

- [ ] **Step 7: Commit**

```bash
git add frontend/src/components/chart/dashboard/inlineParameters.ts \
        frontend/src/components/chart/dashboard/__tests__/inlineParameters.test.ts \
        frontend/src/api/dashboards.ts
git commit -m "feat(dashboard): model a filter that lives on a card instead of the top bar"
```

---

## Task 2: The collapsing list

**Files:**
- Create: `frontend/src/components/chart/dashboard/useCollapsedParameterList.ts`,
  `frontend/src/components/chart/dashboard/CardInlineFilters.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/CardInlineFilters.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/CardInlineFilters.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { CardInlineFilters } from "../CardInlineFilters";
import type { DashboardParameter } from "@/api/dashboards";

const params: DashboardParameter[] = [
  { id: "p1", name: "Region", slug: "region", type: "category", operator: "is" },
  { id: "p2", name: "City", slug: "city", type: "category", operator: "is" },
];

const base = {
  parameters: params,
  values: {} as Record<string, unknown>,
  optionsBySlug: { region: ["EU", "US"], city: ["Hanoi"] },
  onChange: vi.fn(),
  collapsed: false,
  listRef: { current: null },
};

describe("CardInlineFilters expanded", () => {
  it("draws one pill per hosted filter and reports the RAW value the pill emits", () => {
    // The raw/display trap: asserting the label "EU" would pass against an implementation
    // storing ["EU"], which is not what a single-value category holds.
    const onChange = vi.fn();
    render(<CardInlineFilters {...base} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "Region" }));
    fireEvent.click(screen.getByLabelText("option EU"));
    expect(onChange).toHaveBeenCalledWith("region", ["EU"]);
  });

  it("offers no Show filters trigger while there is room", () => {
    render(<CardInlineFilters {...base} />);
    expect(screen.queryByRole("button", { name: "Show filters" })).toBeNull();
  });

  it("renders nothing at all for a card that hosts no filters", () => {
    const { container } = render(<CardInlineFilters {...base} parameters={[]} />);
    expect(container.firstChild).toBeNull();
  });
});

describe("CardInlineFilters collapsed", () => {
  it("replaces the list with a Show filters trigger", () => {
    render(<CardInlineFilters {...base} collapsed />);
    expect(screen.getByRole("button", { name: "Show filters" })).toBeTruthy();
    // The invisible measurement copy still renders — that is what un-collapses the list when the
    // card is widened again — so assert on the TRIGGER's absence of a sibling pill, by counting
    // the pills that are actually reachable.
    expect(screen.queryAllByRole("button", { name: "Region" })).toHaveLength(1);
  });

  it("counts only the filters that have a value, as Metabase's badge does", () => {
    // CollapsibleDashboardParameterList.tsx:29-31 — `parameters.filter((p) => p.value != null)`.
    render(<CardInlineFilters {...base} collapsed values={{ region: ["EU"], city: [] }} />);
    expect(screen.getByTestId("inline-filter-count").textContent).toBe("1");
  });

  it("shows no count when nothing is set", () => {
    render(<CardInlineFilters {...base} collapsed />);
    expect(screen.queryByTestId("inline-filter-count")).toBeNull();
  });

  it("opens the filters in a popover", async () => {
    render(<CardInlineFilters {...base} collapsed />);
    fireEvent.click(screen.getByRole("button", { name: "Show filters" }));
    expect(await screen.findByRole("button", { name: "City" })).toBeTruthy();
  });
});

describe("CardInlineFilters in edit mode", () => {
  it("selects the filter instead of picking a value, and marks the active one", () => {
    // This is the ONLY route from a card-hosted filter back to its settings panel, and so the
    // only route back to the top bar. If it stops selecting, the filter is unreachable.
    const onSelect = vi.fn();
    render(<CardInlineFilters {...base} onSelect={onSelect} activeId="p2" />);
    expect(screen.getByRole("button", { name: "City" }).getAttribute("data-active")).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: "Region" }));
    expect(onSelect).toHaveBeenCalledWith("p1");
    expect(base.onChange).not.toHaveBeenCalledWith("region", expect.anything());
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/CardInlineFilters.test.tsx`

Expected: FAIL — `Failed to resolve import "../CardInlineFilters"`.

- [ ] **Step 3: Write the measurement hook**

Create `frontend/src/components/chart/dashboard/useCollapsedParameterList.ts`:

```ts
import { useCallback, useEffect, useRef, useState } from "react";
import { shouldCollapseParameterList } from "./inlineParameters";

/**
 * Metabase's `useResponsiveParameterList` (dashboard/hooks/use-responsive-parameter-list.ts:23-35),
 * ported. Three refs, because three widths decide the answer:
 *
 *   containerRef — the row that holds everything;
 *   reservedRef  — whatever shares the row (the card title, or the heading text);
 *   listRef      — an ALWAYS-EXPANDED copy of the filter list, rendered invisibly.
 *
 * TWO THINGS THAT LOOK LIKE DETAILS AND ARE NOT.
 *
 * `reservedRef` is measured with `scrollWidth`, not `getBoundingClientRect().width`. The title sits
 * in a flex row with `min-w-0 truncate`, so its BOX shrinks to whatever space is left — the sum
 * could then never exceed the container and the list would never collapse. Metabase avoids this by
 * measuring the text on a canvas (`measureTextWidth`); `scrollWidth` on a truncating element is the
 * DOM's answer to the same question, "how wide would this text like to be".
 *
 * `listRef` points at a copy that is never collapsed. Measuring the VISIBLE list would read the
 * trigger's width once collapsed, decide there is room, expand, and oscillate. Metabase renders the
 * same invisible copy for the same reason (CollapsibleDashboardParameterList.tsx:87-101).
 *
 * jsdom reports 0 for every width and has no ResizeObserver, so this hook is inert there — exactly
 * as `DashboardGridLayout.tsx:404` is. The arithmetic is tested through
 * `shouldCollapseParameterList` with the numbers fed in directly, and the geometry is measured in
 * the browser.
 */
export function useCollapsedParameterList(bufferPx: number) {
  const containerRef = useRef<HTMLDivElement>(null);
  const reservedRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const [collapsed, setCollapsed] = useState(false);

  const measure = useCallback(() => {
    setCollapsed(
      shouldCollapseParameterList({
        containerWidth: containerRef.current?.getBoundingClientRect().width ?? 0,
        reservedWidth: reservedRef.current?.scrollWidth ?? 0,
        listWidth: listRef.current?.getBoundingClientRect().width ?? 0,
        bufferPx,
      }),
    );
  }, [bufferPx]);

  useEffect(() => {
    measure();
    const el = containerRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [measure]);

  return { collapsed, containerRef, reservedRef, listRef };
}
```

- [ ] **Step 4: Write the list**

Create `frontend/src/components/chart/dashboard/CardInlineFilters.tsx`:

```tsx
import { Filter } from "lucide-react";
import type { DashboardParameter } from "@/api/dashboards";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { FilterPill } from "./FilterPill";
import { isBlank } from "./params";

interface Props {
  parameters: DashboardParameter[];
  values: Record<string, unknown>;
  optionsBySlug: Record<string, string[]>;
  onChange: (slug: string, value: unknown) => void;
  /** Edit mode: a chip SELECTS its filter rather than picking a value. This is the only route from
   *  a card-hosted filter back to its settings panel, and so the only route back to the top bar. */
  onSelect?: (paramId: string) => void;
  activeId?: string | null;
  /** From `useCollapsedParameterList`. The component never measures anything itself. */
  collapsed: boolean;
  listRef: React.Ref<HTMLDivElement>;
}

/** Same predicate `FilterPill` uses for its own "is this set" styling (`FilterPill.tsx:33-36`);
 *  three lines, kept local rather than exported, because the two will not diverge. */
function hasValue(value: unknown): boolean {
  if (Array.isArray(value)) return value.filter((v) => !isBlank(v)).length > 0;
  return !isBlank(value);
}

/** One card's filter list. Metabase's `CollapsibleDashboardParameterList`
 *  (`:36,40-86,87-101`), including its `isSortable: false` — an inline list is deliberately NOT
 *  the reorderable one. The order is the card's `options.inlineParameters`; W6's drag-reorder
 *  stays on the top bar, where the order is a saved property of the dashboard. */
export function CardInlineFilters({
  parameters, values, optionsBySlug, onChange, onSelect, activeId, collapsed, listRef,
}: Props) {
  if (parameters.length === 0) return null;

  const list = (
    <div className="flex flex-wrap items-center gap-2">
      {parameters.map((p) =>
        onSelect ? (
          // Edit mode. A plain chip, not `EditFilterChips`: that one carries a drag handle, and
          // Metabase's inline list is explicitly `isSortable: false`.
          <button key={p.id} type="button" aria-label={p.name} data-active={p.id === activeId}
            className={`dash-nodrag inline-flex h-8 items-center rounded-sm border px-2 text-sm ${p.id === activeId ? "border-primary bg-secondary text-primary" : "bg-background hover:bg-accent/40"}`}
            onClick={(e) => { e.stopPropagation(); onSelect(p.id); }}>
            {p.name}{p.required ? " *" : ""}
          </button>
        ) : (
          <div key={p.id} className="dash-nodrag" onClick={(e) => e.stopPropagation()}>
            <FilterPill param={p} value={values[p.slug]} options={optionsBySlug[p.slug] ?? []}
              onChange={(v) => onChange(p.slug, v)} />
          </div>
        ),
      )}
    </div>
  );

  const setCount = parameters.filter((p) => hasValue(values[p.slug])).length;

  return (
    <>
      {/* The invisible always-expanded copy the collision test measures. `fixed` takes it out of
          flow, `invisible` keeps it laid out (`display:none` would measure 0), and the inner
          `absolute` lets it shrink-wrap to its natural width. Metabase renders the same thing for
          the same reason (CollapsibleDashboardParameterList.tsx:87-101). */}
      <div aria-hidden className="pointer-events-none invisible fixed left-0 top-0 w-full">
        <div ref={listRef} className="absolute">{list}</div>
      </div>
      {collapsed ? (
        <Popover>
          <PopoverTrigger asChild>
            <button type="button" aria-label="Show filters" title="Show filters"
              className="dash-nodrag inline-flex h-8 shrink-0 items-center gap-1 rounded-sm border bg-background px-2 text-sm hover:bg-accent/40"
              onClick={(e) => e.stopPropagation()}>
              <Filter className="h-3.5 w-3.5 text-muted-foreground" />
              {setCount > 0 && <span data-testid="inline-filter-count">{setCount}</span>}
            </button>
          </PopoverTrigger>
          <PopoverContent align="end" className="w-auto max-w-[20rem] p-2">{list}</PopoverContent>
        </Popover>
      ) : (
        <div className="shrink-0">{list}</div>
      )}
    </>
  );
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/CardInlineFilters.test.tsx`

Expected: PASS, 9 tests.

- [ ] **Step 6: Prove it survives mutation**

1. Change the collapsed branch to render `{list}` instead of the popover. Re-run. Expected: FAIL —
   "replaces the list with a Show filters trigger".
2. Restore. Change `setCount` to `parameters.length`. Re-run. Expected: FAIL — both count tests.
3. Restore. Change the edit chip's `onClick` to `onChange(p.slug, p.id)`. Re-run. Expected: FAIL —
   the edit-mode selection test.
4. Restore. Delete the invisible measurement copy. Re-run. Expected: PASS — **and that is
   correct**: jsdom cannot see it. Restore it and rely on Task 7 Step 5, which is the only place
   the oscillation is observable. Say this in the PR body rather than claiming the unit test covers
   it.

- [ ] **Step 7: Commit**

```bash
git add frontend/src/components/chart/dashboard/CardInlineFilters.tsx \
        frontend/src/components/chart/dashboard/useCollapsedParameterList.ts \
        frontend/src/components/chart/dashboard/__tests__/CardInlineFilters.test.tsx
git commit -m "feat(dashboard): a card's filter list, collapsing to Show filters when the row is too narrow"
```

---

## Task 3: The chart card and the heading card host the list

**Files:**
- Modify: `frontend/src/components/chart/dashboard/DashboardCardView.tsx`,
  `frontend/src/components/chart/dashboard/HeadingCard.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/DashboardCardView.inline.test.tsx`

Read "egret's 100px buffer is not spare room" above before starting. The header row of a chart card
already carries the title and, on hover, the export menu; adding a filter list is a real collision.

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/DashboardCardView.inline.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import type { DashboardParameter } from "@/api/dashboards";

vi.mock("@/components/chart/ChartRenderer", () => ({
  ChartRenderer: () => <svg data-testid="chart-svg" />,
}));
const useChartData = vi.fn();
vi.mock("@/api/useChartData", () => ({ useChartData: (...a: unknown[]) => useChartData(...a) }));
vi.mock("@/api/useSavedChartData", () => ({ useDashboardCardData: (...a: unknown[]) => useChartData(...a) }));

import { DashboardCardView } from "../DashboardCardView";

const chart = { id: "c", dataset_id: "d", name: "Orders", description: "", created_by: null, updated_at: "", viz_type: "bar", params: { spec: { metrics: [], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] } } } as never;
const region: DashboardParameter = { id: "p1", name: "Region", slug: "region", type: "category", operator: "is" };

const inline = {
  parameters: [region],
  values: {} as Record<string, unknown>,
  optionsBySlug: { region: ["EU"] },
  onChange: vi.fn(),
  activeId: null,
};

const chartCard = { id: "k", card_type: "chart", chart_id: "c", options: { inlineParameters: ["p1"] } } as never;
const headingCard = { id: "h", card_type: "heading", chart_id: null, options: { text: "Sales", inlineParameters: ["p1"] } } as never;

describe("a chart card hosting a filter", () => {
  it("draws the filter in the SAME row as the title", () => {
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    render(<DashboardCardView ws="eng" dashboardId="d1" card={chartCard} chart={chart} parameters={[region]} paramValues={{}} inline={inline} />);
    const pill = screen.getByRole("button", { name: "Region" });
    const title = screen.getByText("Orders");
    expect(title.parentElement).toBe(pill.closest("[data-testid='dashcard-header']"));
  });

  it("reserves the hover export gutter so the menu cannot land on the last pill", () => {
    // The export menu is `absolute right-1` and 36px wide (ui/button.tsx:66), so it takes NO
    // layout width. 4 + 36 = 40px = pr-10. Assert the class, because the collision is only
    // visible on hover and would survive every static review otherwise.
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    render(<DashboardCardView ws="eng" dashboardId="d1" card={chartCard} chart={chart} parameters={[region]} paramValues={{}} inline={inline} />);
    expect(screen.getByTestId("dashcard-header").className).toContain("pr-10");
  });

  it("does not reserve the gutter when the card cannot export", () => {
    useChartData.mockReturnValue({ columns: [], rows: [], isLoading: true, error: null });
    render(<DashboardCardView ws="eng" dashboardId="d1" card={chartCard} chart={chart} parameters={[region]} paramValues={{}} inline={inline} />);
    expect(screen.getByTestId("dashcard-header").className).toContain("pr-4");
  });

  it("draws a header row for the filters even when the title is hidden", () => {
    // options.title === "" means "hide the header" (DashboardCardView.tsx:55-57). The filters
    // still need somewhere to live, so the row renders with an empty title.
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    const hidden = { ...(chartCard as object), options: { title: "", inlineParameters: ["p1"] } } as never;
    render(<DashboardCardView ws="eng" dashboardId="d1" card={hidden} chart={chart} parameters={[region]} paramValues={{}} inline={inline} />);
    expect(screen.queryByText("Orders")).toBeNull();
    expect(screen.getByRole("button", { name: "Region" })).toBeTruthy();
  });

  it("keeps the filter reachable on a card whose chart has gone missing", () => {
    // Otherwise the filter is absent from the top bar (a card claims it) AND absent from the
    // board — unreachable until the dashboard is hand-edited.
    useChartData.mockReturnValue({ columns: [], rows: [], isLoading: false, error: null });
    render(<DashboardCardView ws="eng" dashboardId="d1" card={chartCard} chart={undefined} parameters={[region]} paramValues={{}} inline={inline} />);
    expect(screen.getByText("Chart not found")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Region" })).toBeTruthy();
  });

  it("renders exactly as before for a card that hosts nothing", () => {
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    const plain = { ...(chartCard as object), options: {} } as never;
    render(<DashboardCardView ws="eng" dashboardId="d1" card={plain} chart={chart} parameters={[region]} paramValues={{}} />);
    expect(screen.getByText("Orders")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Region" })).toBeNull();
  });
});

describe("a heading card hosting a filter", () => {
  it("draws the heading text and the filter in one row, in view mode", () => {
    render(<DashboardCardView ws="eng" dashboardId="d1" card={headingCard} chart={undefined} parameters={[region]} paramValues={{}} inline={inline} />);
    expect(screen.getByText("Sales")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Region" })).toBeTruthy();
  });

  it("keeps the heading editable with a filter beside it", () => {
    render(<DashboardCardView ws="eng" dashboardId="d1" card={headingCard} chart={undefined} parameters={[region]} paramValues={{}} editing onEditText={vi.fn()} inline={{ ...inline, onSelect: vi.fn() }} />);
    expect(screen.getByLabelText("heading text")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Region" })).toBeTruthy();
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/DashboardCardView.inline.test.tsx`

Expected: FAIL — `DashboardCardView` has no `inline` prop, so TypeScript errors and no filter or
`dashcard-header` node is ever rendered.

- [ ] **Step 3: Give the heading card a filter slot**

Replace the body of `frontend/src/components/chart/dashboard/HeadingCard.tsx` with:

```tsx
import { useEffect, useRef } from "react";

/** The filter list a heading card draws beside its text, plus the two refs the collision test
 *  measures. Supplied by `DashboardCardView`; absent on a heading that hosts nothing. */
export interface HeadingInlineFilters {
  node: React.ReactNode;
  containerRef: React.Ref<HTMLDivElement>;
  textRef: React.Ref<HTMLDivElement>;
}

/** A dashboard heading card: a single styled line, no card chrome. In edit mode it is an inline
 *  input bound to the raw text (so {{slug}} stays literal). A heading may host dashboard filters
 *  (Metabase's `supportsInlineParameters`), which sit at the trailing edge of the same row —
 *  Heading.tsx:167-215 does exactly this, in both modes. */
export function HeadingCard({ text, editing, autoFocus, onChange, inlineFilters }: { text: string; editing?: boolean; autoFocus?: boolean; onChange?: (text: string) => void; inlineFilters?: HeadingInlineFilters }) {
  const ref = useRef<HTMLInputElement>(null);
  // Deferred focus: when added via the Radix dropdown, Radix's close teardown blurs focus to
  // <body> after mount, so a plain autoFocus is lost. Re-focus on the next tick.
  useEffect(() => {
    if (!autoFocus) return;
    const id = window.setTimeout(() => ref.current?.focus(), 0);
    return () => window.clearTimeout(id);
  }, [autoFocus]);
  if (editing && onChange) {
    return (
      <div ref={inlineFilters?.containerRef} className="flex h-full items-center gap-2">
        <div ref={inlineFilters?.textRef} className="min-w-0 flex-1">
          <input
            ref={ref}
            aria-label="heading text"
            className="dash-nodrag h-full w-full bg-transparent px-1 text-xl font-bold text-foreground placeholder:font-normal placeholder:text-muted-foreground focus:outline-none"
            placeholder="Heading"
            autoFocus={autoFocus}
            value={text}
            onChange={(e) => onChange(e.target.value)}
          />
        </div>
        {inlineFilters?.node}
      </div>
    );
  }
  return (
    <div ref={inlineFilters?.containerRef} className="flex h-full items-center gap-2 px-1 text-xl font-bold text-foreground">
      <div ref={inlineFilters?.textRef} className="min-w-0 flex-1 truncate">
        {text || <span className="text-muted-foreground">Heading</span>}
      </div>
      {inlineFilters?.node}
    </div>
  );
}
```

- [ ] **Step 4: Rebuild the chart card's header row and thread the prop**

In `frontend/src/components/chart/dashboard/DashboardCardView.tsx`:

Add the imports beside the existing dashboard ones:

```tsx
import { CardInlineFilters } from "./CardInlineFilters";
import { INLINE_PARAM_BUFFER_PX, INLINE_PARAM_HEADING_BUFFER_PX } from "./inlineParameters";
import { useCollapsedParameterList } from "./useCollapsedParameterList";
```

Add the shared prop shape above `ChartCardTileProps`:

```tsx
/** Everything a card needs to draw the dashboard filters it hosts. One object rather than five
 *  props, so the route's call site stays a single expression. Undefined — or an empty
 *  `parameters` — on a card that hosts none, which is every card until an author moves one. */
export interface InlineFilterProps {
  parameters: DashboardParameter[];
  values: Record<string, unknown>;
  optionsBySlug: Record<string, string[]>;
  onChange: (slug: string, value: unknown) => void;
  onSelect?: (paramId: string) => void;
  activeId?: string | null;
}
```

Add `inline?: InlineFilterProps` to `ChartCardTileProps`, then replace `ChartCardTileBody`'s
opening block — everything from `function ChartCardTileBody(` down to and including the
`{showHeader && …}` line — with:

```tsx
function ChartCardTileBody({ card, chart, onValueClick, blocked, editing, autoHeight, onAutoHeight, rowHeight, inline, data }: ChartCardTileProps & { data: ChartData }) {
  // Ref the render-only container (NOT the whole tile), so the PNG export targets the
  // chart's <svg> and not the export menu's own Download icon. Also keeps PNG hidden for
  // table/pivot cards (their render container has no <svg>).
  const chartRef = useRef<HTMLDivElement>(null);
  const { collapsed, containerRef, reservedRef, listRef } = useCollapsedParameterList(INLINE_PARAM_BUFFER_PX);
  const titleOverride = asString(card.options.title);          // undefined → use chart name; "" → hide
  const headerText = titleOverride || chart?.name || "";
  const showHeader = titleOverride !== "" && headerText !== "";
  const canExport = !editing && !blocked && !data.isLoading && !data.error && (data.columns?.length ?? 0) > 0;
  const inlineParams = inline?.parameters ?? [];
  // ONE ROW, title left and filters right — Metabase hands the inline list to the visualization's
  // `actionButtons`, which renders it in the title row beside the card menu
  // (DashCardVisualization.tsx:474-487).
  //
  // `pr-10` is not decoration. The export menu above is `absolute right-1 top-1`, and a
  // `<Button size="sm">` with one glyph is 36px wide (ui/button.tsx:66, `has-[>svg]:px-2.5`
  // around a 16px icon) — `absolute`, so it contributes NO layout width. Without the 4 + 36 = 40px
  // reservation the hover menu draws on top of the last filter pill, and because it only appears
  // on hover it survives every static review.
  //
  // The row also renders when the title is HIDDEN (`options.title === ""`) but filters are
  // present: they have to live somewhere.
  const headerRow = (showHeader || inlineParams.length > 0) ? (
    <div ref={containerRef} data-testid="dashcard-header"
      className={`flex items-center gap-2 pl-4 pt-3 pb-2 ${canExport ? "pr-10" : "pr-4"}`}>
      <div ref={reservedRef} className="min-w-0 flex-1 truncate text-sm font-medium text-foreground">
        {showHeader ? headerText : null}
      </div>
      {inline && (
        <CardInlineFilters parameters={inlineParams} values={inline.values}
          optionsBySlug={inline.optionsBySlug} onChange={inline.onChange}
          onSelect={inline.onSelect} activeId={inline.activeId}
          collapsed={collapsed} listRef={listRef} />
      )}
    </div>
  ) : null;
  // The header comes FIRST even here: a card whose chart has gone missing still claims its
  // filters, so dropping the row would leave them absent from the top bar and absent from the
  // board at the same time.
  if (!chart) return (
    <div className="flex h-full flex-col">
      {headerRow}
      <div className="flex flex-1 items-center justify-center text-sm text-muted-foreground">Chart not found</div>
    </div>
  );
  // Computed once the chart is known to exist, so both the export menu (which needs the
  // rename that's actually in effect) and the render branch below share the same override.
  const eff = applyVizOverride(chart, card.options.vizOverride as Record<string, unknown> | undefined);
  return (
    <div className="group relative flex h-full flex-col">
      {canExport && (
        <div className="dash-nodrag absolute right-1 top-1 z-10 opacity-0 transition-opacity group-hover:opacity-100">
          <ChartExportMenu title={headerText || chart?.name} columns={data.columns} rows={data.rows}
            columnSettings={shellSettings(eff.params).columnSettings} containerRef={chartRef} />
        </div>
      )}
      {headerRow}
```

(the rest of the function — the `<div ref={chartRef} …>` block and its closing tags — is unchanged.)

Then, in `DashboardCardView` itself, add `inline` to the signature and its type, wire the heading
branch, and pass it through to the tile:

```tsx
export function DashboardCardView({ ws, dashboardId, card, chart, refreshNonce, parameters, paramValues, paramFilters, paramTemporal, onValueClick, blocked, editing, autoHeight, onAutoHeight, rowHeight, onEditText, autoFocusText, inline }: { ws: string; dashboardId: string; card: DashboardCard; chart: Chart | undefined; refreshNonce?: number; parameters?: DashboardParameter[]; paramValues?: Record<string, unknown>; paramFilters?: ChartFilter[]; paramTemporal?: Record<string, string>; onValueClick?: (ctx: DrillContext, x: number, y: number) => void; blocked?: boolean; editing?: boolean; autoHeight?: boolean; onAutoHeight?: (rows: number) => void; rowHeight?: number; onEditText?: (text: string) => void; autoFocusText?: boolean; inline?: InlineFilterProps }) {
```

Replace the `heading` branch (`:102-106`) with:

```tsx
  if (card.card_type === "heading") {
    const raw = asString(card.options.text) ?? "";
    const display = editing ? raw : interpolateParams(raw, parameters ?? [], paramValues ?? {});
    return wrap(<HeadingCardHost text={display} editing={editing} autoFocus={autoFocusText} onChange={onEditText} inline={inline} />);
  }
```

and add, immediately above `DashboardCardView`:

```tsx
/** A heading card plus its filter list. Split out so the measurement hook has a component of its
 *  own to live in — `DashboardCardView` returns before it could call one, and hooks may not be
 *  called conditionally. The heading buffer is 24px, not the chart card's 100: a heading is
 *  chrome-less (`px-1`) and has no export menu, which is exactly why Metabase leaves it on the
 *  `useResponsiveParameterList` default. */
function HeadingCardHost({ text, editing, autoFocus, onChange, inline }: { text: string; editing?: boolean; autoFocus?: boolean; onChange?: (t: string) => void; inline?: InlineFilterProps }) {
  const { collapsed, containerRef, reservedRef, listRef } = useCollapsedParameterList(INLINE_PARAM_HEADING_BUFFER_PX);
  const params = inline?.parameters ?? [];
  return (
    <HeadingCard text={text} editing={editing} autoFocus={autoFocus} onChange={onChange}
      inlineFilters={inline && params.length > 0 ? {
        containerRef, textRef: reservedRef,
        node: (
          <CardInlineFilters parameters={params} values={inline.values} optionsBySlug={inline.optionsBySlug}
            onChange={inline.onChange} onSelect={inline.onSelect} activeId={inline.activeId}
            collapsed={collapsed} listRef={listRef} />
        ),
      } : undefined} />
  );
}
```

Finally add `inline` to `tileProps` (`:108`):

```tsx
  const tileProps = { ws, dashboardId, card, chart, refreshNonce, paramFilters, paramTemporal, onValueClick, blocked, editing, autoHeight, onAutoHeight, rowHeight, inline };
```

- [ ] **Step 5: Run the new test and the three existing card-view suites**

Run each as its own command.

```bash
cd frontend && npx vitest run src/components/chart/dashboard/__tests__/DashboardCardView.inline.test.tsx
```
Expected: PASS, 8 tests.

```bash
cd frontend && npx vitest run src/components/chart/dashboard/__tests__/DashboardCardView.test.tsx
```
```bash
cd frontend && npx vitest run src/components/chart/dashboard/__tests__/DashboardCardView.export.test.tsx
```
```bash
cd frontend && npx vitest run src/components/chart/dashboard/__tests__/DashboardCardView.vizoverride.test.tsx
```

Expected: PASS, all three unchanged. The export menu's accessible name, its position in the DOM and
its `containerRef` are untouched, and the heading's `aria-label="heading text"` and its rendered
text node are both still present — a `getByText("Sales")` still resolves through the new wrapper
`<div>`. If one of these fails, the cause is the markup restructure, not a behaviour change: fix the
markup rather than the assertion.

- [ ] **Step 6: Prove the new tests survive mutation**

1. Change the header row's class to always `pr-4`. Re-run the inline suite. Expected: FAIL — the
   gutter test.
2. Restore. Change `headerRow`'s condition to `showHeader ? … : null`. Re-run. Expected: FAIL — the
   hidden-title test.
3. Restore. Move `if (!chart) return …` back above `headerRow`. Re-run. Expected: FAIL — the
   missing-chart test.

- [ ] **Step 7: Run the gates and commit**

Each as its own command. Never chain them through a pipe, and never pipe into `rtk` — a swallowed
pipe breaks the `&&` chain behind it and the later gates silently never run.

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
git add frontend/src/components/chart/dashboard/DashboardCardView.tsx \
        frontend/src/components/chart/dashboard/HeadingCard.tsx \
        frontend/src/components/chart/dashboard/__tests__/DashboardCardView.inline.test.tsx
git commit -m "feat(dashboard): draw a card's hosted filters in its header row, clearing the export gutter"
```

---

## Task 4: The move control

**Files:**
- Create: `frontend/src/components/chart/dashboard/MoveParameterField.tsx`
- Modify: `frontend/src/components/chart/dashboard/FilterSettingsPanel.tsx`,
  `frontend/src/components/chart/dashboard/EditSidebar.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/MoveParameterField.test.tsx`, plus
  additions to `FilterSettingsPanel.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/MoveParameterField.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { MoveParameterField } from "../MoveParameterField";
import type { MoveDestination } from "../inlineParameters";

const tabbed: MoveDestination[] = [
  { cardId: "head", label: "Sales", tabName: "Overview" },
  { cardId: "bar", label: "Revenue", tabName: "Overview" },
  { cardId: "far", label: "Detail chart", tabName: "Detail" },
];
const flat: MoveDestination[] = [
  { cardId: "head", label: "Sales", tabName: null },
  { cardId: "bar", label: "Revenue", tabName: null },
];

describe("MoveParameterField", () => {
  it("offers Top of page plus one group per tab, in tab order", () => {
    // MoveParameterMenu.tsx:104-129 — the root group holds only "Top of page" while any tab has a
    // hostable card.
    render(<MoveParameterField value={null} destinations={tabbed} onChange={vi.fn()} />);
    const groups = [...document.querySelectorAll("optgroup")].map((g) => g.label);
    expect(groups).toEqual(["Overview", "Detail"]);
    expect(screen.getByRole("option", { name: "Top of page" })).toBeTruthy();
    expect(screen.getByRole("option", { name: "Detail chart" })).toBeTruthy();
  });

  it("drops the grouping on a dashboard with no tabs", () => {
    render(<MoveParameterField value={null} destinations={flat} onChange={vi.fn()} />);
    expect(document.querySelectorAll("optgroup")).toHaveLength(0);
    expect(screen.getByRole("option", { name: "Revenue" })).toBeTruthy();
  });

  it("shows Top of page for a filter no card hosts, and the host otherwise", () => {
    const { rerender } = render(<MoveParameterField value={null} destinations={tabbed} onChange={vi.fn()} />);
    expect((screen.getByLabelText("filter location") as HTMLSelectElement).value).toBe("top-of-page");
    rerender(<MoveParameterField value="bar" destinations={tabbed} onChange={vi.fn()} />);
    expect((screen.getByLabelText("filter location") as HTMLSelectElement).value).toBe("bar");
  });

  it("emits the card id, and null for Top of page", () => {
    // Assert the RAW value the handler receives: a test that read the visible label would pass
    // against an implementation emitting "top-of-page" as a card id, which would then be looked
    // up as a card and silently found missing.
    const onChange = vi.fn();
    render(<MoveParameterField value="bar" destinations={tabbed} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText("filter location"), { target: { value: "head" } });
    expect(onChange).toHaveBeenCalledWith("head");
    fireEvent.change(screen.getByLabelText("filter location"), { target: { value: "top-of-page" } });
    expect(onChange).toHaveBeenCalledWith(null);
  });

  it("renders nothing when there is nowhere to move the filter", () => {
    const { container } = render(<MoveParameterField value={null} destinations={[]} onChange={vi.fn()} />);
    expect(container.firstChild).toBeNull();
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/MoveParameterField.test.tsx`

Expected: FAIL — `Failed to resolve import "../MoveParameterField"`.

- [ ] **Step 3: Write the implementation**

Create `frontend/src/components/chart/dashboard/MoveParameterField.tsx`:

```tsx
import type { MoveDestination } from "./inlineParameters";

/** The sentinel the `<select>` carries for "not on any card". Metabase's `TOP_NAV_VALUE`
 *  (MoveParameterMenu.tsx:30) with egret's wording; it is never a card id, and the handler
 *  converts it to `null` before anything downstream sees it. */
const TOP_OF_PAGE = "top-of-page";

interface Props {
  /** The host card's id, or null when the filter is on the top bar. */
  value: string | null;
  destinations: MoveDestination[];
  onChange: (cardId: string | null) => void;
}

/** Where a filter appears: the top of the page, or a heading/question card, grouped by tab —
 *  Metabase's `MoveParameterMenu` (`:67-134`). A native grouped `<select>` rather than a popover,
 *  matching every other control in this panel (`FilterSettingsPanel.tsx`) and the `<optgroup>`
 *  pattern `FilterPill`'s temporal-unit picker already uses.
 *
 *  Metabase's select carries no visible caption because its options are icon-prefixed and
 *  self-describing. egret labels every other control in this panel, so it gets one; that is the
 *  only intentional difference. */
export function MoveParameterField({ value, destinations, onChange }: Props) {
  if (destinations.length === 0) return null;
  const grouped = destinations.some((d) => d.tabName !== null);
  const tabNames = [...new Set(destinations.map((d) => d.tabName))].filter((n): n is string => n !== null);
  return (
    <label className="flex flex-col gap-1 text-xs text-muted-foreground">Where this filter appears
      <select aria-label="filter location" className="h-8 w-full rounded border bg-background px-2 text-sm"
        value={value ?? TOP_OF_PAGE}
        onChange={(e) => onChange(e.target.value === TOP_OF_PAGE ? null : e.target.value)}>
        <option value={TOP_OF_PAGE}>Top of page</option>
        {grouped
          ? tabNames.map((name) => (
              <optgroup key={name} label={name}>
                {destinations.filter((d) => d.tabName === name).map((d) => (
                  <option key={d.cardId} value={d.cardId}>{d.label}</option>
                ))}
              </optgroup>
            ))
          : destinations.map((d) => <option key={d.cardId} value={d.cardId}>{d.label}</option>)}
      </select>
    </label>
  );
}
```

- [ ] **Step 4: Put it in the settings panel**

In `frontend/src/components/chart/dashboard/FilterSettingsPanel.tsx`, add the import:

```tsx
import { MoveParameterField } from "./MoveParameterField";
import type { MoveDestination } from "./inlineParameters";
```

extend `Props` (this is on top of W6's `defaultOptions`):

```ts
  /** Every heading/question card the filter could be moved to, grouped by tab. Empty on a
   *  dashboard with no hostable card, which hides the control entirely. */
  moveDestinations?: MoveDestination[];
  /** The card that currently hosts this filter, or null for the top bar. */
  hostCardId?: string | null;
  onMove?: (cardId: string | null) => void;
```

and insert, **after** the Required checkbox and **before** the "Delete filter" button — the position
Metabase gives it (`ParameterSettings.tsx:293`, after the default/required block and before the
destructive actions):

```tsx
      {onMove && (
        <MoveParameterField value={hostCardId ?? null} destinations={moveDestinations ?? []} onChange={onMove} />
      )}
```

with `moveDestinations = [], hostCardId = null` defaulted in the destructuring.

- [ ] **Step 5: Pass it through the sidebar**

In `frontend/src/components/chart/dashboard/EditSidebar.tsx`, add to `Props`:

```tsx
  moveDestinations?: MoveDestination[];
  hostCardId?: string | null;
  onMoveParam?: (cardId: string | null) => void;
```

(with `import type { MoveDestination } from "./inlineParameters";`) and extend the
`FilterSettingsPanel` call:

```tsx
        <FilterSettingsPanel param={props.paramForPanel} allParameters={props.parameters}
          cardColumns={props.cardColumns ?? []} defaultOptions={props.defaultOptions ?? []}
          moveDestinations={props.moveDestinations ?? []} hostCardId={props.hostCardId ?? null}
          onMove={props.onMoveParam}
          onChange={props.onChangeParam} onDelete={props.onDeleteParam} />
```

- [ ] **Step 6: Write the panel tests**

Add to `frontend/src/components/chart/dashboard/__tests__/FilterSettingsPanel.test.tsx`:

```tsx
  it("offers the move control between Required and Delete filter", () => {
    const onMove = vi.fn();
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={vi.fn()} onDelete={vi.fn()}
      moveDestinations={[{ cardId: "bar", label: "Revenue", tabName: null }]} hostCardId={null} onMove={onMove} />);
    const order = [...document.querySelectorAll("[aria-label]")].map((e) => e.getAttribute("aria-label"));
    expect(order.indexOf("filter location")).toBeGreaterThan(order.indexOf("parameter required"));
    expect(order.indexOf("filter location")).toBeLessThan(order.indexOf("delete filter"));
    fireEvent.change(screen.getByLabelText("filter location"), { target: { value: "bar" } });
    expect(onMove).toHaveBeenCalledWith("bar");
  });

  it("hides the move control on a dashboard with no card that could host a filter", () => {
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={vi.fn()} onDelete={vi.fn()}
      moveDestinations={[]} hostCardId={null} onMove={vi.fn()} />);
    expect(screen.queryByLabelText("filter location")).toBeNull();
  });
```

- [ ] **Step 7: Run the tests and prove they survive mutation**

```bash
cd frontend && npx vitest run src/components/chart/dashboard/__tests__/MoveParameterField.test.tsx
```
Expected: PASS, 5 tests.

```bash
cd frontend && npx vitest run src/components/chart/dashboard/__tests__/FilterSettingsPanel.test.tsx
```
Expected: PASS, including W6's existing cases unchanged.

Mutation: change the handler to `onChange(e.target.value)`. Re-run `MoveParameterField.test.tsx`.
Expected: FAIL — "emits the card id, and null for Top of page". Restore. Change
`if (destinations.length === 0) return null;` to always render. Re-run. Expected: FAIL — the
nothing-to-move test. Restore.

- [ ] **Step 8: Run the gates and commit**

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
git add frontend/src/components/chart/dashboard/MoveParameterField.tsx \
        frontend/src/components/chart/dashboard/__tests__/MoveParameterField.test.tsx \
        frontend/src/components/chart/dashboard/FilterSettingsPanel.tsx \
        frontend/src/components/chart/dashboard/__tests__/FilterSettingsPanel.test.tsx \
        frontend/src/components/chart/dashboard/EditSidebar.tsx
git commit -m "feat(dashboard): move a filter to a heading or question card from its settings panel"
```

---

## Task 5: Wire the route

**Files:**
- Modify: `frontend/src/routes/explore/DashboardView.tsx`

No new test file: every decision this task reaches for is already tested in Task 1, and the route
itself is exercised by Task 7's live check. That is the same split W6 used, and it is why
`inlineParameters.ts` exists.

- [ ] **Step 1: Import the model**

In `frontend/src/routes/explore/DashboardView.tsx`, add beside the other dashboard imports:

```tsx
import { findHostCardId, inlineParametersFor, mergeTopBarOrder, moveDestinations, moveParameterToCard, topBarParameters } from "@/components/chart/dashboard/inlineParameters";
```

- [ ] **Step 2: Split the parameter list at the two render sites, and only those two**

Add one line immediately after `const activeParamId = …` (`:409`):

```tsx
  // Only the TWO widget lists get the split. `cardQueryArgs`, `applyParamsToChart` and
  // `interpolateParams` keep reading the full `parameters` array, because moving a filter onto a
  // card changes where its widget is drawn and nothing about what it filters
  // (Metabase subtracts inline parameters from the header list alone — selectors.ts:447-454).
  const topBar = topBarParameters(parameters, cards);
```

Then replace the filter-bar block (`:562-564` on the W6 branch — the version that already carries
W6's reorder and draft props) with:

```tsx
      {editing
        ? <EditFilterChips parameters={topBar} activeId={editPanel?.kind === "filter" ? editPanel.paramId : null}
            onSelect={(id) => openPanel({ kind: "filter", paramId: id })}
            onReorder={(next) => setDraftParameters(mergeTopBarOrder(parameters, next))} />
        : <DashboardFilterBar parameters={topBar} values={filters.editorValues}
            onChange={filters.setValue} optionsBySlug={optionsBySlug}
            autoApply={filters.autoApply} onAutoApplyChange={filters.setAutoApply}
            changedCount={filters.changedCount} onApply={filters.apply} onCancel={filters.cancel} />}
```

**`mergeTopBarOrder` is the load-bearing change here.** W6's `onReorder={setDraftParameters}` wrote
the list it was handed straight into the draft; now that the list is a subset, writing it back
unmerged would delete every filter an author had moved onto a card.

- [ ] **Step 3: Hand each card its own list**

At the `DashboardCardView` call site (`:639-646`), add one prop after `autoFocusText`:

```tsx
                    inline={{
                      parameters: inlineParametersFor(c as DraftCard, parameters),
                      values: filters.editorValues, optionsBySlug, onChange: filters.setValue,
                      onSelect: editing ? (paramId: string) => openPanel({ kind: "filter", paramId }) : undefined,
                      activeId: activeParamId,
                    }}
```

`CardInlineFilters` returns `null` for an empty list, so a card that hosts nothing renders exactly
as it does today. The values are W6's **editor** values, so a card-hosted filter behaves identically
to a top-bar one under the draft/Apply mode.

- [ ] **Step 4: Feed the move control**

At the `EditSidebar` call site (`:667-685`), add three props after `paramForPanel`:

```tsx
            moveDestinations={moveDestinations({
              cards: cards as DraftCard[], tabs,
              chartNameOf: (cardId) => chartById.get((cards as DraftCard[]).find((x) => x.id === cardId)?.chart_id ?? "")?.name,
            })}
            hostCardId={editPanel?.kind === "filter" ? findHostCardId(editPanel.paramId, cards as DraftCard[]) : null}
            onMoveParam={(cardId) => { if (editPanel?.kind === "filter") setDraftCards(moveParameterToCard(cards as DraftCard[], editPanel.paramId, cardId)); }}
```

- [ ] **Step 5: Run the gates**

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

- [ ] **Step 6: Commit**

```bash
git add frontend/src/routes/explore/DashboardView.tsx
git commit -m "feat(dashboard): route the top bar, the cards and the move control through the inline-parameter model"
```

---

## Task 6: The four lifecycle cases

**Files:**
- Modify: `frontend/src/routes/explore/DashboardView.tsx`,
  `frontend/src/components/chart/dashboard/tabs.ts`
- Test: additions to `frontend/src/components/chart/dashboard/__tests__/tabs.test.ts`

Read the lifecycle table in "Background the implementer needs" before starting. All four are draft
operations; nothing reaches the server until Save.

- [ ] **Step 1: Write the failing test for the clone case**

Add to `frontend/src/components/chart/dashboard/__tests__/tabs.test.ts`:

```ts
  it("duplicateTab does not let a clone claim the original's filters", () => {
    // Two cards holding the same parameter id is not a state the model can render:
    // findHostCardId picks the first, the clone draws nothing, and the dashboard carries an
    // invisible inconsistency. Metabase duplicates the PARAMETERS instead (cards-typed.ts:416-441);
    // that forks the URL contract with a fresh slug and is deliberately out of scope here.
    const tabs: DraftTab[] = [{ key: "t1", name: "Tab 1", position: 0 }];
    const cards: DraftCard[] = [
      { id: "a", chart_id: "c", card_type: "chart", grid_col: 0, grid_row: 0, size_x: 12, size_y: 6,
        options: { title: "Revenue", inlineParameters: ["p1"] }, tab_id: null, tabKey: "t1" },
    ];
    const r = duplicateTab(tabs, cards, "t1");
    const clone = r.draftCards.find((c) => c.id !== "a")!;
    expect(clone.options).toEqual({ title: "Revenue" });
    expect(r.draftCards.find((c) => c.id === "a")!.options).toEqual({ title: "Revenue", inlineParameters: ["p1"] });
  });
```

- [ ] **Step 2: Run it and see it fail**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/tabs.test.ts`

Expected: FAIL — the clone's `options` still carries `inlineParameters: ["p1"]`.

- [ ] **Step 3: Strip the placement from a tab's clones**

In `frontend/src/components/chart/dashboard/tabs.ts`, add the import:

```ts
import { withoutInlineParameters } from "./inlineParameters";
```

and change the clone map inside `duplicateTab` (`:130-132`):

```ts
  const clones = draftCards
    .filter((c) => c.tabKey === key)
    // A clone must not claim the original's inline filters — see inlineParameters.ts.
    .map((c) => ({ ...c, id: `new-dup-${seq++}`, tabKey: newKey, options: withoutInlineParameters(c.options) }));
```

Re-run: PASS.

- [ ] **Step 4: Prove it survives mutation**

Drop `options: withoutInlineParameters(c.options)` from the clone. Re-run. Expected: FAIL. Restore.

- [ ] **Step 5: Wire the other three cases in the route**

In `frontend/src/routes/explore/DashboardView.tsx`, extend the import added in Task 5:

```tsx
import { dropParameters, findHostCardId, inlineParameterIds, inlineParametersFor, keepInlineParameters, mergeTopBarOrder, moveDestinations, moveParameterToCard, topBarParameters, withoutInlineParameters } from "@/components/chart/dashboard/inlineParameters";
```

Replace `removeCard` (`:261-263`):

```tsx
  function removeCard(cardId: string) {
    // Metabase deletes a card's inline filters WITH the card (cards-typed.ts:490-495 →
    // removeParameterAndReferences) rather than returning them to the top bar, and so do we: a
    // filter that materialised at the top of the page out of a DELETE is the surprise that
    // behaviour exists to avoid. Nothing is lost until Save — Cancel restores the whole draft —
    // and W2's "remove card" undo toast covers it properly when it lands.
    const doomed = new Set(inlineParameterIds((cards as DraftCard[]).find((c) => c.id === cardId) ?? { options: {} }));
    setDraftCards((cards as DraftCard[]).filter((c) => c.id !== cardId));
    if (doomed.size > 0) setDraftParameters(dropParameters(parameters, doomed));
  }
```

Replace the `options` reset in `duplicateCard` (`:271`):

```tsx
      return [...base, { ...src, id: `new-dup-${cardSeqRef.current++}`, grid_row: maxY, options: withoutInlineParameters({ ...src.options }) }];
```

Replace `replaceCard` (`:274-277`):

```tsx
  function replaceCard(cardId: string, chart: Chart) {
    // The `options` reset is deliberate and §9 of the design records it as NOT a divergence — but
    // the filter list is a PLACEMENT, not a property of the chart, and Metabase's own replaceCard
    // leaves `inline_parameters` alone (cards-typed.ts:262-270). Without this, replacing a chart
    // silently teleports its filter back to the top bar.
    setDraftCards((prev) => (prev ?? cardsAsDraft()).map((c) => (c.id === cardId ? { ...c, chart_id: chart.id, options: keepInlineParameters(c.options) } : c)));
    setReplacingId(null);
  }
```

`moveCardToTab` needs **no change** — it spreads `{...c}`, so `options` rides along and the filter
travels with its card. Task 1's test pins that.

- [ ] **Step 6: Run the gates and commit**

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
        frontend/src/components/chart/dashboard/tabs.ts \
        frontend/src/components/chart/dashboard/__tests__/tabs.test.ts
git commit -m "fix(dashboard): decide what happens to a card's filters when it is deleted, replaced or cloned"
```

---

## Task 7: Live check — measure, do not eyeball

**Files:** none. This task produces the numbers for the PR body.

Every defect that mattered in the chart programme was found by driving the running app. Two things
in this workstream **cannot** be verified in jsdom at all — the collision arithmetic against real
geometry, and the oscillation the invisible measurement copy exists to prevent — which is why they
are here rather than given a green test that proves nothing.

- [ ] **Step 1: Build and deploy**

```bash
just deploy-kind
```

Expected: `deployment "egret" successfully rolled out`, SPA at `http://localhost:30080`.

Sign in as `admin@egret.local` / `Admin12345!` and open the harness dashboard
`http://localhost:30080/explore/demo/dashboards/8a196343-ad93-4968-8040-67598d29c3cb`.
**Do not hand-edit any harness card** — see `docs/en/guides/chart-parity-harness.md`. Add the filter
this check needs through the UI, and leave it in place; it is a filter, not a card.

- [ ] **Step 2: A filter moves onto a card and stops being a top-bar filter**

In edit mode, add a category filter, map it to one card, then open its settings panel, set
**Where this filter appears** to a question card, and Save. Reload, then evaluate:

```js
(() => {
  const bar = document.querySelector('[data-testid="dashcard-header"]');
  const topPills = [...document.querySelectorAll('.mb-3 > .flex.flex-wrap > *')].length;
  return {
    topBarPills: topPills,
    onCard: !!bar && !!bar.querySelector('button'),
    onCardLabels: bar ? [...bar.querySelectorAll('button')].map(b => b.getAttribute('aria-label')) : [],
  };
})()
```

Required: `onCardLabels` contains the filter's name, and `topBarPills` is one **fewer** than before
the move. Record both numbers.

- [ ] **Step 3: The filter still filters — this is the assertion the feature turns on**

With the moved filter set to a value, count the rows each mapped card returns, then move the filter
back to **Top of page**, Save, and count again with the same value in the URL.

```js
(() => {
  const cards = [...document.querySelectorAll('[data-testid="dashcard-body"]')];
  return { url: location.search, rows: cards.map(c => c.querySelectorAll('table tbody tr').length) };
})()
```

Required: **identical `rows` arrays** on both sides of the move, and the same `url`. A filter that
filters less after being moved onto a card means the split list reached `cardQueryArgs` — recheck
Task 5 Step 2.

- [ ] **Step 4: The collision test, against real geometry**

Resize the host card narrower one grid column at a time and, at each width, evaluate:

```js
(() => {
  const row = document.querySelector('[data-testid="dashcard-header"]');
  const title = row.firstElementChild;
  const copy = row.querySelector('.invisible .absolute');
  const trigger = row.querySelector('[aria-label="Show filters"]');
  return {
    container: Math.round(row.getBoundingClientRect().width),
    reserved: title.scrollWidth,
    list: Math.round(copy.getBoundingClientRect().width),
    sum: title.scrollWidth + Math.round(copy.getBoundingClientRect().width) + 100,
    collapsed: !!trigger,
  };
})()
```

Required: `collapsed` is `false` while `sum <= container` and `true` from the first width where
`sum > container`, with **no width where the two disagree**. Record the last expanded reading and
the first collapsed one — those two objects are the evidence that the ported arithmetic is the
arithmetic actually running.

- [ ] **Step 5: The list does not oscillate**

Park the card at the width found in Step 4, one column narrower, and watch for 5 seconds:

```js
window.__flips = 0;
new MutationObserver(() => { window.__flips++; })
  .observe(document.querySelector('[data-testid="dashcard-header"]'), { childList: true, subtree: true });
setTimeout(() => console.log('flips', window.__flips), 5000);
```

Required: `flips 0`. Anything else means the measurement is reading the visible list rather than the
invisible expanded copy, and the list will flicker between "Show filters" and its pills forever.
This is the one check no unit test can make.

- [ ] **Step 6: The mobile breakpoint**

Resize the viewport to 390×844 and evaluate Step 4's expression again on both a question card and a
heading card hosting a filter.

Required: `collapsed: true` on both, `document.documentElement.scrollWidth === innerWidth` (the
inline list must not be what reintroduces horizontal scroll), and the "Show filters" trigger's
`getBoundingClientRect().height` is `32` and fits inside the card. Record all four numbers. If W1
has not merged yet, note that this is measured against the pre-W1 grid and that the card is
therefore still narrow for a different reason — the conclusion (collapsed, no overflow) is the same
either way.

- [ ] **Step 7: The hover export menu does not land on a pill**

On a question card hosting a filter, with the pointer over the card:

```js
(() => {
  const row = document.querySelector('[data-testid="dashcard-header"]');
  const pills = [...row.querySelectorAll('button')].filter(b => b.getAttribute('aria-label') !== 'Export chart');
  const last = pills[pills.length - 1].getBoundingClientRect();
  const menu = document.querySelector('[aria-label="Export chart"]').getBoundingClientRect();
  return { pillRight: Math.round(last.right), menuLeft: Math.round(menu.left), gap: Math.round(menu.left - last.right) };
})()
```

Required: `gap >= 0`. Record it. A negative gap means the `pr-10` reservation is missing or the
button's size changed.

- [ ] **Step 8: The three lifecycle cases, on the running app**

For each, record the filter's location before and after, read off the saved dashboard:

```js
window.__where = async (url) => {
  const d = await fetch(url, { headers: { accept: 'application/json' } }).then(r => r.json());
  const hosts = d.data.cards.flatMap(c => (c.options.inlineParameters ?? []).map(id => [id, c.card_type]));
  return { parameters: d.data.parameters.map(p => p.name), hosts };
};
```

1. **Delete the host card**, Save, re-read. Required: the parameter is **gone** from `parameters`
   and `hosts` is empty.
2. **Replace the host card's chart**, Save, re-read. Required: the parameter is still in
   `parameters` and `hosts` still names that card.
3. **Move the host card to another tab**, Save, re-read, then switch to the *other* tab in the UI.
   Required: `hosts` unchanged, and the top bar shows the filter on **neither** tab.

Record the three before/after pairs.

- [ ] **Step 9: Compare against Metabase**

Open `localhost:3033/dashboard/2`, move a filter onto a card there, and compare side by side at
1440px:

- the destination menu's grouping and its "Top of page" entry;
- where the filter sits on a question card and on a heading card;
- the collapsed trigger's icon, its count badge, and what its popover contains.

Record every difference and say for each whether it is in scope for W7 or belongs to a later
workstream.

- [ ] **Step 10: Open the PR and stop**

Open the PR against `feat/dashboard-parity-06-filters` with every measurement above quoted verbatim
and the frontend test count from the final `npm run test`. The body must also state, in one sentence
each:

- the field is `card.options.inlineParameters` — parameter ids on the card, not a card id on the
  parameter — because `replace_layout` deletes and re-inserts every card row on save, so card ids
  do not survive (`crates/egret-bi/src/dashboard.rs:344-376`), and it needs no backend change
  because `CardInput.options` is unvalidated `serde_json::Value` (`bi.rs:393-394`);
- an inline parameter still filters every card it is mapped to — only the widget moves;
- deleting a host card deletes its filters, matching Metabase, and Cancel is today's recovery
  because W2's undo toast has not landed;
- **duplicating** a host card gives the clone no filters, which is a deliberate divergence from
  Metabase's parameter-duplicating behaviour (`cards-typed.ts:416-441`) and why;
- a filter hosted by a card that hide-empty has hidden is unreachable until the card returns, and
  the required-filter version of that cannot happen (`hide-empty.ts:39`).

Then **stop** — the merge is the user's call.

---

## Self-review checklist for the implementer

- [ ] `topBarParameters` reaches **exactly two** call sites — `DashboardFilterBar` and
      `EditFilterChips`. `cardQueryArgs`, `applyParamsToChart`, `interpolateParams`,
      `resolveValuesSource`, `computeLinkedFilters` and the required-filter check all still read the
      full `parameters` array. Grep for `topBar` and count: two.
- [ ] `EditFilterChips`'s `onReorder` goes through `mergeTopBarOrder`. Without it, dragging a chip
      deletes every filter an author moved onto a card, and nothing in the suite would say so.
- [ ] `findHostCardId` scans `cards`, never `visibleCards` or `tabCards`.
- [ ] `DashboardView.tsx` gained wiring only: one `topBar` line, one `inline` object at the card
      call site, three props on `EditSidebar`, and four lifecycle one-liners that each call a tested
      pure function. Every decision lives in `inlineParameters.ts`.
- [ ] The plan imports **nothing** from W1's `grid-breakpoints.ts` — W7 must stay droppable, and it
      is cut from W6, which is cut from `main`.
- [ ] The collapse predicate is a pure exported function taking four numbers, and its test feeds
      them in directly. No test pretends to measure width in jsdom.
- [ ] `reservedRef` is read with `scrollWidth`, not `getBoundingClientRect().width`. The second
      never collapses anything, and the failure is silent.
- [ ] No test asserts a display string where the code keys on a raw value. The three places this was
      a live risk — the pill emitting `["EU"]`, the move field emitting `null` rather than
      `"top-of-page"`, the clone's `options` — all assert the raw value.
- [ ] Every new assertion was watched failing against a stubbed implementation, scored by **exit
      code**, not by matching stdout text.
- [ ] No new dependency. The collapse uses one `ResizeObserver`, guarded for jsdom exactly as
      `DashboardGridLayout.tsx:404` guards its own.
- [ ] The PR targets `feat/dashboard-parity-06-filters` and re-includes none of W6's commits.
- [ ] The plan stopped at the PR.
