# Bar Chart Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the bar chart good before anyone configures it, and give its Display panel structure — carrying the four architecture changes every later chart inherits.

**Architecture:** Controls gain a group and an advanced flag, so `StudioPanel` renders titled sections instead of a flat toggle list. `cartesianDescriptor(kind)` declares its controls per kind rather than one list for six. Defaults ("legend when there's a breakout", "axis title from the column name") live in the option builder as `cfg.x ?? derived`, never written to storage, so charts saved months ago pick them up. Two shared helpers — folding a long tail into "Other" and compact number formatting — are built here and reused by line, area and combo.

**Tech Stack:** React 18, TypeScript, Vitest + Testing Library, ECharts (SVG renderer).

**Spec:** `docs/en/specs/2026-07-28-chart-redesign-01-bar.md`; program standard in `docs/en/specs/2026-07-28-chart-redesign-program-design.md`.

**Starting point:** branch `feat/chart-redesign-bar` off `main` at `3a30e820` (holds both specs). `npm test` green (1362 tests / 240 files).

**Ground rules:**

- Run tests with `npm test -- <path>`, never `npx vitest` (it may resolve a cached global vitest without jsdom).
- Defaults go in the option builder, not into `settings`. If a task has you writing a default into stored settings, it is the wrong task.
- The `cartesian.*` settings namespace stays shared across the six kinds — bar→line must keep the axis and metrics.

---

### Task 1: Controls carry a group and a level

**Files:**
- Modify: `frontend/src/components/chart/viz/types.ts`
- Modify: `frontend/src/components/chart/StudioPanel.tsx`
- Test: `frontend/src/components/chart/__tests__/StudioPanel.test.tsx`

- [ ] **Step 1: Write the failing test**

Add to `StudioPanel.test.tsx`:

```tsx
  it("renders controls under their group heading, in first-appearance order", () => {
    const grouped = {
      ...desc,
      controls: [
        { id: "a", section: "display", group: "Series", render: () => <div>A</div> },
        { id: "b", section: "display", group: "Axes", render: () => <div>B</div> },
        { id: "c", section: "display", group: "Series", render: () => <div>C</div> },
      ],
    } as unknown as VizDescriptor<{ yMetrics: string[] }>;
    render(<StudioPanel {...props} descriptor={grouped} tab="display" />);
    const headings = screen.getAllByRole("heading").map((h) => h.textContent);
    expect(headings).toEqual(["Series", "Axes"]);
  });

  it("puts advanced controls behind a disclosure that starts closed", async () => {
    const withAdvanced = {
      ...desc,
      controls: [
        { id: "a", section: "display", group: "Axes", render: () => <div>Always</div> },
        { id: "b", section: "display", group: "Axes", advanced: true, render: () => <div>Rare</div> },
      ],
    } as unknown as VizDescriptor<{ yMetrics: string[] }>;
    render(<StudioPanel {...props} descriptor={withAdvanced} tab="display" />);
    expect(screen.getByText("Always")).toBeInTheDocument();
    expect(screen.queryByText("Rare")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /more options/i }));
    expect(screen.getByText("Rare")).toBeInTheDocument();
  });

  it("still renders ungrouped controls, with no heading", () => {
    render(<StudioPanel {...props} tab="data" />);
    expect(screen.getByText("X-axis control")).toBeInTheDocument();
    expect(screen.queryByRole("heading")).not.toBeInTheDocument();
  });
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npm test -- src/components/chart/__tests__/StudioPanel.test.tsx`
Expected: FAIL — no headings are rendered.

- [ ] **Step 3: Extend the control type**

In `viz/types.ts`, add to `VizControl`:

```ts
export interface VizControl<Cfg> {
  id: string;
  section: "data" | "display";
  /** Heading this control sits under; ungrouped controls render before any group. */
  group?: string;
  /** Hidden behind the group's "More options" disclosure. */
  advanced?: boolean;
  render: FC<VizControlProps<Cfg>>;
}
```

- [ ] **Step 4: Render groups in StudioPanel**

Replace the body of `StudioPanel` with:

