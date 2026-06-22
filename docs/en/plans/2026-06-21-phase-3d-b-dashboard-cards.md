# Phase 3d-B — Dashboard cards + polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add text/heading/link cards, per-card titles, duplicate/replace, and URL-synced auto-refresh + fullscreen to egret dashboards — frontend only, on top of Phase A.

**Architecture:** A `DashboardCardView` dispatcher renders each card by `card_type` (chart → existing query+ChartRenderer with a title override; text/heading/link → small pure components). Edit mode gains add-buttons + inline option editors + duplicate/replace, all mutating the Phase A draft-cards and persisted by the existing whole-layout PUT. A `useDashboardViewControls` hook syncs `?refresh`/`?fullscreen` to the URL and ticks a refresh nonce threaded into `useChartData`.

**Tech Stack:** React 18 + TS + Vite + Vitest, `react-markdown` + `remark-gfm`, react-router `useSearchParams`, existing `ChartRenderer` / `useChartData` / `react-grid-layout`.

**Spec:** `docs/en/specs/2026-06-21-phase-3d-b-dashboard-cards-design.md`

---

## Conventions for the implementer

- Run all frontend commands from `frontend/`.
- Full sweep before each commit: `npm run typecheck && npm run lint && npm run build && npx vitest run`.
- Commit with the configured git identity — **never** `--author`. Footer every message:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- Stage only explicit paths. Branch is `feat/dashboards-phase-b` (already checked out).
- **No backend changes** — `dashboard_cards.card_type` + `options` jsonb already exist and the API already accepts `chart_id=null` cards (Phase A).

## Background: Phase A facts you build on

- `frontend/src/api/dashboards.ts` — `DashboardCard` = `{ id, chart_id: string|null, card_type: string, grid_col, grid_row, size_x, size_y, options: Record<string, unknown> }`; `CardInput` likewise (options optional). Whole-layout PUT save sends every card's `options`.
- `frontend/src/routes/explore/DashboardView.tsx` — holds `draftCards` (single source of truth), `addCard`, `removeCard`, `onLayoutChange`, `onSave`, `cardSeqRef`, and renders a `CardTile` (chart-only) per grid child. `cards = editing && draftCards ? draftCards : dashboard.cards`. The grid child wrapper is `<div className="relative overflow-hidden rounded-md border bg-background">` with a remove ✕ button shown when `editing`.
- `frontend/src/api/useChartData.ts` — `useChartData(ws, chart): ChartData`. Effect deps `[ws, specKey]`.
- `ChartRenderer` from `@/components/chart/ChartRenderer` — `{ vizType, params, columns, rows, metricTypes? }`.

## File structure

```
frontend/
  package.json                                   MODIFY  (react-markdown, remark-gfm)
  src/components/chart/dashboard/
    TextCard.tsx                                  NEW
    HeadingCard.tsx                               NEW
    LinkCard.tsx                                  NEW
    DashboardCardView.tsx                         NEW   (dispatcher + ChartCardTile)
    __tests__/cards.test.tsx                      NEW
    __tests__/DashboardCardView.test.tsx          NEW
  src/api/
    useChartData.ts                               MODIFY (optional nonce arg)
    useDashboardViewControls.ts                   NEW
    __tests__/useDashboardViewControls.test.tsx   NEW
  src/routes/explore/
    DashboardView.tsx                             MODIFY (dispatcher, add/edit/dup/replace, header controls)
    __tests__/DashboardView.test.tsx              MODIFY (new tests)
```

---

## Task 1: Install react-markdown + remark-gfm

**Files:** `frontend/package.json`, `frontend/package-lock.json`

- [ ] **Step 1: Install**

Run (from `frontend/`):
```bash
npm install react-markdown@^9 remark-gfm@^4
```

- [ ] **Step 2: Verify build**

Run: `npm run build`
Expected: clean (no usage yet; confirms the deps resolve under Vite/React 18).

- [ ] **Step 3: Commit**

```bash
git add package.json package-lock.json
git commit -m "build(deps): add react-markdown + remark-gfm

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: TextCard / HeadingCard / LinkCard components

Three small pure presentational components. TDD: test first.

**Files:**
- Create: `frontend/src/components/chart/dashboard/TextCard.tsx`, `HeadingCard.tsx`, `LinkCard.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/cards.test.tsx`

- [ ] **Step 1: Write the failing test**

```tsx
// frontend/src/components/chart/dashboard/__tests__/cards.test.tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { TextCard } from "../TextCard";
import { HeadingCard } from "../HeadingCard";
import { LinkCard } from "../LinkCard";

