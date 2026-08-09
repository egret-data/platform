# W6 — Filter and Parameter Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring the dashboard filter surface to Metabase's — a multi-value filter that survives a
reload, an author-chosen multiplicity, reorderable chips, one-click connection of a filter to every
card that has the column, an optional draft-then-Apply mode, a linked-filters panel that says why it
is empty, and a default-value control that matches the parameter's type.

**Architecture:** every piece of decision-making lands in a **pure module** under
`components/chart/dashboard/`, and `DashboardView.tsx` gets wiring only. That is not stylistic:
`DashboardView.tsx` is 769 lines and seven of the nine workstreams touch it, so a rule that each
workstream must be testable without rendering the route is the only thing keeping it reviewable.
Every behaviour below is therefore unit-tested against a function, and the route is exercised only
by the live check.

**Tech Stack:** React 18 + TypeScript, Tailwind v4 (OKLCH tokens), Radix (`Switch`, `Checkbox`,
`Popover`), `@dnd-kit/core` + `@dnd-kit/sortable` (**already dependencies** — `FieldZone.tsx`,
`SortRows.tsx`, `ChartTable.tsx`, `NotebookPage.tsx` all use them), `sonner` for toasts, vitest +
Testing Library.

**Design reference:** `docs/en/specs/2026-08-09-dashboard-parity-design.md` §2.5 and §4 W6. §9 lists
findings that did **not** survive inspection — do not plan work for them.

**Metabase reference:** the sibling checkout at `data-portal/metabase`. Files quoted in this plan:
`metabase-lib/v1/parameters/constants.ts:243-259`,
`metabase/parameters/utils/parameter-type.ts`,
`metabase-lib/v1/parameters/utils/parameter-values.js:178-180`,
`metabase/parameters/components/ParametersList/ParametersList.tsx:51`,
`metabase/dashboard/actions/auto-wire-parameters/{toasts.ts,constants.ts}`,
`metabase/dashboard/components/FilterApplyToast/{FilterApplyToast.tsx,utils.ts}`,
`metabase/dashboard/components/ParameterLinkedFilters/ParameterLinkedFilters.tsx`,
`metabase/dashboard/components/ParameterSettings/ParameterSettings.tsx:221-241`.

**Branch:** `feat/dashboard-parity-06-filters`, from `main`. W6 has no dependency on W1–W5.

---

## Background the implementer needs

### The `location` bug is real, but the spec describes the wrong half of it

§2.5 says a multi-value `location` "is written with `String(value)` — a comma-join". **It is not.**
Read `useDashboardParams.ts:32`:

```ts
else n.set(slug, typeof value === "object" ? JSON.stringify(value) : String(value));
```

`typeof [] === "object"`, so the **write is already correct JSON** for every array, `location`
included. The defect is entirely on the **read** side, at `:21`:

```ts
if (ARRAY_TYPES.has(p.type) || p.operator === "between" || p.operator === "range") {
```

`ARRAY_TYPES` is `new Set(["category"])`, so a `location` raw of `["Hanoi","Da Nang"]` falls through
to `values[p.slug] = raw` and comes back as the **string** `'["Hanoi","Da Nang"]'`. Downstream,
`paramToFilters`'s `location` case (`params.ts:240-248`) sees a non-array, wraps it, and emits
`in ['["Hanoi","Da Nang"]']` — a filter that matches nothing — while `FilterPill`'s list shows no
box ticked (`FilterPill.tsx:159` does `[String(value)]`). So the user-visible symptom is *silently
zero rows after a reload*, not "loses every value after the first".

The fix is the same one line the spec asks for. The round-trip test is still exactly the right test,
and the comma-containing value still belongs in it: it is the case a comma-join could not survive
even in principle, so it pins the encoding against a future "simplification" that reintroduces one.

**A second defect found while reading, fixed in the same task.** Because `category` *is* in
`ARRAY_TYPES`, any scalar category value in the URL is fed to `JSON.parse`. `?tags=42` therefore
reads back as the **number** `42`, not `"42"`, and `in [42]` against a text column matches nothing.
This is reachable today, not hypothetical: `DashboardView.tsx:754` (drill → "Filter by") and
`:236` (crossfilter click behaviour) both call `setValue` with a **scalar**. Task 1 gates the decode
on the raw actually looking like a JSON array.

### The multiplicity table ports as a rename, not a reinterpretation

Metabase keys `SINGLE_OR_MULTI_SELECTABLE_TYPES` on its parameter *sectionId* and *subtype*; egret
keys on `ParamType` and `ParamOperator`. The two axes line up one-for-one:

| Metabase | egret |
|---|---|
| `string: ["=", "!=", "contains", "does-not-contain", "starts-with", "ends-with"]` | `text: ["is", "is-not", "contains", "does-not-contain", "starts-with", "ends-with"]` |
| `category: "any"` | `category: "any"` |
| `id: "any"` | `id: "any"` |
| `location: ["=", "!="]` | `location: ["is", "is-not"]` |
| `number: ["=", "!="]` | `number: ["=", "!="]` (spelled identically) |

`date`, `boolean` and `temporal_unit` are absent from Metabase's table and stay absent.

**Where egret cannot honour it, the control is suppressed rather than faked.** egret's `text` and
`number` widgets are single-value `<input>`s (`FilterPill.tsx:151-154,200-202`); a "Multiple values"
radio there would change nothing a user could see. The table is ported verbatim and unit-tested
against Metabase's pairs, and a second predicate decides what is actually *rendered*. Say so in the
code, so the next reader does not "fix" the gap by deleting the suppression.

**The `id` default must not flip.** Metabase's `getIsMultiSelect` returns `parameter.isMultiSelect ??
true`. Applying that blanket default to egret's `id` parameters would turn every existing id filter
from a scalar into an array on load, and `paramToFilters`'s `id` case emits `eq` with whatever it is
handed — so every id filter on every saved dashboard would start filtering on an array. `id` keeps
its current single-value default and becomes multi only when the author says so.

### Drag: use what is already here

The repo already depends on `@dnd-kit/core`, `@dnd-kit/sortable` and `@dnd-kit/utilities`, and
`FieldZone.tsx:129-133` establishes the sensor pattern (`PointerSensor` with an activation distance,
plus `KeyboardSensor` with `sortableKeyboardCoordinates`). `lib/reorderIds.ts` is the pure
`arrayMove` helper. Metabase's `ParametersList.tsx:51` uses `useDndSensors({ distance: 15 })` — the
same library, the same knob. Do **not** add a dependency, and do **not** copy `DashboardTabBar`'s
native HTML5 drag (`:44-48`): it gives no keyboard path and no activation distance, and a chip that
is also a click target needs the distance.

Reorder is **edit-mode only**, matching Metabase (`isSortable && isEditing && setParameterIndex`).
The order is a saved property of the dashboard; a viewer must not be able to change it.

### Auto-apply has **no typed home**, and this plan does not invent one

Investigated end to end:

- `DashboardDetail` (`api/dashboards.ts:66-76`) carries `name`, `description`, `created_by`,
  `updated_at`, `parameters`, `tabs`, `width`, `cards`, `deleted_at`, `bookmarked`. Nothing else.
- `UpdateDashboard` (`crates/egret-analytics-api/src/bi.rs:419-430`) accepts exactly `name`,
  `description`, `parameters`, `width`, `tabs`, `cards`. The handler
  (`crates/egret-bi/src/routes.rs:2693-2722`) persists them with
  `st.dashboards.update(id, &b.name, &b.description, &b.parameters, &b.width)`.
- The `dashboards` table (`migrations/0015_dashboard.sql`, `0016`, `0018`) has
  `name, description, parameters jsonb, created_by, folder_id, updated_at, width, deleted_at`.
- The only opaque JSON bags that reach the server are `parameters` (typed `DashboardParameter[]`
  on the client and mapped over in eight places — a non-parameter entry there would render a
  phantom filter pill) and each card's `options`.

So: **there is no dashboard-level settings bag, and Metabase's `auto_apply_filters` is a real column
on `report_dashboard`.** Matching it needs a migration + DTO field + PUT field, which is backend
work the index's file table declares W6 does not do.

**The smallest honest option, and what this plan implements:** ship the whole draft/Apply mechanism
(the valuable, testable part) and persist the *toggle* as a **per-user, per-dashboard browser
preference** in `localStorage` under `egret:dash-auto-apply:<dashboardId>` — the same convention as
`egret:last-workspace` (`components/layout/Header.tsx:51`) and `sqllab:tabs:<ws>`
(`hooks/useTabs.ts:11`). The control is labelled with that scope so it does not lie about being
shared. Making it a dashboard-level setting is a **follow-up needing a column**, and it is called out
in the PR body rather than smuggled into this branch.

### The linked-filters panel is closer than the spec implies

`FilterSettingsPanel.tsx:107-130` already renders a "Linked filters" heading and a one-line
paraphrase. What is missing is Metabase's heading wording, its exact sentence, and — the real gap —
the three states where the list is *legitimately* empty and today renders nothing at all (the whole
block is behind `others.length > 0`).

---

## Files this plan adds beyond the index's table, and why

The index (`…-00-index.md`) lists two new files for W6: `autoConnect.ts` and `DefaultValueField.tsx`.
Both are created here. Five more are added, every one of them a pure module or a small component
extracted **so `DashboardView.tsx` does not grow**, which is the rule the table exists to enforce:

| Extra file | Why it is not in `DashboardView.tsx` |
|---|---|
| `multiSelect.ts` | The ported Metabase table plus two predicates. Pure; tested against Metabase's pairs. |
| `filterDrafts.ts` | Change counting and its wording. Pure; a port of Metabase's `getFilterChangeDescription`. |
| `api/useFilterDrafts.ts` | The draft/apply state machine + the localStorage preference. A hook, not route code. |
| `FilterApplyBar.tsx` | The Apply/Cancel bar. Rendered by `DashboardFilterBar`, so the route passes props and nothing else. |
| `linkedFilters.ts` | The four-state machine behind the linked-filters panel. Pure. |

---

## File structure

| Path | Responsibility after this workstream |
|---|---|
| `frontend/src/api/useDashboardParams.ts` | URL round-trip for every parameter value. Gains `location` (and later `id`) as array types, a decode guard, and a batched `setValues`. |
| `frontend/src/components/chart/dashboard/multiSelect.ts` | **New.** `SINGLE_OR_MULTI_SELECTABLE`, `isSingleOrMultiSelectable`, `canRenderMultiValue`, `isMultiValue`. Pure. |
| `frontend/src/components/chart/dashboard/autoConnect.ts` | **New.** Target resolution, the mapping patch, its inverse, the two toast strings and their timeouts, and `mappableColumns`. Pure except `offerAutoConnect`, which only calls `sonner`. |
| `frontend/src/components/chart/dashboard/filterDrafts.ts` | **New.** `countChangedFilters`, `describeFilterChanges`. Pure. |
| `frontend/src/api/useFilterDrafts.ts` | **New.** Draft values, Apply/Cancel, and the auto-apply preference. |
| `frontend/src/components/chart/dashboard/FilterApplyBar.tsx` | **New.** The bottom-centre Apply/Cancel bar. |
| `frontend/src/components/chart/dashboard/linkedFilters.ts` | **New.** `linkedFilterState(param, others)` → list / no-usable / custom-source / input-widget. Pure. |
| `frontend/src/components/chart/dashboard/DefaultValueField.tsx` | **New.** The typed default-value widget; reuses `FilterControl`. |
| `frontend/src/components/chart/dashboard/FilterPill.tsx` | Honours per-parameter multiplicity; exports `FilterControl` so the settings panel can reuse it. |
| `frontend/src/components/chart/dashboard/FilterSettingsPanel.tsx` | Gains "People can pick", the Metabase linked-filters framing, and the typed default field. |
| `frontend/src/components/chart/dashboard/EditFilterChips.tsx` | Drag-reorderable chips (edit mode only). |
| `frontend/src/components/chart/dashboard/DashboardFilterBar.tsx` | Hosts the auto-apply switch and the Apply bar. |
| `frontend/src/components/chart/dashboard/EditSidebar.tsx` | Two new pass-through props. |
| `frontend/src/routes/explore/DashboardView.tsx` | Wiring only. |

---

## Task 1: `location` survives the URL

This is a data-loss bug, it is one line plus a guard, and it lands first so it can ship even if the
rest of the branch stalls.

**Files:**
- Modify: `frontend/src/api/useDashboardParams.ts`
- Test: `frontend/src/api/__tests__/useDashboardParams.test.tsx` (extend the existing file — it
  already has the `MemoryRouter` wrapper and the `renderHook`/`act` shape this needs)

- [ ] **Step 1: Write the failing tests**

In `frontend/src/api/__tests__/useDashboardParams.test.tsx`, add a `location` parameter to the
shared fixture and append two tests:

```tsx
const params: DashboardParameter[] = [
  { id: "p1", name: "Region", slug: "region", type: "text", operator: "is", default: "EU" },
  { id: "p2", name: "Tags", slug: "tags", type: "category", operator: "is" },
  { id: "p3", name: "City", slug: "city", type: "location", operator: "is" },
];
```

```tsx
  it("round-trips a multi-value location, including a value containing a comma", () => {
    // A genuine round trip: the write goes into the MemoryRouter's search params and the read
    // comes back out of them. The comma-bearing value is the point — it is the case a
    // comma-joined encoding could not survive even in principle, so this pins the encoding
    // against anyone who reintroduces one.
    const { result } = renderHook(() => useDashboardParams(params), { wrapper: wrap("/d") });
    const picked = ["Hà Nội", "Đà Nẵng, Việt Nam"];
    act(() => result.current.setValue("city", picked));
    expect(result.current.values.city).toEqual(picked);
  });

  it("does not JSON-decode a scalar list value", () => {
    // Reachable today: DashboardView's drill "Filter by" (:754) and crossfilter click behaviour
    // (:236) both call setValue with a SCALAR. Decoding it turned "42" into the NUMBER 42, and
    // `in [42]` against a text column matches nothing. Assert the RAW value, not a label.
    const { result } = renderHook(() => useDashboardParams(params), { wrapper: wrap("/d?tags=42") });
    expect(result.current.values.tags).toBe("42");
  });
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd frontend && npx vitest run src/api/__tests__/useDashboardParams.test.tsx`

Expected: FAIL, 2 of 6. The first reports
`expected '["Hà Nội","Đà Nẵng, Việt Nam"]' to deeply equal [ 'Hà Nội', 'Đà Nẵng, Việt Nam' ]`;
the second reports `expected 42 to be '42'`.

- [ ] **Step 3: Write the implementation**

In `frontend/src/api/useDashboardParams.ts`, replace `ARRAY_TYPES` and the read branch:

```ts
/** Parameter types whose value is an ARRAY, and so is JSON-encoded in the URL. `location` is
 *  multi-select (`FilterPill.tsx:87`) and was missing here, so a multi-value location was written
 *  as JSON and read back as the JSON *string* — `paramToFilters` then emitted
 *  `in ['["Hanoi","Da Nang"]']` and the card silently returned no rows. */
