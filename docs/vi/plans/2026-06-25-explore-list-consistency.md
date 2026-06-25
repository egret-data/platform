# Kế hoạch: Đồng bộ & polish list Explore (Project A)

> Bản chuẩn (canonical) & là bản agent thực thi:
> `docs/en/plans/2026-06-25-explore-list-consistency.md`. Bản này là tham chiếu tiếng Việt.

**Mục tiêu:** Đưa list Charts/Dashboards lên chuẩn Datasets; nâng cả 3 bằng khối dùng chung
(relative time, skeleton, icon loại chart, kebab actions, xác nhận khi xóa cứng). Chỉ frontend.

**Spec:** `docs/en/specs/2026-06-25-explore-list-consistency-design.md`

## Các task (code đầy đủ ở bản en)
1. `ui/skeleton.tsx` — primitive Skeleton + test.
2. `explore/RelativeTime.tsx` — `<span title={fullTimestamp}>{timeAgo}</span>` + test.
3. `explore/SortableHeader.tsx` — th sort generic (`SortState<K>`) + test.
4. `explore/RowActionsMenu.tsx` — vỏ kebab (mở được qua user-event) + test.
5. `explore/ListSkeleton.tsx` — bảng skeleton + test.
6. **Charts** viết lại + `ChartActionsMenu`: search + filter loại viz + icon loại + sort + kebab
   (Open/Delete có AlertDialog) + relative + skeleton + empty + test mới.
7. **Dashboards** viết lại + `DashboardActionsMenu`: search + sort + kebab (Open/Duplicate/Move
   to Trash; Trash: Restore/Delete permanently) + relative + skeleton; **cập nhật 3 test cũ**
   (mock đủ mutations + mở kebab bằng user-event).
8. **Datasets** nâng nhẹ: relative time + skeleton + dùng `SortableHeader`; `DatasetActionsMenu`
   chuyển sang `RowActionsMenu`.
9. Sweep + verify live (Playwright, 0 lỗi console mới) + finish.

Quy ước: lệnh chạy trong `egret/frontend/`; sweep
`npm run typecheck && npm run lint && npm run build && npx vitest run`; `git add` prefix
`frontend/`; branch `feat/explore-list-consistency`; cuối commit thêm
`Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
