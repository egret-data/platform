# Tier 2d — Kế hoạch triển khai Đại tu Filter UX

> Bản tiếng Việt tham chiếu. Bản chuẩn (canonical) — gồm toàn bộ code từng task — là tiếng Anh:
> `docs/en/plans/2026-06-22-tier2d-filter-ux.md`. Spec: `docs/en/specs/2026-06-22-tier2d-filter-ux-design.md`.

**Mục tiêu:** Đưa filter dashboard egret theo UX Metabase — map cột trên card, widget giá trị,
default, linked (chain) filter, và type mới boolean / location / month-year / quarter-year.
Chỉ frontend (param nằm trong `dashboards.parameters` jsonb — không đổi backend/migration).

**Quy ước mọi task:** TDD (đỏ → xanh); trước mỗi commit chạy full sweep từ `frontend/`:
`npm run typecheck && npm run lint && npm run build && npx vitest run`; stage path tường minh;
identity git đã cấu hình + footer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
Branch `feat/dashboards-tier2d-filter-ux` (off main).

**Defer (không làm):** text does-not-contain (cần backend `not_like`), date all-options, per-card
viz-settings override, link→dashboard/question.

## Các task (xem code đầy đủ ở bản tiếng Anh)

1. **Data model** — `api/dashboards.ts`: `ParamType` thêm `boolean`/`location`; `ParamOperator` thêm
   `month-year`/`quarter-year`; `ParamWidget`; `DashboardParameter.widget?`+`filteringParameters?`.
   `OPERATORS` (`ParameterEditor.tsx`) thêm `boolean:["is"]`, `location:["is","is-not"]`, date thêm
   month/quarter-year.
2. **`paramToFilters` + `summarize`** (`params.ts`) cho type mới: boolean→`eq` bool; location is/is-not
   →`in`/`not_in`; month-year→`ge`/`le` đầu–cuối tháng; quarter-year→`ge`/`le` đầu–cuối quý;
   `summarize` boolean True/False, quarter-year "Q2 2024", location đếm.
3. **`useColumnValues`** thêm arg `filters?: ChartFilter[]` nối vào spec `{columns,filters}` + vào key.
4. **`FilterPill`** tôn trọng `widget` (input→text scalar) + control type mới (boolean 3-trạng-thái;
   location multi-checkbox; month-year `<input type=month>`; quarter-year year+select quý).
5. **`FilterSettingsPanel`** thêm `allParameters`, radio widget (Dropdown/Search/Input), ô Default,
   mục Linked filters (checkbox mỗi param khác → `filteringParameters`); `EditSidebar` truyền
   `allParameters`.
6. **Map cột trên card** — `CardFilterMapOverlay` mới; trong `DashboardView`, khi
   `editPanel.kind==="filter"` mỗi chart card hiện overlay "Column to filter on" ghi `paramMappings`;
   **bỏ** mục map per-param khỏi `CardSettingsPanel`.
7. **Linked chain** — helper thuần `computeLinkedFilters(param, mappings, parameters, values)`;
   `ParamOptions` truyền vào `useColumnValues(...,filters)`; loader map thêm `location`.
8. **Verify live + PR** — full sweep; chạy stack + đăng nhập `admin@egret.local`; Playwright MCP song
   song Metabase (map cột, widget, boolean/location/month/quarter, link 2 filter thu hẹp dropdown,
   default apply khi reload), **0 lỗi console**; **không đụng dashboard "a1"**, xóa dashboard test;
   push + PR + squash-merge khi CI xanh.
