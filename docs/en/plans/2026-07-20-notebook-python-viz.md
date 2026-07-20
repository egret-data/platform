# Notebook external Python viz libraries — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** DA/DS can use plotly (interactive) + matplotlib/seaborn (static) + the numeric/ML stack in notebook cells; plotly renders natively via `application/vnd.plotly.v1+json` + a lazy-loaded plotly.js, matplotlib/seaborn via image/png.

**Architecture:** Kernel-image + FE only (no backend change). Kernel installs the libs and sets plotly's default renderer to the mimetype bundle; the FE gains a lazy `PlotlyOutput` component and a MimeBundle dispatch branch.

**Tech Stack:** Docker (kernel image), Python (plotly/matplotlib/…), React/TS (plotly.js-dist-min), Vitest, Playwright.

**Design doc:** `docs/en/specs/2026-07-20-notebook-python-viz-design.md`.

**Branch:** `feat/notebook-python-viz` (created off main; design committed).

**Conventions:** FE cmds from `egret/frontend`. Commit per task. Verify FE `npm run typecheck/test/lint/build`; kernel pytest via `kernel-image/.venv/bin/python -m pytest`. Live e2e on the kind notebook stack (`deploy-kind-oidc`, no Ranger).

**Grounded facts:**
- Kernel deps: `kernel-image/Dockerfile:14` pip line; startup hook `kernel-image/startup/00-egret.py`.
- FE render seam: `frontend/src/components/notebook/CellOutputRenderer.tsx` — `MimeBundle({ data })` at line 48; `const html` branch at :49/:53, `png` at :50/:60. `React` is already imported (line 1). `OutputErrorBoundary` already wraps outputs.
- Existing test to extend: `frontend/src/components/notebook/__tests__/CellOutputRenderer.test.tsx`.
- Theme: `.dark` class on `document.documentElement` (set by `src/components/theme-provider.tsx:34-36`). Detect dark via `document.documentElement.classList.contains("dark")`.
- Mirror pattern: `frontend/src/components/chart/echarts/EChartsRenderer.tsx` (init on mount, ResizeObserver, dispose).
- Build: no `manualChunks` in `vite.config.ts` — a dynamic `import("plotly.js-dist-min")` auto-splits into its own chunk.

---

## Task 1: Kernel image — install viz libs + configure renderers

**Files:** `kernel-image/Dockerfile`, `kernel-image/startup/00-egret.py`. Test: `kernel-image/tests/test_viz.py`.

- [ ] **Step 1: Add the libs** to the pip line (`Dockerfile:14`): append `plotly matplotlib seaborn numpy scipy scikit-learn` to the existing `pip install --no-cache-dir …` list. Keep it one layer.

- [ ] **Step 2: Configure renderers in `startup/00-egret.py`** (best-effort, guarded — must never break kernel start; mirror the existing `try/except` blocks there):
```python
# Interactive plotly → emit the application/vnd.plotly.v1+json MIME bundle so the
# egret frontend renders it with plotly.js (not the sanitized text/html, which
# strips plotly.js and breaks interactivity).
try:
    import plotly.io as _pio
    _pio.renderers.default = "plotly_mimetype"
except Exception as _e:  # noqa: BLE001
    print(f"plotly renderer not configured: {_e}")

# matplotlib inline → figures emit image/png (rendered by the notebook's <img>).
try:
    import matplotlib
    matplotlib.use("module://matplotlib_inline.backend_inline")
except Exception as _e:  # noqa: BLE001
    print(f"matplotlib inline not configured: {_e}")
```

