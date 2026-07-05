# Tái cấu trúc UI Cấu hình Engine Trino — Thiết kế

> Bản tiếng Việt tham chiếu. Bản chuẩn (canonical) tiếng Anh:
> `docs/en/specs/2026-07-05-trino-engine-config-ui-remap-design.md`

- **Trạng thái:** Đã duyệt (thiết kế)
- **Ngày:** 2026-07-05
- **Phạm vi:** frontend control-plane (`frontend/src/routes/control-plane/**`) + `egret-orchestrator`

## 1. Động cơ

Phản hồi review về UI **Cấu hình** engine nêu 4 vấn đề:

1. Nút "Add property" chưa hoàn chỉnh — chỉ là một ô lọc luôn mở, tìm chung cả ba
   file thuộc tính, và mọi key tự nhập đều rơi vào `config.properties`.
2. `config.properties` / `node.properties` / `log.properties` được backend render
   thành ba file riêng biệt, nhưng UI gộp chung một danh sách. Cần tách riêng.
3. Không có chỗ cấu hình **image** — image bị hardcode
   `trinodb/trino:{image_version}`, nên UI luôn dùng image mặc định.
4. Tổ chức thông tin (IA) của tab Cấu hình nên sắp lại theo cấu trúc một file Helm
   `values.yaml` chuẩn của Trino.

## 2. Hiện trạng

### Backend
- `egret-orchestrator/src/trino.rs` render **manifest Kubernetes thuần** (không có
  Helm chart / Stackable CRD trong repo) — "values.yaml" chỉ là *mô hình để sắp
  xếp IA*.
- Ba file `config/node/log.properties` là ba entry ConfigMap tách biệt.
- Image bị hardcode `format!("trinodb/trino:{}", spec.image_version)`
  (`trino.rs:573`); chỉ có *tag* (`image_version`), không cấu hình được repository.
- `imagePullPolicy` và `imagePullSecrets` đã có sẵn trong `TrinoConfig`.

### Frontend
- Nav tab Cấu hình = 5 mục phẳng: **Properties · Resources · Workload ·
  Scheduling · Scaling**. Tab riêng: **Security & Exposure**.
- `PropertiesSection.tsx` gộp ba map thuộc tính + một combobox "Add" chung.
- `trinoCatalog.ts` đã gắn `file` cho từng entry → lọc theo file là đủ.

## 3. Tham chiếu: các key cấp cao của Helm chart Trino chính thức

`image` · `imagePullSecrets` · `server` · `accessControl` · `resourceGroups` ·
`additional{Config,Node,Log}Properties` · `sessionProperties` ·
`eventListenerProperties` · `env`/`envFrom` · `initContainers`/`sidecarContainers`
· `securityContext` · `containerSecurityContext` · `shareProcessNamespace` ·
`service` · `auth` · `serviceAccount` · `configMounts`/`secretMounts` ·
`coordinator` · `worker` · `ingress`.

## 4. Thiết kế mục tiêu

### 4.1 Nav Cấu hình — mỗi mục ứng một key cấp cao của chart

| # | Mục nav | Nguồn (chart) | Trường egret |
|---|---------|---------------|--------------|
| 1 | **Image** *(mới)* | `image`, `imagePullSecrets` | chuỗi image (ghi đè `trinodb/trino:{version}`), pull policy, pull secrets |
| 2 | **Server** | `server`, `additional*Properties`, `eventListenerProperties` | Query max memory · Log level · **config.properties** · **node.properties** · **log.properties** · Exchange manager (fault-tolerant) · Event listener |
| 3 | **Coordinator** | `coordinator` | replicas, JVM/heap, resources, node config, JVM thêm, probes, cổng thêm, **đặt chỗ pod** (nodeSelector/tolerations/affinity/topology) |
| 4 | **Worker** | `worker` | như Coordinator + graceful shutdown |
| 5 | **Workload** | `accessControl`, `resourceGroups`, `sessionProperties` | Resource groups · Session rules |
| 6 | **Pod & Containers** | `env`, `securityContext`, `serviceAccount`, `initContainers`, `secretMounts`, deployment | Env, service account, security context pod/container, shareProcessNamespace, init/sidecar containers, mounts, deployment strategy/annotations/… |
| 7 | **Autoscaling** | `server.autoscaling`, `server.keda` | HPA / KEDA (giữ nguyên) |
| — | *Security & Exposure* (tab riêng) | `auth`, `ingress` | giữ nguyên |