const ARRAY_TYPES = new Set(["category", "location"]);
```

```ts
    if (ARRAY_TYPES.has(p.type) || p.operator === "between" || p.operator === "range") {
      // Decode only what `setValue` actually encoded. It JSON-encodes objects and arrays and
      // writes everything else through String(), so a raw that is not array-shaped is a scalar —
      // a hand-edited URL, or a drill/crossfilter write (`DashboardView.tsx:754,236`). Parsing it
      // anyway turned "42" into the NUMBER 42 and the resulting `in [42]` matched nothing.
      if (raw.startsWith("[")) {
        try { values[p.slug] = JSON.parse(raw); } catch { values[p.slug] = raw; }
      } else {
        values[p.slug] = raw;
      }
    } else {
      values[p.slug] = raw;
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd frontend && npx vitest run src/api/__tests__/useDashboardParams.test.tsx`

Expected: PASS, 6 tests.

- [ ] **Step 5: Prove the tests survive mutation**

Two separate mutations, each run on its own; score by **exit code**, never by matching stdout.

1. Revert `ARRAY_TYPES` to `new Set(["category"])`. Re-run. Expected: FAIL — the round-trip test.
2. Restore it, then drop the guard so the branch reads
   `try { values[p.slug] = JSON.parse(raw); } catch { values[p.slug] = raw; }` unconditionally.
   Re-run. Expected: FAIL — the scalar test.

Restore both. If either mutation leaves the suite green, the assertion is not verifying what it
claims and must be rewritten before moving on.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/api/useDashboardParams.ts frontend/src/api/__tests__/useDashboardParams.test.tsx
git commit -m "fix(dashboard): round-trip a multi-value location filter through the URL"
```

---

## Task 2: "People can pick" — Multiple values / A single value

**Files:**
- Create: `frontend/src/components/chart/dashboard/multiSelect.ts`
- Modify: `frontend/src/api/dashboards.ts`, `frontend/src/api/useDashboardParams.ts`,
  `frontend/src/components/chart/dashboard/FilterPill.tsx`,
  `frontend/src/components/chart/dashboard/FilterSettingsPanel.tsx`,
  `frontend/src/components/chart/dashboard/params.ts`
- Test: `frontend/src/components/chart/dashboard/__tests__/multiSelect.test.ts`, plus additions to
  `FilterPill.test.tsx` and `FilterSettingsPanel.test.tsx`

- [ ] **Step 1: Write the failing test for the ported table**

Create `frontend/src/components/chart/dashboard/__tests__/multiSelect.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import {
  SINGLE_OR_MULTI_SELECTABLE,
  canRenderMultiValue,
  isMultiValue,
  isSingleOrMultiSelectable,
} from "../multiSelect";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "1", name: "Region", slug: "region", type: "category", operator: "is", ...over });

describe("SINGLE_OR_MULTI_SELECTABLE", () => {
  it("carries exactly Metabase's pairs, under egret's operator spellings", () => {
    // Ported from metabase-lib/v1/parameters/constants.ts:243-259. Metabase's "string" is
    // egret's "text" and its "=" / "!=" subtypes are egret's "is" / "is-not"; every other
    // spelling is already identical.
    expect(SINGLE_OR_MULTI_SELECTABLE).toEqual({
      text: ["is", "is-not", "contains", "does-not-contain", "starts-with", "ends-with"],
      category: "any",
      id: "any",
      location: ["is", "is-not"],
      number: ["=", "!="],
    });
  });
});

describe("isSingleOrMultiSelectable", () => {
  it("is true for every listed pair and false for the types Metabase omits", () => {
    expect(isSingleOrMultiSelectable(p({ type: "category", operator: "is" }))).toBe(true);
    expect(isSingleOrMultiSelectable(p({ type: "id", operator: "is" }))).toBe(true);
    expect(isSingleOrMultiSelectable(p({ type: "location", operator: "is-not" }))).toBe(true);
    expect(isSingleOrMultiSelectable(p({ type: "text", operator: "contains" }))).toBe(true);
    expect(isSingleOrMultiSelectable(p({ type: "number", operator: "=" }))).toBe(true);
    // number/between is NOT in Metabase's list — the operator gate has to bite, not just the type.
    expect(isSingleOrMultiSelectable(p({ type: "number", operator: "between" }))).toBe(false);
    expect(isSingleOrMultiSelectable(p({ type: "date", operator: "range" }))).toBe(false);
    expect(isSingleOrMultiSelectable(p({ type: "boolean", operator: "is" }))).toBe(false);
    expect(isSingleOrMultiSelectable(p({ type: "temporal_unit", operator: "is" }))).toBe(false);
  });
});

describe("canRenderMultiValue", () => {
  it("is true only where egret has a list widget that can honour it", () => {
    expect(canRenderMultiValue(p({ type: "category" }))).toBe(true);
    expect(canRenderMultiValue(p({ type: "location", operator: "is" }))).toBe(true);
    expect(canRenderMultiValue(p({ type: "id", operator: "is" }))).toBe(true);
    // In Metabase's table but single-value inputs here — a radio would change nothing visible.
    expect(canRenderMultiValue(p({ type: "text", operator: "contains" }))).toBe(false);
    expect(canRenderMultiValue(p({ type: "number", operator: "=" }))).toBe(false);
    // The input-box widget is a free-text box, so multiplicity has nowhere to land.
    expect(canRenderMultiValue(p({ type: "category", widget: "input" }))).toBe(false);
    expect(canRenderMultiValue(p({ type: "category", widget: "search" }))).toBe(true);
  });
});

describe("isMultiValue", () => {
  it("defaults multi for the list types, single for id, and obeys an explicit choice", () => {
    expect(isMultiValue(p({ type: "category" }))).toBe(true);
    expect(isMultiValue(p({ type: "location", operator: "is" }))).toBe(true);
    // id stays single by default: every saved id filter is a scalar today, and flipping the
    // default would hand `paramToFilters` an array on load.
    expect(isMultiValue(p({ type: "id", operator: "is" }))).toBe(false);
    expect(isMultiValue(p({ type: "id", operator: "is", isMultiSelect: true }))).toBe(true);
    expect(isMultiValue(p({ type: "category", isMultiSelect: false }))).toBe(false);
    expect(isMultiValue(p({ type: "text", operator: "is" }))).toBe(false);
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/multiSelect.test.ts`

Expected: FAIL — `Failed to resolve import "../multiSelect"`.

- [ ] **Step 3: Add the `isMultiSelect` field to the parameter type**

In `frontend/src/api/dashboards.ts`, inside `interface DashboardParameter`, after
`widget?: ParamWidget;`:

```ts
  /** "People can pick": true = Multiple values, false = A single value. Absent means the type's
   *  default (see `multiSelect.ts`). The server stores `parameters` as an opaque JSONB value
   *  (`crates/egret-bi/src/dashboard.rs:189`), so this needs no backend change. */
  isMultiSelect?: boolean;
```

- [ ] **Step 4: Write the implementation**

Create `frontend/src/components/chart/dashboard/multiSelect.ts`:

```ts
import type { DashboardParameter, ParamOperator, ParamType } from "@/api/dashboards";

/**
 * Which (type, operator) pairs let the author choose between "Multiple values" and "A single
 * value". Ported verbatim from Metabase's SINGLE_OR_MULTI_SELECTABLE_TYPES
 * (metabase-lib/v1/parameters/constants.ts:243-259), which keys on its own sectionId and subtype.
 * The two axes are egret's ParamType and ParamOperator under different spellings:
 *
 *   Metabase "string"            → egret "text",     "=" / "!=" → "is" / "is-not"
 *   Metabase "category" / "id"   → "any": every operator qualifies
 *   Metabase "location" ["=","!="] → egret ["is", "is-not"]
 *   Metabase "number"   ["=","!="] → identical spelling here
 *
 * `date`, `boolean` and `temporal_unit` are absent from Metabase's table and stay absent: they are
 * never single-or-multi selectable.
 */
export const SINGLE_OR_MULTI_SELECTABLE: Partial<Record<ParamType, "any" | ParamOperator[]>> = {
  text: ["is", "is-not", "contains", "does-not-contain", "starts-with", "ends-with"],
  category: "any",
  id: "any",
  location: ["is", "is-not"],
  number: ["=", "!="],
};

/** Metabase's `isSingleOrMultiSelectable` (parameters/utils/parameter-type.ts), unchanged. */
export function isSingleOrMultiSelectable(param: DashboardParameter): boolean {
  const entry = SINGLE_OR_MULTI_SELECTABLE[param.type];
  if (!entry) return false;
  if (entry === "any") return true;
  return entry.includes(param.operator);
}

/**
 * Whether egret has a widget that can actually honour the choice — and so whether the radio group
 * is rendered at all.
 *
 * `text` and `number` are in the ported table above, and they are DELIBERATELY excluded here:
 * egret's text and number filters are single-value `<input>`s (`FilterPill.tsx:151-154,200-202`),
 * so a "Multiple values" radio beside them would set a flag nothing reads. Metabase can offer it
 * because its widget is a token input; adding one here is a separate piece of work. Do not "fix"
 * the gap by deleting this predicate — that produces a control that lies.
 *
 * The input-box widget is excluded for the same reason: it is a free-text box either way.
 */
export function canRenderMultiValue(param: DashboardParameter): boolean {
  const listType = param.type === "category" || param.type === "location" || param.type === "id";
  return listType && (param.widget ?? "dropdown") !== "input";
}

/** Per-type default when the author has not chosen.
 *
 *  Metabase's `getIsMultiSelect` (parameter-values.js:178-180) defaults everything to multi. egret
 *  cannot: an `id` filter has always written a scalar and `paramToFilters`'s id case has always
 *  emitted `eq`, so a blanket multi default would change how every saved dashboard's id filter
 *  behaves the moment it loads. Multi for id is opt-in per parameter. */
const DEFAULT_MULTI_VALUE: Partial<Record<ParamType, boolean>> = {
  category: true,
  location: true,
  id: false,
};

export function isMultiValue(param: DashboardParameter): boolean {
  return param.isMultiSelect ?? DEFAULT_MULTI_VALUE[param.type] ?? false;
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/multiSelect.test.ts`

Expected: PASS, 5 tests.

- [ ] **Step 6: Prove it survives mutation**

Change `isSingleOrMultiSelectable`'s last line to `return true;`. Re-run. Expected: FAIL on the
`number/between` case. Restore. Change `DEFAULT_MULTI_VALUE.id` to `true`. Re-run. Expected: FAIL on
the `isMultiValue` id case. Restore.

- [ ] **Step 7: Make `id` accept an array before anything can produce one**

An `id` parameter set to "Multiple values" now yields an array, and `params.ts:222-223` emits
`eq` with whatever it is handed. Widen it, preserving the scalar path byte for byte.

In `frontend/src/components/chart/dashboard/params.ts`, replace the `case "id":` arm:

```ts
    case "id": {
      // A single id keeps emitting `eq` — that is what every saved dashboard's id filter does and
      // what `params.test.ts` pins. Only an author who turned "People can pick" to Multiple values
      // can produce more than one, and that becomes `in`.
      const arr = Array.isArray(value)
        ? value.filter((v) => !isBlank(v))
        : isBlank(value)
          ? []
          : [value];
      if (arr.length === 0) return [];
      return arr.length === 1 ? [mkf("eq", arr[0])] : [mkf("in", arr)];
    }
```

Add to `frontend/src/components/chart/dashboard/__tests__/params.test.ts`, inside the existing
`describe("paramToFilters", …)`:

```ts
  it("id: one value stays eq, several become in", () => {
    expect(paramToFilters(p({ type: "id", operator: "is" }), ["7"], "id")).toEqual([{ col: "id", op: "eq", value: "7" }]);
    expect(paramToFilters(p({ type: "id", operator: "is" }), ["7", "9"], "id")).toEqual([{ col: "id", op: "in", value: ["7", "9"] }]);
  });
```

Then add `id` to the URL array types, so a multi id round-trips. In
`frontend/src/api/useDashboardParams.ts`:

```ts
const ARRAY_TYPES = new Set(["category", "location", "id"]);
```

The decode guard from Task 1 is what makes this safe: a single-value id of `"42"` is not
array-shaped, so it still reads back as the string `"42"`.

Add to `frontend/src/api/__tests__/useDashboardParams.test.tsx`:

```ts
  it("round-trips a multi-value id without coercing the single-value form", () => {
    const idParams: DashboardParameter[] = [
      { id: "p4", name: "User", slug: "user", type: "id", operator: "is", isMultiSelect: true },
    ];
    const { result } = renderHook(() => useDashboardParams(idParams), { wrapper: wrap("/d?user=42") });
    expect(result.current.values.user).toBe("42");
    act(() => result.current.setValue("user", ["42", "43"]));
    expect(result.current.values.user).toEqual(["42", "43"]);
  });
```

- [ ] **Step 8: Honour the multiplicity in the pill**

In `frontend/src/components/chart/dashboard/FilterPill.tsx`:

Add the import beside the existing `./params` one:

```tsx
import { isMultiValue } from "./multiSelect";
```

Replace `clearedValue` (`:26-31`):

```tsx
/** Cleared value for a parameter: [] where the control emits an array, "" otherwise. */
function clearedValue(p: DashboardParameter): unknown {
  const listType = p.type === "category" || p.type === "location" || p.type === "id";
  if (listType && isMultiValue(p)) return [];
  if (p.type === "number" && p.operator === "between") return [];
  if (p.type === "date" && p.operator === "range") return [];
  return "";
}
```

In `FilterControl`, replace the `const multi = listType;` line (`:158`) inside the list branch:

```tsx
    // The list is rendered for the list types and for an id that has options; whether it is a
    // checkbox list or a radio list is the author's "People can pick" choice.
    const multi = isMultiValue(param);
```

- [ ] **Step 9: Write the failing pill tests, then see them pass**

Add to `frontend/src/components/chart/dashboard/__tests__/FilterPill.test.tsx`:

```tsx
  it("single-value category emits the RAW option, not an array, and renders no checkboxes", () => {
    // Keying on the raw value matters: the code stores what it emits here, and a test that
    // asserted the label "EU" would pass against an implementation storing ["EU"].
    const onChange = vi.fn();
    render(<FilterPill param={p({ isMultiSelect: false })} value="" options={["EU", "US"]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /^Region/ }));
    expect(screen.queryAllByRole("checkbox")).toHaveLength(0);
    fireEvent.click(screen.getByLabelText("option US"));
    expect(onChange).toHaveBeenCalledWith("US");
  });

  it("multi-value id emits an array once the author opts in", () => {
    const onChange = vi.fn();
    render(<FilterPill param={p({ type: "id", name: "User", slug: "user", isMultiSelect: true })}
      value={[]} options={["1", "2"]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /^User/ }));
    fireEvent.click(screen.getByLabelText("option 2"));
    expect(onChange).toHaveBeenCalledWith(["2"]);
  });

  it("clearing a single-value category yields \"\", not []", () => {
    const onChange = vi.fn();
    render(<FilterPill param={p({ isMultiSelect: false })} value="EU" options={["EU"]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "clear Region" }));
    expect(onChange).toHaveBeenCalledWith("");
  });
```

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/FilterPill.test.tsx`

Expected: PASS, 3 new tests plus the existing ones. The existing "id popover: single-select calls
onChange with a scalar" test must still pass untouched — that is the regression this default
protects.

- [ ] **Step 10: Add the radio group to the settings panel**

In `frontend/src/components/chart/dashboard/FilterSettingsPanel.tsx`, add the import:

```tsx
import { canRenderMultiValue, isMultiValue, isSingleOrMultiSelectable } from "./multiSelect";
```

and, immediately **after** the `{showValueSource && ( … )}` block and **before** the default-value
label — the same position Metabase gives it (`ParameterSettings.tsx:221-241`, between "How should
people filter on this column?" and "Default value"):

```tsx
      {isSingleOrMultiSelectable(param) && canRenderMultiValue(param) && (
        <div className="flex flex-col gap-1">
          <div className="text-xs text-muted-foreground">People can pick</div>
          <label className="flex items-center gap-2 text-sm">
            <input type="radio" name={`multi-${param.id}`} aria-label="people can pick multiple"
              checked={isMultiValue(param)}
              onChange={() => onChange({ ...param, isMultiSelect: true })} /> Multiple values
          </label>
          <label className="flex items-center gap-2 text-sm">
            <input type="radio" name={`multi-${param.id}`} aria-label="people can pick single"
              checked={!isMultiValue(param)}
              onChange={() => onChange({ ...param, isMultiSelect: false })} /> A single value
          </label>
        </div>
      )}
```

- [ ] **Step 11: Write the panel tests**

Add to `frontend/src/components/chart/dashboard/__tests__/FilterSettingsPanel.test.tsx`, inside
`describe("FilterSettingsPanel tier2d", …)`:

```tsx
  it("offers People can pick for a category, and writes the RAW flag", () => {
    const onChange = vi.fn();
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={onChange} onDelete={vi.fn()} />);
    expect(screen.getByLabelText("people can pick multiple")).toBeTruthy();
    fireEvent.click(screen.getByLabelText("people can pick single"));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ isMultiSelect: false }));
  });

  it("hides People can pick where egret has no widget that could honour it", () => {
    // date is absent from Metabase's table; text is present but rendered as a single input here.
    render(<FilterSettingsPanel param={p({ type: "date", operator: "range" })} allParameters={[]} onChange={vi.fn()} onDelete={vi.fn()} />);
    expect(screen.queryByLabelText("people can pick multiple")).toBeNull();
  });

  it("hides People can pick when the widget is an input box", () => {
    render(<FilterSettingsPanel param={p({ widget: "input" })} allParameters={[]} onChange={vi.fn()} onDelete={vi.fn()} />);
    expect(screen.queryByLabelText("people can pick multiple")).toBeNull();
  });
