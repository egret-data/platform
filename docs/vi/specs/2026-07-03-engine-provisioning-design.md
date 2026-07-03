# Provision engine (Phase 1: Trino trên Kubernetes) — thiết kế (2026-07-03)

Bản tiếng Anh (canonical): `docs/en/specs/2026-07-03-engine-provisioning-design.md`.

**Mục tiêu:** Platform admin **provision / cấu hình / quản lý query engine ngay
trên UI egret**, egret chạy trên Kubernetes. Phase 1 làm trọn vertical slice cho
**Trino** (qua **Stackable** operator): tạo cluster, **tự wire vào embedded
catalog của egret**, xem status live, sửa config (scale / resources / catalogs),
xoá. Phase sau: StarRocks, RisingWave (cùng trait `EngineProvisioner`).

## Quyết định nền tảng
- **Cơ chế:** egret tạo **Custom Resource**; **operator cài sẵn** (điều kiện
  platform). Trino = Stackable (`TrinoCluster` + `TrinoCatalog`). egret KHÔNG cài
  operator/CRD ở Phase 1 → RBAC hẹp (chỉ tạo CR trong namespace).
- **Tenancy:** shared platform-wide (như warehouse/storage).
- **Phase 1 gồm:** provision + auto-wire + status + delete + **scale + config editing**.

## Kiến trúc
- Crate mới **`egret-orchestrator`**: giữ `kube::Client` + trait
  `EngineProvisioner { provision, status, update, delete }` (Phase 1: `TrinoProvisioner`).
  Có `KubeClient` trait + fake impl để test không cần cluster.
- **Provision Trino = apply 3 resource:** `Secret` (creds Catalog Client) +
  `TrinoCatalog` (Iceberg REST trỏ egret + OAuth2) + `TrinoCluster` (coordinator +
  N workers).
- **Auto-wire:** egret tự tạo Catalog Client cho warehouse đã chọn → nhét creds
  vào TrinoCatalog → Trino tự nối catalog egret khi chạy (đúng config Connect tab).
  Auto-wire fail → rollback CR.

## Data model
Bảng `engines` (control-plane): `id, name, type, namespace, warehouse_id, spec(jsonb),
status, created_at`. Row = intent; health đọc live từ CR + pods.

## Backend / Frontend / Test
- `egret-controlplane`: router `/control-plane/engines` (list/create/get/update/delete),
  `EngineProvisioner` trong CpState (như `WarehouseProvisioner`).
- Frontend: fill stub `ControlPlaneEngines.tsx` → types Trino/StarRocks/RisingWave,
  wizard (name·warehouse·size·resources·catalogs), trang detail (status·config·delete).
- Test: unit qua fake `KubeClient`; e2e opt-in `just k8s-up` (kind + Stackable operator, không CI).

## Prereq
Operator Stackable cài sẵn; egret ServiceAccount có RBAC create/get/patch/delete
`TrinoCluster`+`TrinoCatalog`+`Secret` + read `Pods` trong namespace `egret-engines`
(default, config được).

Chi tiết đầy đủ xem bản EN.
