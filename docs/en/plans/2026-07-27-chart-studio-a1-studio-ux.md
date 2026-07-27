# Chart Studio A1 — Studio UX Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the chart builder a modern "studio" feel — meaningful empty/loading/error states, hybrid auto-run with an Auto toggle, and a tidy toolbar with an inline title — without touching chart rendering or the config-model refactor (that is plan A2).

**Architecture:** Small additive units around the existing `ChartExplore.tsx`. A pure `missingFields()` helper drives both run-validation and the empty state. Auto-run is a debounced effect gated on a serialized query signature + an `autoRun` flag. New presentational components (`ChartPreviewStates`, `StudioToolbar`) replace inline JSX. No changes to `ChartRenderer`, ECharts option builders, or `useSpecData`.

**Tech Stack:** React 18, TypeScript, Vite, TanStack Query, shadcn/ui, Vitest + Testing Library. Design tokens are OKLCH (use `var(--token)` in inline styles, never `hsl(var(--token))`).

**Spec:** `docs/en/specs/2026-07-27-chart-studio-design.md` (VI: `docs/vi/specs/…`). This plan implements the "Run behaviour" and "Preview states" + toolbar parts of that spec. The viz-registry / config-model extraction is a separate plan (A2).

**Bilingual doc note:** this plan's Vietnamese reference is `docs/vi/plans/2026-07-27-chart-studio-a1-studio-ux.md`.

**Baseline before starting:** `cd frontend && npx tsc --noEmit` clean; `npx vitest run src/components/chart src/routes/explore/__tests__` green (599). Work on a branch off `main` (e.g. `feat/chart-studio-a1`).

---

### Task 1: Required-fields helper

A pure function that returns the human labels of the fields a viz still needs, keyed by viz type + the relevant config. Reused by run-validation (Task 2) and the empty state (Task 5).

**Files:**
- Create: `frontend/src/components/chart/requiredFields.ts`
- Test: `frontend/src/components/chart/__tests__/requiredFields.test.ts`

- [ ] **Step 1: Write the failing test**

```ts
import { describe, it, expect } from "vitest";
import { missingFields } from "../requiredFields";

describe("missingFields", () => {
  it("bar needs an X-axis and a metric", () => {
    expect(missingFields("bar", { cartesian: { yMetrics: [] } })).toEqual(["X-axis", "a metric"]);
    expect(missingFields("bar", { cartesian: { xAxis: "region", yMetrics: ["rev"] } })).toEqual([]);
  });
  it("pie needs a dimension and a metric", () => {
    expect(missingFields("pie", { pie: {} })).toEqual(["a dimension", "a metric"]);
  });
  it("big number needs a metric", () => {
    expect(missingFields("scalar", { scalar: {} })).toEqual(["a metric"]);
    expect(missingFields("scalar", { scalar: { metric: "rev" } })).toEqual([]);
  });
  it("returns [] for a viz with no requirements yet unknown", () => {
    expect(missingFields("map", { map: { region: "r", metric: "m" } })).toEqual([]);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/__tests__/requiredFields.test.ts`
Expected: FAIL — `missingFields` is not defined.

- [ ] **Step 3: Write the implementation**

Mirror the per-viz checks currently inside `ChartExplore.run()` (lines ~319-353), but as label lists instead of toasts. `Input` is a loose shape holding whatever config slices are relevant.