```

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/FilterSettingsPanel.test.tsx`

Expected: PASS.

- [ ] **Step 12: Prove the wiring survives mutation**

Change `FilterPill`'s `const multi = isMultiValue(param);` back to `const multi = listType;`.
Re-run `FilterPill.test.tsx`. Expected: FAIL on the single-value category test. Restore.

Change the panel's guard to `{isSingleOrMultiSelectable(param) && (`. Re-run
`FilterSettingsPanel.test.tsx`. Expected: FAIL on the input-box test. Restore.

- [ ] **Step 13: Run the gates and commit**

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
git add frontend/src/components/chart/dashboard/multiSelect.ts \
        frontend/src/components/chart/dashboard/__tests__/multiSelect.test.ts \
        frontend/src/components/chart/dashboard/FilterPill.tsx \
        frontend/src/components/chart/dashboard/FilterSettingsPanel.tsx \
        frontend/src/components/chart/dashboard/params.ts \
        frontend/src/api/dashboards.ts \
        frontend/src/api/useDashboardParams.ts \
        frontend/src/components/chart/dashboard/__tests__/FilterPill.test.tsx \
        frontend/src/components/chart/dashboard/__tests__/FilterSettingsPanel.test.tsx \
        frontend/src/components/chart/dashboard/__tests__/params.test.ts \
        frontend/src/api/__tests__/useDashboardParams.test.tsx
git commit -m "feat(dashboard): let a filter's author choose Multiple values or A single value"
```

---

## Task 3: Drag-reorder of filter chips

**Files:**
- Modify: `frontend/src/components/chart/dashboard/EditFilterChips.tsx`,
  `frontend/src/routes/explore/DashboardView.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/EditFilterChips.test.tsx`

Reorder is **edit-mode only**: the order is a saved property, so a viewer must not change it. This
matches Metabase, whose `ParametersList` only renders a drag handle when
`isSortable && isEditing && setParameterIndex` (`ParametersList.tsx:90-104`).

- [ ] **Step 1: Write the failing test**

`@dnd-kit` needs real layout to simulate a pointer drag, and jsdom measures nothing — a test that
"drags" here would be asserting the harness, not the component. So the reorder arithmetic is tested
as a pure function, the activation distance is tested as the exported constant the component
actually uses, and the drag itself is measured in the browser in Task 8. Say that out loud rather
than shipping a green test that proves nothing.

Replace `frontend/src/components/chart/dashboard/__tests__/EditFilterChips.test.tsx` with:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { EditFilterChips, FILTER_DRAG_ACTIVATION_PX, reorderParameters } from "../EditFilterChips";
import type { DashboardParameter } from "@/api/dashboards";

const params: DashboardParameter[] = [
  { id: "a", name: "Region", slug: "region", type: "category", operator: "is" },
  { id: "b", name: "Priority", slug: "priority", type: "category", operator: "is" },
  { id: "c", name: "Owner", slug: "owner", type: "category", operator: "is" },
];

describe("EditFilterChips", () => {
  it("renders a chip per param, marks the active one, and selects on click", () => {
    const onSelect = vi.fn();
    render(<EditFilterChips parameters={params} activeId="a" onSelect={onSelect} onReorder={vi.fn()} />);
    expect(screen.getByRole("button", { name: "Region" }).getAttribute("data-active")).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: "Priority" }));
    expect(onSelect).toHaveBeenCalledWith("b");
  });

  it("names each drag handle after its filter", () => {
    // Three identical "drag" buttons tell a screen-reader user nothing about which chip they hold.
    render(<EditFilterChips parameters={params} activeId={null} onSelect={vi.fn()} onReorder={vi.fn()} />);
    expect(screen.getByRole("button", { name: "reorder Region" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "reorder Owner" })).toBeTruthy();
  });

  it("uses Metabase's 15px drag activation distance", () => {
    // A chip is also a click target (it opens the settings panel). Metabase's own list sets
    // `useDndSensors({ distance: 15 })` (ParametersList.tsx:51) for exactly that reason; below
    // this, a click that wanders a pixel starts a drag instead of opening the panel.
    expect(FILTER_DRAG_ACTIVATION_PX).toBe(15);
  });
});

describe("reorderParameters", () => {
  it("moves the dragged parameter to the position it was dropped on", () => {
    expect(reorderParameters(params, "c", "a").map((p) => p.id)).toEqual(["c", "a", "b"]);
    expect(reorderParameters(params, "a", "c").map((p) => p.id)).toEqual(["b", "c", "a"]);
  });

  it("returns the same array reference for a no-op, so the caller can skip the write", () => {
    expect(reorderParameters(params, "b", "b")).toBe(params);
    expect(reorderParameters(params, "b", "zzz")).toBe(params);
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/EditFilterChips.test.tsx`

Expected: FAIL — `EditFilterChips.tsx` exports neither `FILTER_DRAG_ACTIVATION_PX` nor
`reorderParameters`, and there is no `reorder Region` button.

- [ ] **Step 3: Write the implementation**

Replace `frontend/src/components/chart/dashboard/EditFilterChips.tsx` with:

