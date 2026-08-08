# W3 — Fixed / Full Width Control Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the dashboard a control for the width capability the stack has carried since
migration 0018 and nothing has ever reached — a "Full width" switch in a new edit-mode "…" menu —
move the fixed measure to Metabase's 1048px, and make **newly created** dashboards default to
`"fixed"` without touching a single existing row.

**Architecture:** Everything for this already exists except the control and one column default.
The column is there (`migrations/0018_dashboard_width.sql`), the repo reads and writes it
(`dashboard.rs:91,186-199`), the PUT validates it (`routes.rs:2705-2709`), the DTO carries it
(`bi.rs:368`), the generated client types it (`dashboardDetailDto.ts:19`), the hand-written adapter
narrows it to `"fixed" | "full"` (`api/dashboards.ts:72`), and the route *renders* it
(`DashboardView.tsx:178,462`). What is missing is (a) anything that sets it and (b) a save path that
sends anything other than the value it just read back. So this workstream is small in code and
almost entirely about not breaking the two things that are easy to break: the draft/stored
distinction on the two save paths, and existing rows on a default change.

**Tech Stack:** React 18 + TypeScript, Radix Popover + Radix Switch via shadcn wrappers, Tailwind
v4 (OKLCH tokens), vitest + Testing Library. Backend: Rust (axum + sqlx), Postgres.

**Design reference:** `docs/en/specs/2026-08-09-dashboard-parity-design.md` §3.1 and §4 W3. §9 lists
findings that did **not** survive inspection — do not plan or write work for any of them.

**Branch:** `feat/dashboard-parity-03-width-control`, from `main`. W3 does **not** depend on W1;
the two touch disjoint files apart from `DashboardView.tsx`, and the hunks there are far apart
(W1 edits `applyAutoHeight`/`onLayoutChange`/the grid call site; W3 edits the width state, the
wrapper `<div>` and `onSave`).

---

## Background the implementer needs

**The two save paths are not symmetric, and that is correct.** `onSave` (`DashboardView.tsx:385-394`)
saves a *draft*; `saveDescription` (`:400-407`) saves from the *server* dashboard while in view
mode, where no draft exists. Both currently send `width: dashboard.width`. Only `onSave` should
change. If `saveDescription` is "fixed" by making it read the draft width too, then editing a
description in view mode after a cancelled edit writes a width the user cancelled. Leave it reading
`dashboard.width`, and there is a test in Task 2 that fails if someone later changes it.

**The fixed measure has to be a literal string.** Tailwind v4's scanner emits only classes it can
see written out in the source. `` `max-w-[${DASHBOARD_FIXED_WIDTH_PX}px]` `` compiles, type-checks,
renders the right class name into the DOM, and produces **no CSS** — the dashboard would silently
render full width with a `max-w-[1048px]` class attached to it. That is exactly the shape of defect
the chart programme kept finding: correct-looking DOM, wrong pixels. So the constant and the
literal class are two separate things, and one test exists purely to pin them to each other.

**A column default change is a change to future INSERTs only.** `ALTER COLUMN width SET DEFAULT
'fixed'` rewrites the catalog entry `pg_attrdef` holds for the column. It does not scan the table,
does not take an `ACCESS EXCLUSIVE` lock for any meaningful time, and does not touch a single
existing row. Every dashboard that exists today keeps whatever is stored in its `width` column —
which, because nothing has ever set it, is `'full'` for all of them. Task 3 proves this with a
before/after row census rather than asserting it.

**Where the default actually takes effect, and where it does not.** `DashboardRepo::create`
(`dashboard.rs:110-129`) inserts `(workspace_id, name, description, created_by)` and omits `width`
entirely, so Postgres supplies the column default. That is the whole "create path" — there is no
Rust-side width on creation, `CreateDashboard` (`bi.rs:374-379`) has no `width` field, and the
frontend create mutation sends `{name, description?}` (`api/dashboards.ts:113-116`). The Rust
`default_width()` (`bi.rs:414-416`) is the serde default on **`UpdateDashboard`** — it is the value
a `PUT` gets when the client omits `width`. It therefore governs *updates*, not creates. It still
has to move: leaving it at `"full"` means an API client that PUTs a dashboard without a `width`
field silently flips a fixed dashboard to full, and the two defaults would document contradictory
intentions in two files. Move both, in the same commit.

**Not changing `UpdateDashboard.width` to `Option<String>`.** The genuinely safe shape is
"absent means leave the stored value alone". That is a larger behavioural change to a PUT that
every other field treats as a whole-object replace (name, description, parameters, tabs and cards
are all unconditionally overwritten), and it would make `width` the single field in the body with
preserve-on-absent semantics. Out of scope here; the defaults are made to agree instead.

**`analytics.dashboards`, not `dashboards`.** 0018 says `ALTER TABLE dashboards` because it predates
`0067_analytics_kernel_schema.sql:14`, which did `ALTER TABLE dashboards SET SCHEMA analytics`. The
new migration must qualify the table or it fails with "relation does not exist".

**The trigger lives in the edit toolbar, not the route.** `DashboardView.tsx` is 769 lines and seven
workstreams touch it (index, "Modified files" table). The menu is a new component; the toolbar
renders it; the route contributes one piece of state and two props. A W3 diff that adds more than
about ten net lines to `DashboardView.tsx` has missed an extraction.