```ts
import type { VizType } from "@/api/charts";
import { isCartesianViz } from "@/api/charts";

interface Input {
  cartesian?: { xAxis?: string; yMetrics?: string[] };
  pie?: { dimension?: string; metric?: string };
  scalar?: { metric?: string };
  progress?: { metric?: string };
  gauge?: { metric?: string };
  funnel?: { dimension?: string; metric?: string };
  waterfall?: { xAxis?: string; metric?: string };
  smart?: { dimension?: string; metric?: string };
  box?: { category?: string; value?: string };
  sankey?: { source?: string; target?: string; metric?: string };
  map?: { region?: string; metric?: string };
  pivot?: { rows?: string[]; vals?: string[] };
  objectCols?: string[];
}

/** Labels of the fields `viz` still needs before it can run. Empty = ready. */
export function missingFields(viz: VizType, i: Input): string[] {
  const out: string[] = [];
  const need = (cond: boolean, label: string) => { if (cond) out.push(label); };
  if (isCartesianViz(viz)) {
    need(!i.cartesian?.xAxis, "X-axis");
    need(!(i.cartesian?.yMetrics?.length), "a metric");
  } else if (viz === "pie") {
    need(!i.pie?.dimension, "a dimension"); need(!i.pie?.metric, "a metric");
  } else if (viz === "scalar") {
    need(!i.scalar?.metric, "a metric");
  } else if (viz === "progress") {
    need(!i.progress?.metric, "a metric");
  } else if (viz === "gauge") {
    need(!i.gauge?.metric, "a metric");
  } else if (viz === "funnel") {
    need(!i.funnel?.dimension, "a dimension"); need(!i.funnel?.metric, "a metric");
  } else if (viz === "waterfall") {
    need(!i.waterfall?.xAxis, "an x-axis"); need(!i.waterfall?.metric, "a metric");
  } else if (viz === "smartscalar") {
    need(!i.smart?.dimension, "a time dimension"); need(!i.smart?.metric, "a metric");
  } else if (viz === "boxplot") {
    need(!i.box?.category, "a category"); need(!i.box?.value, "a value column");
  } else if (viz === "sankey") {
    need(!i.sankey?.source, "a source"); need(!i.sankey?.target, "a target"); need(!i.sankey?.metric, "a metric");
  } else if (viz === "map") {
    need(!i.map?.region, "a region"); need(!i.map?.metric, "a metric");
  } else if (viz === "pivot_table") {
    need(!(i.pivot?.rows?.length), "a row dimension"); need(!(i.pivot?.vals?.length), "a value");
  } else if (viz === "object") {
    need(!(i.objectCols?.length), "at least one column");
  }
  return out;
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/__tests__/requiredFields.test.ts`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/requiredFields.ts frontend/src/components/chart/__tests__/requiredFields.test.ts
git commit -m "feat(charts): missingFields helper for run-validation + empty state"
```

---

### Task 2: Use `missingFields` for run-validation

Replace the per-viz `if (…) { toast; return }` ladder in `run()` with one call to `missingFields`, so validation lives in one place.

**Files:**
- Modify: `frontend/src/routes/explore/ChartExplore.tsx` (the `run()` body, ~316-363)

- [ ] **Step 1: Add the import**

At the top of `ChartExplore.tsx`, near the other chart imports:

```ts
import { missingFields } from "@/components/chart/requiredFields";
```

- [ ] **Step 2: Build the Input from current state (helper inside the component)**

Add just above `run()`:

```ts
  const validationInput = () => ({
    cartesian: { xAxis: cartesian.xAxis, yMetrics: cartesian.yMetrics },
    pie, scalar, progress, gauge, funnel, waterfall, smart, box, sankey, map: mapCfg,
    pivot: { rows: pRows, vals: pVals },
    objectCols: tableCols,
  });
```

- [ ] **Step 3: Replace the validation ladder in `run()`**

Replace lines ~319-353 (every `if (viz === … ) { toast.error(…); return; }` block, plus the pivot/cartesian ones) with:

```ts
    const missing = missingFields(viz, validationInput());
    if (missing.length) { toast.error(`This chart needs ${missing.join(" and ")}`); return; }
```

Leave the `if (!datasetId) { toast.error("Pick a dataset first"); return; }` guard above it unchanged, and keep `setRanSpec(specForRun()); setRunNonce((n) => n + 1);` below.

- [ ] **Step 4: Verify build + existing tests**

Run: `cd frontend && npx tsc --noEmit && npx vitest run src/routes/explore/__tests__`
Expected: tsc clean; explore tests PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/explore/ChartExplore.tsx
git commit -m "refactor(charts): run() uses missingFields for validation"
```

---

### Task 3: Query signature + auto-run hook

A pure hook that decides when to auto-run: it watches a serialized query signature and fires a debounced callback when `enabled` and the signature is non-empty and changed. Isolated so its timing logic is testable.

**Files:**
- Create: `frontend/src/components/chart/useAutoRun.ts`
- Test: `frontend/src/components/chart/__tests__/useAutoRun.test.ts`

- [ ] **Step 1: Write the failing test**