```tsx
import {
  DndContext, KeyboardSensor, PointerSensor, useSensor, useSensors, closestCenter,
  type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext, horizontalListSortingStrategy, useSortable, sortableKeyboardCoordinates,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { GripVertical } from "lucide-react";
import type { DashboardParameter } from "@/api/dashboards";
import { reorderIds } from "@/lib/reorderIds";

/** Metabase's own activation distance for this list (`ParametersList.tsx:51`,
 *  `useDndSensors({ distance: 15 })`). A chip is also a click target — it opens the filter's
 *  settings panel — so a drag must not start until the pointer has clearly committed. */
export const FILTER_DRAG_ACTIVATION_PX = 15;

/** Move `activeId` to the slot `overId` occupies. Same-reference no-op when the ids match or
 *  either is missing, so the caller can skip the draft write entirely. */
export function reorderParameters(
  parameters: DashboardParameter[], activeId: string, overId: string,
): DashboardParameter[] {
  const ids = parameters.map((p) => p.id);
  const next = reorderIds(ids, activeId, overId);
  if (next === ids) return parameters;
  const byId = new Map(parameters.map((p) => [p.id, p]));
  return next.map((id) => byId.get(id)!);
}

interface Props {
  parameters: DashboardParameter[];
  activeId: string | null;
  onSelect: (id: string) => void;
  onReorder: (next: DashboardParameter[]) => void;
}

function Chip({ param, active, onSelect }: {
  param: DashboardParameter; active: boolean; onSelect: () => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } =
    useSortable({ id: param.id });
  return (
    <div ref={setNodeRef}
      style={{ transform: CSS.Translate.toString(transform), transition, opacity: isDragging ? 0.6 : 1 }}
      className={`inline-flex h-8 items-center rounded-sm border pr-2 text-sm ${active ? "border-primary bg-secondary text-primary" : "bg-background hover:bg-accent/40"}`}>
      {/* The handle carries dnd-kit's keyboard listeners as well as its pointer ones, so the order
          can be changed without a mouse (focus, space to lift, arrows to move, space to drop).
          `size-6` is the 24×24 minimum in WCAG 2.2 SC 2.5.8 around a 14px glyph — the same shape
          `FieldZone`'s chip settled on. The label names the FILTER: a run of identical "reorder"
          buttons is unusable from a screen reader. */}
      <button type="button" aria-label={`reorder ${param.name}`}
        className="flex size-6 shrink-0 cursor-grab items-center justify-center rounded text-muted-foreground hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset"
        {...attributes} {...listeners}>
        <GripVertical className="h-3.5 w-3.5" />
      </button>
      <button type="button" aria-label={param.name} data-active={active}
        className="h-full" onClick={onSelect}>
        {param.name}{param.required ? " *" : ""}
      </button>
    </div>
  );
}

export function EditFilterChips({ parameters, activeId, onSelect, onReorder }: Props) {
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: FILTER_DRAG_ACTIVATION_PX } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );
  function onDragEnd(e: DragEndEvent) {
    const { active, over } = e;
    if (!over) return;
    const next = reorderParameters(parameters, String(active.id), String(over.id));
    if (next !== parameters) onReorder(next);
  }
  if (parameters.length === 0) return null;
  return (
    <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={onDragEnd}>
      <SortableContext items={parameters.map((p) => p.id)} strategy={horizontalListSortingStrategy}>
        <div className="mb-3 flex flex-wrap items-center gap-2">
          {parameters.map((p) => (
            <Chip key={p.id} param={p} active={p.id === activeId} onSelect={() => onSelect(p.id)} />
          ))}
        </div>
      </SortableContext>
    </DndContext>
  );
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/EditFilterChips.test.tsx`

Expected: PASS, 5 tests.

- [ ] **Step 5: Prove it survives mutation**

Change `reorderParameters` to `return parameters;`. Re-run. Expected: FAIL on both reorder cases.
Restore. Change `FILTER_DRAG_ACTIVATION_PX` to `8`. Re-run. Expected: FAIL. Restore.

- [ ] **Step 6: Wire it into the route**

In `frontend/src/routes/explore/DashboardView.tsx`, at the `EditFilterChips` call site (`:563`),
add the one new prop:

```tsx
        ? <EditFilterChips parameters={parameters} activeId={editPanel?.kind === "filter" ? editPanel.paramId : null}
            onSelect={(id) => openPanel({ kind: "filter", paramId: id })}
            onReorder={setDraftParameters} />
```

The draft array **is** the order: `onSave` (`:390`) already sends `parameters` straight into the PUT
body, and the server stores it as an opaque JSON array, so nothing else has to change to persist it.

- [ ] **Step 7: Run the gates and commit**

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
git add frontend/src/components/chart/dashboard/EditFilterChips.tsx \
        frontend/src/components/chart/dashboard/__tests__/EditFilterChips.test.tsx \
        frontend/src/routes/explore/DashboardView.tsx
git commit -m "feat(dashboard): drag-reorder filter chips in edit mode with a 15px activation distance"
```

---

## Task 4: Auto-connect a filter to every card with the same column

**Files:**
- Create: `frontend/src/components/chart/dashboard/autoConnect.ts`
- Modify: `frontend/src/routes/explore/DashboardView.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/autoConnect.test.ts`

Mapping one card offers to map every other card on the same tab that has a column of that name.
**It must be undoable** — it writes mappings across many cards at once, which is why Metabase pairs
its auto-connect toast with a follow-up undo toast (`toasts.ts:26-91`, §7 of the design).

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/autoConnect.test.ts`:

```ts
import { describe, it, expect, vi, beforeEach } from "vitest";
import {
  AUTO_CONNECT_TOAST_MS,
  AUTO_CONNECT_UNDO_TOAST_MS,
  applyAutoConnect,
  autoConnectDoneMessage,
  autoConnectPrompt,
  findAutoConnectTargets,
  mappableColumns,
  offerAutoConnect,
  revertAutoConnect,
  type AutoConnectCard,
} from "../autoConnect";

const card = (over: Partial<AutoConnectCard>): AutoConnectCard => ({
  id: "c1", card_type: "chart", options: {}, tabKey: "t1", ...over,
});

const cards: AutoConnectCard[] = [
  card({ id: "src" }),
  card({ id: "same-col" }),
  card({ id: "other-col" }),
  card({ id: "already", options: { paramMappings: { p1: "region" } } }),
  card({ id: "other-tab", tabKey: "t2" }),
  card({ id: "text", card_type: "text" }),
];

const columnsOf = (id: string) => (id === "other-col" ? ["country"] : ["region", "total"]);

const find = () =>
  findAutoConnectTargets({ cards, columnsOf, paramId: "p1", column: "region", sourceCardId: "src", tabKey: "t1" });

describe("findAutoConnectTargets", () => {
  it("takes chart cards on this tab that have the column and no mapping yet", () => {
    expect(find()).toEqual([{ cardId: "same-col", column: "region" }]);
  });

  it("excludes the source card even when it has the column", () => {
    expect(find().some((t) => t.cardId === "src")).toBe(false);
  });

  it("returns nothing when the column is not mappable anywhere else", () => {
    expect(findAutoConnectTargets({
      cards, columnsOf, paramId: "p1", column: "total", sourceCardId: "src", tabKey: "t1",
    })).toEqual([{ cardId: "same-col", column: "total" }]);
    expect(findAutoConnectTargets({
      cards, columnsOf, paramId: "p1", column: "nope", sourceCardId: "src", tabKey: "t1",
    })).toEqual([]);
  });
});

describe("applyAutoConnect / revertAutoConnect", () => {
  const targets = [{ cardId: "same-col", column: "region" }];

  it("writes the mapping without disturbing the card's other options", () => {
    const before = [card({ id: "same-col", options: { autoHeight: true, paramMappings: { p9: "x" } } })];
    const after = applyAutoConnect(before, "p1", targets);
    expect(after[0].options).toEqual({ autoHeight: true, paramMappings: { p9: "x", p1: "region" } });
    expect(before[0].options).toEqual({ autoHeight: true, paramMappings: { p9: "x" } });  // no mutation
  });

  it("undo removes exactly the mappings it added and nothing else", () => {
    const before = [card({ id: "same-col", options: { paramMappings: { p9: "x" } } })];
    const after = revertAutoConnect(applyAutoConnect(before, "p1", targets), "p1", targets);
    expect(after[0].options).toEqual({ paramMappings: { p9: "x" } });
  });

  it("undo leaves a card the user had mapped by hand alone", () => {
    // `targets` never contains an already-mapped card, so a blanket delete-by-param-id would
    // destroy a hand-made mapping that auto-connect never touched.
    const before = [card({ id: "already", options: { paramMappings: { p1: "region" } } })];
    expect(revertAutoConnect(before, "p1", targets)[0].options).toEqual({ paramMappings: { p1: "region" } });
  });
});

describe("mappableColumns", () => {
  const spec = { dimensions: ["region"], columns: ["city"], temporal_units: { created_at: "month" } };
  it("offers dimensions and columns, or the temporally grouped columns for a time-grouping filter", () => {
    expect(mappableColumns(spec, false)).toEqual(["region", "city"]);
    expect(mappableColumns(spec, true)).toEqual(["created_at"]);
    expect(mappableColumns({ dimensions: [], columns: [] }, true)).toEqual([]);
  });
});

describe("the toast copy", () => {
  it("matches Metabase's wording and timeouts", () => {
    // toasts.ts:41-43 and constants.ts:1-2.
    expect(autoConnectPrompt("Region", false)).toBe("Auto-connect this filter to all questions containing “Region”?");
    expect(autoConnectPrompt("Region", true)).toBe("Auto-connect this filter to all questions containing “Region”, in the current tab?");
    expect(autoConnectDoneMessage("Region")).toBe("The filter was auto-connected to all questions containing “Region”.");
    expect(AUTO_CONNECT_TOAST_MS).toBe(12000);
    expect(AUTO_CONNECT_UNDO_TOAST_MS).toBe(8000);
  });
});

vi.mock("sonner", () => ({ toast: vi.fn() }));
// eslint-disable-next-line import/first
import { toast } from "sonner";

describe("offerAutoConnect", () => {
  beforeEach(() => vi.mocked(toast).mockClear());

  it("offers the connection, and only after the user accepts does it connect and offer undo", () => {
    const connect = vi.fn();
    const revert = vi.fn();
    offerAutoConnect({
      targets: [{ cardId: "same-col", column: "region" }],
      columnName: "Region", hasMultipleTabs: false, connect, revert,
    });

    expect(connect).not.toHaveBeenCalled();          // nothing happens until the user asks
    const [msg, opts] = vi.mocked(toast).mock.calls[0];
    expect(msg).toBe("Auto-connect this filter to all questions containing “Region”?");
    expect(opts?.duration).toBe(AUTO_CONNECT_TOAST_MS);

    opts!.action!.onClick(new MouseEvent("click") as never);
    expect(connect).toHaveBeenCalledTimes(1);

    const [undoMsg, undoOpts] = vi.mocked(toast).mock.calls[1];
    expect(undoMsg).toBe("The filter was auto-connected to all questions containing “Region”.");
    expect(undoOpts?.duration).toBe(AUTO_CONNECT_UNDO_TOAST_MS);
    undoOpts!.action!.onClick(new MouseEvent("click") as never);
    expect(revert).toHaveBeenCalledTimes(1);
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/autoConnect.test.ts`

Expected: FAIL — `Failed to resolve import "../autoConnect"`.

- [ ] **Step 3: Write the implementation**

Create `frontend/src/components/chart/dashboard/autoConnect.ts`:

```ts
import { toast } from "sonner";

/** The shape auto-connect needs from a draft card. Deliberately structural rather than importing
 *  `DraftCard`: the module has to be usable from a test that never builds a dashboard. */
export interface AutoConnectCard {
  id: string;
  card_type: string;
  options: Record<string, unknown>;
  tabKey?: string | null;
}

export interface AutoConnectTarget {
  cardId: string;
  column: string;
}

/** Metabase's timeouts, verbatim (auto-wire-parameters/constants.ts:1-2). */
export const AUTO_CONNECT_TOAST_MS = 12000;
export const AUTO_CONNECT_UNDO_TOAST_MS = 8000;

/** toasts.ts:41-43. The curly quotes are Metabase's. */
export function autoConnectPrompt(columnName: string, hasMultipleTabs: boolean): string {
  return hasMultipleTabs
    ? `Auto-connect this filter to all questions containing “${columnName}”, in the current tab?`
    : `Auto-connect this filter to all questions containing “${columnName}”?`;
}

/** toasts.ts:78. */
export function autoConnectDoneMessage(columnName: string): string {
  return `The filter was auto-connected to all questions containing “${columnName}”.`;
}

/** The columns a card can offer a parameter — the same list the mapping overlay shows. A
 *  temporal-unit parameter re-grids an already-grouped column, so it can only be mapped to a
 *  column the saved spec already buckets. */
export function mappableColumns(
  spec: { dimensions: string[]; columns: string[]; temporal_units?: Record<string, string> },
  isTemporalUnit: boolean,
): string[] {
  if (isTemporalUnit) return Object.keys(spec.temporal_units ?? {});
  return [...spec.dimensions, ...spec.columns];
}

const mappingsOf = (c: AutoConnectCard): Record<string, string> =>
  (c.options.paramMappings as Record<string, string> | undefined) ?? {};

/**
 * The other cards this filter could be connected to. Ported from Metabase's
 * `getAllDashboardCardsWithUnmappedParameters` + `getMatchingParameterOption`:
 *
 *  - chart cards only (a text/heading/link card has no columns);
 *  - the SAME TAB only — Metabase's toast says "in the current tab" precisely because it never
 *    reaches across one;
 *  - never the card the user just mapped;
 *  - never a card that already has a mapping for this parameter, so an existing hand-made choice
 *    is not overwritten (this is also what makes the undo safe — see `revertAutoConnect`);
 *  - the column must be present by NAME on the card's chart.
 */
export function findAutoConnectTargets(args: {
  cards: AutoConnectCard[];
  columnsOf: (cardId: string) => string[];
  paramId: string;
  column: string;
  sourceCardId: string;
  tabKey: string | null;
}): AutoConnectTarget[] {
  const { cards, columnsOf, paramId, column, sourceCardId, tabKey } = args;
  return cards
    .filter((c) =>
      c.card_type === "chart" &&
      c.id !== sourceCardId &&
      (c.tabKey ?? null) === tabKey &&
      !mappingsOf(c)[paramId] &&
      columnsOf(c.id).includes(column))
    .map((c) => ({ cardId: c.id, column }));
}

/** The patch: each target gains `paramMappings[paramId]`. Returns a new array and new option
 *  objects — the draft setters compare by reference. */
export function applyAutoConnect<T extends AutoConnectCard>(
  cards: T[], paramId: string, targets: AutoConnectTarget[],
): T[] {
  const byId = new Map(targets.map((t) => [t.cardId, t.column]));
  return cards.map((c) => {
    const column = byId.get(c.id);
    if (!column) return c;
    return { ...c, options: { ...c.options, paramMappings: { ...mappingsOf(c), [paramId]: column } } };
  });
}

/** The exact inverse: remove the mapping from the cards in `targets` and nobody else. A blanket
 *  "delete this parameter's mapping everywhere" would also destroy the mapping the user made by
 *  hand on the source card, and any mapping auto-connect skipped because one was already there. */
export function revertAutoConnect<T extends AutoConnectCard>(
  cards: T[], paramId: string, targets: AutoConnectTarget[],
): T[] {
  const ids = new Set(targets.map((t) => t.cardId));
  return cards.map((c) => {
    if (!ids.has(c.id)) return c;
    const { [paramId]: _removed, ...rest } = mappingsOf(c);
    return { ...c, options: { ...c.options, paramMappings: rest } };
  });
}

/**
 * The two toasts. Metabase asks first and connects second — the toast is an OFFER, not a report —
 * and then immediately offers the undo, because the accepted action rewrote many cards at once.
 * Both halves are required; a connect without the undo is the failure mode §7 of the design calls
 * out by name.
 */
export function offerAutoConnect(opts: {
  targets: AutoConnectTarget[];
  columnName: string;
  hasMultipleTabs: boolean;
  connect: () => void;
  revert: () => void;
}): void {
  if (opts.targets.length === 0) return;
  toast(autoConnectPrompt(opts.columnName, opts.hasMultipleTabs), {
    duration: AUTO_CONNECT_TOAST_MS,
    action: {
      label: "Auto-connect",
      onClick: () => {
        opts.connect();
        toast(autoConnectDoneMessage(opts.columnName), {
          duration: AUTO_CONNECT_UNDO_TOAST_MS,
          action: { label: "Undo", onClick: () => opts.revert() },
        });
      },
    },
  });
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/autoConnect.test.ts`