**Designing for W4.** W4 fills this same menu with Duplicate / Move / Trash / Edit settings / Reset
filters, and renders it in **view** mode where there is no width to toggle. So `width` and
`onChangeWidth` are optional and the rows come in through `children`. One consequence to know up
front: the menu is a Radix **Popover**, not a Radix DropdownMenu, because a `role="switch"` control
cannot live inside a `role="menuitem"` without nesting two interactive widgets. W4's rows are
therefore plain buttons inside the popover, not `<DropdownMenuItem>` — a `DropdownMenuItem` outside
a `DropdownMenu.Root` throws at render. Metabase makes the same split for the same reason: its
edit-mode "…" is a `Popover` with a `Switch` (`ExtraEditButtonsMenu.tsx:27-49`) while its view-mode
"…" is a `Menu` (`DashboardActionMenu.tsx:88-163`). egret merges the two surfaces into one
component, and the popover form is the superset that can host both.

**Migration numbering collides with the index.** The index reserves `migrations/0105_dashboard_revisions.sql`
for W8. The highest migration on `main` today is `0104_dataset_semantic_type_allowlist.sql`, so W3
takes **0105** and W8 renumbers to whatever is free when it lands. Confirm with
`ls migrations/ | tail -3` before creating the file; if another workstream has already merged a
0105, take the next free number and keep the same file name suffix.

**There is already a width test file.** `frontend/src/routes/explore/__tests__/DashboardView.width.test.tsx`
exists and asserts `max-w-[1200px]` plus "saving sends the stored dashboard width". Neither the
design nor the index mentions it. It is not an obstacle — it is the file Task 2 rewrites, and its
third test is the exact assertion this workstream inverts.

---

## File structure

| Path | Responsibility after this workstream |
|---|---|
| `frontend/src/components/chart/dashboard/DashboardActionsMenu.view.tsx` | **New.** The dashboard-page "…" menu, plus the fixed-canvas measure and the Tailwind class that expresses it. Exports `DashboardViewActionsMenu`, `DASHBOARD_FIXED_WIDTH_PX`, `dashboardWidthClass`. The export name is deliberately **not** `DashboardActionsMenu`: the list-page component of that name (`DashboardActionsMenu.tsx`) already exists and is a different menu for a different surface, and two identically-named exports would make every import line ambiguous to a reader. |
| `frontend/src/components/chart/dashboard/DashboardEditToolbar.tsx` | Gains the "…" trigger at the end of its icon row, and the two width props it forwards. |
| `frontend/src/routes/explore/DashboardView.tsx` | Wiring only: one piece of draft state, the width the canvas renders at, two props, and the width `onSave` sends. |
| `migrations/0105_dashboard_width_default_fixed.sql` | **New.** `ALTER COLUMN width SET DEFAULT 'fixed'` on `analytics.dashboards`. Future inserts only. |
| `crates/egret-analytics-api/src/bi.rs` | `default_width()` returns `"fixed"`, agreeing with the column default. |
| `crates/egret-metastore/src/lib.rs` | One re-embed comment line so `sqlx::migrate!` picks the new file up. |
| `crates/egret-bi/tests/dashboard_repo.rs` | Gains the default + stored-value round-trip test. |

Files this workstream must **not** touch: `components/chart/dashboard-grid.ts` (W1 and W9 own it —
W3 has no reason to be in there, and being in there would produce a merge conflict with W1 for no
gain) and `components/chart/dashboard/DashboardActionsMenu.tsx` (the list-page menu, untouched).

---

## Task 1: The "…" menu and the 1048px measure

**Files:**
- Create: `frontend/src/components/chart/dashboard/DashboardActionsMenu.view.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/DashboardActionsMenu.view.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/DashboardActionsMenu.view.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {
  DASHBOARD_FIXED_WIDTH_PX,
  DashboardViewActionsMenu,
  dashboardWidthClass,
} from "../DashboardActionsMenu.view";

describe("dashboardWidthClass", () => {
  it("centres a fixed dashboard at Metabase's measured width and leaves a full one unconstrained", () => {
    // Measured live at an 1800px viewport on localhost:3033: Metabase's grid stayed 1048px wide.
    // egret's previous measure was 1200px, which is not a number Metabase has anywhere.
    expect(DASHBOARD_FIXED_WIDTH_PX).toBe(1048);
    expect(dashboardWidthClass("fixed")).toBe("mx-auto w-full max-w-[1048px]");
    expect(dashboardWidthClass("full")).toBe("");
  });

  it("keeps the literal class in step with the constant", () => {
    // Tailwind's scanner emits only classes it can see written out in the source, so the class
    // CANNOT be built from the constant — a template literal would render the right class name
    // with no CSS behind it. This assertion is the only thing stopping the two from drifting.
    expect(dashboardWidthClass("fixed")).toContain(`max-w-[${DASHBOARD_FIXED_WIDTH_PX}px]`);
  });
});

describe("DashboardViewActionsMenu", () => {
  it("shows the switch off for a fixed dashboard and reports 'full' when it is turned on", async () => {
    const user = userEvent.setup();
    const onChangeWidth = vi.fn();
    render(<DashboardViewActionsMenu width="fixed" onChangeWidth={onChangeWidth} />);

    await user.click(screen.getByRole("button", { name: "dashboard actions" }));
    const sw = await screen.findByRole("switch", { name: "Full width" });
    expect(sw).toHaveAttribute("aria-checked", "false");

    await user.click(sw);
    expect(onChangeWidth).toHaveBeenCalledWith("full");
  });

  it("shows the switch on for a full dashboard and reports 'fixed' when it is turned off", async () => {
    // Both directions are asserted because a handler hard-wired to one of them passes the other
    // test and is exactly the mutation Step 5 introduces.
    const user = userEvent.setup();
    const onChangeWidth = vi.fn();
    render(<DashboardViewActionsMenu width="full" onChangeWidth={onChangeWidth} />);

    await user.click(screen.getByRole("button", { name: "dashboard actions" }));
    const sw = await screen.findByRole("switch", { name: "Full width" });
    expect(sw).toHaveAttribute("aria-checked", "true");

    await user.click(sw);
    expect(onChangeWidth).toHaveBeenCalledWith("fixed");
  });

  it("renders extra rows, and renders no switch when no width is supplied", async () => {
    // This pins the contract W4 depends on: the same menu is rendered in VIEW mode, where there
    // is no draft width to toggle, carrying only its action rows.
    const user = userEvent.setup();
    render(
      <DashboardViewActionsMenu>
        <button type="button">Duplicate</button>
      </DashboardViewActionsMenu>,
    );

    await user.click(screen.getByRole("button", { name: "dashboard actions" }));
    expect(await screen.findByRole("button", { name: "Duplicate" })).toBeInTheDocument();
    expect(screen.queryByRole("switch")).toBeNull();
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/DashboardActionsMenu.view.test.tsx`

