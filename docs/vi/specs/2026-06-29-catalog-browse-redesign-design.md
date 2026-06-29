# Catalog — Thiết kế lại tab Browse (cây namespace scale được) — Thiết kế (tham chiếu VN)

> Bản chuẩn: `docs/en/specs/2026-06-29-catalog-browse-redesign-design.md`. Lệch thì theo bản tiếng Anh.

**Ngày:** 2026-06-29 · **Branch:** `feat/catalog-browse-redesign` (off main). Tham chiếu: browser warehouse của lakekeeper console (`WarehousesNavigationTree` + breadcrumb).

## Mục tiêu
Thay tab **Browse** hiện tại (list phẳng toàn bộ namespace 16rem + list table/view phẳng, không search) bằng **cây namespace + breadcrumb** vẫn dùng tốt khi warehouse có nhiều namespace/table.

## Hiện trạng
`CatalogBrowse.tsx` = grid 2 cột: trái = `useCatalogNamespaces` dạng **list phẳng dotted**, phải = list table/view của namespace (`NamespaceContentsView`) hoặc `TableDetailView`. Không search/cây/virtualize; lựa chọn là state component (không URL). Backend đã trả đủ: namespaces (BFS toàn cây, phẳng), tables/views theo từng namespace → việc còn lại ở **frontend**. Có sẵn: shadcn `Breadcrumb`, `useSearchParams` (Storage đã dùng), `DetailSection`/`Card`. **Chưa** có thư viện virtualization.

## Quyết định
1. **Cây namespace + breadcrumb (kiểu console).** Trái = `NamespaceTree`: gấp list phẳng thành **cây lồng nhau** (theo `levels`); mỗi node expand/collapse; lần đầu mở thì **lazy-load** tables/views qua `useNamespaceContents` (TanStack cache); lá table click được, view chỉ hiện (chưa có trang view-detail). Có ô **filter** trên đầu. Phải = `Breadcrumb` (`warehouse › namespace › table`) + `TableDetailView` (giữ nguyên) hoặc overview namespace. Giữ dialog **New namespace** (chọn parent) + **Delete namespace**.
2. **Đồng bộ URL.** Namespace + table chọn lưu ở `?ns=…&table=…` (`useSearchParams`, như Storage). Khi load: tự **mở ancestor** của `?ns` + mở detail `?table` → refresh / deep-link / nút Back đều chạy.
3. **Xử lý scale (không thêm dependency).** Collapse → DOM nhỏ. Filter lọc cây theo path (auto-expand match); namespace đã mở thì lọc luôn table/view. Namespace > **200** table+view: render phần đã lọc + dòng **"Showing X of Y — refine search"** (không cắt âm thầm). **Virtualization để sau** (cần dep mới) — ghi là future.
4. **Cấu trúc component.** `CatalogBrowse.tsx` (điều phối, URL-sync, dialog); `NamespaceTree.tsx` (flat→nested, filter, `TreeNode` đệ quy: expand, lazy contents, lá, highlight, hover delete); breadcrumb nhỏ; tái dùng `TableDetailView` + `DetailSection`/`Card`.

## Test (Vitest, mock `@/api/catalog`)
Gấp list phẳng → cây (`sale` cha, `sale.2024` con); expand lazy-load + hiện table/view (`useNamespaceContents` gọi đúng ns); filter thu hẹp cây + contents; chọn table → `TableDetailView` + breadcrumb + đặt `?ns`/`?table`; deep-link `?ns=sale&table=orders` khôi phục; namespace > 200 hiện dòng "Showing X of Y"; create + delete namespace (409 surfaced) vẫn chạy.

## Rủi ro
Lazy-load fan-out (TanStack cache, chỉ load khi mở → có giới hạn); filter table chỉ chạy sau khi mở namespace (search xuyên namespace = ngoài phạm vi, cần endpoint BE); namespace cực lớn → dòng "Showing X of Y" tránh vỡ DOM, virtualization là bước sau.

## Ngoài phạm vi
Global search table xuyên namespace (cần endpoint BE); trang VIEW detail; virtualization thật; paging/search server-side; generic-table.
