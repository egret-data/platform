# Phase 3c-3.4 — UX trang Chart Explore (gallery loại chart + tab Data/Display)

> Trạng thái: thiết kế đã duyệt. Bản tham chiếu (Tiếng Việt). Bản gốc chuẩn (Tiếng Anh):
> `docs/en/specs/2026-06-19-phase-3c-3-4-explore-ux-design.md`.
> Redesign UX thuần frontend cho trang Chart Explore (Phase 3c-3 … 3c-3.3, đã merge `30c8bbf`).
> Branch `feat/phase-3c-3-4-explore-ux`.

## Mục tiêu

Đưa UX trang Chart Explore ngang Metabase/Superset (chỉ thị của người dùng: chép sát Metabase —
[[metabase-fidelity-preference]]):

1. Một **gallery loại chart kiểu Superset** khi tạo chart mới.
2. Panel trái của Explore chia thành **hai tab kiểu Metabase — Data và Display** — với mục
   **Format** theo cột và **Conditional Formatting** gộp vào tab Display (bỏ sidebar phải, preview
   chiếm full chiều rộng).
3. Tinh chỉnh thị giác kiểu Metabase (tab switcher gạch chân, section header, spacing nhất quán).

Đây là **tái cấu trúc frontend thuần** — không đổi backend, engine, hay mô hình dữ liệu. Tái dùng
nguyên `FormatSection`, `ConditionalFormattingSection`, renderers, và logic persist/hydrate.

(Ghi chú đánh số lại: phần UX này là **3c-3.4**; table interactions dời sang **3c-3.5**, pivot
collapse/sticky/sort sang **3c-3.6**, drill-through sang **3c-3.7**.)

## Phạm vi

**Trong phạm vi:**
- Một **registry** loại chart (`chartTypes.ts`): `{ key, name, description, icon }` cho các viz
  hỗ trợ (giờ: Table, Pivot Table — mở rộng bar/line/pie sau).
- Component **`ChartTypeGallery`** tái dùng được (full-page khi tạo mới; dạng dialog khi "Đổi
  visualization").
- Routing: `/charts/new` hiện gallery (không `?type`) → chọn loại → editor
  (`/charts/new?type=<key>`); `/charts/:id` mở editor thẳng.
- `ChartExplore` tái cấu trúc: header viz có nút **"Đổi visualization"**, một tab **Data** và một
  tab **Display**; bỏ `SettingsSidebar` bên phải (Format + Conditional vào Display); preview full
  chiều rộng.
- Style Metabase (shadcn `Tabs`, section header in hoa, spacing).

**Ngoài phạm vi:** mọi thay đổi backend/engine; loại viz mới (vẫn Table + Pivot); search /
category sidebar / details pane trong gallery (chỉ 2 loại — lưới thẻ đơn giản; xem lại khi nhiều
loại); table interactions (3c-3.5), pivot collapse (3c-3.6), drill-through (3c-3.7). Shell
`SettingsSidebar.tsx` thành vô dụng và bị xoá.

## 1. Registry loại chart — `frontend/src/components/chart/chartTypes.ts`

```ts
import { Table2, LayoutGrid, type LucideIcon } from "lucide-react";
import type { VizType } from "@/api/charts";

export interface ChartTypeMeta {
  key: VizType;
  name: string;
  description: string;
  icon: LucideIcon;
}

export const CHART_TYPES: ChartTypeMeta[] = [
  { key: "table", name: "Table", description: "Raw rows or an aggregated table with sorting and formatting.", icon: Table2 },
  { key: "pivot_table", name: "Pivot Table", description: "Cross-tab rows × columns × measures with subtotals and grand totals.", icon: LayoutGrid },
];
```

(Chọn icon lucide có thật trong `lucide-react` đã cài; `Table2`/`LayoutGrid` là placeholder —
implementer xác nhận có sẵn.)

## 2. `ChartTypeGallery` — `frontend/src/components/chart/ChartTypeGallery.tsx`

Lưới thẻ tái dùng. Dùng full-page (tạo mới) và trong dialog (đổi).

```tsx
export function ChartTypeGallery({
  selected, onSelect,
}: { selected?: VizType; onSelect: (key: VizType) => void }) { … }
```

- Lưới responsive (`grid grid-cols-2 sm:grid-cols-3 gap-3`) các thẻ. Mỗi thẻ: icon, **tên** (đậm),
  **mô tả** 1 dòng; thẻ được chọn có ring primary đậm.
- Click → `onSelect(key)`. Truy cập bàn phím (`role="button"`, Enter/Space).
- Search / category sidebar / examples pane của Superset chủ ý bỏ (2 loại).

## 3. Routing + flow tạo chart mới

Trong `App.tsx` route đã là `charts/new` → `ChartExplore` và `charts/:id` → `ChartExplore`. Giữ
cả hai trỏ `ChartExplore`; quyết định gallery-hay-editor xử lý bên trong nó:

- `ChartExplore` đọc `id = useParams().id` và `type = useSearchParams().get("type")`.
- **Nếu `!id && !type`** → render `<ChartTypeGallery onSelect={(k) => setSearchParams({ type: k })} />`
  (full-page). Chọn loại → set `?type=<key>` và editor render.
- **Nếu có `type` (mới) hoặc `id` (sửa)** → render editor. Chart mới: `viz` ban đầu lấy từ
  `?type`; chart có sẵn: lấy từ hydration (giữ nguyên).

## 4. Tái cấu trúc editor — `ChartExplore.tsx`

Editor giữ nguyên mọi state và logic (datasetId, pRows/pCols/pVals, tableMetrics/tableCols,
rowLimit, columnSettings, conditionalFormatting, run/poll/save/hydrate). Chỉ đổi **layout**.

### 4.1 Header viz (đầu panel trái)
Toggle Table/Pivot hiện tại được thay bằng header nhỏ hiện tên viz hiện tại + nút **"Đổi
visualization"** mở `ChartTypeGallery` trong `Dialog`. Chọn loại → set `viz` và đóng dialog (giữ
config; đổi viz đã clear query cũ qua `changeViz` sẵn có). Trên preview, `viz` quyết định Table vs
Pivot y như hiện tại.

