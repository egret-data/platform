# Chart Studio A1 — Kế hoạch triển khai Studio UX

> **Cho worker tự động:** SUB-SKILL BẮT BUỘC: dùng superpowers:subagent-driven-development (khuyến nghị) hoặc superpowers:executing-plans để triển khai plan này theo từng task. Các step dùng cú pháp checkbox (`- [ ]`) để theo dõi.

**Bản canonical (tiếng Anh):** `docs/en/plans/2026-07-27-chart-studio-a1-studio-ux.md` — code trong hai bản giống hệt nhau; bản này dịch phần diễn giải.

**Mục tiêu:** Cho chart builder cảm giác "studio" hiện đại — trạng thái empty/loading/error có ý nghĩa, auto-run hybrid có toggle Auto, và toolbar gọn với tên chart inline — mà **không** đụng phần render chart hay refactor config-model (đó là plan A2).

**Kiến trúc:** Các unit nhỏ, cộng thêm quanh `ChartExplore.tsx` hiện có. Một helper thuần `missingFields()` phục vụ cả validate-khi-run lẫn empty state. Auto-run là một effect debounce, gác bởi một "chữ ký truy vấn" đã serialize + cờ `autoRun`. Các component trình bày mới (`ChartPreviewStates`, `StudioToolbar`) thay JSX inline. **Không** đổi `ChartRenderer`, các option builder ECharts, hay `useSpecData`.

**Tech stack:** React 18, TypeScript, Vite, TanStack Query, shadcn/ui, Vitest + Testing Library. Design token là OKLCH (dùng `var(--token)` trong inline style, không dùng `hsl(var(--token))`).

**Spec:** `docs/vi/specs/2026-07-27-chart-studio-design.md`. Plan này hiện thực phần "Hành vi Run" + "Trạng thái preview" + toolbar của spec. Phần tách viz-registry / config-model là plan riêng (A2).

**Điểm xuất phát trước khi bắt đầu:** `cd frontend && npx tsc --noEmit` sạch; `npx vitest run src/components/chart src/routes/explore/__tests__` xanh (599). Làm trên một branch từ `main` (vd `feat/chart-studio-a1`).

---

### Task 1: Helper field bắt buộc

Một hàm thuần trả về nhãn các field mà một viz còn thiếu, theo loại viz + config liên quan. Dùng lại cho validate-khi-run (Task 2) và empty state (Task 5).

**Files:**
- Tạo: `frontend/src/components/chart/requiredFields.ts`
- Test: `frontend/src/components/chart/__tests__/requiredFields.test.ts`

- [ ] **Step 1: Viết test thất bại** — (dùng đúng test trong bản EN, mục Task 1 Step 1)
- [ ] **Step 2: Chạy test để thấy FAIL** — `cd frontend && npx vitest run src/components/chart/__tests__/requiredFields.test.ts` → FAIL (`missingFields` chưa định nghĩa).
- [ ] **Step 3: Viết implementation** — code y hệt bản EN Task 1 Step 3 (`export function missingFields(viz, i)`), phản chiếu các check hiện nằm trong `ChartExplore.run()` (dòng ~319-353) nhưng trả về danh sách nhãn thay vì toast.
- [ ] **Step 4: Chạy test PASS** — 4 test xanh.
- [ ] **Step 5: Commit** — `feat(charts): missingFields helper for run-validation + empty state`

---

### Task 2: Dùng `missingFields` để validate khi Run

Thay thang `if (…) { toast; return }` theo từng viz trong `run()` bằng một lời gọi `missingFields`, để validation nằm một chỗ.

**Files:** Sửa `frontend/src/routes/explore/ChartExplore.tsx` (thân `run()`, ~316-363).

- [ ] **Step 1: Thêm import** — `import { missingFields } from "@/components/chart/requiredFields";`
- [ ] **Step 2: Dựng Input từ state** — thêm helper `validationInput()` ngay trên `run()` (code như bản EN Task 2 Step 2).
- [ ] **Step 3: Thay thang validation trong `run()`** — thay các block `if (viz === …) { toast; return }` (dòng ~319-353) bằng:
  ```ts
      const missing = missingFields(viz, validationInput());
      if (missing.length) { toast.error(`This chart needs ${missing.join(" and ")}`); return; }
  ```
  Giữ nguyên guard `if (!datasetId)` ở trên và `setRanSpec(specForRun()); setRunNonce((n) => n + 1);` ở dưới.
