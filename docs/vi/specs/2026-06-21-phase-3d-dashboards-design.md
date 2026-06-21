# Phase 3d — Dashboards (thiết kế)

> Bản tiếng Việt tham chiếu. Bản chuẩn (canonical) là tiếng Anh:
> `docs/en/specs/2026-06-21-phase-3d-dashboards-design.md`.

**Trạng thái:** đã duyệt (brainstorm) · **Ngày:** 2026-06-21 · **Tiếp nối:** 3c-3.8 ECharts viz + công việc parity Table/Pivot/Charts (đã merge PR #22–#24)

## Mục tiêu

Thêm **dashboard** cho egret: một lưới (grid) các saved chart (và ở các phase sau
là text card + bộ lọc tương tác) cho phép một workspace gom nhiều câu hỏi lên một
canvas — **bám sát Metabase cả về hành vi lẫn style**, đúng mức độ trung thực đã
áp cho Table, Pivot và Charts.

## Vì sao bám Metabase

Dashboard của Metabase là một **lưới 24 cột** chứa các *dashcard*. Mỗi dashcard
trỏ tới một saved question và được định vị bằng `(col, row, size_x, size_y)` theo
đơn vị ô lưới. Chỉnh sửa là một chế độ riêng (bút chì → kéo/resize/thêm/xoá →
Save); xem thì chạy query từng card độc lập rồi render. **Parameter/filter** cấp
dashboard nối một widget ở trên cùng tới một field trong query của từng card. Ta
tái hiện mô hình này để UX quen thuộc và data model là một superset sạch để lớn
dần lên filter, tab, chia sẻ.

Đây là reimplementation **clean-room**: chỉ sao chép hành vi và style, **không**
dùng mã nguồn AGPL của Metabase. egret giữ giấy phép Apache-2.0.

## Phạm vi (chia phase — mỗi phase 1 PR)

Tính năng lớn nên chia 4 phase. **Spec này mô tả toàn bộ thiết kế A→D; Phase A là
sản phẩm của PR đầu tiên.** Các phase sau có plan triển khai riêng nhưng dùng
chung data model này.

- **Phase A — Lõi (PR này):** CRUD dashboard, lưới 24 cột chứa *question card*
  (mỗi card là một saved chart), view mode (chạy + render mọi card) và edit mode
  (kéo / resize / thêm từ saved charts / xoá / lưu layout).
- **Phase B — Đa dạng card + hoàn thiện:** text / heading / link card (markdown),
  tiêu đề từng card, nhân bản / thay thế card, auto-refresh, fullscreen, night
  mode.
- **Phase C — Filter / parameter:** widget parameter cấp dashboard (date / text /
  number / id), map parameter tới một cột trong query từng card, inject thành SQL
  filter lúc chạy, linked filter, required / locked.
- **Phase D — Nâng cao:** click behaviour (cross-filter, drill-through, custom
  URL), dashboard tab, public link / embed.

### Ngoài phạm vi Phase A (nêu rõ để reviewer biết là cố ý)

Text / heading / link card, dashboard filter, auto-refresh, fullscreen, click
behaviour, tab, public sharing, lịch sử bản sửa. Data model đã được tạo hình để
nhận chúng sau này mà không phải đập migration (`card_type`, `options` jsonb).

## Kiến trúc

### Backend (đã build trong PR này)

Mô phỏng đúng slice `chart` sẵn có trong `egret-bi`.

**Migration `0015_dashboard.sql`:** bảng `dashboards` (unique tên theo workspace)
và `dashboard_cards` (`chart_id` nullable cho text card phase sau; `card_type`;
`grid_col`/`grid_row`/`size_x`/`size_y` theo lưới 24 cột; `options` jsonb;
`ordinal`). Perm `dashboard:read` / `dashboard:manage` cấp cho ws_editor +
ws_admin. Dùng `grid_col` / `grid_row` (không phải `col` / `row`) vì `row` là từ
khoá trong Postgres.

**`DashboardRepo`** (`dashboard.rs`): `create`, `get`, `list_by_workspace` (kèm số
card), `update_meta`, `delete`, `cards`, và **`replace_cards`** — lưu cả layout:
trong một transaction, xoá hết card của dashboard rồi chèn lại đúng thứ tự. Thêm
`dashboard_workspace` và `owner` cho các guard.

**Mô hình lưu: PUT cả layout.** Client luôn gửi *toàn bộ* tập card khi lưu; server
swap nguyên tử. Cách này giữ chỉnh sửa lưới có tính transaction và tránh diff từng
card — đúng kiểu một trình sửa layout cần.

**Routes** (`dashboard_router`): `GET/POST /ws/:w/dashboards`,
`GET/PUT/DELETE /ws/:w/dashboards/:id`. Guard theo thứ tự: quyền workspace
(`dashboard:read` cho GET, `dashboard:manage` cho mutation) → dashboard thuộc
workspace trong path → owner-or-admin với mutation → **mọi `chart_id` trong cards
gửi lên phải thuộc cùng workspace** (nếu không là rò rỉ embed chéo workspace —
bài học từ review nested-ownership của chart). `GET` và `PUT` trả
`DashboardDetailDto` (dashboard + cards theo thứ tự); list trả
`DashboardSummaryDto[]` (kèm `card_count`).

### Frontend — lấy dữ liệu card (quyết định: client-side, tái dùng pipeline)

Một card chạy **đúng pipeline chart sẵn có**, không thêm endpoint backend:
`build-chart-data` (dataset + query spec → SQL + database_id) → `useSubmitQuery`
→ poll `useQueryStatus` / `useResults` tới khi xong → `(columns, rows)`. Mỗi card
fetch **độc lập và song song**, có loading / error / empty riêng, giống Metabase.
Tái dùng toàn bộ thứ trang chart Explore đã làm và **không** thêm bề mặt server
nào cho Phase A.

### Frontend — refactor renderer tái dùng (điều kiện tiên quyết)

Hiện mọi visualization được render **inline** trong `ChartExplore.tsx` (file 54 KB
với một switch lớn dựng option `EChartsRenderer` / `ChartTable` / `BigNumber`).
Card dashboard cần đúng phần render đó, nên Phase A trước hết tách ra:

- **`<ChartRenderer chart columns rows />`** — component thuần trình bày: cho
  `viz_type` + `params` của chart và kết quả `(columns, rows)`, render đúng
  visualization. Đây là switch nhấc nguyên từ `ChartExplore` ra, chỉ phụ thuộc
  input.
- **`useChartData(ws, chart)`** — hook build→submit→poll trả
  `{ columns, rows, isLoading, error }` cho một saved chart.

`ChartExplore` được refactor để dùng `ChartRenderer` nên hành vi không đổi và test
cũ vẫn xanh. Mỗi đơn vị giữ một nhiệm vụ rõ: renderer không biết gì về fetch, hook
không biết gì về render, cả hai test độc lập được.

### Frontend — các trang

- **`api/dashboards.ts`** — type `Dashboard` / `DashboardCard` + hook react-query
  (`useDashboards`, `useDashboard`, `useDashboardMutations`), mô phỏng
  `api/charts.ts`.
- **`Dashboards.tsx`** — trang list (mô phỏng `Charts.tsx`): tên (link), số card,
  updated-at, **+ Dashboard**, delete. Thay placeholder hiện tại.
- **`DashboardView.tsx`** — lưới, dùng **react-grid-layout** (24 cột, row height
  kiểu Metabase), hai chế độ:
  - **View mode:** mỗi card → `useChartData` + `<ChartRenderer>`; loading / error
    từng card; card không kéo được.
  - **Edit mode** (toggle bút chì, như Metabase): card kéo / resize được; sidebar
    **"Add a chart"** liệt kê saved chart để thả lên lưới; xoá card; **Save** (PUT
    cả layout) / **Cancel** (huỷ, refetch).

react-grid-layout là thư viện lưới-dashboard de-facto và chính Metabase dùng; có
sẵn drag, resize, collision. Tương thích React 18.

**Sizing lưới (theo hằng số Metabase).** 24 cột (`GRID_WIDTH = 24`). Row height
suy ra từ width chứ không cố định: `rowHeight = max(width / 24 / (10/9), 40px)` —
tỉ lệ ô `10/9` với tối thiểu `40px` (`GRID_ASPECT_RATIO`, `MIN_ROW_HEIGHT`). Card
có **min size** theo từng loại viz (scalar nhỏ được, table cần to hơn) và
**default size** khi mới thả (Metabase chọn default theo display type). Phase A
dùng một default (vd 6×4) và một min toàn cục; min theo từng viz làm sau.

**Đã validate với Metabase + Superset (2026-06-21).** Metabase xác nhận mô hình:
lưới 24 cột, dashcard phẳng `{col, row, size_x, size_y}`, PUT cả dashboard, fetch
song song từng card (mỗi dashcard một request, không batch), `react-grid-layout`,
edit mode tường minh. Superset đi hướng khác — cây component lồng nhau
(`ROW`/`COLUMN`/`TABS` chứa `CHART`/`MARKDOWN`), 12 cột, `react-dnd`, PUT cả
`position_json` — qua đó xác nhận lưu cả-layout và fetch độc lập từng chart là
chuẩn ngành, đồng thời cho thấy mô hình cây nặng hơn mức ta cần. Ta giữ phẳng
(kiểu Metabase) cho Phase A; **native-filter scoping** của Superset (`targets` +
`chartsInScope`) định hướng Phase C và **tabs** first-class định hướng Phase D.

## Xử lý lỗi

- Card có chart đã bị xoá, hoặc query lỗi: card hiển thị ô lỗi inline, không làm
  hỏng phần còn lại của lưới.
- Trùng tên dashboard trong workspace → `409 Conflict` (unique), hiện toast.
- `chart_id` chéo workspace khi lưu → `404` (guard), hiện toast.
- Không phải owner / admin mà sửa → `403`.

## Kiểm thử

- **Backend:** `dashboard_repo.rs` (gated theo DB, tự skip nếu thiếu
  `DATABASE_URL`) phủ CRUD, `replace_cards` swap toàn bộ, số card, cascade delete,
  unique conflict; `http.rs` thêm smoke test từ chối khi chưa auth. *(Đã xong.)*
- **Frontend (Vitest):** `ChartRenderer` render được từng họ viz từ
  `(columns, rows)`; `useChartData` chạy pipeline (mock api); lưới dashboard map
  cards ↔ layout và editor dựng đúng payload lưu.
- **Live (Playwright):** tạo → edit → thêm chart → kéo / resize → save → reload
  thấy layout đã lưu với card render data; 0 lỗi console.
- **Full sweep trước mỗi commit:** từ `frontend/`,
  `npm run typecheck && npm run lint && npm run build && npm test`.

## Quy ước

Metabase là chuẩn cho mọi quyết định · clean-room (không dùng mã AGPL) · TDD ·
full sweep trước mỗi commit · commit bằng identity đã cấu hình (không bao giờ
`--author`), footer `Co-Authored-By: Claude Opus 4.8 (1M context)
<noreply@anthropic.com>` · chỉ stage path tường minh · squash-merge PR.