### 4.2 Hai tab — `Data` và `Display` (shadcn `Tabs`)
Một `<Tabs defaultValue="data">` với `TabsList` gạch chân (kiểu Metabase):

- **Tab Data** (`TabsContent value="data"`): Dataset picker; rồi controls dữ liệu theo viz đã có —
  **Pivot**: `OrderedPicker` Rows / Columns / Values; **Table**: danh sách checkbox Metrics +
  Columns. Rồi **Row limit**.
- **Tab Display** (`TabsContent value="display"`): với **Pivot**, các toggle Row-totals /
  Column-totals / Condense; rồi — cho **cả hai** loại viz — **`FormatSection`** (theo cột kết quả,
  đã lọc `__grp_*`) và **`ConditionalFormattingSection`** (với `isPivot`). Đây chính là các
  component sidebar (đã bỏ) dùng — chuyển vào đây nguyên vẹn. Tab Display hiện hint ("Run the chart
  to configure columns") khi chưa có cột kết quả.

### 4.3 Bỏ sidebar phải
Xoá state `settingsOpen`, nút Settings, phần render `SettingsSidebar` bên phải, và wrapper flex-row.
Vùng preview full chiều rộng. Xoá `SettingsSidebar.tsx` (đã thay thế — Format/Conditional giờ ở tab
Display).

## 5. Style (Metabase)
- Tab switcher: trạng thái active gạch chân (shadcn `Tabs` style, hoặc gạch chân tùy biến nhỏ).
- Section header trong tab: in hoa, nhỏ, semibold, muted (khớp `text-xs font-medium uppercase
  tracking-wide text-muted-foreground` đã dùng).
- Spacing nhất quán (`space-y-3` cho mục, `space-y-1` cho field).

## 6. Persistence / hydration
Không đổi. `paramsForSave()` đã ghi mọi thứ (spec + bố cục pivot + columnSettings +
conditionalFormatting); hydration khôi phục. Việc chia Data/Display thuần trình bày; không đổi shape
param. Chart mới tạo qua gallery lưu cùng `viz_type` + `params`.

## 7. Xử lý lỗi
- Gallery: chọn loại luôn cho `viz` hợp lệ; không có đường lỗi.
- Editor với `?type` không hợp lệ → quay về gallery (coi như không có type).
- Tab Display khi chưa có cột kết quả → hint (không crash), như empty state của sidebar.

## 8. Kiểm thử
- Test `ChartTypeGallery`: render một thẻ mỗi `CHART_TYPES`, gọi `onSelect(key)` khi click, đánh
  dấu thẻ được chọn.
- Test routing/tab của `ChartExplore` nhẹ (chưa có unit test ChartExplore); dựa vào typecheck +
  smoke thủ công. Test renderer/section/engine sẵn có giữ nguyên, phải còn xanh.
- **Smoke Playwright thủ công:** Chart mới → gallery → chọn Pivot → editor; tab Data (rows/cols/
  values) + tab Display (totals + Format currency + một luật Conditional) → Run → preview phản ánh
  cả hai; "Đổi visualization" → Table → chạy lại; Save → mở lại → mọi thứ hydrate.

## 9. Cấu trúc tệp
- Tạo `frontend/src/components/chart/chartTypes.ts`.
- Tạo `frontend/src/components/chart/ChartTypeGallery.tsx` (+ test).
- Sửa `frontend/src/routes/explore/ChartExplore.tsx` (nhánh gallery + layout 2 tab + header viz +
  bỏ sidebar).
- Xoá `frontend/src/components/chart/settings/SettingsSidebar.tsx` (không có test).
- Tái dùng nguyên: `FormatSection`, `ConditionalFormattingSection`, `RuleEditor`, `OrderedPicker`
  (đang trong ChartExplore — giữ), `ChartTable`, `pivot/PivotTable`, `format.ts`, `conditional.ts`,
  `charts.ts`.
- Route `App.tsx` không đổi (cả `charts/new` + `charts/:id` đã → `ChartExplore`).
