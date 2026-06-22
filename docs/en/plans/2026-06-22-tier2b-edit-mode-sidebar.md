# Tier 2b — Dashboard Edit-Mode Sidebar Overhaul Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the cluttered dashboard edit mode (inline per-card controls + a 47-button "Add a chart" wall) with a Metabase-style **right sidebar** and **clean cards**.

**Architecture:** A new `editPanel` state (`add` / `card` / `filter`) drives a right `EditSidebar` that renders one of three panels. The per-card inline controls move into `CardSettingsPanel`; the filter editor into `FilterSettingsPanel`; adding into a searchable `AddPanel`. Cards become clean with a hover `CardEditToolbar`. All existing draft mutators are reused unchanged — only **where** they're triggered changes.

**Tech Stack:** React 18 + TS + Vite + Vitest; existing Radix primitives (`scroll-area`, `input`, `button`, `select`) — no new deps.

**Spec:** `docs/en/specs/2026-06-22-tier2b-edit-mode-sidebar-design.md`

**Branch:** `feat/dashboards-tier2b-edit-ux` (off main).

## Conventions (every task)

- TDD: failing test → run (fail) → implement → run (pass) → commit.
- **Full sweep before each commit (from `frontend/`):** `npm run typecheck && npm run lint && npm run build && npx vitest run`.
- Configured git identity (never `--author`); body footer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Stage explicit paths.
- Clean-room (no AGPL); Metabase as the visual standard.

## File structure

- `frontend/src/components/chart/dashboard/CardEditToolbar.tsx` (Create) — hover toolbar.
- `frontend/src/components/chart/dashboard/EditFilterChips.tsx` (Create) — edit-mode filter chips.
- `frontend/src/components/chart/dashboard/FilterSettingsPanel.tsx` (Create) — filter config panel.
- `frontend/src/components/chart/dashboard/CardSettingsPanel.tsx` (Create) — card config panel.
- `frontend/src/components/chart/dashboard/AddPanel.tsx` (Create) — searchable add panel.
- `frontend/src/components/chart/dashboard/EditSidebar.tsx` (Create) — sidebar switch + `EditPanel` type.
- `frontend/src/components/chart/dashboard/DashboardCardView.tsx` (Modify) — render virtual-card displays in edit too.
- `frontend/src/routes/explore/DashboardView.tsx` (Modify) — `editPanel` state, top-bar, clean cards, sidebar layout, chips.
- Tests under `…/dashboard/__tests__/` and `…/explore/__tests__/`.

The new components are presentational (props in, callbacks out) — `DashboardView` stays the single owner of draft state.

---

## Task 1: `CardEditToolbar` + `EditFilterChips`

**Files:** Create the two components; Test: `__tests__/CardEditToolbar.test.tsx`, `__tests__/EditFilterChips.test.tsx`.

- [ ] **Step 1: Failing tests**

`__tests__/CardEditToolbar.test.tsx`:
```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { CardEditToolbar } from "../CardEditToolbar";

describe("CardEditToolbar", () => {
  it("calls each action; shows replace only for chart cards", () => {
    const fns = { onSettings: vi.fn(), onDuplicate: vi.fn(), onReplace: vi.fn(), onRemove: vi.fn() };
    const { rerender } = render(<CardEditToolbar cardType="chart" {...fns} />);
    fireEvent.click(screen.getByRole("button", { name: "card settings" })); expect(fns.onSettings).toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "duplicate card" })); expect(fns.onDuplicate).toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "replace card" })); expect(fns.onReplace).toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "remove card" })); expect(fns.onRemove).toHaveBeenCalled();
    rerender(<CardEditToolbar cardType="text" {...fns} />);
    expect(screen.queryByRole("button", { name: "replace card" })).toBeNull();
  });
});
```

`__tests__/EditFilterChips.test.tsx`:
```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { EditFilterChips } from "../EditFilterChips";
import type { DashboardParameter } from "@/api/dashboards";

const params: DashboardParameter[] = [
  { id: "a", name: "Region", slug: "region", type: "category", operator: "is" },
  { id: "b", name: "Priority", slug: "priority", type: "category", operator: "is" },
];

describe("EditFilterChips", () => {
  it("renders a chip per param, marks the active one, and selects on click", () => {
    const onSelect = vi.fn();
    render(<EditFilterChips parameters={params} activeId="a" onSelect={onSelect} />);
    expect(screen.getByRole("button", { name: "Region" }).getAttribute("data-active")).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: "Priority" }));
    expect(onSelect).toHaveBeenCalledWith("b");
  });
});
```

- [ ] **Step 2: Run — RED** (`npx vitest run src/components/chart/dashboard/__tests__/CardEditToolbar.test.tsx src/components/chart/dashboard/__tests__/EditFilterChips.test.tsx`).