Expected: FAIL — `Failed to resolve import "../DashboardActionsMenu.view"`.

- [ ] **Step 3: Write the implementation**

Create `frontend/src/components/chart/dashboard/DashboardActionsMenu.view.tsx`:

```tsx
import type { ReactNode } from "react";
import { MoreHorizontal } from "lucide-react";
import { Popover, PopoverTrigger, PopoverContent } from "@/components/ui/popover";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";

/** Metabase's fixed canvas measure. Measured live at an 1800px viewport on localhost:3033: the
 *  dashboard grid stayed 1048px wide, centred. egret's previous 1200px was not a Metabase
 *  number. Exported for the test that pins it to the class below — nothing composes it. */
export const DASHBOARD_FIXED_WIDTH_PX = 1048;

/** The wrapper classes for a dashboard canvas at the given width.
 *
 *  The measure is written out as a LITERAL and not interpolated from DASHBOARD_FIXED_WIDTH_PX:
 *  Tailwind's scanner emits only the classes it can see in the source, so an interpolated
 *  `max-w-[…]` would render as a class name with no rule behind it and the canvas would silently
 *  go full width. A test asserts the two agree. */
export function dashboardWidthClass(width: "fixed" | "full"): string {
  return width === "fixed" ? "mx-auto w-full max-w-[1048px]" : "";
}

/** Ghost icon button, matching the density of the surfaces this menu is rendered on. Callers
 *  that sit in a toolbar with its own icon class pass that class instead. */
const DEFAULT_TRIGGER =
  "inline-flex size-8 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground";

interface Props {
  /** Rendered as the "Full width" switch when supplied. Edit mode passes the DRAFT width; view
   *  mode (W4) passes nothing and the switch is absent. */
  width?: "fixed" | "full";
  onChangeWidth?: (next: "fixed" | "full") => void;
  /** Action rows below the switch — W4's Duplicate / Move / Trash / Edit settings / Reset
   *  filters. They must be plain buttons: this is a Popover, not a DropdownMenu (a role="switch"
   *  control cannot nest inside a role="menuitem"), so a <DropdownMenuItem> here would throw for
   *  want of a DropdownMenu.Root ancestor. */
  children?: ReactNode;
  triggerClassName?: string;
}

/** The dashboard PAGE's "…" menu. Deliberately distinct from `DashboardActionsMenu.tsx`, which is
 *  the row menu on the dashboard LIST page and shares nothing with this but a shape.
 *
 *  Mirrors Metabase's ExtraEditButtonsMenu.tsx:27-49 — a popover holding a single "Full width"
 *  switch — and is the component W4 fills with the view-mode actions. */
export function DashboardViewActionsMenu({ width, onChangeWidth, children, triggerClassName }: Props) {
  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          className={triggerClassName ?? DEFAULT_TRIGGER}
          aria-label="dashboard actions"
          title="More"
        >
          <MoreHorizontal className="h-4 w-4" />
        </button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-56 p-1">
        {width && onChangeWidth && (
          <div className="flex items-center justify-between gap-3 rounded-sm px-2 py-1.5">
            <Label htmlFor="dashboard-full-width" className="cursor-pointer font-normal">
              Full width
            </Label>
            {/* The <Label htmlFor> makes the text a click target; the aria-label makes the
                accessible name independent of how the label is wired, so a future restyle of the
                row cannot quietly rename the control out from under its tests. */}
            <Switch
              id="dashboard-full-width"
              aria-label="Full width"
              checked={width === "full"}
              onCheckedChange={(on) => onChangeWidth(on ? "full" : "fixed")}
            />
          </div>
        )}
        {children}
      </PopoverContent>
    </Popover>
  );
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/DashboardActionsMenu.view.test.tsx`

Expected: PASS, 5 tests.

- [ ] **Step 5: Prove the tests survive mutation**

Three mutations, one per assertion that matters. Run the same command after each and restore before
the next.

1. `return width === "fixed" ? "mx-auto w-full" : "";` — drops the measure entirely.
   Expected: FAIL on both `dashboardWidthClass` tests.
2. `return width === "fixed" ? \`mx-auto w-full max-w-[${DASHBOARD_FIXED_WIDTH_PX}px]\` : "";` —
   the interpolation the comment warns about. Expected: **PASS**. This one is a warning, not a
   check: the test suite cannot see the Tailwind build, so the only thing standing between this
   mutation and a silently-full-width dashboard is the comment and the live check in Task 4. Do not
   "improve" the code into this form.
3. `onCheckedChange={() => onChangeWidth("full")}` — hard-wires one direction.
   Expected: FAIL on "reports 'fixed' when it is turned off".