Expected: PASS, 10 tests.

- [ ] **Step 5: Prove it survives mutation**

1. Drop `!mappingsOf(c)[paramId] &&` from the filter. Re-run. Expected: FAIL — `already` appears in
   the targets.
2. Restore. Drop `(c.tabKey ?? null) === tabKey &&`. Re-run. Expected: FAIL — `other-tab` appears.
3. Restore. Change `revertAutoConnect` to clear `paramMappings` entirely
   (`options: { ...c.options, paramMappings: {} }`). Re-run. Expected: FAIL — the `p9` mapping is
   gone.
4. Restore. In `offerAutoConnect`, call `opts.connect()` outside the `action.onClick`. Re-run.
   Expected: FAIL — `connect` was called before the user accepted.

- [ ] **Step 6: Wire it into the route, and shrink the overlay call site while you are there**

In `frontend/src/routes/explore/DashboardView.tsx`, add the import beside the other dashboard
imports:

```tsx
import { applyAutoConnect, findAutoConnectTargets, mappableColumns, offerAutoConnect } from "@/components/chart/dashboard/autoConnect";
```

Add one function immediately after `updateCardOptions` (`:331-336`):

```tsx
  /** Auto-connect: mapping ONE card offers to map every other card on this tab that has a column
   *  of the same name. Resolution, the patch, its inverse and both toasts live in autoConnect.ts;
   *  this is the wiring that gives them the draft. */
  function offerAutoConnectFor(sourceCardId: string, paramId: string, column: string) {
    const isTemporal = parameters.find((p) => p.id === paramId)?.type === "temporal_unit";
    const columnsOf = (cardId: string) => {
      const chart = chartById.get((cards as DraftCard[]).find((x) => x.id === cardId)?.chart_id ?? "");
      return chart ? mappableColumns(chart.params.spec, isTemporal) : [];
    };
    const targets = findAutoConnectTargets({
      cards: cards as DraftCard[], columnsOf, paramId, column, sourceCardId,
      tabKey: activeTab?.key ?? null,
    });
    offerAutoConnect({
      targets, columnName: column, hasMultipleTabs: tabs.length > 1,
      connect: () => setDraftCards((prev) => applyAutoConnect(prev ?? cardsAsDraft(), paramId, targets)),
      revert: () => setDraftCards((prev) => revertAutoConnect(prev ?? cardsAsDraft(), paramId, targets)),
    });
  }
```

(`revertAutoConnect` goes in the same import list.)

At the `CardFilterMapOverlay` call site (`:648-658`), replace the inline column expression with
`mappableColumns` — this **removes** five lines and stops the overlay and auto-connect from being
able to disagree about what a card offers — and fire the offer after a mapping is chosen:

```tsx
                {activeParamId && c.card_type === "chart" && baseChart && (
                  <CardFilterMapOverlay
                    columns={mappableColumns(
                      baseChart.params.spec,
                      parameters.find((pp) => pp.id === activeParamId)?.type === "temporal_unit",
                    )}
                    value={mappings[activeParamId] ?? ""}
                    onChange={(col) => {
                      updateCardOptions(c.id, { paramMappings: { ...mappings, [activeParamId]: col } });
                      if (col) offerAutoConnectFor(c.id, activeParamId, col);
                    }}
                  />
                )}
```

Net effect on `DashboardView.tsx`: +17 lines of wiring, −5 lines of duplicated column logic.

- [ ] **Step 7: Run the gates and commit**

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
git add frontend/src/components/chart/dashboard/autoConnect.ts \
        frontend/src/components/chart/dashboard/__tests__/autoConnect.test.ts \
        frontend/src/routes/explore/DashboardView.tsx
git commit -m "feat(dashboard): offer to auto-connect a filter to every card with the same column, undoably"
```

---

## Task 5: Auto-apply toggle and the Apply bar

**Files:**
- Create: `frontend/src/components/chart/dashboard/filterDrafts.ts`,
  `frontend/src/api/useFilterDrafts.ts`,
  `frontend/src/components/chart/dashboard/FilterApplyBar.tsx`
- Modify: `frontend/src/api/useDashboardParams.ts`,
  `frontend/src/components/chart/dashboard/DashboardFilterBar.tsx`,
  `frontend/src/routes/explore/DashboardView.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/filterDrafts.test.ts`,
  `frontend/src/api/__tests__/useFilterDrafts.test.tsx`,
  `frontend/src/components/chart/dashboard/__tests__/FilterApplyBar.test.tsx`

**Read "Auto-apply has no typed home" above before starting.** The toggle is a per-browser
preference in this workstream, and the control says so. Do not add a column.

- [ ] **Step 1: Write the failing test for the change counter**

Create `frontend/src/components/chart/dashboard/__tests__/filterDrafts.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { countChangedFilters, describeFilterChanges } from "../filterDrafts";

describe("countChangedFilters", () => {
  it("counts additions, removals and edits, and ignores untouched values", () => {
    expect(countChangedFilters({ a: "x" }, { a: "x" })).toBe(0);
    expect(countChangedFilters({}, { a: "x" })).toBe(1);                       // added
    expect(countChangedFilters({ a: "x" }, { a: "" })).toBe(1);                // removed
    expect(countChangedFilters({ a: "x" }, { a: "y" })).toBe(1);               // edited
    expect(countChangedFilters({ a: "x", b: "1" }, { a: "y", b: "2" })).toBe(2);
  });

  it("compares arrays by content, not by reference", () => {
    expect(countChangedFilters({ a: ["x", "y"] }, { a: ["x", "y"] })).toBe(0);
    expect(countChangedFilters({ a: ["x"] }, { a: ["x", "y"] })).toBe(1);
  });

  it("treats every shape of empty as the same state", () => {
    // "" / undefined / [] are one state here, because `setValue` deletes all three from the URL —
    // reporting a change the URL cannot represent would leave an Apply button that does nothing.
    expect(countChangedFilters({ a: "" }, {})).toBe(0);
    expect(countChangedFilters({ a: [] }, { a: undefined })).toBe(0);
  });
});

describe("describeFilterChanges", () => {
  it("is singular at one and plural otherwise", () => {
    expect(describeFilterChanges(1)).toBe("1 filter changed");
    expect(describeFilterChanges(3)).toBe("3 filters changed");
  });
});
```

- [ ] **Step 2: Run it, see it fail, write the module**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/filterDrafts.test.ts`
→ FAIL, `Failed to resolve import "../filterDrafts"`.

Create `frontend/src/components/chart/dashboard/filterDrafts.ts`:

```ts
/** A value the URL cannot hold. `useDashboardParams.setValue` deletes exactly these, so all three
 *  spellings are one state — Metabase's `isParameterValueEmpty` (FilterApplyToast/utils.ts) omits
 *  `""` because its parameter values live in Redux, not the query string. */
const isEmpty = (v: unknown) =>
  v === undefined || v === null || v === "" || (Array.isArray(v) && v.length === 0);

/** How many filters differ between what the cards are showing and what the user has drafted.
 *  Ported from Metabase's `getFilterChangeDescription` counting loop. Values are compared by
 *  serialisation rather than by reference: a drafted array is a new array every keystroke. */
export function countChangedFilters(
  applied: Record<string, unknown>,
  draft: Record<string, unknown>,
): number {
  let changed = 0;
  for (const slug of new Set([...Object.keys(applied), ...Object.keys(draft)])) {
    const a = applied[slug];
    const d = draft[slug];
    const ae = isEmpty(a);
    const de = isEmpty(d);
    if (ae && de) continue;
    if (ae !== de) { changed++; continue; }
    if (JSON.stringify(a) !== JSON.stringify(d)) changed++;
  }
  return changed;
}

/** Metabase's `ngettext(msgid`${n} filter changed`, `${n} filters changed`, n)`. */
export function describeFilterChanges(count: number): string {
  return count === 1 ? "1 filter changed" : `${count} filters changed`;
}
```

Re-run: PASS, 5 tests. Then mutate: change `if (ae !== de) { changed++; continue; }` to
`continue;`. Re-run. Expected: FAIL on additions and removals. Restore.

- [ ] **Step 3: Give `useDashboardParams` a batched write**

Apply writes several filters at once. Calling `setValue` in a loop queues several updater functions
against the same location and the last one wins — the Apply button would drop every filter but one.

In `frontend/src/api/useDashboardParams.ts`, replace `setValue` with:

```ts
  /** Write several parameters in ONE URL update. `setValue` in a loop does NOT work: each call
   *  reads the same current location, so the last write wins and the rest vanish. Apply writes
   *  every drafted filter at once, which is exactly that case. */
  const setValues = (patch: Record<string, unknown>) =>
    setParams((prev) => {
      const n = new URLSearchParams(prev);
      for (const [slug, value] of Object.entries(patch)) {
        const blank = value === undefined || value === null || value === "" || (Array.isArray(value) && value.length === 0);
        if (blank) n.delete(slug);
        else n.set(slug, typeof value === "object" ? JSON.stringify(value) : String(value));
      }
      return n;
    }, { replace: true });
  const setValue = (slug: string, value: unknown) => setValues({ [slug]: value });
  return { values, setValue, setValues };
```

and extend the interface:

```ts
export interface DashboardParamsState {
  values: Record<string, unknown>;
  setValue: (slug: string, value: unknown) => void;
  setValues: (patch: Record<string, unknown>) => void;
}
```

Add to `frontend/src/api/__tests__/useDashboardParams.test.tsx`:

```tsx
  it("setValues writes every key in one update", () => {
    // Two setValue calls in one tick would leave only the last: each reads the same location.
    const { result } = renderHook(() => useDashboardParams(params), { wrapper: wrap("/d") });
    act(() => result.current.setValues({ region: "APAC", city: ["Hà Nội"] }));
    expect(result.current.values.region).toBe("APAC");
    expect(result.current.values.city).toEqual(["Hà Nội"]);
  });
```

Mutation: implement `setValues` as `Object.entries(patch).forEach(([k, v]) => setValue(k, v))`.
Re-run. Expected: FAIL — one of the two keys is missing.

- [ ] **Step 4: Write the draft hook and its failing test**

Create `frontend/src/api/__tests__/useFilterDrafts.test.tsx`:

