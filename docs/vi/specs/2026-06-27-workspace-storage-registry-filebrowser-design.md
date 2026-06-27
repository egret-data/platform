# Workspace Storage — registry (N storage) + file browser — Thiết kế (tham chiếu VN)

> Bản chuẩn: `docs/en/specs/2026-06-27-workspace-storage-registry-filebrowser-design.md`. Lệch thì theo bản tiếng Anh.

**Ngày:** 2026-06-27 · **Branch:** `feat/workspace-storage-registry` (off main). Re-architect mô hình single-storage của PR #73.

## Mục tiêu & re-architect
Tách rời storage khỏi catalog (theo mô hình lakekeeper):
- **Storage** = **registry N kết nối storage/workspace** (mỗi cái = 1 S3/MinIO bucket, có name + purpose) **+ file browser** kiểu MinIO console (duyệt/xem/upload/download/xoá object).
- **Catalog** (epic SAU) = tạo warehouse **chọn 1 storage đã đăng ký** (lakekeeper `create_warehouse`, N/ws).

**Epic này chỉ làm Storage area.** Catalog vẫn dùng model auto-provision cũ (`workspace_storage` 1-per-ws) NGUYÊN VẸN tới epic Catalog (lúc đó mới rewire catalog dùng registry + bỏ model cũ). Coexist tạm thời.

## Quyết định
- Bảng MỚI `workspace_storages` (N/ws, id PK riêng) — additive; `workspace_storage` cũ giữ nguyên cho catalog.
- Secret qua egret-secrets (mỗi conn 1 secret_ref); không trả/log.
- Mọi route Storage gate `workspace:manage_members`.
- File browser ĐẦY ĐỦ: list (duyệt folder qua prefix+delimiter), download, upload, delete — confine trong bucket + `key_prefix` root của conn.
- S3 only (provider chừa GCS/Azure). Đăng ký conn **validate reachability** (gọi S3 thật) trước khi lưu.

## Backend
1. **Migration 0030** `workspace_storages(id PK, workspace_id FK CASCADE, name, purpose, provider, endpoint, region, bucket, key_prefix, access_key, secret_ref, path_style, created_at, UNIQUE(workspace_id,name))`. Touch metastore lib.
2. **Store** (egret-catalog 0.8): `WorkspaceStorageConn` (không secret) + CRUD `create/list/get/update/delete_storage_conn` — đều scope theo workspace (IDOR-safe).
3. **`ObjectStore::list`** (egret-storage): thêm `list(prefix, delimiter) -> Listing{folders, objects:[{key,size,last_modified}]}` qua `aws_sdk_s3 list_objects_v2` (paginate).
4. **Routes** (browse.rs/storage_admin, gate admin, build `S3Store` từ conn):
   - Registry: `GET/POST/PUT/DELETE /ws/{ws}/storages[/{id}]` (POST/PUT validate reachability → 400 nếu sai; secret→egret-secrets; secret_key optional=giữ).
   - File browser (scope theo conn): `GET .../objects?prefix=` (list folder+file), `GET .../objects/content?key=` (download, cap size), `POST .../objects?key=` (upload), `DELETE .../objects?key=`. **Path-safety:** chặn `..` / thoát khỏi `key_prefix` root; effective key = `<conn.key_prefix>/<user path>`.

## Frontend (`/explore/:ws/storage` — thay page single-storage cũ)
Storage AREA 2 view:
- **Connections** (registry): bảng conn (name/purpose/provider/bucket/endpoint) + Add (form, secret write-only, server validate) + Edit + Delete. 403 → notice. Chọn conn → mở file browser.
- **File browser**: breadcrumb prefix; list folder (click vào) + file (size/modified) + Upload/Download/Delete + back; empty state; 403.
- Hook `src/api/storage.ts`: `useStorageConns/Create/Update/Delete`, `useStorageObjects/UploadObject/DeleteObject` (paths không `/api` prefix).

## Test
BE (DB+MinIO): registry CRUD (create validate; bad creds 400; scope theo ws; cross-ws id 404 IDOR; non-admin 403; không secret); file browser (upload→list→download→delete; `..`/thoát root → 400; folder qua delimiter); `ObjectStore::list` test MinIO. FE (mock): connections list/add/edit/delete (không secret); file browser list/navigate/upload/download/delete; 403.

## Rủi ro
Secret hygiene N conn; **path traversal** (confine root, chặn `..`); object lớn (cap/stream); validate reachability (gọi S3 thật, sai → 400); coexist với catalog cũ (epic này KHÔNG đụng catalog provisioning).

## Ngoài phạm vi
Epic Catalog (warehouse-on-storage + bỏ auto-provision + bảng cũ); GCS/Azure; multipart/resumable upload; versioning; wiring per-purpose.