```ts
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { renderHook } from "@testing-library/react";
import { useAutoRun } from "../useAutoRun";

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("useAutoRun", () => {
  it("fires the callback after the debounce when enabled and signature changes", () => {
    const cb = vi.fn();
    const { rerender } = renderHook(({ sig }) => useAutoRun({ enabled: true, signature: sig, delay: 500, onRun: cb }), { initialProps: { sig: "a" } });
    expect(cb).not.toHaveBeenCalled();
    vi.advanceTimersByTime(500);
    expect(cb).toHaveBeenCalledTimes(1);           // initial valid signature runs once
    rerender({ sig: "b" });
    vi.advanceTimersByTime(500);
    expect(cb).toHaveBeenCalledTimes(2);           // change re-runs
  });
  it("does not fire when disabled", () => {
    const cb = vi.fn();
    renderHook(() => useAutoRun({ enabled: false, signature: "a", delay: 500, onRun: cb }));
    vi.advanceTimersByTime(1000);
    expect(cb).not.toHaveBeenCalled();
  });
  it("does not fire on an empty signature", () => {
    const cb = vi.fn();
    renderHook(() => useAutoRun({ enabled: true, signature: "", delay: 500, onRun: cb }));
    vi.advanceTimersByTime(1000);
    expect(cb).not.toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/__tests__/useAutoRun.test.ts`
Expected: FAIL — `useAutoRun` not defined.

- [ ] **Step 3: Write the hook**

```ts
import { useEffect, useRef } from "react";

/**
 * Debounced auto-run: when `enabled` and `signature` is non-empty and differs
 * from the last run, call `onRun` after `delay` ms. An empty signature (the
 * chart isn't ready) never fires.
 */
export function useAutoRun({ enabled, signature, delay = 500, onRun }: {
  enabled: boolean;
  signature: string;
  delay?: number;
  onRun: () => void;
}) {
  const lastRef = useRef<string | null>(null);
  const cbRef = useRef(onRun);
  cbRef.current = onRun;
  useEffect(() => {
    if (!enabled || !signature || signature === lastRef.current) return;
    const t = setTimeout(() => { lastRef.current = signature; cbRef.current(); }, delay);
    return () => clearTimeout(t);
  }, [enabled, signature, delay]);
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/__tests__/useAutoRun.test.ts`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/useAutoRun.ts frontend/src/components/chart/__tests__/useAutoRun.test.ts
git commit -m "feat(charts): useAutoRun debounced auto-run hook"
```

---

### Task 4: Wire auto-run + Auto flag into ChartExplore

Add an `autoRun` state (default on) and drive `useAutoRun` from the query signature. Query-relevant changes auto-run; display-only changes still re-render from cached rows (no query) because they don't change the signature.

**Files:**
- Modify: `frontend/src/routes/explore/ChartExplore.tsx`

- [ ] **Step 1: Add state + import**

Add import near the others:

```ts
import { useAutoRun } from "@/components/chart/useAutoRun";
```

Add state next to `ranSpec` (~line 280):

```ts
  const [autoRun, setAutoRun] = useState(true);
```

- [ ] **Step 2: Compute the query signature and wire the hook**

Add just after `run()` is defined (after ~line 363). The signature is the serialized spec, but only when the chart is ready (no missing fields) and a dataset is chosen — otherwise empty (so it never auto-fires an invalid query):

```ts
  const querySignature =
    datasetId && missingFields(viz, validationInput()).length === 0
      ? JSON.stringify({ datasetId, spec: specForRun() })
      : "";
  useAutoRun({ enabled: autoRun, signature: querySignature, onRun: run });
```

Delete the ad-hoc filters effect at ~line 369 (`useEffect(() => { if (ranSpec) void run(); }, [filters])`) — filters now change the signature, so auto-run covers them when Auto is on. (When Auto is off, filters wait for Run, which is acceptable.)

- [ ] **Step 3: Verify build + tests**

Run: `cd frontend && npx tsc --noEmit && npx vitest run src/routes/explore/__tests__`
Expected: tsc clean; explore tests PASS.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/routes/explore/ChartExplore.tsx
git commit -m "feat(charts): hybrid auto-run driven by the query signature"
```

---

