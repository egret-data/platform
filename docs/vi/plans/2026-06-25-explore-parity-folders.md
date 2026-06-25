# Explore Parity + Folders — Kế hoạch triển khai (Project B) — tham chiếu VN

> Bản chuẩn (đầy đủ code từng bước) là tiếng Anh: `docs/en/plans/2026-06-25-explore-parity-folders.md`.
> File này chỉ liệt kê task để theo dõi; chi tiết code/test xem bản EN.

**Mục tiêu:** Đưa Charts & Dashboards bằng Datasets (Chart Duplicate + bulk-select) và thêm folder per-type phẳng cho cả ba trang Explore (move qua kebab/bulk/kéo-thả). Backend trước, mỗi task xanh.

**Spec:** `docs/en/specs/2026-06-25-explore-parity-folders-design.md`
**Branch:** `feat/explore-parity-folders`.
**Lưu ý:** sau khi thêm migration nhớ **touch `crates/egret-metastore/src/lib.rs`** ([[egret-sqlx-migration-staleness]]); backend chạy `cargo fmt --check` ([[egret-cargo-fmt-ci]]) + clippy + test trước mỗi commit.

## Danh sách task

**Backend:**
1. Migration `0021_folders.sql` (bảng `folders` + cột `folder_id` cho charts/dashboards/datasets) + touch metastore.
2. `FolderRepo` (`crates/egret-bi/src/folder.rs`) + wire vào `BiState` + test repo.
3. Thêm `folder_id` vào struct/COLS/DTO của chart/dashboard/dataset + `set_folder` mỗi repo.
4. Folder CRUD router generic theo `{kind}` + handler + per-resource move `…/{id}/folder` (guard cross-tenant/cross-type).
5. Chart duplicate endpoint (`next_copy_name` né `chart_name_uniq`) + unit test helper.

**Frontend:**
6. `api/folders.ts` (`useFolders`/`useFolderMutations`/`useSetFolder`) + `duplicate` chart + `folder_id` vào type summary.
7. Hook `useBulkSelect` + test.
8. `BulkBar`/`BulkToggle`; refactor Datasets sang hook chung (không đổi hành vi).
9. Charts: Duplicate UI + bulk Delete.
10. Dashboards: bulk Move to Trash.
11. Helper `groupByFolder` + `useCollapsed` + test.
12. `FolderHeaderRow` + `MoveToFolderItems` + test.
13. Charts: render group-inline + move (kebab/bulk/drag-drop) — bản mẫu.
14. Áp group-inline + move cho Dashboards (gate `!showTrash`) và Datasets.
15. Full sweep (BE + FE) + live verify Playwright + finish branch.

## Ghi chú
- Drag-drop dùng HTML5 DnD gốc (không thêm dnd-kit) — nhẹ hơn, vẫn đúng tinh thần spec.
- Trash view của Dashboards giữ list phẳng (không folder); `folder_id` được giữ để restore đúng folder.
