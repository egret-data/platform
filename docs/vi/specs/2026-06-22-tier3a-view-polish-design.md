# Tier 3a — Hoàn thiện view dashboard (thiết kế)

> Bản tiếng Việt tham chiếu. Bản chuẩn (canonical) là tiếng Anh:
> `docs/en/specs/2026-06-22-tier3a-view-polish-design.md`.

**Trạng thái:** đã duyệt (brainstorm) · **Ngày:** 2026-06-22 · **Branch:** `feat/dashboards-tier3a` (off main)

## Mục tiêu

Đóng các gap "view polish" so với Metabase: **info sidebar**, **duplicate dashboard**, **trash/soft-delete**
(thay hard delete), **bookmark**. Cộng audit dark-mode (global theme đã phủ dashboard; chỉ verify + sửa màu lệch).

Trash và Bookmark vốn là tính năng platform-wide, nhưng tier này chỉ wire **cho dashboard**, với **schema generic**
để sau mở cho chart/dataset không phải migrate lại.

## Phạm vi

**Có:** (1) audit dark-mode (không thêm toggle); (2) info sidebar (chỉ FE); (3) duplicate (1 endpoint + UI);
(4) trash/soft-delete + restore + xóa vĩnh viễn (migration + UI); (5) bookmark (bảng generic + endpoint dashboard + UI).

**Defer:** night toggle riêng cho dashboard (global theme là đủ — YAGNI); trash/bookmark cho chart/dataset (schema
generic, wiring để sau); bookmark folder/sắp xếp; trash auto-purge; lịch sử chỉnh sửa dashboard.

## Data model (2 migration)

```sql
-- 0019_dashboard_soft_delete.sql
ALTER TABLE dashboards ADD COLUMN deleted_at timestamptz NULL;
CREATE INDEX dashboards_deleted_idx ON dashboards (workspace_id, deleted_at);

-- 0020_bookmarks.sql (generic, hiện chỉ wire dashboard)
CREATE TABLE bookmarks (
  user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  entity_type text NOT NULL,         -- 'dashboard' ở tier này
  entity_id uuid NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (user_id, entity_type, entity_id)
);
CREATE INDEX bookmarks_user_idx ON bookmarks (user_id, entity_type);
```

`bookmarks` polymorphic theo `entity_type`, không FK tới bảng entity; xóa vĩnh viễn dashboard sẽ dọn luôn bookmark
tương ứng. "Live" = `deleted_at IS NULL` ở mọi nơi.

## 1. Audit dark-mode (FE, ~0)
Dashboard đã render đúng ở dark theme global (đã verify). Một task: rà view + edit + popover filter + UI mới (info
sidebar, trash) ở dark, sửa màu hardcode nếu có. Không thêm toggle riêng.

## 2. Info sidebar (chỉ FE)
Nút **ⓘ** ở header `DashboardView` → mở slide-over phải, hiện từ `DashboardDetail` đã load: **Owner** (`created_by`→email),
**Last edited** (`updated_at`), **Description** sửa inline (lưu qua `PUT /dashboards/{id}` sẵn có, giữ nguyên layout).
Owner/admin mới sửa được. Không endpoint mới. (Không có cột `created_at` → không có dòng "Created".)

## 3. Duplicate dashboard (BE + FE)
`POST /ws/{w}/dashboards/{id}/duplicate` → `db_duplicate` (cần read access). `DashboardRepo::duplicate` chèn dashboard mới
(id mới, `name = "<tên> (copy)"`, cùng description/parameters/width, owner = user hiện tại), copy tabs (id mới) + cards
(id mới, remap `tab_id`), 1 transaction, tái dùng SQL của `replace_layout`. FE: action "Duplicate" ở row list + overflow
header → điều hướng sang bản copy.

## 4. Trash / soft-delete (migration + FE)
- `db_list` thêm `?trashed=<bool>` (mặc định false): false→`deleted_at IS NULL`, true→`IS NOT NULL`.
- `POST .../trash` → set `deleted_at=now()`; `POST .../restore` → set NULL (owner/admin, `require_dashboard_owner`).
- `DELETE /dashboards/{id}` giữ nguyên = **xóa vĩnh viễn** (cascade cards/tabs + dọn bookmarks). Trash view dùng nút này.
- `GET /dashboards/{id}` giữ nguyên (deep link tới dashboard trashed vẫn mở); thêm `deleted_at` vào detail DTO để hiện
  banner "In Trash — Restore".
- FE (`Dashboards.tsx`): nút "Delete" → **"Move to trash"**; thêm **section/toggle Trash** (`?trashed=true`) có Restore +
  Delete permanently (confirm).

## 5. Bookmark (bảng + FE)
- Method: `add_bookmark(user,'dashboard',id)` (INSERT … ON CONFLICT DO NOTHING), `remove_bookmark(...)`.
- `db_list` kèm cờ `bookmarked` (EXISTS theo user); thêm `bookmarked` vào DTO summary + detail.
- `POST/DELETE /ws/{w}/dashboards/{id}/bookmark` (mọi member tự bookmark).
- FE: **nút sao** ở header + mỗi row; **section Bookmarks** lọc theo cờ `bookmarked` (client-side ở tier này).

## Xử lý lỗi
- Duplicate dashboard không tồn tại/khác ws → 404. Trash/restore không phải owner/admin → 403.
- Bookmark add idempotent; remove không tồn tại → no-op. Xóa vĩnh viễn → dọn bookmarks (không star mồ côi).
- Restore mà tên trùng dashboard live → unique `(workspace_id, name)` báo **409** ("đổi tên trước"), không tự đổi tên.

## Kiểm thử
- **Backend** (DB-gated): duplicate copy tabs+cards remap id + owner mới; filter `trashed`; restore; xóa vĩnh viễn dọn
  bookmark; bookmark idempotent + cờ trong list; restore trùng tên → conflict.
- **Frontend** (Vitest): info sidebar + sửa description gọi update; duplicate gọi hook + điều hướng; move-to-trash + Trash
  restore/permanent-delete; star toggle add/remove + phản ánh `bookmarked`.
- **Live** (Playwright, song song Metabase): info sidebar; duplicate; move-to-trash→biến mất→Trash→restore; star +
  Bookmarks filter; rà dark-mode toàn UI mới. **0 lỗi console.**
- **Migration staleness:** sau khi thêm 2 migration, `touch crates/egret-metastore/src/lib.rs` trước `cargo build`/`serve`.
- **Full sweep** trước mỗi commit — BE: `cargo test -p egret-bi` + `cargo fmt --all`; FE (từ `frontend/`):
  `npm run typecheck && npm run lint && npm run build && npx vitest run`.

## Quy ước
Metabase là chuẩn · clean-room · TDD · full sweep trước mỗi commit · identity git đã cấu hình +
`Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` · stage path tường minh · docs song ngữ
(en canonical + vi reference) · squash-merge · branch `feat/dashboards-tier3a`.