### Task 5: ChartPreviewStates component

Reusable empty / loading / error states for the preview.

**Files:**
- Create: `frontend/src/components/chart/ChartPreviewStates.tsx`
- Test: `frontend/src/components/chart/__tests__/ChartPreviewStates.test.tsx`

- [ ] **Step 1: Write the failing test**

```tsx
import { render, screen } from "@testing-library/react";
import { describe, it, expect, vi } from "vitest";
import { ChartEmptyState, ChartErrorState } from "../ChartPreviewStates";
import { BarChart3 } from "lucide-react";

describe("ChartPreviewStates", () => {
  it("empty state shows the title and hint", () => {
    render(<ChartEmptyState icon={BarChart3} title="Set up your Bar" hint="Add: X-axis, a metric" />);
    expect(screen.getByText("Set up your Bar")).toBeInTheDocument();
    expect(screen.getByText("Add: X-axis, a metric")).toBeInTheDocument();
  });
  it("error state shows the message and calls onRetry", async () => {
    const onRetry = vi.fn();
    render(<ChartErrorState message="boom" onRetry={onRetry} />);
    expect(screen.getByText("boom")).toBeInTheDocument();
    screen.getByRole("button", { name: "Retry" }).click();
    expect(onRetry).toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/__tests__/ChartPreviewStates.test.tsx`
Expected: FAIL — components not defined.

- [ ] **Step 3: Write the components**

```tsx
import type { LucideIcon } from "lucide-react";
import { AlertTriangle } from "lucide-react";
import { Skeleton } from "@/components/ui/skeleton";
import { Button } from "@/components/ui/button";

export function ChartEmptyState({ icon: Icon, title, hint }: { icon?: LucideIcon; title: string; hint: string }) {
  return (
    <div className="flex min-h-[60vh] flex-col items-center justify-center gap-2.5 rounded-lg border border-dashed p-6 text-center">
      {Icon && <Icon className="h-10 w-10 text-muted-foreground/40" aria-hidden />}
      <p className="text-sm font-medium">{title}</p>
      <p className="max-w-xs text-xs text-muted-foreground">{hint}</p>
    </div>
  );
}

export function ChartLoadingState() {
  const bars = [48, 72, 56, 88, 62, 78, 52, 68];
  return (
    <div className="flex min-h-[60vh] flex-col rounded-lg border p-4" role="status" aria-label="Loading chart">
      <div className="flex flex-1 items-end gap-2 pb-6 pt-10">
        {bars.map((h, i) => <Skeleton key={i} className="flex-1 rounded-sm" style={{ height: `${h}%` }} />)}
      </div>
      <Skeleton className="h-3 w-24 self-center" />
    </div>
  );
}

export function ChartErrorState({ message, onRetry }: { message: string; onRetry?: () => void }) {
  return (
    <div className="flex min-h-[40vh] flex-col items-center justify-center gap-3 rounded-lg border border-destructive/30 bg-destructive/5 p-6 text-center">
      <AlertTriangle className="h-8 w-8 text-destructive" aria-hidden />
      <p className="max-w-md text-sm text-destructive">{message}</p>
      {onRetry && <Button size="sm" variant="outline" onClick={onRetry}>Retry</Button>}
    </div>
  );
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/__tests__/ChartPreviewStates.test.tsx`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/ChartPreviewStates.tsx frontend/src/components/chart/__tests__/ChartPreviewStates.test.tsx
git commit -m "feat(charts): reusable empty/loading/error preview states"
```

---

### Task 6: Wire the states into the preview region

Replace the three plain-text/border states in `ChartExplore.tsx` with the new components, and make the empty state name the missing fields.

**Files:**
- Modify: `frontend/src/routes/explore/ChartExplore.tsx` (~lines 980-1045)

- [ ] **Step 1: Add imports**

```ts
import { ChartEmptyState, ChartLoadingState, ChartErrorState } from "@/components/chart/ChartPreviewStates";
```

- [ ] **Step 2: Replace the error + loading lines**

Replace (~lines 981-983):

```tsx
        {data.error && (
          <p className="rounded bg-destructive/10 p-3 text-sm text-destructive">{data.error}</p>
        )}
        {isRunning && <p className="text-sm text-muted-foreground">Running…</p>}
