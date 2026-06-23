# Style dashboard Metabase — Kế hoạch triển khai

> Bản tiếng Việt tham chiếu. Bản chuẩn (đủ code từng task) là tiếng Anh:
> `docs/en/plans/2026-06-23-metabase-dashboard-style.md`. Spec: `docs/en/specs/2026-06-23-metabase-dashboard-style-design.md`.

**Mục tiêu:** Restyle bề mặt dashboard egret giống Metabase — nền xám, card trắng viền-phẳng (không bóng, bo 6px), header/tab/filter/typography — **giữ brand teal**. FRONTEND-ONLY, chỉ đổi class/token (không logic). Branch `feat/metabase-dashboard-style` (off main, spec đã commit).

**Thang radius (đã verify `index.css:187-190`, từ `--radius: 10px`):** `rounded-sm`=**6px**, `rounded-md`=**8px**, `rounded-lg`=**10px**. ⇒ Metabase 6px = egret **`rounded-sm`**; toolbar 8px = `rounded-md` (đã đúng, không đổi).

**Quy ước:** Đọc file trước khi sửa. Chỉ styling (không đổi logic/aria/test query). Full sweep trước mỗi commit (`npm run typecheck && lint && build && npx vitest run`). Stage path tường minh; identity git + footer. KHÔNG push tới Task 5.

## Task 1 — Token `--dashboard` (`src/index.css`)
- `:root` (~dòng 93, sau `--secondary`): `--dashboard: oklch(0.955 0.004 270);`
- `.dark` (~dòng 132, sau `--secondary`): `--dashboard: oklch(0.16 0 0);` (tối hơn card 0.21 để card nổi)
- `@theme inline` (~dòng 164, sau `--color-secondary`): `--color-dashboard: var(--dashboard);`
- Sweep + commit.

## Task 2 — Canvas + card + edit bar (`DashboardView.tsx`)
- `:277` wrapper: cả 2 `bg-muted/30` → `bg-dashboard`.
- `:426` card: `rounded-lg border bg-card shadow-sm` → `rounded-sm border bg-card` (bỏ shadow, 6px). Giữ `overflow-hidden`, nhánh heading no-border, `ring-2 ring-primary`.
- `:279` edit bar: `bg-primary` → `bg-primary/90`.
- Sweep + commit. (Nếu test assert class `shadow-sm`/`rounded-lg`/`bg-muted` thì cập nhật.)

## Task 3 — Header + card title + heading
- `DashboardView:290-291`: `<div className="mb-3 flex … border-b pb-3">` + `<h1 className="text-xl font-bold">` (text-xl=20px).
- `DashboardCardView:36`: `px-3 pt-2 pb-1 text-sm font-medium` → `px-4 pt-3 pb-2 text-sm font-medium text-foreground`.
- `HeadingCard.tsx`: `text-lg font-semibold` → `text-xl font-bold` (giữ phần còn lại + placeholder muted).
- Sweep + commit.

## Task 4 — Tab baseline + filter pill
- `DashboardTabBar.tsx`: container `border-b` → `border-b-2 border-border`; `cls` thêm `-mb-[2px]` (giữ active teal `border-primary text-primary font-medium`).
- `DashboardFilterBar.tsx` / `EditFilterChips.tsx` (đọc trước): pill `h-8 rounded-sm border bg-background px-2 text-sm` + icon + chevron; active `bg-secondary`. Chỉ đổi class, không đụng logic/aria. Báo cáo đã đổi gì; stage chỉ file thực sự đổi.
- Sweep + commit.

## Task 5 — Live verify + PR
- Final sweep xanh. `npm run dev`, login admin, **KHÔNG đụng a1**; tạo dashboard mới (Table+Bar+Scalar+Heading); mở Metabase (3001, alice@test.com/123123a@, x-ray "A look at Orders") cạnh để so. Xoá sau.
- Playwright, 0 lỗi: canvas xám rõ; card trắng viền 1px **không bóng** (`borderRadius≈6px`, `boxShadow:none`); header 20px bold + viền đáy; card title `px-4 pt-3`; heading 20px bold no-chrome; tab indicator teal trên baseline 2px; filter pill ~32px/6px/viền; edit mode (bar teal mềm, toolbar pill, handle SE, lưới); **dark mode** canvas tối hơn card + teal nguyên; PARITY (read-only, KHÔNG save) mạch lạc.
- push + PR `feat(dashboards): Metabase-faithful dashboard style (...)` + footer; squash-merge khi CI xanh (ranger flaky → rerun).

## Thứ tự
Task 1 → 2 → 3 → 4 → 5.