describe("TextCard", () => {
  it("renders markdown (heading, bold, list)", () => {
    render(<TextCard text={"# Title\n\n- **a**\n- b"} />);
    expect(screen.getByRole("heading", { name: "Title" })).toBeInTheDocument();
    expect(screen.getByText("a").tagName).toBe("STRONG");
    expect(screen.getAllByRole("listitem")).toHaveLength(2);
  });
  it("renders nothing visible for empty text", () => {
    const { container } = render(<TextCard text="   " />);
    expect(container.textContent?.trim()).toBe("");
  });
});

describe("HeadingCard", () => {
  it("renders a single-line heading text", () => {
    render(<HeadingCard text="Quarterly KPIs" />);
    expect(screen.getByText("Quarterly KPIs")).toBeInTheDocument();
  });
});

describe("LinkCard", () => {
  it("renders an external link with label, opening a new tab", () => {
    render(<LinkCard url="https://example.com" label="Docs" />);
    const a = screen.getByRole("link", { name: "Docs" });
    expect(a).toHaveAttribute("href", "https://example.com");
    expect(a).toHaveAttribute("target", "_blank");
    expect(a).toHaveAttribute("rel", "noreferrer");
  });
  it("falls back to the url as label when no label", () => {
    render(<LinkCard url="https://example.com" />);
    expect(screen.getByRole("link", { name: "https://example.com" })).toBeInTheDocument();
  });
  it("shows a placeholder (no link) when url is empty", () => {
    render(<LinkCard url="" />);
    expect(screen.queryByRole("link")).not.toBeInTheDocument();
    expect(screen.getByText(/no url/i)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npx vitest run src/components/chart/dashboard/__tests__/cards.test.tsx`
Expected: FAIL — modules not found.

- [ ] **Step 3: Implement the three components**

```tsx
// frontend/src/components/chart/dashboard/TextCard.tsx
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

/** A dashboard text card: GFM markdown. react-markdown does not render raw HTML, so this is XSS-safe. */
export function TextCard({ text }: { text: string }) {
  if (!text.trim()) return <div className="h-full p-2" />;
  return (
    <div className="dash-markdown h-full overflow-auto p-2 text-sm [&_h1]:mb-1 [&_h1]:text-lg [&_h1]:font-semibold [&_h2]:mb-1 [&_h2]:text-base [&_h2]:font-semibold [&_ul]:list-disc [&_ul]:pl-5 [&_ol]:list-decimal [&_ol]:pl-5 [&_a]:text-primary [&_a]:underline [&_code]:rounded [&_code]:bg-muted [&_code]:px-1">
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{text}</ReactMarkdown>
    </div>
  );
}
```

```tsx
// frontend/src/components/chart/dashboard/HeadingCard.tsx
/** A dashboard heading card: a single styled line, no card chrome. */
export function HeadingCard({ text }: { text: string }) {
  return (
    <div className="flex h-full items-center px-1 text-lg font-semibold text-foreground">
      {text || <span className="text-muted-foreground">Heading</span>}
    </div>
  );
}
```

```tsx
// frontend/src/components/chart/dashboard/LinkCard.tsx
/** A dashboard link card: a clickable tile to an external URL. */
export function LinkCard({ url, label }: { url?: string; label?: string }) {
  const href = (url ?? "").trim();
  if (!href) {
    return <div className="flex h-full items-center justify-center p-2 text-sm text-muted-foreground">No URL set</div>;
  }
  return (
    <a href={href} target="_blank" rel="noreferrer" className="flex h-full items-center gap-2 p-2 text-sm text-primary underline">
      {label?.trim() || href}
    </a>
  );
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `npx vitest run src/components/chart/dashboard/__tests__/cards.test.tsx`
Expected: PASS (6 assertions across 3 components).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/dashboard/TextCard.tsx frontend/src/components/chart/dashboard/HeadingCard.tsx frontend/src/components/chart/dashboard/LinkCard.tsx frontend/src/components/chart/dashboard/__tests__/cards.test.tsx
git commit -m "feat(dashboard): text, heading, and link card components

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `useChartData` optional refresh nonce

Add an optional `nonce` so a dashboard can force every card to re-run its query. Default behaviour unchanged.

**Files:**
- Modify: `frontend/src/api/useChartData.ts`
- Test: `frontend/src/api/__tests__/useChartData.test.tsx` (append a test)

- [ ] **Step 1: Write the failing test (append to the existing file)**

```tsx
it("refetches when the nonce changes", async () => {
  let nonce = 0;
  const { result, rerender } = renderHook(() => useChartData("eng", chart, nonce), { wrapper });
  await waitFor(() => expect(result.current.rows).toEqual([[5]]));
  const buildCalls = apiJson.mock.calls.filter((c: unknown[]) => String(c[0]).endsWith("/charts/data")).length;
  nonce = 1;
  rerender();
  await waitFor(() => {
    const after = apiJson.mock.calls.filter((c: unknown[]) => String(c[0]).endsWith("/charts/data")).length;
    expect(after).toBe(buildCalls + 1);   // build ran again
  });
});
```

(The existing test file already defines `chart`, `wrapper`, and the `apiJson` mock.)

- [ ] **Step 2: Run to verify it fails**

Run: `npx vitest run src/api/__tests__/useChartData.test.tsx`
Expected: FAIL — `useChartData` ignores the 3rd arg, so no second build call.

- [ ] **Step 3: Implement**

Change the signature and effect deps in `useChartData.ts`:

```tsx
export function useChartData(ws: string, chart: Chart | null, nonce?: number): ChartData {
```
and the effect dependency array (currently `[ws, specKey]`):
```tsx
  }, [ws, specKey, nonce]);
```

- [ ] **Step 4: Run to verify it passes**

Run: `npx vitest run src/api/__tests__/useChartData.test.tsx`
Expected: PASS (existing test + the new one).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/api/useChartData.ts frontend/src/api/__tests__/useChartData.test.tsx
git commit -m "feat(api): useChartData optional refresh nonce

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: `DashboardCardView` dispatcher

Move the chart-only `CardTile` out of `DashboardView` into a `DashboardCardView` that switches on `card_type` and adds the per-card **title override** (`options.title`; `""` hides the chart header).

**Files:**
- Create: `frontend/src/components/chart/dashboard/DashboardCardView.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/DashboardCardView.test.tsx`
- Modify: `frontend/src/routes/explore/DashboardView.tsx` (use the dispatcher; delete the local `CardTile`)

- [ ] **Step 1: Write the failing test**

```tsx
// frontend/src/components/chart/dashboard/__tests__/DashboardCardView.test.tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { DashboardCardView } from "../DashboardCardView";
import type { DashboardCard } from "@/api/dashboards";
import type { Chart } from "@/api/charts";

vi.mock("@/api/useChartData", () => ({
  useChartData: () => ({ columns: [{ name: "rev", type: "double" }], rows: [[5]], isLoading: false, error: null }),
}));
vi.mock("@/components/chart/ChartRenderer", () => ({ ChartRenderer: () => <div data-testid="chart" /> }));

const chart: Chart = {
  id: "c1", dataset_id: "ds1", name: "Sales", viz_type: "table", description: "",
  params: { spec: { metrics: [], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] } },
  created_by: null, updated_at: "",
};
const card = (over: Partial<DashboardCard>): DashboardCard => ({
  id: "k", chart_id: null, card_type: "chart", grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: {}, ...over,
});

describe("DashboardCardView", () => {
  it("renders a chart card with the chart name as header by default", () => {
    render(<DashboardCardView ws="eng" card={card({ chart_id: "c1" })} chart={chart} />);
    expect(screen.getByText("Sales")).toBeInTheDocument();
    expect(screen.getByTestId("chart")).toBeInTheDocument();
  });
  it("uses options.title to override the header", () => {
    render(<DashboardCardView ws="eng" card={card({ chart_id: "c1", options: { title: "Revenue" } })} chart={chart} />);
    expect(screen.getByText("Revenue")).toBeInTheDocument();
    expect(screen.queryByText("Sales")).not.toBeInTheDocument();
  });
  it("hides the header when options.title is empty string", () => {
    render(<DashboardCardView ws="eng" card={card({ chart_id: "c1", options: { title: "" } })} chart={chart} />);
    expect(screen.queryByText("Sales")).not.toBeInTheDocument();
    expect(screen.getByTestId("chart")).toBeInTheDocument();
  });
  it("routes a text card to markdown", () => {
    render(<DashboardCardView ws="eng" card={card({ card_type: "text", options: { text: "# Hi" } })} chart={undefined} />);
    expect(screen.getByRole("heading", { name: "Hi" })).toBeInTheDocument();
  });
  it("routes a heading card", () => {
    render(<DashboardCardView ws="eng" card={card({ card_type: "heading", options: { text: "Section" } })} chart={undefined} />);
    expect(screen.getByText("Section")).toBeInTheDocument();
  });
  it("routes a link card", () => {
    render(<DashboardCardView ws="eng" card={card({ card_type: "link", options: { url: "https://x.com", label: "X" } })} chart={undefined} />);
    expect(screen.getByRole("link", { name: "X" })).toHaveAttribute("href", "https://x.com");
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npx vitest run src/components/chart/dashboard/__tests__/DashboardCardView.test.tsx`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement the dispatcher**

```tsx
// frontend/src/components/chart/dashboard/DashboardCardView.tsx
import type { DashboardCard } from "@/api/dashboards";
import type { Chart } from "@/api/charts";
import { useChartData } from "@/api/useChartData";
import { ChartRenderer } from "@/components/chart/ChartRenderer";
import { TextCard } from "./TextCard";
import { HeadingCard } from "./HeadingCard";
import { LinkCard } from "./LinkCard";

function asString(v: unknown): string | undefined {
  return typeof v === "string" ? v : undefined;
}

function ChartCardTile({ ws, card, chart, refreshNonce }: { ws: string; card: DashboardCard; chart: Chart | undefined; refreshNonce?: number }) {
  const data = useChartData(ws, chart ?? null, refreshNonce);
  const titleOverride = asString(card.options.title);          // undefined → use chart name; "" → hide
  const showHeader = titleOverride !== "";
  const headerText = titleOverride || chart?.name || "";
  if (!chart) return <div className="flex h-full items-center justify-center text-sm text-muted-foreground">Chart not found</div>;
  return (
    <div className="flex h-full flex-col">
      {showHeader && <div className="border-b px-2 py-1 text-sm font-medium">{headerText}</div>}
      <div className="min-h-0 flex-1 overflow-auto p-1">
        {data.error ? <div className="text-sm text-destructive">{data.error}</div>
          : data.isLoading ? <div className="text-sm text-muted-foreground">Loading…</div>
          : <ChartRenderer vizType={chart.viz_type} params={chart.params} columns={data.columns} rows={data.rows} />}
      </div>
    </div>
  );
}

/** Render one dashboard card by its card_type. */
export function DashboardCardView({ ws, card, chart, refreshNonce }: { ws: string; card: DashboardCard; chart: Chart | undefined; refreshNonce?: number }) {
  if (card.card_type === "text") return <TextCard text={asString(card.options.text) ?? ""} />;
  if (card.card_type === "heading") return <HeadingCard text={asString(card.options.text) ?? ""} />;
  if (card.card_type === "link") return <LinkCard url={asString(card.options.url)} label={asString(card.options.label)} />;
  return <ChartCardTile ws={ws} card={card} chart={chart} refreshNonce={refreshNonce} />;
}
```

- [ ] **Step 4: Refactor `DashboardView` to use it**

In `DashboardView.tsx`: delete the local `CardTile` function (lines ~17-30) and its now-unused imports (`useChartData`, `ChartRenderer` — only if not otherwise used; the dispatcher owns them now). Add `import { DashboardCardView } from "@/components/chart/dashboard/DashboardCardView";`. Replace the grid-child body `<CardTile ws={ws} chart={chartById.get(c.chart_id ?? "")} />` with:
```tsx
<DashboardCardView ws={ws} card={c} chart={chartById.get(c.chart_id ?? "")} />
```
Also make the heading card border-less (spec: no card chrome) — change the grid-child wrapper className to:
```tsx
className={`relative overflow-hidden ${c.card_type === "heading" ? "" : "rounded-md border bg-background"}`}
```

- [ ] **Step 5: Run tests + sweep**

Run: `npx vitest run` then `npm run typecheck && npm run lint && npm run build`
Expected: the new dispatcher tests pass and ALL Phase A DashboardView tests still pass (behaviour unchanged for chart cards). Clean typecheck/lint/build (remove dead imports the linter flags).

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/dashboard/DashboardCardView.tsx frontend/src/components/chart/dashboard/__tests__/DashboardCardView.test.tsx frontend/src/routes/explore/DashboardView.tsx
git commit -m "feat(dashboard): card-type dispatcher with per-card title override

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Edit mode — add Text/Heading/Link + inline option editors

In edit mode: add three buttons to create non-chart cards, and render an inline editor per card so the user can fill its `options`.

**Files:**
- Modify: `frontend/src/routes/explore/DashboardView.tsx`
- Modify: `frontend/src/routes/explore/__tests__/DashboardView.test.tsx` (add tests)

- [ ] **Step 1: Write the failing tests (append)**

```tsx
describe("DashboardView (Phase B — card variety)", () => {
  it("adds a text card and edits its body", () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    fireEvent.click(screen.getByRole("button", { name: /add text/i }));
    const editor = screen.getByLabelText(/text body/i);
    fireEvent.change(editor, { target: { value: "## Notes" } });
    expect((editor as HTMLTextAreaElement).value).toBe("## Notes");
  });
  it("adds a heading card", () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    fireEvent.click(screen.getByRole("button", { name: /add heading/i }));
    expect(screen.getByLabelText(/heading text/i)).toBeInTheDocument();
  });
  it("adds a link card with url + label", () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    fireEvent.click(screen.getByRole("button", { name: /add link/i }));
    expect(screen.getByLabelText(/link url/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/link label/i)).toBeInTheDocument();
  });
  it("offers a title input on a chart card in edit mode", () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    expect(screen.getByLabelText(/card title/i)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npx vitest run src/routes/explore/__tests__/DashboardView.test.tsx`
Expected: FAIL — no add-text/heading/link buttons or editors yet.

- [ ] **Step 3: Implement add-helpers + option update**

In `DashboardView.tsx`, add near `addCard`:
```tsx
function addVirtualCard(card_type: "text" | "heading" | "link", options: Record<string, unknown>) {
  const maxY = cards.reduce((m, c) => Math.max(m, c.grid_row + c.size_y), 0);
  const newCard: Card = {
    id: `new-${card_type}-${cardSeqRef.current++}`, chart_id: null, card_type,
    grid_col: 0, grid_row: maxY, size_x: CARD_DEFAULT_SIZE.w, size_y: card_type === "heading" ? 1 : CARD_DEFAULT_SIZE.h, options,
  };
  setDraftCards([...cards, newCard]);
}
function updateCardOptions(cardId: string, patch: Record<string, unknown>) {
  setDraftCards(cards.map((c) => (c.id === cardId ? { ...c, options: { ...c.options, ...patch } } : c)));
}
```

Add the three buttons inside the existing edit-mode `<aside>` (after the saved-chart list `</div>`), as a second row:
```tsx
<div className="mt-2 flex gap-1 border-t pt-2">
  <Button size="sm" variant="outline" onClick={() => addVirtualCard("text", { text: "" })}>+ Text</Button>
  <Button size="sm" variant="outline" onClick={() => addVirtualCard("heading", { text: "" })}>+ Heading</Button>
  <Button size="sm" variant="outline" onClick={() => addVirtualCard("link", { url: "", label: "" })}>+ Link</Button>
</div>
```

- [ ] **Step 4: Implement inline editors**

In the grid-child map, render an editor above `<DashboardCardView>` when `editing`. Add this just inside the child `<div>` (after the remove button), keyed off `c.card_type`:
```tsx
{editing && c.card_type === "chart" && (
  <input aria-label="card title" placeholder="Title (blank = chart name)" className="m-1 w-[calc(100%-0.5rem)] rounded border px-1 text-xs"
    value={(c.options.title as string) ?? ""} onChange={(e) => updateCardOptions(c.id, { title: e.target.value })} />
)}
{editing && (c.card_type === "text") && (
  <textarea aria-label="text body" className="m-1 h-[calc(100%-1rem)] w-[calc(100%-0.5rem)] resize-none rounded border p-1 text-xs"
    value={(c.options.text as string) ?? ""} onChange={(e) => updateCardOptions(c.id, { text: e.target.value })} />
)}
{editing && c.card_type === "heading" && (
  <input aria-label="heading text" className="m-1 w-[calc(100%-0.5rem)] rounded border px-1 text-sm"
    value={(c.options.text as string) ?? ""} onChange={(e) => updateCardOptions(c.id, { text: e.target.value })} />
)}
{editing && c.card_type === "link" && (
  <div className="m-1 flex flex-col gap-1">
    <input aria-label="link url" placeholder="https://…" className="rounded border px-1 text-xs"
      value={(c.options.url as string) ?? ""} onChange={(e) => updateCardOptions(c.id, { url: e.target.value })} />
    <input aria-label="link label" placeholder="Label (optional)" className="rounded border px-1 text-xs"
      value={(c.options.label as string) ?? ""} onChange={(e) => updateCardOptions(c.id, { label: e.target.value })} />
  </div>
)}
{(!editing || c.card_type === "chart") && (
  <DashboardCardView ws={ws} card={c} chart={chartById.get(c.chart_id ?? "")} />
)}
```
(Chart cards show BOTH the title input and the live chart while editing; text/heading/link show the editor *instead of* the preview while editing — simpler and avoids a tall card. When not editing, all show `DashboardCardView`.)

- [ ] **Step 5: Run tests + sweep**

Run: `npx vitest run src/routes/explore/__tests__/DashboardView.test.tsx` then `npm run typecheck && npm run lint`
Expected: the 4 new tests pass; existing tests stay green; clean.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/routes/explore/DashboardView.tsx frontend/src/routes/explore/__tests__/DashboardView.test.tsx
git commit -m "feat(dashboard): add text/heading/link cards + inline editors in edit mode

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Duplicate + Replace card actions

Edit mode gains a duplicate (⧉) button on every card and a replace button on chart cards (swaps the chart, resets options, keeps position).

**Files:**
- Modify: `frontend/src/routes/explore/DashboardView.tsx`
- Modify: `frontend/src/routes/explore/__tests__/DashboardView.test.tsx`

- [ ] **Step 1: Write the failing tests (append)**

```tsx
describe("DashboardView (Phase B — duplicate/replace)", () => {
  it("duplicates a card", () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    const before = screen.getAllByRole("button", { name: /remove card/i }).length;
    fireEvent.click(screen.getAllByRole("button", { name: /duplicate card/i })[0]);
    const after = screen.getAllByRole("button", { name: /remove card/i }).length;
    expect(after).toBe(before + 1);
  });
  it("replaces a chart card's chart via the picker", () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    fireEvent.click(screen.getAllByRole("button", { name: /replace card/i })[0]);
    // picker dialog lists workspace charts; pick "Sales"
    fireEvent.click(screen.getByRole("button", { name: /^use sales$/i }));
    // still one card, still a chart (remove button present)
    expect(screen.getAllByRole("button", { name: /remove card/i }).length).toBeGreaterThan(0);
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npx vitest run src/routes/explore/__tests__/DashboardView.test.tsx`
Expected: FAIL — no duplicate/replace buttons.

- [ ] **Step 3: Implement duplicate + replace**

Add to `DashboardView.tsx` near `removeCard`:
```tsx
const [replacingId, setReplacingId] = useState<string | null>(null);

function duplicateCard(cardId: string) {
  const src = cards.find((c) => c.id === cardId);
  if (!src) return;
  const maxY = cards.reduce((m, c) => Math.max(m, c.grid_row + c.size_y), 0);
  setDraftCards([...cards, { ...src, id: `new-dup-${cardSeqRef.current++}`, grid_row: maxY, options: { ...src.options } }]);
}
function replaceCard(cardId: string, chart: Chart) {
  setDraftCards(cards.map((c) => (c.id === cardId ? { ...c, chart_id: chart.id, options: {} } : c)));
  setReplacingId(null);
}
```

Add duplicate + replace buttons next to the remove ✕ (inside the `{editing && (...)}` block in the grid child — make it a small button row):
```tsx
{editing && (
  <div className="absolute right-1 top-1 z-10 flex gap-1">
    <button type="button" aria-label={`duplicate card ${c.id}`} className="rounded bg-background/80 px-1 text-xs"
      onClick={() => duplicateCard(c.id)}>⧉</button>
    {c.card_type === "chart" && (
      <button type="button" aria-label={`replace card ${c.id}`} className="rounded bg-background/80 px-1 text-xs"
        onClick={() => setReplacingId(c.id)}>↺</button>
    )}
    <button type="button" aria-label={`remove card ${chartById.get(c.chart_id ?? "")?.name ?? ""}`.trim() || "remove card"}
      className="rounded bg-background/80 px-1 text-xs text-destructive" onClick={() => removeCard(c.id)}>✕</button>
  </div>
)}
```
(Replace the existing single remove-button block with this row.)

Add a replace-picker dialog at the end of the component (before the closing `</div>`), using the shadcn `Dialog` already imported in the project (import if needed: `import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";`):
```tsx
<Dialog open={replacingId !== null} onOpenChange={(o) => { if (!o) setReplacingId(null); }}>
  <DialogContent>
    <DialogHeader><DialogTitle>Replace chart</DialogTitle></DialogHeader>
    <div className="flex flex-wrap gap-1 py-2">
      {(charts ?? []).map((ch) => (
        <Button key={ch.id} size="sm" variant="outline" aria-label={`use ${ch.name}`} onClick={() => replaceCard(replacingId!, ch)}>
          {ch.name}
        </Button>
      ))}
    </div>
  </DialogContent>
</Dialog>
```

- [ ] **Step 4: Run tests + sweep**

Run: `npx vitest run src/routes/explore/__tests__/DashboardView.test.tsx` then `npm run typecheck && npm run lint`
Expected: the 2 new tests pass; existing green; clean.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/explore/DashboardView.tsx frontend/src/routes/explore/__tests__/DashboardView.test.tsx
git commit -m "feat(dashboard): duplicate + replace card actions

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: `useDashboardViewControls` hook (URL-synced refresh + fullscreen)

**Files:**
- Create: `frontend/src/api/useDashboardViewControls.ts`
- Test: `frontend/src/api/__tests__/useDashboardViewControls.test.tsx`

- [ ] **Step 1: Write the failing test**

```tsx
// frontend/src/api/__tests__/useDashboardViewControls.test.tsx
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import type { ReactNode } from "react";
import { useDashboardViewControls } from "../useDashboardViewControls";

function wrap(initial: string) {
  return ({ children }: { children: ReactNode }) => <MemoryRouter initialEntries={[initial]}>{children}</MemoryRouter>;
}

describe("useDashboardViewControls", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("parses ?refresh and ?fullscreen from the URL", () => {
    const { result } = renderHook(() => useDashboardViewControls(), { wrapper: wrap("/d?refresh=60&fullscreen=true") });
    expect(result.current.refreshSeconds).toBe(60);
    expect(result.current.isFullscreen).toBe(true);
  });

  it("treats a missing/invalid refresh as off", () => {
    const { result } = renderHook(() => useDashboardViewControls(), { wrapper: wrap("/d?refresh=abc") });
    expect(result.current.refreshSeconds).toBeNull();
    expect(result.current.isFullscreen).toBe(false);
  });

  it("ticks the refresh nonce on the interval", () => {
    const { result } = renderHook(() => useDashboardViewControls(), { wrapper: wrap("/d?refresh=1") });
    const start = result.current.refreshNonce;
    act(() => { vi.advanceTimersByTime(3000); });
    expect(result.current.refreshNonce).toBeGreaterThan(start);
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npx vitest run src/api/__tests__/useDashboardViewControls.test.tsx`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement**

```tsx
// frontend/src/api/useDashboardViewControls.ts
import { useEffect, useState } from "react";
import { useSearchParams } from "react-router-dom";

export interface DashboardViewControls {
  refreshSeconds: number | null;
  setRefreshSeconds: (s: number | null) => void;
  isFullscreen: boolean;
  setFullscreen: (v: boolean) => void;
  refreshNonce: number;
}

/** Dashboard view controls synced to the URL: ?refresh=<seconds> and ?fullscreen=true. */
export function useDashboardViewControls(): DashboardViewControls {
  const [params, setParams] = useSearchParams();
  const raw = params.get("refresh");
  const refreshSeconds = raw && /^\d+$/.test(raw) && Number(raw) > 0 ? Number(raw) : null;
  const isFullscreen = params.get("fullscreen") === "true";
  const [refreshNonce, setRefreshNonce] = useState(0);

  const setRefreshSeconds = (s: number | null) =>
    setParams((p) => { const n = new URLSearchParams(p); if (s) n.set("refresh", String(s)); else n.delete("refresh"); return n; }, { replace: true });
  const setFullscreen = (v: boolean) =>
    setParams((p) => { const n = new URLSearchParams(p); if (v) n.set("fullscreen", "true"); else n.delete("fullscreen"); return n; }, { replace: true });

  useEffect(() => {
    if (!refreshSeconds) return;
    const t = setInterval(() => setRefreshNonce((n) => n + 1), refreshSeconds * 1000);
    return () => clearInterval(t);
  }, [refreshSeconds]);

  return { refreshSeconds, setRefreshSeconds, isFullscreen, setFullscreen, refreshNonce };
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `npx vitest run src/api/__tests__/useDashboardViewControls.test.tsx`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/api/useDashboardViewControls.ts frontend/src/api/__tests__/useDashboardViewControls.test.tsx
git commit -m "feat(api): useDashboardViewControls — URL-synced refresh + fullscreen

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Header view controls — auto-refresh dropdown + fullscreen toggle

Wire the hook into `DashboardView`: a refresh dropdown and fullscreen toggle in the header (view mode only), the fullscreen CSS mode on the container, and `refreshNonce` threaded to each chart card.

**Files:**
- Modify: `frontend/src/routes/explore/DashboardView.tsx`
- Modify: `frontend/src/routes/explore/__tests__/DashboardView.test.tsx`

- [ ] **Step 1: Write the failing test (append)**

```tsx
describe("DashboardView (Phase B — view controls)", () => {
  it("shows an auto-refresh control and a fullscreen toggle in view mode", () => {
    renderAt();
    expect(screen.getByLabelText(/auto-refresh/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /fullscreen/i })).toBeInTheDocument();
  });
});
```
(`renderAt` already wraps in `MemoryRouter`, so `useSearchParams` works.)

- [ ] **Step 2: Run to verify it fails**

Run: `npx vitest run src/routes/explore/__tests__/DashboardView.test.tsx`
Expected: FAIL — no auto-refresh / fullscreen controls.

- [ ] **Step 3: Implement**

In `DashboardView.tsx`:
- Import the hook and a native-select element (plain `<select>` is fine — keep it dependency-light): `import { useDashboardViewControls } from "@/api/useDashboardViewControls";`
- Call it near the other hooks: `const view = useDashboardViewControls();`
- In the header `<div className="flex gap-2">`, BEFORE the Edit button and only when `!editing`, add:
```tsx
{!editing && (
  <>
    <label className="flex items-center gap-1 text-xs text-muted-foreground">
      <span className="sr-only">Auto-refresh</span>
      <select aria-label="auto-refresh" className="h-8 rounded border bg-background px-1 text-xs"
        value={view.refreshSeconds ?? 0}
        onChange={(e) => view.setRefreshSeconds(Number(e.target.value) || null)}>
        <option value={0}>No refresh</option>
        <option value={60}>1 min</option>
        <option value={300}>5 min</option>
        <option value={600}>10 min</option>
        <option value={900}>15 min</option>
        <option value={1800}>30 min</option>
        <option value={3600}>60 min</option>
      </select>
    </label>
    <Button size="sm" variant="outline" aria-label="fullscreen"
      onClick={() => {
        const next = !view.isFullscreen;
        view.setFullscreen(next);
        // best-effort native fullscreen on the user gesture
        try { if (next) document.documentElement.requestFullscreen?.(); else if (document.fullscreenElement) document.exitFullscreen?.(); } catch { /* ignore */ }
      }}>
      {view.isFullscreen ? "Exit fullscreen" : "Fullscreen"}
    </Button>
  </>
)}
```
- Wrap the outer container so fullscreen expands it. Change the root `return (<div className="p-4">` to:
```tsx
return (
  <div className={view.isFullscreen ? "fixed inset-0 z-50 overflow-auto bg-background p-4" : "p-4"}>
```
- Thread the nonce to chart cards: pass `refreshNonce={view.refreshNonce}` into `<DashboardCardView ... />`.

- [ ] **Step 4: Run tests + full sweep**

Run: `npm run typecheck && npm run lint && npm run build && npx vitest run`
Expected: all green (new test + all prior).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/explore/DashboardView.tsx frontend/src/routes/explore/__tests__/DashboardView.test.tsx
git commit -m "feat(dashboard): header auto-refresh dropdown + fullscreen toggle

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Live verification (Playwright) + PR

- [ ] **Step 1: Ensure the stack is running** (per the egret-frontend-viz-verify memory): docker deps, `cargo run -p egret-bin -- serve` (:8080), `npm run dev` (:5173, from `frontend/`). Log in `admin@egret.local` / `Admin12345!`, workspace `eng`. (No new migration this phase, so no backend rebuild needed beyond what's already running.)

- [ ] **Step 2: Manual flow** — open a dashboard → Edit → **+ Text** (type `## Notes\n- a\n- b`), **+ Heading** (type a title), **+ Link** (`https://example.com`, label "Docs"), add a chart, set that chart card's **title** and blank another to hide its header, **duplicate** a card, **replace** a chart card's chart → **Save** → reload. Then in view mode set **Auto-refresh = 1 min** (watch the network re-issue card queries within a minute) and click **Fullscreen**. Confirm: markdown renders, heading/link tiles render, titles apply/hide, layout + options persisted after reload, `?refresh=60`/`?fullscreen=true` appear in the URL, and `mcp__playwright__browser_console_messages` shows **0 errors**.

- [ ] **Step 3: Open the PR**

```bash
git push -u origin feat/dashboards-phase-b
gh pr create --base main --title "feat(dashboards): Phase B — text/heading/link cards, titles, duplicate/replace, refresh + fullscreen" --body "<summary + testing notes; reference docs/en/specs/2026-06-21-phase-3d-b-dashboard-cards-design.md; 🤖 Generated with [Claude Code](https://claude.com/claude-code)>"
```
Wait for CI; squash-merge.

---

## Acceptance (Phase B)

- Text (markdown), heading, and link cards can be added, edited, and persist; chart cards can have a custom title or a hidden header.
- Cards can be duplicated; chart cards can have their chart replaced (position kept, options reset).
- Auto-refresh re-runs all card queries on the chosen interval; fullscreen expands the dashboard; both round-trip through `?refresh`/`?fullscreen`.
- All Phase A behaviour intact; full sweep green; live Playwright check has 0 console errors.