```tsx
import { describe, it, expect, beforeEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import type { ReactNode } from "react";
import { useDashboardParams } from "../useDashboardParams";
import { useFilterDrafts } from "../useFilterDrafts";
import type { DashboardParameter } from "@/api/dashboards";

const params: DashboardParameter[] = [
  { id: "p1", name: "Region", slug: "region", type: "text", operator: "is" },
  { id: "p2", name: "Owner", slug: "owner", type: "text", operator: "is" },
];
const wrap = (initial: string) => ({ children }: { children: ReactNode }) =>
  <MemoryRouter initialEntries={[initial]}>{children}</MemoryRouter>;

const setup = (initial = "/d") =>
  renderHook(() => useFilterDrafts("dash-1", useDashboardParams(params)), { wrapper: wrap(initial) });

beforeEach(() => localStorage.clear());

describe("useFilterDrafts with auto-apply on (the default)", () => {
  it("writes straight through, so the cards see the change immediately", () => {
    const { result } = setup();
    expect(result.current.autoApply).toBe(true);
    act(() => result.current.setValue("region", "EU"));
    expect(result.current.appliedValues.region).toBe("EU");
    expect(result.current.changedCount).toBe(0);
  });
});

describe("useFilterDrafts with auto-apply off", () => {
  it("holds the change out of appliedValues until Apply", () => {
    const { result } = setup();
    act(() => result.current.setAutoApply(false));
    act(() => result.current.setValue("region", "EU"));

    // THE ASSERTION THAT MATTERS: the cards read appliedValues, and it must not have moved.
    expect(result.current.appliedValues.region).toBeUndefined();
    expect(result.current.editorValues.region).toBe("EU");
    expect(result.current.changedCount).toBe(1);

    act(() => result.current.apply());
    expect(result.current.appliedValues.region).toBe("EU");
    expect(result.current.changedCount).toBe(0);
  });

  it("applies every drafted filter, not just the last one", () => {
    const { result } = setup();
    act(() => result.current.setAutoApply(false));
    act(() => { result.current.setValue("region", "EU"); });
    act(() => { result.current.setValue("owner", "ana"); });
    expect(result.current.changedCount).toBe(2);
    act(() => result.current.apply());
    expect(result.current.appliedValues).toMatchObject({ region: "EU", owner: "ana" });
  });

  it("Cancel drops the draft and leaves the applied values alone", () => {
    const { result } = setup("/d?region=EU");
    act(() => result.current.setAutoApply(false));
    act(() => result.current.setValue("region", "US"));
    act(() => result.current.cancel());
    expect(result.current.editorValues.region).toBe("EU");
    expect(result.current.changedCount).toBe(0);
  });

  it("turning auto-apply back on applies the pending draft rather than stranding it", () => {
    const { result } = setup();
    act(() => result.current.setAutoApply(false));
    act(() => result.current.setValue("region", "EU"));
    act(() => result.current.setAutoApply(true));
    expect(result.current.appliedValues.region).toBe("EU");
    expect(result.current.changedCount).toBe(0);
  });
});

describe("the auto-apply preference", () => {
  it("persists per dashboard, and defaults to on", () => {
    const { result } = setup();
    act(() => result.current.setAutoApply(false));
    expect(localStorage.getItem("egret:dash-auto-apply:dash-1")).toBe("0");
    expect(setup().result.current.autoApply).toBe(false);
    localStorage.clear();
    expect(setup().result.current.autoApply).toBe(true);
  });
});
```

Run: `cd frontend && npx vitest run src/api/__tests__/useFilterDrafts.test.tsx` → FAIL,
`Failed to resolve import "../useFilterDrafts"`.

Create `frontend/src/api/useFilterDrafts.ts`:

```ts
import { useState } from "react";
import { countChangedFilters } from "@/components/chart/dashboard/filterDrafts";
import type { DashboardParamsState } from "./useDashboardParams";

export interface FilterDraftsState {
  /** What the CARDS run on. Never moves while auto-apply is off and a draft is pending. */
  appliedValues: Record<string, unknown>;
  /** What the FILTER BAR shows — applied values with the draft laid over them. */
  editorValues: Record<string, unknown>;
  setValue: (slug: string, value: unknown) => void;
  changedCount: number;
  apply: () => void;
  cancel: () => void;
  autoApply: boolean;
  setAutoApply: (v: boolean) => void;
}

/** Per-user, per-dashboard, in this browser.
 *
 *  Metabase stores `auto_apply_filters` as a column on the dashboard, shared by every viewer.
 *  egret's dashboard PUT has no field for it and the `dashboards` table has no settings column
 *  (`UpdateDashboard` is name/description/parameters/width/tabs/cards), so a shared setting means
 *  a migration. Rather than smuggle a column into a frontend-only workstream — or hide a scalar
 *  inside the `parameters` array, which is typed as a list of filters and mapped over in eight
 *  places — this is a browser preference, and the switch is labelled as one. */
const key = (dashboardId: string) => `egret:dash-auto-apply:${dashboardId}`;

function readPreference(dashboardId: string): boolean {
  try { return localStorage.getItem(key(dashboardId)) !== "0"; } catch { return true; }
}

export function useFilterDrafts(
  dashboardId: string, params: DashboardParamsState,
): FilterDraftsState {
  const [autoApply, setAutoApplyState] = useState(() => readPreference(dashboardId));
  // null = nothing drafted. An empty object is a different state: a draft that has been emptied.
  const [draft, setDraft] = useState<Record<string, unknown> | null>(null);

  const editorValues = draft === null ? params.values : { ...params.values, ...draft };

  const setValue = (slug: string, value: unknown) => {
    if (autoApply) { params.setValue(slug, value); return; }
    setDraft((prev) => ({ ...(prev ?? {}), [slug]: value }));
  };

  const apply = () => {
    if (draft) params.setValues(draft);   // ONE URL write — see setValues
    setDraft(null);
  };

  const cancel = () => setDraft(null);

  const setAutoApply = (v: boolean) => {
    try { localStorage.setItem(key(dashboardId), v ? "1" : "0"); } catch { /* private mode */ }
    setAutoApplyState(v);
    // Switching auto-apply on removes the only control that could commit a pending draft, so
    // commit it here rather than stranding a change the user can no longer reach.
    if (v && draft) { params.setValues(draft); setDraft(null); }
  };

  return {
    appliedValues: params.values,
    editorValues,
    setValue,
    changedCount: countChangedFilters(params.values, editorValues),
    apply,
    cancel,
    autoApply,
    setAutoApply,
  };
}
```

Run the test again: PASS, 7 tests.

- [ ] **Step 5: Prove the hook survives mutation**

1. Change `appliedValues` to `editorValues`. Re-run. Expected: FAIL — the draft-holding test.
2. Restore. Change `apply` to `params.setValue(...)` over the draft's first key only. Re-run.
   Expected: FAIL — "applies every drafted filter".
3. Restore. Delete the `if (v && draft)` line in `setAutoApply`. Re-run. Expected: FAIL — the
   strand test.

- [ ] **Step 6: Build the bar and its failing test**

Create `frontend/src/components/chart/dashboard/__tests__/FilterApplyBar.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FilterApplyBar } from "../FilterApplyBar";

describe("FilterApplyBar", () => {
  it("stays out of the way while auto-apply is on", () => {
    render(<FilterApplyBar autoApply changedCount={2} onApply={vi.fn()} onCancel={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "Apply" })).toBeNull();
  });

  it("stays out of the way when nothing has changed", () => {
    render(<FilterApplyBar autoApply={false} changedCount={0} onApply={vi.fn()} onCancel={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "Apply" })).toBeNull();
  });

  it("counts the changes and wires both buttons", () => {
    const onApply = vi.fn();
    const onCancel = vi.fn();
    render(<FilterApplyBar autoApply={false} changedCount={2} onApply={onApply} onCancel={onCancel} />);
    expect(screen.getByText("2 filters changed")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(onApply).toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onCancel).toHaveBeenCalled();
  });
});
```

Create `frontend/src/components/chart/dashboard/FilterApplyBar.tsx`:

```tsx
import { Button } from "@/components/ui/button";
import { describeFilterChanges } from "./filterDrafts";

interface Props {
  autoApply: boolean;
  changedCount: number;
  onApply: () => void;
  onCancel: () => void;
}

/** The draft-filters bar, bottom-centre. Metabase's `FilterApplyToast` renders on exactly these
 *  conditions (`FilterApplyToast.tsx:44-46`: auto-apply off, something unapplied, not editing) —
 *  the editing case is handled by the caller, which only renders the view-mode filter bar. */
export function FilterApplyBar({ autoApply, changedCount, onApply, onCancel }: Props) {
  if (autoApply || changedCount === 0) return null;
  return (
    <div role="status"
      className="fixed bottom-5 left-1/2 z-40 flex -translate-x-1/2 items-center gap-3 rounded-md border bg-card px-4 py-2 shadow-lg">
      <span className="text-sm">{describeFilterChanges(changedCount)}</span>
      <Button size="sm" variant="ghost" onClick={onCancel}>Cancel</Button>
      <Button size="sm" onClick={onApply}>Apply</Button>
    </div>
  );
}
```

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/FilterApplyBar.test.tsx`
→ PASS, 3 tests. Mutation: change the guard to `if (changedCount === 0) return null;`. Re-run.
Expected: FAIL on the auto-apply test. Restore.

- [ ] **Step 7: Host the switch and the bar in the filter bar**

In `frontend/src/components/chart/dashboard/DashboardFilterBar.tsx`:

```tsx
import type { DashboardParameter } from "@/api/dashboards";
import { Switch } from "@/components/ui/switch";
import { FilterPill } from "./FilterPill";
import { FilterApplyBar } from "./FilterApplyBar";
import { isBlank } from "./params";

interface Props {
  parameters: DashboardParameter[];
  values: Record<string, unknown>;
  onChange: (slug: string, value: unknown) => void;
  optionsBySlug: Record<string, string[]>;
  /** Auto-apply defaults to ON, which is exactly today's behaviour — so a caller that has not been
   *  updated yet behaves as it always did. (Contrast W1's breakpoint props, which are required
   *  because forgetting one silently destroys a layout; forgetting these changes nothing.) */
  autoApply?: boolean;
  onAutoApplyChange?: (v: boolean) => void;
  changedCount?: number;
  onApply?: () => void;
  onCancel?: () => void;
}
```

and, inside the component, after the pill row `</div>`:

```tsx
        {onAutoApplyChange && (
          <label className="ml-auto flex items-center gap-2 text-xs text-muted-foreground"
            title="Saved in this browser, for this dashboard">
            <span>Auto-apply filters</span>
            <Switch aria-label="auto-apply filters" checked={autoApply}
              onCheckedChange={(v) => onAutoApplyChange(v === true)} />
          </label>
        )}
```

(put the `<label>` inside the existing `flex flex-wrap items-center gap-2` row, so `ml-auto` pushes
it to the trailing edge), and after that row:

```tsx
      <FilterApplyBar autoApply={autoApply} changedCount={changedCount ?? 0}
        onApply={onApply ?? (() => {})} onCancel={onCancel ?? (() => {})} />
```

with `autoApply = true, changedCount = 0` defaulted in the destructuring.

- [ ] **Step 8: Wire the route — substitutions only**

In `frontend/src/routes/explore/DashboardView.tsx`, after the `paramsState` line (`:87`):

```tsx
  const filters = useFilterDrafts(id, paramsState);
```

with `import { useFilterDrafts } from "@/api/useFilterDrafts";` beside the `useDashboardParams`
import. Then replace, one for one — the cards read **applied** values, the bar reads **editor**
values:

| Line | Was | Becomes |
|---|---|---|
| `:194` | `paramsState.values[p.slug]` | `filters.appliedValues[p.slug]` |
| `:224` | `paramsState.values` | `filters.appliedValues` |
| `:227` | `paramsState.values` | `filters.appliedValues` |
| `:233` | `paramsState.values` | `filters.appliedValues` |
| `:236` | `paramsState.setValue(s.slug, s.value)` | `filters.setValue(s.slug, s.value)` |
| `:537` | `paramsState.values` | `filters.appliedValues` |
| `:612` | `paramsState.values` | `filters.appliedValues` |
| `:639` | `paramValues={paramsState.values}` | `paramValues={filters.appliedValues}` |
| `:754` | `paramsState.setValue(target.slug, f.value)` | `filters.setValue(target.slug, f.value)` |

and the filter bar itself (`:564`):

```tsx
        : <DashboardFilterBar parameters={parameters} values={filters.editorValues}
            onChange={filters.setValue} optionsBySlug={optionsBySlug}
            autoApply={filters.autoApply} onAutoApplyChange={filters.setAutoApply}
            changedCount={filters.changedCount} onApply={filters.apply} onCancel={filters.cancel} />
```

Net: `DashboardView.tsx` gains **one** line.

- [ ] **Step 9: Run the gates and commit**

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
git add frontend/src/components/chart/dashboard/filterDrafts.ts \
        frontend/src/components/chart/dashboard/FilterApplyBar.tsx \
        frontend/src/api/useFilterDrafts.ts \
        frontend/src/api/useDashboardParams.ts \
        frontend/src/components/chart/dashboard/DashboardFilterBar.tsx \
        frontend/src/routes/explore/DashboardView.tsx \
        frontend/src/components/chart/dashboard/__tests__/filterDrafts.test.ts \
        frontend/src/components/chart/dashboard/__tests__/FilterApplyBar.test.tsx \
        frontend/src/api/__tests__/useFilterDrafts.test.tsx \
        frontend/src/api/__tests__/useDashboardParams.test.tsx
git commit -m "feat(dashboard): optional draft filters with an Apply bar, and a batched URL write"
```