```tsx
export function StudioPanel<Cfg>({
  descriptor, config, onConfigChange, columns, metrics, resultColumns, rows, unitRow, tab,
}: Props<Cfg>) {
  const shown = descriptor.controls.filter((c) => c.section === tab);
  if (shown.length === 0) {
    return <p className="text-xs text-muted-foreground">No {tab} options for this chart type.</p>;
  }
  const props = { config, onChange: onConfigChange, columns, metrics, resultColumns, rows, unitRow };
  const ungrouped = shown.filter((c) => !c.group);
  // Group order follows first appearance in the descriptor, so a viz controls its own
  // panel order without a separate list to keep in sync.
  const groups: string[] = [];
  for (const c of shown) if (c.group && !groups.includes(c.group)) groups.push(c.group);

  return (
    <>
      {ungrouped.map(({ id, render: Control }) => <Control key={id} {...props} />)}
      {groups.map((g) => (
        <ControlGroup key={g} title={g}
          controls={shown.filter((c) => c.group === g)} controlProps={props} />
      ))}
    </>
  );
}

function ControlGroup<Cfg>({ title, controls, controlProps }: {
  title: string;
  controls: VizControl<Cfg>[];
  controlProps: VizControlProps<Cfg>;
}) {
  const [open, setOpen] = useState(false);
  const advanced = controls.filter((c) => c.advanced);
  return (
    <div className="space-y-1.5">
      <SectionLabel>{title}</SectionLabel>
      {controls.filter((c) => !c.advanced).map(({ id, render: Control }) => (
        <Control key={id} {...controlProps} />
      ))}
      {advanced.length > 0 && (
        <>
          {open && advanced.map(({ id, render: Control }) => <Control key={id} {...controlProps} />)}
          <button type="button" onClick={() => setOpen((o) => !o)}
            className="text-xs text-muted-foreground hover:text-foreground">
            {open ? "Fewer options" : `More options (${advanced.length})`}
          </button>
        </>
      )}
    </div>
  );
}
```

with imports for `useState`, `SectionLabel` (`@/components/chart/settings/SectionLabel`) and the `VizControl` / `VizControlProps` types. `SectionLabel` renders an `<h4>`; if it does not, make it one so `getAllByRole("heading")` finds it.

- [ ] **Step 5: Run test to verify it passes**

Run: `cd frontend && npm test -- src/components/chart/__tests__/StudioPanel.test.tsx`
Expected: all PASS.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/viz/types.ts frontend/src/components/chart/StudioPanel.tsx frontend/src/components/chart/__tests__/StudioPanel.test.tsx
git commit -m "feat(charts): controls carry a group and an advanced level"
```

---

### Task 2: Compact number formatting

**Files:**
- Modify: `frontend/src/components/chart/format.ts`
- Test: `frontend/src/components/chart/__tests__/format.compact.test.ts`

- [ ] **Step 1: Write the failing test**

```ts
// frontend/src/components/chart/__tests__/format.compact.test.ts
import { describe, it, expect } from "vitest";
import { compactNumber } from "../format";