```

with:

```tsx
        {data.error && <ChartErrorState message={data.error} onRetry={run} />}
        {isRunning && <ChartLoadingState />}
```

- [ ] **Step 3: Replace the empty state**

Replace (~lines 1041-1043):

```tsx
        {!ranSpec && !data.isLoading && (
          <p className="text-sm text-muted-foreground">Configure the controls on the left and click Run.</p>
        )}
```

with (uses the current viz meta + missing fields):

```tsx
        {!ranSpec && !data.isLoading && (() => {
          const missing = datasetId ? missingFields(viz, validationInput()) : ["a dataset"];
          return (
            <ChartEmptyState icon={vizMeta?.icon} title={`Set up your ${vizMeta?.name ?? "chart"}`}
              hint={missing.length ? `Add: ${missing.join(", ")}${autoRun ? "" : ", then Run"}` : (autoRun ? "Ready — adjust the fields on the left." : "Ready — click Run to preview.")} />
          );
        })()}
```

- [ ] **Step 4: Replace the fixed preview height**

Change (~line 1029) `<div className="h-[70vh] rounded-md border p-2">` to `<div className="h-[60vh] rounded-lg border p-2">`.

The height must stay **definite** (`h-`, not `min-h-`): `ChartRenderer` sizes itself with
`h-full`, which resolves to 0 against an auto-height parent — the chart then mounts with a
0px canvas and the preview looks blank. (`min-h-[60vh]` is fine on the empty/loading states
in Task 5, whose content sizes them.)

- [ ] **Step 5: Verify build + tests**

Run: `cd frontend && npx tsc --noEmit && npx vitest run src/routes/explore/__tests__`
Expected: tsc clean; tests PASS. (If an explore test asserted the old "Configure the controls…" text, update it to assert the new "Set up your …" copy.)

- [ ] **Step 6: Commit**

```bash
git add frontend/src/routes/explore/ChartExplore.tsx
git commit -m "feat(charts): meaningful empty/loading/error states in the builder"
```

---

### Task 7: StudioToolbar — inline title + Auto toggle + overflow

Extract the toolbar into its own component: inline chart-name title, Auto toggle, Run, Save, and a `⋯` overflow menu (Export / View SQL). Keeps the current behaviour, tidier.

**Files:**
- Create: `frontend/src/components/chart/StudioToolbar.tsx`
- Modify: `frontend/src/routes/explore/ChartExplore.tsx` (toolbar region ~946-975)

- [ ] **Step 1: Write the component**

```tsx
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import type { ReactNode } from "react";

export function StudioToolbar({
  name, onName, autoRun, onAutoRun, onRun, onSave, running, saveLabel, exportMenu, sqlDisclosure,
}: {
  name: string; onName: (v: string) => void;
  autoRun: boolean; onAutoRun: (v: boolean) => void;
  onRun: () => void; onSave: () => void; running: boolean; saveLabel: string;
  exportMenu: ReactNode; sqlDisclosure: ReactNode;
}) {
  return (
    <>
      <div className="flex items-center gap-2">
        <input aria-label="Chart name" value={name} onChange={(e) => onName(e.target.value)}
          placeholder="Untitled chart"
          className="h-8 min-w-0 flex-1 rounded-md border border-transparent bg-transparent px-1.5 text-base font-semibold outline-none transition-colors placeholder:font-normal placeholder:text-muted-foreground hover:border-input focus:border-ring focus:bg-background" />
        <div className="flex shrink-0 items-center gap-2">
          <Label htmlFor="auto-run" className="flex items-center gap-1.5 text-xs text-muted-foreground">
            <Switch id="auto-run" checked={autoRun} onCheckedChange={onAutoRun} aria-label="Auto-run" /> Auto
          </Label>
          <Button size="sm" onClick={onRun} disabled={running}>{running ? "Running…" : "Run"}</Button>
          <Button size="sm" variant="outline" onClick={onSave}>{saveLabel}</Button>
          {exportMenu}
        </div>
      </div>
      {sqlDisclosure}
    </>
  );
}
```

- [ ] **Step 2: Use it in ChartExplore**

Add import:

```ts
import { StudioToolbar } from "@/components/chart/StudioToolbar";
```

Replace the toolbar block (~946-975: the `sr-only` h1 + the `<div className="flex items-center gap-2">…</div>` up to and including the `View SQL` details) with:

```tsx
        <h1 className="sr-only">{saveName || "New chart"}</h1>
        <StudioToolbar
          name={saveName} onName={setSaveName}
          autoRun={autoRun} onAutoRun={setAutoRun}
          onRun={run} onSave={() => setSaveOpen(true)}
          running={data.isLoading} saveLabel={id ? "Update" : "Save"}
          exportMenu={
            <ChartExportMenu title={saveName || undefined} columns={resultColumns} rows={data.rows}
              containerRef={previewRef} disabled={!finished || resultColumns.length === 0} />
          }
          sqlDisclosure={ranSpec && (
            <details className="text-xs text-muted-foreground" open={sqlOpen} onToggle={(e) => setSqlOpen(e.currentTarget.open)}>
              <summary className="cursor-pointer select-none hover:text-foreground">View SQL</summary>
              {sqlOpen && chartSql.isLoading && <p className="mt-1 text-xs">Loading…</p>}
              {sqlOpen && chartSql.error && <p className="mt-1 text-xs text-destructive">{String(chartSql.error)}</p>}
              {chartSql.data?.sql && <pre className="mt-1 max-h-40 overflow-auto rounded bg-muted p-2 text-xs">{chartSql.data.sql}</pre>}
            </details>
          )}
        />