---

## Task 6: The linked-filters panel says why it is empty

**Files:**
- Create: `frontend/src/components/chart/dashboard/linkedFilters.ts`
- Modify: `frontend/src/components/chart/dashboard/FilterSettingsPanel.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/linkedFilters.test.ts`, plus additions to
  `FilterSettingsPanel.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/linkedFilters.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { linkedFilterState } from "../linkedFilters";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "1", name: "Region", slug: "region", type: "category", operator: "is", ...over });
const other = p({ id: "2", name: "City", slug: "city" });
const timeGrouping = p({ id: "3", name: "By", slug: "by", type: "temporal_unit" });

describe("linkedFilterState", () => {
  it("lists the other parameters that could constrain this one", () => {
    expect(linkedFilterState(p({}), [other])).toEqual({ kind: "list", usable: [other] });
  });

  it("reports no-usable when every other parameter is a time grouping", () => {
    // A temporal_unit param re-grids a column; it holds no value that could narrow a list.
    expect(linkedFilterState(p({}), [timeGrouping])).toEqual({ kind: "no-usable" });
    expect(linkedFilterState(p({}), [])).toEqual({ kind: "no-usable" });
  });

  it("reports custom-source when the values do not come from the data", () => {
    expect(linkedFilterState(p({ valuesSource: { type: "static", static: ["EU"] } }), [other]).kind).toBe("custom-source");
    expect(linkedFilterState(p({ valuesSource: { type: "card", cardId: "c", column: "x" } }), [other]).kind).toBe("custom-source");
    expect(linkedFilterState(p({ valuesSource: { type: "distinct" } }), [other]).kind).toBe("list");
  });

  it("reports input-widget for a free-text widget", () => {
    expect(linkedFilterState(p({ widget: "input" }), [other]).kind).toBe("input-widget");
    expect(linkedFilterState(p({ widget: "search" }), [other]).kind).toBe("list");
  });

  it("keeps Metabase's precedence: no-usable, then custom source, then widget", () => {
    // ParameterLinkedFilters.tsx:92-103 checks in exactly this order, and a parameter can be in
    // more than one of these states at once.
    const both = p({ widget: "input", valuesSource: { type: "static", static: [] } });
    expect(linkedFilterState(both, [other]).kind).toBe("custom-source");
    expect(linkedFilterState(both, [timeGrouping]).kind).toBe("no-usable");
  });
});
```

- [ ] **Step 2: Run it, see it fail, write the module**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/linkedFilters.test.ts`
→ FAIL, `Failed to resolve import "../linkedFilters"`.

Create `frontend/src/components/chart/dashboard/linkedFilters.ts`:

```ts
import type { DashboardParameter } from "@/api/dashboards";

export type LinkedFilterState =
  | { kind: "list"; usable: DashboardParameter[] }
  | { kind: "no-usable" }
  | { kind: "custom-source" }
  | { kind: "input-widget" };

/** Metabase's `usableAsLinkedFilter` (parameters/utils/linked-filters.ts) admits every
 *  FIELD_FILTER_PARAMETER_TYPES entry — date, string, number, boolean, id, category, location.
 *  egret's set is the same minus `temporal_unit`, which has no counterpart there: it re-grids a
 *  column rather than holding a value, so it can never narrow another filter's option list. */
function usableAsLinkedFilter(param: DashboardParameter): boolean {
  return param.type !== "temporal_unit";
}

/**
 * Which of the four things the linked-filters section should show, in Metabase's order
 * (`ParameterLinkedFilters.tsx:92-103`). The order is load-bearing: a parameter can be both
 * custom-sourced and an input box, and Metabase names the source first.
 */
export function linkedFilterState(
  param: DashboardParameter,
  others: DashboardParameter[],
): LinkedFilterState {
  const usable = others.filter(usableAsLinkedFilter);
  if (usable.length === 0) return { kind: "no-usable" };
  const source = param.valuesSource?.type;
  if (source === "static" || source === "card") return { kind: "custom-source" };
  if ((param.widget ?? "dropdown") === "input") return { kind: "input-widget" };
  return { kind: "list", usable };
}
```

Re-run: PASS, 5 tests.

- [ ] **Step 3: Prove it survives mutation**

Swap the `custom-source` and `input-widget` checks. Re-run. Expected: FAIL on the precedence test.
Restore. Change `usableAsLinkedFilter` to `return true;`. Re-run. Expected: FAIL on the
time-grouping case. Restore.

- [ ] **Step 4: Render the four states**

In `frontend/src/components/chart/dashboard/FilterSettingsPanel.tsx`, add the import:

```tsx
import { linkedFilterState } from "./linkedFilters";
```

replace `const others = …` / `const linked = …` (`:24-27`) with:

```tsx
  const others = allParameters.filter((o) => o.id !== param.id);
  const linkState = linkedFilterState(param, others);
  const linked = param.filteringParameters ?? [];
  const toggleLink = (id: string) =>
    onChange({ ...param, filteringParameters: linked.includes(id) ? linked.filter((x) => x !== id) : [...linked, id] });
```

and replace the whole `{others.length > 0 && ( … )}` block (`:107-130`) with:

```tsx
      {/* Metabase's framing, verbatim (ParameterLinkedFilters.tsx:52,93-103,120-127,143-160). The
          three non-list states used to render as NOTHING AT ALL, because the whole section sat
          behind `others.length > 0` — a user whose only other filter was a time grouping, or who
          had put this filter on a custom value list, saw a panel that simply stopped. */}
      <div className="flex flex-col gap-1">
        <div className="text-xs font-medium">Limit this filter's choices</div>

        {linkState.kind === "no-usable" && (
          <p className="text-xs text-muted-foreground">
            If you have another dashboard filter, you can limit the choices that are listed for this
            filter based on the selection of the other one. So first, add another dashboard filter.
          </p>
        )}

        {linkState.kind === "custom-source" && (
          <p className="text-xs text-muted-foreground">
            If the filter has values that are from another question or model, or a custom list, then
            this filter can't be limited by another dashboard filter.
          </p>
        )}

        {linkState.kind === "input-widget" && (
          <p className="text-xs text-muted-foreground">
            This filter can't be limited by another dashboard filter because its widget type is an
            input box.
          </p>
        )}

        {linkState.kind === "list" && (
          <>
            <p className="text-xs text-muted-foreground/70">
              If you toggle on one of these dashboard filters, selecting a value for that filter will
              limit the available choices for <strong>this</strong> filter.
            </p>
            {/* THE DENSEST LIST THIS CHANGE TOUCHES, and the one worth measuring rather than
                assuming. #443 sized the primitive's transparent hit area at 24×24 against the
                permission picker's 26px pitch, which leaves 2px between neighbouring targets.
                These rows are `text-sm` (a 20px line box) in a `gap-1` column: a 24px pitch, so
                the 24px targets ABUT exactly — 0px between, and still no overlap, so no click
                can land on the wrong filter. Measured in the browser at 24.00px pitch with
                neighbouring target boxes sharing an edge and never crossing it.

                `min-h-8` was considered and NOT applied. It would give a 36px pitch and a
                comfortable gap, but it also grows a list that is already one of several stacked
                in a 255px side panel, and #441's argument for 32px rows was that THE ROW BECOMES
                THE TARGET — which it already is here, because the `<label>` wraps the control and
                spans the row. Height would buy nothing this list does not have. */}
            {linkState.usable.map((o) => (
              <label key={o.id} htmlFor={`fsp-link-${o.id}`} className="flex cursor-pointer items-center gap-2 text-sm">
                <Checkbox id={`fsp-link-${o.id}`} aria-label={`link ${o.name}`} checked={linked.includes(o.id)} onCheckedChange={() => toggleLink(o.id)} /> {o.name}
              </label>
            ))}
          </>
        )}
      </div>