- [ ] **Step 4: Verify** — `npx tsc --noEmit && npx vitest run src/routes/explore/__tests__` → sạch + PASS.
- [ ] **Step 5: Commit** — `refactor(charts): run() uses missingFields for validation`

---

### Task 3: Chữ ký truy vấn + hook auto-run

Một hook thuần quyết định khi nào auto-run: theo dõi "chữ ký truy vấn" đã serialize, và bắn callback (đã debounce) khi `enabled` và chữ ký khác rỗng & thay đổi. Tách riêng để test được phần timing.

**Files:**
- Tạo: `frontend/src/components/chart/useAutoRun.ts`
- Test: `frontend/src/components/chart/__tests__/useAutoRun.test.ts`

- [ ] **Step 1: Viết test thất bại** — dùng test bản EN Task 3 Step 1 (fake timers, 3 case: enabled+đổi chữ ký → bắn; disabled → không; chữ ký rỗng → không).
- [ ] **Step 2: FAIL** — `useAutoRun` chưa định nghĩa.
- [ ] **Step 3: Viết hook** — code y hệt bản EN Task 3 Step 3 (`export function useAutoRun(...)`).
- [ ] **Step 4: PASS** — 3 test xanh.
- [ ] **Step 5: Commit** — `feat(charts): useAutoRun debounced auto-run hook`

---

### Task 4: Nối auto-run + cờ Auto vào ChartExplore

Thêm state `autoRun` (mặc định bật) và điều khiển `useAutoRun` từ chữ ký truy vấn. Thay đổi liên-quan-query thì auto-run; thay đổi chỉ-hiển-thị vẫn re-render từ rows cache (không query) vì không đổi chữ ký.

**Files:** Sửa `frontend/src/routes/explore/ChartExplore.tsx`.

- [ ] **Step 1: Thêm import + state** — `import { useAutoRun } from "@/components/chart/useAutoRun";` và `const [autoRun, setAutoRun] = useState(true);` (cạnh `ranSpec`, ~dòng 280).
- [ ] **Step 2: Tính chữ ký + nối hook** — thêm ngay sau `run()` (code như bản EN Task 4 Step 2): chữ ký = spec đã serialize, chỉ khi dataset đã chọn và không thiếu field (ngược lại rỗng → không auto-bắn query lỗi). Xoá effect filters tạm ở ~dòng 369 (filters giờ đổi chữ ký nên auto-run lo khi Auto bật; khi Auto tắt thì chờ Run — chấp nhận được).
- [ ] **Step 3: Verify** — `npx tsc --noEmit && npx vitest run src/routes/explore/__tests__` → sạch + PASS.
- [ ] **Step 4: Commit** — `feat(charts): hybrid auto-run driven by the query signature`

---

### Task 5: Component ChartPreviewStates

Trạng thái empty / loading / error dùng chung cho preview.

**Files:**
- Tạo: `frontend/src/components/chart/ChartPreviewStates.tsx`
- Test: `frontend/src/components/chart/__tests__/ChartPreviewStates.test.tsx`

- [ ] **Step 1: Viết test thất bại** — dùng test bản EN Task 5 Step 1 (empty hiện title+hint; error hiện message + gọi onRetry).
- [ ] **Step 2: FAIL** — components chưa định nghĩa.
- [ ] **Step 3: Viết components** — code y hệt bản EN Task 5 Step 3 (`ChartEmptyState`, `ChartLoadingState` skeleton dạng cột với `role="status"`, `ChartErrorState` có Retry).
- [ ] **Step 4: PASS** — 2 test xanh.
- [ ] **Step 5: Commit** — `feat(charts): reusable empty/loading/error preview states`

---

### Task 6: Nối các state vào vùng preview

Thay ba trạng thái chữ-thô/khung trong `ChartExplore.tsx` bằng component mới, và cho empty state nêu đúng field còn thiếu.

**Files:** Sửa `frontend/src/routes/explore/ChartExplore.tsx` (~980-1045).