**Khác biệt so với hiện tại (5 → 7 mục):** `Properties`→**Server** (gộp query max
memory + log level + 3 danh sách tách + Exchange + Event listener); `Resources`
tách thành **Coordinator**+**Worker** (kéo Placement từ Scheduling vào; env
chuyển sang Pod & Containers); `Scheduling`→**Pod & Containers**; thêm **Image**;
`Scaling`→**Autoscaling** (đổi tên).

### 4.2 Quyết định đã chốt
- **(a)** Coordinator và Worker là **hai mục nav riêng** (bám theo chart), bỏ
  toggle trong Resources.
- **(b)** Các trường cấp deployment (strategy/annotations/…) **giữ ở phạm vi
  cluster** trong Pod & Containers (đúng mô hình dữ liệu egret), không nhân đôi
  theo role.

### 4.3 Fix 1 — Bộ chọn thuộc tính
- Thay ô lọc luôn mở bằng **nút "+ Add property"** mở **popover tìm kiếm** (kiểu
  command-palette): lọc theo key + help; mỗi kết quả hiện key · group · type ·
  default.
- Mỗi danh sách (config/node/log) có bộ chọn **giới hạn theo file**
  (`trinoCatalog.filter(p => p.file === file)`).
- Key tự nhập không khớp catalog sẽ vào **đúng file của mục đó** (không phải luôn
  `config`).

### 4.4 Fix 2 — Tách config/node/log
- Bỏ danh sách gộp trong `PropertiesSection`. Thêm `PropertyList` tái sử dụng theo
  tham số `file`, render trong **Server** thành ba mục con. Payload
  (`configProperties`/`additionalNodeProperties`/`additionalLogProperties`) giữ
  y nguyên byte.

### 4.5 Fix 3 — Cấu hình Image
- **Backend:** thêm `image: Option<String>` vào `TrinoConfig`; `trino_deployment`
  render `image` nếu có, ngược lại fallback `trinodb/trino:{image_version}`.
- **Frontend:** thêm `image?: string` vào type + hook; mục **Image** gồm ô nhập
  chuỗi image (placeholder hiện mặc định), select pull policy, danh sách pull
  secrets (chuyển từ Pod sang).

### 4.6 sectionCounts
Chia lại theo key nav mới (`image`, `server`, `coordinator`, `worker`,
`workload`, `pod`, `autoscaling`); không đếm trùng nhóm trường; cập nhật
`sectionCounts.test.ts`.

## 5. Thành phần chạm tới
Xem mục 5 bản tiếng Anh (danh sách file backend + frontend + test).

## 6. Ngoài phạm vi
Wizard tạo engine giữ nguyên · không thêm entry vào `trinoCatalog` · manifest
render không đổi trừ image · tab Security & Exposure giữ nguyên · không cần
migration (image là trường optional mới).

## 7. Rủi ro
Chia lại nav là bề mặt lớn nhất — không được làm rớt/đếm trùng trường;
`sectionCounts` + diff review-changes (`builtTrinoConfig`) là chốt chặn (payload
phải giữ nguyên byte trừ key `image`). Effect prefill/dirty phải bao gồm `image`.

## 8. Chấp nhận
Nav 7 mục theo thứ tự chart; mọi trường cũ vẫn truy cập & lưu y hệt; mỗi
config/node/log có bộ chọn riêng theo file; đặt Image render `image:<value>`,
xóa thì fallback; lint/typecheck/test/build frontend + fmt/clippy/test backend
xanh cục bộ trước khi merge.
