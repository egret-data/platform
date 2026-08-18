# Explore List Primitives + Data API List Pages Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract the list-page shapes that six Explore pages already duplicate into four shared components, and make the two Data API list pages the first consumers.

**Architecture:** Four presentational primitives under `frontend/src/components/explore/`, each taking state and callbacks and owning no data fetching. `Endpoints` and `Clients` then adopt them and gain the two things they lack that need no backend: facet filters and a proper row-actions menu.

**Tech Stack:** React 18, TypeScript, react-router-dom, TanStack Query via orval-generated hooks, shadcn/Radix, Tailwind v4 OKLCH tokens, vitest + Testing Library.

**Spec:** `docs/en/specs/2026-08-18-data-api-frontend-rework-design.md` (English canonical; Vietnamese mirror alongside). This plan covers **phases 1 and 2 only**.

**One correction to the spec, made deliberately.** The spec's phase 1 lists six primitives including `BookmarkToggle`, `TrashToggle` and `FolderToolbar`. Those three have **no consumer** until phases 3 and 6 (bookmark and folder-move need backend routes; trash needs a migration). Building them now would violate the spec's own rule — *"a primitive with no consumer is unreviewable in practice: nothing shows whether its interface is right."* So they move to the phases that wire them, and this plan builds only the four that phase 2 consumes immediately.

## Global Constraints

- **No primitive owns data fetching.** Each takes state and callbacks. A primitive that fetched would make every page's query key its business — the coupling behind the P5b defect where six mutations toasted success over a stale list.
- **Error must never fold into empty.** Two account pickers did that in the previous phase and told an operator to create a service account that already existed — a dead end with a false explanation. A shared component that permits the collapse spreads the defect instead of containing it.
- **No msw.** Tests `vi.mock` the API module and use `vi.mocked(hook).mockReturnValue(...)`.
- **The eslint baseline is exactly 0 errors / 178 warnings.** A new warning is a regression. `eslint.config.js` enables `react-hooks/set-state-in-effect`.
- **`Badge variant="success"` is a hardcoded `bg-green-600`, not the OKLCH token.** Use `bg-success/15 text-success`.
- Inline styles use `var(--token)`, never `hsl(var(--token))`.
- Wide content scrolls inside its own `overflow-x: auto` container; the page body never scrolls horizontally.
- **Every page keeps exactly one `h1`.** `ListPageShell` owns it; a page that renders its own as well has two.
- **Every assertion is mutation-checked before being believed, and a mutation that fails to compile proves nothing** — build to zero errors first, confirm that, then read the test result. Prefer asserting the specific outcome over "it failed somehow": a recent mutation removed an entire validation scan and the input was still refused, by a different rule, with a different error.
- **Do not touch `Datasets.tsx`, `Charts.tsx`, `Dashboards.tsx` or `Alerts.tsx`.** Migrating them is a separate risk with a separate review (spec §10). This plan proves the primitives on pages that have none of this today, precisely so nothing working can regress.
- **Run the FULL frontend suite before every commit that touches a shared component.** A shared file changed under a narrowly-scoped test run is how a regression reached `main` in the previous session.

---

## File Structure

| File | Responsibility |
|---|---|
| `frontend/src/components/explore/ListToolbarButton.tsx` | **New.** One tooltip icon-button. The `Tooltip`/`TooltipTrigger`/`TooltipContent`/`Button` cluster is written out four times per page across four pages. |
| `frontend/src/components/explore/ListPageShell.tsx` | **New.** `p-4` wrapper, the single `h1`, a toolbar slot, a primary-action slot. |
| `frontend/src/components/explore/ListStates.tsx` | **New.** The loading / error / empty / filtered-empty branch, copied six times as a four-way ternary. Renders `children` only when there is data. |
| `frontend/src/components/explore/EmptyState.tsx` | **Modify.** Exists, imported by nothing. Gains the dashed-border variant the pages actually render. |
| `frontend/src/components/explore/BulkBar.tsx` | **Modify.** Exists, imported by nothing. `BulkToggle` becomes the icon+tooltip shape the pages use, not a text button. |
| `frontend/src/routes/explore/dataapi/EndpointActionsMenu.tsx` | **New.** Endpoints' row menu, matching the four `*ActionsMenu` components the other domains have. |
| `frontend/src/routes/explore/dataapi/ServiceAccountActionsMenu.tsx` | **New.** Clients has no row menu at all today; its actions are loose buttons in the row. |
| `frontend/src/routes/explore/dataapi/Endpoints.tsx` | **Modify.** Adopts the four primitives; gains state and engine facets. |
| `frontend/src/routes/explore/dataapi/Clients.tsx` | **Modify.** Adopts the four primitives; search moves from inside `FilterableTable` to the page toolbar. |

**Task order:** the four primitives first (Tasks 1-4), each with its own tests, then the two pages (Tasks 5-6). Tasks 1-4 change no page, so they cannot break anything; Tasks 5-6 are where the interfaces are proven.

---

### Task 1: `ListToolbarButton`