Score every run by process **exit code**, never by matching stdout text.

This step is not optional. Six assertions in the chart programme passed against a stubbed
implementation; doing exactly this is the only thing that caught them.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/dashboard/DashboardActionsMenu.view.tsx \
        frontend/src/components/chart/dashboard/__tests__/DashboardActionsMenu.view.test.tsx
git commit -m "feat(dashboard): dashboard-page actions menu with a Full width switch, at Metabase's 1048px measure"
```

---

## Task 2: The toolbar shows it and the route saves it

**Files:**
- Modify: `frontend/src/components/chart/dashboard/DashboardEditToolbar.tsx`
- Modify: `frontend/src/routes/explore/DashboardView.tsx` (`:178`, `:240-249`, `:385-394`, `:462`, the `<DashboardEditToolbar>` call site at `:511-518`)
- Modify: `frontend/src/routes/explore/__tests__/DashboardView.width.test.tsx`

- [ ] **Step 1: Rewrite the existing width test to the new expectations**

`frontend/src/routes/explore/__tests__/DashboardView.width.test.tsx` already exists. Replace its
whole body with:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import DashboardView from "../DashboardView";

vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({ DashboardCardView: () => <div /> }));
vi.mock("@/api/charts", async (orig) => ({
  ...(await orig<typeof import("@/api/charts")>()),
  useCharts: () => ({ data: [] }),
}));

const update = { mutateAsync: vi.fn().mockResolvedValue({}), isPending: false };
const base = {
  id: "d1", name: "D", description: "", created_by: null, updated_at: "", parameters: [], tabs: [],
  cards: [{ id: "c1", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 0, size_x: 6, size_y: 1, options: { text: "h" }, tab_id: null }],
  width: "full",
};
let detail: Record<string, unknown> = { ...base };
vi.mock("@/api/dashboards", async (orig) => {
  const mod = await (orig as () => Promise<Record<string, unknown>>)();
  return { ...mod, useDashboard: () => ({ data: detail, isLoading: false }), useDashboardMutations: () => ({ update }) };
});

const renderView = () => render(
  <QueryClientProvider client={new QueryClient()}>
    <MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}>
      <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
    </MemoryRouter>
  </QueryClientProvider>,
);

const FIXED = ".max-w-\\[1048px\\]";

/** Open edit mode and flip the Full width switch. */
async function toggleFullWidth(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("button", { name: "Edit" }));
  await user.click(screen.getByRole("button", { name: "dashboard actions" }));
  await user.click(await screen.findByRole("switch", { name: "Full width" }));
}

describe("DashboardView width", () => {
  it("view mode 'fixed' wraps content in a centered 1048px container", () => {
    detail = { ...base, width: "fixed" };
    const { container } = renderView();
    expect(container.querySelector(FIXED)).toBeTruthy();
  });

  it("view mode 'full' has no fixed wrapper", () => {
    detail = { ...base, width: "full" };
    const { container } = renderView();
    expect(container.querySelector(FIXED)).toBeNull();
  });

  it("edit mode: the canvas follows the DRAFT width before anything is saved", async () => {
    // Without a draft the canvas would keep rendering the stored width and the switch would look
    // inert until Save — which is how the toggle reads as broken even when the payload is right.
    detail = { ...base, width: "fixed" };
    const user = userEvent.setup();
    const { container } = renderView();
    expect(container.querySelector(FIXED)).toBeTruthy();
    await toggleFullWidth(user);
    expect(container.querySelector(FIXED)).toBeNull();
  });

  it("edit mode: saving sends the toggled width, not the stored one", async () => {
    detail = { ...base, width: "fixed" };
    update.mutateAsync.mockClear();
    const user = userEvent.setup();
    renderView();
    await toggleFullWidth(user);
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(update.mutateAsync).toHaveBeenCalledWith(expect.objectContaining({ width: "full" }));
  });

  it("cancelling an edit discards the width change", async () => {
    detail = { ...base, width: "fixed" };
    const user = userEvent.setup();
    const { container } = renderView();
    await toggleFullWidth(user);
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(container.querySelector(FIXED)).toBeTruthy();
  });

  it("the description-only save keeps sending the STORED width", async () => {
    // saveDescription runs in VIEW mode, where there is no draft. If it ever starts reading the
    // draft width it will write back a width the user cancelled.
    detail = { ...base, width: "fixed" };
    update.mutateAsync.mockClear();
    const user = userEvent.setup();
    renderView();
    await user.click(screen.getByRole("button", { name: "dashboard info" }));
    const box = await screen.findByLabelText("description");
    fireEvent.change(box, { target: { value: "notes" } });
    fireEvent.blur(box);
    expect(update.mutateAsync).toHaveBeenCalledWith(
      expect.objectContaining({ description: "notes", width: "fixed" }),
    );
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/routes/explore/__tests__/DashboardView.width.test.tsx`

Expected: FAIL — the first two fail because the wrapper is still `max-w-[1200px]`; the three edit
tests fail at `getByRole("button", { name: "dashboard actions" })` because the toolbar has no such
trigger yet.

- [ ] **Step 3: Add the trigger to the edit toolbar**

In `frontend/src/components/chart/dashboard/DashboardEditToolbar.tsx`, add the import beneath the
existing `dashboard-sections` import:

```tsx
import { DashboardViewActionsMenu } from "./DashboardActionsMenu.view";
```

Extend `Props` with the two width fields:

```tsx
interface Props {
  onAddChart: () => void;
  onAddText: () => void;
  onAddHeading: () => void;
  onAddLink: () => void;
  onAddSection: (layout: SectionLayout) => void;
  onAddFilter: () => void;
  /** The DRAFT width — the switch reflects what will be saved, not what is stored. */
  width: "fixed" | "full";
  onChangeWidth: (next: "fixed" | "full") => void;
}
```

Widen the destructuring on the component signature:

```tsx
export function DashboardEditToolbar({ onAddChart, onAddText, onAddHeading, onAddLink, onAddSection, onAddFilter, width, onChangeWidth }: Props) {
```

And add the menu as the last child of the toolbar's `<div className="flex items-center gap-1">`,
directly after the "add filter" button:

```tsx
      <DashboardViewActionsMenu width={width} onChangeWidth={onChangeWidth} triggerClassName={ICON} />
```

- [ ] **Step 4: Wire the route**

In `frontend/src/routes/explore/DashboardView.tsx`, add the import beside the other dashboard
component imports (after the `DashboardEditToolbar` import at `:30`):

```tsx
import { dashboardWidthClass } from "@/components/chart/dashboard/DashboardActionsMenu.view";
```

Add the draft state next to `draftName` (currently `:67`):

```tsx
  const [draftWidth, setDraftWidth] = useState<"fixed" | "full">("full");
```

Replace `effectiveWidth` (`:178`):

```tsx
  const effectiveWidth = editing ? draftWidth : dashboard.width;
```

In `startEdit()` (`:240-248`), snapshot the width alongside the name — add after `setDraftName(dashboard.name);`:

```tsx
    setDraftWidth(dashboard.width);
```

`cancelEdit()` needs no change: `effectiveWidth` reads `dashboard.width` the moment `editing` is
false, and `startEdit()` re-snapshots on the next entry, so a stale `draftWidth` is unreachable.

Replace the wrapper `<div>` at `:462`:

```tsx
<div className={dashboardWidthClass(effectiveWidth)}>
```

In `onSave()` (`:390`), send the draft rather than echoing the stored value:

```tsx
      await update.mutateAsync({ id: dashboard.id, name: draftName.trim() || dashboard.name, description: dashboard.description, parameters, tabs: tabPayload, width: draftWidth, cards: cardPayload });
```

Leave `saveDescription` (`:404`) reading `width: dashboard.width` — it runs in view mode where no
draft exists, and there is a test above that fails if it changes.

Pass the two props at the `<DashboardEditToolbar>` call site (`:511-518`):

```tsx
            <DashboardEditToolbar
              onAddChart={() => openPanel({ kind: "add" })}
              onAddText={() => addVirtualCard("text", { text: "" })}
              onAddHeading={() => addVirtualCard("heading", { text: "" })}
              onAddLink={() => addVirtualCard("link", { url: "", label: "" })}
              onAddSection={addSection}
              onAddFilter={addFilter}
              width={draftWidth}
              onChangeWidth={setDraftWidth}
            />
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/routes/explore/__tests__/DashboardView.width.test.tsx`

Expected: PASS, 6 tests.

- [ ] **Step 6: Prove the tests survive mutation**

Run the same command after each mutation; restore before the next.

1. Revert `effectiveWidth` to `dashboard.width`. Expected: FAIL on "the canvas follows the DRAFT
   width".
2. Revert `onSave`'s `width: draftWidth` to `width: dashboard.width`. Expected: FAIL on "saving
   sends the toggled width".
3. Change `saveDescription`'s `width: dashboard.width` to `width: draftWidth`. Expected: FAIL on
   "the description-only save keeps sending the STORED width".
4. Move `setDraftWidth(dashboard.width)` out of `startEdit()`. Expected: FAIL on "cancelling an
   edit discards the width change" — the second entry into edit mode would start from the stale
   draft. If this one passes, the test is asserting less than it looks like it is; say so rather
   than moving on.

- [ ] **Step 7: Run the whole frontend gate set**

Each as its own command. Do not chain them, do not pipe them into `rtk` — a swallowed pipe breaks
the `&&` behind it and the later gates silently never run.

```bash
cd frontend && npm run lint
```

```bash
cd frontend && npx tsc --noEmit
```

```bash
cd frontend && npm run test
```

Expected: all three green. Record the total test count from the third. The delta against the
previous PR's count is **+8**: five new tests in `DashboardActionsMenu.view.test.tsx`, and
`DashboardView.width.test.tsx` going from three tests to six. Any other delta means a gate did not
run or a file was not collected — investigate before opening the PR rather than adjusting the
expectation.

- [ ] **Step 8: Commit**

```bash
git add frontend/src/components/chart/dashboard/DashboardEditToolbar.tsx \
        frontend/src/routes/explore/DashboardView.tsx \
        frontend/src/routes/explore/__tests__/DashboardView.width.test.tsx
git commit -m "feat(dashboard): edit-mode Full width switch, saved through the existing layout PUT"
```

---

## Task 3: New dashboards default to fixed, existing ones do not move

**Files:**
- Create: `migrations/0105_dashboard_width_default_fixed.sql`
- Modify: `crates/egret-metastore/src/lib.rs`
- Modify: `crates/egret-analytics-api/src/bi.rs` (`default_width` at `:414-416`)
- Modify: `crates/egret-bi/tests/dashboard_repo.rs`

Read "A column default change is a change to future INSERTs only" and "Where the default actually
takes effect, and where it does not" in the background section before starting.

- [ ] **Step 1: Take the before census**

The dev stack must be up (`just up`) and `DATABASE_URL` set — `.env` already carries
`postgres://egret:egret@localhost:5432/egret` and `set dotenv-load := true` in the justfile picks
it up. Record, verbatim, what the table holds *before* the migration:

```bash
psql "$DATABASE_URL" -c "SELECT width, count(*) FROM analytics.dashboards GROUP BY width ORDER BY width;"
```

If `psql` is not installed:

```bash
docker compose -f deploy/docker-compose.dev.yml exec -T postgres psql -U egret -d egret -c "SELECT width, count(*) FROM analytics.dashboards GROUP BY width ORDER BY width;"
```

Expected on a seeded dev database: one row, `full | <n>`. Keep the number — Step 6 compares
against it and it goes in the PR body.

- [ ] **Step 2: Write the failing test**

Append to `crates/egret-bi/tests/dashboard_repo.rs`:

```rust
#[tokio::test]
async fn new_dashboards_default_to_fixed_and_a_stored_width_survives() {
    let Some(f) = setup().await else {
        eprintln!("skip");
        return;
    };

    // The column default is the ONLY thing that sets width on create: DashboardRepo::create omits
    // the column, CreateDashboard has no width field, and the create dialog sends {name}.
    let d = f.repo.create(f.ws, "Fresh", "", Some(f.user)).await.unwrap();
    assert_eq!(
        d.width, "fixed",
        "a newly created dashboard must adopt Metabase's fixed canvas"
    );

    // A dashboard that has been given a width keeps it. This is the half of the change that must
    // NOT happen: SET DEFAULT governs future inserts and nothing else, so a stored value always
    // wins over it.
    f.repo
        .update(d.id, "Fresh", "", &serde_json::json!([]), "full")
        .await
        .unwrap();
    let reread = f.repo.get(d.id).await.unwrap().unwrap();
    assert_eq!(reread.width, "full");

    // And the default is not sticky per-connection or per-transaction — the next create still
    // gets 'fixed' after a row has been moved to 'full'.
    let d2 = f.repo.create(f.ws, "Fresher", "", Some(f.user)).await.unwrap();
    assert_eq!(d2.width, "fixed");
}
```

- [ ] **Step 3: Run the test to verify it fails**

```bash
cargo test -p egret-bi --test dashboard_repo new_dashboards_default_to_fixed
```

Expected: FAIL — `assertion \`left == right\` failed: a newly created dashboard must adopt
Metabase's fixed canvas; left: "full", right: "fixed"`.

If it prints `skip` and passes, `DATABASE_URL` is not set — the test self-skips without it
(`dashboard_repo.rs:15-18`) and proves nothing. Fix the environment before continuing.

- [ ] **Step 4: Write the migration**

Confirm the number first:

```bash
ls migrations/ | tail -3
```

Expected: `…0104_dataset_semantic_type_allowlist.sql` last. Create
`migrations/0105_dashboard_width_default_fixed.sql`:

```sql
-- New dashboards adopt Metabase's fixed, centred canvas.
--
-- WHAT THIS DOES AND DOES NOT DO. `SET DEFAULT` rewrites one catalog entry (pg_attrdef) and
-- applies to future INSERTs only. It does not scan the table, does not rewrite a page, and does
-- not change a single existing row: every dashboard that exists keeps whatever its `width` column
-- already holds. That matters here because nothing has ever set the column — 0018 created it
-- `NOT NULL DEFAULT 'full'` and no UI reached it until this change — so every existing dashboard
-- reads 'full' and must keep reading 'full'. Anything already built keeps its appearance; only
-- boards created from now on start centred.
--
-- WHY 'fixed'. Metabase's default, measured live at an 1800px viewport: the grid stayed 1048px
-- wide and centred. Design §3.1.
--
-- THE OTHER DEFAULT. `default_width()` in egret-analytics-api/src/bi.rs is the serde default on
-- UpdateDashboard — the value a PUT gets when the client omits `width`. It moves to "fixed" in the
-- same commit. It governs updates, not creates (DashboardRepo::create omits the column entirely
-- and lets this default supply it), but leaving the two disagreeing would mean an API client that
-- omits `width` silently flips a fixed dashboard to full.
--
-- SCHEMA. The table is `analytics.dashboards`. 0018 says plain `dashboards` because it predates
-- 0067_analytics_kernel_schema.sql, which did `ALTER TABLE dashboards SET SCHEMA analytics`.
-- The CHECK from 0018 (`width IN ('fixed','full')`) already admits the new default; nothing to
-- change there.
ALTER TABLE analytics.dashboards
  ALTER COLUMN width SET DEFAULT 'fixed';
```

- [ ] **Step 5: Force the migrations to be re-embedded**

`MIGRATOR` is `sqlx::migrate!("../../migrations")` at `crates/egret-metastore/src/lib.rs:25`. The
macro expands at compile time and rustc does not reliably re-run it when a file is merely *added*
to the directory, so this repo touches the crate on every migration — the comment block at
`:1-7` and `:13-23` is that record. Add one line to the end of that block, above
`/// All SQL migrations, embedded at compile time…` (i.e. after the existing
`// re-embed: 0104 …` line at `:23`):

```rust
// re-embed: 0105 dashboard_width_default_fixed (new dashboards default to Metabase's fixed canvas)
```

Then apply it:

```bash
just migrate
```

Expected: the command exits 0 and reports the new migration applied.

- [ ] **Step 6: Take the after census and compare**

```bash
psql "$DATABASE_URL" -c "SELECT width, count(*) FROM analytics.dashboards GROUP BY width ORDER BY width;"
```

Required: **byte-identical to Step 1's output.** If any row moved, the migration did more than set
a default — stop and read it again.

Then confirm the default itself actually changed:

```bash
psql "$DATABASE_URL" -c "SELECT column_default FROM information_schema.columns WHERE table_schema='analytics' AND table_name='dashboards' AND column_name='width';"
```

Expected: `'fixed'::text`.

- [ ] **Step 7: Run the repo test to verify it passes**

```bash
cargo test -p egret-bi --test dashboard_repo new_dashboards_default_to_fixed
```

Expected: PASS, 1 test (`test result: ok. 1 passed`).

- [ ] **Step 8: Move the serde default and pin it with a test**

In `crates/egret-analytics-api/src/bi.rs`, replace `default_width` (`:414-416`):

```rust
/// The width a PUT gets when the client omits the field. Must agree with the column default set
/// by migration 0105 — if these two disagree, a dashboard's width depends on whether the row was
/// created by the API or by an INSERT, which is exactly the kind of split nobody debugs twice.
fn default_width() -> String {
    "fixed".into()
}
```

Add to the `#[cfg(test)] mod tests` block at the bottom of the same file (`:559`):

```rust
    #[test]
    fn update_dashboard_width_defaults_to_fixed_when_the_client_omits_it() {
        // Agreeing with migration 0105's column default. A PUT body with no `width` used to reset
        // the dashboard to full width, which for a fixed dashboard is a silent data loss through
        // an endpoint the client thought it was using to rename something.
        let b: UpdateDashboard = serde_json::from_str(r#"{"name":"D"}"#).unwrap();
        assert_eq!(b.width, "fixed");
    }

    #[test]
    fn update_dashboard_width_is_taken_from_the_body_when_present() {
        // The default must not shadow an explicit value — the frontend always sends one.
        let b: UpdateDashboard = serde_json::from_str(r#"{"name":"D","width":"full"}"#).unwrap();
        assert_eq!(b.width, "full");
    }
```

Run:

```bash
cargo test -p egret-analytics-api update_dashboard_width
```

Expected: PASS, 2 tests.

- [ ] **Step 9: Prove both defaults survive mutation**

Two mutations. Restore after each.

1. Revert `default_width()` to `"full".into()`.
   Run: `cargo test -p egret-analytics-api update_dashboard_width`
   Expected: FAIL on `update_dashboard_width_defaults_to_fixed_when_the_client_omits_it`.

2. Put the column default back by hand, without touching the migration file:
   ```bash
   psql "$DATABASE_URL" -c "ALTER TABLE analytics.dashboards ALTER COLUMN width SET DEFAULT 'full';"
   cargo test -p egret-bi --test dashboard_repo new_dashboards_default_to_fixed
   ```
   Expected: FAIL. Restore:
   ```bash
   psql "$DATABASE_URL" -c "ALTER TABLE analytics.dashboards ALTER COLUMN width SET DEFAULT 'fixed';"
   ```

- [ ] **Step 10: Confirm the OpenAPI surface did not move**

`#[serde(default = "…")]` without a `#[schema(default)]` emits nothing into the schema — the
checked-in `openapi.json` describes `UpdateDashboard.width` as a bare `{"type":"string"}` with no
`default` key. So the generated client needs no regeneration and `frontend/src/api/generated/` must
come back clean.

```bash
git status --short openapi.json frontend/src/api/generated
```

Expected: no output. If either changed, run `cd frontend && npm run gen:api` and include the result
in this commit rather than leaving a dirty tree.

- [ ] **Step 11: Run the backend gate set**

The justfile's `check` recipe (`justfile:14-17`) is the definition of these gates. Run them
individually so a failure names itself:

```bash
cargo fmt --all -- --check
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cargo test --workspace
```

Expected: `fmt` and `clippy` silent and green. `cargo test --workspace` green except for the two
known pre-existing engine-job `open_secret` failures that a fresh test database always produces —
those are not yours; anything else is. Alternatively `just check` runs all three in order.

- [ ] **Step 12: Commit**

```bash
git add migrations/0105_dashboard_width_default_fixed.sql \
        crates/egret-metastore/src/lib.rs \
        crates/egret-analytics-api/src/bi.rs \
        crates/egret-bi/tests/dashboard_repo.rs
git commit -m "feat(dashboard): default new dashboards to the fixed canvas, leaving existing rows untouched"
```

---

## Task 4: Live check — measure, do not eyeball

**Files:** none. This task produces the numbers that go in the PR body.

Every defect that mattered in the chart programme was found by measuring the running app. A
screenshot that "looks right" is not evidence — and this workstream has one failure mode that is
*invisible* to review, tests and screenshots taken at the wrong size: a `max-w-[1048px]` class that
Tailwind never emitted a rule for. Only a measured pixel width catches it.

- [ ] **Step 1: Build and deploy**

```bash
just deploy-kind
```

Expected: `deployment "egret" successfully rolled out`, SPA served at `http://localhost:30080`.

- [ ] **Step 2: Measure an existing dashboard, which must still be full width**

Open `http://localhost:30080/explore/demo/dashboards/8a196343-ad93-4968-8040-67598d29c3cb`
(admin@egret.local / Admin12345!) at an **1800×900** viewport and evaluate:

```js
(() => {
  const g = document.querySelector('.react-grid-layout');
  const r = g.getBoundingClientRect();
  const main = g.closest('main') ?? document.body;
  const m = main.getBoundingClientRect();
  return {
    vw: innerWidth,
    gridW: Math.round(r.width),
    gridX: Math.round(r.x),
    leftGap: Math.round(r.x - m.x),
    rightGap: Math.round(m.right - r.right),
    scrollW: document.documentElement.scrollWidth,
  };
})()
```

