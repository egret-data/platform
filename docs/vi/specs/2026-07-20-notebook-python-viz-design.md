# Thư viện viz Python ngoài cho notebook (plotly + matplotlib/seaborn + numeric stack) — Thiết kế

## 1. Mục tiêu

DA/DS dùng được viz Python chuẩn trong cell notebook cho ad-hoc analysis: chart
**plotly tương tác** (hover/zoom/theme/resize), cộng **matplotlib/seaborn** (ảnh
tĩnh), cộng numeric/ML stack (numpy/scipy/scikit-learn) — render native trong
notebook egret.

## 2. Hiện trạng (đã kiểm)

- **Kernel image không có lib viz.** `kernel-image/Dockerfile:14` cài
  `jupyter_kernel_gateway ipykernel daft pyiceberg[s3fs] pymysql sqlglot sqlalchemy
  trino pandas` — không có plotly/matplotlib/seaborn/scipy/sklearn.
- **Renderer output gỡ script.** `CellOutputRenderer.MimeBundle`
  (`frontend/src/components/notebook/CellOutputRenderer.tsx`) render `image/png`/
  `image/jpeg` qua `<img>` (nên **matplotlib chạy được khi cài**), nhưng `text/html`
  đi qua `DOMPurify.sanitize(...)` gỡ `<script>` → **plotly (plotly.js) bị hỏng**.
  Chưa có xử lý MIME plotly, chưa có theme/resize.
- FE **chưa có dep plotly**. `EChartsRenderer`
  (`frontend/src/components/chart/echarts/EChartsRenderer.tsx`) là pattern để mirror:
  init on mount, resize qua `ResizeObserver`, dispose on unmount.

## 3. Yêu cầu (chốt từ brainstorm)

| Quyết định | Chọn |
| --- | --- |
| Render plotly | **MIME native** — kernel xuất `application/vnd.plotly.v1+json`; FE render bằng **plotly.js lazy-load** (theme-aware + resizable) |
| matplotlib/seaborn | **image/png** (đã render bằng `<img>`; chỉ cài + cấu hình inline) |
| Thư viện | `plotly matplotlib seaborn numpy scipy scikit-learn` |
| Bundle FE | plotly.js **dynamic import** (không nằm main bundle; chỉ load khi cell có output plotly) |

## 4. Kiến trúc

### 4.1 Kernel image — cài lib + cấu hình renderer

**File:** `kernel-image/Dockerfile`, `kernel-image/startup/00-egret.py`.

- Thêm `plotly matplotlib seaborn numpy scipy scikit-learn` vào dòng `pip install`
  (Dockerfile:14).
- Trong `startup/00-egret.py` (best-effort, guarded — không được làm kernel chết):
  - `import plotly.io as pio; pio.renderers.default = "plotly_mimetype"` để figure
    hiển thị xuất MIME `application/vnd.plotly.v1+json` (kiểu JupyterLab) thay vì
    `text/html`. Verify MIME thật khi chạy live (plotly bare kernel có thể mặc định
    `text/html`/`browser`).
  - Đảm bảo matplotlib dùng inline backend (ipykernel có sẵn; nếu figure không
    tự hiện `image/png`, set inline backend / `%matplotlib inline` ở startup).

### 4.2 FE — render MIME plotly (lazy plotly.js)

**File:** `frontend/package.json` (dep), mới `PlotlyOutput.tsx`,
`CellOutputRenderer.tsx` (dispatch).

- Thêm dep `plotly.js-dist-min` (bundle standalone; không cần wrapper React — 1
  component mỏng cho lazy-load + theme + resize, mirror `EChartsRenderer`).
- `PlotlyOutput({ figure })` — `figure` = `{ data, layout, config }` từ MIME:
  - `useEffect` on mount: `const Plotly = (await import("plotly.js-dist-min")).default;`
    rồi `Plotly.react(el, figure.data, layout+theme, { responsive: true, ...config })`.
  - Theme: merge template dark/light (paper/plot bg trong suốt + màu font theo theme
    app; cách detect resolve khi plan). Re-render khi đổi theme.
  - Resize: `ResizeObserver` → `Plotly.Plots.resize(el)`. Dispose: `Plotly.purge(el)`.
  - Container cao cố định (vd `h-96 w-full`).
