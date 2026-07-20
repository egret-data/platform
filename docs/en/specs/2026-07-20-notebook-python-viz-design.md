# Notebook external Python viz libraries (plotly + matplotlib/seaborn + numeric stack) — Design

## 1. Goal

Data analysts / data scientists can use standard Python viz in notebook cells for
ad-hoc analysis: **interactive plotly** charts (hover / zoom / theme / resize),
plus **matplotlib / seaborn** static plots, plus the numeric/ML stack
(numpy / scipy / scikit-learn) — all rendered natively in the egret notebook.

## 2. Current state (verified)

- **Kernel image ships no viz libs.** `kernel-image/Dockerfile:14` installs
  `jupyter_kernel_gateway ipykernel daft pyiceberg[s3fs] pymysql sqlglot sqlalchemy
  trino pandas` — no plotly, matplotlib, seaborn, numpy(explicit), scipy, sklearn.
- **The notebook output renderer strips scripts.** `CellOutputRenderer.MimeBundle`
  (`frontend/src/components/notebook/CellOutputRenderer.tsx`) renders `image/png`/
  `image/jpeg` via `<img>` (so **matplotlib works once installed**), but `text/html`
  goes through `DOMPurify.sanitize(...)` which strips `<script>` — so **plotly's
  interactive HTML (plotly.js) is broken**. There is no plotly-specific MIME
  handling and no theme/resize support.
- The FE has **no plotly dependency**. The existing `EChartsRenderer`
  (`frontend/src/components/chart/echarts/EChartsRenderer.tsx`) is the pattern to
  mirror: init on mount, resize via `ResizeObserver`, dispose on unmount.

## 3. Requirements (decisions from brainstorming)

| Decision | Choice |
| --- | --- |
| plotly rendering | **Native MIME** — kernel emits `application/vnd.plotly.v1+json`; FE renders with **plotly.js, lazy-loaded** (theme-aware + resizable) |
| matplotlib/seaborn | **image/png** (already rendered by `<img>`; just install + configure inline) |
| Libraries | `plotly matplotlib seaborn numpy scipy scikit-learn` |
| FE bundle | plotly.js **dynamically imported** (not in the main bundle; loads only when a cell has a plotly output) |

## 4. Architecture

### 4.1 Kernel image — install libs + configure renderers

**Files:** `kernel-image/Dockerfile`, `kernel-image/startup/00-egret.py`.

- Add `plotly matplotlib seaborn numpy scipy scikit-learn` to the `pip install`
  line (Dockerfile:14). (numpy/pandas are largely pulled by daft/pandas already;
  explicit is fine.)
