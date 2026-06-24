# Date Grouping — Sub-project C: Tham số Date-Grouping cấp dashboard (Thiết kế)

**Bản tham chiếu tiếng Việt.** Bản chuẩn: `docs/en/specs/2026-06-24-date-grouping-dashboard-param-design.md`.
**Thuộc chương trình:** Date Grouping (A → B → C). Đây là **C**, phụ thuộc A (PR #47) + B (PR #48).
**C chỉ frontend.** Backend A đã áp `temporal_units`.

## Mục tiêu

Một tham số dashboard (pill ở filter bar) ghi đè **đơn vị** gom thời gian cho các card nó được nối tới,
re-grid tất cả cùng lúc (vd lật mọi card time-series từ tháng sang quý). Tái dùng hạ tầng map param
per-card sẵn có, đúng kiểu Metabase.

## Quyết định (brainstorm)

- **Map per-card** (đúng Metabase): tái dùng `card.options.paramMappings` (param id → cột). Chỉ ảnh hưởng
  card/cột người dùng nối.
- **Toàn bộ đơn vị** (truncate + extract), tái dùng `TEMPORAL_UNITS` của B.

## Quy tắc cốt lõi — chỉ ghi đè cột ĐÃ được gom

`applyParamsToChart` ghi đè `spec.temporal_units[cộtMap]` **chỉ khi `cộtMap` đã là key trong
`spec.temporal_units` của card đó** (đã gom thời gian bởi B — vốn auto-default mọi dimension temporal về
`month`). Quy tắc này:
- **An toàn**: cột đã là dimension temporal hợp lệ → backend A không bao giờ từ chối (không 400 orphan-key,
  không "non-temporal column");
- **Không cần metadata `is_temporal`** ở tầng param — chỉ đọc `spec.temporal_units` của chính chart;
- Đúng mô hình: param "re-grid các card thời gian", không biến cột chưa gom thành gom.

Map tới cột chưa gom → no-op âm thầm (không lỗi).

## Component (đều tái dùng hạ tầng)

1. **Loại param mới** (`dashboards.ts`): thêm `"temporal_unit"` vào `ParamType`. Giá trị = token đơn vị
   (`month`, `day_of_week`…). Không có operator filter.
2. **Tạo param** (`ParameterEditor.tsx`): thêm `temporal_unit` vào `OPERATORS` (để hiện trong dropdown
   type; operator để placeholder `["is"]`, `showOp` vẫn false cho nó). Khi chọn type → set
   `param.default = "month"`. Thêm dropdown đơn vị mặc định (bind `param.default`), chỉ hiện cho temporal_unit.
3. **Widget view-mode** (`FilterPill.tsx`): icon `Clock`; với `type === "temporal_unit"` render dropdown đơn
   vị từ `TEMPORAL_UNITS` (nhóm Group-by/Extract như B), chọn → set value = token. Không có "None" (luôn có
   đơn vị; clear → fallback về `default`).
4. **Ghi đè** (`params.ts` `applyParamsToChart`): thu thập `temporalOverrides` song song với `extra` filter.
   Với param temporal_unit có cột map + `cột in spec.temporal_units` → `temporalOverrides[cột] = unit`. Return
   sớm chỉ khi cả `extra` lẫn `temporalOverrides` rỗng. Merge:
   `temporal_units: { ...(spec.temporal_units ?? {}), ...temporalOverrides }`.
5. **Nhãn pill** (`summarize`): type temporal_unit → trả nhãn đơn vị
   (`TEMPORAL_UNITS.find(u=>u.token===value)?.label ?? String(value)`), vd `month` → `Month`.
6. **Picker map cột** (`DashboardView.tsx` ~437): với param temporal_unit đang active, **lọc options cột về
   các cột đã gom** (keys của `chart.params.spec.temporal_units` của card đó). Dữ liệu có sẵn trên chart.

## Luồng

Edit → thêm param type "Date grouping" (default month), nối tới cột ngày đã-gom của từng card. View → pill
dropdown đơn vị; đổi → mọi card đã map re-query đơn vị mới, re-render cùng lúc. Card vừa có filter param vừa
date-grouping → áp cả hai trong 1 lượt.

## Test

`applyParamsToChart`: param temporal_unit map tới cột ∈ temporal_units → ghi đè; cột ∉ temporal_units →
no-op; filter param + temporal_unit cùng card → cả hai áp; không map gì → chart nguyên. `summarize`: month→
Month, day_of_week→Day of week. `ParameterEditor`: chọn temporal_unit → default=month, ẩn operator.
`FilterPill`: temporal_unit render dropdown + icon Clock. Live verify (dashboard throwaway, KHÔNG đụng a1, 0
console error): thêm param Date-grouping, map 2 card time-series, đổi Month→Quarter → cả 2 re-grid quý; card
không map không đổi; reload giữ param+map. Sweep xanh.

## File

Sửa: `dashboards.ts`, `params.ts`, `FilterPill.tsx`, `ParameterEditor.tsx`, `DashboardView.tsx`. Tái dùng
`temporal-units.ts`.

## Ngoài phạm vi

Không đụng backend; không Object/Map viz; không thêm hạ tầng map mới; param chỉ re-grid cột đã-gom (không
gom cột raw mới). Sweep + git identity + footer Co-Authored-By; stage path tường minh.