Required: `gridW` is the full width of `<main>` (roughly 1504 with the 264px nav rail and the
route's `p-4`), and `scrollW === 1800`. This is the harness dashboard, it existed before the
migration, and its stored width is `'full'` — it must be **unchanged**. A `gridW` of 1048 here
means the migration touched existing rows and Task 3 Step 6's census lied.

- [ ] **Step 3: Toggle it to fixed and measure again**

Click **Edit** → the **…** button in the edit toolbar → turn **Full width** *off* → **Save**.
Re-evaluate the same expression.

Required:

- `gridW === 1048`, exactly. Not 1046, not "about a thousand" — an off-by-a-few means the class is
  being applied to the wrong element and something else is doing the constraining. **A `gridW` near
  1504 with the wrapper present in the DOM is the Tailwind-never-emitted-the-rule failure**; check
  that `dashboardWidthClass` still returns a literal.
- `leftGap === rightGap` (± 1px for rounding) — the canvas is centred in `<main>`.
- `scrollW === 1800` — no horizontal scroll.

`gridX` will **not** be Metabase's 376. Metabase's reference measurement is 1048 wide at x=376 at
an 1800px viewport, but Metabase's chrome is a top bar and egret's is a 264px side rail, so egret's
x is `264 + 16 + (mainWidth − 1048) / 2` ≈ 508. The number to compare against Metabase is the
**width**, 1048, and the centring. The x offset is a shell difference, not a width-control defect.

- [ ] **Step 4: Toggle it back and confirm the round trip**

Click **Edit** → **…** → turn **Full width** back *on* → **Save**, and re-measure. Required:
`gridW` back to the Step 2 number. Then confirm the server agrees:

```bash
curl -s -b cookies.txt http://localhost:30080/api/ws/demo/dashboards/8a196343-ad93-4968-8040-67598d29c3cb | python3 -c "import json,sys; print(json.load(sys.stdin)['width'])"
```

Expected: `full`. (Use whatever authenticated fetch you already have; the point is the stored value,
not the transport.)

- [ ] **Step 5: Prove a NEW dashboard arrives fixed**

Create a dashboard from `/explore/demo/dashboards` ("New dashboard", any name), open it, and
evaluate the Step 2 expression on the empty canvas — the wrapper is present even with no cards, so
measure the wrapper instead:

```js
(() => {
  const w = document.querySelector('.max-w-\\[1048px\\]');
  const r = w.getBoundingClientRect();
  return { vw: innerWidth, wrapperW: Math.round(r.width), scrollW: document.documentElement.scrollWidth };
})()
```

Required: the element exists (so the *stored* width is `fixed` — this is the column default, live)
and `wrapperW === 1048`. If the element exists but `wrapperW` is the full `<main>` width, Tailwind
did not emit the rule: that is the one failure this whole task exists to catch.

Then delete the dashboard you created so the workspace is left as you found it. **Do not touch the
harness dashboard's cards** — 22 cards per side on byte-identical data
(`docs/en/guides/chart-parity-harness.md`).

- [ ] **Step 6: Compare against Metabase**

At the same 1800×900 viewport, open `http://localhost:3033/dashboard/2` and evaluate the width of
its grid. Reference, measured 2026-08-09: **1048px wide at x=376**. egret's number from Step 3 must
match on width.

- [ ] **Step 7: Open the PR and stop**

Open `feat/dashboard-parity-03-width-control` against `main` with, quoted verbatim in the body:

- the before/after `width, count(*)` censuses from Task 3 Steps 1 and 6, which are the proof that
  no existing row moved;
- the `column_default` query result;
- the four measured numbers: existing dashboard `gridW`, toggled-to-fixed `gridW` + `leftGap`/
  `rightGap`, new-dashboard `wrapperW`, Metabase's 1048;
- the frontend test count and its delta from the previous PR;
- a note that W8 must renumber its migration, since the index reserved 0105 and W3 has taken it.

Then **stop**. The merge is the user's call.

---

## Self-review checklist for the implementer

- [ ] `DashboardView.tsx` gained wiring only — one `useState`, one snapshot line in `startEdit`, a
      changed expression, a changed class, a changed payload field, two props. No new behaviour
      lives in the route.
- [ ] `dashboardWidthClass` returns a **literal** `max-w-[1048px]`, not an interpolation, and the
      test pinning it to `DASHBOARD_FIXED_WIDTH_PX` is present.
- [ ] No occurrence of `1200` remains anywhere in the dashboard route or its tests
      (`cd frontend && npx tsc --noEmit` will not catch this — check by reading).
- [ ] `onSave` sends `draftWidth`; `saveDescription` still sends `dashboard.width`; a test fails if
      either changes.
- [ ] The switch's two directions are separately asserted — no test passes against a handler
      hard-wired to one of them.
- [ ] The migration is `ALTER COLUMN … SET DEFAULT` on `analytics.dashboards` and contains no
      `UPDATE`, no backfill, and no `USING` clause.
- [ ] The before/after row census is recorded and identical.
- [ ] `default_width()` and the column default both say `fixed`, and each has a test that fails
      when only it is reverted.
- [ ] `crates/egret-metastore/src/lib.rs` carries the 0105 re-embed line.
- [ ] `openapi.json` and `frontend/src/api/generated/` are unchanged.
- [ ] The new menu component is `DashboardViewActionsMenu` in `DashboardActionsMenu.view.tsx`, and
      `DashboardActionsMenu.tsx` (list page) was not touched.
- [ ] `width` / `onChangeWidth` are **optional** props and `children` renders without them, so W4
      can mount the same component in view mode without a rewrite.
- [ ] No test asserts a display string where the code keys on a raw one.