- In `startup/00-egret.py` (best-effort, guarded — never break kernel start):
  - `import plotly.io as pio; pio.renderers.default = "plotly_mimetype"` so a
    displayed figure emits the `application/vnd.plotly.v1+json` MIME bundle (the
    JupyterLab-style renderer) rather than `text/html`. Verify the emitted MIME
    live (plotly's default in a bare kernel may be `text/html`/`browser`).
  - Ensure matplotlib uses the inline backend (ipykernel provides it; if a figure
    isn't auto-displayed as `image/png`, set `matplotlib.use("module://matplotlib_inline.backend_inline")`
    or run the `%matplotlib inline` equivalent at startup).

### 4.2 FE — plotly MIME rendering (lazy plotly.js)

**Files:** `frontend/package.json` (dep), a new
`frontend/src/components/notebook/PlotlyOutput.tsx`,
`frontend/src/components/notebook/CellOutputRenderer.tsx` (dispatch).

- Add `plotly.js-dist-min` as a dependency (the standalone bundle; no React
  wrapper needed — a thin component gives us lazy-load + theme + resize control,
  mirroring `EChartsRenderer`).
- `PlotlyOutput({ figure })` — `figure` = the `{ data, layout, config }` from the
  MIME bundle:
  - `useEffect` on mount: `const Plotly = (await import("plotly.js-dist-min")).default;`
    then `Plotly.react(elRef.current, figure.data, mergedLayout, { responsive: true, ...figure.config })`.
  - Theme: merge a dark/light template into `layout` (transparent
    `paper_bgcolor`/`plot_bgcolor` + font color from the app's theme — detect via
    the app's dark/light mechanism, resolve exact mechanism in planning). Re-render
    on theme change.
  - Resize: `ResizeObserver` → `Plotly.Plots.resize(el)`. Dispose:
    `Plotly.purge(el)` on unmount.
  - Render into a fixed-height container (e.g. `h-96 w-full`, like the notebook's
    other outputs) that plotly fills.
- In `CellOutputRenderer.MimeBundle`, add — checked before the `text/html` branch:
  ```tsx
  const plotly = data["application/vnd.plotly.v1+json"];
  if (plotly != null) {
    const fig = typeof plotly === "string" ? JSON.parse(plotly) : plotly;
    return <div className="h-96 w-full" data-testid="notebook-plotly">
      <React.Suspense fallback={<div className="text-xs text-muted-foreground">Loading chart…</div>}>
        <PlotlyOutput figure={fig} />
      </React.Suspense></div>;
  }
  ```
  (Use `React.lazy(() => import("./PlotlyOutput"))` so plotly.js only loads when a
  plotly output is present.) Keep `image/png`/`image/jpeg` for matplotlib/seaborn.
- The existing `OutputErrorBoundary` already wraps outputs — a malformed figure is
  contained to one cell.

### 4.3 Tests

**Files:** `kernel-image/tests/test_viz.py` (new); a FE test near CellOutputRenderer;
`frontend/tests/e2e/notebook-viz-kind.spec.ts` (new) + a `just` recipe.

- **Kernel pytest:** `import plotly, matplotlib, seaborn, numpy, scipy, sklearn`
  succeed; a `plotly.express` figure's `_repr_mimebundle_` (or `pio.to_...`)
  contains the `application/vnd.plotly.v1+json` key; matplotlib inline emits
  `image/png`. (Mock/stub only what's needed — these libs are installed in the
  image; the local `.venv` may lack them, so gate on import-availability and mark
  skip if absent, like the sqlalchemy-absent case.)
- **FE unit:** the MimeBundle with `application/vnd.plotly.v1+json` renders the
  `notebook-plotly` container + (with a mocked `plotly.js-dist-min`) calls
  `Plotly.react` with the figure's `data`/`layout`. An `image/png` bundle still
  renders `<img>`.
- **e2e (kind):** a notebook cell `import plotly.express as px; px.bar(x=["a","b"],
  y=[1,2])` renders a plotly chart (assert `getByTestId("notebook-plotly")` +
  an svg/plotly node); a `matplotlib`/`seaborn` cell renders an `<img>`.

## 5. Data flow

```
notebook cell: import plotly.express as px; fig = px.bar(...); fig   (last expr / fig.show())
  └─ kernel (pio.renderers.default="plotly_mimetype")
       → display_data { "application/vnd.plotly.v1+json": {data, layout, config}, "text/html": <fallback> }
         → persisted in last_output → WS → CellOutputRenderer

FE CellOutputRenderer.MimeBundle
  └─ sees application/vnd.plotly.v1+json  → <PlotlyOutput> (lazy import plotly.js)
       → Plotly.react(el, data, layout+theme, {responsive:true}) → interactive chart

matplotlib: plt.plot(...); plt.show()  → image/png → <img>   (unchanged)
```

## 6. Edge cases, risks & open questions (resolve in planning)

1. **plotly default renderer.** In a bare kernel plotly may default to
   `text/html`/`browser`; MUST set `pio.renderers.default = "plotly_mimetype"` at
   startup. Verify the emitted MIME live (the whole feature hinges on the
   `application/vnd.plotly.v1+json` bundle appearing).
2. **matplotlib inline.** ipykernel usually auto-inlines, but under Kernel Gateway
   confirm `plt.show()` (or the last figure) emits `image/png`; set the inline
   backend explicitly at startup if not.
3. **Theme.** Resolve how the FE detects dark/light (root class / `data-theme` /
   `prefers-color-scheme`) and apply a plotly template; keep it simple
   (transparent bg + themed font) so plotly matches the app.
4. **Bundle size / lazy-load.** plotly.js-dist-min (~3.5MB) MUST be dynamically
   imported (`React.lazy`/`import()`) so it's excluded from the main bundle and the
   `npm run build` size doesn't regress for non-notebook users. Confirm the build
   code-splits it into its own chunk.
5. **Figure JSON in `last_output`.** The plotly MIME bundle (figure data) is
   persisted per cell; large plots bloat cell storage. Inherent to plotly; note it,
   no cap in v1.
6. **plotly `config`/`layout` merge.** Respect the figure's own layout; only
   overlay theme + `responsive:true` without clobbering user settings.

## 7. Testing strategy

Kernel pytest (imports + plotly mimebundle + matplotlib png), FE unit (plotly MIME
→ PlotlyOutput + `Plotly.react`; matplotlib png → img), e2e (plotly + matplotlib
cells render live). Before merge: FE lint/typecheck/test/build (watch bundle
code-split); kernel pytest; the standard Rust gates are unaffected (no backend
change).

## 8. Out of scope

- Other HTML-emitting viz libs (bokeh, altair-as-html) — the general sandboxed-
  iframe path was NOT chosen; only plotly's native MIME is rendered. (matplotlib/
  seaborn work via image/png.)
- The egret-native `chart()` ECharts helper (a separate, unmerged branch) — this
  feature uses plotly instead, per the DA/DS preference.
- Notebook-side viz helpers/wrappers — users call plotly/matplotlib directly.
- Any backend/manifest change (this is kernel image + FE rendering only).
