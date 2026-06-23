# Style dashboard bám Metabase — thiết kế

> Bản tiếng Việt tham chiếu. Bản chuẩn (canonical) là tiếng Anh:
> `docs/en/specs/2026-06-23-metabase-dashboard-style-design.md`.

**Trạng thái:** đã duyệt (brainstorm) · **Ngày:** 2026-06-23 · **Branch:** `feat/metabase-dashboard-style` (off main)

## Mục tiêu
Làm bề mặt **dashboard** egret giống Metabase — "card trắng nổi trên nền xám nhạt, định hình bằng viền (không bóng)" + header/tab/filter/edit chrome, typography, spacing khớp. **Giữ brand teal egret** (`--primary`); chỉ copy cấu trúc/neutral. Chỉnh token OKLCH của egret (1 nguồn, light+dark), KHÔNG thêm hệ `--mb-color-*`. Phạm vi: chỉ dashboard. Một spec toàn diện.

## Tham chiếu Metabase (đã verify repo + live)
- Nền canvas `hsla(240,4%,95%)` (xám lạnh nhạt); card trắng nổi trên đó.
- Card: trắng, **viền 1px** `hsla(195,6%,87%)`, radius **6px**, **không bóng lúc nghỉ** (chỉ bóng khi kéo); title ~14px.
- Header: nền trắng, viền đáy 1px; title 20px/700/24lh; padding 16/12; icon gap 8.
- Tab: viền đáy 2px + indicator active.
- Filter bar: padding 8/0, sticky bg xám + viền đáy; pill radius 6.
- Edit bar: brand@85%, chữ trắng đậm. Toolbar card: pill trắng, radius 8, shadow nhẹ, hover hiện.
- Typography: base 14px; scale xs11/sm12/md14/lg17/xl21. Spacing 4/8/16/24/32; radius 4/6/8; gap 6.

## Hiện trạng egret (đã verify — `index.css` + components)
- Token: `--background/--card: oklch(1 0 0)` (trắng), `--primary: oklch(0.59 0.15 185)` (teal — GIỮ), `--secondary: oklch(0.95…)` (xám lạnh ≈ canvas Metabase), `--muted: oklch(…/3%)` (gần trong suốt → `bg-muted/30` quá nhạt), `--border: oklch(0.92…)` (≈ Metabase), `--radius: 0.625rem` (**10px** → card tròn hơn 6px). `.dark`: card 0.21, background 0.14, secondary/muted 0.27.
- `@theme` (dòng ~8) đăng ký token Tailwind.
- Wrapper dashboard (`DashboardView.tsx:277`): `bg-muted/30 p-4` (nền quá nhạt).
- Card (`DashboardView.tsx:426`): `rounded-lg border bg-card shadow-sm` (+ `ring-2 ring-primary` khi mở settings). **Có bóng; 10px.**
- Edit bar (`:279`): `bg-primary` (teal — GIỮ). Header title (`:291`): `text-lg font-semibold`. Card title (`DashboardCardView:36`): `px-3 pt-2 pb-1 text-sm font-medium`. Toolbar card: đã là pill trắng `rounded-md border bg-card shadow-sm`. Tab: container `border-b`, active `border-b-2 border-primary` (teal — GIỮ).

## Thiết kế — token (light + dark)
Thêm token **canvas dashboard** riêng (không lạm dụng `--secondary`), đăng ký `@theme`:
- `:root`: `--dashboard: oklch(0.955 0.004 270)`. `.dark`: `--dashboard: oklch(0.16 0 0)` (tối hơn `--card` 0.21 để card nổi). `@theme`: `--color-dashboard: var(--dashboard)`.
- `--border`/`--card` giữ nguyên (đã khớp). Card radius: giữ `--radius` toàn cục; card đặt **6px** qua `rounded-md` (nếu `@theme` remap thì `rounded-[6px]` — kiểm lúc build). Token teal giữ nguyên.

## Thiết kế — component (chỉ dashboard)
1. **Canvas** (`:277`): `bg-muted/30` → `bg-dashboard` (cả bản thường + isFullscreen).
2. **Card** (`:426`): `rounded-lg border bg-card shadow-sm` → `rounded-md border bg-card` (bỏ shadow, 10→6px). Giữ `overflow-hidden`, nhánh heading no-border, `ring-2 ring-primary`.
3. **Header title** (`:291`): `text-lg font-semibold` → `text-xl font-bold`; thêm viền đáy + padding (`border-b pb-3`).
4. **Card title** (`DashboardCardView:36`): → `px-4 pt-3 pb-2 text-sm font-medium text-foreground`.
5. **Tab bar**: giữ indicator teal; baseline 2px (`border-b-2 border-border`, active `-mb-[2px] border-b-2 border-primary`).
6. **Filter bar** (`DashboardFilterBar`/`EditFilterChips`): pill `h-8 rounded-md border bg-background px-2 text-sm` + icon tag + chevron; active dùng `bg-secondary`. Chỉ restyle.
7. **Edit bar** (`:279`): giữ teal; `bg-primary/90` (khớp 85%, tuỳ chọn).
8. **Toolbar card** (`CardEditToolbar`): pill trắng sẵn — radius `rounded-lg` (8px).
9. **Heading card** (`HeadingCard`): chữ `text-xl font-bold text-foreground`, card trong suốt không viền.

## Data flow / xử lý lỗi
Không đổi data/behavior — chỉ restyle. Token qua `@theme`; component đổi class. Dark mode: định nghĩa `--dashboard` ở cả `:root` + `.dark`; card (0.21) phải sáng hơn canvas (0.16). Heading card giữ no-border (đổi card-chrome KHÔNG được thêm viền/bg cho heading). Nếu `rounded-md` ≠ 6px thì `rounded-[6px]`. Giữ contrast AA.

## Kiểm thử
- **Vitest**: styling không có logic; giữ test dashboard cũ xanh (đổi class không phá role/text query). Tuỳ chọn assert card không còn `shadow-sm` + canvas `bg-dashboard` nếu rẻ.
- **Live (Playwright, 0 lỗi console, dashboard test, KHÔNG đụng a1)** — mở egret + Metabase (3001, alice@test.com) cạnh nhau: canvas xám rõ, card trắng viền 1px **không bóng**, radius ≈6px; header 20px bold + viền đáy; tab indicator teal trên baseline 2px; filter pill khớp; edit mode (bar teal, toolbar pill trắng, handle SE hover, lưới); **dark mode** canvas tối hơn card, teal còn nguyên; PARITY (read-only) mạch lạc.
- **Full sweep** trước mỗi commit.

## Quy ước
Metabase fidelity (cấu trúc/neutral; giữ teal egret) · clean-room · chỉnh token OKLCH egret (1 nguồn, light+dark) · full sweep · identity git + footer · docs song ngữ · squash-merge · branch `feat/metabase-dashboard-style`.

## Thứ tự task
1. Token `--dashboard` (+ `@theme`). 2. Canvas + card chrome. 3. Header + card title + heading typography. 4. Tab baseline + filter pill. 5. Edit chrome polish. 6. Live verify (egret vs Metabase, light+dark) + PR.