- [ ] **Step 3: Implement**

`CardEditToolbar.tsx`:
```tsx
interface Props {
  cardType: string;
  onSettings: () => void;
  onDuplicate: () => void;
  onReplace: () => void;
  onRemove: () => void;
}

export function CardEditToolbar({ cardType, onSettings, onDuplicate, onReplace, onRemove }: Props) {
  const btn = "rounded bg-background/80 px-1 text-xs hover:bg-accent";
  return (
    <div className="absolute right-1 top-1 z-10 flex gap-1">
      <button type="button" aria-label="card settings" className={btn} onClick={onSettings}>⚙</button>
      <button type="button" aria-label="duplicate card" className={btn} onClick={onDuplicate}>⧉</button>
      {cardType === "chart" && <button type="button" aria-label="replace card" className={btn} onClick={onReplace}>↺</button>}
      <button type="button" aria-label="remove card" className={`${btn} text-destructive`} onClick={onRemove}>✕</button>
    </div>
  );
}
```

`EditFilterChips.tsx`:
```tsx
import type { DashboardParameter } from "@/api/dashboards";

interface Props { parameters: DashboardParameter[]; activeId: string | null; onSelect: (id: string) => void }

export function EditFilterChips({ parameters, activeId, onSelect }: Props) {
  if (parameters.length === 0) return null;
  return (
    <div className="mb-3 flex flex-wrap items-center gap-2">
      {parameters.map((p) => (
        <button key={p.id} type="button" aria-label={p.name} data-active={p.id === activeId}
          className={`rounded-full border px-3 py-1 text-sm ${p.id === activeId ? "border-primary bg-primary/5 text-primary" : "hover:bg-accent/40"}`}
          onClick={() => onSelect(p.id)}>{p.name}{p.required ? " *" : ""}</button>
      ))}
    </div>
  );
}
```

- [ ] **Step 4: Run — GREEN.** **Step 5: Full sweep + commit.**
```bash
git add frontend/src/components/chart/dashboard/CardEditToolbar.tsx frontend/src/components/chart/dashboard/EditFilterChips.tsx frontend/src/components/chart/dashboard/__tests__/CardEditToolbar.test.tsx frontend/src/components/chart/dashboard/__tests__/EditFilterChips.test.tsx
git commit -m "feat(dashboards): CardEditToolbar + EditFilterChips (edit-mode UI primitives)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `FilterSettingsPanel`

**Files:** Create `FilterSettingsPanel.tsx`; Test `__tests__/FilterSettingsPanel.test.tsx`. Reuses the `OPERATORS` map from `ParameterEditor.tsx` (export it).

- [ ] **Step 1: Failing test**
```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FilterSettingsPanel } from "../FilterSettingsPanel";
import type { DashboardParameter } from "@/api/dashboards";

const p: DashboardParameter = { id: "a", name: "Region", slug: "region", type: "category", operator: "is" };

describe("FilterSettingsPanel", () => {
  it("edits name and required, and deletes", () => {
    const onChange = vi.fn(); const onDelete = vi.fn();
    render(<FilterSettingsPanel param={p} onChange={onChange} onDelete={onDelete} />);
    fireEvent.change(screen.getByLabelText("parameter name"), { target: { value: "Reg" } });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ name: "Reg" }));
    fireEvent.click(screen.getByLabelText("parameter required"));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ required: true }));
    fireEvent.click(screen.getByRole("button", { name: "delete filter" }));
    expect(onDelete).toHaveBeenCalled();
  });
  it("changing type resets the operator to the first for that type", () => {
    const onChange = vi.fn();
    render(<FilterSettingsPanel param={p} onChange={onChange} onDelete={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("parameter type"), { target: { value: "number" } });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ type: "number", operator: "=" }));
  });
});
```

- [ ] **Step 2: Run — RED.**

- [ ] **Step 3: Implement.** First, in `ParameterEditor.tsx` change `export const OPERATORS` (it is already exported — confirm) and reuse it. `FilterSettingsPanel.tsx`:
```tsx
import type { DashboardParameter, ParamType, ParamOperator } from "@/api/dashboards";
import { OPERATORS } from "./ParameterEditor";

interface Props { param: DashboardParameter; onChange: (next: DashboardParameter) => void; onDelete: () => void }

