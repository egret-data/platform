# Tier 2a — Tùy chọn hiển thị dashboard: fixed/full width + dark-mode (thiết kế)

> Bản tiếng Việt tham chiếu. Bản chuẩn (canonical) là tiếng Anh:
> `docs/en/specs/2026-06-22-tier2a-dashboard-view-options-design.md`.

**Trạng thái:** đã duyệt (brainstorm) · **Ngày:** 2026-06-22 · **Branch:** `feat/dashboards-tier2a` (off main)
**Thuộc:** chương trình parity dashboard với Metabase ([audit backlog](./../../en/specs/2026-06-22-dashboard-metabase-parity-audit.md), Tier 2)

## Mục tiêu

Hai tùy chọn hiển thị nhỏ kiểu Metabase: setting **fixed/full width** lưu bền, và **dark-mode**
để dashboard hiển thị đúng dưới global dark theme sẵn có của egret. Clean-room (giống look/behaviour
Metabase, không copy code).

## Phạm vi

**Có:** (A) `dashboards.width` ('fixed'|'full') lưu bền + toggle ở edit mode + container căn giữa ở
view mode khi fixed; (B) verify + sửa phần chrome dashboard (canvas, card, filter pill, tab,
popover) dưới global dark theme.

**Ngoài / defer (nêu rõ):** **PDF/print export** (feature riêng — ECharts→PDF cần lib client +
xử lý `getDataURL()` từng viz); toggle night-mode *riêng cho dashboard* (global theme toggle của
egret đã đóng vai "night mode" của Metabase — không cần toggle riêng); dark theme nội bộ ECharts
(màu axis/label) — vấn đề chart rộng hơn, ghi nhận chứ không thuộc tier này trừ khi gap tìm thấy
là vá một dòng.

## A. Fixed/full width (lưu bền, đúng Metabase)

Metabase lưu `width: "fixed" | "full"` trên dashboard (setting edit-mode áp cho mọi viewer;
"fixed" căn giữa grid ở max width, "full" full-bleed). egret làm theo.

### Backend (`egret-bi`)
- **Migration `0018_dashboard_width.sql`:**
  ```sql
  ALTER TABLE dashboards ADD COLUMN width text NOT NULL DEFAULT 'full'
    CHECK (width IN ('fixed', 'full'));
  ```
  (sqlx migration-staleness: `touch crates/egret-metastore/src/lib.rs` trước khi build/test.)
- `dashboard.rs`: struct `Dashboard` + `COLS` thêm `width: String`. `update(...)` thêm tham số
  `width` và set nó.
- `model.rs`: `DashboardDetailDto` thêm `width: String`; `UpdateDashboard` thêm
  `#[serde(default = "default_width")] width: String` (default `"full"`); `dashboard_detail_dto`
  truyền qua. Reject width không thuộc {fixed, full} → 400 trong handler (DB CHECK là backstop).
- `routes.rs` `db_update` truyền `b.width` vào `update`.

### Frontend
- `api/dashboards.ts`: `DashboardDetail` thêm `width: "fixed" | "full"`; body mutation `update`
  thêm `width`.
- `DashboardView.tsx`:
  - **Edit mode:** toggle "Width" nhỏ ở header edit (hai nút hoặc select: Fixed / Full), giữ trong
    state `draftWidth` (seed từ `dashboard.width` khi `startEdit`), lưu trong PUT toàn-dashboard
    (`onSave` gửi `width: draftWidth`).
  - **View mode:** khi `width === "fixed"`, bọc nội dung dashboard (filter bar + tab + grid) trong
    container căn giữa `mx-auto w-full max-w-[1200px]`; khi `"full"` thì full-bleed như hiện tại.
    Canvas muted ngoài vẫn full-bleed; chỉ cột nội dung bị giới hạn (như Metabase).

## B. Dark-mode polish

egret đã có global light/dark theme (nút moon ở header). Component dashboard dùng token theme
(`bg-card`, `bg-muted`, `text-foreground`, `border`, `text-muted-foreground`) — grep không thấy màu
hardcode — nên chrome đáng lẽ tự thích nghi.

- **Verify live ở dark mode** (toggle theme trên dashboard PARITY seed): canvas, card, filter pill
  (gồm state chọn `bg-primary/5` + popover), tab, control edit-mode, drill menu/records dialog,
  placeholder empty/blocked.
- **Sửa gap nếu có** (màu hardcode, contrast kém, border vô hình) bằng token theme phù hợp. Giữ vá
  tối thiểu, theo token.
- Thân chart ECharts tự render màu; nếu khó đọc trên canvas tối, ghi nhận follow-up (dark theme
  chart) thay vì mở rộng tier này — trừ khi vá container một dòng là đủ.

## Luồng dữ liệu

```
edit: draftWidth (toggle Fixed/Full) → onSave PUT { …, width } → dashboards.width
view: dashboard.width === "fixed" → nội dung bọc mx-auto max-w-[1200px]
dark: global theme (html.dark) → chrome dashboard dùng token → tự thích nghi (verify/fix)
```

## Xử lý lỗi

- PUT `width` không thuộc {'fixed','full'} → 400 (check tường minh ở handler; DB CHECK là backstop).
- Dashboard cũ default `width = 'full'` (migration default) — không đổi behaviour.
- Dashboard không tab / rỗng không bị ảnh hưởng bởi wrapper width.

## Kiểm thử

- **Backend (test repo `egret-bi`):** `update` round-trip `width`; `dashboard_detail_dto`/`get`
  trả về; default `'full'` cho dashboard mới tạo.
- **Vitest:** toggle Width edit-mode ghi `draftWidth` và payload lưu có `width`; ở view mode
  `width: "fixed"` render wrapper căn giữa `max-w-[1200px]` (assert class / attr `data-width`),
  `"full"` thì không.
- **Live (Playwright MCP):** toggle Width Fixed↔Full ở edit, lưu, reload → bền + layout view đổi;
  toggle global theme → chrome dashboard đúng ở dark mode (sửa gap); so với Metabase
  `localhost:3001/dashboard/1`. **0 lỗi console.**
- **Full sweep** trước mỗi commit: frontend `npm run typecheck && lint && build && npx vitest run`;
  backend `cargo fmt --all --check && cargo test -p egret-bi` (fmt nằm trong CI — xem bài học cargo-fmt).

## Quy ước

Metabase là chuẩn · clean-room (không AGPL) · TDD · full sweep (gồm `cargo fmt`) trước mỗi commit ·
identity git đã cấu hình (không `--author`), footer `Co-Authored-By: Claude Opus 4.8 (1M context)
<noreply@anthropic.com>` · stage path tường minh · squash-merge · branch `feat/dashboards-tier2a`.