- [ ] **Step 1: Thêm import** — `import { ChartEmptyState, ChartLoadingState, ChartErrorState } from "@/components/chart/ChartPreviewStates";`
- [ ] **Step 2: Thay error + loading** — theo bản EN Task 6 Step 2 (`<ChartErrorState message={data.error} onRetry={run} />` và `<ChartLoadingState />`).
- [ ] **Step 3: Thay empty state** — theo bản EN Task 6 Step 3 (IIFE tính `missing` rồi render `<ChartEmptyState icon={vizMeta?.icon} title=… hint=…/>`; nếu Auto tắt thì thêm ", then Run").
- [ ] **Step 4: Thay chiều cao preview cố định** — `h-[70vh]` → `min-h-[60vh] rounded-lg` (~dòng 1029).
- [ ] **Step 5: Verify** — `npx tsc --noEmit && npx vitest run src/routes/explore/__tests__` → sạch + PASS. (Nếu có test cũ assert chữ "Configure the controls…" thì cập nhật sang copy mới "Set up your …".)
- [ ] **Step 6: Commit** — `feat(charts): meaningful empty/loading/error states in the builder`

---

### Task 7: StudioToolbar — tên inline + toggle Auto + overflow

Tách toolbar ra component riêng: tên chart inline, toggle Auto, Run, Save, và menu `⋯` (Export / View SQL). Giữ hành vi hiện tại, gọn hơn.

**Files:**
- Tạo: `frontend/src/components/chart/StudioToolbar.tsx`
- Sửa: `frontend/src/routes/explore/ChartExplore.tsx` (vùng toolbar ~946-975).

- [ ] **Step 1: Viết component** — code y hệt bản EN Task 7 Step 1 (`export function StudioToolbar(...)` với input tên/onName, autoRun/onAutoRun, onRun/onSave, running/saveLabel, exportMenu/sqlDisclosure).
- [ ] **Step 2: Dùng trong ChartExplore** — thêm import `StudioToolbar`; thay block toolbar (~946-975) bằng `<StudioToolbar .../>` truyền props như bản EN Task 7 Step 2 (giữ h1 sr-only cho landmark; `saveName`/`setSaveName` đã có sẵn).
- [ ] **Step 3: Verify** — `npx tsc --noEmit && npx vitest run src/routes/explore/__tests__` → sạch + PASS.
- [ ] **Step 4: Commit** — `feat(charts): StudioToolbar — inline title + Auto toggle + overflow`

---

### Task 8: Verify toàn bộ + deploy + kiểm tra live

**Files:** không (chỉ verify).

- [ ] **Step 1: Cổng FE đầy đủ** — `cd frontend && npx tsc --noEmit && npx eslint src/components/chart src/routes/explore/ChartExplore.tsx && npx vitest run src/components/chart src/routes/explore/__tests__ && npm run build` → tất cả xanh.
- [ ] **Step 2: Deploy FE lên kind** — theo khối lệnh bản EN Task 8 Step 2 (docker build → kind load → rollout restart → rollout status).
- [ ] **Step 3: Kiểm tra live** (cần dataset có dimension + metric) — tại `.../charts/new?type=bar`: chọn dataset, đặt X-axis + metric với **Auto bật** → chart tự chạy (không bấm Run). Tắt **Auto**, đổi metric → chart chờ; bấm **Run**. Empty state (trước khi đủ field) hiện "Set up your Bar — Add: X-axis, a metric". Mở chart đã lưu → render khi mở.
- [ ] **Step 4: Mở PR (không merge; chờ duyệt)** — theo bản EN Task 8 Step 4.

---

## Ghi chú cho người triển khai

- **KHÔNG** đụng `ChartRenderer`, `echarts/options/*`, hay `useSpecData` — render giữ nguyên ở A1.
- Auto-run chỉ được bắn khi thay đổi **liên quan truy vấn**: chữ ký dựng từ `specForRun()`, nên các setting chỉ-hiển-thị (sort, màu, nhãn…) không nằm trong spec → không bao giờ kích query, đúng như thiết kế.
- Giữ dataset demo + chart mẫu để kiểm tra live; đừng xoá.
- Nếu chart đã lưu không render khi mở, đó là gap có sẵn cần sửa ở đây: hydrate `ranSpec` từ `params.spec` trong effect load (`setRanSpec(params.spec)` khi hydrate). Thêm như một step nhỏ ở Task 4 nếu kiểm tra live Task 8 lộ ra.