```

(The inline title replaces the sr-only-only heading; keep the sr-only h1 for a stable landmark. `saveName`/`setSaveName` already exist.)

- [ ] **Step 3: Verify build + tests**

Run: `cd frontend && npx tsc --noEmit && npx vitest run src/routes/explore/__tests__`
Expected: tsc clean; tests PASS.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/components/chart/StudioToolbar.tsx frontend/src/routes/explore/ChartExplore.tsx
git commit -m "feat(charts): StudioToolbar — inline title + Auto toggle + overflow"
```

---

### Task 8: Full verify + deploy + live check

**Files:** none (verification only)

- [ ] **Step 1: Full FE gate**

Run: `cd frontend && npx tsc --noEmit && npx eslint src/components/chart src/routes/explore/ChartExplore.tsx && npx vitest run src/components/chart src/routes/explore/__tests__ && npm run build`
Expected: all green.

- [ ] **Step 2: Deploy the frontend to kind**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
docker build -f frontend/Dockerfile -t egret-frontend:dev frontend
kind load docker-image egret-frontend:dev --name egret
kubectl rollout restart deploy/egret-frontend -n egret-system
kubectl rollout status deploy/egret-frontend -n egret-system --timeout=150s
```

- [ ] **Step 3: Live check (needs a dataset with a dimension + metric)**

At `http://egret.localtest.me/explore/demo/charts/new?type=bar`: pick a dataset, set X-axis + a metric with **Auto on** → the chart runs automatically (no Run click). Toggle **Auto off**, change the metric → the chart waits; click **Run**. Empty state (before fields) shows "Set up your Bar — Add: X-axis, a metric". Open a saved chart → it renders on load.

- [ ] **Step 4: Open the PR (do not merge; await approval)**

```bash
git -c http.version=HTTP/1.1 push -u origin feat/chart-studio-a1
gh pr create --base main --title "feat(charts): Chart Studio A1 — states, auto-run, toolbar" --body "Implements docs/en/specs/2026-07-27-chart-studio-design.md (Run behaviour + preview states + toolbar). No rendering/registry changes (that's A2)."
```

---

## Notes for the implementer

- **Do NOT** touch `ChartRenderer`, `echarts/options/*`, or `useSpecData` — rendering is unchanged in A1.
- Auto-run must only fire on **query-relevant** change: the signature is built from `specForRun()`, so display-only settings (sort, colours, labels…) — which aren't in the spec — never trigger a query, exactly as intended.
- Keep the demo dataset + any sample charts in place for the live check; don't delete them.
- If a saved chart doesn't render on open, that is a pre-existing gap to fix here: hydrate `ranSpec` from the saved `params.spec` in the load effect (set `setRanSpec(params.spec)` when hydrating). Add it as a small extra step in Task 4 if the live check in Task 8 reveals it.