**Files:**
- Create: `frontend/src/components/explore/ListToolbarButton.tsx`
- Test: `frontend/src/components/explore/__tests__/ListToolbarButton.test.tsx`

**Interfaces:**
- Consumes: `Button` from `@/components/ui/button`; `Tooltip`, `TooltipTrigger`, `TooltipContent` from `@/components/ui/tooltip`.
- Produces: `export function ListToolbarButton(props: { icon: ComponentType<{ className?: string }>; label: string; active?: boolean; filled?: boolean; onClick: () => void })`.

- [ ] **Step 1: Write the failing test**

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Star } from "lucide-react";
import { ListToolbarButton } from "@/components/explore/ListToolbarButton";

describe("ListToolbarButton", () => {
  it("exposes its label to assistive tech, since the button shows only an icon", () => {
    render(<ListToolbarButton icon={Star} label="Bookmarks" onClick={() => {}} />);
    // The visible content is an icon. Without an accessible name the control is
    // unreachable by anything but sight, which is why `label` is required rather
    // than optional.
    expect(screen.getByRole("button", { name: "Bookmarks" })).toBeInTheDocument();
  });

  it("reads as pressed when active, so the toolbar shows which filters are on", () => {
    const { rerender } = render(<ListToolbarButton icon={Star} label="Bookmarks" onClick={() => {}} />);
    expect(screen.getByRole("button", { name: "Bookmarks" })).toHaveAttribute("aria-pressed", "false");
    rerender(<ListToolbarButton icon={Star} label="Bookmarks" active onClick={() => {}} />);
    expect(screen.getByRole("button", { name: "Bookmarks" })).toHaveAttribute("aria-pressed", "true");
  });

  it("calls onClick", async () => {
    const onClick = vi.fn();
    render(<ListToolbarButton icon={Star} label="Trash" onClick={onClick} />);
    await userEvent.click(screen.getByRole("button", { name: "Trash" }));
    expect(onClick).toHaveBeenCalledTimes(1);
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd frontend && npx vitest run src/components/explore/__tests__/ListToolbarButton.test.tsx`
Expected: FAIL — `Cannot find module '@/components/explore/ListToolbarButton'`.

- [ ] **Step 3: Write the component**

```tsx
import type { ComponentType } from "react";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

/**
 * One icon-only control in a list page's header toolbar.
 *
 * This cluster — `Tooltip` + `TooltipTrigger asChild` + an icon `Button` with an
 * `aria-label` + `TooltipContent side="bottom"` — is written out four times per
 * page in `Datasets.tsx`, `Charts.tsx`, `Dashboards.tsx` and `Alerts.tsx`. The
 * copies had already drifted: some pass `variant={active ? "secondary" : "outline"}`
 * and some do not, and none of them set `aria-pressed`, so a screen reader could
 * not tell an engaged filter from an idle one.
 *
 * `label` is required, not optional. The button's only visible content is an
 * icon, so without it the control has no accessible name at all.
 *
 * `filled` is separate from `active` on purpose: the bookmark and trash buttons
 * fill their glyph when engaged, the bulk and folder buttons do not, and that is
 * a per-icon decision rather than a consequence of being active.
 */
export function ListToolbarButton({
  icon: Icon,
  label,
  active = false,
  filled = false,
  onClick,
}: {
  icon: ComponentType<{ className?: string }>;
  label: string;
  active?: boolean;
  filled?: boolean;
  onClick: () => void;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          size="icon"
          variant={active ? "secondary" : "outline"}
          aria-label={label}
          aria-pressed={active}
          onClick={onClick}
        >
          <Icon className={`h-4 w-4 ${active && filled ? "fill-current" : ""}`} />
        </Button>
      </TooltipTrigger>
      <TooltipContent side="bottom">{label}</TooltipContent>
    </Tooltip>
  );
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cd frontend && npx vitest run src/components/explore/__tests__/ListToolbarButton.test.tsx`
Expected: PASS, 3 tests.

- [ ] **Step 5: Mutation-check the `aria-pressed` assertion**

Delete `aria-pressed={active}` from the component. Run `npm run typecheck` and confirm **zero errors**, then run the test file: the "reads as pressed" test must FAIL. Restore.

This is the assertion most likely to be vacuous — `getByRole("button", {name})` passes with or without it — so it is the one worth proving.

- [ ] **Step 6: Run the full gates and commit**

```bash
cd frontend && npx vitest run && npm run typecheck && npm run lint
```
Expected: all pass; `tsc` 0 errors; eslint 0 errors / 178 warnings.

```bash
git add frontend/src/components/explore/ListToolbarButton.tsx \
        frontend/src/components/explore/__tests__/ListToolbarButton.test.tsx
git commit -m "feat(explore): one toolbar icon-button, with the accessible name the copies lacked"
```

---

### Task 2: `ListPageShell`

**Files:**
- Create: `frontend/src/components/explore/ListPageShell.tsx`
- Test: `frontend/src/components/explore/__tests__/ListPageShell.test.tsx`

**Interfaces:**
- Consumes: nothing from Task 1 — the shell takes the toolbar as a `ReactNode`, so it does not need to know what a toolbar button is.
- Produces: `export function ListPageShell(props: { title: string; description?: string; toolbar?: ReactNode; action?: ReactNode; children: ReactNode })`.

- [ ] **Step 1: Write the failing test**

```tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { ListPageShell } from "@/components/explore/ListPageShell";

describe("ListPageShell", () => {
  it("renders exactly one h1, and it is the title", () => {
    render(
      <ListPageShell title="Endpoints" toolbar={<button type="button">tb</button>} action={<button type="button">+ Endpoint</button>}>
        <p>body</p>
      </ListPageShell>,
    );
    // Every Explore page must have exactly one document-level heading. In the
    // previous phase, removing a shell silently took a route's only `h1` with it
    // and no test noticed — the browser did.
    const h1s = screen.getAllByRole("heading", { level: 1 });
    expect(h1s).toHaveLength(1);
    expect(h1s[0]).toHaveTextContent("Endpoints");
  });

  it("renders toolbar, action and children", () => {
    render(
      <ListPageShell title="Endpoints" toolbar={<button type="button">tb</button>} action={<button type="button">+ Endpoint</button>}>
        <p>body</p>
      </ListPageShell>,
    );
    expect(screen.getByRole("button", { name: "tb" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "+ Endpoint" })).toBeInTheDocument();
    expect(screen.getByText("body")).toBeInTheDocument();
  });

  it("omits the description element entirely when there is none", () => {
    const { container } = render(
      <ListPageShell title="Endpoints"><p>body</p></ListPageShell>,
    );
    // Not "renders an empty <p>" — an empty paragraph still takes vertical space
    // and pushes the table down on every page that has no description, which is
    // all six of them today.
    expect(container.querySelector("header p")).toBeNull();
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd frontend && npx vitest run src/components/explore/__tests__/ListPageShell.test.tsx`
Expected: FAIL — module not found.

- [ ] **Step 3: Write the component**

```tsx
import type { ReactNode } from "react";

/**
 * The frame every Explore list page shares: `p-4`, one `h1`, a right-aligned
 * toolbar, and a primary action.
 *
 * The `p-4` + `flex items-center justify-between` header is written out by hand
 * in all six list pages, and three of them carry a comment calling it "the
 * canonical Explore list-page scaffold" — a convention documented six times and
 * enforced nowhere.
 *
 * It takes `toolbar` and `action` as nodes rather than as props describing what
 * they contain. A shell that knew about bookmarks, bulk mode and trash would
 * have to grow a prop per capability, and the pages differ in which they have.
 */
export function ListPageShell({
  title,
  description,
  toolbar,
  action,
  children,
}: {
  title: string;
  description?: string;
  toolbar?: ReactNode;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="p-4">
      <header className="mb-4 flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <h1 className="text-lg font-semibold">{title}</h1>
          {description ? (
            <p className="mt-0.5 text-sm text-muted-foreground">{description}</p>
          ) : null}
        </div>
        <div className="flex items-center gap-2">
          {toolbar}
          {action}
        </div>
      </header>
      {children}
    </div>
  );
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cd frontend && npx vitest run src/components/explore/__tests__/ListPageShell.test.tsx`
Expected: PASS, 3 tests.

- [ ] **Step 5: Mutation-check the single-`h1` assertion**

Change `<h1>` to `<h2>`. Typecheck (zero errors), then run the file: the first test must FAIL on the `getAllByRole("heading", { level: 1 })` lookup. Restore.

- [ ] **Step 6: Run the full gates and commit**

```bash
cd frontend && npx vitest run && npm run typecheck && npm run lint
```

```bash
git add frontend/src/components/explore/ListPageShell.tsx \
        frontend/src/components/explore/__tests__/ListPageShell.test.tsx
git commit -m "feat(explore): a shell for the list-page scaffold six pages document and none enforce"
```

---

### Task 3: `ListStates` and the `EmptyState` dashed variant

**Files:**
- Create: `frontend/src/components/explore/ListStates.tsx`
- Modify: `frontend/src/components/explore/EmptyState.tsx`
- Test: `frontend/src/components/explore/__tests__/ListStates.test.tsx`

**Interfaces:**
- Consumes: `ListSkeleton` from `@/components/explore/ListSkeleton`; `Alert`, `AlertDescription` from `@/components/ui/alert`; `EmptyState` from `./EmptyState`.
- Produces:
  - `export function ListStates(props: { isLoading: boolean; isError: boolean; error?: unknown; errorText: string; hasData: boolean; skeletonCols: number; empty: ReactNode; children: ReactNode })`
  - `EmptyState` gains `variant?: "plain" | "dashed"` (default `"plain"`).

- [ ] **Step 1: Write the failing test**

```tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { ListStates } from "@/components/explore/ListStates";

const empty = <p>No endpoints yet</p>;

describe("ListStates", () => {
  it("shows a skeleton while loading and nothing else", () => {
    render(
      <ListStates isLoading isError={false} errorText="Could not load." hasData={false} skeletonCols={5} empty={empty}>
        <p>rows</p>
      </ListStates>,
    );
    expect(screen.queryByText("rows")).not.toBeInTheDocument();
    expect(screen.queryByText("No endpoints yet")).not.toBeInTheDocument();
  });

  it("shows the ERROR, not the empty state, when the query failed", () => {
    render(
      <ListStates isLoading={false} isError error={new Error("boom")} errorText="Could not load endpoints." hasData={false} skeletonCols={5} empty={empty}>
        <p>rows</p>
      </ListStates>,
    );
    // This is the whole reason the component exists. Folding `isError` into
    // "empty" tells an operator their workspace has nothing when in fact the
    // request failed — the exact defect two account pickers shipped, which sent
    // an operator to create a service account that already existed.
    expect(screen.getByText("boom")).toBeInTheDocument();
    expect(screen.queryByText("No endpoints yet")).not.toBeInTheDocument();
  });

  it("falls back to errorText when the error carries no message", () => {
    render(
      <ListStates isLoading={false} isError error={new Error("")} errorText="Could not load endpoints." hasData={false} skeletonCols={5} empty={empty}>
        <p>rows</p>
      </ListStates>,
    );
    // `api/client.ts` throws a message-less `UnauthorizedError` on 401, and
    // `ApiError.from` falls back to `res.statusText`, which is "" over HTTP/2.
    // Without this fallback the alert renders blank — a failure that looks
    // exactly like a dead page.
    expect(screen.getByText("Could not load endpoints.")).toBeInTheDocument();
  });

  it("shows the empty state when the query succeeded with no rows", () => {
    render(
      <ListStates isLoading={false} isError={false} errorText="Could not load." hasData={false} skeletonCols={5} empty={empty}>
        <p>rows</p>
      </ListStates>,
    );
    expect(screen.getByText("No endpoints yet")).toBeInTheDocument();
    expect(screen.queryByText("rows")).not.toBeInTheDocument();
  });

  it("renders children when there is data", () => {
    render(
      <ListStates isLoading={false} isError={false} errorText="Could not load." hasData skeletonCols={5} empty={empty}>
        <p>rows</p>
      </ListStates>,
    );
    expect(screen.getByText("rows")).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd frontend && npx vitest run src/components/explore/__tests__/ListStates.test.tsx`
Expected: FAIL — module not found.

- [ ] **Step 3: Write `ListStates`**

```tsx
import type { ReactNode } from "react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { ListSkeleton } from "@/components/explore/ListSkeleton";

/**
 * The four-way branch every Explore list page opens with: loading, error, empty,
 * or data.
 *
 * Written out as a nested ternary in all six pages, and they have already drifted
 * — `Dashboards.tsx` checks `hasData` before `isError`, so a failed refetch that
 * still holds cached rows renders the table and swallows the error, while the
 * other five render the alert.
 *
 * **`isError` is checked before `hasData`, and empty is never used to stand in
 * for a failure.** Collapsing the two is the defect two account pickers shipped:
 * an operator was told to create a service account that already existed.
 */
export function ListStates({
  isLoading,
  isError,
  error,
  errorText,
  hasData,
  skeletonCols,
  empty,
  children,
}: {
  isLoading: boolean;
  isError: boolean;
  error?: unknown;
  /** Shown when the thrown error carries no message of its own. */
  errorText: string;
  hasData: boolean;
  skeletonCols: number;
  empty: ReactNode;
  children: ReactNode;
}) {
  if (isLoading) return <ListSkeleton cols={skeletonCols} />;
  if (isError) {
    // `|| errorText`, not `??`: a message-less `UnauthorizedError` and an
    // `ApiError` built from an empty `statusText` both produce "", which `??`
    // would happily render as a blank alert.
    const message = (error as Error | undefined)?.message || errorText;
    return (
      <Alert variant="destructive">
        <AlertDescription>{message}</AlertDescription>
      </Alert>
    );
  }
  if (!hasData) return <>{empty}</>;
  return <>{children}</>;
}
```

- [ ] **Step 4: Add the dashed variant to `EmptyState`**

Modify `frontend/src/components/explore/EmptyState.tsx`: add a `variant` prop and keep every existing prop and default behaviour.

```tsx
export function EmptyState({
  icon: Icon,
  title,
  description,
  action,
  variant = "plain",
}: {
  icon?: ComponentType<{ className?: string }>;
  title: string;
  description?: string;
  action?: ReactNode;
  /**
   * `plain` is the original centred block, sized for `FilterableTable`'s
   * `emptyText` slot. `dashed` is the framed first-run block the six list pages
   * hand-roll today; it is a variant rather than a second component because the
   * content is identical and only the frame differs.
   */
  variant?: "plain" | "dashed";
}) {
  const frame =
    variant === "dashed"
      ? "rounded border border-dashed p-8"
      : "max-w-sm py-4";
  return (
    <div className={`mx-auto flex flex-col items-center gap-2 text-center ${frame}`}>
      {/* …the existing body, unchanged… */}
    </div>
  );
}
```

Keep the existing icon/title/description/action markup exactly as it is; only the wrapper's classes become variant-dependent.

- [ ] **Step 5: Run to verify it passes**

Run: `cd frontend && npx vitest run src/components/explore/__tests__/ListStates.test.tsx`
Expected: PASS, 5 tests.

- [ ] **Step 6: Mutation-check the error/empty separation**

This is the constraint the component exists for, so prove the test is not vacuous. Change the branch order so empty wins:

```tsx
  if (!hasData) return <>{empty}</>;
  if (isError) { /* … */ }
```

Typecheck (zero errors first), then run the file. The "shows the ERROR, not the empty state" test must FAIL. Restore.

Then a second mutation: change `|| errorText` to `?? errorText`. Typecheck, run: the "falls back to errorText" test must FAIL. Restore.

- [ ] **Step 7: Run the full gates and commit**

```bash
cd frontend && npx vitest run && npm run typecheck && npm run lint
```

```bash
git add frontend/src/components/explore/ListStates.tsx \
        frontend/src/components/explore/EmptyState.tsx \
        frontend/src/components/explore/__tests__/ListStates.test.tsx
git commit -m "feat(explore): one list-state branch, with error kept distinct from empty"
```

---

### Task 4: Reshape `BulkToggle` to the toolbar idiom

**Files:**
- Modify: `frontend/src/components/explore/BulkBar.tsx`
- Test: `frontend/src/components/explore/__tests__/BulkBar.test.tsx`

**Interfaces:**
- Consumes: `ListToolbarButton` from Task 1.
- Produces: `BulkToggle(props: { active: boolean; onToggle: () => void })` — unchanged signature, new rendering; and `BulkBar(props: { count: number; children: ReactNode })` — unchanged.

**Why this task exists:** `BulkBar.tsx` is in the tree and imported by nothing. Its `BulkToggle` renders a text button reading "Bulk select"/"Cancel", while all four pages that have bulk mode render an icon button with a tooltip. Adopting the file as-is would change how four pages look when they migrate later; leaving it as-is means writing a fifth copy. So it is reshaped to match what the pages actually do.

- [ ] **Step 1: Write the failing test**

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BulkBar, BulkToggle } from "@/components/explore/BulkBar";

describe("BulkToggle", () => {
  it("is an icon button carrying an accessible name, matching the rest of the toolbar", async () => {
    const onToggle = vi.fn();
    render(<BulkToggle active={false} onToggle={onToggle} />);
    const btn = screen.getByRole("button", { name: "Bulk select" });
    expect(btn).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(btn);
    expect(onToggle).toHaveBeenCalledTimes(1);
  });

  it("reads as pressed when bulk mode is on", () => {
    render(<BulkToggle active onToggle={() => {}} />);
    expect(screen.getByRole("button", { name: "Bulk select" })).toHaveAttribute("aria-pressed", "true");
  });
});

describe("BulkBar", () => {
  it("renders nothing at all when no rows are selected", () => {
    const { container } = render(<BulkBar count={0}><button type="button">Move</button></BulkBar>);
    // Not "renders an empty bar": an empty bar still occupies vertical space and
    // shifts the table down the moment bulk mode is entered.
    expect(container).toBeEmptyDOMElement();
  });

  it("renders its actions and the selected count once rows are selected", () => {
    render(<BulkBar count={2}><button type="button">Move</button></BulkBar>);
    expect(screen.getByRole("button", { name: "Move" })).toBeInTheDocument();
    expect(screen.getByText(/2 selected/i)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd frontend && npx vitest run src/components/explore/__tests__/BulkBar.test.tsx`
Expected: FAIL — the current `BulkToggle` renders text, so `getByRole("button", {name:"Bulk select"})` finds it but `aria-pressed` is absent; and `BulkBar` renders no count.

- [ ] **Step 3: Rewrite `BulkBar.tsx`**

```tsx
import type { ReactNode } from "react";
import { ListChecks } from "lucide-react";
import { ListToolbarButton } from "@/components/explore/ListToolbarButton";

/**
 * The bulk-mode toggle, as an icon button in the header toolbar.
 *
 * It used to render a text button reading "Bulk select"/"Cancel". Every page
 * that actually has bulk mode renders an icon button with a tooltip instead, so
 * this file described a shape nothing used — which is consistent with it having
 * had no importers at all.
 */
export function BulkToggle({ active, onToggle }: { active: boolean; onToggle: () => void }) {
  return <ListToolbarButton icon={ListChecks} label="Bulk select" active={active} onClick={onToggle} />;
}

/**
 * The action strip shown once rows are selected. Renders NOTHING at zero — an
 * always-present empty bar shifts the table down the moment bulk mode is
 * entered, which reads as the page jumping.
 */
export function BulkBar({ count, children }: { count: number; children: ReactNode }) {
  if (count === 0) return null;
  return (
    <div className="mb-2 flex flex-wrap items-center gap-2 text-sm">
      <span className="text-muted-foreground">{count} selected</span>
      {children}
    </div>
  );
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cd frontend && npx vitest run src/components/explore/__tests__/BulkBar.test.tsx`
Expected: PASS, 4 tests.

- [ ] **Step 5: Mutation-check the zero-count guard**

Replace `if (count === 0) return null;` with `if (count < 0) return null;`. Typecheck (zero errors), run the file: "renders nothing at all" must FAIL. Restore.

- [ ] **Step 6: Run the full gates and commit**

```bash
cd frontend && npx vitest run && npm run typecheck && npm run lint
```

```bash
git add frontend/src/components/explore/BulkBar.tsx \
        frontend/src/components/explore/__tests__/BulkBar.test.tsx
git commit -m "feat(explore): make the unused BulkBar match the toolbar idiom pages actually use"
```

---

### Task 5: `Endpoints` adopts the primitives, and gains facets and a row menu

**Files:**
- Create: `frontend/src/routes/explore/dataapi/EndpointActionsMenu.tsx`
- Modify: `frontend/src/routes/explore/dataapi/Endpoints.tsx`
- Modify: `frontend/src/routes/explore/dataapi/__tests__/Endpoints.test.tsx`

**Interfaces:**
- Consumes: `ListPageShell`, `ListStates`, `EmptyState`, `BulkToggle`, `BulkBar`, `ListToolbarButton` (Tasks 1-4); `useBulkSelect` from `@/hooks/useBulkSelect`; `FacetSelect` and its `FACET_ALL` sentinel from `@/components/explore/FacetSelect`; `RowActionsMenu` from `@/components/explore/RowActionsMenu`.
- Produces: `EndpointActionsMenu(props: { ws: string; endpoint: DataApiEndpointDto; onDelete: () => void })`.

**Scope note:** bulk mode's only action in this task is **Delete selected**, because move-to-folder needs `folder_id` on `UpdateEndpointInput` (phase 3) and move-to-trash needs the migration (phase 6). Do not add disabled buttons for either — a control that cannot work is worse than an absent one.

- [ ] **Step 1: Write the failing tests**

Append to `frontend/src/routes/explore/dataapi/__tests__/Endpoints.test.tsx`, reusing that file's existing render helper and mocks:

```tsx
  it("filters by state through a facet, not only by clicking a summary chip", async () => {
    // The state filter exists today but is reachable only by clicking a chip in
    // the SummaryStrip, which does not read as a filter control.
    renderEndpoints([
      makeEndpoint({ id: "e1", name: "live one", state: "live" }),
      makeEndpoint({ id: "e2", name: "draft one", state: "draft" }),
    ]);
    await screen.findByText("live one");
    await userEvent.click(screen.getByRole("combobox", { name: "State" }));
    await userEvent.click(await screen.findByRole("option", { name: "draft" }));
    expect(screen.queryByText("live one")).not.toBeInTheDocument();
    expect(screen.getByText("draft one")).toBeInTheDocument();
  });

  it("shows the error, not the empty state, when the list fails to load", async () => {
    vi.mocked(api.useDataApiListEndpoints).mockReturnValue({
      data: undefined, isLoading: false, isError: true, error: new Error("nope"),
    } as never);
    renderEndpoints();
    expect(await screen.findByText("nope")).toBeInTheDocument();
    expect(screen.queryByText(/no endpoints yet/i)).not.toBeInTheDocument();
  });

  it("puts row actions in a menu rather than loose in the row", async () => {
    renderEndpoints([makeEndpoint({ id: "e1", name: "one", state: "live" })]);
    await screen.findByText("one");
    await userEvent.click(screen.getByRole("button", { name: /actions/i }));
    expect(await screen.findByRole("menuitem", { name: /open/i })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /disable/i })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /delete/i })).toBeInTheDocument();
  });

  it("offers a bulk action only once rows are selected", async () => {
    renderEndpoints([makeEndpoint({ id: "e1", name: "one", state: "draft" })]);
    await screen.findByText("one");
    await userEvent.click(screen.getByRole("button", { name: "Bulk select" }));
    // Bulk mode is on but nothing is chosen: the action strip must not be there
    // at all, or the table jumps down the moment the mode is entered.
    expect(screen.queryByRole("button", { name: /delete selected/i })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("checkbox", { name: /select one/i }));
    expect(screen.getByRole("button", { name: /delete selected/i })).toBeInTheDocument();
  });
```

If `makeEndpoint` or `renderEndpoints` do not exist in that file under those names, use the real helpers it defines — read the file first and do not add a second copy of either.

- [ ] **Step 2: Run to verify they fail**

Run: `cd frontend && npx vitest run src/routes/explore/dataapi/__tests__/Endpoints.test.tsx`
Expected: FAIL — no State combobox, no actions menu, no bulk toggle.

- [ ] **Step 3: Write `EndpointActionsMenu`**

Read `frontend/src/routes/explore/DatasetActionsMenu.tsx` (or whichever `*ActionsMenu` file exists beside the four domains) first and follow its structure exactly — the menu item order, the `RowActionsMenu` wrapper, and how destructive items are marked. Then write the Data API equivalent with the items **Open**, **Disable**/**Enable** (state-dependent, exactly as the current inline menu decides it), and **Delete**.

Move the existing inline menu logic from `Endpoints.tsx:166-212` across rather than rewriting it; the state-dependent Disable/Enable decision is already correct there.

- [ ] **Step 4: Rewrite `Endpoints.tsx`'s body**

Replace the hand-written header with `ListPageShell`, the four-way ternary with `ListStates`, the dashed empty block with `<EmptyState variant="dashed" …/>`, and add:

- a `BulkToggle` in the toolbar, plus a checkbox column and `BulkBar` when `bulkMode` is on, using `useBulkSelect(rows)`;
- `FacetSelect` for **State** (`live` / `draft` / `disabled`) and **Engine**, beside the existing search input;
- `EndpointActionsMenu` in the last column.

Keep `SummaryStrip` and its clickable chips: they are a shortcut, and the facet is the discoverable control. Both must drive the **same** `stateFilter` state — two filters that disagree is worse than one that is hard to find.

Keep the existing `FolderHeaderRow` grouping exactly as it is. Creating and assigning folders is phase 3.

- [ ] **Step 5: Run to verify they pass**

Run: `cd frontend && npx vitest run src/routes/explore/dataapi/__tests__/Endpoints.test.tsx`
Expected: PASS.

- [ ] **Step 6: Mutation-check the facet and the bulk guard**

1. Make the State facet's `onValueChange` a no-op. Typecheck (zero errors), run: the facet test must FAIL.
2. Render `BulkBar` unconditionally (drop the `count === 0` guard at the call site by passing `count={1}`). Typecheck, run: the "only once rows are selected" test must FAIL.

Restore both.

- [ ] **Step 7: Run the full gates and commit**

```bash
cd frontend && npx vitest run && npm run typecheck && npm run lint && npm run build
```
Expected: all tests pass; `tsc` 0 errors; eslint 0 errors / 178 warnings; build green.

```bash
git add frontend/src/routes/explore/dataapi/EndpointActionsMenu.tsx \
        frontend/src/routes/explore/dataapi/Endpoints.tsx \
        frontend/src/routes/explore/dataapi/__tests__/Endpoints.test.tsx
git commit -m "feat(dataapi): endpoints on the shared list primitives, with facets and a row menu"
```

---

### Task 6: `Clients` adopts the primitives and gains a row menu

**Files:**
- Create: `frontend/src/routes/explore/dataapi/ServiceAccountActionsMenu.tsx`
- Modify: `frontend/src/routes/explore/dataapi/Clients.tsx`
- Modify: `frontend/src/routes/explore/dataapi/__tests__/Clients.test.tsx`

**Interfaces:**
- Consumes: `ListPageShell`, `ListStates`, `EmptyState` (Tasks 2-3); `FilterableTable` from `@/components/explore/FilterableTable`; `RowActionsMenu`.
- Produces: `ServiceAccountActionsMenu(props: { account: DataApiServiceAccountDto; onIssueClient: () => void; onDelete: () => void })`.

**Scope note:** no bulk mode on this page in this task. A service account's destructive action is delete, and bulk-deleting credentials is a bigger decision than this plan should make silently.

- [ ] **Step 1: Write the failing tests**

Append to `frontend/src/routes/explore/dataapi/__tests__/Clients.test.tsx`, reusing its existing helpers and mocks:

```tsx
  it("shows the error, not the empty state, when the list fails to load", async () => {
    vi.mocked(api.useDataApiListServiceAccounts).mockReturnValue({
      data: undefined, isLoading: false, isError: true, error: new Error("nope"),
    } as never);
    renderClients();
    expect(await screen.findByText("nope")).toBeInTheDocument();
    expect(screen.queryByText(/no service accounts yet/i)).not.toBeInTheDocument();
  });

  it("puts the account's actions in a menu, not loose in the row", async () => {
    renderClients([makeAccount({ id: "a1", name: "bot" })]);
    await screen.findByText("bot");
    await userEvent.click(screen.getByRole("button", { name: /actions/i }));
    expect(await screen.findByRole("menuitem", { name: /issue client/i })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /delete/i })).toBeInTheDocument();
  });

  it("searches from the page toolbar, where every other Explore page has it", async () => {
    renderClients([makeAccount({ id: "a1", name: "alpha" }), makeAccount({ id: "a2", name: "beta" })]);
    await screen.findByText("alpha");
    // `FilterableTable` has its own internal search box, so this page's search
    // sat in a different place from all five siblings.
    await userEvent.type(screen.getByPlaceholderText(/search service accounts/i), "bet");
    expect(screen.queryByText("alpha")).not.toBeInTheDocument();
    expect(screen.getByText("beta")).toBeInTheDocument();
  });
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd frontend && npx vitest run src/routes/explore/dataapi/__tests__/Clients.test.tsx`
Expected: FAIL.

- [ ] **Step 3: Write `ServiceAccountActionsMenu`**

Same structure as Task 3's `EndpointActionsMenu`: a `RowActionsMenu` wrapper with **Issue client…** and **Delete**. Move the existing inline Delete button's confirm flow across; do not duplicate it.

The per-client **Revoke** button stays where it is, inside `ClientsCell` — it acts on a client credential, not on the account, and moving it into the account's menu would put an action on the wrong object.

- [ ] **Step 4: Rewrite `Clients.tsx`'s body**

`ListPageShell`, `ListStates`, `EmptyState variant="dashed"`, the row menu, and a search input in the page toolbar whose value is passed to `FilterableTable` as a controlled filter (read `FilterableTable.tsx` for the prop it accepts; if it only supports its own internal box, filter `rows` before passing them in and leave its box unused).

- [ ] **Step 5: Run to verify they pass**

Run: `cd frontend && npx vitest run src/routes/explore/dataapi/__tests__/Clients.test.tsx`
Expected: PASS.

- [ ] **Step 6: Mutation-check the search wiring**

Make the toolbar search input's `onChange` a no-op. Typecheck (zero errors), run: the search test must FAIL. Restore.

- [ ] **Step 7: Run every gate and commit**

```bash
cd frontend && npx vitest run && npm run typecheck && npm run lint && npm run build
```

```bash
git add frontend/src/routes/explore/dataapi/ServiceAccountActionsMenu.tsx \
        frontend/src/routes/explore/dataapi/Clients.tsx \
        frontend/src/routes/explore/dataapi/__tests__/Clients.test.tsx
git commit -m "feat(dataapi): clients on the shared list primitives, with a row menu and toolbar search"
```

---

## After the plan: verify it in a browser

Every phase of this project has found a defect in the browser that no test could see — CSRF traversal, a missing `h1`, literal backticks in copy, a UUID where a name belonged, and a preview that refused the most ordinary draft an author could have. Do not report this complete on a green suite.

```bash
EGRET_DATABASE__URL="postgres://egret:egret@localhost:5432/egret" ./target/debug/egret serve
cd frontend && npm run dev
```

Log in `admin@egret.local` / `Admin12345!`, workspace `default`, and check:

1. **Kill any stale `egret` first.** A previous session's binary held :8080 and served an older API; every check would have measured the wrong server. Confirm the running server is the one you just built before touching the UI.
2. **Both list pages have exactly one `h1`** and it is the page title.
3. **Break the API** (stop the backend) and reload: both pages must show the error, not "no endpoints yet".
4. **The State facet and the summary chips agree.** Pick `draft` in the facet, then click the `live` chip: the page must not end up claiming two different filters at once.
5. **Bulk mode**: entering it must not shift the table; the action strip appears only after a row is checked.
6. **The row menu opens with the keyboard** and its items are reachable by arrow keys.
7. **Read the copy on screen** — anything moved into a `ConfigTableRow.details` prop renders backticks literally.

---

## Self-Review

**Spec coverage (phases 1-2 only):**

| Spec | Task |
|---|---|
| §2 `ListPageShell` | Task 2 |
| §2 `BulkBar` + `useBulkSelect` — adopt the unused component | Task 4, consumed in Task 5 |
| §2 `EmptyState` — three states, error never folded into empty | Task 3 |
| §2 no primitive owns fetching | All four take props only; asserted structurally by their tests, which render them with no provider |
| §2 `BookmarkToggle`, `TrashToggle`, `FolderToolbar` | **Deliberately deferred** to the phases that give them a consumer — stated at the top of this plan |
| §3 facet filters | Task 5 (State, Engine) |
| §3 dedicated row-actions menu on both pages | Tasks 5 and 6 |
| §3 search in the page toolbar for Clients | Task 6 |
| §3 no certification | Not added anywhere |
| §8 primitives get their own tests | Tasks 1-4 each ship a test file |
| §8 `EmptyState` error-vs-empty mutation-checked | Task 3 Step 6 |
| §4, §5, §6 (New pages, detail layout, trash) | Not in this plan — phases 4, 5, 6 |

**Placeholder scan:** Three steps say "read the existing file and follow it" rather than pasting code — Task 5 Step 3 (`*ActionsMenu` structure), Task 5 Step 4 and Task 6 Step 4 (moving existing inline logic). Each names the exact file to read and the exact lines to move, and each is a *move* rather than an invention; pasting a rewritten copy would risk dropping the comments those blocks carry. Task 6 Step 4 names a genuine unknown — whether `FilterableTable` accepts a controlled filter — and states what to do either way.

**Type consistency:** `ListToolbarButton`'s props are used identically in Task 4's `BulkToggle`. `ListStates`'s prop names (`isLoading`, `isError`, `error`, `errorText`, `hasData`, `skeletonCols`, `empty`) appear in Tasks 5 and 6 unchanged. `EmptyState`'s new `variant` is `"plain" | "dashed"` in Task 3 and used as `variant="dashed"` in Tasks 5 and 6.