- Trong `CellOutputRenderer.MimeBundle`, thêm — trước nhánh `text/html`:
  `application/vnd.plotly.v1+json` → `<PlotlyOutput>` bọc `React.Suspense` +
  `React.lazy(() => import("./PlotlyOutput"))` (plotly.js chỉ load khi có output
  plotly). Giữ `image/png`/`image/jpeg` cho matplotlib. `OutputErrorBoundary` sẵn có
  bọc lỗi figure hỏng về 1 cell.

### 4.3 Test

**File:** `kernel-image/tests/test_viz.py` (mới); test FE cạnh CellOutputRenderer;
`frontend/tests/e2e/notebook-viz-kind.spec.ts` (mới) + just recipe.

- **Kernel pytest:** import plotly/matplotlib/seaborn/numpy/scipy/sklearn thành công;
  figure `plotly.express` có key `application/vnd.plotly.v1+json` trong mimebundle;
  matplotlib inline xuất `image/png`. (Gate theo import-availability, skip nếu venv
  local thiếu — như case sqlalchemy.)
- **FE unit:** MimeBundle với `application/vnd.plotly.v1+json` render container
  `notebook-plotly` + (mock `plotly.js-dist-min`) gọi `Plotly.react` với figure. Bundle
  `image/png` vẫn render `<img>`.
- **e2e (kind):** cell `import plotly.express as px; px.bar(...)` render chart plotly
  (assert `getByTestId("notebook-plotly")` + node svg/plotly); cell matplotlib/seaborn
  render `<img>`.

## 5. Luồng dữ liệu

```
cell: import plotly.express as px; fig = px.bar(...); fig   (last expr / fig.show())
  └─ kernel (pio.renderers.default="plotly_mimetype")
       → display_data { "application/vnd.plotly.v1+json": {data, layout, config}, "text/html": fallback }
         → last_output → WS → CellOutputRenderer
  └─ MimeBundle thấy vnd.plotly → <PlotlyOutput> (lazy plotly.js) → Plotly.react → chart tương tác

matplotlib: plt.show() → image/png → <img>   (không đổi)
```

## 6. Edge case, rủi ro & câu hỏi mở (giải khi plan)

1. **Renderer mặc định plotly.** Bare kernel có thể mặc định `text/html`; PHẢI set
   `pio.renderers.default = "plotly_mimetype"`. Verify MIME live (cả feature phụ thuộc
   bundle `application/vnd.plotly.v1+json` xuất hiện).
2. **matplotlib inline.** ipykernel thường auto-inline; dưới Kernel Gateway verify
   `plt.show()` xuất `image/png`; set inline backend tường minh nếu không.
3. **Theme.** Resolve cách FE detect dark/light (class root / `data-theme` /
   `prefers-color-scheme`) + apply template plotly; giữ đơn giản (bg trong suốt + font
   theo theme).
4. **Bundle size / lazy-load.** plotly.js-dist-min (~3.5MB) PHẢI dynamic import
   (`React.lazy`/`import()`) để không vào main bundle; verify build tách chunk riêng.
5. **Figure JSON trong `last_output`.** Bundle MIME plotly (data figure) lưu mỗi cell;
   plot lớn phình storage. Bản chất plotly; ghi chú, không cap ở v1.
6. **Merge `config`/`layout`.** Tôn trọng layout của figure; chỉ overlay theme +
   `responsive:true`, không đè setting user.

## 7. Chiến lược test

Kernel pytest (import + mimebundle plotly + matplotlib png), FE unit (plotly MIME →
PlotlyOutput + `Plotly.react`; matplotlib png → img), e2e (cell plotly + matplotlib
render live). Trước merge: FE lint/typecheck/test/build (theo dõi code-split bundle);
kernel pytest; Rust không đổi (không sửa backend).

## 8. Ngoài phạm vi

- Lib viz HTML khác (bokeh, altair-as-html) — không chọn đường sandboxed-iframe; chỉ
  render MIME native của plotly. (matplotlib/seaborn qua image/png.)
- Helper `chart()` ECharts native (nhánh cũ chưa merge) — feature này dùng plotly theo
  ý DA/DS.
- Helper/wrapper viz phía notebook — user gọi plotly/matplotlib trực tiếp.
- Đổi backend/manifest (chỉ kernel image + FE rendering).
