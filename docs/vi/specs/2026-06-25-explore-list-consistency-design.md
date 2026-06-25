# Đồng bộ & polish list Explore (Thiết kế)

> Bản chuẩn (canonical): `docs/en/specs/2026-06-25-explore-list-consistency-design.md`. Bản này
> là tham chiếu tiếng Việt.

**Trạng thái:** Đã duyệt · **Ngày:** 2026-06-25 · **Phạm vi:** Chỉ frontend. Không đụng backend.
**Thứ tự:** Đây là **Project A**. **Project B** (folder/group per-type, phẳng 1 cấp cho
dataset/chart/dashboard — cần backend) làm sau, spec riêng.

## Vấn đề
3 list Explore không đồng đều: **Datasets** giàu (search/filter/sort/kebab/confirm/bulk) nhưng
timestamp thô + "Loading…"; **Charts** trơ (bảng thường, nút "Delete" đỏ không xác nhận, không
search/filter/sort, không icon loại); **Dashboards** lưng chừng (có bookmark/trash/duplicate
nhưng không search/sort, action là nút chữ, timestamp thô). Cả app **không có loading skeleton**;
không dùng `lib/relativeTime.ts` đã có.

## Mục tiêu
Đưa Charts/Dashboards lên chuẩn Datasets, và nâng cả 3 bằng các khối dùng chung: relative time,
loading skeleton, icon theo loại chart, kebab actions, xác nhận khi xóa cứng.

## Không làm
Folder (Project B); không thêm backend (Charts không có endpoint duplicate → không có Duplicate);
không thêm Bulk select cho Charts/Dashboards (Datasets giữ nguyên); không làm responsive; không
dọn seed rác trong code (xóa qua UI lúc verify nếu muốn).

## Thiết kế
**Khối dùng chung mới:** `ui/skeleton.tsx` (primitive đầu tiên); `explore/RelativeTime.tsx`
(`<span title={fullTimestamp}>{timeAgo}</span>`); `explore/SortableHeader.tsx` (th sort generic,
mũi tên ↑↓); `explore/RowActionsMenu.tsx` (vỏ kebab `MoreHorizontal` + DropdownMenuContent);
`explore/ListSkeleton.tsx` (bảng skeleton thay "Loading…").

**Charts** (viết lại theo Datasets): search theo tên + filter theo **loại viz** (từ
`CHART_TYPES`) + sort Name/Type/Updated; **icon loại** cạnh tên; kebab `ChartActionsMenu`: Open ·
Delete (**AlertDialog xác nhận**, vì xóa cứng); skeleton + empty state.

**Dashboards** (viết lại, giữ trash/bookmark): thêm search + sort Name/Cards/Updated; kebab
`DashboardActionsMenu`: view thường = Open · Duplicate · Move to Trash (hồi phục được → không cần
confirm nặng); view Trash = Restore · Delete permanently (giữ `ConfirmDeleteDialog`); relative
time + skeleton.

**Datasets** (nâng nhẹ): đổi `toLocaleString` → `<RelativeTime>`; "Loading…" → `<ListSkeleton>`;
refactor nhẹ `Th`→`SortableHeader`, `DatasetActionsMenu` dùng `RowActionsMenu`. Giữ bulk select.

## Test
Vitest cho từng khối + mỗi list (search/sort/filter/confirm-delete/relative). Verify live cuối
(0 lỗi console mới).