describe("compactNumber", () => {
  it("shortens thousands, millions and billions", () => {
    expect(compactNumber(25468)).toBe("25.5K");
    expect(compactNumber(1_250_000)).toBe("1.3M");
    expect(compactNumber(3_400_000_000)).toBe("3.4B");
  });

  it("leaves small numbers alone", () => {
    expect(compactNumber(0)).toBe("0");
    expect(compactNumber(999)).toBe("999");
    expect(compactNumber(-42)).toBe("-42");
  });

  it("keeps the sign on shortened negatives", () => {
    expect(compactNumber(-25468)).toBe("-25.5K");
  });

  it("drops a trailing .0", () => {
    expect(compactNumber(2000)).toBe("2K");
  });

  it("passes non-finite values through as an empty string", () => {
    expect(compactNumber(NaN)).toBe("");
    expect(compactNumber(Infinity)).toBe("");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npm test -- src/components/chart/__tests__/format.compact.test.ts`
Expected: FAIL — `compactNumber` is not exported.

- [ ] **Step 3: Implement it**

Append to `format.ts`:

```ts
/**
 * A short axis label: 25,468 → "25.5K". Used where the axis is too narrow for the full
 * number; the tooltip and data labels keep the exact value.
 */
export function compactNumber(v: number): string {
  if (!Number.isFinite(v)) return "";
  const abs = Math.abs(v);
  const units: [number, string][] = [
    [1e9, "B"],
    [1e6, "M"],
    [1e3, "K"],
  ];
  for (const [size, suffix] of units) {
    if (abs >= size) {
      const n = v / size;
      // One decimal, but "2.0K" reads worse than "2K".
      const text = Math.abs(n) >= 100 ? n.toFixed(0) : n.toFixed(1).replace(/\.0$/, "");
      return `${text}${suffix}`;
    }
  }
  return String(v);
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd frontend && npm test -- src/components/chart/__tests__/format.compact.test.ts`
Expected: 5 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/format.ts frontend/src/components/chart/__tests__/format.compact.test.ts
git commit -m "feat(charts): compact number formatting for narrow axes"
```

---

### Task 3: Folding a long tail into "Other"

**Files:**
- Create: `frontend/src/components/chart/echarts/fold.ts`
- Test: `frontend/src/components/chart/echarts/__tests__/fold.test.ts`

- [ ] **Step 1: Write the failing test**

```ts
// frontend/src/components/chart/echarts/__tests__/fold.test.ts
import { describe, it, expect } from "vitest";
import { foldTail } from "../fold";

const points = [
  { name: "a", value: 10 },
  { name: "b", value: 8 },
  { name: "c", value: 6 },
  { name: "d", value: 4 },
  { name: "e", value: 2 },
];

describe("foldTail", () => {
  it("keeps everything when no maximum is set", () => {
    expect(foldTail(points, undefined)).toEqual(points);
  });

  it("keeps the largest n-1 and sums the rest into Other", () => {
    expect(foldTail(points, 3)).toEqual([
      { name: "a", value: 10 },
      { name: "b", value: 8 },
      { name: "Other", value: 12 },
    ]);
  });

  it("does nothing when the data already fits", () => {
    expect(foldTail(points, 5)).toEqual(points);
    expect(foldTail(points, 9)).toEqual(points);
  });

  it("treats a maximum below 2 as no folding — one bar labelled Other says nothing", () => {
    expect(foldTail(points, 1)).toEqual(points);
    expect(foldTail(points, 0)).toEqual(points);
  });

  it("ignores non-numeric values when summing", () => {
    const mixed = [{ name: "a", value: 5 }, { name: "b", value: null }, { name: "c", value: 3 }];
    expect(foldTail(mixed, 2)).toEqual([{ name: "a", value: 5 }, { name: "Other", value: 3 }]);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npm test -- src/components/chart/echarts/__tests__/fold.test.ts`
Expected: FAIL — cannot resolve `../fold`.

- [ ] **Step 3: Implement it**

```ts
// frontend/src/components/chart/echarts/fold.ts
export interface FoldPoint {
  name: string;
  value: unknown;
}

/** The label the folded remainder gets, matching Metabase's wording. */
export const OTHER_LABEL = "Other";

/**
 * Folds a long tail into a single "Other" point, keeping the largest `max - 1`.
 *
 * Off unless `max` is given: Metabase's `graph.max_categories_enabled` defaults to false
 * and egret follows, because silently hiding rows behind "Other" is worse than a crowded
 * axis. Order is preserved for the kept points so the chart does not reshuffle when the
 * setting is switched on.
 */
export function foldTail(points: FoldPoint[], max: number | undefined): FoldPoint[] {
  if (max == null || max < 2 || points.length <= max) return points;
  const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : 0);
  const ranked = [...points].sort((a, b) => num(b.value) - num(a.value));
  const keep = new Set(ranked.slice(0, max - 1).map((p) => p.name));
  const kept = points.filter((p) => keep.has(p.name));
  const rest = points.filter((p) => !keep.has(p.name)).reduce((t, p) => t + num(p.value), 0);
  return [...kept, { name: OTHER_LABEL, value: rest }];
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd frontend && npm test -- src/components/chart/echarts/__tests__/fold.test.ts`
Expected: 5 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/echarts/fold.ts frontend/src/components/chart/echarts/__tests__/fold.test.ts
git commit -m "feat(charts): foldTail helper for long-tail categories"
```

---

### Task 4: Render-time defaults for the cartesian option

Legend and axis titles stop depending on the user having typed something.

**Files:**
- Modify: `frontend/src/components/chart/echarts/options/cartesian.ts`
- Test: `frontend/src/components/chart/echarts/__tests__/cartesian.defaults.test.ts`

- [ ] **Step 1: Write the failing test**

```ts
// frontend/src/components/chart/echarts/__tests__/cartesian.defaults.test.ts
import { describe, it, expect } from "vitest";
import { buildCartesianOption } from "../options/cartesian";

const columns = [
  { name: "region", type: "varchar" },
  { name: "category", type: "varchar" },
  { name: "total_revenue", type: "double" },
  { name: "order_count", type: "double" },
];
const rows = [["North", "A", 10, 1], ["South", "B", 20, 2]];
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const opt = (cfg: any) => buildCartesianOption(columns, rows, cfg) as any;

describe("cartesian defaults, applied at render time", () => {
  it("hides the legend for a single metric with no breakout", () => {
    expect(opt({ kind: "bar", xAxis: "region", yMetrics: ["total_revenue"] }).legend).toBeUndefined();
  });

  it("shows it once there is a breakout", () => {
    const o = opt({ kind: "bar", xAxis: "region", series: "category", yMetrics: ["total_revenue"] });
    expect(o.legend).toBeDefined();
  });

  it("shows it for two metrics", () => {
    const o = opt({ kind: "bar", xAxis: "region", yMetrics: ["total_revenue", "order_count"] });
    expect(o.legend).toBeDefined();
  });

  it("still obeys an explicit choice in both directions", () => {
    expect(opt({ kind: "bar", xAxis: "region", yMetrics: ["a", "b"], showLegend: false }).legend)
      .toBeUndefined();
    expect(opt({ kind: "bar", xAxis: "region", yMetrics: ["total_revenue"], showLegend: true }).legend)
      .toBeDefined();
  });

  it("titles the axes from the column and metric names", () => {
    const o = opt({ kind: "bar", xAxis: "region", yMetrics: ["total_revenue"] });
    expect(o.xAxis.name).toBe("region");
    expect(o.yAxis.name).toBe("total_revenue");
  });

  it("lets a typed title win, and an empty string mean 'no title'", () => {
    expect(opt({ kind: "bar", xAxis: "region", yMetrics: ["m"], xLabel: "Region" }).xAxis.name)
      .toBe("Region");
    expect(opt({ kind: "bar", xAxis: "region", yMetrics: ["m"], xLabel: "" }).xAxis.name)
      .toBe("");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npm test -- src/components/chart/echarts/__tests__/cartesian.defaults.test.ts`
Expected: FAIL — the legend is undefined in the breakout case and the axis names are undefined.

- [ ] **Step 3: Apply the defaults in the builder**

In `cartesian.ts`, inside `baseOption(cfg, categories, series)`, before the grid is built:

```ts
  // Defaults are decided here, never stored: a chart saved before this change picks them
  // up, and `?? ` keeps an explicit choice — including `false` and `""` — winning.
  const hasBreakout = cfg.series != null && cfg.series !== "";
  const showLegend = cfg.showLegend ?? (hasBreakout || cfg.yMetrics.length > 1);
  const xTitle = cfg.xLabel ?? cfg.xAxis;
  const yTitle = cfg.yLabel ?? cfg.yMetrics[0];
```

then replace every `cfg.showLegend` in that function with `showLegend`, `name: cfg.xLabel` with `name: xTitle`, and `name: cfg.yLabel` with `name: yTitle` (the value axis builder uses `cfg.yLabel` at the `name:` for the left axis only — leave the right axis undefined as it is).

- [ ] **Step 4: Run test to verify it passes**

Run: `cd frontend && npm test -- src/components/chart/echarts/__tests__/cartesian.defaults.test.ts && npm test -- src/components/chart/echarts`
Expected: the new file PASSES and the existing cartesian tests stay green. If an existing test asserted `legend` was undefined for a two-metric chart, update it — that was the old default, and the spec changes it deliberately.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/echarts/options/cartesian.ts frontend/src/components/chart/echarts/__tests__/cartesian.defaults.test.ts
git commit -m "feat(charts): legend and axis titles default from the data"
```

---

### Task 5: Compact and thinned category labels

**Files:**
- Modify: `frontend/src/components/chart/echarts/options/cartesian.ts`
- Test: `frontend/src/components/chart/echarts/__tests__/cartesian.labels.test.ts`

- [ ] **Step 1: Write the failing test**

```ts
// frontend/src/components/chart/echarts/__tests__/cartesian.labels.test.ts
import { describe, it, expect } from "vitest";
import { buildCartesianOption } from "../options/cartesian";

const columns = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }];
const many = Array.from({ length: 40 }, (_, i) => [`r${i}`, i * 1000]);
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const opt = (cfg: any, rows = many) => buildCartesianOption(columns, rows, cfg) as any;

describe("cartesian labels", () => {
  it("formats value-axis labels compactly", () => {
    const o = opt({ kind: "bar", xAxis: "region", yMetrics: ["rev"], compactValues: true });
    expect(o.yAxis.axisLabel.formatter(25468)).toBe("25.5K");
  });

  it("keeps full numbers when compact is off", () => {
    const o = opt({ kind: "bar", xAxis: "region", yMetrics: ["rev"], compactValues: false });
    expect(o.yAxis.axisLabel.formatter(25468)).toBe("25,468");
  });

  it("defaults to compact only when there are enough categories to crowd the axis", () => {
    expect(opt({ kind: "bar", xAxis: "region", yMetrics: ["rev"] }).yAxis.axisLabel.formatter(25468))
      .toBe("25.5K");
    const few = [["a", 1], ["b", 2]];
    expect(opt({ kind: "bar", xAxis: "region", yMetrics: ["rev"] }, few).yAxis.axisLabel.formatter(25468))
      .toBe("25,468");
  });

  it("thins category labels when there are many, and shows every one when there are few", () => {
    expect(opt({ kind: "bar", xAxis: "region", yMetrics: ["rev"] }).xAxis.axisLabel.interval).toBeGreaterThan(0);
    const few = [["a", 1], ["b", 2]];
    expect(opt({ kind: "bar", xAxis: "region", yMetrics: ["rev"] }, few).xAxis.axisLabel.interval).toBe(0);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npm test -- src/components/chart/echarts/__tests__/cartesian.labels.test.ts`
Expected: FAIL — the formatter returns the full number and `interval` is undefined.

- [ ] **Step 3: Implement**

In `cartesian.ts`, add to `CartesianBuildConfig`:

```ts
  /** Short value-axis labels (25.5K). Defaults to on once the axis is crowded. */
  compactValues?: boolean;
```

In `baseOption`, next to the other derived defaults:

```ts
  // A crowded axis is the case compact labels are for; 12 categories is where the
  // default 13px label starts colliding at the widths this preview uses.
  const crowded = categories.length > 12;
  const compact = cfg.compactValues ?? crowded;
```

Change `fmtFor` to take the compact flag into account:

```ts
  const fmtFor = (metrics: string[]) => (v: number) => {
    if (normalized) return `${Math.round(v)}%`;
    if (compact) return compactNumber(v);
    const m = metrics[0];
    return m ? formatValue(v, cfg.metricTypes?.[m] ?? "double", cfg.columnSettings?.[m]) : String(v);
  };
```

and give the category axis a thinning interval:

```ts
    axisLabel: {
      fontSize: CHART_THEME.axisFontSize,
      color: axisLabelColor,
      // 0 means "every label"; ECharts hides every other label at 1, and so on.
      interval: crowded ? Math.ceil(categories.length / 12) - 1 : 0,
    },
```

with `import { compactNumber } from "@/components/chart/format";`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cd frontend && npm test -- src/components/chart/echarts`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/echarts/options/cartesian.ts frontend/src/components/chart/echarts/__tests__/cartesian.labels.test.ts
git commit -m "feat(charts): compact value labels and thinned category labels when crowded"
```

---

### Task 6: Max categories, wired to the fold helper

**Files:**
- Modify: `frontend/src/components/chart/echarts/options/cartesian.ts`
- Modify: `frontend/src/components/chart/viz/cartesian-codec.ts`
- Test: `frontend/src/components/chart/echarts/__tests__/cartesian.fold.test.ts`

- [ ] **Step 1: Write the failing test**

```ts
// frontend/src/components/chart/echarts/__tests__/cartesian.fold.test.ts
import { describe, it, expect } from "vitest";
import { buildCartesianOption } from "../options/cartesian";

const columns = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }];
const rows = [["a", 10], ["b", 8], ["c", 6], ["d", 4], ["e", 2]];
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const opt = (cfg: any) => buildCartesianOption(columns, rows, cfg) as any;

describe("max categories", () => {
  it("keeps every category by default", () => {
    const o = opt({ kind: "bar", xAxis: "region", yMetrics: ["rev"] });
    expect(o.xAxis.data).toEqual(["a", "b", "c", "d", "e"]);
  });

  it("folds the tail into Other when a maximum is set", () => {
    const o = opt({ kind: "bar", xAxis: "region", yMetrics: ["rev"], maxCategories: 3 });
    expect(o.xAxis.data).toEqual(["a", "b", "Other"]);
    expect(o.series[0].data).toEqual([10, 8, 12]);
  });

  it("does not fold a breakout chart's series, only its categories", () => {
    const o = opt({ kind: "bar", xAxis: "region", yMetrics: ["rev"], maxCategories: 3 });
    expect(o.series).toHaveLength(1);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npm test -- src/components/chart/echarts/__tests__/cartesian.fold.test.ts`
Expected: FAIL — the second test sees all five categories.

- [ ] **Step 3: Implement**

Add to `CartesianBuildConfig`:

```ts
  /** Fold all but the largest `maxCategories - 1` into "Other". Unset = keep every one. */
  maxCategories?: number;
```

In the single-series path of `buildCartesianOption` (where `displayCats` and `series` are built, around line 265), fold before the option is assembled:

```ts
  // Fold only the un-broken-out case: with a breakout, "Other" would have to combine
  // series as well, which changes what the chart means. Combo/breakout folding is a
  // separate decision, taken when those charts are redesigned.
  if (cfg.maxCategories != null && !cfg.series && series.length === 1) {
    const s = series[0] as SeriesLike;
    const folded = foldTail(
      (displayCats as (string | number)[]).map((c, i) => ({ name: String(c), value: s.data[i] })),
      cfg.maxCategories,
    );
    displayCats = folded.map((p) => p.name);
    s.data = folded.map((p) => (typeof p.value === "number" ? p.value : null));
  }
```

with `import { foldTail } from "../fold";`. Add `maxCategories` and `compactValues` to the `KEYS` list in `viz/cartesian-codec.ts` so both persist.

- [ ] **Step 4: Run test to verify it passes**

Run: `cd frontend && npm test -- src/components/chart/echarts src/components/chart/viz`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/echarts/options/cartesian.ts frontend/src/components/chart/viz/cartesian-codec.ts frontend/src/components/chart/echarts/__tests__/cartesian.fold.test.ts
git commit -m "feat(charts): optional Other bucket for long-tailed categories"
```

---

### Task 7: Series order and visibility

**Files:**
- Modify: `frontend/src/components/chart/echarts/options/cartesian.ts`
- Modify: `frontend/src/components/chart/viz/cartesian-codec.ts`
- Test: `frontend/src/components/chart/echarts/__tests__/cartesian.seriesorder.test.ts`

- [ ] **Step 1: Write the failing test**

```ts
// frontend/src/components/chart/echarts/__tests__/cartesian.seriesorder.test.ts
import { describe, it, expect } from "vitest";
import { buildCartesianOption } from "../options/cartesian";

const columns = [
  { name: "region", type: "varchar" },
  { name: "rev", type: "double" },
  { name: "cnt", type: "double" },
];
const rows = [["a", 10, 1], ["b", 8, 2]];
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const names = (cfg: any) => (buildCartesianOption(columns, rows, cfg) as any).series.map((s: any) => s.name);

describe("series order and visibility", () => {
  it("follows the metric order by default", () => {
    expect(names({ kind: "bar", xAxis: "region", yMetrics: ["rev", "cnt"] })).toEqual(["rev", "cnt"]);
  });

  it("reorders to the stored order", () => {
    expect(names({ kind: "bar", xAxis: "region", yMetrics: ["rev", "cnt"], seriesOrder: ["cnt", "rev"] }))
      .toEqual(["cnt", "rev"]);
  });

  it("drops hidden series", () => {
    expect(names({ kind: "bar", xAxis: "region", yMetrics: ["rev", "cnt"], hiddenSeries: ["rev"] }))
      .toEqual(["cnt"]);
  });

  it("ignores names that are no longer plotted", () => {
    expect(names({ kind: "bar", xAxis: "region", yMetrics: ["rev"], seriesOrder: ["gone", "rev"] }))
      .toEqual(["rev"]);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npm test -- src/components/chart/echarts/__tests__/cartesian.seriesorder.test.ts`
Expected: FAIL — order and hiding are ignored.

- [ ] **Step 3: Implement**

Add to `CartesianBuildConfig`:

```ts
  /** Series names in display order; names not present are appended in their natural order. */
  seriesOrder?: string[];
  /** Series names to leave out of the chart entirely. */
  hiddenSeries?: string[];
```

In `baseOption`, immediately before the option is returned:

```ts
  // Applied last so it works the same for metric series and breakout series, and so a
  // stale name (a metric since removed) simply has no effect.
  const ordered = (() => {
    const list = (series as SeriesLike[]).filter((s) => !cfg.hiddenSeries?.includes(s.name ?? ""));
    if (!cfg.seriesOrder?.length) return list;
    const rank = new Map(cfg.seriesOrder.map((n, i) => [n, i]));
    return [...list].sort(
      (a, b) => (rank.get(a.name ?? "") ?? Number.MAX_SAFE_INTEGER) - (rank.get(b.name ?? "") ?? Number.MAX_SAFE_INTEGER),
    );
  })();
```

and use `ordered` in place of `series` in the returned object and in the legend-name computation. Add `seriesOrder` and `hiddenSeries` to `KEYS` in `viz/cartesian-codec.ts`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cd frontend && npm test -- src/components/chart/echarts src/components/chart/viz`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/echarts/options/cartesian.ts frontend/src/components/chart/viz/cartesian-codec.ts frontend/src/components/chart/echarts/__tests__/cartesian.seriesorder.test.ts
git commit -m "feat(charts): series order and visibility"
```

---

### Task 8: Bar declares its own grouped controls

**Files:**
- Modify: `frontend/src/components/chart/viz/cartesian.tsx`
- Create: `frontend/src/components/chart/settings/SeriesOrderSection.tsx`
- Modify: `frontend/src/components/chart/settings/CartesianDisplaySection.tsx`
- Test: `frontend/src/components/chart/viz/__tests__/cartesian.controls.test.tsx`

- [ ] **Step 1: Write the failing test**

```tsx
// frontend/src/components/chart/viz/__tests__/cartesian.controls.test.tsx
import { describe, it, expect } from "vitest";
import { cartesianDescriptor } from "../cartesian";

const groupsOf = (kind: "bar" | "line") =>
  cartesianDescriptor(kind).controls.filter((c) => c.section === "display").map((c) => c.group);

describe("cartesian controls are grouped and per-kind", () => {
  it("groups bar's display controls under Series, Axes and Format", () => {
    expect(new Set(groupsOf("bar"))).toEqual(new Set(["Series", "Axes", "Format"]));
  });

  it("puts the rare ones behind advanced", () => {
    const advanced = cartesianDescriptor("bar").controls.filter((c) => c.advanced).map((c) => c.id);
    expect(advanced).toContain("range");
    expect(advanced).toContain("goal");
  });

  it("offers max categories on the Data tab", () => {
    const data = cartesianDescriptor("bar").controls.filter((c) => c.section === "data").map((c) => c.id);
    expect(data).toContain("maxCategories");
  });

  it("does not offer bar's orientation to line", () => {
    const ids = (k: "bar" | "line") => cartesianDescriptor(k).controls.map((c) => c.id);
    expect(ids("bar")).toContain("orientation");
    expect(ids("line")).not.toContain("orientation");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npm test -- src/components/chart/viz/__tests__/cartesian.controls.test.tsx`
Expected: FAIL — every display control is one ungrouped `display` entry today.

- [ ] **Step 3: Split `CartesianDisplaySection` into the pieces the groups need**

Export four components from `CartesianDisplaySection.tsx`, moving the existing markup unchanged into them: `SeriesOptions` (stacked, 100% stacked, stack totals, per-series colour and label), `AxesOptions` (orientation for bar, x-axis scale, x/y titles), `AxisRangeOptions` (y min, y max, log scale), `FormatOptions` (data labels, compact values), `GoalOptions` (goal value and label), `TrendOption` (trend line). Keep the default export working for any caller that still wants everything.

- [ ] **Step 4: Declare bar's controls**

In `cartesian.tsx`, replace the single `display` control with a per-kind list. For `bar`:

```tsx
    { id: "maxCategories", section: "data", render: MaxCategoriesControl },
    { id: "series", section: "display", group: "Series", render: SeriesOptionsControl },
    { id: "seriesOrder", section: "display", group: "Series", render: SeriesOrderControl },
    { id: "orientation", section: "display", group: "Axes", render: OrientationControl },
    { id: "axes", section: "display", group: "Axes", render: AxesOptionsControl },
    { id: "range", section: "display", group: "Axes", advanced: true, render: AxisRangeControl },
    { id: "format", section: "display", group: "Format", render: FormatOptionsControl },
    { id: "goal", section: "display", group: "Format", advanced: true, render: GoalControl },
    { id: "trend", section: "display", group: "Format", advanced: true, render: TrendControl },
```

Each `*Control` is a thin wrapper over the matching section component. They all look like this — write one per section:

```tsx
const AxesOptionsControl = ({ config, onChange }: VizControlProps<CartesianConfig>) => (
  <AxesOptions kind={kind} config={config} onChange={onChange} />
);
```

`MaxCategoriesControl` is new UI rather than a move — a number input that clears to "keep every category":

```tsx
function MaxCategoriesControl({ config, onChange }: VizControlProps<CartesianConfig>) {
  return (
    <div className="space-y-1">
      <Label htmlFor="cz-maxcat" className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
        Max categories
      </Label>
      <Input id="cz-maxcat" type="number" min={2} className="h-8 text-sm"
        placeholder="Keep every category"
        value={config.maxCategories ?? ""}
        onChange={(e) => {
          const n = Number(e.target.value);
          onChange({ maxCategories: e.target.value === "" || Number.isNaN(n) ? undefined : n });
        }} />
      <p className="text-xs text-muted-foreground">The rest are summed into “Other”.</p>
    </div>
  );
}
```

`SeriesOrderSection.tsx` is the other new component — the series names with up/down buttons and a visibility toggle, writing `seriesOrder` and `hiddenSeries`:

```tsx
export function SeriesOrderSection({ names, order, hidden, onChange }: {
  names: string[];
  order?: string[];
  hidden?: string[];
  onChange: (patch: { seriesOrder?: string[]; hiddenSeries?: string[] }) => void;
}) {
  // The stored order may name series that are no longer plotted; display order is the
  // stored one filtered to what exists, then anything new appended.
  const ranked = [...names].sort(
    (a, b) => (order?.indexOf(a) ?? -1) - (order?.indexOf(b) ?? -1),
  );
  const move = (i: number, delta: number) => {
    const next = [...ranked];
    const j = i + delta;
    if (j < 0 || j >= next.length) return;
    [next[i], next[j]] = [next[j], next[i]];
    onChange({ seriesOrder: next });
  };
  return (
    <div className="space-y-1">
      {ranked.map((name, i) => (
        <div key={name} className="flex items-center gap-1.5">
          <button type="button" aria-label={`move ${name} up`} disabled={i === 0}
            className="text-xs text-muted-foreground disabled:opacity-40" onClick={() => move(i, -1)}>↑</button>
          <button type="button" aria-label={`move ${name} down`} disabled={i === ranked.length - 1}
            className="text-xs text-muted-foreground disabled:opacity-40" onClick={() => move(i, 1)}>↓</button>
          <span className="flex-1 truncate text-sm">{name}</span>
          <Switch aria-label={`show ${name}`} checked={!hidden?.includes(name)}
            onCheckedChange={(v) => onChange({
              hiddenSeries: v ? (hidden ?? []).filter((n) => n !== name) : [...(hidden ?? []), name],
            })} />
        </div>
      ))}
    </div>
  );
}
```

`SeriesOrderControl` derives `names` with `cartesianSeriesNames(resultColumns, rows, config)` — the same call the display section already uses for per-series colours.

`line`, `area`, `scatter`, `combo` and `row` keep their existing behaviour by declaring the same list minus `orientation` (and, for the non-stacking kinds, minus the stacking part of `SeriesOptions`, which that component already gates internally on `kind`).

- [ ] **Step 5: Run test to verify it passes**

Run: `cd frontend && npm test -- src/components/chart/viz src/components/chart/settings`
Expected: all PASS.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/viz/cartesian.tsx frontend/src/components/chart/settings
git commit -m "feat(charts): bar declares its own grouped controls"
```

---

### Task 9: Full verify, deploy, live check, PR

**Files:** none (verification only)

- [ ] **Step 1: Full FE gate**

Run: `cd frontend && npm run lint && npm run typecheck && npm test && npm run build`
Expected: all green.

- [ ] **Step 2: Deploy to kind**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
docker build -f frontend/Dockerfile -t egret-frontend:dev frontend
kind load docker-image egret-frontend:dev --name egret
kubectl rollout restart deploy/egret-frontend -n egret-system
kubectl rollout status deploy/egret-frontend -n egret-system --timeout=180s
```

Check the build actually succeeded before trusting the rollout — a failed `docker build` leaves the old image loadable and `rollout status` still reports success.

- [ ] **Step 3: Live check, with screenshots**

The spec's six steps, on Demo Sales, capturing before/after for 1 and 3:

1. `region` × `total_revenue`, nothing else: four bars, no legend, compact axis, both axis titles filled.
2. Add `category` as a breakout: legend appears by itself; clicking an entry hides that series.
3. `order_date` by day: category labels thinned, not overlapping; setting Max categories to 8 folds the tail into "Other".
4. Reorder two series, save, reload: order survives.
5. The same chart as a small dashboard card: still legible.
6. Open `A1 studio smoke` (saved before this change): it picks up the new defaults.

- [ ] **Step 4: Open the PR (do not merge; await approval)**

```bash
git -c http.version=HTTP/1.1 push -u origin feat/chart-redesign-bar
gh pr create --base main --title "feat(charts): redesign 01 — bar" --body "First of nineteen, per docs/en/specs/2026-07-28-chart-redesign-01-bar.md. Carries the program's shared changes: grouped controls, render-time defaults, foldTail and compactNumber."
```

---

## Notes for the implementer

- **Defaults never get written to `settings`.** Every default in this plan is a `??` in the option builder. If you find yourself calling `patchDescConfig` to seed one, stop — that would freeze today's default into every chart saved from now on.
- The five other cartesian kinds must come out of this behaving as they do now. Their controls are re-declared, not redesigned; line, area, scatter, combo and row get their own passes later in the program.
- `foldTail` deliberately does nothing for breakout charts. Folding across both categories and series changes what the chart claims, and that decision belongs to the combo pass.
- If an existing cartesian test breaks on the new legend default, that is the intended change — update the test rather than the default, and say so in the commit.
