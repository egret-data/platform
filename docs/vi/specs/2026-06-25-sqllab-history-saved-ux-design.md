# SQL Lab — UX History / Saved / Toolbar (Thiết kế)

> Bản tiếng Anh là bản chuẩn (canonical): `docs/en/specs/2026-06-25-sqllab-history-saved-ux-design.md`.
> Bản này là bản tham chiếu tiếng Việt.

**Trạng thái:** Đã duyệt
**Ngày:** 2026-06-25
**Phạm vi:** Chỉ frontend (React/TS). Không đổi backend / API / migration — endpoint `PUT`
sửa saved query đã có sẵn (`get(load).put(update).delete(remove)` trong
`crates/egret-bi/src/routes.rs`).

## Vấn đề

Panel dưới và toolbar của SQL Lab đang nghèo nàn so với dữ liệu API đã trả về:

1. **History tab** chỉ là list phẳng `[status] sql`. Bỏ qua `submitted_at`, `row_count`,
   `database_id` mà `HistoryItem` đã có. Status hiện dạng text `[finished]`/`[failed]`. Chỉ có
   hành động "click để load SQL"; không copy, không re-run, không empty state.
2. **Saved tab** chỉ hiện `name` + link chữ đỏ `delete`. Bỏ qua `description`, `updated_at`,
   `database_id`. Tệ hơn: `delete` **xóa ngay không xác nhận**. Không có rename/edit, không
   empty state.
3. **Limit selector** `w-28` quá hẹp — "1,000 rows" bị chevron cắt thành "1,000 row⋮". Không
   nhập được limit tùy ý ngoài các mốc preset.
4. **Toolbar / Results**: lỗi query (`status.data.error`) in thẳng trên toolbar → câu lỗi dài
   làm vỡ layout. Sau khi chạy không biết mất bao lâu.

## Mục tiêu

Làm History và Saved giống workspace truy vấn thật (rich rows kiểu Superset), sửa limit bị cắt
và cho nhập custom, đưa lỗi query ra khỏi toolbar vào vùng kết quả kèm dòng trạng thái sau khi
chạy — tất cả chỉ frontend.

## Không làm (Non-Goals)

- **Không đổi backend.** `update` đã nhận `{ name, sql, description, database_id }`.
- Không **search / filter / phân trang** history (giữ list đơn giản, có giới hạn hiển thị).
- Không **tag / favorite / chọn nhiều** cho saved.
- Không đo thời gian ở backend. Elapsed **đo phía client** (status API không trả timestamp),
  hiển thị xấp xỉ.
- Delete saved dùng **confirm nhẹ** (Cancel / Delete) — KHÔNG dùng `ConfirmDeleteDialog` kiểu
  gõ-chữ-xác-nhận (quá nặng cho một saved SQL ít rủi ro).

## Thiết kế (tóm tắt)

**File mới:** `lib/relativeTime.ts` (`timeAgo`, `fullTimestamp` từ dayjs `relativeTime`);
`sqllab/QueryStatusBadge.tsx` (badge màu theo status); `sqllab/HistoryTab.tsx`,
`sqllab/SavedTab.tsx` (tách khỏi `SqlTab` + làm giàu).

**File sửa:** `api/sqllab.ts` (+`useUpdateSavedQuery` PUT); `SaveQueryDialog.tsx` (thêm chế độ
edit: `initialName`/`initialDescription`/`mode`); `SqlTab.tsx` (nới + mở rộng limit, đo thời
gian client, đưa lỗi ra Results, tự chuyển sang tab Results, truyền `databases`/`onLoad`/`onRun`
cho HistoryTab); `ResultsGrid.tsx` (nhận `error` + dòng trạng thái elapsed/rows ở footer).

**History (rich rows):** dòng 1 = badge · tên DB · `timeAgo(submitted_at)` (tooltip giờ đầy đủ)
· `N rows`; dòng 2 = SQL monospace cắt 1 dòng (title = full). Click hàng = load. Hover:
**Copy** (clipboard + toast), **Re-run** (set DB + SQL rồi chạy ngay). Empty: "No queries run yet."

**Saved (rich rows):** dòng 1 = name (đậm) · tên DB · "updated " + `timeAgo(updated_at)`; dòng
2 = description (mờ) hoặc SQL preview. Click hàng = open. Hover: **Edit** (mở SaveQueryDialog
edit-mode → PUT), **Delete** (AlertDialog Cancel/Delete). Empty: "No saved queries."

**Limit:** nới trigger `w-36` + nhãn "Limit"; thêm mục **"Custom…"** → hiện ô số inline
(Enter/blur commit, Esc hủy, clamp số nguyên dương ≤ 100000); `tab.limit` lưu & hiện lại được
giá trị custom.

**Toolbar/Results:** bỏ span lỗi trên toolbar; khi `failed` hiện block đỏ trong Results; tự
chuyển sang tab Results khi chạy; đo elapsed phía client (mốc khi set `queryId`, tính khi
finished/failed) → footer "Done in 1.2s · N rows".

## Test

Vitest: `relativeTime`; `QueryStatusBadge`; `HistoryTab` (meta, click load, re-run, copy,
empty); `SavedTab` (meta, open, edit gọi update đúng body, delete có confirm, empty); limit
custom; `SqlTab` (failed → lỗi ở Results không ở toolbar; submit thành công → chọn tab Results).
Cuối cùng verify live bằng Playwright (0 lỗi console mới).