```

- [ ] **Step 5: Write the panel tests**

Add to `frontend/src/components/chart/dashboard/__tests__/FilterSettingsPanel.test.tsx`:

```tsx
  it("uses Metabase's heading and explains an empty linked-filters list", () => {
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} onChange={vi.fn()} onDelete={vi.fn()} />);
    expect(screen.getByText("Limit this filter's choices")).toBeTruthy();
    expect(screen.getByText(/So first, add another dashboard filter/)).toBeTruthy();
  });

  it("says why a custom-sourced filter cannot be linked, instead of showing an empty list", () => {
    const other = p({ id: "2", name: "City", slug: "city" });
    render(<FilterSettingsPanel param={p({ valuesSource: { type: "static", static: ["EU"] } })}
      allParameters={[p({}), other]} onChange={vi.fn()} onDelete={vi.fn()} />);
    expect(screen.getByText(/can't be limited by another dashboard filter/)).toBeTruthy();
    expect(screen.queryByLabelText("link City")).toBeNull();
  });

  it("says why an input-box filter cannot be linked", () => {
    const other = p({ id: "2", name: "City", slug: "city" });
    render(<FilterSettingsPanel param={p({ widget: "input" })} allParameters={[p({}), other]}
      onChange={vi.fn()} onDelete={vi.fn()} />);
    expect(screen.getByText(/its widget type is an input box/)).toBeTruthy();
  });
```

The existing "linked-filters checkbox writes filteringParameters with other param ids" and
"does not offer the param itself as a linked filter" tests must pass unchanged — the list state is
the default for a plain category parameter.

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/FilterSettingsPanel.test.tsx`
→ PASS.

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
git add frontend/src/components/chart/dashboard/linkedFilters.ts \
        frontend/src/components/chart/dashboard/__tests__/linkedFilters.test.ts \
        frontend/src/components/chart/dashboard/FilterSettingsPanel.tsx \
        frontend/src/components/chart/dashboard/__tests__/FilterSettingsPanel.test.tsx
git commit -m "feat(dashboard): give the linked-filters panel Metabase's framing and its three blocked states"
```

---

## Task 7: A typed default-value widget

**Files:**
- Create: `frontend/src/components/chart/dashboard/DefaultValueField.tsx`
- Modify: `frontend/src/components/chart/dashboard/FilterPill.tsx` (export `FilterControl`),
  `frontend/src/components/chart/dashboard/FilterSettingsPanel.tsx`,
  `frontend/src/components/chart/dashboard/EditSidebar.tsx`,
  `frontend/src/routes/explore/DashboardView.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/DefaultValueField.test.tsx`, plus edits to
  two existing tests in `FilterSettingsPanel.test.tsx`

The default is a **parameter value**, so it is picked with the parameter's own widget. Metabase says
this structurally: `ParameterSettings.tsx:253-258` renders the same `ParameterValueWidget` the filter
bar uses. egret already has that widget — `FilterControl` inside `FilterPill.tsx` — it is simply not
exported. Export it rather than growing a second implementation that will drift.

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/DefaultValueField.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { DefaultValueField } from "../DefaultValueField";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "1", name: "Region", slug: "region", type: "category", operator: "is", ...over });

describe("DefaultValueField", () => {
  it("picks a category default from the value list, and stores the RAW array", () => {
    // The raw/display trap: a test that asserted the visible text "EU" would pass against an
    // implementation storing the string "EU" — which is not what a multi-value category holds,
    // and not what `paramToFilters` turns into `in [...]`.
    const onChange = vi.fn();
    render(<DefaultValueField param={p({})} options={["EU", "US"]} onChange={onChange} />);
    fireEvent.click(screen.getByLabelText("option EU"));
    expect(onChange).toHaveBeenCalledWith(["EU"]);
  });

  it("picks a date default with the date picker, not a text box", () => {
    render(<DefaultValueField param={p({ type: "date", operator: "all-options" })} options={[]} onChange={vi.fn()} />);
    expect(screen.getByLabelText("Relative")).toBeTruthy();
    expect(screen.getByLabelText("Specific")).toBeTruthy();
  });

  it("picks a number default with a number input", () => {
    const onChange = vi.fn();
    render(<DefaultValueField param={p({ type: "number", operator: "=" })} options={[]} onChange={vi.fn()} />);
    expect((screen.getByLabelText("Region value") as HTMLInputElement).type).toBe("number");
    void onChange;
  });

  it("clears to undefined rather than to an empty string", () => {
    // `undefined` is what `useDashboardParams` reads as "no default"; "" would be a default that
    // exists and is blank, and the required-filter notice keys on the difference.
    const onChange = vi.fn();
    render(<DefaultValueField param={p({ default: ["EU"] })} options={["EU"]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "clear default value" }));
    expect(onChange).toHaveBeenCalledWith(undefined);
  });

  it("offers no clear button when there is no default to clear", () => {
    render(<DefaultValueField param={p({})} options={["EU"]} onChange={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "clear default value" })).toBeNull();
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/DefaultValueField.test.tsx`

Expected: FAIL — `Failed to resolve import "../DefaultValueField"`.

- [ ] **Step 3: Export `FilterControl`**

In `frontend/src/components/chart/dashboard/FilterPill.tsx`, change the declaration at `:73`:

```tsx
/** The value control for one parameter, dispatching on type and operator. Exported because the
 *  DEFAULT value is a parameter value too, and the settings panel must offer the same widget the
 *  viewer will use — the structure Metabase gets from sharing `ParameterValueWidget`. */
export function FilterControl({ param, value, options, onChange }: Props) {
```

- [ ] **Step 4: Write the implementation**

Create `frontend/src/components/chart/dashboard/DefaultValueField.tsx`:

```tsx
import type { DashboardParameter } from "@/api/dashboards";
import { FilterControl } from "./FilterPill";

interface Props {
  param: DashboardParameter;
  /** The parameter's option list, when it has one — the same `optionsBySlug` entry the filter bar
   *  is given, so a category default is chosen from the real values rather than typed. */
  options: string[];
  onChange: (next: unknown) => void;
}

/** Nothing the URL can hold. `useDashboardParams` only seeds `values` when `default !== undefined`,
 *  so an emptied default must collapse to `undefined` — "" would be a default that exists and is
 *  blank, which the required-filter notice treats differently. */
function isEmptyDefault(v: unknown): boolean {
  return v === undefined || v === null || v === "" || (Array.isArray(v) && v.filter((x) => x !== "" && x != null).length === 0);
}

/** The default value, picked with the parameter's own widget: a date with the date picker, a
 *  category from its value list, a number with a number input. Replaces the plain text box that
 *  stored every default as a string and could not round-trip a multi-value or range default. */
export function DefaultValueField({ param, options, onChange }: Props) {
  const hasDefault = !isEmptyDefault(param.default);
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center justify-between text-xs text-muted-foreground">
        <span>Default value</span>
        {hasDefault && (
          <button type="button" aria-label="clear default value"
            className="text-xs text-muted-foreground hover:text-foreground"
            onClick={() => onChange(undefined)}>Clear</button>
        )}
      </div>
      <div className="rounded border p-2">
        <FilterControl param={param} value={param.default} options={options}
          onChange={(v) => onChange(isEmptyDefault(v) ? undefined : v)} />
      </div>
    </div>
  );
}
```

- [ ] **Step 5: Use it in the settings panel**

In `frontend/src/components/chart/dashboard/FilterSettingsPanel.tsx`, add the import and a prop, and
replace the block at `:100-105`:

```tsx
import { DefaultValueField } from "./DefaultValueField";
```

```ts
interface Props {
  param: DashboardParameter;
  allParameters: DashboardParameter[];
  cardColumns?: { cardId: string; label: string; columns: string[] }[];
  /** The parameter's loaded option list, so a category/location default is picked, not typed. */
  defaultOptions?: string[];
  onChange: (next: DashboardParameter) => void;
  onDelete: () => void;
}
```

```tsx
      <DefaultValueField param={param} options={defaultOptions}
        onChange={(d) => onChange({ ...param, default: d })} />
```

with `defaultOptions = []` in the destructuring.

- [ ] **Step 6: Fix the two existing tests this replaces**

`FilterSettingsPanel.test.tsx` has two tests written against the old text box. Both must change —
they are now asserting a control that no longer exists, and leaving them would leave the suite red:

```tsx
  it("default control writes param.default", () => {
    const onChange = vi.fn();
    render(<FilterSettingsPanel param={p({})} allParameters={[p({})]} defaultOptions={["West"]}
      onChange={onChange} onDelete={vi.fn()} />);
    fireEvent.click(screen.getByLabelText("option West"));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ default: ["West"] }));
  });

  it("clearing the default writes undefined", () => {
    const onChange = vi.fn();
    render(<FilterSettingsPanel param={p({ default: ["West"] })} allParameters={[p({})]} defaultOptions={["West"]}
      onChange={onChange} onDelete={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "clear default value" }));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ default: undefined }));
  });
```

- [ ] **Step 7: Pass the options through**

In `frontend/src/components/chart/dashboard/EditSidebar.tsx`, add to `Props`:

```tsx
  defaultOptions?: string[];
```

and to the `FilterSettingsPanel` call:

```tsx
        <FilterSettingsPanel param={props.paramForPanel} allParameters={props.parameters}
          cardColumns={props.cardColumns ?? []} defaultOptions={props.defaultOptions ?? []}
          onChange={props.onChangeParam} onDelete={props.onDeleteParam} />
```

In `frontend/src/routes/explore/DashboardView.tsx`, at the `EditSidebar` call site, beside
`paramForPanel`:

```tsx
            defaultOptions={editPanel?.kind === "filter"
              ? (optionsBySlug[parameters.find((p) => p.id === editPanel.paramId)?.slug ?? ""] ?? [])
              : []}
```

The `<ParamOptions>` children at `:525-539` already run in edit mode — they are rendered from
`parameters`, not gated on `editing` — so `optionsBySlug` is populated while the panel is open.

- [ ] **Step 8: Run the tests and prove they survive mutation**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/DefaultValueField.test.tsx`
→ PASS, 5 tests.

Mutation: change `onChange(isEmptyDefault(v) ? undefined : v)` to `onChange(v)`. Re-run. Expected:
FAIL — clearing through the control now emits `[]` rather than `undefined`. Restore. Change
`hasDefault` to `true`. Re-run. Expected: FAIL on the no-clear-button test. Restore.

- [ ] **Step 9: Run the gates and commit**

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
git add frontend/src/components/chart/dashboard/DefaultValueField.tsx \
        frontend/src/components/chart/dashboard/__tests__/DefaultValueField.test.tsx \
        frontend/src/components/chart/dashboard/FilterPill.tsx \
        frontend/src/components/chart/dashboard/FilterSettingsPanel.tsx \
        frontend/src/components/chart/dashboard/__tests__/FilterSettingsPanel.test.tsx \
        frontend/src/components/chart/dashboard/EditSidebar.tsx \
        frontend/src/routes/explore/DashboardView.tsx
git commit -m "feat(dashboard): pick a filter's default with the filter's own widget"
```

---

## Task 8: Live check — measure, do not eyeball

**Files:** none. This task produces the numbers for the PR body.

Every defect that mattered in the chart programme was found by driving the running app. Two of the
six items in this workstream (drag activation, request suppression) **cannot** be verified in jsdom
at all, which is why they were left to this task rather than given a green test that proves nothing.

- [ ] **Step 1: Build and deploy**

```bash
just deploy-kind
```

Expected: `deployment "egret" successfully rolled out`, SPA at `http://localhost:30080`.

Sign in as `admin@egret.local` / `Admin12345!` and open the harness dashboard
`http://localhost:30080/explore/demo/dashboards/8a196343-ad93-4968-8040-67598d29c3cb`.
Do not hand-edit any harness card — see `docs/en/guides/chart-parity-harness.md`.

- [ ] **Step 2: The `location` round trip, with a real reload**

Add a `location` filter mapped to a text column, set two values, one of them containing a comma.
Then evaluate, **reload**, and evaluate again:

```js
(() => {
  const raw = new URL(location.href).searchParams.get('city');
  let parsed = null; try { parsed = JSON.parse(raw); } catch { /* not JSON */ }
  const rows = [...document.querySelectorAll('[data-testid="dashcard-body"] table tbody tr')].length;
  return { raw, parsed, isArray: Array.isArray(parsed), rows };
})()
```

Required: identical `parsed` arrays and identical `rows` before and after the reload, and
`isArray: true` on both. Record both objects. The pre-fix baseline is `isArray: true` before the
reload and a filter matching **0 rows** after it.

- [ ] **Step 3: Single vs multiple values**

Set the region filter to "A single value" in its settings panel, save, and pick a value in view
mode:

```js
(() => {
  const raw = new URL(location.href).searchParams.get('region');
  const boxes = document.querySelectorAll('[role="checkbox"]').length;
  return { raw, checkboxesInOpenPopover: boxes };
})()
```

Required: `raw` is the bare value (`"EU"`, no brackets) and `checkboxesInOpenPopover` is `0` with
the popover open. Flip back to "Multiple values" and confirm `raw` becomes `["EU"]`.

- [ ] **Step 4: Drag activation distance — the number jsdom cannot produce**

In edit mode, press the pointer on a chip's grip, move it **10px**, release. Required: the filter's
settings panel opens and the order does **not** change. Repeat with **20px** onto a neighbouring
chip. Required: the order changes and the panel does not open. Then Save, reload, and read the
persisted order:

```js
await fetch(location.pathname.replace('/explore/','/api/ws/').replace('/dashboards/','/dashboards/'), { headers: { accept: 'application/json' } })
  .then(r => r.json()).then(d => d.parameters.map(p => p.name))
```

Record the array before and after. (Use the browser's network tab to confirm the exact dashboard
GET URL if the path rewrite above does not resolve in your build.)

- [ ] **Step 5: Auto-connect, and its undo**

With ≥ 3 harness cards sharing a column, open a filter and map it on **one** card. Count mappings at
three moments:

```js
window.__countMapped = async () => {
  const d = await fetch(DASHBOARD_API_URL, { headers: { accept: 'application/json' } }).then(r => r.json());
  return d.data.cards.filter(c => (c.options.paramMappings ?? {})[PARAM_ID]).length;
};
```

For the **draft** (pre-Save) count, read it off the DOM instead — the number of cards whose overlay
select is non-empty:

```js
[...document.querySelectorAll('select[aria-label="column to filter on"]')].filter(s => s.value).length
```

Required, in order: `1` right after the manual mapping; `N` (every card on the tab with that column)
after pressing **Auto-connect** in the toast; `1` again after pressing **Undo** in the follow-up
toast. Record all three numbers and `N`. A third number that is not `1` means `revertAutoConnect` is
removing more or less than it added.

- [ ] **Step 6: Auto-apply off actually suppresses the queries**

This is the assertion the feature exists for, and it is a request count, not an appearance.

```js
window.__cardRequests = () => performance.getEntriesByType('resource')
  .filter(e => e.name.includes('/cards/') && e.name.includes('/data')).length;
```

Turn "Auto-apply filters" off. Note `__cardRequests()`. Change two filters. Note it again — required:
**unchanged**, and the bar reads exactly `2 filters changed`. Press **Apply** — required: the count
rises and the bar disappears. Record the three counts and the bar's text.

- [ ] **Step 7: The linked-filters blocked states**

With a filter whose only sibling is a "Time grouping" parameter, open its settings panel. Required:
the heading reads `Limit this filter's choices` and the body reads
`If you have another dashboard filter, …` — not an empty area. Set the same filter's values source to
a custom list. Required: the body changes to
`If the filter has values that are from another question or model, or a custom list, …`. Record both
strings verbatim.

- [ ] **Step 8: Compare against Metabase**

Open `localhost:3033/dashboard/2` and, for the settings panel of an equivalent filter, list the
sections in order on both sides. Required: egret shows Name, Type, (Operator), "How should people
filter on this?", "Where do values come from?", "People can pick", "Default value", "Limit this
filter's choices", Required, Delete filter — the same order Metabase uses. Record any section
present on one side and absent on the other, and say whether it is in scope for W6 or belongs to a
later workstream.

- [ ] **Step 9: Record the numbers and open the PR**

Open the PR with every measurement above quoted verbatim, plus the frontend test count from the
final `npm run test`. The PR body must also state, in one sentence, that the auto-apply preference
is stored per browser because the dashboard PUT has no field for it, and that making it a
dashboard-level setting needs a migration.

Then **stop** — the merge is the user's call.

---

## Self-review checklist for the implementer

- [ ] `DashboardView.tsx` gained wiring only: one line for the draft hook, one prop on
      `EditFilterChips`, one prop on `EditSidebar`, and `offerAutoConnectFor` — which is offset by
      the five duplicated lines `mappableColumns` removed from the overlay call site. Every
      decision lives in a module with its own test.
- [ ] The `location` fix is a **round trip** — write, then read back — and includes a value with a
      comma in it.
- [ ] No test asserts a display string where the code keys on a raw value. The three places this
      was a live risk — the single-value option emitting `"US"`, the category default storing
      `["EU"]`, the auto-connect patch — all assert the raw value; the only string assertions are
      against functions whose *return value is the string*.
- [ ] Every new assertion was watched failing against a stubbed implementation, scored by **exit
      code**, not by matching stdout text.
- [ ] No new dependency was added. Drag uses `@dnd-kit`, already present, and the 15px constant is
      exported once and asserted once.
- [ ] The auto-apply toggle did **not** grow a database column, and its scope is stated in the UI.
- [ ] `paramToFilters`'s existing `id` behaviour (one value → `eq`) is unchanged, and the existing
      test that pins it still passes.
- [ ] `text` and `number` are in the ported multiplicity table but suppressed from the UI, with the
      reason written in `multiSelect.ts` where the next reader will find it.
- [ ] Gates ran as three separate commands, never through a pipe and never into `rtk`.
- [ ] The plan stopped at the PR.
