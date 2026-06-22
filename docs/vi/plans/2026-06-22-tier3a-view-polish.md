# Tier 3a — Kế hoạch triển khai Hoàn thiện view dashboard

> Bản tiếng Việt tham chiếu. Bản chuẩn (canonical) — đủ code từng task — là tiếng Anh:
> `docs/en/plans/2026-06-22-tier3a-view-polish.md`. Spec: `docs/en/specs/2026-06-22-tier3a-view-polish-design.md`.

**Mục tiêu:** Thêm view polish kiểu Metabase: duplicate, trash/soft-delete, bookmark, info sidebar, audit dark-mode.
**Kiến trúc:** Backend Rust `egret-bi` + frontend React. 2 migration mới (`0019` cột soft-delete, `0020` bảng bookmarks).
Branch `feat/dashboards-tier3a` (off main, spec đã commit).

**Quy ước mọi task:** TDD (đỏ→xanh). BE sweep trước commit: `cargo test -p egret-bi && cargo fmt --all --check`
(DB-gated, set `DATABASE_URL=postgres://egret:egret@localhost:5432/egret`). FE sweep (từ `frontend/`):
`npm run typecheck && lint && build && npx vitest run`. **sqlx staleness:** sau khi thêm file `migrations/`,
`touch crates/egret-metastore/src/lib.rs`. Stage path tường minh; identity git + footer
`Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

## Các task (code đầy đủ ở bản tiếng Anh)

1. **Duplicate — BE.** `DashboardRepo::duplicate(src,new_owner)` deep-copy row+tabs+cards (id mới, name "(copy)",
   remap tab_id, 1 transaction) + route `POST /dashboards/{id}/duplicate` (`require_dashboard_in_ws`). Repo test.
2. **Duplicate — FE.** `useDashboardMutations.duplicate` + nút "Duplicate" ở list → điều hướng sang copy. Vitest.
3. **Migration 0019 + Trash — BE.** `ALTER TABLE dashboards ADD deleted_at` + index. `Dashboard.deleted_at`+`COLS`;
   `list_by_workspace(ws,trashed)` lọc `deleted_at IS [NOT] NULL`; `set_trashed`; `db_list ?trashed`; `db_trash`/
   `db_restore` (`require_dashboard_owner`); `deleted_at` vào detail DTO. Repo test.
4. **Trash — FE.** types `deleted_at`; `useDashboards(ws,{trashed})`; `trash`/`restore`; nút "Delete"→"Move to trash";
   section **Trash** có Restore + Delete permanently (confirm); banner "In Trash — Restore" ở detail. Vitest.
5. **Migration 0020 + Bookmark — BE.** bảng `bookmarks(user_id,entity_type,entity_id,created_at, PK)` generic.
   `add_bookmark`/`remove_bookmark` (ON CONFLICT DO NOTHING/DELETE), `is_bookmarked`, `list_by_workspace_for(ws,user,
   trashed)` trả cờ `bookmarked` (EXISTS); `delete` dọn bookmarks; DTO summary+detail thêm `bookmarked`;
   `load_dashboard(st,id,user)`; routes `POST/DELETE /dashboards/{id}/bookmark`. Repo test.
6. **Bookmark — FE.** types `bookmarked`; `addBookmark`/`removeBookmark`; nút sao ở list rows + header; filter
   **Bookmarks** (client-side theo cờ). Vitest.
7. **Info sidebar — FE.** `DashboardInfoSidebar` (Owner, Last edited, Description sửa inline lưu qua PUT sẵn có) +
   nút ⓘ ở header `DashboardView`. Vitest. (Owner label: dùng `created_by` nếu chưa có hook email.)
8. **Audit dark-mode — FE.** grep màu hardcode trong UI dashboard, sửa về token nếu có (kỳ vọng ~0). Không test.
9. **Verify live + PR.** full sweep; chạy stack (migrate 0019/0020) + login `admin@egret.local`; Playwright song song
   Metabase: duplicate / trash→restore→delete permanently / bookmark + filter / info sidebar (sửa description reload) /
   dark-mode toàn UI mới; **0 lỗi console**; **không đụng a1**, xóa dashboard test; push + PR + squash-merge khi CI xanh.

## Thứ tự
(A) Task 1–2 duplicate · (B) 3–4 trash · (C) 5–6 bookmark · (D) 7 info · (E) 8 dark-mode · (F) 9 live+PR.
Migration + backend luôn trước frontend tiêu thụ.
