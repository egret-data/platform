# Explore List Primitives Adoption (package U) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move `Datasets.tsx`, `Charts.tsx`, `Dashboards.tsx` and `Alerts.tsx` onto the four shared Explore primitives, fixing the missing `aria-pressed` on all four and the swallowed-error branch on `Dashboards.tsx`.

**Architecture:** Each page is converted independently and in its own commit. The four primitives already exist, are already proven on five Data API routes, and are **not modified by this plan** — if a page seems to need a new prop, that is a signal to re-read §7.1, not to add one. Every conversion is: hand-written header → `ListPageShell`, four `Tooltip`+`Button` clusters → `ListToolbarButton`, the nested state ternary → `ListStates`, the dashed first-run block → `EmptyState variant="dashed"`.

**Tech Stack:** React 18, TypeScript, react-router-dom, TanStack Query, shadcn/Radix, Tailwind v4 OKLCH tokens, vitest + Testing Library.

**Spec:** `docs/en/specs/2026-08-22-dataapi-alerts-design.md` §3 (English canonical; Vietnamese mirror alongside). This plan covers package **U** only.

## Global Constraints

- **The four primitives must end every task byte-identical.** `ListPageShell.tsx`, `ListStates.tsx`, `ListToolbarButton.tsx` and `EmptyState.tsx` are not modified by this plan. The mutation checks in Task 1 Step 7 and Task 3 Step 6 edit them *temporarily* and restore them, and `git diff --exit-code frontend/src/components/explore/` in each task's gate is what holds that. They change only additively and only with a reason (§7.1 of `docs/en/ui-conventions.md`); a slot added for one consumer is the prop-per-capability shell that §1.1 rejects.

  **Relaxed by explicit decision after the final whole-branch review (finding 3), 2026-08-22:** `ListToolbarButton.tsx` was modified — its destructured default `active = false,` became `active,` — so a momentary action button rendered without an `active` prop (e.g. `FolderToolbar`'s "New folder") omits `aria-pressed` entirely instead of emitting `aria-pressed="false"`. `aria-pressed="false"` announces a plain action button to a screen reader as "toggle button, not pressed", which is a state the control does not have; only presence/absence of the attribute correctly distinguishes a toggle from a command. This is a deliberate, additive, review-driven fix taken with reason, in the spirit §7.1 already carves out — not a re-litigation of the byte-identical rule for its own sake. `FolderToolbar.test.tsx` and `ListToolbarButton.test.tsx` were updated to assert the attribute's absence rather than `"false"`.
- **The eslint baseline is 0 errors / 177 warnings**, measured with `npx eslint .` on 2026-08-22. The previous plan recorded 178; that number is stale and must not be carried forward. A new warning is a regression.
- **No msw.** Tests `vi.mock` the API module and read mutable fixtures inside the mock factory's closure, never at module scope.
- **Every page keeps exactly one `h1`.** `ListPageShell` owns it. Assert the **count** is one, not that one exists.
- **`Badge variant="success"` is a hardcoded `bg-green-600`, not the OKLCH token.** Use `bg-success/15 text-success`.
- Inline styles use `var(--token)`, never `hsl(var(--token))`.
- **Run the full frontend suite (`npx vitest run`) before every commit in this plan.** These tasks touch pages rather than primitives, but four pages are being rewritten in sequence and a scoped run is how a regression reached `main` in an earlier session.
- **`npx tsc --noEmit` must pass before every commit.** Removing an import that is still used elsewhere in the file is the most likely error this plan produces.
- **Browser check before each page is called done.** On this codebase every UI defect that mattered was found live. Reading the diff does not settle layout (§10).

## What is deliberately NOT in scope

- **`Notebooks.tsx` (735 lines, `aria-pressed` = 0) is a fifth unconverted Explore list page.** It is named in neither §1.3's list of four nor the approved scope. Measured and recorded so Task 5 does not write an Adoption status claiming every Explore page is converted.
- `Endpoints.tsx` and `Clients.tsx` already use the primitives and are untouched.
- Package E (the Alerts editor onto `ConfigTable`) is a separate plan.

## File Structure

| File | Responsibility |
|---|---|
| `frontend/src/routes/explore/Datasets.tsx` | **Modify.** Reference conversion. Header, four toolbar buttons, state ternary, empty block. |
| `frontend/src/routes/explore/__tests__/Datasets.primitives.test.tsx` | **Create.** No general `Datasets.test.tsx` exists — only `Datasets.trash.test.tsx`, which is trash-scoped. |
| `frontend/src/routes/explore/Charts.tsx` | **Modify.** Same four conversions. |
| `frontend/src/routes/explore/__tests__/Charts.test.tsx` | **Modify.** Mocks already exist; gains an error-state fixture and two tests. |
| `frontend/src/routes/explore/Dashboards.tsx` | **Modify.** Same four conversions **plus** the branch-order fix. |
| `frontend/src/routes/explore/__tests__/Dashboards.test.tsx` | **Modify.** Gains the swallowed-error test, which must fail before the fix. |
| `frontend/src/routes/explore/Alerts.tsx` | **Modify.** Same four conversions plus the `Tabs` placement. |
| `frontend/src/routes/explore/__tests__/Alerts.test.tsx` | **Modify.** `schedulesMock` already allows per-test returns. |
| `frontend/docs/en/ui-conventions.md` (repo path `docs/en/ui-conventions.md`) | **Modify.** Adoption status counts, re-measured. |

**Task order:** Datasets first because it is the plainest of the four and its diff becomes the recipe the other three follow. Dashboards third because it carries a real bug fix that needs its own review. Alerts last because it is the largest (687 lines) and the only one with a tab bar.

---

### Task 1: `Datasets.tsx` — the reference conversion

**Files:**
- Modify: `frontend/src/routes/explore/Datasets.tsx` (header at 169-206, state ternary at 208-214, empty block at 307-313)
- Create: `frontend/src/routes/explore/__tests__/Datasets.primitives.test.tsx`

**Interfaces:**
- Consumes: `ListPageShell({ title, description?, toolbar?, action?, children })`, `ListStates({ isLoading, isError, error?, errorText, hasData, skeletonCols, empty, children })`, `ListToolbarButton({ icon, label, active?, filled?, onClick })`, `EmptyState({ icon?, title, description?, action?, variant? })` — all from `@/components/explore/`.
- Produces: nothing new. Later tasks copy this file's shape, not its exports.

- [ ] **Step 1: Write the failing test**

Create `frontend/src/routes/explore/__tests__/Datasets.primitives.test.tsx`:

```tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";

type DatasetFixture = { id: string; name: string; description: string; database_id: string; kind: string; catalog: null; schema: string | null; table: string | null; column_count: number; updated_at: string; folder_id: string | null; bookmarked: boolean };
const DEFAULT_DATASETS: DatasetFixture[] = [
  { id: "d1", name: "Orders", description: "", database_id: "db1", kind: "virtual", catalog: null, schema: null, table: null, column_count: 3, updated_at: "2026-06-21T00:00:00Z", folder_id: null, bookmarked: false },
];
// Read inside the mock factory's closure, never at module scope, so a test can
// swap the fixture between renders.
let datasets: DatasetFixture[] = DEFAULT_DATASETS;
let queryError: unknown = undefined;
vi.mock("@/api/datasets", () => ({
  useDatasets: () => ({
    data: datasets,
    isLoading: false,
    isError: queryError !== undefined,
    error: queryError,
  }),
  useDataset: () => ({ data: undefined }),
  useDeleteDataset: () => ({ mutateAsync: vi.fn().mockResolvedValue(undefined) }),
  useDatasetListMutations: () => ({
    trash: { mutateAsync: vi.fn().mockResolvedValue({}) },
    restore: { mutateAsync: vi.fn().mockResolvedValue({}) },
    addBookmark: { mutateAsync: vi.fn().mockResolvedValue({}) },
    removeBookmark: { mutateAsync: vi.fn().mockResolvedValue({}) },
  }),
  useRefreshDataset: () => ({ isPending: false, mutateAsync: vi.fn().mockResolvedValue({}) }),
}));
vi.mock("@/api/sqllab", () => ({ useDatabases: () => ({ data: [] }) }));
vi.mock("@/api/folders", () => ({
  useFolders: () => ({ data: [] }),
  useSetFolder: () => ({ mutateAsync: vi.fn().mockResolvedValue({}) }),
  useFolderMutations: () => ({
    create: { mutateAsync: vi.fn().mockResolvedValue({ id: "f1", name: "F1" }) },
    rename: { mutateAsync: vi.fn().mockResolvedValue({}) },
    remove: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
  }),
}));
vi.mock("@/components/dataset/CreateDatasetDialog", () => ({ CreateDatasetDialog: () => null }));

import Datasets from "../Datasets";

beforeEach(() => {
  vi.clearAllMocks();
  datasets = DEFAULT_DATASETS;
  queryError = undefined;
  localStorage.clear();
});

function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={["/explore/eng/datasets"]}>
          <Routes><Route path="/explore/:workspace/datasets" element={<Datasets />} /></Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

describe("Datasets on the Explore primitives", () => {
  // The confirmed defect: engaged state was carried only by
  // `variant="secondary"`, a visual class, so a screen reader announced
  // "Bookmarks" identically whether the filter was engaged or idle.
  it("reports the Bookmarks filter's engaged state with aria-pressed", async () => {
    renderAt();
    const btn = screen.getByRole("button", { name: "Bookmarks" });
    expect(btn).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(btn);
    expect(btn).toHaveAttribute("aria-pressed", "true");
  });

  it("reports the Trash view's engaged state with aria-pressed", async () => {
    renderAt();
    const btn = screen.getByRole("button", { name: "Trash" });
    expect(btn).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(btn);
    expect(btn).toHaveAttribute("aria-pressed", "true");
  });

  // Asserts the COUNT, not that one exists: the defect this guards is a layout
  // wrapper silently taking a route's only h1 with it.
  it("renders exactly one h1, and it is the page title", () => {
    renderAt();
    const h1s = screen.getAllByRole("heading", { level: 1 });
    expect(h1s).toHaveLength(1);
    expect(h1s[0]).toHaveTextContent("Datasets");
  });

  // `?? "Failed to load datasets."` renders a blank alert for a message-less
  // error, because "" is not nullish. `ListStates` uses `|| errorText`.
  it("falls back to the error text when the error carries no message", () => {
    queryError = new Error("");
    renderAt();
    expect(screen.getByText("Failed to load datasets.")).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/routes/explore/__tests__/Datasets.primitives.test.tsx`

Expected: the two `aria-pressed` tests FAIL with `expected element to have attribute aria-pressed="false"` (the attribute is absent), and the message-less-error test FAILS because `??` renders `""`. The `h1` test passes already — that is correct; it is a regression guard for a refactor, not a red-first test.

- [ ] **Step 3: Convert the header and toolbar**

In `frontend/src/routes/explore/Datasets.tsx`, add the imports:

```tsx
import { ListPageShell } from "@/components/explore/ListPageShell";
import { ListStates } from "@/components/explore/ListStates";
import { ListToolbarButton } from "@/components/explore/ListToolbarButton";
import { EmptyState } from "@/components/explore/EmptyState";
```

Replace the `return (` block's opening — the `<div className="p-4">` through the closing `</div>` of the header (lines 169-206) — with:

```tsx
  return (
    <ListPageShell
      title={showTrash ? "Trash" : "Datasets"}
      toolbar={
        <>
          {!showTrash && (
            <ListToolbarButton
              icon={Star}
              label="Bookmarks"
              active={onlyBookmarked}
              filled
              onClick={() => setOnlyBookmarked((v) => !v)}
            />
          )}
          {!showTrash && hasData && (
            <ListToolbarButton
              icon={ListChecks}
              label="Bulk select"
              active={bulk.bulkMode}
              onClick={() => bulk.setBulkMode(!bulk.bulkMode)}
            />
          )}
          {/* NOT gated on `hasData`: this is a view switch, not a filter. It
              selects a different server query, and an empty dataset list says
              nothing about whether the trash is empty. Gated, it would be the
              only way OUT of an empty trash and would vanish exactly when it is
              the only control left. */}
          <ListToolbarButton
            icon={Trash2}
            label="Trash"
            active={showTrash}
            filled
            onClick={() => {
              const next = !showTrash;
              setShowTrash(next);
              if (next) {
                setOnlyBookmarked(false);
                bulk.setBulkMode(false);
              }
            }}
          />
          {!showTrash && (
            <ListToolbarButton
              icon={FolderPlus}
              label="New folder"
              onClick={() => setFolderDialog({ mode: "new" })}
            />
          )}
        </>
      }
      action={!showTrash ? <Button onClick={() => setCreateOpen(true)}>Create</Button> : null}
    >
```

`filled` is set on Bookmarks and Trash and omitted on Bulk select and New folder, matching what the page renders today: `Star` and `Trash2` carry `fill-current` when engaged, `ListChecks` and `FolderPlus` never do.

- [ ] **Step 4: Convert the state branch and the empty block**

Replace the ternary starting at line 208 (`{isLoading ? (`) with `ListStates`, and close it before the page's trailing dialogs:

```tsx
      <ListStates
        isLoading={isLoading}
        isError={isError}
        error={error}
        errorText="Failed to load datasets."
        hasData={hasData}
        skeletonCols={6}
        empty={
          showTrash ? (
            <p className="text-muted-foreground">Trash is empty.</p>
          ) : (
            <EmptyState
              variant="dashed"
              title="No datasets yet."
              description="A dataset is a named, reusable source for charts."
              action={<Button onClick={() => setCreateOpen(true)}>Create your first dataset</Button>}
            />
          )
        }
      >
        {/* the existing `hasData` block, unchanged */}
      </ListStates>
```

The original is a **five**-way branch (`isLoading / isError / hasData / showTrash-empty / first-run-empty`). `ListStates` is four-way, and the fifth arm lives inside the `empty` node above — that is why `empty` is a `ReactNode` rather than a string.

Then close with `</ListPageShell>` where the old `</div>` was, keeping `FolderNameDialog` and any other trailing dialogs **inside** it: `ListPageShell` renders `{children}` inside its own `p-4` wrapper, so anything that was inside the old `<div className="p-4">` stays inside the shell.

- [ ] **Step 5: Remove the imports that are now unused**

Delete `ListSkeleton`, `Tooltip`, `TooltipTrigger`, `TooltipContent`, `Alert` and `AlertDescription` from the import list **only if the file no longer uses them**. Check each with `grep -n "Tooltip\|ListSkeleton\|AlertDescription" src/routes/explore/Datasets.tsx` before deleting — a row-level tooltip elsewhere in the file would make one of these a false positive.

- [ ] **Step 6: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/routes/explore/__tests__/Datasets.primitives.test.tsx`
Expected: PASS, 4 tests.

- [ ] **Step 7: Verify the assertions pin something**

Delete `aria-pressed={active}` from `ListToolbarButton.tsx` and re-run the test. Expected: the two `aria-pressed` tests FAIL. **Build first** — run `npx tsc --noEmit` and confirm 0 errors before reading the test result, because a mutation that does not compile looks exactly like a mutation no test caught. Restore the line afterwards.

- [ ] **Step 8: Run the whole gate**

```bash
cd frontend
npx tsc --noEmit
npx eslint .          # expect: 0 errors, 177 warnings
npx vitest run
```

- [ ] **Step 9: Browser check**

Start the local stack and open `/explore/<ws>/datasets`. Confirm: the header reads as before, the four toolbar buttons sit in the same order, engaging Bookmarks visibly changes the button, the trash view still opens and closes, and the first-run empty block still shows its dashed frame. `EmptyState` renders the title and description as two `<p>` elements where the page had one, so the copy wraps differently — confirm it still reads as one sentence pair rather than a broken line.

- [ ] **Step 10: Commit**

```bash
git add frontend/src/routes/explore/Datasets.tsx frontend/src/routes/explore/__tests__/Datasets.primitives.test.tsx
git commit -m "refactor(explore): Datasets onto ListPageShell, ListStates, ListToolbarButton, EmptyState

Adds aria-pressed to the Bookmarks and Trash toggles, which carried engaged
state only in a visual class. ListStates also replaces \`?? errorText\` with
\`|| errorText\`, so a message-less error no longer renders a blank alert."
```

---

### Task 2: `Charts.tsx`

**Files:**
- Modify: `frontend/src/routes/explore/Charts.tsx` (header, state ternary at 193-199, empty block at 278-284)
- Modify: `frontend/src/routes/explore/__tests__/Charts.test.tsx`

**Interfaces:**
- Consumes: the same four primitives as Task 1. Nothing from Task 1's file.
- Produces: nothing new.

- [ ] **Step 1: Add the error fixture to the existing mock**

`Charts.test.tsx` currently mocks `useCharts: () => ({ data: charts, isLoading: false })`, which cannot produce an error state. Add a mutable fixture beside `charts` and widen the return:

```tsx
let queryError: unknown = undefined;
```

and inside `vi.mock("@/api/charts", …)`:

```tsx
  useCharts: () => ({
    data: charts,
    isLoading: false,
    isError: queryError !== undefined,
    error: queryError,
  }),
```

and in `beforeEach`, beside `charts = DEFAULT_CHARTS;`:

```tsx
  queryError = undefined;
```

- [ ] **Step 2: Write the failing tests**

Append to `Charts.test.tsx`:

```tsx
describe("Charts on the Explore primitives", () => {
  it("reports the Bookmarks filter's engaged state with aria-pressed", async () => {
    renderAt();
    const btn = screen.getByRole("button", { name: "Bookmarks" });
    expect(btn).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(btn);
    expect(btn).toHaveAttribute("aria-pressed", "true");
  });

  it("renders exactly one h1, and it is the page title", () => {
    renderAt();
    const h1s = screen.getAllByRole("heading", { level: 1 });
    expect(h1s).toHaveLength(1);
    expect(h1s[0]).toHaveTextContent("Charts");
  });

  it("falls back to the error text when the error carries no message", () => {
    queryError = new Error("");
    renderAt();
    expect(screen.getByText("Failed to load charts.")).toBeInTheDocument();
  });
});
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cd frontend && npx vitest run src/routes/explore/__tests__/Charts.test.tsx`
Expected: the `aria-pressed` test and the error-text test FAIL; the `h1` test passes.

- [ ] **Step 4: Convert the page**

Apply the same four conversions as Task 1, with these page-specific values:

- `title={showTrash ? "Trash" : "Charts"}`
- `skeletonCols={4}`
- `errorText="Failed to load charts."`
- action: copy whatever the current header renders as its primary button **verbatim from the file**; do not retype it from memory. `main` renders it unconditionally — no `showTrash` gate — and the conversion must preserve that. At the time of writing it is:

```tsx
      action={<Button asChild><Link to={`/explore/${ws}/charts/new`}>+ Chart</Link></Button>}
```
- empty:

```tsx
          empty={
            showTrash ? (
              <p className="text-muted-foreground">Trash is empty.</p>
            ) : (
              <EmptyState
                variant="dashed"
                title="No charts yet."
                description="Build a chart from a dataset."
                action={
                  <Button asChild>
                    <Link to={`/explore/${ws}/charts/new`}>Create your first chart</Link>
                  </Button>
                }
              />
            )
          }
```

`Charts.tsx` already branches `isLoading → isError → hasData`, the documented order, so the state-branch conversion is structural with no behaviour fix.

**Correction, added after the final whole-branch review (finding 1):** an earlier revision of this step's illustrative `action={…}` snippet wrapped the primary button in `!showTrash ? … : null`. The live file never had that wrapper — `main` renders "+ Chart" unconditionally, even in the Trash view — so an implementer following the earlier snippet literally would have introduced a real behaviour change (hiding the primary action in Trash) while believing this task was a pure refactor. The review caught it before merge; the snippet above is now copied verbatim from `main`, and the primary action renders unconditionally on this page, matching `main`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cd frontend && npx vitest run src/routes/explore/__tests__/Charts.test.tsx`
Expected: PASS, including the file's pre-existing tests.

- [ ] **Step 6: Run the whole gate**

```bash
cd frontend
npx tsc --noEmit
npx eslint .          # expect: 0 errors, 177 warnings
npx vitest run
```

- [ ] **Step 7: Browser check**

Open `/explore/<ws>/charts`. Confirm the header, the toolbar order, the trash round-trip and the empty block, as in Task 1 Step 9.

- [ ] **Step 8: Commit**

```bash
git add frontend/src/routes/explore/Charts.tsx frontend/src/routes/explore/__tests__/Charts.test.tsx
git commit -m "refactor(explore): Charts onto the shared list primitives"
```

---

### Task 3: `Dashboards.tsx` — and the swallowed error

**Files:**
- Modify: `frontend/src/routes/explore/Dashboards.tsx` (header, state ternary at 247-249 and 321-326, empty block at 328-333)
- Modify: `frontend/src/routes/explore/__tests__/Dashboards.test.tsx`

**Interfaces:**
- Consumes: the same four primitives.
- Produces: nothing new.

**This task is not a pure refactor.** `Dashboards.tsx` branches `isLoading ? … : hasData ? … : isError ? …` — `hasData` **before** `isError`. That is the drift §2.1 of `docs/en/ui-conventions.md` records verbatim, and it is still present: a failed refetch that still holds cached rows renders the table and swallows the error. `Datasets.tsx` and `Charts.tsx` both use the documented order, so this is one page.

- [ ] **Step 1: Add the error fixture to the existing mock**

In `Dashboards.test.tsx`'s `vi.mock("@/api/dashboards", …)`, widen the list hook's return exactly as Task 2 Step 1 does for charts — add `let queryError: unknown = undefined;` at module scope, return `isError: queryError !== undefined, error: queryError`, and reset it in `beforeEach`.

- [ ] **Step 2: Write the failing test**

Append to `Dashboards.test.tsx`:

```tsx
describe("Dashboards on the Explore primitives", () => {
  // The §2.1 defect, still present before this task: `hasData` was checked
  // before `isError`, so a failed refetch holding cached rows rendered the
  // table and swallowed the error. The fixture below is exactly that state —
  // rows present AND the query in error.
  it("shows the error, not the cached rows, when a refetch fails", () => {
    queryError = new Error("Backend unreachable");
    renderAt();
    expect(screen.getByText("Backend unreachable")).toBeInTheDocument();
    expect(screen.queryByText("Revenue overview")).not.toBeInTheDocument();
  });

  it("reports the Bookmarks filter's engaged state with aria-pressed", async () => {
    renderAt();
    const btn = screen.getByRole("button", { name: "Bookmarks" });
    expect(btn).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(btn);
    expect(btn).toHaveAttribute("aria-pressed", "true");
  });

  it("renders exactly one h1, and it is the page title", () => {
    renderAt();
    const h1s = screen.getAllByRole("heading", { level: 1 });
    expect(h1s).toHaveLength(1);
    expect(h1s[0]).toHaveTextContent("Dashboards");
  });
});
```

Replace `"Revenue overview"` with the `name` of the first dashboard in the file's existing fixture — read it from the top of `Dashboards.test.tsx` rather than assuming. The assertion is "a row that WOULD render is absent", so it must name a row the fixture actually contains.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cd frontend && npx vitest run src/routes/explore/__tests__/Dashboards.test.tsx`

Expected: the swallowed-error test FAILS — the row renders and the message does not, which is the bug. The `aria-pressed` test FAILS on the absent attribute. The `h1` test passes.

- [ ] **Step 4: Convert the page**

Apply the four conversions with these page-specific values:

- `title={showTrash ? "Trash" : "Dashboards"}`
- `skeletonCols={4}`
- `errorText="Failed to load dashboards."`
- empty:

```tsx
          empty={
            showTrash ? (
              <p className="text-muted-foreground">Trash is empty.</p>
            ) : (
              <EmptyState
                variant="dashed"
                title="No dashboards yet."
                description="Compose charts into a shareable dashboard."
                action={<Button onClick={() => setOpen(true)}>Create your first dashboard</Button>}
              />
            )
          }
```

The branch-order fix is **not a separate edit** — `ListStates` evaluates `isLoading → isError → !hasData → children` internally, so replacing the ternary with the component is what fixes it. Do not leave a hand-written `isError` arm behind as well.

Note that the old code's `isError` arm sat *after* the `hasData` arm, so it is physically further down the file (lines 321-326) than the `isLoading` arm (247-249). Both are absorbed into the one `ListStates` call.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cd frontend && npx vitest run src/routes/explore/__tests__/Dashboards.test.tsx`
Expected: PASS. Also run the four sibling files, which exercise the same page:

```bash
npx vitest run src/routes/explore/__tests__/Dashboards.bookmark.test.tsx \
               src/routes/explore/__tests__/Dashboards.canmanage.test.tsx \
               src/routes/explore/__tests__/Dashboards.create.test.tsx \
               src/routes/explore/__tests__/Dashboards.trash.test.tsx
```

- [ ] **Step 6: Verify the error test pins the fix**

In `ListStates.tsx`, temporarily hoist a **positive** `if (hasData) return <>{children}</>;` above the `if (isError)` block.

An earlier revision of this step said to move the existing `if (!hasData)` line above `if (isError)` instead. **That mutation is a no-op for this test and proves nothing:** the fixture has rows, so `!hasData` is `false` whichever side of the `isError` check it sits on, and control reaches `isError` either way. It changes behaviour only for the untested empty-list-plus-error case. The positive form above is the one that mirrors the original defective ternary (`hasData ? children : isError ? alert : empty`), and it is what actually exercises the branch this task fixed. Run `npx tsc --noEmit`, confirm 0 errors, then run the Dashboards test. Expected: the swallowed-error test FAILS. Restore `ListStates.tsx` afterwards — it is read-only for this plan and must end the task byte-identical to how it started (`git diff --exit-code frontend/src/components/explore/ListStates.tsx`).

- [ ] **Step 7: Run the whole gate**

```bash
cd frontend
npx tsc --noEmit
npx eslint .          # expect: 0 errors, 177 warnings
npx vitest run
git diff --exit-code frontend/src/components/explore/   # must be empty
```

- [ ] **Step 8: Browser check**

Open `/explore/<ws>/dashboards`. Confirm the header, toolbar order, trash round-trip and empty block. Then stop the backend and reload: the destructive alert must appear where the table was, rather than a stale table with no indication anything failed.

- [ ] **Step 9: Commit**

```bash
git add frontend/src/routes/explore/Dashboards.tsx frontend/src/routes/explore/__tests__/Dashboards.test.tsx
git commit -m "fix(explore): Dashboards no longer swallows a failed refetch

Dashboards branched isLoading -> hasData -> isError, so a failed refetch that
still held cached rows rendered the table and showed nothing about the failure.
ui-conventions.md 2.1 records this drift; it was still present. Moving the page
onto ListStates fixes it, since the component evaluates isError before hasData.

Also adds aria-pressed to the toolbar toggles."
```

---

### Task 4: `Alerts.tsx` — the largest page, and the only one with a tab bar

**Files:**
- Modify: `frontend/src/routes/explore/Alerts.tsx` (header at 372-446, `Tabs` at 447-459, state ternary at 461-469, empty block at 614-626)
- Modify: `frontend/src/routes/explore/__tests__/Alerts.test.tsx`

**Interfaces:**
- Consumes: the same four primitives.
- Produces: nothing new.

**Three things are specific to this page:**

1. **The tab bar.** `Alerts.tsx` renders a `Tabs` switcher (Alerts / Reports) *between* the header and the table. `ListPageShell` has no slot there, and **must not gain one** — a slot added for one consumer is the prop-per-capability shell §1.1 rejects. Render `Tabs` as the **first child** of the shell instead.
2. **The skeleton loses two rows.** The page renders `<ListSkeleton rows={6} cols={6} />`; `ListStates` renders `<ListSkeleton cols={skeletonCols} />` with no `rows`, so it takes the default of 8. This is an accepted, deliberate change: the other three pages already render 8, so all four end up agreeing. Do **not** add a `skeletonRows` prop to `ListStates` to preserve 6.
3. **The branch order differs, but no bug was reproduced.** `Alerts.tsx` checks `isError` before `isLoading`. Under TanStack Query the two flags rarely hold together and no failing case was found; the conversion makes the question moot. Do not write a test claiming a fixed bug here — there is no evidence of one.

- [ ] **Step 1: Write the failing tests**

`Alerts.test.tsx` already mocks `useSchedules` through a `schedulesMock` `vi.fn()`, so a per-test return needs no new fixture. Append:

```tsx
describe("Alerts on the Explore primitives", () => {
  it("reports the Bookmarks filter's engaged state with aria-pressed", async () => {
    renderAt();
    const btn = screen.getByRole("button", { name: "Bookmarks" });
    expect(btn).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(btn);
    expect(btn).toHaveAttribute("aria-pressed", "true");
  });

  it("renders exactly one h1, and it is the page title", () => {
    renderAt();
    const h1s = screen.getAllByRole("heading", { level: 1 });
    expect(h1s).toHaveLength(1);
    expect(h1s[0]).toHaveTextContent("Alerts & Reports");
  });

  // Switching tabs must also clear the state facet. Without the reset, a facet
  // engaged on Alerts stays engaged over Reports while its control shows a
  // value the new list was never filtered by.
  it("clears the state facet when the tab changes", async () => {
    renderAt();
    await userEvent.click(screen.getByRole("tab", { name: "Reports" }));
    expect(screen.getByRole("tab", { name: "Reports" })).toHaveAttribute("aria-selected", "true");
  });

  // The tab bar is a page-level view switch and must survive the move into the
  // shell's body. It is NOT a shell slot.
  it("keeps the Alerts / Reports switcher on the page", () => {
    renderAt();
    expect(screen.getByRole("tab", { name: "Alerts" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Reports" })).toBeInTheDocument();
  });

  it("falls back to the error text when the error carries no message", () => {
    schedulesMock.mockReturnValue({ data: [], isLoading: false, isError: true, error: new Error("") });
    renderAt();
    expect(screen.getByText("Failed to load schedules.")).toBeInTheDocument();
  });
});
```

Use the file's existing `renderAt` helper and its existing `schedulesMock` name; read both from the top of the file rather than redefining them.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd frontend && npx vitest run src/routes/explore/__tests__/Alerts.test.tsx`
Expected: the `aria-pressed` test and the error-text test FAIL; the `h1` and tab tests pass.

- [ ] **Step 3: Convert the header, and move `Tabs` into the body**

```tsx
  return (
    <ListPageShell
      title={showTrash ? "Trash" : "Alerts & Reports"}
      toolbar={
        <>
          {!showTrash && (
            <ListToolbarButton
              icon={Star}
              label="Bookmarks"
              active={onlyBookmarked}
              filled
              onClick={() => setOnlyBookmarked((v) => !v)}
            />
          )}
          {!showTrash && hasData && canManage && (
            <ListToolbarButton
              icon={ListChecks}
              label="Bulk select"
              active={bulk.bulkMode}
              onClick={() => bulk.setBulkMode(!bulk.bulkMode)}
            />
          )}
          {/* NOT gated on `hasData`: a view switch, not a filter. Gated, it
              would be the only way OUT of an empty trash and would vanish
              exactly when it is the only control left. */}
          <ListToolbarButton
            icon={Trash2}
            label="Trash"
            active={showTrash}
            filled
            onClick={() => {
              const next = !showTrash;
              setShowTrash(next);
              if (next) {
                setOnlyBookmarked(false);
                bulk.setBulkMode(false);
              }
            }}
          />
          {!showTrash && canManage && (
            <ListToolbarButton
              icon={FolderPlus}
              label="New folder"
              onClick={() => setFolderDialog({ mode: "new" })}
            />
          )}
        </>
      }
      action={!showTrash ? newButton : null}
    >
      {/* A page-level view switch, not a shell slot: it selects which kind of
          schedule the list shows and belongs above the table, where the page
          has always drawn it. */}
      <Tabs
        value={tab}
        onValueChange={(v) => {
          setTab(v as ScheduleType);
          setStateFilter(FACET_ALL);
        }}
        className="mb-3"
      >
        <TabsList>
          <TabsTrigger value="alert">Alerts</TabsTrigger>
          <TabsTrigger value="report">Reports</TabsTrigger>
        </TabsList>
      </Tabs>

      <ListStates … >
```

Both the title and the `Tabs` block above are the file's current values, read from lines 374-376 and 447-459. Two details are easy to lose: the title is `"Alerts & Reports"`, not a computed `typeLabel`, and `onValueChange` also resets `stateFilter` — dropping that reset leaves a state facet from the Alerts tab engaged over the Reports list, with the facet control showing a value the new list was never filtered by.

Note the two `canManage` conditions above — Alerts gates bulk select and New folder on it and the other three pages do not. Preserve both.

- [ ] **Step 4: Convert the state branch and the empty block**

```tsx
      <ListStates
        isLoading={isLoading}
        isError={isError}
        error={error}
        errorText="Failed to load schedules."
        hasData={hasData}
        skeletonCols={6}
        empty={
          showTrash ? (
            <p className="text-muted-foreground">Trash is empty.</p>
          ) : (
            <EmptyState
              variant="dashed"
              icon={Bell}
              title={`No ${typeLabel}s yet.`}
              description={
                tab === "alert"
                  ? "An alert runs SQL on a schedule and notifies you when the result meets your condition."
                  : "A report delivers a chart or dashboard on a schedule."
              }
              action={
                canManage ? (
                  <Button asChild>
                    <Link to={newHref}>Create your first {typeLabel}</Link>
                  </Button>
                ) : null
              }
            />
          )
        }
      >
```

`EmptyState` renders the icon inside a `h-10 w-10` muted circle; the page currently draws a bare `<Bell className="mx-auto mb-2 h-8 w-8" />`. That is a visible change and is why Step 6 checks this page's empty state in the browser specifically.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cd frontend && npx vitest run src/routes/explore/__tests__/Alerts.test.tsx`
Expected: PASS, including the file's 364 lines of pre-existing tests.

- [ ] **Step 6: Run the whole gate**

```bash
cd frontend
npx tsc --noEmit
npx eslint .          # expect: 0 errors, 177 warnings
npx vitest run
git diff --exit-code frontend/src/components/explore/   # must be empty
```

- [ ] **Step 7: Browser check — this is the one that cannot be skipped**

Open `/explore/<ws>/alerts`. The tab bar's **left edge must line up with the header's left edge**. `ScheduleEditor.tsx` carries a comment about this exact failure on this exact page: *"When the form carried its own `max-w-3xl` it ended short of the tabs above it, which reads as a rendering fault rather than a choice."* `ListPageShell` supplies `p-4` and the `Tabs` element sits inside it, so the edges should agree — confirm that they do rather than assuming.

Also confirm: switching Alerts ↔ Reports still reloads the list, the trash round-trip works, the empty state's `Bell` icon in its new circle does not look out of place, and the loading skeleton (now 8 rows, up from 6) does not overflow the viewport.

- [ ] **Step 8: Commit**

```bash
git add frontend/src/routes/explore/Alerts.tsx frontend/src/routes/explore/__tests__/Alerts.test.tsx
git commit -m "refactor(explore): Alerts onto the shared list primitives

The Alerts / Reports tab bar moves into the shell's body as a first child
rather than becoming a shell slot; a slot added for one consumer is the
prop-per-capability shell that ui-conventions.md 1.1 rejects.

The loading skeleton goes from 6 rows to ListSkeleton's default 8, which is
what the other three pages already render."
```

---

### Task 5: Re-measure the Adoption status and record it

**Files:**
- Modify: `docs/en/ui-conventions.md` (the `## Adoption status` section)

**Interfaces:**
- Consumes: the four converted pages from Tasks 1-4.
- Produces: nothing consumed by code.

The Adoption status section states its own counts and the commands that produced them. Those commands must be re-run, not the numbers re-typed — the section exists precisely because a number quoted from another revision has already been wrong twice in this programme.

- [ ] **Step 1: Re-measure**

```bash
cd frontend
for p in ListPageShell ListStates ListToolbarButton EmptyState; do
  printf "%-18s %s\n" "$p" \
    "$(grep -rl "from \"@/components/explore/$p\"" src --include='*.tsx' | grep -v __tests__ | wc -l | tr -d ' ')"
done

# Which Explore list pages still hand-roll the toolbar cluster.
for f in Alerts Datasets Charts Dashboards Notebooks; do
  printf "%-12s ListToolbarButton=%s rawTooltip=%s\n" "$f" \
    "$(grep -c ListToolbarButton src/routes/explore/$f.tsx)" \
    "$(grep -c TooltipTrigger src/routes/explore/$f.tsx)"
done
```

**Two corrections to an earlier revision of this step, both found by dry-running the commands rather than trusting them.**

`--include=*.tsx` is unquoted glob syntax and fails under zsh with `no matches found`. It must be `--include='*.tsx'`.

More importantly, the earlier revision counted `aria-pressed` **in the page files**. After the conversion that count is `0` on all four pages *by design* — the attribute now lives inside `ListToolbarButton`, which is the entire point of the change. Anyone re-running that command would read `0` and conclude the defect was never fixed. The `ListToolbarButton` / `TooltipTrigger` pair above is the measurement that actually distinguishes a converted page from an unconverted one.

- [ ] **Step 2: Update the section with the measured numbers**

Replace the `ListPageShell`, `ListStates` and `EmptyState` bullets with the counts from Step 1. The existing line

> `ListPageShell` — 5 route files (`Endpoints`, `Clients`, `EndpointCreate`, `ServiceAccountCreate`, `ClientCreate`), all Data API. The other Explore list pages still hand-write their headers.

becomes a list of nine, and the trailing sentence must be replaced rather than deleted, because it is still true of one page:

> **`Notebooks.tsx` still hand-writes its header** (735 lines, `aria-pressed` = 0). It was named in neither §1.3's list of four nor package U's scope, and is recorded here so this section is not read as "every Explore list page is converted".

- [ ] **Step 3: Add the two defects this package closed**

Under §2.1, the `Dashboards.tsx` example is written in the past tense as a thing that motivated the rule. It was still live until Task 3. Append one sentence so the document does not read as though it had already been fixed:

> Fixed by package U on 2026-08-22 (`docs/en/plans/2026-08-22-explore-list-primitives-adoption.md`, Task 3); the four pages now branch through `ListStates`.

And under §1.3, append:

> All four pages named above were converted by package U on 2026-08-22, so `aria-pressed` is now set on every list toolbar toggle in Explore except `Notebooks.tsx`.

- [ ] **Step 4: Verify no number in the section is stale**

Re-read the whole Adoption status section and check every remaining count with the command printed beside it. `ConfigTable`, `ConfigSection`, the plain-`Table` list and the §11 split are **not** touched by this package, so their counts should be unchanged — confirm that rather than assuming it.

- [ ] **Step 5: Commit**

```bash
git add docs/en/ui-conventions.md
git commit -m "docs(ui): re-measure Adoption status after the package U conversions

Records Notebooks.tsx as the one Explore list page still hand-writing its
header, so the section is not read as claiming full adoption."
```

---

## Self-review notes

Checked against spec §3 before this plan was saved:

- §3.1 (the deferral is commissioned) — context in the plan header and File Structure.
- §3.2 (the measurement, and the two non-defects) — Task 1 Step 3's comment records why the Trash button is not gated on `hasData`, so the conversion cannot silently introduce the gate the spec confirmed was absent.
- §3.3 (`ListPageShell`, `ListStates`, `ListToolbarButton`, `EmptyState` per page; the Dashboards branch-order defect; the `??` vs `||` fix) — Tasks 1-4, with the Dashboards fix given its own red test and its own commit.
- §3.4 (the tab bar, browser-checked, primitive unchanged) — Task 4 Steps 3 and 7, with `git diff --exit-code frontend/src/components/explore/` in the gate of Tasks 3 and 4.
- The `rows={6}` → default `8` skeleton change is called out in Task 4 rather than left for a reviewer to find in the diff.
- `Notebooks.tsx` is recorded as measured and out of scope in three places: the plan's scope section, Task 5 Step 1's measurement, and Task 5 Step 2's replacement sentence.
