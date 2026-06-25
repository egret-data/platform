# Explore Parity + Folders (Thiết kế — tham chiếu VN)

> Bản chuẩn (canonical) là tiếng Anh: `docs/en/specs/2026-06-25-explore-parity-folders-design.md`.
> File này là bản tham chiếu rút gọn; khi lệch, lấy bản tiếng Anh làm chuẩn.

**Trạng thái:** Đã duyệt · **Ngày:** 2026-06-25
**Phạm vi:** Frontend (React/TS) **và** backend (Rust + migration Postgres) — phần đụng backend mà Project A đã hoãn.
**Thứ tự:** Đây là **Project B** của mạch Explore, tiếp nối Project A (PR #60, đã merge). Gộp hai phần người dùng muốn ship cùng nhau:
1. **Parity** — đưa Charts & Dashboards bằng Datasets (Duplicate, bulk-select).
2. **Folders** — folder per-type, phẳng 1 cấp cho datasets/charts/dashboards (phần Project A chuyển tiếp cho "Project B": cần bảng + routes backend).

## Vấn đề

Sau Project A ba trang Explore đã đồng bộ về **giao diện**, nhưng **năng lực** vẫn lệch và chưa có cách tổ chức list khi nhiều item:
- **Datasets** là mốc tham chiếu: Edit / Refresh / Duplicate (virtual) / Delete + **Bulk select** (chọn nhiều → Delete N qua `AlertDialog`).
- **Charts** chỉ có Open / Delete — **thiếu Duplicate** (chart mutations chỉ create/update/remove, không có endpoint duplicate) và **thiếu bulk select**.
- **Dashboards** có Open / Duplicate / Move to Trash (+ Restore / Delete permanently) — đã có backend `POST /dashboards/{id}/duplicate`, nhưng **thiếu bulk select**.
- Cả ba **chưa tổ chức được vào folder**.

## Mục tiêu

1. **Parity:** Charts thêm Duplicate; Charts & Dashboards thêm bulk-select. Datasets chuyển sang dùng bulk-select chung, không đổi hành vi.
2. **Folders:** folder per-type, phẳng 1 cấp; hiển thị dạng **section gom nhóm collapse được**; move-to-folder qua kebab, qua bulk, và qua kéo-thả.

## Non-Goals

- Không folder lồng nhau / không Collection hợp nhất kiểu Metabase. Folder là **per-type, phẳng**.
- Không folder per-user — folder **dùng chung workspace**, quyền `<resource>:manage`.
- Không responsive overhaul, không dọn seed-data trong code.
- Chart Duplicate dùng **endpoint backend thật** (giống dashboard), không copy client-side như dataset (để né `chart_name_uniq` an toàn).

## Quyết định thiết kế (tóm tắt — chi tiết xem bản EN)

- **A. Schema:** một bảng `folders(id, workspace_id, entity_type, name, created_by, created_at, UNIQUE(workspace_id,entity_type,name))` (theo pattern `bookmarks`) + cột `folder_id uuid NULL REFERENCES folders(id) ON DELETE SET NULL` trên charts/dashboards/datasets. Xóa folder → item về Uncategorized (SET NULL). Chống gán chéo loại/chéo workspace ở tầng API. Nhớ **touch `egret-metastore/src/lib.rs`** để re-embed migration.
- **B. Chart Duplicate:** `POST /api/ws/{ws}/charts/{id}/duplicate`, mirror `db_duplicate` của dashboard; đặt tên "… (copy N)" né unique; copy cả `folder_id`. FE thêm `duplicate` vào `useChartMutations` + item menu.
- **C. Folder API (song song mỗi type):** `GET/POST …/{type}/folders`, `PATCH/DELETE …/folders/{id}`, move qua `PATCH …/{resourceId}` `{folder_id}`. Guard ownership + entity_type [[api-nested-resource-ownership-scoping]]. Summary trả thêm `folder_id`.
- **D. Bulk-select dùng chung:** tách `useBulkSelect` + `<BulkBar>` + checkbox column từ Datasets. Charts → Delete N; Dashboards → Move N to Trash; Datasets → Delete N (giữ nguyên hành vi). Có folders thì thêm **Move N to folder…**.
- **E. UI group-inline:** không sidebar; rows gom **section theo folder** (collapse/count/Uncategorized cuối, folder sort A→Z). Search/type-filter lọc xuyên folder; SortableHeader sort trong từng section. Collapse lưu localStorage. Component `<FolderSection>` + `useFolderGroups` dùng chung 3 trang. Kebab row "Move to folder ▸" (+ "New folder…"); kebab header section Rename/Delete.
- **F. Drag-drop:** kéo row vào section header (dnd-kit) — **task riêng cùng PR**, kebab vẫn là đường chính (a11y).

## Tương tác / biên

- Dashboards trash: item trashed ngoài folder-view, giữ `folder_id` để restore đúng folder.
- Charts không cần "Edit" riêng — "Open" đã vào editor (= parity với Edit của dataset).
- Bulk + group collapse: select-all chọn mọi row đã load; collapse không bỏ chọn.
- Folder rỗng: hiện `(0)`, mở được để làm đích drop khi không search/filter.

## Test

- **Backend:** folders CRUD; SET-NULL khi xóa folder; reject move chéo type/workspace; chart duplicate + name-collision.
- **Frontend (Vitest):** `useBulkSelect`; `<FolderSection>`; Chart Duplicate; move đơn + bulk-move; bulk Delete/Trash.
- **Sweep:** frontend `typecheck/lint/build/vitest`; backend `cargo fmt --check` + `clippy -D warnings` + `cargo test` [[egret-cargo-fmt-ci]].
- **Live (Playwright):** duplicate chart; bulk delete/trash; tạo/đổi tên/xóa folder; move qua kebab/bulk/drag-drop; collapse giữ qua reload; 0 lỗi console mới.

## Thứ tự task (1 PR, green-per-commit)

BE: (1) migration folders + folder_id + touch metastore → (2) repo + CRUD routes + folder_id vào summary → (3) move endpoint + guard → (4) chart duplicate endpoint (+ helper `unique_copy_name`).
FE: (5) tách `useBulkSelect`/`<BulkBar>`, refactor Datasets (không đổi hành vi) → (6) Chart Duplicate UI → (7) bulk Delete (Charts) + bulk Trash (Dashboards) → (8) `useFolderGroups` + `<FolderSection>` group-inline cả 3 trang → (9) move-to-folder kebab + bulk "Move N to folder…" → (10) drag-drop (commit riêng) → (11) full sweep + live verify + finish branch.