export function FilterSettingsPanel({ param, onChange, onDelete }: Props) {
  const cls = "h-8 w-full rounded border bg-background px-2 text-sm";
  const showOp = param.type === "text" || param.type === "number" || param.type === "date";
  return (
    <div className="flex flex-col gap-3 p-3">
      <div className="text-sm font-medium">Filter settings</div>
      <label className="flex flex-col gap-1 text-xs text-muted-foreground">Name
        <input aria-label="parameter name" className={cls} value={param.name} onChange={(e) => onChange({ ...param, name: e.target.value })} />
      </label>
      <label className="flex flex-col gap-1 text-xs text-muted-foreground">Type
        <select aria-label="parameter type" className={cls} value={param.type}
          onChange={(e) => { const t = e.target.value as ParamType; onChange({ ...param, type: t, operator: OPERATORS[t][0] }); }}>
          {(Object.keys(OPERATORS) as ParamType[]).map((t) => <option key={t} value={t}>{t}</option>)}
        </select>
      </label>
      {showOp && (
        <label className="flex flex-col gap-1 text-xs text-muted-foreground">Operator
          <select aria-label="parameter operator" className={cls} value={param.operator}
            onChange={(e) => onChange({ ...param, operator: e.target.value as ParamOperator })}>
            {OPERATORS[param.type].map((o) => <option key={o} value={o}>{o}</option>)}
          </select>
        </label>
      )}
      <label className="flex items-center gap-2 text-sm">
        <input type="checkbox" aria-label="parameter required" checked={!!param.required} onChange={(e) => onChange({ ...param, required: e.target.checked })} /> Required
      </label>
      <button type="button" aria-label="delete filter" className="self-start text-sm text-destructive hover:underline" onClick={onDelete}>Delete filter</button>
    </div>
  );
}
```

- [ ] **Step 4: Run — GREEN. Step 5: sweep + commit** (`git add` the two files; message `feat(dashboards): FilterSettingsPanel`).

---

## Task 3: `CardSettingsPanel`

**Files:** Create `CardSettingsPanel.tsx`; Test `__tests__/CardSettingsPanel.test.tsx`. This absorbs the per-card inline controls (title, param mapping, on-click, move-to-tab) ported verbatim from `DashboardView.tsx`.

- [ ] **Step 1: Failing test**
```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { CardSettingsPanel } from "../CardSettingsPanel";
import type { DashboardParameter } from "@/api/dashboards";
import type { DraftCard, DraftTab } from "../tabs";

const param: DashboardParameter = { id: "pr", name: "Region", slug: "region", type: "category", operator: "is" };
const chart = { id: "ch", dataset_id: "d", viz_type: "bar", name: "Sales", params: { spec: { dimensions: ["region"], columns: [], metrics: ["m"], filters: [], orderby: [], row_limit: 100, rollup_groups: [] } } } as never;
const card = (opts = {}): DraftCard => ({ id: "c1", chart_id: "ch", card_type: "chart", grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: opts, tab_id: null, tabKey: "ta" });
const tabs: DraftTab[] = [{ key: "ta", name: "A", position: 0 }, { key: "tb", name: "B", position: 1 }];