- [ ] **Step 3: Write the kernel test** `kernel-image/tests/test_viz.py`. Since the local `.venv` may lack these libs (like the sqlalchemy-absent case), gate on import availability:
```python
import importlib.util
import pytest

def _have(mod):
    return importlib.util.find_spec(mod) is not None

@pytest.mark.skipif(not all(_have(m) for m in
    ["plotly", "matplotlib", "seaborn", "numpy", "scipy", "sklearn"]),
    reason="viz libs not installed in this venv (they are in the kernel image)")
def test_viz_libs_import():
    import plotly, matplotlib, seaborn, numpy, scipy, sklearn  # noqa: F401

@pytest.mark.skipif(not _have("plotly"), reason="plotly not installed")
def test_plotly_emits_mimetype_bundle():
    import plotly.express as px
    fig = px.bar(x=["a", "b"], y=[1, 2])
    bundle = fig._repr_mimebundle_()
    # plotly returns a (data, metadata) tuple or a dict depending on version
    data = bundle[0] if isinstance(bundle, tuple) else bundle
    assert "application/vnd.plotly.v1+json" in data

@pytest.mark.skipif(not _have("matplotlib"), reason="matplotlib not installed")
def test_matplotlib_inline_backend_configures():
    import matplotlib
    matplotlib.use("module://matplotlib_inline.backend_inline")
    import matplotlib.pyplot as plt
    fig, ax = plt.subplots()
    ax.plot([1, 2, 3])
    assert fig is not None
```
Run: `kernel-image/.venv/bin/python -m pytest kernel-image/tests/test_viz.py -q` (skips cleanly if the libs aren't in the local venv; the live e2e is the real check). Verify the whole `kernel-image/tests/` suite still green (the pre-existing sqlalchemy-absent failure aside).

- [ ] **Step 4: Commit.**
```bash
git add kernel-image/Dockerfile kernel-image/startup/00-egret.py kernel-image/tests/test_viz.py
git commit -m "feat(notebook): install plotly/matplotlib/seaborn + numeric stack; plotly mimetype + matplotlib inline"
```

---

## Task 2: FE — PlotlyOutput component (lazy plotly.js, theme, resize)

**Files:** `frontend/package.json` (dep), create `frontend/src/components/notebook/PlotlyOutput.tsx`. Test: `frontend/src/components/notebook/__tests__/PlotlyOutput.test.tsx`.

- [ ] **Step 1: Add the dependency.** `cd frontend && npm install plotly.js-dist-min` (and `npm install -D @types/plotly.js` if types are needed — plotly.js-dist-min ships no types, so declare a minimal module or use `@types/plotly.js`). Confirm `package.json` + `package-lock.json` updated.

- [ ] **Step 2: Write the failing test** `PlotlyOutput.test.tsx` — mock `plotly.js-dist-min` and assert `Plotly.react` is called with the figure's `data` + a merged `layout`, and `Plotly.purge` on unmount:
```tsx
import { render, cleanup } from "@testing-library/react";
import { vi, describe, it, expect, afterEach } from "vitest";

const react = vi.fn();
const purge = vi.fn();
const resize = vi.fn();
vi.mock("plotly.js-dist-min", () => ({
  default: { react, purge, Plots: { resize } },
}));

import PlotlyOutput from "../PlotlyOutput";

afterEach(() => { cleanup(); react.mockClear(); purge.mockClear(); });

describe("PlotlyOutput", () => {
  it("renders the figure via Plotly.react and purges on unmount", async () => {
    const figure = { data: [{ type: "bar", x: ["a"], y: [1] }], layout: { title: "t" } };
    const { unmount } = render(<PlotlyOutput figure={figure} />);
    // the dynamic import + effect are async; flush microtasks
    await vi.waitFor(() => expect(react).toHaveBeenCalled());
    const [, data, layout] = react.mock.calls[0];
    expect(data).toEqual(figure.data);
    expect(layout).toMatchObject({ title: "t" }); // user layout preserved
    unmount();
    expect(purge).toHaveBeenCalled();
  });
});
```
Run `npm run test -- PlotlyOutput` → FAIL (component missing).

- [ ] **Step 3: Implement `PlotlyOutput.tsx`** (default export; mirror `EChartsRenderer`):
```tsx
import { useEffect, useRef } from "react";

export interface PlotlyFigure {
  data: unknown[];
  layout?: Record<string, unknown>;
  config?: Record<string, unknown>;
}

/** Render a plotly figure (from application/vnd.plotly.v1+json) with a lazily
 *  imported plotly.js. Theme-aware (transparent bg + themed font), responsive,
 *  purged on unmount. Mirrors EChartsRenderer's init/resize/dispose lifecycle. */
export default function PlotlyOutput({ figure }: { figure: PlotlyFigure }) {
  const elRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = elRef.current;
    if (!el) return;
    let disposed = false;
    let ro: ResizeObserver | undefined;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    let plotly: any;

    const dark = document.documentElement.classList.contains("dark");
    const themedLayout = {
      paper_bgcolor: "rgba(0,0,0,0)",
      plot_bgcolor: "rgba(0,0,0,0)",
      font: { color: dark ? "#e5e7eb" : "#111827" },
      ...(figure.layout ?? {}),           // user layout wins over theme defaults
      autosize: true,
    };
    const config = { responsive: true, displaylogo: false, ...(figure.config ?? {}) };

    void import("plotly.js-dist-min").then((mod) => {
      if (disposed) return;
      plotly = mod.default;
      plotly.react(el, figure.data, themedLayout, config);
      ro = new ResizeObserver(() => plotly?.Plots?.resize(el));
      ro.observe(el);
    });

    return () => {
      disposed = true;
      ro?.disconnect();
      if (plotly && el) plotly.purge(el);
    };
  }, [figure]);

  return <div ref={elRef} className="h-full w-full" />;
}
```
(If `@typescript-eslint/no-explicit-any` is disallowed by the repo lint, declare a minimal `Plotly` type or `// @ts-expect-error` the dist-min import; match the repo's lint rules.)

- [ ] **Step 4:** `npm run test -- PlotlyOutput`, `npm run typecheck`, `npm run lint` (the new file) clean. Commit.
```bash
git add frontend/package.json frontend/package-lock.json frontend/src/components/notebook/PlotlyOutput.tsx frontend/src/components/notebook/__tests__/PlotlyOutput.test.tsx
git commit -m "feat(notebook): PlotlyOutput — lazy plotly.js renderer (theme + resize)"
```

---

## Task 3: FE — dispatch the plotly MIME in CellOutputRenderer

**Files:** `frontend/src/components/notebook/CellOutputRenderer.tsx`. Test: extend `__tests__/CellOutputRenderer.test.tsx`.

- [ ] **Step 1: Write the failing test** (extend `CellOutputRenderer.test.tsx`): a `display_data` output whose `data` has `application/vnd.plotly.v1+json` renders the `notebook-plotly` container; an `image/png` output still renders an `<img>`. Mock `plotly.js-dist-min` (as in Task 2) so the lazy import resolves; assert `getByTestId("notebook-plotly")` is present (the lazy child may render async — assert the container div renders synchronously). Run → FAIL.

- [ ] **Step 2: Add the lazy import + dispatch branch.** At the top of `CellOutputRenderer.tsx`, add:
```tsx
import { lazy, Suspense } from "react";
const PlotlyOutput = lazy(() => import("./PlotlyOutput"));
const PLOTLY_MIME = "application/vnd.plotly.v1+json";
```
In `MimeBundle` (line 48), BEFORE the `const html` branch:
```tsx
  const plotlyRaw = data[PLOTLY_MIME];
  if (plotlyRaw != null) {
    const figure = (typeof plotlyRaw === "string" ? JSON.parse(plotlyRaw) : plotlyRaw) as {
      data: unknown[]; layout?: Record<string, unknown>; config?: Record<string, unknown>;
    };
    return (
      <div className="h-96 w-full" data-testid="notebook-plotly">
        <Suspense fallback={<div className="text-xs text-muted-foreground">Loading chart…</div>}>
          <PlotlyOutput figure={figure} />
        </Suspense>
      </div>
    );
  }
```
Keep the existing `image/png`/`image/jpeg`/`text/html`/`text/plain` branches unchanged (matplotlib/seaborn keep working via `<img>`).

- [ ] **Step 3:** `npm run test -- CellOutputRenderer`, `typecheck`, `lint`. Commit.
```bash
git add frontend/src/components/notebook/CellOutputRenderer.tsx frontend/src/components/notebook/__tests__/CellOutputRenderer.test.tsx
git commit -m "feat(notebook): render application/vnd.plotly.v1+json via lazy PlotlyOutput"
```

---

## Task 4: e2e — plotly + matplotlib render live

**Files:** Create `frontend/tests/e2e/notebook-viz-kind.spec.ts`; `justfile`.

- [ ] **Step 1: Spec** (opt-in `EGRET_E2E_KIND=1`, `test.setTimeout(300_000)`, `loginLocalAdmin`, model on `notebook-ecommerce.spec.ts` for driving Monaco + Run all):
  - Test "a plotly cell renders an interactive chart": new notebook, first cell:
    ```python
    import plotly.express as px
    fig = px.bar(x=["a", "b", "c"], y=[1, 2, 3])
    fig
    ```
    Run all → assert `page.getByTestId("notebook-plotly")` visible (150s cold-start wait) and it contains a plotly-rendered node (e.g. `.locator("[data-testid=notebook-plotly] svg, [data-testid=notebook-plotly] .plotly")` visible).
  - Test "a matplotlib cell renders an image": a cell:
    ```python
    import matplotlib
    import matplotlib.pyplot as plt
    fig, ax = plt.subplots(); ax.plot([1, 2, 3]); plt.show()
    ```
    Run → assert an `<img>` output appears (`page.locator("img[src^='data:image/png']")` visible). (Best-effort: if the inline backend doesn't emit under Kernel Gateway, adjust in the live run.)

- [ ] **Step 2: `just` recipe** `e2e-kind-notebook-viz` mirroring `e2e-kind` (notebook kernel stack via `deploy-kind-oidc`, no Ranger), running `npx playwright test notebook-viz-kind`.

- [ ] **Step 3:** `npm run typecheck` + `eslint` the spec clean. Commit.
```bash
git add frontend/tests/e2e/notebook-viz-kind.spec.ts justfile
git commit -m "test(notebook): e2e — plotly + matplotlib render in the notebook"
```

---

## Task 5: Verify + live-deploy + run

- [ ] **Step 1: Full FE CI + build (bundle check).** `npm run lint` · `typecheck` · `test` · `build`. In the build output, CONFIRM plotly.js is code-split into its OWN chunk (a separate `plotly*.js` asset), not merged into the main `index-*.js` — the whole point of the lazy import. If it's not split, add a `manualChunks` entry for `plotly.js-dist-min` in `vite.config.ts`. Kernel pytest: `kernel-image/.venv/bin/python -m pytest kernel-image/tests/ -q` (viz tests skip locally if libs absent).
- [ ] **Step 2: Rebuild + deploy.** Build+load `egret-frontend:dev` + `egret-kernel:dev`; `kubectl rollout restart deploy/egret-frontend`; delete stale kernel pods (new image). Deploy is OIDC (kernel stack), no Ranger.
- [ ] **Step 3: Live run** `just e2e-kind-notebook-viz` (or the direct playwright invocation after a rollout). Confirm: a `px.bar` cell renders an interactive plotly chart (hover works); a matplotlib cell renders a PNG. Verify the plotly MIME actually appears (if plotly still emits `text/html`, fix the `pio.renderers.default` at startup and re-verify — the design §6.1 risk).

---

## Self-Review Notes
- No backend change — kernel image + FE only. plotly.js is lazy-imported (own chunk) so the main bundle doesn't grow for non-notebook users (verified in Task 5 Step 1).
- matplotlib/seaborn work through the existing `image/png` → `<img>` path (no FE change needed beyond the install); only plotly needs the new MIME + component.
- Theme detection via the `.dark` root class (the app's `theme-provider`); user figure layout wins over theme defaults (only bg/font overlaid).
- The startup renderer config is the live linchpin (design §6.1): plotly MUST emit `application/vnd.plotly.v1+json`, verified in the live run.
- Out of scope: other HTML viz libs (bokeh/altair-html), the ECharts `chart()` helper, notebook-side viz wrappers.
