# Date Grouping — Sub-project C (Param Date-Grouping dashboard) — Plan (tham chiếu VN)

**Bản chuẩn:** `docs/en/plans/2026-06-24-date-grouping-dashboard-param.md`. **Spec:**
`docs/en/specs/2026-06-24-date-grouping-dashboard-param-design.md`. **Branch:** `feat/date-grouping-dashboard-param`.
**Phụ thuộc:** A (PR #47) + B (PR #48, `temporal-units.ts` → `TEMPORAL_UNITS`). Chỉ frontend. Sweep
`npm run typecheck && lint && build && vitest` (từ `frontend/`) trước mỗi commit; footer Co-Authored-By;
không push tới Task 5.

**Task 1 — type + override + summary:** thêm `"temporal_unit"` vào `ParamType` (`dashboards.ts`).
`applyParamsToChart` (`params.ts`): thu `temporalOverrides` song song `extra`; với param temporal_unit có
cột map + `cột in spec.temporal_units` → override (unit = value, rỗng → `param.default`); chỉ thêm
`temporal_units` vào spec trả về khi CÓ override (giữ test filter cũ xanh). `summarize`: type temporal_unit →
nhãn từ `TEMPORAL_UNITS`. Test mới `params.temporal.test.ts` (override cột đã-gom; no-op cột chưa-gom;
fallback default; không map → nguyên; summarize month→Month). Commit.

**Task 2 — ParameterEditor:** thêm `temporal_unit: ["is"]` vào `OPERATORS`; onChange type → set
`default:"month"` khi chọn temporal_unit; thêm `<select aria-label="default unit">` (từ TEMPORAL_UNITS) chỉ
hiện cho temporal_unit; operator tự ẩn (showOp loại trừ). Test: chọn type → default month + ẩn operator +
có "default unit". Commit.

**Task 3 — FilterPill:** import `Clock`; `iconFor` thêm temporal_unit → Clock; `FilterControl` thêm nhánh
temporal_unit → `<select>` optgroup Group-by/Extract từ TEMPORAL_UNITS, value `?? param.default ?? "month"`.
Test: render dropdown + chọn quarter → onChange("quarter"). Commit.

**Task 4 — DashboardView mapping picker (~436):** prop `columns` của `CardFilterMapOverlay` → nếu param
active là temporal_unit thì `Object.keys(baseChart.params.spec.temporal_units ?? {})`, ngược lại
`[...dimensions, ...columns]`. Sweep + commit.

**Task 5 — verify + PR:** sweep. Playwright (backend rebuild A+B, dashboard throwaway KHÔNG đụng a1, 0
console error): 2 card bar gom theo created_at (Month); thêm param type temporal_unit (default Month), map
2 card (picker chỉ hiện created_at); pill Clock "Date grouping: Month"; đổi Quarter → cả 2 re-grid quý; card
không map không đổi; reload giữ param+map+unit. Xoá dashboard. Push + PR.