describe("CardSettingsPanel (chart card)", () => {
  it("edits title", () => {
    const onUpdateOptions = vi.fn();
    render(<CardSettingsPanel card={card()} chart={chart} parameters={[param]} tabs={tabs} onUpdateOptions={onUpdateOptions} onMoveToTab={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("card title"), { target: { value: "Hi" } });
    expect(onUpdateOptions).toHaveBeenCalledWith({ title: "Hi" });
  });
  it("maps a parameter to a column", () => {
    const onUpdateOptions = vi.fn();
    render(<CardSettingsPanel card={card()} chart={chart} parameters={[param]} tabs={tabs} onUpdateOptions={onUpdateOptions} onMoveToTab={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("map Region"), { target: { value: "region" } });
    expect(onUpdateOptions).toHaveBeenCalledWith({ paramMappings: { pr: "region" } });
  });
  it("sets on-click to cross-filter", () => {
    const onUpdateOptions = vi.fn();
    render(<CardSettingsPanel card={card()} chart={chart} parameters={[param]} tabs={tabs} onUpdateOptions={onUpdateOptions} onMoveToTab={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("on click"), { target: { value: "crossfilter" } });
    expect(onUpdateOptions).toHaveBeenCalledWith({ clickBehavior: { type: "crossfilter", parameterMapping: {} } });
  });
  it("moves the card to another tab", () => {
    const onMoveToTab = vi.fn();
    render(<CardSettingsPanel card={card()} chart={chart} parameters={[param]} tabs={tabs} onUpdateOptions={vi.fn()} onMoveToTab={onMoveToTab} />);
    fireEvent.change(screen.getByLabelText("move card to tab"), { target: { value: "tb" } });
    expect(onMoveToTab).toHaveBeenCalledWith("tb");
  });
});

describe("CardSettingsPanel (text card)", () => {
  it("edits the text body", () => {
    const onUpdateOptions = vi.fn();
    const c: DraftCard = { id: "t1", chart_id: null, card_type: "text", grid_col: 0, grid_row: 0, size_x: 6, size_y: 2, options: { text: "" }, tab_id: null, tabKey: null };
    render(<CardSettingsPanel card={c} chart={undefined} parameters={[]} tabs={[]} onUpdateOptions={onUpdateOptions} onMoveToTab={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("text body"), { target: { value: "hello" } });
    expect(onUpdateOptions).toHaveBeenCalledWith({ text: "hello" });
  });
});
```

- [ ] **Step 2: Run — RED.**

- [ ] **Step 3: Implement `CardSettingsPanel.tsx`** (port the on-click + mapping logic from DashboardView; `ClickBehavior` type from `./click-behavior`):
```tsx
import type { Chart } from "@/api/charts";
import type { DashboardParameter } from "@/api/dashboards";
import type { ClickBehavior } from "./click-behavior";
import type { DraftCard, DraftTab } from "./tabs";

interface Props {
  card: DraftCard;
  chart: Chart | undefined;
  parameters: DashboardParameter[];
  tabs: DraftTab[];
  onUpdateOptions: (patch: Record<string, unknown>) => void;
  onMoveToTab: (tabKey: string) => void;
}

const inp = "h-8 w-full rounded border bg-background px-2 text-sm";
const sel = "h-8 rounded border bg-background px-1 text-sm";

export function CardSettingsPanel({ card, chart, parameters, tabs, onUpdateOptions, onMoveToTab }: Props) {
  if (card.card_type === "text" || card.card_type === "heading") {
    return (
      <div className="flex flex-col gap-2 p-3">
        <div className="text-sm font-medium">{card.card_type === "heading" ? "Heading" : "Text"}</div>
        <textarea aria-label={card.card_type === "heading" ? "heading text" : "text body"} className={`${inp} h-24 py-1`}
          value={(card.options.text as string) ?? ""} onChange={(e) => onUpdateOptions({ text: e.target.value })} />
      </div>
    );
  }
  if (card.card_type === "link") {
    return (
      <div className="flex flex-col gap-2 p-3">
        <div className="text-sm font-medium">Link</div>
        <input aria-label="link url" placeholder="https://…" className={inp} value={(card.options.url as string) ?? ""} onChange={(e) => onUpdateOptions({ url: e.target.value })} />
        <input aria-label="link label" placeholder="Label (optional)" className={inp} value={(card.options.label as string) ?? ""} onChange={(e) => onUpdateOptions({ label: e.target.value })} />
      </div>
    );
  }

  const cols = chart ? [...chart.params.spec.dimensions, ...chart.params.spec.columns] : [];
  const mappings = (card.options.paramMappings as Record<string, string>) ?? {};
  const cb = (card.options.clickBehavior as ClickBehavior | undefined) ?? { type: "menu" };
  return (
    <div className="flex flex-col gap-3 p-3">
      <div className="text-sm font-medium">{chart?.name ?? "Card"} settings</div>
      <label className="flex flex-col gap-1 text-xs text-muted-foreground">Title
        <input aria-label="card title" placeholder="Blank = chart name" className={inp} value={(card.options.title as string) ?? ""} onChange={(e) => onUpdateOptions({ title: e.target.value })} />
      </label>

      {parameters.length > 0 && (
        <div className="flex flex-col gap-1">
          <div className="text-xs font-medium text-muted-foreground">Filters</div>
          {parameters.map((p) => (
            <label key={p.id} className="flex items-center justify-between gap-2 text-sm">
              <span>{p.name}</span>
              <select aria-label={`map ${p.name}`} className={sel} value={mappings[p.id] ?? ""}
                onChange={(e) => onUpdateOptions({ paramMappings: { ...mappings, [p.id]: e.target.value } })}>
                <option value="">none</option>
                {cols.map((col) => <option key={col} value={col}>{col}</option>)}
              </select>
            </label>
          ))}
        </div>
      )}

      <div className="flex flex-col gap-1">
        <label className="flex items-center justify-between gap-2 text-sm"><span>On click</span>
          <select aria-label="on click" className={sel} value={cb.type}
            onChange={(e) => {
              const t = e.target.value as ClickBehavior["type"];
              onUpdateOptions({ clickBehavior: t === "url" ? { type: "url", template: "" } : t === "crossfilter" ? { type: "crossfilter", parameterMapping: {} } : { type: "menu" } });
            }}>
            <option value="menu">Menu</option><option value="crossfilter">Cross-filter</option><option value="url">Custom URL</option>
          </select>
        </label>
        {cb.type === "url" && (
          <input aria-label="url template" placeholder="https://…/{{column}}?p={{slug}}" className={inp} value={cb.template}
            onChange={(e) => onUpdateOptions({ clickBehavior: { type: "url", template: e.target.value } })} />
        )}
        {cb.type === "crossfilter" && parameters.map((p) => {
          const m = cb.parameterMapping ?? {};
          return (
            <label key={p.id} className="flex items-center justify-between gap-2 text-sm"><span>{p.name} ←</span>
              <select aria-label={`crossfilter ${p.name}`} className={sel} value={m[p.id] ?? ""}
                onChange={(e) => onUpdateOptions({ clickBehavior: { type: "crossfilter", parameterMapping: { ...m, [p.id]: e.target.value } } })}>
                <option value="">none</option>
                {cols.map((col) => <option key={col} value={col}>{col}</option>)}
              </select>
            </label>
          );
        })}
      </div>

      {tabs.length >= 2 && (
        <label className="flex items-center justify-between gap-2 text-sm"><span>Move to tab</span>
          <select aria-label="move card to tab" className={sel} value={card.tabKey ?? ""} onChange={(e) => onMoveToTab(e.target.value)}>
            {tabs.map((t) => <option key={t.key} value={t.key}>{t.name}</option>)}
          </select>
        </label>
      )}
    </div>
  );
}
```

- [ ] **Step 4: Run — GREEN. Step 5: sweep + commit** (message `feat(dashboards): CardSettingsPanel`).

---

## Task 4: `AddPanel`

**Files:** Create `AddPanel.tsx`; Test `__tests__/AddPanel.test.tsx`.

- [ ] **Step 1: Failing test**
```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { AddPanel } from "../AddPanel";
import type { Chart } from "@/api/charts";

const charts = [{ id: "1", name: "Revenue bar" }, { id: "2", name: "Orders line" }] as unknown as Chart[];

describe("AddPanel", () => {
  it("filters the chart list by search and adds on click", () => {
    const onAddChart = vi.fn();
    render(<AddPanel charts={charts} onAddChart={onAddChart} onAddVirtual={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("search charts"), { target: { value: "order" } });
    expect(screen.queryByRole("button", { name: "Revenue bar" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Orders line" }));
    expect(onAddChart).toHaveBeenCalledWith(charts[1]);
  });
  it("adds virtual cards", () => {
    const onAddVirtual = vi.fn();
    render(<AddPanel charts={charts} onAddChart={vi.fn()} onAddVirtual={onAddVirtual} />);
    fireEvent.click(screen.getByRole("button", { name: "Add Text" }));
    expect(onAddVirtual).toHaveBeenCalledWith("text");
  });
});
```

- [ ] **Step 2: Run — RED.**

- [ ] **Step 3: Implement `AddPanel.tsx`:**
```tsx
import { useState } from "react";
import type { Chart } from "@/api/charts";

interface Props { charts: Chart[]; onAddChart: (c: Chart) => void; onAddVirtual: (t: "text" | "heading" | "link") => void }

export function AddPanel({ charts, onAddChart, onAddVirtual }: Props) {
  const [q, setQ] = useState("");
  const shown = charts.filter((c) => c.name.toLowerCase().includes(q.toLowerCase()));
  return (
    <div className="flex h-full flex-col gap-2 p-3">
      <div className="text-sm font-medium">Add a chart</div>
      <input aria-label="search charts" placeholder="Search charts…" className="h-8 w-full rounded border bg-background px-2 text-sm" value={q} onChange={(e) => setQ(e.target.value)} />
      <div className="-mx-1 flex max-h-[60vh] flex-col overflow-auto px-1">
        {shown.length === 0 ? <p className="px-1 py-2 text-xs text-muted-foreground">No charts</p>
          : shown.map((c) => (
            <button key={c.id} type="button" aria-label={c.name} className="rounded px-2 py-1.5 text-left text-sm hover:bg-accent/40" onClick={() => onAddChart(c)}>{c.name}</button>
          ))}
      </div>
      <div className="flex gap-1 border-t pt-2">
        <button type="button" aria-label="Add Text" className="rounded border px-2 py-1 text-xs hover:bg-accent/40" onClick={() => onAddVirtual("text")}>+ Text</button>
        <button type="button" aria-label="Add Heading" className="rounded border px-2 py-1 text-xs hover:bg-accent/40" onClick={() => onAddVirtual("heading")}>+ Heading</button>
        <button type="button" aria-label="Add Link" className="rounded border px-2 py-1 text-xs hover:bg-accent/40" onClick={() => onAddVirtual("link")}>+ Link</button>
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Run — GREEN. Step 5: sweep + commit** (message `feat(dashboards): AddPanel (searchable add)`).

---

## Task 5: `EditSidebar` (switch) + `EditPanel` type

**Files:** Create `EditSidebar.tsx`; Test `__tests__/EditSidebar.test.tsx`.

- [ ] **Step 1: Failing test** — verify it renders the right sub-panel by `panel.kind`:
```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { EditSidebar } from "../EditSidebar";

const noop = vi.fn();
const common = { charts: [], parameters: [], tabs: [], cardForPanel: undefined, chartForCard: undefined, paramForPanel: undefined,
  onAddChart: noop, onAddVirtual: noop, onUpdateCardOptions: noop, onMoveCardToTab: noop, onChangeParam: noop, onDeleteParam: noop };

describe("EditSidebar", () => {
  it("renders the Add panel for kind 'add'", () => {
    render(<EditSidebar panel={{ kind: "add" }} {...common} />);
    expect(screen.getByLabelText("search charts")).toBeTruthy();
  });
  it("renders the Filter panel for kind 'filter'", () => {
    const param = { id: "a", name: "Region", slug: "region", type: "category", operator: "is" } as never;
    render(<EditSidebar panel={{ kind: "filter", paramId: "a" }} {...common} paramForPanel={param} />);
    expect(screen.getByLabelText("parameter name")).toBeTruthy();
  });
});
```

- [ ] **Step 2: Run — RED.**

- [ ] **Step 3: Implement `EditSidebar.tsx`** (define + export `EditPanel`):
```tsx
import type { Chart } from "@/api/charts";
import type { DashboardParameter } from "@/api/dashboards";
import type { DraftCard, DraftTab } from "./tabs";
import { AddPanel } from "./AddPanel";
import { CardSettingsPanel } from "./CardSettingsPanel";
import { FilterSettingsPanel } from "./FilterSettingsPanel";

export type EditPanel =
  | { kind: "add" }
  | { kind: "card"; cardId: string }
  | { kind: "filter"; paramId: string }
  | null;

interface Props {
  panel: EditPanel;
  charts: Chart[];
  parameters: DashboardParameter[];
  tabs: DraftTab[];
  cardForPanel: DraftCard | undefined;
  chartForCard: Chart | undefined;
  paramForPanel: DashboardParameter | undefined;
  onAddChart: (c: Chart) => void;
  onAddVirtual: (t: "text" | "heading" | "link") => void;
  onUpdateCardOptions: (patch: Record<string, unknown>) => void;
  onMoveCardToTab: (tabKey: string) => void;
  onChangeParam: (next: DashboardParameter) => void;
  onDeleteParam: () => void;
}

export function EditSidebar(props: Props) {
  const { panel } = props;
  return (
    <aside className="w-[340px] shrink-0 self-start rounded-lg border bg-card">
      {panel?.kind === "add" && <AddPanel charts={props.charts} onAddChart={props.onAddChart} onAddVirtual={props.onAddVirtual} />}
      {panel?.kind === "card" && props.cardForPanel && (
        <CardSettingsPanel card={props.cardForPanel} chart={props.chartForCard} parameters={props.parameters} tabs={props.tabs}
          onUpdateOptions={props.onUpdateCardOptions} onMoveToTab={props.onMoveCardToTab} />
      )}
      {panel?.kind === "filter" && props.paramForPanel && (
        <FilterSettingsPanel param={props.paramForPanel} onChange={props.onChangeParam} onDelete={props.onDeleteParam} />
      )}
    </aside>
  );
}
```

- [ ] **Step 4: Run — GREEN. Step 5: sweep + commit** (message `feat(dashboards): EditSidebar switch`).

---

## Task 6: Wire it into `DashboardView` + clean cards

**Files:** Modify `DashboardView.tsx`, `DashboardCardView.tsx`; Test `__tests__/DashboardView.editsidebar.test.tsx`. This is the integration: add `editPanel`, the top-bar, the sidebar layout, clean cards, chips; remove the old inline blocks.

- [ ] **Step 1: `DashboardCardView` renders virtual displays in edit too** — currently `DashboardView` only renders `DashboardCardView` for `(!editing || chart)`. We will always render it. Confirm `DashboardCardView` already routes text/heading/link to `TextCard`/`HeadingCard`/`LinkCard` (it does, by `card_type`) — no change needed there beyond ensuring it doesn't require `editing`. (No code change expected; verify.)

- [ ] **Step 2: Failing test `__tests__/DashboardView.editsidebar.test.tsx`** (mirror existing DashboardView test mocks; the heading-card fixture is enough to assert the sidebar + clean card):
```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import DashboardView from "../DashboardView";

vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({ DashboardCardView: ({ card }: { card: { id: string } }) => <div data-testid={`disp-${card.id}`} /> }));
vi.mock("@/api/charts", () => ({ useCharts: () => ({ data: [{ id: "ch", name: "Sales", dataset_id: "d", viz_type: "bar", params: { spec: { dimensions: [], columns: [], metrics: ["m"], filters: [], orderby: [], row_limit: 100, rollup_groups: [] } } }] }) }));
const detail = { id: "d1", name: "D", description: "", created_by: null, updated_at: "", parameters: [], tabs: [], width: "full",
  cards: [{ id: "c1", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 0, size_x: 6, size_y: 1, options: { text: "h" }, tab_id: null }] };
vi.mock("@/api/dashboards", async (orig) => { const m = await (orig as () => Promise<Record<string, unknown>>)();
  return { ...m, useDashboard: () => ({ data: detail, isLoading: false }), useDashboardMutations: () => ({ update: { mutateAsync: vi.fn(), isPending: false } }) }; });
const renderView = () => render(<QueryClientProvider client={new QueryClient()}><MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}><Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes></MemoryRouter></QueryClientProvider>);

describe("DashboardView edit sidebar", () => {
  it("entering edit shows the Add panel; +Filter opens a filter panel", () => {
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    expect(screen.getByLabelText("search charts")).toBeTruthy();      // Add panel default
    fireEvent.click(screen.getByRole("button", { name: "+ Filter" }));
    expect(screen.getByLabelText("parameter name")).toBeTruthy();     // filter panel
  });
  it("clicking a card's settings opens the card panel", () => {
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.click(screen.getByRole("button", { name: "card settings" }));
    expect(screen.getByLabelText(/heading text|text body/)).toBeTruthy(); // heading card settings
  });
});
```

- [ ] **Step 3: Run — RED.**

- [ ] **Step 4: Refactor `DashboardView.tsx`.** Concretely:
  1. Add imports: `EditSidebar, type EditPanel` from `@/components/chart/dashboard/EditSidebar`; `EditFilterChips`; `CardEditToolbar`.
  2. Add state above the early return: `const [editPanel, setEditPanel] = useState<EditPanel>(null);`
  3. `startEdit` sets `setEditPanel({ kind: "add" });` `cancelEdit`/`onSave` set `setEditPanel(null);`
  4. **Top-bar:** in the `editing ? ( … )` header block, before the Width toggle, add:
     ```tsx
     <Button size="sm" variant="outline" onClick={() => setEditPanel({ kind: "add" })}>+ Chart</Button>
     <Button size="sm" variant="outline" onClick={() => {
       const slug = slugify("filter", parameters.map((p) => p.slug));
       const id = `new-${slug}-${cardSeqRef.current++}`;
       setDraftParameters([...parameters, { id, name: "Filter", slug, type: "text", operator: "is" }]);
       setEditPanel({ kind: "filter", paramId: id });
     }}>+ Filter</Button>
     ```
  5. **Delete** the old `{editing && (<aside …>Add a chart…Filters…</aside>)}` block (lines ~248-280).
  6. **Filter bar:** replace `<DashboardFilterBar … />` with: in view mode keep it; in edit mode render the chips:
     ```tsx
     {editing
       ? <EditFilterChips parameters={parameters} activeId={editPanel?.kind === "filter" ? editPanel.paramId : null} onSelect={(id) => setEditPanel({ kind: "filter", paramId: id })} />
       : <DashboardFilterBar parameters={parameters} values={paramsState.values} onChange={paramsState.setValue} optionsBySlug={optionsBySlug} />}
     ```
  7. **Grid column + sidebar:** wrap the grid block in a flex row with the sidebar when editing:
     ```tsx
     <div className={editing ? "flex gap-4" : ""}>
       <div className={editing ? "min-w-0 flex-1" : ""}>
         { …existing cards.length===0 / empty-tab / <Grid> ternary… }
       </div>
       {editing && (
         <EditSidebar
           panel={editPanel}
           charts={charts ?? []}
           parameters={parameters}
           tabs={tabs}
           cardForPanel={editPanel?.kind === "card" ? (cards as DraftCard[]).find((c) => c.id === editPanel.cardId) : undefined}
           chartForCard={editPanel?.kind === "card" ? chartById.get((cards as DraftCard[]).find((c) => c.id === editPanel.cardId)?.chart_id ?? "") : undefined}
           paramForPanel={editPanel?.kind === "filter" ? parameters.find((p) => p.id === editPanel.paramId) : undefined}
           onAddChart={(c) => addCard(c)}
           onAddVirtual={(t) => addVirtualCard(t, t === "link" ? { url: "", label: "" } : { text: "" })}
           onUpdateCardOptions={(patch) => { if (editPanel?.kind === "card") updateCardOptions(editPanel.cardId, patch); }}
           onMoveCardToTab={(tabKey) => { if (editPanel?.kind === "card") setDraftCards(moveCardToTab((cards as DraftCard[]), editPanel.cardId, tabKey)); }}
           onChangeParam={(next) => setDraftParameters(parameters.map((x) => x.id === next.id ? { ...next, slug: x.slug } : x))}
           onDeleteParam={() => { if (editPanel?.kind === "filter") { const id = editPanel.paramId; setDraftParameters(parameters.filter((x) => x.id !== id)); setEditPanel({ kind: "add" }); } }}
         />
       )}
     </div>
     ```
  8. **Clean cards:** inside `visibleCards.map`, replace the whole `{editing && (…toolbar…)}` + the inline title/mapping/on-click/move-to-tab/text/heading/link blocks (lines ~336-442) with just the `CardEditToolbar`, and always render `DashboardCardView`:
     ```tsx
     <div key={c.id} className={`relative overflow-hidden ${c.card_type === "heading" ? "" : "rounded-lg border bg-card shadow-sm"} ${editing && editPanel?.kind === "card" && editPanel.cardId === c.id ? "ring-2 ring-primary" : ""}`}>
       {editing && (
         <CardEditToolbar cardType={c.card_type}
           onSettings={() => setEditPanel({ kind: "card", cardId: c.id })}
           onDuplicate={() => duplicateCard(c.id)}
           onReplace={() => setReplacingId(c.id)}
           onRemove={() => removeCard(c.id)} />
       )}
       <DashboardCardView ws={ws} card={c} chart={injected} refreshNonce={view.refreshNonce} parameters={parameters} paramValues={paramsState.values}
         blocked={!editing && requiredUnset}
         onValueClick={editing ? undefined : (ctx, x, y) => onCardValueClick(c, ctx, x, y)} />
     </div>
     ```
  9. **Stale-panel guard:** after deriving `cards`/`parameters`, add: if `editPanel?.kind === "card"` and no card matches, or `"filter"` and no param matches, you may leave the sidebar empty (EditSidebar already no-ops when `cardForPanel`/`paramForPanel` is undefined) — acceptable. (Optional: a `useEffect` to reset to `{kind:"add"}`, but keep it simple; the undefined-guard in EditSidebar prevents crashes.)

- [ ] **Step 5: Run the new test — GREEN.** Then run ALL existing DashboardView suites (`DashboardView.test.tsx`, `.tabs`, `.clickbehavior`, `.required`, `.width`) — fix any that asserted the OLD inline edit controls (e.g. a test that did `getByLabelText("card title")` on the card face must now open the card panel first, or assert via the sidebar). Update those tests to the new flow WITHOUT weakening intent. The `DashboardView.tabs.test.tsx` move-card test and `.clickbehavior` on-click config likely need updating to go through the sidebar.

- [ ] **Step 6: Full sweep + commit**
```bash
git add frontend/src/routes/explore/DashboardView.tsx frontend/src/components/chart/dashboard/DashboardCardView.tsx frontend/src/routes/explore/__tests__/DashboardView.editsidebar.test.tsx <updated existing tests>
git commit -m "feat(dashboards): Metabase-style edit sidebar + clean cards in DashboardView

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Live verification + PR

**Files:** none.

- [ ] **Step 1: Stack up** (frontend `npm run dev`, backend running). Log in `admin@egret.local` / `Admin12345!`, ws `eng`.
- [ ] **Step 2: Drive the seeded PARITY dashboard** (`/explore/eng/dashboards/56f83b64-b7a8-488f-8bf4-cec47af7b703`) in edit mode (Playwright MCP), watching **0 console errors**:
  1. Cards are clean (chart + hover toolbar ⚙/⧉/↺/✕); the inline clutter is gone.
  2. Sidebar opens on Add; search filters the chart list; clicking adds to the active tab.
  3. Click a card's ⚙ → card panel; set Title / map Region / On-click cross-filter / Move to tab → reflected.
  4. `+ Filter` → a filter chip + filter panel; edit name/type/operator/required; Delete removes it.
  5. Click a filter chip → its panel. Tabs add/rename/reorder/delete still work.
  6. Save → reload → everything persisted. Switch to **view mode** → unchanged (value pills, etc.).
- [ ] **Step 3: Side-by-side** with Metabase `localhost:3001/dashboard/1` edit mode — the right-sidebar + clean-cards shape should match.
- [ ] **Step 4: Full sweep** (`npm run typecheck && lint && build && npx vitest run`). All green.
- [ ] **Step 5: Push + PR** (`git push -u origin feat/dashboards-tier2b-edit-ux`; `gh pr create --base main …`). Await user merge.

---

## Notes for the implementer

- **Rules of Hooks:** `editPanel` `useState` above the `if (isLoading || !dash) return …` guard.
- **No behaviour/data change** — every callback wraps an existing draft mutator; the save payload is identical. View mode is untouched.
- **Existing edit-mode tests** (move-card, on-click config) move from "inline on the card" to "via the sidebar after clicking ⚙" — update them, don't delete the coverage.
- The Replace-chart dialog (`replacingId`) and the delete-tab dialog stay as-is.
